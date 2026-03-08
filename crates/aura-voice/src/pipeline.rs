use std::sync::Arc;

use anyhow::{Context, Result};
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;

use crate::audio::{AudioCapture, SAMPLE_RATE};
use crate::stt::{SpeechToText, SttConfig};
use crate::vad::{VadConfig, VadState, VoiceActivityDetector};

#[derive(Debug, Clone)]
pub enum VoiceEvent {
    ListeningStarted,
    Transcription { text: String },
    ListeningStopped,
    Error { message: String },
}

/// Run the voice capture → VAD → STT pipeline as a long-lived async task.
///
/// Audio is captured from the microphone via cpal (which uses std::sync::mpsc).
/// A bridge thread forwards chunks to a tokio::sync::mpsc channel.
/// VAD detects speech boundaries, and STT transcribes the accumulated audio.
/// Results are published via the broadcast sender.
pub async fn run_voice_task(
    stt_config: SttConfig,
    vad_config: VadConfig,
    event_tx: broadcast::Sender<VoiceEvent>,
    cancel: CancellationToken,
) -> Result<()> {
    // Initialize STT (heavy — load whisper model)
    let stt = Arc::new(
        tokio::task::spawn_blocking(move || SpeechToText::new(stt_config))
            .await
            .context("STT init panicked")?
            .context("Failed to initialize STT")?,
    );

    // Set up audio capture
    let capture = AudioCapture::new(None).context("Failed to open microphone")?;

    // Bridge: cpal's std::sync::mpsc -> tokio::sync::mpsc
    let (std_tx, std_rx) = std::sync::mpsc::channel::<Vec<f32>>();
    let (tok_tx, mut tok_rx) = tokio::sync::mpsc::channel::<Vec<f32>>(64);

    // Start cpal stream (must keep `_stream` alive)
    let _stream = capture.start(std_tx).context("Failed to start audio stream")?;

    // Bridge thread: forward from std channel to tokio channel
    let bridge_cancel = cancel.clone();
    std::thread::spawn(move || {
        while !bridge_cancel.is_cancelled() {
            match std_rx.recv_timeout(std::time::Duration::from_millis(100)) {
                Ok(chunk) => {
                    if tok_tx.blocking_send(chunk).is_err() {
                        break;
                    }
                }
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
            }
        }
    });

    tracing::info!("Voice pipeline running");

    let mut vad = VoiceActivityDetector::new(vad_config)
        .context("Failed to initialize VAD")?;
    let mut audio_buffer: Vec<f32> = Vec::new();
    let mut was_speaking = false;

    loop {
        tokio::select! {
            _ = cancel.cancelled() => {
                tracing::info!("Voice pipeline shutting down");
                break;
            }
            chunk = tok_rx.recv() => {
                let Some(chunk) = chunk else { break };

                let state = vad.process(&chunk);

                match state {
                    VadState::Speaking => {
                        if !was_speaking {
                            tracing::info!("Speech detected — listening");
                            let _ = event_tx.send(VoiceEvent::ListeningStarted);
                            audio_buffer.clear();
                            was_speaking = true;
                        }
                        audio_buffer.extend_from_slice(&chunk);
                    }
                    VadState::Silent => {
                        if was_speaking {
                            was_speaking = false;
                            let _ = event_tx.send(VoiceEvent::ListeningStopped);

                            // Skip very short audio (< 0.5s at 16kHz)
                            let min_samples = SAMPLE_RATE as usize / 2;
                            if audio_buffer.len() < min_samples {
                                tracing::info!(
                                    samples = audio_buffer.len(),
                                    min = min_samples,
                                    "Audio too short, skipping transcription"
                                );
                                audio_buffer.clear();
                                vad.reset();
                                continue;
                            }

                            tracing::info!(
                                samples = audio_buffer.len(),
                                duration_ms = audio_buffer.len() as u64 * 1000 / SAMPLE_RATE as u64,
                                "Speech ended — transcribing"
                            );

                            // Transcribe in background
                            let stt = Arc::clone(&stt);
                            let audio = std::mem::take(&mut audio_buffer);
                            let tx = event_tx.clone();

                            tokio::task::spawn_blocking(move || {
                                match stt.transcribe(&audio) {
                                    Ok(text) if !text.is_empty() => {
                                        tracing::info!(text = %text, "Transcription complete");
                                        let _ = tx.send(VoiceEvent::Transcription { text });
                                    }
                                    Ok(_) => {
                                        tracing::debug!("Empty transcription, ignoring");
                                    }
                                    Err(e) => {
                                        tracing::error!("STT error: {e}");
                                        let _ = tx.send(VoiceEvent::Error {
                                            message: format!("STT failed: {e}"),
                                        });
                                    }
                                }
                            });

                            vad.reset();
                        }
                    }
                }
            }
        }
    }

    Ok(())
}
