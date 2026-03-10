# Aura Architecture

> Voice-first AI desktop companion for macOS, powered by Gemini Live.
> Pure Rust, native Cocoa FFI, no Electron.

## System Overview

```mermaid
flowchart LR
    MIC[Microphone] -->|16kHz PCM| DAEMON["aura-daemon<br/>(orchestrator)"]
    DAEMON <-->|WebSocket| GEMINI["Gemini Live API"]
    DAEMON -->|24kHz PCM| SPEAKER[Speaker]
    DAEMON -->|tool calls| MACOS["macOS<br/>AppleScript / CGEvent"]
    DAEMON -->|status| MENUBAR["Menu Bar Dot"]
    DAEMON -->|persist| DB[("SQLite")]
    DISPLAY[Display] -->|screenshots| DAEMON
```

---

## Crate Map (9 crates)

```mermaid
flowchart TB
    subgraph WORKSPACE["Rust Workspace"]
        direction TB

        DAEMON["aura-daemon<br/>Orchestrator"]

        DAEMON --> GEMINI["aura-gemini<br/>WebSocket Client"]
        DAEMON --> VOICE["aura-voice<br/>Audio I/O"]
        DAEMON --> SCREEN["aura-screen<br/>Vision Loop"]
        DAEMON --> BRIDGE["aura-bridge<br/>Script Executor"]
        DAEMON --> INPUT["aura-input<br/>CGEvent Control"]
        DAEMON --> MENUBAR["aura-menubar<br/>Cocoa UI"]
        DAEMON --> MEMORY["aura-memory<br/>SQLite Storage"]

        PROXY["aura-proxy<br/>Cloud Run Relay"]
    end

    GEMINI <-->|"WS"| API["Gemini Live API"]
    GEMINI <-.->|"optional"| PROXY
    PROXY <-->|"WS relay"| API
```

---

## 1. Audio Data Flow: Mic to Gemini to Speaker

The audio pipeline is fully bidirectional over a single WebSocket connection. No turn-taking protocol -- the user can interrupt (barge-in) at any time.

```mermaid
flowchart LR
    subgraph CAPTURE["Audio Capture (aura-voice)"]
        MIC["Microphone<br/>cpal"] -->|"48kHz raw"| RESAMPLE["SincFixedIn<br/>Resampler"]
        RESAMPLE -->|"16kHz f32 PCM"| STD_TX["std::sync::mpsc"]
    end

    subgraph BRIDGE["Mic Bridge (aura-daemon)"]
        STD_TX --> TOKIO_RX["tokio::mpsc<br/>cap: 256"]
        TOKIO_RX --> GATE["RMS Energy Gate<br/>threshold: 0.04"]
    end

    subgraph SEND["Gemini Send"]
        GATE -->|"gated PCM"| ENCODE["f32 -> i16 LE<br/>-> base64"]
        ENCODE -->|"realtimeInput.audio"| WS["WebSocket"]
    end

    subgraph RECV["Gemini Receive"]
        WS -->|"serverContent<br/>inline_data audio/*"| DECODE["base64 -> i16 LE<br/>-> f32"]
    end

    subgraph PLAYBACK["Audio Playback (aura-voice)"]
        DECODE -->|"24kHz f32"| PREBUF["PreBuffer<br/>~80ms jitter"]
        PREBUF --> SINK["rodio::Sink<br/>dedicated thread"]
        SINK --> SPEAKER["Speaker"]
    end

    %% Barge-in path
    GATE -.->|"high energy<br/>while speaking"| BARGEIN["Barge-In"]
    BARGEIN -.->|"sink.stop()"| SINK
```

### Key details (verified from source)

| Parameter | Value | Source |
|-----------|-------|--------|
| Capture rate | 48kHz preferred, resampled to 16kHz | `aura-voice/src/audio.rs` |
| Resampler | `rubato::SincFixedIn`, chunk size 1024 | `aura-voice/src/audio.rs` |
| Output rate | 24,000 Hz PCM 16-bit LE | `aura-gemini/src/session.rs` |
| Pre-buffer | 80ms (~1920 samples at 24kHz) | `aura-voice/src/playback.rs` |
| Mic bridge capacity | 256 chunks (bounded) | `aura-daemon/src/main.rs` |
| Barge-in threshold | 0.04 RMS energy | `aura-daemon/src/main.rs` |
| Encoding | f32 -> i16 LE -> base64 (send), reverse (recv) | `aura-gemini/src/session.rs` |

---

## 2. Tool Execution Flow: Gemini Decides, macOS Acts

Gemini declares 9 tools at session setup. When it decides to act, it sends a `toolCall` message. Tools execute concurrently (semaphore limit: 8) and return JSON results.

```mermaid
flowchart TB
    GEMINI["Gemini Live API"] -->|"toolCall JSON<br/>{id, name, args}"| SESSION["GeminiLiveSession"]
    SESSION -->|"GeminiEvent::ToolCall"| PROCESSOR["run_processor()"]

    PROCESSOR -->|"dispatch by name"| DISPATCH{{"Tool Router"}}

    DISPATCH -->|"run_applescript"| BRIDGE["aura-bridge<br/>sandbox-exec + osascript"]
    DISPATCH -->|"get_screen_context"| CTXREAD["aura-screen<br/>MacOSScreenReader"]
    DISPATCH -->|"move_mouse / click /<br/>drag / scroll"| MOUSE["aura-input::mouse<br/>CGEvent"]
    DISPATCH -->|"type_text / press_key"| KB["aura-input::keyboard<br/>CGEvent"]
    DISPATCH -->|"shutdown_aura"| SHUTDOWN["Cancel + Quit<br/>(inline, breaks loop)"]

    subgraph SAFETY["Safety Layer"]
        BRIDGE --> CHECK{"Blocked patterns?<br/>rm -rf, sudo, dd,<br/>chmod 777, $.system,<br/>ObjC.import, ..."}
        CHECK -->|"blocked"| REJECT["Return error"]
        CHECK -->|"safe"| SANDBOX["sandbox-exec<br/>(macOS sandbox profile)"]
        SANDBOX --> OSASCRIPT["osascript<br/>AppleScript / JXA"]
    end

    OSASCRIPT -->|"stdout/stderr"| RESULT["JSON Result"]
    MOUSE -->|"success/error"| RESULT
    KB -->|"success/error"| RESULT
    CTXREAD -->|"context summary"| RESULT

    RESULT -->|"send_tool_response()"| SESSION
    SESSION -->|"toolResponse JSON"| GEMINI

    RESULT -->|"trigger"| CAPTURE["Immediate<br/>Screen Capture"]
    CAPTURE -->|"post-action frame"| SESSION

    RESULT -->|"add_message(ToolResult)"| DB[("SQLite")]
```

### The 9 Declared Tools

| Tool | Crate | Mechanism |
|------|-------|-----------|
| `run_applescript` | aura-bridge | `sandbox-exec` + `osascript` with safety checks |
| `get_screen_context` | aura-screen | Frontmost app, windows, clipboard via osascript |
| `shutdown_aura` | aura-daemon | Cancels session, sends Shutdown event |
| `move_mouse` | aura-input | `CGEvent::new_mouse_event` (MouseMoved) |
| `click` | aura-input | `CGEvent` LeftMouseDown/Up or RightMouseDown/Up |
| `type_text` | aura-input | `CGEvent` keyboard events, UTF-16 per char |
| `press_key` | aura-input | `CGEvent` with modifier flags (cmd/shift/alt/ctrl) |
| `scroll` | aura-input | `CGEvent::new_scroll_event` (pixel units) |
| `drag` | aura-input | `CGEvent` MouseDown -> MouseDragged -> MouseUp |

---

## 3. Continuous Vision Loop: Screenshot Change Detection

The daemon captures the screen at ~1 FPS and sends changed frames to Gemini as JPEG. Tool completions trigger immediate captures so Gemini sees the result.

```mermaid
flowchart TB
    subgraph LOOP["Vision Loop (tokio::spawn, 1s interval)"]
        TICK["interval.tick() OR<br/>cap_notify.notified()"] --> CAPTURE["capture_screen()<br/>spawn_blocking"]
        CAPTURE --> CGDISPLAY["CGDisplay::image()<br/>active display<br/>(under mouse cursor)"]
        CGDISPLAY --> CONVERT["BGRA -> RGB"]
        CONVERT --> HASH["FNV-1a hash<br/>sample 2048 pixels"]
        HASH --> CHANGED{"hash !=<br/>last_hash?"}
        CHANGED -->|"no"| TICK
        CHANGED -->|"yes"| SCALE["Downscale if > 1920px<br/>Triangle filter"]
        SCALE --> JPEG["JPEG encode<br/>quality: 60"]
        JPEG --> B64["base64 encode"]
        B64 --> META["Send coord metadata<br/>'Image resolution: WxH'"]
        META --> SEND["session.send_video()<br/>realtimeInput.video"]
    end

    subgraph TRIGGER["Post-Action Trigger"]
        TOOL["Tool completes"] --> FLAG["CaptureTrigger<br/>AtomicBool::store(true)"]
        FLAG --> NOTIFY["cap_notify.notify_one()"]
        NOTIFY --> TICK
    end

    SEND --> GEMINI["Gemini Live API"]
```

### Frame pipeline parameters

| Step | Detail |
|------|--------|
| Source | `CGDisplay::image()` -- physical (retina) pixels |
| Scale detection | `raw_width / display_bounds.width` for retina factor |
| Max width | 1920px (downscaled with `image::Triangle` filter) |
| JPEG quality | 60 (readability vs. bandwidth tradeoff) |
| Change detection | FNV-1a hash over 2048 sampled pixel positions |
| Frame rate | ~1 FPS (1-second `tokio::time::interval`) |
| Immediate capture | `CaptureTrigger` + `Notify` after tool execution |

---

## 4. Menu Bar UI: Native Cocoa on Main Thread

The menu bar runs on the main thread (required by AppKit). Communication with the tokio runtime happens via `tokio::mpsc` channels, polled by an NSTimer every 50ms.

```mermaid
flowchart TB
    subgraph MAIN_THREAD["Main Thread (AppKit)"]
        APP["NSApp<br/>AccessoryPolicy"] --> STATUSITEM["NSStatusItem<br/>Colored Dot"]
        APP --> POPOVER_UI["NSPopover<br/>Chat Transcript"]

        STATUSITEM -->|"left click"| TOGGLE["Toggle Popover"]
        STATUSITEM -->|"right click"| CTXMENU["Context Menu"]

        CTXMENU --> RECONNECT["Reconnect"]
        CTXMENU --> QUIT["Quit Aura"]

        TIMER["NSTimer 50ms"] -->|"poll rx.try_recv()"| HANDLER["Message Handler"]
    end

    subgraph MESSAGES["MenuBarMessage (tokio::mpsc)"]
        direction TB
        M1["SetColor(Green/Amber/Red/Gray)"]
        M2["SetPulsing(true/false)"]
        M3["SetStatus { text }"]
        M4["AddMessage { text, is_user }"]
        M5["Shutdown"]
    end

    subgraph TOKIO["Tokio Runtime (background thread)"]
        PROCESSOR["run_processor()"] -->|"send"| MESSAGES
    end

    MESSAGES -->|"try_recv()"| HANDLER

    HANDLER --> STATUSITEM
    HANDLER --> POPOVER_UI

    RECONNECT -->|"reconnect_tx"| TOKIO
    QUIT -->|"shutdown_tx"| TOKIO

    subgraph PULSE["Pulse Animation"]
        TIMER -->|"every 10 ticks (500ms)"| TOGGLE_BRIGHT["Toggle Green / GreenDim"]
    end
```

### Status dot colors

| Color | Meaning |
|-------|---------|
| Green (pulsing) | Connected, listening |
| Amber | Running a tool / reconnecting |
| Red | Error (connection failure, mic permission) |
| Gray | Disconnected |

---

## 5. Cloud Run Proxy Relay

For region-restricted Gemini API access, the optional `aura-proxy` crate runs on Google Cloud Run as a transparent WebSocket relay.

```mermaid
flowchart LR
    CLIENT["aura-daemon<br/>(client)"] <-->|"WebSocket"| PROXY["aura-proxy<br/>Cloud Run<br/>axum server"]
    PROXY <-->|"WebSocket"| GEMINI["Gemini Live API<br/>v1beta"]

    subgraph PROXY_DETAIL["Proxy Internals"]
        direction TB
        HEALTH["/health<br/>GET -> { status: ok }"]
        WS_ENDPOINT["/ws?api_key=...&auth_token=...<br/>WebSocket upgrade"]
        AUTH["Constant-time auth<br/>AURA_PROXY_AUTH_TOKEN"]
        RELAY["relay_websocket()<br/>bidirectional frame copy"]
        LIMIT["ConcurrencyLimit: 10"]
    end

    WS_ENDPOINT --> AUTH
    AUTH -->|"valid"| RELAY
    AUTH -->|"invalid"| REJECT["401 Unauthorized"]
```

---

## 6. SQLite Memory Storage

Session data persists in `~/.local/share/aura/aura.db` using WAL mode for concurrent read/write.

```mermaid
erDiagram
    sessions {
        TEXT id PK "UUID v4"
        TEXT started_at "RFC 3339"
        TEXT ended_at "nullable"
        TEXT summary "nullable"
    }

    messages {
        INTEGER id PK "autoincrement"
        TEXT session_id FK "-> sessions.id"
        TEXT role "user | assistant | tool_call | tool_result"
        TEXT content
        TEXT timestamp "RFC 3339"
        TEXT metadata "nullable JSON"
    }

    settings {
        TEXT key PK
        TEXT value
    }

    sessions ||--o{ messages : "has"
```

### Storage operations

| Operation | When |
|-----------|------|
| `start_session()` | New connection (UUID v4) |
| `add_message()` | Every tool call, tool result, greeting context |
| `set_setting("resumption_handle")` | On `SessionResumptionUpdate` from Gemini |
| `end_session()` | On `GeminiEvent::Disconnected` |
| `prune_old_sessions(days)` | Manual cleanup |
| `vacuum()` | Reclaim disk space post-prune |

---

## 7. WebSocket Protocol Detail

```mermaid
sequenceDiagram
    participant C as aura-daemon
    participant G as Gemini Live API

    C->>G: Setup {model, tools, systemPrompt, sessionResumption, compressionConfig}
    G-->>C: setupComplete

    par Audio Stream
        C->>G: realtimeInput.audio (base64 PCM 16kHz, ~100ms chunks)
    and Video Stream
        C->>G: realtimeInput.video (JPEG base64, ~1fps)
    and Context Messages
        C->>G: clientContent.turns (text, turnComplete: true)
    end

    G-->>C: serverContent.modelTurn (audio/* inline_data, 24kHz PCM)
    G-->>C: serverContent.turnComplete

    Note over C,G: Tool Call Flow
    G-->>C: toolCall {functionCalls: [{id, name, args}]}
    C->>C: execute_tool() with safety checks
    C->>G: toolResponse {functionResponses: [{id, name, response}]}
    G-->>C: serverContent.modelTurn (audio response about tool result)

    Note over C,G: Barge-In (User Interrupts)
    C->>G: realtimeInput.audio (high-energy speech)
    G-->>C: serverContent {interrupted: true}
    C->>C: player.stop(), is_speaking = false

    Note over C,G: Session Resumption
    G-->>C: sessionResumptionUpdate {newHandle: "..."}
    C->>C: persist handle to SQLite

    Note over C,G: Server-Initiated Reconnection
    G-->>C: goAway
    C->>C: reconnect with backoff (200ms -> 30s, max 5 attempts)
    C->>G: Setup {sessionResumption: {handle: "..."}}
```

---

## 8. Thread Model

| Thread/Task | Crate | Runtime | Purpose |
|-------------|-------|---------|---------|
| **Main thread** | aura-menubar | AppKit | NSApp.run(), NSStatusItem, NSPopover |
| **aura-mic-capture** | aura-voice | std::thread | cpal audio input (Stream is !Send) |
| **aura-playback** | aura-voice | std::thread | rodio OutputStream (!Send) |
| **tokio runtime** | aura-daemon | tokio | Event processor, mic bridge, vision loop |
| **mic bridge** | aura-daemon | spawn_blocking | Drains std::sync::mpsc -> tokio::mpsc |
| **vision loop** | aura-screen | tokio::spawn | 1fps capture + change detection |
| **tool tasks** | aura-daemon | tokio::spawn | Concurrent tool execution (semaphore: 8) |
| **connection_loop** | aura-gemini | tokio::spawn | WebSocket send/recv + reconnection |

---

## 9. Reconnection & Resilience

```mermaid
stateDiagram-v2
    [*] --> Connecting
    Connecting --> Connected: setupComplete
    Connected --> Streaming: send greeting/context

    Streaming --> Interrupted: serverContent.interrupted
    Interrupted --> Streaming: new audio arrives

    Streaming --> Reconnecting: WS error / goAway
    Reconnecting --> Connecting: backoff (200ms -> 30s)
    Reconnecting --> Failed: max 5 attempts

    Failed --> Connecting: menu Reconnect / auto 3s

    Streaming --> ShuttingDown: shutdown_aura tool
    Connected --> ShuttingDown: menu Quit
    ShuttingDown --> [*]
```

| Parameter | Value |
|-----------|-------|
| Initial backoff | 200ms |
| Max backoff | 30,000ms |
| Max attempts | 5 (per connection_loop) |
| Stable connection threshold | 30s (resets attempt counter) |
| Jitter | Deterministic +/-25% based on attempt number |
| Outer reconnect | Auto-retry after 3s or manual via menu |
| Session resumption | Handle persisted in SQLite, sent on reconnect |
| Context window | Sliding window compression enabled |
