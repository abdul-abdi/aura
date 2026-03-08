use anyhow::{Context, Result};
use rodio::{OutputStream, Sink};
use std::sync::mpsc;

pub enum PlaybackCommand {
    Play { samples: Vec<f32>, sample_rate: u32 },
    Stop,
    IsPlaying(std::sync::mpsc::Sender<bool>),
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

                let mut current_sink: Option<Sink> = None;

                while let Ok(cmd) = rx.recv() {
                    match cmd {
                        PlaybackCommand::Play {
                            samples,
                            sample_rate,
                        } => {
                            // Stop current playback
                            if let Some(sink) = current_sink.take() {
                                sink.stop();
                            }

                            match Sink::try_new(&handle) {
                                Ok(sink) => {
                                    let source =
                                        rodio::buffer::SamplesBuffer::new(1, sample_rate, samples);
                                    sink.append(source);
                                    current_sink = Some(sink);
                                }
                                Err(e) => {
                                    tracing::error!("Failed to create audio sink: {e}");
                                }
                            }
                        }
                        PlaybackCommand::Stop => {
                            if let Some(sink) = current_sink.take() {
                                sink.stop();
                            }
                        }
                        PlaybackCommand::IsPlaying(reply) => {
                            let playing = current_sink
                                .as_ref()
                                .map(|s| !s.empty())
                                .unwrap_or(false);
                            let _ = reply.send(playing);
                        }
                    }
                }
            })
            .context("Failed to spawn playback thread")?;

        Ok(Self { tx })
    }

    /// Play f32 PCM audio at the given sample rate.
    pub fn play(&self, samples: Vec<f32>, sample_rate: u32) -> Result<()> {
        self.tx
            .send(PlaybackCommand::Play {
                samples,
                sample_rate,
            })
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
            reply_rx.recv_timeout(std::time::Duration::from_millis(100)).unwrap_or(false)
        } else {
            false
        }
    }
}
