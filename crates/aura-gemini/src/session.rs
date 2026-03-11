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

// Gemini Live API audio protocol parameters:
// - Audio chunk duration: 100ms
// - Input sample rate: 16,000 Hz (PCM 16-bit LE)
// - Output sample rate: 24,000 Hz (PCM 16-bit LE)
const MAX_RECONNECT_ATTEMPTS: u32 = 5;
const INITIAL_BACKOFF_MS: u64 = 200;
const MAX_BACKOFF_MS: u64 = 30_000;
/// Minimum duration a connection must be alive before we consider it "stable"
/// and reset the reconnection attempt counter.
const MIN_STABLE_CONNECTION_SECS: u64 = 30;

/// Events emitted by the Gemini session.
#[derive(Debug, Clone)]
pub enum GeminiEvent {
    Connected {
        is_first: bool,
    },
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
    SessionHandle {
        handle: String,
    },
}

/// Handle for interacting with a live Gemini session.
pub struct GeminiLiveSession {
    audio_tx: mpsc::Sender<Vec<f32>>,
    video_tx: mpsc::Sender<String>,
    tool_response_tx: mpsc::Sender<(String, String, serde_json::Value)>,
    text_tx: mpsc::Sender<String>,
    event_tx: broadcast::Sender<GeminiEvent>,
    cancel: tokio_util::sync::CancellationToken,
    reconnect_tx: mpsc::Sender<()>,
    is_first_connect: Arc<std::sync::atomic::AtomicBool>,
}

impl GeminiLiveSession {
    /// Connect to the Gemini Live API and start the streaming loop.
    /// Spawns background tasks for sending and receiving.
    /// If `initial_resumption_handle` is provided, it will be used to resume a previous session.
    pub async fn connect(
        config: GeminiConfig,
        initial_resumption_handle: Option<String>,
    ) -> Result<Self> {
        let (audio_tx, audio_rx) = mpsc::channel::<Vec<f32>>(64);
        let (video_tx, video_rx) = mpsc::channel::<String>(8);
        let (tool_response_tx, tool_response_rx) =
            mpsc::channel::<(String, String, serde_json::Value)>(16);
        let (text_tx, text_rx) = mpsc::channel::<String>(16);
        let (reconnect_tx, reconnect_rx) = mpsc::channel::<()>(4);
        let (event_tx, _) = broadcast::channel::<GeminiEvent>(128);
        let cancel = tokio_util::sync::CancellationToken::new();
        let is_first_connect = Arc::new(std::sync::atomic::AtomicBool::new(true));

        let session_state = SessionState {
            config: Arc::new(config),
            resumption_handle: Arc::new(Mutex::new(initial_resumption_handle)),
            is_first_connect: Arc::clone(&is_first_connect),
        };

        // Spawn the connection manager task
        let state = session_state.clone();
        let tx = event_tx.clone();
        let token = cancel.clone();
        tokio::spawn(async move {
            connection_loop(
                state,
                audio_rx,
                video_rx,
                tool_response_rx,
                text_rx,
                reconnect_rx,
                tx,
                token,
            )
            .await;
        });

        Ok(Self {
            audio_tx,
            video_tx,
            tool_response_tx,
            text_tx,
            event_tx,
            cancel,
            reconnect_tx,
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
    ///
    /// Uses `try_send` so this never blocks. Returns `Err` only when the session
    /// is closed (receiver dropped); a full channel is treated as a dropped frame
    /// (non-fatal) and the caller should `continue` rather than `break`.
    pub fn send_video(&self, jpeg_base64: &str) -> Result<()> {
        match self.video_tx.try_send(jpeg_base64.to_string()) {
            Ok(()) => Ok(()),
            Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => {
                Err(anyhow::anyhow!("Video channel full — frame dropped"))
            }
            Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => {
                Err(anyhow::anyhow!("Session closed"))
            }
        }
    }

    /// Send a text message to Gemini.
    ///
    /// Uses `try_send` so this never blocks. Returns `Err` only when the session
    /// is closed (receiver dropped); a full channel is treated as a skipped send
    /// (non-fatal) and the caller should `continue` rather than `break`.
    pub fn send_text(&self, text: &str) -> Result<()> {
        match self.text_tx.try_send(text.to_string()) {
            Ok(()) => Ok(()),
            Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => {
                Err(anyhow::anyhow!("Text channel full — message dropped"))
            }
            Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => {
                Err(anyhow::anyhow!("Session closed"))
            }
        }
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

    /// Force a reconnection — drops the current WebSocket and reconnects.
    pub async fn reconnect(&self) {
        let _ = self.reconnect_tx.send(()).await;
    }

    /// Returns true if this is the first connection (not a reconnection).
    /// After the first connection, this returns false.
    pub fn is_first_connect(&self) -> bool {
        self.is_first_connect
            .load(std::sync::atomic::Ordering::Acquire)
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
    /// How long the connection was alive after setupComplete.
    connected_duration: Duration,
    error: anyhow::Error,
}

/// Channel receivers for inbound user data (audio, video, text, tool responses).
struct StreamChannels {
    audio_rx: mpsc::Receiver<Vec<f32>>,
    video_rx: mpsc::Receiver<String>,
    text_rx: mpsc::Receiver<String>,
    tool_response_rx: mpsc::Receiver<(String, String, serde_json::Value)>,
}

/// Main connection loop with reconnection logic.
#[allow(clippy::too_many_arguments)]
async fn connection_loop(
    state: SessionState,
    audio_rx: mpsc::Receiver<Vec<f32>>,
    video_rx: mpsc::Receiver<String>,
    tool_response_rx: mpsc::Receiver<(String, String, serde_json::Value)>,
    text_rx: mpsc::Receiver<String>,
    mut reconnect_rx: mpsc::Receiver<()>,
    event_tx: broadcast::Sender<GeminiEvent>,
    cancel: tokio_util::sync::CancellationToken,
) {
    let mut channels = StreamChannels {
        audio_rx,
        video_rx,
        text_rx,
        tool_response_rx,
    };
    let mut attempt: u32 = 0;

    loop {
        if cancel.is_cancelled() {
            break;
        }

        // Drain any pending reconnect signals before connecting
        while reconnect_rx.try_recv().is_ok() {}

        // Race the stream against a user-initiated reconnect request.
        // If reconnect fires, the stream future is dropped (closing the WebSocket)
        // and we loop back to reconnect immediately.
        let stream_result = tokio::select! {
            result = connect_and_stream(&state, &mut channels, &event_tx, &cancel) => result,
            _ = reconnect_rx.recv() => {
                tracing::info!("User-initiated reconnect, dropping current connection");
                let _ = event_tx.send(GeminiEvent::Reconnecting { attempt: 0 });
                attempt = 0;
                continue;
            }
        };

        match stream_result {
            Ok(go_away) => {
                if go_away {
                    // goAway — reconnect immediately without counting as failure
                    tracing::info!("Server sent goAway, reconnecting immediately");
                    continue;
                }
                // Clean disconnect
                let _ = event_tx.send(GeminiEvent::Disconnected);
                break;
            }
            Err(outcome) => {
                // Only reset counter if the connection was stable for long enough
                // — connections that drop immediately should not reset the backoff.
                if outcome.was_connected
                    && outcome.connected_duration.as_secs() >= MIN_STABLE_CONNECTION_SECS
                {
                    attempt = 0;
                }

                attempt += 1;
                if attempt > MAX_RECONNECT_ATTEMPTS {
                    let _ = event_tx.send(GeminiEvent::Error {
                        message: format!("Max reconnection attempts exceeded: {}", outcome.error),
                    });
                    let _ = event_tx.send(GeminiEvent::Disconnected);

                    // Park here waiting for a user-initiated reconnect instead of
                    // breaking.  Breaking would drop reconnect_rx, making all future
                    // IPC reconnect signals silently fail.
                    tracing::info!("Waiting for user-initiated reconnect signal");
                    tokio::select! {
                        _ = cancel.cancelled() => break,
                        _ = reconnect_rx.recv() => {
                            tracing::info!("User-initiated reconnect after max retries");
                            let _ = event_tx.send(GeminiEvent::Reconnecting { attempt: 0 });
                            attempt = 0;
                            continue;
                        }
                    }
                }

                tracing::warn!(attempt, error = %outcome.error, "Connection lost, reconnecting");
                let _ = event_tx.send(GeminiEvent::Reconnecting { attempt });

                let base_backoff_ms =
                    (INITIAL_BACKOFF_MS * 2u64.pow(attempt - 1)).min(MAX_BACKOFF_MS);
                // Jitter: +-25% using timestamp-based entropy to avoid thundering herd.
                // Mixes attempt number with nanosecond timestamp for per-attempt variation.
                let nanos = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .subsec_nanos();
                let hash = (nanos as u64).wrapping_mul(2654435761) ^ (attempt as u64 * 7);
                let jitter_factor = (hash % 1000) as f64 / 2000.0 + 0.75; // 0.75..1.25
                let backoff =
                    Duration::from_millis((base_backoff_ms as f64 * jitter_factor) as u64);
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
    channels: &mut StreamChannels,
    event_tx: &broadcast::Sender<GeminiEvent>,
    cancel: &tokio_util::sync::CancellationToken,
) -> std::result::Result<bool, StreamOutcome> {
    let mut was_connected = false;
    let mut connected_at = std::time::Instant::now();

    let result = connect_and_stream_inner(
        state,
        channels,
        event_tx,
        cancel,
        &mut was_connected,
        &mut connected_at,
    )
    .await;

    match result {
        Ok(go_away) => Ok(go_away),
        Err(error) => {
            let connected_duration = if was_connected {
                connected_at.elapsed()
            } else {
                Duration::ZERO
            };
            Err(StreamOutcome {
                was_connected,
                connected_duration,
                error,
            })
        }
    }
}

/// Returns `Ok(false)` for clean disconnect, `Ok(true)` for goAway (reconnect without penalty),
/// or `Err` for a real error.
async fn connect_and_stream_inner(
    state: &SessionState,
    channels: &mut StreamChannels,
    event_tx: &broadcast::Sender<GeminiEvent>,
    cancel: &tokio_util::sync::CancellationToken,
    was_connected: &mut bool,
    connected_at: &mut std::time::Instant,
) -> Result<bool> {
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
    tracing::debug!("Sending setup message (system prompt and tool declarations redacted)");
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
            _ = cancel.cancelled() => return Ok(false),
        };

        let json_text = match msg {
            Message::Text(text) => Some(text),
            Message::Binary(bytes) => String::from_utf8(bytes).ok(),
            Message::Close(frame) => {
                // Check if server rejected a stale resumption handle
                let reason = frame
                    .as_ref()
                    .map(|f| f.reason.as_ref())
                    .unwrap_or_default();
                if reason.contains("session not found")
                    || reason.contains("Session not found")
                    || reason.contains("SESSION_NOT_FOUND")
                {
                    tracing::warn!("Stale resumption handle rejected, clearing for fresh session");
                    *state.resumption_handle.lock().await = None;
                    // Emit event so daemon can clear it from SQLite too
                    let _ = event_tx.send(GeminiEvent::SessionHandle {
                        handle: String::new(),
                    });
                }
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
    *connected_at = std::time::Instant::now();
    let is_first = state
        .is_first_connect
        .swap(false, std::sync::atomic::Ordering::AcqRel);
    let _ = event_tx.send(GeminiEvent::Connected { is_first });
    tracing::info!("Gemini Live session connected");

    let mut ping_interval = tokio::time::interval(Duration::from_secs(20));
    ping_interval.tick().await; // skip immediate first tick
    ping_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        tokio::select! {
            biased;

            _ = cancel.cancelled() => return Ok(false),

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
                                return Ok(true); // goAway — reconnect without penalty
                            }
                        }
                        Err(e) => {
                            tracing::warn!(
                                error = %e,
                                text_preview = &text[..text.floor_char_boundary(200)],
                                "Failed to parse Gemini message, skipping"
                            );
                        }
                    }
                }
            }

            Some((id, name, response)) = channels.tool_response_rx.recv() => {
                let msg = ToolResponseMessage {
                    tool_response: ToolResponse {
                        function_responses: vec![FunctionResponse { id, name, response }],
                    },
                };
                let json = serde_json::to_string(&msg)?;
                ws_sink.send(Message::Text(json)).await?;
            }

            Some(text) = channels.text_rx.recv() => {
                let msg = ClientContentMessage {
                    client_content: ClientContent {
                        turns: vec![Content {
                            role: Some(ContentRole::User),
                            parts: vec![Part {
                                text: Some(text),
                                inline_data: None,
                            }],
                        }],
                        turn_complete: true,
                    },
                };
                let json = serde_json::to_string(&msg)?;
                ws_sink.send(Message::Text(json)).await?;
            }

            _ = ping_interval.tick() => {
                ws_sink.send(Message::Ping(vec![])).await?;
            }

            Some(jpeg_b64) = channels.video_rx.recv() => {
                let msg = RealtimeVideoMessage {
                    realtime_input: RealtimeVideoInput {
                        video: Blob {
                            mime_type: "image/jpeg".into(),
                            data: jpeg_b64,
                        },
                    },
                };
                let json = serde_json::to_string(&msg)?;
                ws_sink.send(Message::Text(json)).await?;
            }

            Some(pcm) = channels.audio_rx.recv() => {
                let msg = encode_audio_message(&pcm);
                let json = serde_json::to_string(&msg)?;
                ws_sink.send(Message::Text(json)).await?;
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
            *handle = Some(new_handle.clone());
            let _ = event_tx.send(GeminiEvent::SessionHandle { handle: new_handle });
        }
        return false;
    }

    // Server content (audio response, interruption, turn complete)
    if let Some(content) = msg.server_content {
        if content.interrupted == Some(true) {
            let _ = event_tx.send(GeminiEvent::Interrupted);
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
                media_resolution: Some("MEDIA_RESOLUTION_HIGH".to_string()),
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
        let bytes = BASE64.decode(&msg.realtime_input.audio.data).unwrap();
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
            proxy_auth_token: None,
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
            proxy_auth_token: None,
        };
        let msg = build_setup_message(&config, Some("tok_resume_123".into()));
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("tok_resume_123"));
    }

    #[test]
    fn clamp_audio_values() {
        let pcm = vec![2.0, -2.0]; // out of range
        let msg = encode_audio_message(&pcm);
        let bytes = BASE64.decode(&msg.realtime_input.audio.data).unwrap();
        let samples = pcm_bytes_to_f32(&bytes);
        assert!((samples[0] - 1.0).abs() < 0.001); // clamped to 1.0
        assert!((samples[1] - (-1.0)).abs() < 0.001); // clamped to -1.0
    }
}
