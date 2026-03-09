# Gemini Live API Integration — Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Replace the entire local voice pipeline (VAD/STT/LLM/TTS) with a single Gemini Live API WebSocket connection for audio-in, audio-out conversation with function calling.

**Architecture:** Mic audio (cpal, 16kHz PCM) is base64-encoded and streamed over a persistent WebSocket to Gemini's Live API. Gemini returns audio responses (24kHz PCM) and tool calls. Tool calls dispatch to MacOSExecutor. Session resumption and context compression enable unlimited session length.

**Tech Stack:** tokio-tungstenite (WebSocket), base64, serde_json, cpal (mic), rodio (playback)

---

### Task 1: Create `aura-gemini` crate scaffold

**Files:**
- Create: `crates/aura-gemini/Cargo.toml`
- Create: `crates/aura-gemini/src/lib.rs`
- Modify: `Cargo.toml:3` (workspace members)

**Step 1: Create the crate directory**

Run: `mkdir -p crates/aura-gemini/src`

**Step 2: Create Cargo.toml**

Create `crates/aura-gemini/Cargo.toml`:

```toml
[package]
name = "aura-gemini"
version.workspace = true
edition.workspace = true

[features]
test-support = []

[dev-dependencies]
aura-gemini = { path = ".", features = ["test-support"] }
tokio = { workspace = true, features = ["test-util"] }

[dependencies]
tokio.workspace = true
tracing.workspace = true
anyhow.workspace = true
serde.workspace = true
serde_json.workspace = true
tokio-tungstenite = { version = "0.24", features = ["native-tls"] }
base64 = "0.22"
futures-util = "0.3"
tokio-util = "0.7"
```

**Step 3: Create lib.rs**

Create `crates/aura-gemini/src/lib.rs`:

```rust
//! Aura Gemini: real-time voice streaming via Gemini Live API

pub mod config;
pub mod protocol;
pub mod session;
pub mod tools;
```

**Step 4: Add to workspace**

Modify `Cargo.toml` workspace members — add `"crates/aura-gemini"` to the members array.

**Step 5: Verify it compiles**

Run: `cargo check -p aura-gemini`
Expected: Compilation errors for missing modules (config, protocol, session, tools) — that's fine, we'll create them next.

**Step 6: Commit**

```bash
git add crates/aura-gemini/ Cargo.toml
git commit -m "feat: scaffold aura-gemini crate"
```

---

### Task 2: Implement `protocol.rs` — Gemini WebSocket message types

**Files:**
- Create: `crates/aura-gemini/src/protocol.rs`
- Test: `crates/aura-gemini/src/protocol.rs` (inline tests)

**Step 1: Write the failing test**

Create `crates/aura-gemini/src/protocol.rs` with types AND tests at the bottom. Start with tests that exercise serialization:

```rust
use serde::{Deserialize, Serialize};

// ── Client → Server messages ────────────────────────────────────────

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SetupMessage {
    pub setup: Setup,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Setup {
    pub model: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub generation_config: Option<GenerationConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system_instruction: Option<Content>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<Tool>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_resumption: Option<SessionResumptionConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_window_compression: Option<ContextWindowCompression>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GenerationConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub response_modalities: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub speech_config: Option<SpeechConfig>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SpeechConfig {
    pub voice_config: VoiceConfig,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VoiceConfig {
    pub prebuilt_voice_config: PrebuiltVoiceConfig,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PrebuiltVoiceConfig {
    pub voice_name: String,
}

#[derive(Debug, Serialize)]
pub struct Content {
    pub parts: Vec<Part>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Part {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub inline_data: Option<Blob>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Blob {
    pub mime_type: String,
    pub data: String, // base64-encoded
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Tool {
    pub function_declarations: Vec<FunctionDeclaration>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FunctionDeclaration {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionResumptionConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub handle: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextWindowCompression {
    pub sliding_window: SlidingWindow,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SlidingWindow {}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RealtimeInputMessage {
    pub realtime_input: RealtimeInput,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RealtimeInput {
    pub media_chunks: Vec<MediaChunk>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MediaChunk {
    pub mime_type: String,
    pub data: String, // base64-encoded
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolResponseMessage {
    pub tool_response: ToolResponse,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolResponse {
    pub function_responses: Vec<FunctionResponse>,
}

#[derive(Debug, Serialize)]
pub struct FunctionResponse {
    pub id: String,
    pub name: String,
    pub response: serde_json::Value,
}

// ── Server → Client messages ────────────────────────────────────────

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerMessage {
    #[serde(default)]
    pub setup_complete: Option<SetupComplete>,
    #[serde(default)]
    pub server_content: Option<ServerContent>,
    #[serde(default)]
    pub tool_call: Option<ToolCall>,
    #[serde(default)]
    pub tool_call_cancellation: Option<ToolCallCancellation>,
    #[serde(default)]
    pub go_away: Option<serde_json::Value>,
    #[serde(default)]
    pub session_resumption_update: Option<SessionResumptionUpdate>,
}

#[derive(Debug, Deserialize)]
pub struct SetupComplete {}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerContent {
    #[serde(default)]
    pub model_turn: Option<ModelTurn>,
    #[serde(default)]
    pub turn_complete: Option<bool>,
    #[serde(default)]
    pub interrupted: Option<bool>,
}

#[derive(Debug, Deserialize)]
pub struct ModelTurn {
    pub parts: Vec<Part>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolCall {
    pub function_calls: Vec<FunctionCall>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct FunctionCall {
    pub id: String,
    pub name: String,
    pub args: serde_json::Value,
}

#[derive(Debug, Deserialize)]
pub struct ToolCallCancellation {
    pub ids: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionResumptionUpdate {
    pub new_handle: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serialize_setup_message() {
        let msg = SetupMessage {
            setup: Setup {
                model: "models/gemini-live-2.5-flash-native-audio".into(),
                generation_config: Some(GenerationConfig {
                    temperature: Some(0.9),
                    response_modalities: Some(vec!["AUDIO".into()]),
                    speech_config: Some(SpeechConfig {
                        voice_config: VoiceConfig {
                            prebuilt_voice_config: PrebuiltVoiceConfig {
                                voice_name: "Kore".into(),
                            },
                        },
                    }),
                }),
                system_instruction: Some(Content {
                    parts: vec![Part {
                        text: Some("You are Aura, a helpful voice assistant.".into()),
                        inline_data: None,
                    }],
                }),
                tools: None,
                session_resumption: Some(SessionResumptionConfig { handle: None }),
                context_window_compression: Some(ContextWindowCompression {
                    sliding_window: SlidingWindow {},
                }),
            },
        };

        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("gemini-live-2.5-flash-native-audio"));
        assert!(json.contains("\"AUDIO\""));
        assert!(json.contains("Kore"));
        assert!(json.contains("slidingWindow"));
    }

    #[test]
    fn serialize_realtime_input() {
        let msg = RealtimeInputMessage {
            realtime_input: RealtimeInput {
                media_chunks: vec![MediaChunk {
                    mime_type: "audio/pcm;rate=16000".into(),
                    data: "AQIDBA==".into(), // [1,2,3,4] base64
                }],
            },
        };

        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("audio/pcm;rate=16000"));
        assert!(json.contains("AQIDBA=="));
    }

    #[test]
    fn deserialize_setup_complete() {
        let json = r#"{"setupComplete":{}}"#;
        let msg: ServerMessage = serde_json::from_str(json).unwrap();
        assert!(msg.setup_complete.is_some());
    }

    #[test]
    fn deserialize_server_content_with_audio() {
        let json = r#"{
            "serverContent": {
                "modelTurn": {
                    "parts": [{
                        "inlineData": {
                            "mimeType": "audio/pcm;rate=24000",
                            "data": "AQIDBA=="
                        }
                    }]
                },
                "turnComplete": false
            }
        }"#;
        let msg: ServerMessage = serde_json::from_str(json).unwrap();
        let content = msg.server_content.unwrap();
        let parts = content.model_turn.unwrap().parts;
        assert_eq!(parts[0].inline_data.as_ref().unwrap().mime_type, "audio/pcm;rate=24000");
    }

    #[test]
    fn deserialize_interrupted() {
        let json = r#"{"serverContent":{"interrupted":true}}"#;
        let msg: ServerMessage = serde_json::from_str(json).unwrap();
        let content = msg.server_content.unwrap();
        assert_eq!(content.interrupted, Some(true));
    }

    #[test]
    fn deserialize_tool_call() {
        let json = r#"{
            "toolCall": {
                "functionCalls": [{
                    "id": "call_123",
                    "name": "open_app",
                    "args": {"app_name": "Safari"}
                }]
            }
        }"#;
        let msg: ServerMessage = serde_json::from_str(json).unwrap();
        let tc = msg.tool_call.unwrap();
        assert_eq!(tc.function_calls[0].name, "open_app");
        assert_eq!(tc.function_calls[0].args["app_name"], "Safari");
    }

    #[test]
    fn deserialize_session_resumption_update() {
        let json = r#"{"sessionResumptionUpdate":{"newHandle":"tok_abc123"}}"#;
        let msg: ServerMessage = serde_json::from_str(json).unwrap();
        let update = msg.session_resumption_update.unwrap();
        assert_eq!(update.new_handle, "tok_abc123");
    }

    #[test]
    fn serialize_tool_response() {
        let msg = ToolResponseMessage {
            tool_response: ToolResponse {
                function_responses: vec![FunctionResponse {
                    id: "call_123".into(),
                    name: "open_app".into(),
                    response: serde_json::json!({"success": true}),
                }],
            },
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("call_123"));
        assert!(json.contains("open_app"));
    }
}
```

**Step 2: Run tests to verify they pass**

Run: `cargo test -p aura-gemini`
Expected: All 7 tests pass.

**Step 3: Commit**

```bash
git add crates/aura-gemini/src/protocol.rs
git commit -m "feat: add Gemini Live API protocol types with tests"
```

---

### Task 3: Implement `config.rs` — session configuration

**Files:**
- Create: `crates/aura-gemini/src/config.rs`

**Step 1: Write the config module**

Create `crates/aura-gemini/src/config.rs`:

```rust
use anyhow::{Context, Result};

const DEFAULT_MODEL: &str = "models/gemini-live-2.5-flash-native-audio";
const DEFAULT_VOICE: &str = "Kore";
const DEFAULT_SYSTEM_PROMPT: &str = "You are Aura, a friendly and helpful voice assistant running on macOS. \
    Keep responses concise and conversational. When the user asks you to perform an action \
    (open an app, search files, tile windows, open a URL, or describe the screen), \
    use the appropriate tool. For everything else, respond conversationally.";

pub struct GeminiConfig {
    pub api_key: String,
    pub model: String,
    pub voice: String,
    pub system_prompt: String,
    pub temperature: f32,
}

impl GeminiConfig {
    /// Create config from environment. Fails if GEMINI_API_KEY is not set.
    pub fn from_env() -> Result<Self> {
        let api_key = std::env::var("GEMINI_API_KEY")
            .context("GEMINI_API_KEY environment variable is not set")?;

        Ok(Self {
            api_key,
            model: DEFAULT_MODEL.into(),
            voice: DEFAULT_VOICE.into(),
            system_prompt: DEFAULT_SYSTEM_PROMPT.into(),
            temperature: 0.9,
        })
    }

    pub fn websocket_url(&self) -> String {
        format!(
            "wss://generativelanguage.googleapis.com/ws/google.ai.generativelanguage.v1alpha.GenerativeService.BidiGenerateContent?key={}",
            self.api_key
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_from_env_missing_key() {
        // Ensure the var is unset for this test
        std::env::remove_var("GEMINI_API_KEY");
        let result = GeminiConfig::from_env();
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("GEMINI_API_KEY"));
    }

    #[test]
    fn websocket_url_format() {
        let config = GeminiConfig {
            api_key: "test_key_123".into(),
            model: DEFAULT_MODEL.into(),
            voice: DEFAULT_VOICE.into(),
            system_prompt: DEFAULT_SYSTEM_PROMPT.into(),
            temperature: 0.9,
        };
        let url = config.websocket_url();
        assert!(url.starts_with("wss://"));
        assert!(url.contains("BidiGenerateContent"));
        assert!(url.contains("test_key_123"));
    }
}
```

**Step 2: Run tests**

Run: `cargo test -p aura-gemini`
Expected: All tests pass (protocol + config tests).

**Step 3: Commit**

```bash
git add crates/aura-gemini/src/config.rs
git commit -m "feat: add Gemini session configuration"
```

---

### Task 4: Implement `tools.rs` — function declarations for intents

**Files:**
- Create: `crates/aura-gemini/src/tools.rs`

**Step 1: Write the tools module**

Create `crates/aura-gemini/src/tools.rs`:

```rust
use crate::protocol::{FunctionDeclaration, Tool};

/// Build the tool declarations for the Gemini setup message.
/// These map to macOS actions in aura-bridge.
pub fn build_tool_declarations() -> Vec<Tool> {
    vec![Tool {
        function_declarations: vec![
            FunctionDeclaration {
                name: "open_app".into(),
                description: "Open an application by name on macOS".into(),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "app_name": {
                            "type": "string",
                            "description": "Name of the application to open (e.g. Safari, Terminal, Finder)"
                        }
                    },
                    "required": ["app_name"]
                }),
            },
            FunctionDeclaration {
                name: "search_files".into(),
                description: "Search for files on the user's computer using Spotlight".into(),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "query": {
                            "type": "string",
                            "description": "The search query to find files"
                        }
                    },
                    "required": ["query"]
                }),
            },
            FunctionDeclaration {
                name: "tile_windows".into(),
                description: "Arrange windows in a tiling layout on screen".into(),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "layout": {
                            "type": "string",
                            "enum": ["left-right", "grid", "stack"],
                            "description": "The tiling layout to apply"
                        }
                    },
                    "required": ["layout"]
                }),
            },
            FunctionDeclaration {
                name: "launch_url".into(),
                description: "Open a URL in the default web browser".into(),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "url": {
                            "type": "string",
                            "description": "The URL to open (must start with http:// or https://)"
                        }
                    },
                    "required": ["url"]
                }),
            },
            FunctionDeclaration {
                name: "summarize_screen".into(),
                description: "Capture and describe what is currently visible on the user's screen".into(),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {}
                }),
            },
        ],
    }]
}

/// Map a Gemini function call to an aura-bridge Action.
/// Returns None if the function name is not recognized.
pub fn function_call_to_action(
    name: &str,
    args: &serde_json::Value,
) -> Option<aura_bridge::actions::Action> {
    match name {
        "open_app" => Some(aura_bridge::actions::Action::OpenApp {
            name: args["app_name"].as_str()?.to_string(),
        }),
        "search_files" => Some(aura_bridge::actions::Action::SearchFiles {
            query: args["query"].as_str()?.to_string(),
        }),
        "tile_windows" => Some(aura_bridge::actions::Action::TileWindows {
            layout: args["layout"].as_str()?.to_string(),
        }),
        "launch_url" => Some(aura_bridge::actions::Action::LaunchUrl {
            url: args["url"].as_str()?.to_string(),
        }),
        "summarize_screen" => None, // handled specially by caller
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_declarations_are_valid_json() {
        let tools = build_tool_declarations();
        let json = serde_json::to_string(&tools).unwrap();
        assert!(json.contains("open_app"));
        assert!(json.contains("search_files"));
        assert!(json.contains("tile_windows"));
        assert!(json.contains("launch_url"));
        assert!(json.contains("summarize_screen"));
    }

    #[test]
    fn map_open_app() {
        let args = serde_json::json!({"app_name": "Safari"});
        let action = function_call_to_action("open_app", &args).unwrap();
        match action {
            aura_bridge::actions::Action::OpenApp { name } => assert_eq!(name, "Safari"),
            _ => panic!("Expected OpenApp"),
        }
    }

    #[test]
    fn map_unknown_function() {
        let args = serde_json::json!({});
        assert!(function_call_to_action("unknown_fn", &args).is_none());
    }
}
```

**Step 2: Add aura-bridge dependency to aura-gemini**

Add to `crates/aura-gemini/Cargo.toml` under `[dependencies]`:

```toml
aura-bridge = { path = "../aura-bridge" }
```

**Step 3: Run tests**

Run: `cargo test -p aura-gemini`
Expected: All tests pass.

**Step 4: Commit**

```bash
git add crates/aura-gemini/src/tools.rs crates/aura-gemini/Cargo.toml
git commit -m "feat: add Gemini tool declarations for macOS actions"
```

---

### Task 5: Implement `session.rs` — WebSocket client core

**Files:**
- Create: `crates/aura-gemini/src/session.rs`

This is the largest task. The session manages the WebSocket connection, audio encoding/decoding, and message dispatch.

**Step 1: Write the failing test**

Add tests first at the bottom of `session.rs`, then implement. The test uses a mock WebSocket server.

Create `crates/aura-gemini/src/session.rs`:

```rust
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use futures_util::{SinkExt, StreamExt};
use tokio::sync::{broadcast, mpsc, Mutex};
use tokio_tungstenite::tungstenite::Message;

use crate::config::GeminiConfig;
use crate::protocol::*;
use crate::tools::build_tool_declarations;

const AUDIO_CHUNK_DURATION_MS: u64 = 100;
const INPUT_SAMPLE_RATE: u32 = 16_000;
const OUTPUT_SAMPLE_RATE: u32 = 24_000;
const MAX_RECONNECT_ATTEMPTS: u32 = 10;
const INITIAL_BACKOFF_MS: u64 = 1000;
const MAX_BACKOFF_MS: u64 = 30_000;

/// Events emitted by the Gemini session.
#[derive(Debug, Clone)]
pub enum GeminiEvent {
    Connected,
    AudioResponse { samples: Vec<f32> },
    ToolCall { id: String, name: String, args: serde_json::Value },
    Interrupted,
    Transcription { text: String },
    TurnComplete,
    Error { message: String },
    Reconnecting { attempt: u32 },
    Disconnected,
}

/// Handle for interacting with a live Gemini session.
pub struct GeminiLiveSession {
    audio_tx: mpsc::Sender<Vec<f32>>,
    tool_response_tx: mpsc::Sender<(String, String, serde_json::Value)>,
    event_tx: broadcast::Sender<GeminiEvent>,
    cancel: tokio_util::sync::CancellationToken,
}

impl GeminiLiveSession {
    /// Connect to the Gemini Live API and start the streaming loop.
    /// Spawns background tasks for sending and receiving.
    pub async fn connect(config: GeminiConfig) -> Result<Self> {
        let (audio_tx, audio_rx) = mpsc::channel::<Vec<f32>>(64);
        let (tool_response_tx, tool_response_rx) =
            mpsc::channel::<(String, String, serde_json::Value)>(16);
        let (event_tx, _) = broadcast::channel::<GeminiEvent>(128);
        let cancel = tokio_util::sync::CancellationToken::new();

        let session_state = SessionState {
            config: Arc::new(config),
            resumption_handle: Arc::new(Mutex::new(None)),
        };

        // Spawn the connection manager task
        let state = session_state.clone();
        let tx = event_tx.clone();
        let token = cancel.clone();
        tokio::spawn(async move {
            connection_loop(state, audio_rx, tool_response_rx, tx, token).await;
        });

        Ok(Self {
            audio_tx,
            tool_response_tx,
            event_tx,
            cancel,
        })
    }

    /// Send PCM f32 audio at 16kHz to Gemini.
    pub async fn send_audio(&self, pcm_16khz: &[f32]) -> Result<()> {
        self.audio_tx
            .send(pcm_16khz.to_vec())
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
}

/// Main connection loop with reconnection logic.
async fn connection_loop(
    state: SessionState,
    mut audio_rx: mpsc::Receiver<Vec<f32>>,
    mut tool_response_rx: mpsc::Receiver<(String, String, serde_json::Value)>,
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
            &mut tool_response_rx,
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
            Err(e) => {
                attempt += 1;
                if attempt > MAX_RECONNECT_ATTEMPTS {
                    let _ = event_tx.send(GeminiEvent::Error {
                        message: format!("Max reconnection attempts exceeded: {e}"),
                    });
                    break;
                }

                tracing::warn!(attempt, error = %e, "Connection lost, reconnecting");
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
    tool_response_rx: &mut mpsc::Receiver<(String, String, serde_json::Value)>,
    event_tx: &broadcast::Sender<GeminiEvent>,
    cancel: &tokio_util::sync::CancellationToken,
) -> Result<()> {
    let url = state.config.websocket_url();
    let (ws_stream, _) = tokio_tungstenite::connect_async(&url)
        .await
        .context("WebSocket connection failed")?;

    let (mut ws_sink, mut ws_source) = ws_stream.split();

    // Send setup message
    let resumption_handle = state.resumption_handle.lock().await.clone();
    let setup = build_setup_message(&state.config, resumption_handle);
    let setup_json = serde_json::to_string(&setup)?;
    ws_sink.send(Message::Text(setup_json.into())).await?;

    // Wait for setupComplete
    loop {
        let msg = ws_source
            .next()
            .await
            .context("Connection closed before setupComplete")?
            .context("WebSocket error during setup")?;

        if let Message::Text(text) = msg {
            let server_msg: ServerMessage = serde_json::from_str(&text)?;
            if server_msg.setup_complete.is_some() {
                break;
            }
        }
    }

    let _ = event_tx.send(GeminiEvent::Connected);
    tracing::info!("Gemini Live session connected");

    // Wrap sink in Arc<Mutex> for shared access
    let ws_sink = Arc::new(Mutex::new(ws_sink));

    loop {
        tokio::select! {
            _ = cancel.cancelled() => return Ok(()),

            // Forward mic audio to Gemini
            Some(pcm) = audio_rx.recv() => {
                let msg = encode_audio_message(&pcm);
                let json = serde_json::to_string(&msg)?;
                ws_sink.lock().await.send(Message::Text(json.into())).await?;
            }

            // Forward tool responses to Gemini
            Some((id, name, response)) = tool_response_rx.recv() => {
                let msg = ToolResponseMessage {
                    tool_response: ToolResponse {
                        function_responses: vec![FunctionResponse { id, name, response }],
                    },
                };
                let json = serde_json::to_string(&msg)?;
                ws_sink.lock().await.send(Message::Text(json.into())).await?;
            }

            // Receive messages from Gemini
            msg = ws_source.next() => {
                let Some(msg) = msg else {
                    return Err(anyhow::anyhow!("WebSocket connection closed"));
                };
                let msg = msg?;

                match msg {
                    Message::Text(text) => {
                        let server_msg: ServerMessage = serde_json::from_str(&text)?;
                        handle_server_message(server_msg, event_tx, state).await;
                    }
                    Message::Close(_) => {
                        return Err(anyhow::anyhow!("Server closed connection"));
                    }
                    _ => {} // ignore ping/pong/binary
                }
            }
        }
    }
}

async fn handle_server_message(
    msg: ServerMessage,
    event_tx: &broadcast::Sender<GeminiEvent>,
    state: &SessionState,
) {
    // Session resumption token update
    if let Some(update) = msg.session_resumption_update {
        let mut handle = state.resumption_handle.lock().await;
        *handle = Some(update.new_handle);
        return;
    }

    // Server content (audio response, interruption, turn complete)
    if let Some(content) = msg.server_content {
        if content.interrupted == Some(true) {
            let _ = event_tx.send(GeminiEvent::Interrupted);
            return;
        }

        if let Some(model_turn) = content.model_turn {
            for part in model_turn.parts {
                if let Some(blob) = part.inline_data {
                    if blob.mime_type.starts_with("audio/") {
                        if let Ok(bytes) = BASE64.decode(&blob.data) {
                            let samples = pcm_bytes_to_f32(&bytes);
                            let _ = event_tx.send(GeminiEvent::AudioResponse { samples });
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
        return;
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
        return;
    }

    // Go away (server requesting disconnect)
    if msg.go_away.is_some() {
        tracing::info!("Received goAway from server");
        // The connection loop will handle reconnection
    }
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
fn encode_audio_message(pcm: &[f32]) -> RealtimeInputMessage {
    let mut bytes = Vec::with_capacity(pcm.len() * 2);
    for &sample in pcm {
        let clamped = sample.clamp(-1.0, 1.0);
        let i16_val = (clamped * 32767.0) as i16;
        bytes.extend_from_slice(&i16_val.to_le_bytes());
    }

    RealtimeInputMessage {
        realtime_input: RealtimeInput {
            media_chunks: vec![MediaChunk {
                mime_type: "audio/pcm;rate=16000".into(),
                data: BASE64.encode(&bytes),
            }],
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
        let data = &msg.realtime_input.media_chunks[0].data;

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
        let bytes = BASE64.decode(&msg.realtime_input.media_chunks[0].data).unwrap();
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
            model: "models/gemini-live-2.5-flash-native-audio".into(),
            voice: "Kore".into(),
            system_prompt: "Test prompt".into(),
            temperature: 0.9,
        };
        let msg = build_setup_message(&config, None);
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("open_app"));
        assert!(json.contains("search_files"));
        assert!(json.contains("slidingWindow"));
        assert!(json.contains("sessionResumption"));
    }

    #[test]
    fn build_setup_message_with_resumption() {
        let config = GeminiConfig {
            api_key: "test".into(),
            model: "models/gemini-live-2.5-flash-native-audio".into(),
            voice: "Kore".into(),
            system_prompt: "Test prompt".into(),
            temperature: 0.9,
        };
        let msg = build_setup_message(&config, Some("tok_resume_123".into()));
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("tok_resume_123"));
    }

    #[test]
    fn clamp_audio_values() {
        let pcm = vec![2.0, -2.0]; // out of range
        let msg = encode_audio_message(&pcm);
        let bytes = BASE64.decode(&msg.realtime_input.media_chunks[0].data).unwrap();
        let samples = pcm_bytes_to_f32(&bytes);
        assert!((samples[0] - 1.0).abs() < 0.001); // clamped to 1.0
        assert!((samples[1] - (-1.0)).abs() < 0.001); // clamped to -1.0
    }
}
```

**Step 2: Run tests**

Run: `cargo test -p aura-gemini`
Expected: All tests pass (protocol + config + session unit tests).

**Step 3: Commit**

```bash
git add crates/aura-gemini/src/session.rs
git commit -m "feat: implement Gemini Live WebSocket session with reconnection"
```

---

### Task 6: Update `aura-voice` — remove STT, VAD, TTS; keep audio + playback

**Files:**
- Modify: `crates/aura-voice/src/lib.rs`
- Modify: `crates/aura-voice/Cargo.toml`
- Delete: `crates/aura-voice/src/vad.rs`
- Delete: `crates/aura-voice/src/stt.rs`
- Delete: `crates/aura-voice/src/tts.rs`
- Delete: `crates/aura-voice/src/pipeline.rs`

**Step 1: Update lib.rs**

Replace `crates/aura-voice/src/lib.rs` with:

```rust
//! Aura voice engine: audio capture and playback

pub mod audio;
pub mod playback;
pub mod wakeword;
```

**Step 2: Remove unused dependencies from Cargo.toml**

Update `crates/aura-voice/Cargo.toml` — remove `whisper-rs`, `voice_activity_detector`, `kokoro-tts`:

```toml
[package]
name = "aura-voice"
version.workspace = true
edition.workspace = true

[dependencies]
tokio.workspace = true
tracing.workspace = true
anyhow.workspace = true
cpal = "0.15"
rustpotter = "3"
rodio = "0.20"
dirs = "6"
which = "7"
tokio-util = "0.7"
```

**Step 3: Delete removed modules**

Run:
```bash
rm crates/aura-voice/src/vad.rs
rm crates/aura-voice/src/stt.rs
rm crates/aura-voice/src/tts.rs
rm crates/aura-voice/src/pipeline.rs
```

**Step 4: Delete related test files**

Run: `find crates/aura-voice/tests -name '*.rs' | head -20` to see what test files exist, then remove tests for VAD/STT/TTS.

Run:
```bash
rm -f crates/aura-voice/tests/vad_test.rs
rm -f crates/aura-voice/tests/stt_test.rs
rm -f crates/aura-voice/tests/tts_test.rs
rm -f crates/aura-voice/tests/pipeline_test.rs
```

**Step 5: Verify compilation**

Run: `cargo check -p aura-voice`
Expected: Compiles successfully. May have warnings about unused deps — that's fine.

**Step 6: Commit**

```bash
git add -A crates/aura-voice/
git commit -m "refactor: remove STT, VAD, TTS from aura-voice (replaced by Gemini)"
```

---

### Task 7: Remove `aura-llm` crate

**Files:**
- Modify: `Cargo.toml` (workspace members)
- Modify: `crates/aura-daemon/Cargo.toml`
- Modify: `crates/aura-bridge/Cargo.toml`
- Modify: `crates/aura-bridge/src/mapper.rs`
- Modify: `crates/aura-daemon/src/event.rs`

The `aura-bridge` crate currently depends on `aura-llm` for `Intent` in the mapper. We need to either:
- Remove the mapper (since Gemini handles intents directly via function calling)
- Or decouple the Intent type

Since Gemini function calls map directly to `Action` (via `tools.rs` in `aura-gemini`), we no longer need `mapper.rs` or `Intent`.

**Step 1: Remove mapper from aura-bridge**

Delete `crates/aura-bridge/src/mapper.rs`.

Update `crates/aura-bridge/src/lib.rs`:

```rust
//! Aura OS bridge: platform-specific system actions

pub mod actions;

#[cfg(target_os = "macos")]
pub mod macos;
```

Remove `aura-llm` dependency from `crates/aura-bridge/Cargo.toml`:

```toml
[package]
name = "aura-bridge"
version.workspace = true
edition.workspace = true

[features]
test-support = []

[dev-dependencies]
aura-bridge = { path = ".", features = ["test-support"] }

[dependencies]
tokio.workspace = true
tracing.workspace = true
serde.workspace = true
serde_json.workspace = true
async-trait = "0.1"
```

**Step 2: Update aura-daemon event.rs**

Remove the `Intent` import and `IntentParsed` variant from `crates/aura-daemon/src/event.rs`:

```rust
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AuraEvent {
    // Voice pipeline
    WakeWordDetected,
    ListeningStarted,
    ListeningStopped,
    VoiceCommand { text: String },

    // Actions
    ActionExecuted { description: String },
    ActionFailed { description: String, error: String },

    // Conversation
    AssistantSpeaking { text: String },
    BargeIn,

    // Gemini session
    GeminiConnected,
    GeminiReconnecting { attempt: u32 },

    // Overlay
    ShowOverlay { content: OverlayContent },
    HideOverlay,

    // System
    Shutdown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum OverlayContent {
    Listening,
    Processing,
    Response { text: String },
    Error { message: String },
}
```

**Step 3: Remove aura-llm from workspace**

Update `Cargo.toml` workspace members — remove `"crates/aura-llm"`.

Update `crates/aura-daemon/Cargo.toml` — replace `aura-llm` with `aura-gemini`:

```toml
[dependencies]
aura-voice = { path = "../aura-voice" }
aura-overlay = { path = "../aura-overlay" }
aura-screen = { path = "../aura-screen" }
aura-gemini = { path = "../aura-gemini" }
aura-bridge = { path = "../aura-bridge" }
serde.workspace = true
serde_json.workspace = true
tokio.workspace = true
tracing.workspace = true
tracing-subscriber.workspace = true
anyhow.workspace = true
clap = { version = "4", features = ["derive"] }
tokio-util = "0.7"
winit = { version = "0.30", features = ["rwh_06"] }
dirs = "6"
which = "7"
```

**Step 4: Verify compilation**

Run: `cargo check -p aura-bridge && cargo check -p aura-gemini`
Expected: Both compile. `aura-daemon` will fail until we rewrite main.rs (next task).

**Step 5: Commit**

```bash
git add -A
git commit -m "refactor: remove aura-llm, decouple aura-bridge from intent system"
```

---

### Task 8: Rewrite `aura-daemon/src/main.rs` — Gemini-powered orchestration

**Files:**
- Modify: `crates/aura-daemon/src/main.rs`

This is the main integration point. Replace the old voice pipeline + Ollama processing with:
1. cpal mic capture → stream audio to `GeminiLiveSession`
2. GeminiLiveSession events → playback, tool calls, overlay updates

**Step 1: Rewrite main.rs**

Replace `crates/aura-daemon/src/main.rs` with:

```rust
use anyhow::{Context, Result};
use clap::Parser;
use tokio_util::sync::CancellationToken;
use winit::event_loop::EventLoopProxy;

use aura_bridge::actions::ActionExecutor;
use aura_daemon::bus::EventBus;
use aura_daemon::daemon::Daemon;
use aura_daemon::event::{AuraEvent, OverlayContent};
use aura_gemini::config::GeminiConfig;
use aura_gemini::session::{GeminiEvent, GeminiLiveSession};
use aura_gemini::tools::function_call_to_action;
use aura_overlay::renderer::OverlayState;
use aura_overlay::window::{create_event_loop, OverlayMessage, OverlayWindow};
use aura_voice::audio::AudioCapture;
use aura_voice::playback::AudioPlayer;
use tracing_subscriber::EnvFilter;

const EVENT_BUS_CAPACITY: usize = 64;
const OUTPUT_SAMPLE_RATE: u32 = 24_000;

#[derive(Parser)]
#[command(name = "aura", about = "Voice-first AI desktop companion")]
struct Cli {
    /// Run without the overlay window
    #[arg(long)]
    no_overlay: bool,

    /// Enable verbose logging
    #[arg(short, long)]
    verbose: bool,
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    let filter = if cli.verbose { "debug" } else { "info" };
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(filter)),
        )
        .init();

    // Validate API key early
    let gemini_config = GeminiConfig::from_env()
        .context("Gemini configuration failed. Set GEMINI_API_KEY environment variable.")?;

    let bus = EventBus::new(EVENT_BUS_CAPACITY);
    let cancel = CancellationToken::new();

    if cli.no_overlay {
        let rt = tokio::runtime::Runtime::new()?;
        rt.block_on(run_daemon(gemini_config, bus, cancel))?;
    } else {
        let event_loop = create_event_loop().context("Failed to create overlay event loop")?;
        let proxy = event_loop.create_proxy();

        let bg_bus = bus.clone();
        let bg_cancel = cancel.clone();
        let bg_proxy = proxy.clone();
        std::thread::spawn(move || {
            let rt = tokio::runtime::Runtime::new().expect("Failed to create tokio runtime");
            rt.block_on(async {
                let bridge_bus = bg_bus.clone();
                let bridge_proxy = bg_proxy.clone();
                let bridge_cancel = bg_cancel.clone();
                tokio::spawn(async move {
                    run_overlay_bridge(bridge_bus, bridge_proxy, bridge_cancel).await;
                });

                if let Err(e) = run_daemon(gemini_config, bg_bus, bg_cancel).await {
                    tracing::error!("Daemon error: {e}");
                }

                let _ = bg_proxy.send_event(OverlayMessage::Shutdown);
            });
        });

        let mut overlay = OverlayWindow::new();
        event_loop.run_app(&mut overlay)?;
    }

    Ok(())
}

async fn run_daemon(
    gemini_config: GeminiConfig,
    bus: EventBus,
    cancel: CancellationToken,
) -> Result<()> {
    // Connect to Gemini
    let session = GeminiLiveSession::connect(gemini_config)
        .await
        .context("Failed to connect to Gemini Live API")?;

    // Set up mic capture on a dedicated thread (cpal's Stream is !Send)
    let (std_tx, std_rx) = std::sync::mpsc::channel::<Vec<f32>>();
    let capture = AudioCapture::new(None).context("Failed to open microphone")?;
    let _stream = capture.start(std_tx).context("Failed to start audio stream")?;

    // Bridge: cpal std::sync::mpsc → tokio task → GeminiLiveSession
    let audio_session = session.clone_audio_sender();
    let bridge_cancel = cancel.clone();
    let (tok_tx, mut tok_rx) = tokio::sync::mpsc::channel::<Vec<f32>>(64);

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

    // Forward mic audio to Gemini session
    let audio_cancel = cancel.clone();
    let audio_session_ref = &session;
    tokio::spawn({
        let session_clone = unsafe_clone_session_sender(&session);
        let cancel = audio_cancel.clone();
        async move {
            loop {
                tokio::select! {
                    _ = cancel.cancelled() => break,
                    Some(chunk) = tok_rx.recv() => {
                        if let Err(e) = session_clone.send(chunk).await {
                            tracing::error!("Failed to send audio to Gemini: {e}");
                            break;
                        }
                    }
                }
            }
        }
    });

    // Process Gemini events
    run_processor(session, bus.clone(), cancel.clone()).await?;

    // Shutdown
    let daemon = Daemon::new(bus);
    daemon.run().await?;
    cancel.cancel();

    Ok(())
}

async fn run_processor(
    session: GeminiLiveSession,
    bus: EventBus,
    cancel: CancellationToken,
) -> Result<()> {
    let mut events = session.subscribe();

    let player = match AudioPlayer::new() {
        Ok(p) => Some(p),
        Err(e) => {
            tracing::warn!("Audio playback unavailable: {e}");
            None
        }
    };

    #[cfg(target_os = "macos")]
    let executor = aura_bridge::macos::MacOSExecutor::new();

    tracing::info!("Processor running (Gemini Live mode)");

    loop {
        tokio::select! {
            _ = cancel.cancelled() => break,
            event = events.recv() => {
                match event {
                    Ok(GeminiEvent::Connected) => {
                        tracing::info!("Gemini session connected");
                        let _ = bus.send(AuraEvent::GeminiConnected);
                    }
                    Ok(GeminiEvent::AudioResponse { samples }) => {
                        if let Some(p) = &player {
                            if let Err(e) = p.play(samples, OUTPUT_SAMPLE_RATE) {
                                tracing::error!("Playback failed: {e}");
                            }
                        }
                    }
                    Ok(GeminiEvent::ToolCall { id, name, args }) => {
                        tracing::info!(name = %name, "Gemini tool call");

                        let response = if name == "summarize_screen" {
                            // TODO: integrate aura-screen capture
                            serde_json::json!({"error": "Screen summarization not yet implemented"})
                        } else if let Some(action) = function_call_to_action(&name, &args) {
                            #[cfg(target_os = "macos")]
                            {
                                let result = executor.execute(&action).await;
                                if result.success {
                                    let _ = bus.send(AuraEvent::ActionExecuted {
                                        description: result.description.clone(),
                                    });
                                    serde_json::json!({
                                        "success": true,
                                        "description": result.description,
                                        "data": result.data,
                                    })
                                } else {
                                    let _ = bus.send(AuraEvent::ActionFailed {
                                        description: result.description.clone(),
                                        error: result.description.clone(),
                                    });
                                    serde_json::json!({
                                        "success": false,
                                        "error": result.description,
                                    })
                                }
                            }
                            #[cfg(not(target_os = "macos"))]
                            {
                                serde_json::json!({"error": "Platform not supported"})
                            }
                        } else {
                            serde_json::json!({"error": format!("Unknown function: {name}")})
                        };

                        if let Err(e) = session.send_tool_response(id, name, response).await {
                            tracing::error!("Failed to send tool response: {e}");
                        }
                    }
                    Ok(GeminiEvent::Interrupted) => {
                        if let Some(p) = &player {
                            p.stop();
                        }
                        let _ = bus.send(AuraEvent::BargeIn);
                    }
                    Ok(GeminiEvent::Transcription { text }) => {
                        let _ = bus.send(AuraEvent::AssistantSpeaking { text });
                    }
                    Ok(GeminiEvent::TurnComplete) => {
                        // Audio response finished
                    }
                    Ok(GeminiEvent::Error { message }) => {
                        tracing::error!("Gemini error: {message}");
                        let _ = bus.send(AuraEvent::ActionFailed {
                            description: "Gemini".into(),
                            error: message,
                        });
                    }
                    Ok(GeminiEvent::Reconnecting { attempt }) => {
                        tracing::warn!(attempt, "Gemini reconnecting");
                        let _ = bus.send(AuraEvent::GeminiReconnecting { attempt });
                    }
                    Ok(GeminiEvent::Disconnected) => {
                        tracing::info!("Gemini session disconnected");
                        break;
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        tracing::warn!("Processor lagged by {n} Gemini events");
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                }
            }
        }
    }

    Ok(())
}

async fn run_overlay_bridge(
    bus: EventBus,
    proxy: EventLoopProxy<OverlayMessage>,
    cancel: CancellationToken,
) {
    let mut rx = bus.subscribe();

    loop {
        tokio::select! {
            _ = cancel.cancelled() => {
                let _ = proxy.send_event(OverlayMessage::Shutdown);
                break;
            }
            event = rx.recv() => {
                match event {
                    Ok(AuraEvent::ShowOverlay { content }) => {
                        let _ = proxy.send_event(OverlayMessage::Show);
                        let state = match content {
                            OverlayContent::Listening => OverlayState::Listening {
                                audio_levels: vec![0.5; 64],
                                phase: 0.0,
                                transition: 1.0,
                            },
                            OverlayContent::Processing => OverlayState::Processing {
                                phase: 0.0,
                                transition: 1.0,
                            },
                            OverlayContent::Response { text } => OverlayState::Response {
                                chars_revealed: text.len(),
                                text,
                                card_opacity: 1.0,
                            },
                            OverlayContent::Error { message } => OverlayState::Error {
                                message,
                                card_opacity: 1.0,
                                pulse_phase: 0.0,
                            },
                        };
                        let _ = proxy.send_event(OverlayMessage::SetState(state));
                    }
                    Ok(AuraEvent::HideOverlay) => {
                        let _ = proxy.send_event(OverlayMessage::Hide);
                    }
                    Ok(AuraEvent::AssistantSpeaking { text }) => {
                        let _ = proxy.send_event(OverlayMessage::Show);
                        let _ = proxy.send_event(OverlayMessage::SetState(OverlayState::Response {
                            chars_revealed: text.len(),
                            text,
                            card_opacity: 1.0,
                        }));
                    }
                    Ok(AuraEvent::BargeIn) => {
                        let _ = proxy.send_event(OverlayMessage::SetState(OverlayState::Listening {
                            audio_levels: vec![0.5; 64],
                            phase: 0.0,
                            transition: 1.0,
                        }));
                    }
                    Ok(AuraEvent::Shutdown) => {
                        let _ = proxy.send_event(OverlayMessage::Shutdown);
                        break;
                    }
                    Ok(_) => {}
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        tracing::warn!("Overlay bridge lagged by {n} events");
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                }
            }
        }
    }
}
```

**NOTE:** The `run_daemon` function above has a design issue with sharing the session's audio sender across threads. We need to add a method to `GeminiLiveSession` to get a cloneable audio sender. Add this to `session.rs`:

```rust
impl GeminiLiveSession {
    /// Get a cloneable sender for streaming audio to the session.
    pub fn audio_sender(&self) -> mpsc::Sender<Vec<f32>> {
        self.audio_tx.clone()
    }
}
```

Then simplify `run_daemon` to use `session.audio_sender()` directly instead of `clone_audio_sender` / `unsafe_clone_session_sender`.

**Step 2: Fix compilation**

Run: `cargo check -p aura-daemon`
Expected: May need to fix import paths and remove references to old modules. Fix any compilation errors iteratively.

**Step 3: Verify full workspace compiles**

Run: `cargo check --workspace`
Expected: All crates compile.

**Step 4: Commit**

```bash
git add -A
git commit -m "feat: rewrite daemon to use Gemini Live API for voice streaming"
```

---

### Task 9: Integration test — mock WebSocket server

**Files:**
- Create: `crates/aura-gemini/tests/session_test.rs`

**Step 1: Write the integration test**

Create `crates/aura-gemini/tests/session_test.rs`:

```rust
use std::net::SocketAddr;

use futures_util::{SinkExt, StreamExt};
use tokio::net::TcpListener;
use tokio_tungstenite::accept_async;
use tokio_tungstenite::tungstenite::Message;

use aura_gemini::config::GeminiConfig;
use aura_gemini::protocol::ServerMessage;
use aura_gemini::session::{GeminiEvent, GeminiLiveSession};

/// Start a mock WebSocket server that:
/// 1. Accepts setup → sends setupComplete
/// 2. Receives audio → sends back canned audio response
/// 3. Sends a tool call → expects tool response
async fn start_mock_server() -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut ws = accept_async(stream).await.unwrap();

        // 1. Receive setup
        let msg = ws.next().await.unwrap().unwrap();
        assert!(msg.to_text().unwrap().contains("setup"));

        // 2. Send setupComplete
        ws.send(Message::Text(r#"{"setupComplete":{}}"#.into()))
            .await
            .unwrap();

        // 3. Receive audio, send back audio response
        let _audio_msg = ws.next().await.unwrap().unwrap();

        // Send audio response (tiny PCM chunk, base64 of [0, 0])
        ws.send(Message::Text(
            r#"{"serverContent":{"modelTurn":{"parts":[{"inlineData":{"mimeType":"audio/pcm;rate=24000","data":"AAA="}}]},"turnComplete":true}}"#.into(),
        ))
        .await
        .unwrap();

        // 4. Send a tool call
        ws.send(Message::Text(
            r#"{"toolCall":{"functionCalls":[{"id":"call_1","name":"open_app","args":{"app_name":"Safari"}}]}}"#.into(),
        ))
        .await
        .unwrap();

        // 5. Receive tool response
        let tool_resp = ws.next().await.unwrap().unwrap();
        assert!(tool_resp.to_text().unwrap().contains("toolResponse"));

        // 6. Send turn complete
        ws.send(Message::Text(
            r#"{"serverContent":{"turnComplete":true}}"#.into(),
        ))
        .await
        .unwrap();

        // Keep connection open briefly
        tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;
    });

    addr
}

#[tokio::test]
async fn test_session_connect_and_receive_audio() {
    let addr = start_mock_server().await;

    let config = GeminiConfig {
        api_key: "test_key".into(),
        model: "models/test".into(),
        voice: "Kore".into(),
        system_prompt: "Test".into(),
        temperature: 0.9,
    };

    // Override the WebSocket URL to point to our mock server
    // This requires making websocket_url configurable or using a test helper
    // For now, test the protocol types and audio encoding directly

    // TODO: Add URL override support to GeminiConfig for testing
}

#[tokio::test]
async fn test_audio_encoding_roundtrip() {
    use aura_gemini::protocol::*;
    use base64::Engine;
    use base64::engine::general_purpose::STANDARD as BASE64;

    // Simulate the full encoding pipeline
    let pcm: Vec<f32> = vec![0.0, 0.5, -0.5, 1.0, -1.0];

    // Encode as we would in send_audio
    let mut bytes = Vec::with_capacity(pcm.len() * 2);
    for &sample in &pcm {
        let clamped = sample.clamp(-1.0, 1.0);
        let i16_val = (clamped * 32767.0) as i16;
        bytes.extend_from_slice(&i16_val.to_le_bytes());
    }
    let encoded = BASE64.encode(&bytes);

    // Simulate server response with the same data
    let server_json = format!(
        r#"{{"serverContent":{{"modelTurn":{{"parts":[{{"inlineData":{{"mimeType":"audio/pcm;rate=24000","data":"{encoded}"}}}}]}},"turnComplete":true}}}}"#
    );

    let msg: ServerMessage = serde_json::from_str(&server_json).unwrap();
    let content = msg.server_content.unwrap();
    let blob = &content.model_turn.unwrap().parts[0].inline_data.as_ref().unwrap();

    let decoded_bytes = BASE64.decode(&blob.data).unwrap();
    let decoded: Vec<f32> = decoded_bytes
        .chunks_exact(2)
        .map(|c| i16::from_le_bytes([c[0], c[1]]) as f32 / 32768.0)
        .collect();

    assert_eq!(decoded.len(), pcm.len());
    for (a, b) in pcm.iter().zip(decoded.iter()) {
        assert!((a - b).abs() < 0.001);
    }
}
```

**Step 2: Run tests**

Run: `cargo test -p aura-gemini`
Expected: Tests pass.

**Step 3: Commit**

```bash
git add crates/aura-gemini/tests/
git commit -m "test: add integration tests for Gemini session and audio encoding"
```

---

### Task 10: Update overlay bridge for new Gemini events

**Files:**
- Modify: `crates/aura-daemon/src/main.rs` (overlay bridge section)

**Step 1: Add handlers for new events**

In the `run_overlay_bridge` function, add handling for `GeminiConnected` and `GeminiReconnecting`:

Add these match arms in the overlay bridge:

```rust
Ok(AuraEvent::GeminiConnected) => {
    let _ = proxy.send_event(OverlayMessage::Show);
    let _ = proxy.send_event(OverlayMessage::SetState(OverlayState::Listening {
        audio_levels: vec![0.5; 64],
        phase: 0.0,
        transition: 1.0,
    }));
}
Ok(AuraEvent::GeminiReconnecting { attempt }) => {
    let _ = proxy.send_event(OverlayMessage::Show);
    let _ = proxy.send_event(OverlayMessage::SetState(OverlayState::Error {
        message: format!("Reconnecting (attempt {attempt})..."),
        card_opacity: 1.0,
        pulse_phase: 0.0,
    }));
}
```

**Step 2: Verify compilation**

Run: `cargo check -p aura-daemon`
Expected: Compiles.

**Step 3: Commit**

```bash
git add crates/aura-daemon/src/main.rs
git commit -m "feat: add overlay states for Gemini connection and reconnection"
```

---

### Task 11: Add `GeminiConfig` URL override for testing

**Files:**
- Modify: `crates/aura-gemini/src/config.rs`
- Modify: `crates/aura-gemini/tests/session_test.rs`

**Step 1: Add URL override to config**

Add an optional `ws_url_override` field to `GeminiConfig`:

```rust
pub struct GeminiConfig {
    pub api_key: String,
    pub model: String,
    pub voice: String,
    pub system_prompt: String,
    pub temperature: f32,
    /// Override WebSocket URL for testing. If None, uses the default Gemini endpoint.
    pub ws_url_override: Option<String>,
}
```

Update `from_env()` to set it to `None`, and `websocket_url()`:

```rust
pub fn websocket_url(&self) -> String {
    if let Some(url) = &self.ws_url_override {
        return url.clone();
    }
    format!(
        "wss://generativelanguage.googleapis.com/ws/google.ai.generativelanguage.v1alpha.GenerativeService.BidiGenerateContent?key={}",
        self.api_key
    )
}
```

**Step 2: Write full integration test using mock server**

Update `crates/aura-gemini/tests/session_test.rs` to use the URL override and test the full flow against the mock server.

**Step 3: Run tests**

Run: `cargo test -p aura-gemini`
Expected: All tests pass.

**Step 4: Commit**

```bash
git add crates/aura-gemini/
git commit -m "test: add mock WebSocket server for Gemini session integration tests"
```

---

### Task 12: Clean up and verify full workspace

**Step 1: Run full workspace check**

Run: `cargo check --workspace`
Expected: All crates compile.

**Step 2: Run all tests**

Run: `cargo test --workspace`
Expected: All tests pass.

**Step 3: Run clippy**

Run: `cargo clippy --workspace -- -D warnings`
Expected: No warnings.

**Step 4: Format**

Run: `cargo fmt --all`

**Step 5: Final commit**

```bash
git add -A
git commit -m "chore: clean up workspace after Gemini Live API integration"
```

---

## Summary

| Task | Description | Estimated Steps |
|------|-------------|-----------------|
| 1 | Scaffold `aura-gemini` crate | 6 |
| 2 | Protocol types + serialization tests | 3 |
| 3 | Config module | 3 |
| 4 | Tool declarations + action mapping | 4 |
| 5 | WebSocket session client | 3 |
| 6 | Strip aura-voice (remove STT/VAD/TTS) | 6 |
| 7 | Remove aura-llm, decouple bridge | 5 |
| 8 | Rewrite daemon main.rs | 4 |
| 9 | Integration tests with mock WS server | 3 |
| 10 | Update overlay bridge | 3 |
| 11 | Config URL override for tests | 4 |
| 12 | Full workspace cleanup | 5 |
