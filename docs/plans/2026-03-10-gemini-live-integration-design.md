# Gemini Live API Integration Design

**Date:** 2026-03-10
**Status:** Approved

## Goal

Replace the entire local voice pipeline (Silero VAD → Whisper STT → Ollama LLM → Kokoro TTS) with a single Gemini Live API WebSocket connection. Audio goes in, audio comes out. Intent classification moves to Gemini function calling.

## Decisions

- **Full replacement**: No local STT/LLM/TTS. Gemini handles everything.
- **Gemini handles intents**: Function calling replaces Ollama intent parser.
- **No client-side VAD**: Gemini handles speech detection server-side.
- **Env var config**: `GEMINI_API_KEY` from environment. Fail fast if missing.
- **Graceful error + retry**: Exponential backoff on failure. No offline fallback.
- **Starting crate**: `gemini-live-api` (v0.1.1) with raw `tokio-tungstenite` as fallback.

## Architecture

### Current Pipeline (being replaced)

```
Mic (cpal) → VAD (Silero) → STT (Whisper) → LLM (Ollama) → TTS (Kokoro) → Playback (rodio)
     6 stages, all local, ~2-3s latency
```

### New Pipeline

```
Mic (cpal 16kHz PCM) → base64 encode → WebSocket → Gemini Live API → base64 decode → Playback (rodio 24kHz)
     3 stages, single round-trip, <1s latency
```

### Component Diagram

```
┌─────────────┐     base64 PCM 16kHz      ┌──────────────────┐
│  cpal mic    │ ──────────────────────────▶│                  │
│  capture     │                            │  GeminiLive      │
│              │     base64 PCM 24kHz       │  Session         │
│  rodio       │ ◀─────────────────────────│  (WebSocket)     │
│  playback    │                            │                  │
└─────────────┘     tool calls              └────────┬─────────┘
                         │                           │
                         ▼                           │
                  ┌──────────────┐    tool responses  │
                  │ MacOSExecutor│ ◀─────────────────┘
                  │ (aura-bridge)│
                  └──────────────┘
```

### What Stays

- `cpal` audio capture (mic input, resampling to 16kHz)
- `rodio` audio playback (output, barge-in stop)
- `aura-overlay` (visual feedback)
- `aura-bridge` / `aura-screen` (macOS actions, triggered via Gemini function calls)

### What Goes

- `voice_activity_detector` (Silero VAD)
- `whisper-rs` (STT)
- `kokoro-tts` (TTS)
- `OllamaProvider` / `IntentParser` / `Conversation` (all in aura-llm)

## Crate & Module Structure

### `aura-voice` (slimmed down)

- `audio.rs` — mic capture via cpal (unchanged)
- `playback.rs` — rodio playback with barge-in (unchanged, configured for 24kHz)
- Removed: `vad.rs`, `stt.rs`, `tts.rs`, `pipeline.rs`

### `aura-gemini` (replaces `aura-llm`)

- `session.rs` — `GeminiLiveSession`: WebSocket lifecycle, connect/reconnect, send/receive
- `protocol.rs` — all Gemini protocol types (setup, realtimeInput, serverContent, toolCall, etc.)
- `tools.rs` — function declarations for intents (open_app, tile_windows, search_files, launch_url, summarize_screen)
- `config.rs` — session config (model name, voice, system instructions, generation params)

### `aura-daemon` (simplified orchestration)

- Mic capture → `GeminiLiveSession` (send audio chunks)
- `GeminiLiveSession` → playback (receive audio chunks)
- `GeminiLiveSession` → tool calls → `MacOSExecutor` → tool responses back to session
- Overlay events remain the same

## GeminiLiveSession — Core WebSocket Client

### Lifecycle

```
new() → connect() → setup exchange → streaming loop → disconnect/reconnect
```

### Connection Flow

1. Read `GEMINI_API_KEY` from env (fail fast at startup if missing)
2. Open WebSocket to `wss://generativelanguage.googleapis.com/ws/google.ai.generativelanguage.v1alpha.GenerativeService.BidiGenerateContent?key={API_KEY}`
3. Send `setup` message (model, voice, system instructions, tool declarations, SlidingWindow compression, sessionResumption)
4. Wait for `setupComplete` from server
5. Enter streaming loop

### Streaming Loop (two concurrent tasks)

- **Send task**: reads PCM chunks from cpal via `tokio::sync::mpsc`, base64-encodes, sends as `realtimeInput.mediaChunks` messages
- **Receive task**: reads WebSocket messages, dispatches by type:
  - `serverContent.modelTurn.parts[].inlineData` → decode audio → send to rodio playback
  - `serverContent.interrupted` → signal playback to stop (barge-in)
  - `toolCall` → extract function name + args → execute via `MacOSExecutor` → send `toolResponse`
  - `goAway` → trigger reconnection
  - `sessionResumptionUpdate` → store latest resumption token

### Public API

```rust
pub struct GeminiLiveSession { ... }

impl GeminiLiveSession {
    pub async fn connect(config: GeminiConfig) -> Result<Self>;
    pub async fn send_audio(&self, pcm_16khz: &[f32]) -> Result<()>;
    pub fn audio_output_rx(&self) -> broadcast::Receiver<Vec<f32>>;
    pub fn event_rx(&self) -> broadcast::Receiver<GeminiEvent>;
    pub async fn disconnect(&self) -> Result<()>;
}

pub enum GeminiEvent {
    Connected,
    AudioResponse { samples: Vec<f32> },
    ToolCall { id: String, name: String, args: serde_json::Value },
    Interrupted,
    Transcription { text: String },
    Error { message: String },
    Reconnecting { attempt: u32 },
    Disconnected,
}
```

## Function Calling / Tool Declarations

### Tool Declarations (sent in setup)

```json
{
  "tools": [{
    "functionDeclarations": [
      {
        "name": "open_app",
        "description": "Open an application by name on macOS",
        "parameters": {
          "type": "object",
          "properties": {
            "app_name": { "type": "string", "description": "Name of the app to open" }
          },
          "required": ["app_name"]
        }
      },
      {
        "name": "search_files",
        "description": "Search for files on the user's computer",
        "parameters": {
          "type": "object",
          "properties": {
            "query": { "type": "string", "description": "Search query" }
          },
          "required": ["query"]
        }
      },
      {
        "name": "tile_windows",
        "description": "Arrange windows in a tiling layout",
        "parameters": {
          "type": "object",
          "properties": {
            "layout": { "type": "string", "enum": ["left_half", "right_half", "maximize", "split"] }
          },
          "required": ["layout"]
        }
      },
      {
        "name": "launch_url",
        "description": "Open a URL in the default browser",
        "parameters": {
          "type": "object",
          "properties": {
            "url": { "type": "string", "description": "URL to open" }
          },
          "required": ["url"]
        }
      },
      {
        "name": "summarize_screen",
        "description": "Capture and describe what's currently visible on screen",
        "parameters": { "type": "object", "properties": {} }
      }
    ]
  }]
}
```

### Execution Flow

1. Gemini receives audio → decides it's an intent (e.g., "open Safari")
2. Server sends `toolCall { functionCalls: [{ id, name: "open_app", args: { app_name: "Safari" } }] }`
3. Aura dispatches to `MacOSExecutor::execute(action)` (existing aura-bridge code)
4. Aura sends `toolResponse { functionResponses: [{ id, response: { success: true } }] }`
5. Gemini speaks a confirmation as audio

## Audio Data Flow & Encoding

### Sending Audio (mic → Gemini)

```
cpal callback (device sample rate, f32)
  → resample to 16kHz (existing AudioCapture logic)
  → f32 samples in [-1.0, 1.0]
  → convert to i16 (multiply by 32767, clamp)
  → little-endian bytes
  → base64 encode
  → { "realtimeInput": { "mediaChunks": [{ "mimeType": "audio/pcm;rate=16000", "data": "<base64>" }] } }
  → WebSocket text frame
```

Chunk size: ~100ms (1600 samples at 16kHz = 3200 bytes raw = ~4.3KB base64).

### Receiving Audio (Gemini → playback)

```
WebSocket text frame
  → { "serverContent": { "modelTurn": { "parts": [{ "inlineData": { "mimeType": "audio/pcm;rate=24000", "data": "<base64>" } }] } } }
  → base64 decode → LE bytes → i16 → f32 (÷ 32768)
  → rodio playback at 24kHz
```

### Barge-in Flow

```
User starts speaking while Gemini is responding
  → Gemini detects voice activity (server-side)
  → Server sends: { "serverContent": { "interrupted": true } }
  → Pending function calls get toolCallCancellation with cancelled IDs
  → Aura calls AudioPlayer::stop()
  → Overlay shows "Listening" state
  → Gemini processes new user speech
```

### Session Management

```
Connect → Setup (with SlidingWindow compression + sessionResumption enabled)
  → setupComplete
  → Streaming loop
  → Server sends SessionResumptionUpdate periodically → store token
  → At ~10 min: connection drops or goAway
  → Reconnect with stored resumption token → seamless continuation
  → No session time limit (compression enabled)
```

Key limits:
- Connection lifetime: ~10 minutes (WebSocket reset)
- Session lifetime: unlimited (with SlidingWindow compression)
- Context window: 128k tokens
- Resumption tokens valid for 2 hours after disconnection

## Error Handling

| Error | Detection | Response |
|-------|-----------|----------|
| Missing `GEMINI_API_KEY` | Startup check | Fail fast with clear error message |
| WebSocket connect failure | `tokio-tungstenite` error | Exponential backoff (1s→2s→4s→...→30s, max 10 retries) |
| Connection drop / `goAway` | Server message or WS close | Reconnect with session resumption token |
| 10-min connection reset | Track connection age + `goAway` | Seamless reconnect via resumption handle |
| API quota exceeded (429) | HTTP status on WS upgrade | Backoff + emit `AuraEvent::Error` to overlay |
| Invalid tool call args | Deserialization failure | Send error `toolResponse`, Gemini recovers |
| `MacOSExecutor` failure | Action returns `Err` | Send failure `toolResponse` with error message |
| Playback device lost | rodio error | Log warning, audio responses dropped until device returns |

### Overlay States

- `Connecting` — initial WebSocket handshake
- `Listening` — session active, streaming audio
- `Responding` — receiving audio from Gemini
- `Reconnecting` — connection lost, retrying
- `Error` — fatal failure (no API key, max retries exceeded)

## Testing Strategy

1. **Unit tests** — `protocol.rs` message serialization/deserialization (JSON round-trips)
2. **Integration tests** — mock WebSocket server (`tokio-tungstenite` server) that:
   - Accepts setup, sends `setupComplete`
   - Receives audio chunks, responds with canned audio
   - Sends `toolCall`, validates `toolResponse`
   - Simulates `interrupted` and `goAway`
3. **E2E tests** — mock WS server in `aura-daemon` tests (similar to existing wiremock pattern)
4. **Manual testing** — real Gemini API with mic input (gated behind `GEMINI_API_KEY`)

## Dependencies

### Add (`aura-gemini/Cargo.toml`)

```toml
tokio-tungstenite = "0.24"
base64 = "0.22"
serde = { version = "1", features = ["derive"] }
serde_json = "1"
tokio = { version = "1", features = ["full"] }
anyhow = "1"
tracing = "0.1"
```

### Remove (from `aura-voice`)

```toml
# whisper-rs
# voice_activity_detector
# kokoro-tts
```

### Keep (in `aura-voice`)

```toml
cpal = "0.15"
rodio = "0.20"
```

## Model

`gemini-live-2.5-flash-native-audio` (GA on Vertex AI, preview on Gemini API)

Note: `gemini-live-2.5-flash-preview-native-audio-09-2025` is deprecated March 19, 2026. Use the stable model name.
