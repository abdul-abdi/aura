use anyhow::{Context, Result};
use rodio::{OutputStream, Sink};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;

/// Default output sample rate from Gemini audio responses.
const DEFAULT_OUTPUT_SAMPLE_RATE: u32 = 24_000;

pub enum PlaybackCommand {
    /// Create a new Sink for streaming audio at the given sample rate.
    StartStream { sample_rate: u32 },
    /// Append a chunk of f32 PCM samples to the current stream.
    Append { samples: Vec<f32> },
    /// Stop playback and drop the current sink.
    Stop,
}

/// Pre-buffer for absorbing network jitter before starting playback.
/// Buffers ~80ms of audio samples before creating and starting the Sink.
struct PreBuffer {
    samples: Vec<f32>,
    sample_rate: u32,
    target_samples: usize,
}

/// Thread-safe handle to the audio player running on a dedicated thread.
/// rodio's OutputStream is !Send, so all playback happens on a background thread.
#[derive(Clone)]
pub struct AudioPlayer {
    tx: mpsc::Sender<PlaybackCommand>,
    playing: Arc<AtomicBool>,
}

impl AudioPlayer {
    /// Spawn a dedicated playback thread and return a handle.
    pub fn new() -> Result<Self> {
        let (tx, rx) = mpsc::channel::<PlaybackCommand>();
        let (startup_tx, startup_rx) = std::sync::mpsc::channel::<Result<()>>();
        let playing = Arc::new(AtomicBool::new(false));
        let playing_flag = Arc::clone(&playing);

        std::thread::Builder::new()
            .name("aura-playback".into())
            .spawn(move || {
                let Ok((stream, handle)) = OutputStream::try_default() else {
                    tracing::error!("Failed to open audio output device");
                    let _ = startup_tx.send(Err(anyhow::anyhow!("Failed to open audio output device")));
                    return;
                };
                let _stream = stream; // keep alive
                let _ = startup_tx.send(Ok(()));

                // Current sink and its sample rate, kept together so Append
                // knows the rate without the caller resending it each time.
                let mut current: Option<(Sink, u32)> = None;
                // Pre-buffer to absorb network jitter before starting playback
                let mut pre_buffer: Option<PreBuffer> = None;

                loop {
                    // Use recv_timeout so we can periodically check if the sink
                    // has naturally drained (all queued audio finished playing).
                    // Without this, playing_flag stays true forever after playback ends.
                    let cmd = if current.is_some() {
                        match rx.recv_timeout(std::time::Duration::from_millis(100)) {
                            Ok(cmd) => cmd,
                            Err(mpsc::RecvTimeoutError::Timeout) => {
                                // Check if sink has naturally drained
                                if let Some((ref sink, _)) = current
                                    && sink.empty()
                                {
                                    playing_flag.store(false, Ordering::Release);
                                }
                                continue;
                            }
                            Err(mpsc::RecvTimeoutError::Disconnected) => break,
                        }
                    } else {
                        match rx.recv() {
                            Ok(cmd) => cmd,
                            Err(_) => break,
                        }
                    };
                    match cmd {
                        PlaybackCommand::StartStream { sample_rate } => {
                            // Tear down any previous stream
                            if let Some((sink, _)) = current.take() {
                                sink.stop();
                            }
                            playing_flag.store(false, Ordering::Release);

                            // Warn if we're dropping a partially filled pre-buffer
                            if let Some(ref existing) = pre_buffer {
                                tracing::warn!(
                                    samples = existing.samples.len(),
                                    "Dropping partially filled pre-buffer ({} samples) due to new stream",
                                    existing.samples.len()
                                );
                            }

                            // Enter buffering mode: accumulate ~40ms before starting Sink.
                            // Gemini's native audio model delivers chunks at a steady rate,
                            // so 40ms is sufficient jitter absorption with lower latency.
                            let target_samples = (sample_rate as usize * 40) / 1000;
                            pre_buffer = Some(PreBuffer {
                                samples: Vec::with_capacity(target_samples),
                                sample_rate,
                                target_samples,
                            });
                            tracing::debug!(sample_rate, target_samples, "Buffering audio stream");
                        }
                        PlaybackCommand::Append { samples } => {
                            // If we're buffering, accumulate in pre-buffer first
                            if let Some(ref mut buf) = pre_buffer {
                                buf.samples.extend_from_slice(&samples);
                                // Once we have enough pre-buffered audio, flush to Sink
                                if buf.samples.len() >= buf.target_samples {
                                    match Sink::try_new(&handle) {
                                        Ok(sink) => {
                                            let samples =
                                                std::mem::take(&mut buf.samples);
                                            let source = rodio::buffer::SamplesBuffer::new(
                                                1,
                                                buf.sample_rate,
                                                samples,
                                            );
                                            sink.append(source);
                                            current = Some((sink, buf.sample_rate));
                                            playing_flag.store(true, Ordering::Release);
                                            tracing::debug!(
                                                "Audio pre-buffer flushed, playback started"
                                            );
                                        }
                                        Err(e) => {
                                            tracing::error!("Failed to create audio sink: {e}");
                                        }
                                    }
                                    pre_buffer = None;
                                }
                            } else if let Some((ref sink, sample_rate)) = current {
                                // We have an active sink, append directly
                                let source =
                                    rodio::buffer::SamplesBuffer::new(1, sample_rate, samples);
                                sink.append(source);
                            } else {
                                // Auto-create a stream if none exists (e.g. after barge-in)
                                match Sink::try_new(&handle) {
                                    Ok(sink) => {
                                        tracing::debug!(
                                            sample_rate = DEFAULT_OUTPUT_SAMPLE_RATE,
                                            "Auto-starting audio stream"
                                        );
                                        let source = rodio::buffer::SamplesBuffer::new(
                                            1,
                                            DEFAULT_OUTPUT_SAMPLE_RATE,
                                            samples,
                                        );
                                        sink.append(source);
                                        current = Some((sink, DEFAULT_OUTPUT_SAMPLE_RATE));
                                        playing_flag.store(true, Ordering::Release);
                                    }
                                    Err(e) => {
                                        tracing::error!("Failed to create audio sink: {e}");
                                    }
                                }
                            }
                        }
                        PlaybackCommand::Stop => {
                            pre_buffer = None;
                            if let Some((sink, _)) = current.take() {
                                sink.stop();
                            }
                            playing_flag.store(false, Ordering::Release);
                        }
                    }
                }
            })
            .context("Failed to spawn playback thread")?;

        // Wait for thread to confirm device opened
        startup_rx
            .recv()
            .context("Playback thread exited")?
            .context("Audio output device unavailable")?;

        Ok(Self { tx, playing })
    }

    /// Begin a new audio stream at the given sample rate.
    /// Stops any existing stream first.
    pub fn start_stream(&self, sample_rate: u32) -> Result<()> {
        self.tx
            .send(PlaybackCommand::StartStream { sample_rate })
            .map_err(|_| anyhow::anyhow!("Playback thread died"))
    }

    /// Append a chunk of f32 PCM samples to the current stream.
    pub fn append(&self, samples: Vec<f32>) -> Result<()> {
        self.tx
            .send(PlaybackCommand::Append { samples })
            .map_err(|_| anyhow::anyhow!("Playback thread died"))
    }

    /// Stop current playback (for barge-in).
    pub fn stop(&self) {
        if self.tx.send(PlaybackCommand::Stop).is_err() {
            tracing::warn!("Failed to send stop command — playback thread may have exited");
        }
    }

    /// Check if audio is currently playing.
    pub fn is_playing(&self) -> bool {
        self.playing.load(Ordering::Acquire)
    }

    /// Return a clone of the internal playing flag so callers can share it
    /// across tasks without polling `is_playing()`.
    pub fn playing_arc(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.playing)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Generate a sine-wave chunk so the sink actually has audible data to play.
    fn sine_chunk(sample_rate: u32, duration_ms: u32) -> Vec<f32> {
        let num_samples = (sample_rate * duration_ms / 1000) as usize;
        let freq = 440.0_f32;
        (0..num_samples)
            .map(|i| {
                let t = i as f32 / sample_rate as f32;
                (2.0 * std::f32::consts::PI * freq * t).sin() * 0.5
            })
            .collect()
    }

    #[test]
    fn streaming_commands_are_accepted() {
        // This test verifies the command channel works correctly:
        // start_stream, append, stop all succeed without panicking.
        // It does NOT assert is_playing() because that depends on a real
        // audio output device which may not be available in CI.
        let player = AudioPlayer::new().expect("AudioPlayer::new should succeed");

        // Start a stream at 24 kHz (Gemini output rate)
        player
            .start_stream(24_000)
            .expect("start_stream should succeed");

        // Append 3 chunks of audio
        for _ in 0..3 {
            let chunk = sine_chunk(24_000, 100);
            player.append(chunk).expect("append should succeed");
        }

        // Stop playback
        player.stop();

        // After stop, is_playing should return false (even if device was unavailable)
        std::thread::sleep(std::time::Duration::from_millis(50));
        assert!(
            !player.is_playing(),
            "Player should not be playing after stop"
        );
    }

    #[test]
    fn start_stream_replaces_previous_stream() {
        let player = AudioPlayer::new().expect("AudioPlayer::new should succeed");

        // Start first stream
        player.start_stream(24_000).expect("first start_stream");
        player
            .append(sine_chunk(24_000, 100))
            .expect("append to first stream");

        // Start second stream — should replace the first without error
        player.start_stream(16_000).expect("second start_stream");
        player
            .append(sine_chunk(16_000, 100))
            .expect("append to second stream");

        player.stop();
    }

    #[test]
    fn append_without_stream_does_not_panic() {
        let player = AudioPlayer::new().expect("AudioPlayer::new should succeed");

        // Append without calling start_stream — should log a warning, not panic
        let result = player.append(sine_chunk(24_000, 100));
        assert!(result.is_ok(), "append should not error on channel send");

        // Give the thread time to process
        std::thread::sleep(std::time::Duration::from_millis(50));

        // Auto-created sink should be playing now
        // Stop and verify it returns to not playing
        player.stop();
        std::thread::sleep(std::time::Duration::from_millis(50));
        assert!(!player.is_playing());
    }

    #[test]
    fn stop_without_stream_is_noop() {
        let player = AudioPlayer::new().expect("AudioPlayer::new should succeed");
        // Should not panic or error
        player.stop();
        assert!(!player.is_playing());
    }

    #[test]
    fn barge_in_stops_and_ignores_further_data() {
        let player = AudioPlayer::new().expect("AudioPlayer::new should succeed");

        // Start a stream and send some audio
        player.start_stream(24_000).expect("start_stream");
        player
            .append(sine_chunk(24_000, 100))
            .expect("append before barge-in");

        // Barge-in: stop playback
        player.stop();
        std::thread::sleep(std::time::Duration::from_millis(50));

        // Send more data without starting a new stream — should not panic.
        // The player auto-creates a stream, so this is fine.
        let result = player.append(sine_chunk(24_000, 100));
        assert!(result.is_ok(), "append after stop should not panic");

        // Clean up
        player.stop();
        std::thread::sleep(std::time::Duration::from_millis(50));
        assert!(!player.is_playing());
    }

    #[test]
    fn pre_buffer_flush_accepts_more_data() {
        let player = AudioPlayer::new().expect("AudioPlayer::new should succeed");

        // Start a stream at 24kHz (pre-buffer target = 24000 * 40 / 1000 = 960 samples)
        player.start_stream(24_000).expect("start_stream");

        // Send enough data to exceed the pre-buffer target (960 samples = 40ms at 24kHz)
        let chunk = sine_chunk(24_000, 100); // 2400 samples > 960 target
        player.append(chunk).expect("append to fill pre-buffer");

        // Give the thread time to flush the pre-buffer and create a sink
        std::thread::sleep(std::time::Duration::from_millis(50));

        // Send more data — should be appended to the now-active sink without panic
        for _ in 0..3 {
            player
                .append(sine_chunk(24_000, 50))
                .expect("append after pre-buffer flush");
        }

        player.stop();
    }

    #[test]
    fn stop_on_dead_thread_logs_warning() {
        // Create a player, then drop the receiver side by dropping the player's
        // internal state. We simulate this by dropping the player and recreating
        // a raw sender that points to a disconnected channel.
        let (tx, _rx) = mpsc::channel::<PlaybackCommand>();
        // Drop the receiver immediately
        drop(_rx);

        let player = AudioPlayer {
            tx,
            playing: Arc::new(AtomicBool::new(false)),
        };

        // stop() should not panic — it should log a warning via tracing
        player.stop();

        // start_stream and append should return errors since channel is dead
        // but stop() specifically uses is_err() check, not unwrap
    }
}
