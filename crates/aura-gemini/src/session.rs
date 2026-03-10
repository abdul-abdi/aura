use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use futures_util::{SinkExt, StreamExt};
use tokio::sync::{Mutex, broadcast, mpsc};
use tokio_tungstenite::tungstenite::Message;

use crate::config::GeminiConfig;
use crate::protocol::*;
use crate::tools::build_tool_declarations;

// Protocol documentation constants — not directly referenced in code but
// record the sample rates and chunk duration used by the Gemini Live API.
#[allow(dead_code)]
const AUDIO_CHUNK_DURATION_MS: u64 = 100;
#[allow(dead_code)]
const INPUT_SAMPLE_RATE: u32 = 16_000;
#[allow(dead_code)]
const OUTPUT_SAMPLE_RATE: u32 = 24_000;
const MAX_RECONNECT_ATTEMPTS: u32 = 10;
const INITIAL_BACKOFF_MS: u64 = 200;
const MAX_BACKOFF_MS: u64 = 30_000;

/// Events emitted by the Gemini session.
#[derive(Debug, Clone)]
pub enum GeminiEvent {
    Connected,
    AudioResponse {
        samples: Vec<f32>,
    },
    ToolCall {
        id: String,
        name: String,
        args: serde_json::Value,
    },
    Interrupted,
    ToolCallCancellation {
        ids: Vec<String>,
    },
    Transcription {
        text: String,
    },
    TurnComplete,
    Error {
        message: String,
    },
    Reconnecting {
        attempt: u32,
    },
    Disconnected,
}

/// Handle for interacting with a live Gemini session.
pub struct GeminiLiveSession {
    audio_tx: mpsc::Sender<Vec<f32>>,
    video_tx: mpsc::Sender<String>,
    tool_response_tx: mpsc::Sender<(String, String, serde_json::Value)>,
    text_tx: mpsc::Sender<String>,
    event_tx: broadcast::Sender<GeminiEvent>,
    cancel: tokio_util::sync::CancellationToken,
    is_first_connect: Arc<std::sync::atomic::AtomicBool>,
}

impl GeminiLiveSession {
    /// Connect to the Gemini Live API and start the streaming loop.
    /// Spawns background tasks for sending and receiving.
    pub async fn connect(config: GeminiConfig) -> Result<Self> {
        let (audio_tx, audio_rx) = mpsc::channel::<Vec<f32>>(64);
        let (video_tx, video_rx) = mpsc::channel::<String>(8);
        let (tool_response_tx, tool_response_rx) =
            mpsc::channel::<(String, String, serde_json::Value)>(16);
        let (text_tx, text_rx) = mpsc::channel::<String>(16);
        let (event_tx, _) = broadcast::channel::<GeminiEvent>(128);
        let cancel = tokio_util::sync::CancellationToken::new();
        let is_first_connect = Arc::new(std::sync::atomic::AtomicBool::new(true));

        let session_state = SessionState {
            config: Arc::new(config),
            resumption_handle: Arc::new(Mutex::new(None)),
            is_first_connect: Arc::clone(&is_first_connect),
        };

        // Spawn the connection manager task
        let state = session_state.clone();
        let tx = event_tx.clone();
        let token = cancel.clone();
        tokio::spawn(async move {
            connection_loop(state, audio_rx, video_rx, tool_response_rx, text_rx, tx, token).await;
        });

        Ok(Self {
            audio_tx,
            video_tx,
            tool_response_tx,
            text_tx,
            event_tx,
            cancel,
            is_first_connect,
        })
    }

    /// Send PCM f32 audio at 16kHz to Gemini.
    pub async fn send_audio(&self, pcm_16khz: &[f32]) -> Result<()> {
        self.audio_tx
            .send(pcm_16khz.to_vec())
            .await
            .map_err(|_| anyhow::anyhow!("Session closed"))
    }

    /// Send a JPEG screenshot frame to Gemini.
    pub async fn send_video(&self, jpeg_base64: &str) -> Result<()> {
        self.video_tx
            .send(jpeg_base64.to_string())
            .await
            .map_err(|_| anyhow::anyhow!("Session closed"))
    }

    /// Send a text message to Gemini.
    pub async fn send_text(&self, text: &str) -> Result<()> {
        self.text_tx
            .send(text.to_string())
            .await
            .map_err(|_| anyhow::anyhow!("Session closed"))
    }

    /// Send a tool/function response back to Gemini.
    pub async fn send_tool_response(
        &self,
        id: String,
        name: String,
        response: serde_json::Value,
    ) -> Result<()> {
        self.tool_response_tx
            .send((id, name, response))
            .await
            .map_err(|_| anyhow::anyhow!("Session closed"))
    }

    /// Subscribe to session events.
    pub fn subscribe(&self) -> broadcast::Receiver<GeminiEvent> {
        self.event_tx.subscribe()
    }

    /// Disconnect the session.
    pub fn disconnect(&self) {
        self.cancel.cancel();
    }

    /// Returns true if this is the first connection (not a reconnection).
    /// After the first connection, this returns false.
    pub fn is_first_connect(&self) -> bool {
        self.is_first_connect.load(std::sync::atomic::Ordering::Relaxed)
    }
}

impl Drop for GeminiLiveSession {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

#[derive(Clone)]
struct SessionState {
    config: Arc<GeminiConfig>,
    resumption_handle: Arc<Mutex<Option<String>>>,
    is_first_connect: Arc<std::sync::atomic::AtomicBool>,
}

/// Outcome of a single connection attempt.
struct StreamOutcome {
    /// Whether setupComplete was received before the error.
    was_connected: bool,
    error: anyhow::Error,
}

/// Main connection loop with reconnection logic.
async fn connection_loop(
    state: SessionState,
    mut audio_rx: mpsc::Receiver<Vec<f32>>,
    mut video_rx: mpsc::Receiver<String>,
    mut tool_response_rx: mpsc::Receiver<(String, String, serde_json::Value)>,
    mut text_rx: mpsc::Receiver<String>,
    event_tx: broadcast::Sender<GeminiEvent>,
    cancel: tokio_util::sync::CancellationToken,
) {
    let mut attempt: u32 = 0;

    loop {
        if cancel.is_cancelled() {
            break;
        }

        match connect_and_stream(
            &state,
            &mut audio_rx,
            &mut video_rx,
            &mut tool_response_rx,
            &mut text_rx,
            &event_tx,
            &cancel,
        )
        .await
        {
            Ok(()) => {
                // Clean disconnect
                let _ = event_tx.send(GeminiEvent::Disconnected);
                break;
            }
            Err(outcome) => {
                // Reset counter if we had a successful session — transient drops
                // hours apart should not accumulate toward the max.
                if outcome.was_connected {
                    attempt = 0;
                }

                attempt += 1;
                if attempt > MAX_RECONNECT_ATTEMPTS {
                    let _ = event_tx.send(GeminiEvent::Error {
                        message: format!("Max reconnection attempts exceeded: {}", outcome.error),
                    });
                    break;
                }

                tracing::warn!(attempt, error = %outcome.error, "Connection lost, reconnecting");
                let _ = event_tx.send(GeminiEvent::Reconnecting { attempt });

                let backoff = Duration::from_millis(
                    (INITIAL_BACKOFF_MS * 2u64.pow(attempt - 1)).min(MAX_BACKOFF_MS),
                );
                tokio::select! {
                    _ = cancel.cancelled() => break,
                    _ = tokio::time::sleep(backoff) => {}
                }
            }
        }
    }
}

/// Connect to WebSocket, send setup, and enter the streaming loop.
async fn connect_and_stream(
    state: &SessionState,
    audio_rx: &mut mpsc::Receiver<Vec<f32>>,
    video_rx: &mut mpsc::Receiver<String>,
    tool_response_rx: &mut mpsc::Receiver<(String, String, serde_json::Value)>,
    text_rx: &mut mpsc::Receiver<String>,
    event_tx: &broadcast::Sender<GeminiEvent>,
    cancel: &tokio_util::sync::CancellationToken,
) -> std::result::Result<(), StreamOutcome> {
    let mut was_connected = false;

    let result = connect_and_stream_inner(
        state,
        audio_rx,
        video_rx,
        tool_response_rx,
        text_rx,
        event_tx,
        cancel,
        &mut was_connected,
    )
    .await;

    match result {
        Ok(()) => Ok(()),
        Err(error) => Err(StreamOutcome {
            was_connected,
            error,
        }),
    }
}

async fn connect_and_stream_inner(
    state: &SessionState,
    audio_rx: &mut mpsc::Receiver<Vec<f32>>,
    video_rx: &mut mpsc::Receiver<String>,
    tool_response_rx: &mut mpsc::Receiver<(String, String, serde_json::Value)>,
    text_rx: &mut mpsc::Receiver<String>,
    event_tx: &broadcast::Sender<GeminiEvent>,
    cancel: &tokio_util::sync::CancellationToken,
    was_connected: &mut bool,
) -> Result<()> {
    let url = state.config.ws_url();
    tracing::info!(url = %state.config.ws_url_redacted(), "Connecting to Gemini WebSocket");
    let (ws_stream, _) = tokio::time::timeout(
        Duration::from_secs(10),
        tokio_tungstenite::connect_async(&url),
    )
    .await
    .context("WebSocket connection timed out (10s)")?
    .context("WebSocket connection failed")?;

    let (mut ws_sink, mut ws_source) = ws_stream.split();

    // Send setup message
    let resumption_handle = state.resumption_handle.lock().await.clone();
    let setup = build_setup_message(&state.config, resumption_handle);
    let setup_json = serde_json::to_string(&setup)?;
    tracing::debug!("Sending setup: {setup_json}");
    ws_sink.send(Message::Text(setup_json)).await?;

    // Wait for setupComplete with timeout
    let setup_deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    loop {
        let msg = tokio::select! {
            msg = ws_source.next() => {
                msg.context("Connection closed before setupComplete")?
                    .context("WebSocket error during setup")?
            }
            _ = tokio::time::sleep_until(setup_deadline) => {
                anyhow::bail!("Timed out waiting for setupComplete (15s). Model '{}' may not exist or may not support the Live API.", state.config.model);
            }
            _ = cancel.cancelled() => return Ok(()),
        };

        let json_text = match &msg {
            Message::Text(text) => Some(text.clone()),
            Message::Binary(bytes) => {
                String::from_utf8(bytes.clone()).ok()
            }
            Message::Close(frame) => {
                anyhow::bail!("Server closed connection during setup: {frame:?}");
            }
            _ => None,
        };

        if let Some(text) = json_text {
            tracing::debug!("Setup response: {text}");
            let server_msg: ServerMessage = serde_json::from_str(&text)?;
            if server_msg.setup_complete.is_some() {
                break;
            }
        }
    }

    *was_connected = true;
    let _ = event_tx.send(GeminiEvent::Connected);
    // Set after sending so the receiver sees true on first connect
    state.is_first_connect.store(false, std::sync::atomic::Ordering::Relaxed);
    tracing::info!("Gemini Live session connected");

    let ws_sink = Arc::new(Mutex::new(ws_sink));

    let mut ping_interval = tokio::time::interval(Duration::from_secs(20));
    ping_interval.tick().await; // skip immediate first tick

    loop {
        tokio::select! {
            biased;

            _ = cancel.cancelled() => return Ok(()),

            msg = ws_source.next() => {
                let Some(msg) = msg else {
                    return Err(anyhow::anyhow!("WebSocket connection closed"));
                };
                let msg = msg?;

                let json_text = match msg {
                    Message::Text(text) => Some(text),
                    Message::Binary(bytes) => {
                        String::from_utf8(bytes).ok()
                    }
                    Message::Close(_) => {
                        return Err(anyhow::anyhow!("Server closed connection"));
                    }
                    _ => None,
                };

                if let Some(text) = json_text {
                    match serde_json::from_str::<ServerMessage>(&text) {
                        Ok(server_msg) => {
                            if handle_server_message(server_msg, event_tx, state).await {
                                return Err(anyhow::anyhow!("goAway: server requested reconnection"));
                            }
                        }
                        Err(e) => {
                            tracing::warn!(
                                error = %e,
                                text_preview = &text[..text.len().min(200)],
                                "Failed to parse Gemini message, skipping"
                            );
                        }
                    }
                }
            }

            Some((id, name, response)) = tool_response_rx.recv() => {
                let msg = ToolResponseMessage {
                    tool_response: ToolResponse {
                        function_responses: vec![FunctionResponse { id, name, response }],
                    },
                };
                let json = serde_json::to_string(&msg)?;
                ws_sink.lock().await.send(Message::Text(json)).await?;
            }

            Some(text) = text_rx.recv() => {
                let msg = ClientContentMessage {
                    client_content: ClientContent {
                        turns: vec![Content {
                            role: Some("user".into()),
                            parts: vec![Part {
                                text: Some(text),
                                inline_data: None,
                            }],
                        }],
                        turn_complete: true,
                    },
                };
                let json = serde_json::to_string(&msg)?;
                ws_sink.lock().await.send(Message::Text(json)).await?;
            }

            _ = ping_interval.tick() => {
                ws_sink.lock().await.send(Message::Ping(vec![])).await?;
            }

            Some(jpeg_b64) = video_rx.recv() => {
                let msg = RealtimeVideoMessage {
                    realtime_input: RealtimeVideoInput {
                        video: Blob {
                            mime_type: "image/jpeg".into(),
                            data: jpeg_b64,
                        },
                    },
                };
                let json = serde_json::to_string(&msg)?;
                ws_sink.lock().await.send(Message::Text(json)).await?;
            }

            Some(pcm) = audio_rx.recv() => {
                let msg = encode_audio_message(&pcm);
                let json = serde_json::to_string(&msg)?;
                ws_sink.lock().await.send(Message::Text(json)).await?;
            }
        }
    }
}

/// Handle a server message. Returns `true` if the caller should reconnect (goAway).
async fn handle_server_message(
    msg: ServerMessage,
    event_tx: &broadcast::Sender<GeminiEvent>,
    state: &SessionState,
) -> bool {
    // Session resumption token update
    if let Some(update) = msg.session_resumption_update {
        if let Some(new_handle) = update.new_handle {
            let mut handle = state.resumption_handle.lock().await;
            *handle = Some(new_handle);
        }
        return false;
    }

    // Server content (audio response, interruption, turn complete)
    if let Some(content) = msg.server_content {
        if content.interrupted == Some(true) {
            let _ = event_tx.send(GeminiEvent::Interrupted);
            return false;
        }

        if let Some(model_turn) = content.model_turn {
            for part in model_turn.parts {
                if let Some(blob) = part.inline_data
                    && blob.mime_type.starts_with("audio/")
                {
                    match BASE64.decode(&blob.data) {
                        Ok(bytes) => {
                            let samples = pcm_bytes_to_f32(&bytes);
                            let _ = event_tx.send(GeminiEvent::AudioResponse { samples });
                        }
                        Err(e) => {
                            tracing::warn!("Failed to decode audio base64: {e}");
                        }
                    }
                }
                if let Some(text) = part.text {
                    let _ = event_tx.send(GeminiEvent::Transcription { text });
                }
            }
        }

        if content.turn_complete == Some(true) {
            let _ = event_tx.send(GeminiEvent::TurnComplete);
        }
        return false;
    }

    // Tool call
    if let Some(tool_call) = msg.tool_call {
        for fc in tool_call.function_calls {
            let _ = event_tx.send(GeminiEvent::ToolCall {
                id: fc.id,
                name: fc.name,
                args: fc.args,
            });
        }
        return false;
    }

    // Tool call cancellation
    if let Some(cancellation) = msg.tool_call_cancellation {
        let _ = event_tx.send(GeminiEvent::ToolCallCancellation {
            ids: cancellation.ids,
        });
        return false;
    }

    // Go away — server requesting reconnection
    if msg.go_away.is_some() {
        tracing::info!("Received goAway from server, triggering reconnection");
        return true;
    }

    false
}

fn build_setup_message(config: &GeminiConfig, resumption_handle: Option<String>) -> SetupMessage {
    SetupMessage {
        setup: Setup {
            model: config.model.clone(),
            generation_config: Some(GenerationConfig {
                temperature: Some(config.temperature),
                response_modalities: Some(vec!["AUDIO".into()]),
                speech_config: Some(SpeechConfig {
                    voice_config: VoiceConfig {
                        prebuilt_voice_config: PrebuiltVoiceConfig {
                            voice_name: config.voice.clone(),
                        },
                    },
                }),
            }),
            system_instruction: Some(Content {
                role: None,
                parts: vec![Part {
                    text: Some(config.system_prompt.clone()),
                    inline_data: None,
                }],
            }),
            tools: Some(build_tool_declarations()),
            session_resumption: Some(SessionResumptionConfig {
                handle: resumption_handle,
            }),
            context_window_compression: Some(ContextWindowCompression {
                sliding_window: SlidingWindow {},
            }),
        },
    }
}

/// Convert f32 PCM [-1.0, 1.0] to base64-encoded 16-bit LE PCM bytes.
fn encode_audio_message(pcm: &[f32]) -> RealtimeAudioMessage {
    let mut bytes = Vec::with_capacity(pcm.len() * 2);
    for &sample in pcm {
        let clamped = sample.clamp(-1.0, 1.0);
        let i16_val = (clamped * 32767.0) as i16;
        bytes.extend_from_slice(&i16_val.to_le_bytes());
    }

    RealtimeAudioMessage {
        realtime_input: RealtimeAudioInput {
            audio: Blob {
                mime_type: "audio/pcm;rate=16000".into(),
                data: BASE64.encode(&bytes),
            },
        },
    }
}

/// Convert 16-bit LE PCM bytes to f32 PCM [-1.0, 1.0].
fn pcm_bytes_to_f32(bytes: &[u8]) -> Vec<f32> {
    bytes
        .chunks_exact(2)
        .map(|chunk| {
            let sample = i16::from_le_bytes([chunk[0], chunk[1]]);
            sample as f32 / 32768.0
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_f32_to_pcm_base64() {
        let pcm = vec![0.0, 1.0, -1.0, 0.5];
        let msg = encode_audio_message(&pcm);
        let data = &msg.realtime_input.audio.data;

        // Decode and verify
        let bytes = BASE64.decode(data).unwrap();
        assert_eq!(bytes.len(), 8); // 4 samples * 2 bytes
        let samples = pcm_bytes_to_f32(&bytes);
        assert_eq!(samples.len(), 4);
        assert!((samples[0] - 0.0).abs() < 0.001);
        assert!((samples[1] - 1.0).abs() < 0.001);
        assert!((samples[2] - (-1.0)).abs() < 0.001);
        assert!((samples[3] - 0.5).abs() < 0.001);
    }

    #[test]
    fn pcm_bytes_roundtrip() {
        let original = vec![0.0_f32, 0.5, -0.5, 0.25, -0.25];
        let msg = encode_audio_message(&original);
        let bytes = BASE64
            .decode(&msg.realtime_input.audio.data)
            .unwrap();
        let decoded = pcm_bytes_to_f32(&bytes);

        assert_eq!(decoded.len(), original.len());
        for (a, b) in original.iter().zip(decoded.iter()) {
            assert!((a - b).abs() < 0.001, "{a} != {b}");
        }
    }

    #[test]
    fn build_setup_message_includes_tools() {
        let config = GeminiConfig {
            api_key: "test".into(),
            model: "models/gemini-2.5-flash-native-audio-preview-12-2025".into(),
            voice: "Kore".into(),
            system_prompt: "Test prompt".into(),
            temperature: 0.9,
            proxy_url: None,
        };
        let msg = build_setup_message(&config, None);
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("run_applescript"));
        assert!(json.contains("get_screen_context"));
        assert!(json.contains("slidingWindow"));
        assert!(json.contains("sessionResumption"));
    }

    #[test]
    fn build_setup_message_with_resumption() {
        let config = GeminiConfig {
            api_key: "test".into(),
            model: "models/gemini-2.5-flash-native-audio-preview-12-2025".into(),
            voice: "Kore".into(),
            system_prompt: "Test prompt".into(),
            temperature: 0.9,
            proxy_url: None,
        };
        let msg = build_setup_message(&config, Some("tok_resume_123".into()));
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("tok_resume_123"));
    }

    #[test]
    fn clamp_audio_values() {
        let pcm = vec![2.0, -2.0]; // out of range
        let msg = encode_audio_message(&pcm);
        let bytes = BASE64
            .decode(&msg.realtime_input.audio.data)
            .unwrap();
        let samples = pcm_bytes_to_f32(&bytes);
        assert!((samples[0] - 1.0).abs() < 0.001); // clamped to 1.0
        assert!((samples[1] - (-1.0)).abs() < 0.001); // clamped to -1.0
    }
}
