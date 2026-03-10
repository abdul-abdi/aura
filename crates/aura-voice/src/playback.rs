use anyhow::{Context, Result};
use rodio::{OutputStream, Sink};
use std::sync::mpsc;

pub enum PlaybackCommand {
    /// Create a new Sink for streaming audio at the given sample rate.
    StartStream { sample_rate: u32 },
    /// Append a chunk of f32 PCM samples to the current stream.
    Append { samples: Vec<f32> },
    /// Stop playback and drop the current sink.
    Stop,
    /// Query whether audio is currently playing.
    IsPlaying(mpsc::Sender<bool>),
}

/// Thread-safe handle to the audio player running on a dedicated thread.
/// rodio's OutputStream is !Send, so all playback happens on a background thread.
#[derive(Clone)]
pub struct AudioPlayer {
    tx: mpsc::Sender<PlaybackCommand>,
}

impl AudioPlayer {
    /// Spawn a dedicated playback thread and return a handle.
    pub fn new() -> Result<Self> {
        let (tx, rx) = mpsc::channel::<PlaybackCommand>();

        std::thread::Builder::new()
            .name("aura-playback".into())
            .spawn(move || {
                let Ok((stream, handle)) = OutputStream::try_default() else {
                    tracing::error!("Failed to open audio output device");
                    return;
                };
                let _stream = stream; // keep alive

                // Current sink and its sample rate, kept together so Append
                // knows the rate without the caller resending it each time.
                let mut current: Option<(Sink, u32)> = None;

                while let Ok(cmd) = rx.recv() {
                    match cmd {
                        PlaybackCommand::StartStream { sample_rate } => {
                            // Tear down any previous stream
                            if let Some((sink, _)) = current.take() {
                                sink.stop();
                            }

                            match Sink::try_new(&handle) {
                                Ok(sink) => {
                                    current = Some((sink, sample_rate));
                                    tracing::debug!(sample_rate, "Audio stream started");
                                }
                                Err(e) => {
                                    tracing::error!("Failed to create audio sink: {e}");
                                }
                            }
                        }
                        PlaybackCommand::Append { samples } => {
                            // Auto-create a stream if none exists (e.g. after barge-in)
                            if current.is_none() {
                                let default_rate = 24_000;
                                match Sink::try_new(&handle) {
                                    Ok(sink) => {
                                        tracing::debug!(sample_rate = default_rate, "Auto-starting audio stream");
                                        current = Some((sink, default_rate));
                                    }
                                    Err(e) => {
                                        tracing::error!("Failed to create audio sink: {e}");
                                    }
                                }
                            }

                            if let Some((ref sink, sample_rate)) = current {
                                let source = rodio::buffer::SamplesBuffer::new(
                                    1,
                                    sample_rate,
                                    samples,
                                );
                                sink.append(source);
                            }
                        }
                        PlaybackCommand::Stop => {
                            if let Some((sink, _)) = current.take() {
                                sink.stop();
                            }
                        }
                        PlaybackCommand::IsPlaying(reply) => {
                            let playing = current
                                .as_ref()
                                .map(|(sink, _)| !sink.empty())
                                .unwrap_or(false);
                            let _ = reply.send(playing);
                        }
                    }
                }
            })
            .context("Failed to spawn playback thread")?;

        Ok(Self { tx })
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
        let _ = self.tx.send(PlaybackCommand::Stop);
    }

    /// Check if audio is currently playing.
    pub fn is_playing(&self) -> bool {
        let (reply_tx, reply_rx) = mpsc::channel();
        if self.tx.send(PlaybackCommand::IsPlaying(reply_tx)).is_ok() {
            reply_rx
                .recv_timeout(std::time::Duration::from_millis(100))
                .unwrap_or(false)
        } else {
            false
        }
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

        // Give the thread time to process (it should log a warning, not crash)
        std::thread::sleep(std::time::Duration::from_millis(50));

        // The player should still be functional
        assert!(!player.is_playing());
    }

    #[test]
    fn stop_without_stream_is_noop() {
        let player = AudioPlayer::new().expect("AudioPlayer::new should succeed");
        // Should not panic or error
        player.stop();
        assert!(!player.is_playing());
    }
}
