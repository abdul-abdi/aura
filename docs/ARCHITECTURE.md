# Aura Architecture

> Deep technical reference for the Aura codebase.
> For a quick overview and setup instructions, see the [README](../README.md).

---

## 1. System Overview

Aura is a voice-first AI desktop companion for macOS. It captures microphone audio, streams it to the Gemini Live API over a persistent WebSocket, receives tool-call instructions and audio responses, executes macOS automation (mouse, keyboard, AppleScript), and renders the conversation in a floating panel anchored to the menu bar.

The system is split into 9 Rust crates (the daemon and its libraries) and 1 SwiftUI app (the frontend). The daemon runs headless when launched by the SwiftUI app, or standalone with its own Cocoa menu bar. Communication between the SwiftUI app and the daemon uses a Unix domain socket with JSONL framing.

See the full architecture diagram in [architecture.mmd](architecture.mmd).

---

## 2. Crate Map

| Crate | Lines | Description | Key files |
|-------|------:|-------------|-----------|
| **aura-daemon** | ~2,890 | Orchestrator. CLI entry point, tokio runtime, Gemini event loop, tool dispatch, mic bridge, screen capture loop, IPC server. | `main.rs` (1,734), `bus.rs`, `event.rs`, `ipc.rs`, `protocol.rs`, `setup.rs` |
| **aura-gemini** | ~1,879 | WebSocket client for the Gemini Live API. Connection lifecycle, reconnection with exponential backoff, setup message construction, server message parsing, audio encoding/decoding. | `session.rs` (781), `protocol.rs` (559), `config.rs` (279), `tools.rs` (254) |
| **aura-voice** | ~687 | Audio capture via cpal (prefers 48kHz, adapts to device native rate; rubato SincFixedIn resampler to 16kHz) and playback via rodio (24kHz, dedicated thread, 80ms pre-buffer). | `audio.rs`, `playback.rs`, `wakeword.rs` |
| **aura-screen** | ~490 | Screen capture (CGDisplay BGRA to RGB to JPEG 80%, max 1920px), FNV-1a change detection (8192-pixel sample), screen context via osascript, post-action capture trigger. | `capture.rs`, `context.rs`, `macos.rs` |
| **aura-bridge** | ~653 | AppleScript/JXA execution through osascript with polling-based timeout (max 60s), dangerous pattern blocklist (10 shell + 3 JXA patterns), obfuscated command detection (string concatenation/variable splitting). Output capped at 10 KB. | `script.rs`, `automation.rs` |
| **aura-input** | ~525 | Synthetic mouse and keyboard via CGEvent. Mouse: move, click (left/right, 1-3x), scroll (pixel units), drag (with 50ms inter-event delays). Keyboard: type_text (per-character Unicode via UTF-16), press_key (virtual keycodes + modifier flags). | `mouse.rs`, `keyboard.rs`, `accessibility.rs` |
| **aura-memory** | ~387 | SQLite persistence (WAL mode, NORMAL synchronous, foreign keys). Three tables: `sessions`, `messages`, `settings`. Session resumption handle storage. Pruning, recent tool-use summaries, VACUUM support. | `store.rs` |
| **aura-menubar** | ~1,014 | Cocoa FFI menu bar UI. NSStatusItem with 5 dot colors, NSPopover for chat transcript, context menu (Reconnect / Quit), NSTimer polling (50ms), sleep/wake observer. | `status_item.rs`, `app.rs`, `popover.rs` |
| **aura-proxy** | ~251 | Cloud Run WebSocket relay. axum server, SHA-256 constant-time token auth (via `subtle` crate), `/health` endpoint, `/ws` relay with 10-connection semaphore, 1 MiB max frame size, Nagle disabled. | `lib.rs`, `relay.rs`, `main.rs` |

**SwiftUI frontend** (`AuraApp/`, ~8,282 lines Swift): Floating panel UI, onboarding flow, Unix domain socket IPC client, daemon process management. See [Section 9](#9-swiftui-frontend).

---

## 3. Threading Model

### Main thread
The Cocoa run loop (`NSApplication::run`). All AppKit calls -- NSStatusItem creation, icon updates, popover show/hide, context menu -- must happen here. In standalone mode, `aura-menubar` owns this thread. In headless mode (launched by the SwiftUI app), this thread is unused.

### Tokio runtime (background thread)
Spawned on a `std::thread` so the main thread remains free for AppKit. Hosts:
- Gemini WebSocket connection loop (`session.rs::connection_loop`)
- Audio mic bridge (std::sync::mpsc to tokio::mpsc adapter with RMS energy gate)
- Screen capture loop (2 fps / 500ms interval, change-detection gated)
- Tool dispatch (tokio::spawn per tool call, semaphore-limited to 8 concurrent)
- IPC server (Unix domain socket, one tokio task per connected client)
- Event bus subscriber (processes GeminiEvent, updates menu bar, persists to SQLite)

### Playback thread
A dedicated `std::thread` named `aura-playback` (spawned by `AudioPlayer::new()`). Required because rodio's `OutputStream` is `!Send` -- it must be created and used on a single thread. Receives `PlaybackCommand` messages via `std::sync::mpsc`. Manages pre-buffer (80ms at 24kHz = 1,920 samples) before starting the Sink to absorb network jitter.

### cpal callback thread
Managed by the cpal audio backend. Delivers variable-length f32 buffers at the device's native rate (typically 48kHz). The callback accumulates samples in a staging buffer and processes full 1024-frame chunks through the rubato SincFixedIn resampler, outputting 16kHz mono PCM via `std::sync::mpsc`.

---

## 4. Data Flow

### 4.1 Voice Input (Mic to Gemini)

```
Microphone
  --> cpal callback (48kHz f32, buffer size 256)
  --> rubato SincFixedIn resampler (48kHz -> 16kHz, chunk size 1024)
  --> std::sync::mpsc (unbounded, from cpal callback thread)
  --> mic bridge (tokio task, std mpsc -> tokio mpsc adapter)
  --> full mute while speaker is active (unconditional drop); RMS energy gate (threshold: 0.04) when Gemini is speaking but playback hasn't started
  --> tokio::mpsc (capacity: 256)
  --> GeminiLiveSession::audio_tx (capacity: 64)
  --> f32 -> PCM 16-bit LE -> base64 encoding
  --> WebSocket Text frame -> Gemini Live API
```

The energy gate uses adaptive calibration: the first ~100 chunks (~500ms) establish an ambient noise baseline. The calibrated threshold is clamped between 0.02 and 0.15.

### 4.2 Screen Capture (Display to Gemini)

```
CGDisplay::image() (active display under mouse cursor)
  --> BGRA raw pixels
  --> RGB conversion (row-by-row, skip alpha)
  --> FNV-1a hash (8192-pixel sample, step = total_pixels / 8192)
  --> skip if hash matches previous frame (no change)
  --> downscale if width > 1920px (image::resize, Triangle filter)
  --> JPEG encode (quality 80)
  --> base64
  --> GeminiLiveSession::video_tx (capacity: 8, try_send -- drops frames on backpressure)
  --> WebSocket Text frame -> Gemini Live API
```

Post-action capture: After each tool execution, a `CaptureTrigger` (Arc<AtomicBool>) signals the screen capture loop to take an immediate screenshot, bypassing the 500ms interval. A `tokio::sync::Notify` is also used to immediately wake the capture loop. This lets Gemini see the result of its action without waiting.

### 4.3 Tool Execution (Gemini to macOS)

```
Gemini Live API
  --> WebSocket frame (Text or Binary, both handled identically via UTF-8)
  --> ServerMessage parsing
  --> GeminiEvent::ToolCall { id, name, args }
  --> broadcast to event loop subscriber in main.rs
  --> tokio::Semaphore::acquire (8 permits max)
  --> tokio::spawn tool handler
  --> dispatch by name:
      run_applescript  -> ScriptExecutor (osascript, max 60s timeout)
      get_screen_context -> MacOSScreenReader (osascript queries + pbpaste)
      shutdown_aura    -> CancellationToken::cancel (inline, breaks event loop)
      move_mouse       -> aura_input::mouse::move_mouse (CGEvent)
      click            -> aura_input::mouse::click (CGEvent)
      type_text        -> aura_input::keyboard::type_text (CGEvent, 10ms per char)
      press_key        -> aura_input::keyboard::press_key (CGEvent + modifier flags)
      scroll           -> aura_input::mouse::scroll (CGEvent, pixel units)
      drag             -> aura_input::mouse::drag (CGEvent, 50ms delays)
  --> JSON result
  --> GeminiLiveSession::tool_response_tx (capacity: 16)
  --> ToolResponseMessage -> WebSocket Text frame -> Gemini
  --> CaptureTrigger::trigger() (immediate post-action screenshot)
```

### 4.4 Audio Output (Gemini to Speaker)

```
Gemini Live API
  --> ServerMessage.server_content.model_turn.parts[].inline_data (audio/pcm)
  --> base64 decode
  --> PCM 16-bit LE bytes -> f32 conversion (/32768.0)
  --> GeminiEvent::AudioResponse { samples }
  --> AudioPlayer::start_stream(24_000) on first chunk of new turn
  --> AudioPlayer::append(samples)
  --> std::sync::mpsc -> playback thread
  --> PreBuffer accumulates ~80ms (1,920 samples at 24kHz)
  --> rodio::Sink::append(SamplesBuffer) -> Speaker
```

Barge-in: When `GeminiEvent::Interrupted` is received, the event loop calls `AudioPlayer::stop()` which drops the current Sink and clears the pre-buffer immediately.

### 4.5 IPC (Daemon to SwiftUI)

```
Daemon event loop
  --> broadcast::Sender<DaemonEvent> (capacity: 256)
  --> IPC server (Unix domain socket at ~/Library/Application Support/aura/daemon.sock)
  --> per-client tokio task subscribes to broadcast
  --> serde_json::to_string(&DaemonEvent) + "\n"
  --> tokio::io::AsyncWriteExt::write_all -> client socket

SwiftUI app
  --> DaemonConnection (raw Unix socket, non-blocking reads)
  --> 10ms poll loop (EAGAIN/EWOULDBLOCK handling)
  --> JSONL line splitting on "\n"
  --> JSONDecoder.decode(DaemonEvent.self)
  --> AppState.handleEvent() -> @Observable state updates -> SwiftUI re-render
```

---

## 5. Event Bus

The internal event bus (`crates/aura-daemon/src/bus.rs`) uses `tokio::sync::broadcast` with a capacity of 256. It carries `AuraEvent` variants:

| Variant | Meaning |
|---------|---------|
| `WakeWordDetected` | Wake word triggered (reserved for future use) |
| `GeminiConnected` | WebSocket connection established |
| `GeminiReconnecting { attempt }` | Connection lost, retry in progress |
| `BargeIn` | User interrupted assistant speech |
| `ToolExecuted { name, success, output }` | Tool finished execution |
| `Shutdown` | Graceful shutdown requested |

Broadcast was chosen over mpsc because multiple independent subscribers (the daemon event loop and the Gemini processor) each need every event. Dropped events due to lagging receivers are acceptable -- the bus is best-effort.

---

## 6. IPC Protocol

### Transport
Unix domain socket at `~/Library/Application Support/aura/daemon.sock`. JSONL encoding (one JSON object per line, terminated by `\n`). Serde uses internally-tagged representation (`#[serde(tag = "type", rename_all = "snake_case")]`).

### DaemonEvent (daemon -> UI)

| Variant | Fields | Purpose |
|---------|--------|---------|
| `dot_color` | `color: gray\|green\|amber\|red`, `pulsing: bool` | Status bar icon update |
| `transcript` | `role: user\|assistant`, `text: String`, `done: bool` | Conversation transcript (streaming) |
| `tool_status` | `name: String`, `status: running\|completed\|failed`, `output?: String` | Tool execution lifecycle |
| `status` | `message: String` | Status bar text update |
| `shutdown` | (none) | Daemon is shutting down |

Example: `{"type":"transcript","role":"assistant","text":"Done.","done":true}`

### UICommand (UI -> daemon)

| Variant | Fields | Purpose |
|---------|--------|---------|
| `send_text` | `text: String` | Send text message to Gemini |
| `toggle_mic` | (none) | Toggle microphone on/off |
| `reconnect` | (none) | Force Gemini reconnection |
| `shutdown` | (none) | Request graceful shutdown |

Example: `{"type":"send_text","text":"open Safari"}`

---

## 7. Reconnection Strategy

The reconnection logic lives in `aura-gemini/src/session.rs::connection_loop` and implements a layered strategy:

### Exponential Backoff
- **Initial delay**: 200ms
- **Maximum delay**: 30,000ms (30s)
- **Formula**: `min(200ms * 2^(attempt-1), 30s)`
- **Jitter**: +/-25% per attempt (timestamp-based hash: `nanos * 2654435761 ^ attempt * 7`, mod 1000, scaled to 0.75-1.25 factor)
- **Max attempts**: 5

### Stability Threshold
If a connection was alive for >= 30 seconds after `setupComplete`, the attempt counter resets to 0. This prevents a long-running connection that drops from being penalized with exponential backoff.

### goAway Handling
When the Gemini server sends a `goAway` message (requesting client migration), the connection is dropped and reconnected immediately without incrementing the attempt counter or applying backoff.

### Session Resumption
- A `SessionResumptionConfig` with an optional handle is sent in every setup message.
- The server responds with `SessionResumptionUpdate` containing an optional `newHandle`.
- The handle is stored in the SQLite `settings` table (key: `resumption_handle`).
- On reconnection, the stored handle is included in the setup message.
- If the server rejects a stale handle (`SESSION_NOT_FOUND` in WebSocket close reason), the handle is cleared from both memory and SQLite, and a fresh session begins.

### User-Initiated Reconnect
A dedicated `mpsc` channel (`reconnect_tx`/`reconnect_rx`) allows the UI to force an immediate reconnection. This drops the current WebSocket (via `tokio::select!` cancellation) and resets the attempt counter.

### Post-Exhaustion Parking
When all 5 attempts are exhausted, instead of breaking out of the loop (which would drop `reconnect_rx` and make future IPC reconnect signals fail silently), the loop parks on `tokio::select!` waiting for either a user-initiated reconnect signal or cancellation.

---

## 8. Gemini Live API Integration

### Endpoint
```
wss://generativelanguage.googleapis.com/ws/google.ai.generativelanguage.v1beta.GenerativeService.BidiGenerateContent
```
Note: `v1beta`, not `v1` or `v1alpha`. The native audio model is only available on this endpoint.

### Model
`models/gemini-2.5-flash-native-audio-preview-12-2025`

### Setup Message
Sent immediately after WebSocket handshake. Contains:
- **Model** identifier
- **Generation config**: temperature 0.7, response modality AUDIO, voice "Kore", media resolution HIGH
- **System instruction**: ~2KB prompt defining Aura's personality, vision capabilities, tool usage strategy, and safety rules (destructive action confirmation guardrail appended at runtime)
- **Tools**: 3 Tool objects (see below)
- **Session resumption**: Optional handle from previous session
- **Context window compression**: Sliding window (server-managed)

### Tool Declarations (3 Tool objects, 9 function declarations)

**Function declarations** (first Tool object):
1. `run_applescript` -- script, language (applescript/javascript), timeout_secs
2. `get_screen_context` -- no params
3. `shutdown_aura` -- no params
4. `move_mouse` -- x, y
5. `click` -- x, y, button (left/right), click_count
6. `type_text` -- text
7. `press_key` -- key, modifiers (cmd/shift/alt/ctrl)
8. `scroll` -- dx, dy
9. `drag` -- from_x, from_y, to_x, to_y

**Google Search** (second Tool object): Enables grounding for current events, weather, facts.

**Code Execution** (third Tool object): Server-side Python for math and data analysis.

### Audio Protocol
- **Input**: PCM 16-bit LE, mono, 16kHz. Sent as base64 in `realtimeInput.audio` with MIME type `audio/pcm;rate=16000`.
- **Output**: PCM 16-bit LE, mono, 24kHz. Received as base64 in `serverContent.modelTurn.parts[].inlineData` with MIME type `audio/pcm`.
- **Chunk size**: 100ms of audio per message.

### WebSocket Frame Handling
The Gemini server sends both Text and Binary WebSocket frames. Both are treated identically: Binary frames are converted to UTF-8 strings and parsed as JSON. Unparseable messages are logged and skipped (not fatal).

### Keepalive
A WebSocket Ping is sent every 20 seconds to prevent idle disconnection.

---

## 9. SwiftUI Frontend

The SwiftUI app (`AuraApp/`) is a menu bar application (`.accessory` activation policy, no dock icon) with ~8,300 lines of Swift across 16 source files.

### Key Components

| File | Purpose |
|------|---------|
| `AuraApp.swift` | App entry point, `AppDelegate` (NSApplicationDelegateAdaptor), status item, floating panel, global hotkey (Cmd+Shift+A), sleep/wake observer, daemon process management |
| `AppState.swift` | Central `@Observable` state. Onboarding state machine, DaemonEvent processing, conversation message buffer (max 200), dot color/pulsing state |
| `DaemonConnection.swift` | Raw Unix domain socket client. Non-blocking reads (O_NONBLOCK + 10ms poll), JSONL parsing, auto-reconnect with exponential backoff (500ms to 15s, 20 attempts) |
| `ContentView.swift` | Root view. Routes between onboarding steps and main conversation UI based on `AppState.onboardingStep` |
| `WelcomeView.swift` | First-run API key entry. Validates key length >= 20, ASCII-only. Saves to `~/.config/aura/config.toml` with 0600 permissions |
| `PermissionsView.swift` | Permission grant UI for Microphone, Screen Recording, Accessibility. 2-second polling timer auto-advances when all permissions are granted |
| `FloatingPanel.swift` | NSPanel subclass (`.nonactivatingPanel`, `.utilityWindow`). Anchored below the status item, with spring animations on show/hide |
| `ConversationView.swift` | ScrollView of message bubbles (user, assistant, tool status) |
| `Protocol.swift` | Mirror of the Rust IPC protocol types (`DaemonEvent`, `UICommand`) as Swift `Codable` enums |

### Onboarding Flow
1. **Welcome** -- API key input, saved to `~/.config/aura/config.toml`
2. **Permissions** -- Mic, Screen Recording, Accessibility grants (2s poll auto-advances)
3. **Done** -- daemon launched, IPC connection established

State is persisted via `UserDefaults` key `aura.onboardingComplete`. If the config file is deleted after onboarding, the flow restarts.

### Daemon Process Management
The SwiftUI app launches `aura-daemon --headless` as a child process. It looks for the binary first in the app bundle (`Contents/MacOS/aura-daemon`), then at `../target/release/aura-daemon` (development). If the daemon crashes (non-zero exit), it is relaunched after 1 second and the IPC connection is re-established.

### Panel Behavior
- Left-click on status item toggles the floating panel
- Right-click shows context menu (Toggle Panel, Reconnect, Quit Aura)
- Escape key dismisses the panel
- Global hotkey Cmd+Shift+A toggles from anywhere
- Panel positioned below status item, 380x520pt, frosted glass background (`.ultraThinMaterial`)

---

## 10. Security Model

### Script Execution Safety
`aura-bridge/src/script.rs` implements defense-in-depth:

1. **Shell pattern blocklist** (10 patterns): `rm -rf`, `rm -r`, `sudo`, `mkfs`, `dd if=`, `chmod 777`, `:(){ :|:`, `> /dev/sd`, `unlink `, `diskutil erase`
2. **JXA pattern blocklist** (3 patterns): `$.system`, `objc.import`, `.doscript(`
3. **Obfuscation detection**: Multi-atom patterns (`rm` + `-rf`, `dd` + `if=`, `chmod` + `777`) catch dangerous commands split across string concatenation or variables. Uses standalone token matching to avoid false positives (e.g., "rm" in "inform" is not flagged).
4. **Output truncation**: Stdout/stderr capped at 10,240 bytes.
5. **Timeout**: Maximum 60 seconds (configurable per call, default 30s). Uses `Child::kill()` + `Child::wait()` (reap zombie) on timeout.

### Destructive Action Guardrail
A runtime-injected system prompt addendum instructs Gemini to confirm with the user before performing destructive actions (file deletion, emptying trash, quitting unsaved apps, reformatting drives).

### Proxy Authentication
`aura-proxy` uses SHA-256 hashing + constant-time comparison (via the `subtle` crate) for token authentication. This prevents timing side-channels on token validation. Connection count is limited to 10 concurrent WebSocket sessions via a tokio Semaphore.

### Input Validation
- Tool call arguments are validated at dispatch: coordinates must be finite, click count capped at 3, scroll capped at +/-1000, type_text capped at 10,000 characters.
- API key validation accepts any non-empty string (relaxed from a previous prefix-matching check).
- Config file permissions are set to 0600 (owner read/write only).

---

## 11. Configuration

### Config File
`~/.config/aura/config.toml` (also checked at platform config dir `~/Library/Application Support/aura/config.toml`):
```toml
api_key = "your-gemini-api-key"
proxy_url = "wss://your-proxy.run.app/ws"       # optional
proxy_auth_token = "your-secret-token"           # optional
```

### Environment Variables

| Variable | Purpose | Default |
|----------|---------|---------|
| `GEMINI_API_KEY` | Gemini API key (overrides config file) | -- |
| `AURA_PROXY_URL` | WebSocket relay URL | Direct to Gemini |
| `AURA_PROXY_AUTH_TOKEN` | Auth token for proxy | None |
| `PORT` | Proxy server listen port | 8080 |
| `RUST_LOG` | Log filter (e.g., `debug`, `aura_gemini=trace`) | `info` |
| `AURA_CLOUD_REGION` | GCP region for deploy-proxy.sh | `us-central1` |

### Data Directory
`~/Library/Application Support/aura/`:
- `aura.db` -- SQLite database (sessions, messages, settings)
- `daemon.sock` -- Unix domain socket for IPC
- `models/` -- Wake word model files (`.rpw`)
- `logs/` -- Log files

---

## 12. Build & Deployment

### Requirements
- Rust 1.85+ (edition 2024)
- Xcode (for SwiftUI app and macOS SDK headers)
- macOS 13+ (Ventura)

### Commands
```bash
cargo build --release -p aura-daemon     # Daemon binary
swift build -c release                    # SwiftUI app (from AuraApp/)
bash scripts/bundle.sh                   # -> target/release/Aura.app
bash scripts/deploy-proxy.sh             # Deploy proxy to Cloud Run
```

### App Bundle Structure
```
Aura.app/
  Contents/
    MacOS/
      AuraApp          # SwiftUI frontend (entry point)
      aura-daemon      # Rust daemon binary
    Info.plist
    Resources/
```

The SwiftUI app launches the co-bundled daemon as a child process with `--headless` flag.
