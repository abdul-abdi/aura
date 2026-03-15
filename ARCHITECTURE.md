# Architecture

> **Aura enters the [Gemini Live Agent Challenge](https://geminiliveagentchallenge.devpost.com/) as both a Live Agent and a UI Navigator** — a voice-first AI that doesn't just talk, but sees your screen and takes action on your behalf. It is the "Beyond Text Factor" made real: no text box, no chat window, no typing. You speak, Aura acts.

## The idea

Most AI assistants live behind a text box. You type, they reply, you copy-paste the result somewhere useful. The interaction model hasn't changed since the terminal.

Aura removes the text box entirely. It streams your voice and your screen to Gemini's Live API in real-time, and Gemini responds by speaking back or by directly controlling your Mac — clicking, typing, scrolling, running scripts. The entire operating system becomes the interface.

This is only possible because of the Gemini Live API's native audio streaming and multimodal input. Aura sends 16kHz audio and 2 FPS screen captures over a single persistent WebSocket. Gemini processes both modalities simultaneously, deciding in real-time whether to respond with speech or take action through one of 20 native tools.

## System overview

Native Rust (10 crates) + SwiftUI shell. No Electron, no web views. CoreAudio for sound, CoreGraphics for input, Cocoa for UI, and a direct bidirectional WebSocket to Gemini.

| Crate | Role |
|---|---|
| `aura-daemon` | Central orchestrator — event bus, tool dispatch, session lifecycle |
| `aura-gemini` | Gemini Live API bidirectional WebSocket client |
| `aura-voice` | CoreAudio mic capture (16kHz) + rodio playback (24kHz) + barge-in |
| `aura-screen` | Screen capture, perceptual change detection, accessibility tree |
| `aura-bridge` | AppleScript execution with multi-layer safety gates |
| `aura-input` | Synthetic mouse + keyboard via CoreGraphics CGEvents |
| `aura-memory` | SQLite persistence with FTS5 full-text search |
| `aura-menubar` | Native Cocoa menu bar UI (status dot + popover chat) |
| `aura-proxy` | Cloud Run WebSocket relay with per-device auth |
| `aura-firestore` | Firestore REST client for cross-device memory sync |

## Multimodal data flow

```
            ┌───────────────────────────────────────────┐
            │          Gemini Live API                   │
            │   model: gemini-2.5-flash-native-audio     │
            │   protocol: bidirectional WebSocket         │
            │   context: 500K token sliding window        │
            └─────────┬──────────────────┬──────────────┘
           audio/video│ (streaming up)   │ audio response
           16kHz PCM  │                  │ 24kHz PCM
           2 FPS JPEG │                  │ tool calls
                      │                  │ text/transcription
          ┌───────────┘                  └────────────┐
          ▼                                           ▼
  ┌──────────────┐                         ┌──────────────┐
  │  Microphone  │                         │   Speaker    │
  │  16kHz mono  │                         │  24kHz mono  │
  └──────────────┘                         └──────────────┘
          ▲                                           ▲
          │          ┌──────────────────┐              │
          └──────────│  aura-daemon     │──────────────┘
                     │  (orchestrator)  │
                     └──┬────┬────┬────┬┘
                        │    │    │    │
            ┌───────────┘    │    │    └───────────┐
            ▼                ▼    ▼                ▼
      ┌──────────┐    ┌────────┐ ┌────────┐  ┌──────────┐
      │  Screen  │    │ Input  │ │ Bridge │  │  Memory  │
      │  2 FPS   │    │ mouse  │ │ Apple- │  │ SQLite   │
      │  capture │    │ kbd    │ │ Script │  │ FTS5     │
      └──────────┘    └────────┘ └────────┘  └──────────┘
```

The loop is continuous: voice and vision stream up, Gemini responds with speech or tool calls, tool results feed back to Gemini, and the cycle repeats. The user never leaves their workflow — Aura operates _inside_ whatever app they're already using.

## Gemini Live API integration

Aura uses the **Gemini Live API via bidirectional WebSocket** (`v1beta.BidiGenerateContent`), streaming both audio and video input simultaneously.

**Connection setup:**
```
WebSocket connect (TLS)
  → Send BidiGenerateContentSetup:
      model:       gemini-2.5-flash-native-audio-preview
      modalities:  [AUDIO]
      voice:       Kore
      tools:       20 function declarations + Google Search
      resumption:  session handle (if reconnecting)
      compression: sliding window @ 500K tokens
  → Receive setupComplete
  → Begin streaming
```

**Real-time streaming (simultaneous):**
- **Audio in:** 16kHz mono PCM, 100ms chunks, base64-encoded
- **Video in:** JPEG screenshots at 2 FPS with perceptual change detection (skips identical frames)
- **Audio out:** 24kHz mono PCM from Gemini's native audio model
- **Tool calls:** function invocations with JSON arguments, dispatched to native macOS tools

**Session resilience:**
- Proactive session rotation every 10 minutes (prevents server-side context staleness)
- Exponential backoff on disconnect: 200ms → 30s, max 5 attempts, ±25% jitter
- Session resumption via server-provided handles for seamless reconnection
- Poison counter detects stale handles (2 consecutive short-lived sessions → fresh start)
- Context window compression at 500K tokens — Gemini manages pruning server-side

**Interruption handling (barge-in):**
Aura supports natural conversation interruption. When the user speaks while Gemini is responding, Aura detects sustained energy above an ambient-calibrated threshold, immediately stops playback, and signals the interruption to Gemini — all within a single audio frame (~30ms). This is core to the Live Agent experience: conversations feel natural, not turn-based.

## Audio pipeline

```
CAPTURE                              PLAYBACK
───────                              ────────
Hardware mic (48kHz typical)         Gemini audio (24kHz base64 PCM)
  → cpal stream (256-frame buffer)     → decode base64 → f32 samples
  → accumulate 512-frame chunks        → 40ms pre-buffer (jitter absorb)
  → sinc resample → 16kHz mono        → rodio sink → system speaker
  → energy gate during playback        → drain detection (100ms poll)
  → base64 encode → Gemini WS

BARGE-IN
────────
User speaks during Gemini response
  → RMS energy > ambient threshold × 1.3
  → 2 consecutive frames confirm (not a spike)
  → playback stopped immediately (sink dropped)
  → 300ms reverb guard (speaker echo decay)
  → Gemini receives interruption signal
```

**Ambient noise calibration** runs at startup (~500ms, 100 RMS samples) and adapts the barge-in threshold per environment — quiet room vs. noisy cafe.

## Screen understanding (vision)

Aura watches the screen continuously, giving Gemini real-time visual context of what the user sees.

- **Capture rate:** 2 FPS during active changes, drops to 0.67 FPS after 2.5s idle
- **Change detection:** FNV-1a perceptual hash on sampled pixels — unchanged frames are never sent
- **Format:** JPEG (1920px max width, Q80) streamed as base64 via the Live API's video input
- **Coordinate mapping:** Retina-aware scaling from image pixels to logical screen points
- **Accessibility tree:** Reads UI element roles, labels, bounds, and states for precise targeting
- **Vision oracle:** Secondary Gemini 3 Flash model refines click coordinates when accessibility targeting fails (with circuit breaker: 3 failures → 30s cooldown)

This dual approach — pixel-level vision + structured accessibility data — gives Gemini both the "what does it look like" and the "what are the interactive elements" simultaneously.

## Tool execution model

Gemini calls tools through the Live API's function calling protocol. Aura declares 20 native macOS tools:

| Category | Tools |
|---|---|
| **Mouse** | `click`, `click_element`, `context_menu_click`, `move_mouse`, `drag`, `scroll` |
| **Keyboard** | `type_text`, `press_key`, `select_text` |
| **Apps** | `activate_app`, `click_menu_item` |
| **Scripts** | `run_applescript`, `run_javascript`, `run_shell_command` |
| **Memory** | `save_memory`, `recall_memory` |
| **Context** | `get_screen_context`, `write_clipboard` |
| **System** | `shutdown_aura` |

Tools execute asynchronously (max 8 concurrent). Safe action chains — like click→type or type→enter — pipeline without screen verification, up to 3 deep with 30ms settle delays.

### Safety gates

Every tool call passes through these gates before execution:

1. **Blocked app list** — Terminal, iTerm, Kitty, Warp, Alacritty, Hyper, Tabby, Rio, WezTerm blocked from activation and `open`
2. **`do shell script` blocked** — AppleScript's shell escape hatch is rejected entirely (not filtered, blocked), plus `run script` (dynamic eval)
3. **Shell metacharacter rejection** — `run_shell_command` rejects `|`, `;`, `` ` ``, `$()`, `>`, `<`, `&&`, `||`, and `sudo`
4. **Obfuscation detection** — catches AppleScript concatenation tricks (`"do" & " shell" & " script"`)
5. **JXA blocked** — JavaScript for Automation disabled unconditionally
6. **Input clamping** — scroll bounded to ±1000, text capped at 10K chars
7. **Destructive action guardrail** — system prompt instructs Gemini to confirm before deletions (prompt-level, not code-enforced)
8. **Timeouts** — 60s for all scripts, 10KB output cap

## Threading model

| Thread | What runs | Why |
|---|---|---|
| **Main (OS)** | Cocoa NSApp event loop, menu bar UI | macOS requires UI on main thread |
| **std::thread** | cpal mic capture stream | cpal's `Stream` is `!Send` |
| **tokio runtime** | Gemini WS, tool execution, IPC, screen capture, playback | Async I/O + concurrency |

## IPC protocol

The Rust daemon and SwiftUI app communicate over a Unix socket using JSONL:

**Daemon → UI:** `DotColor` (status), `Transcript` (speech), `ToolStatus` (execution), `Status` (connection), `Shutdown`

**UI → Daemon:** `SendText`, `ToggleMic`, `Reconnect`, `Shutdown`

## Resilience & error handling

Aura is designed to stay alive through network instability, API hiccups, and edge-case failures:

| Mechanism | What it does |
|---|---|
| **Exponential backoff** | Reconnects at 200ms → 400ms → ... → 30s with ±25% jitter. Max 5 attempts before surfacing to user. |
| **Session resumption** | Server-provided handles allow seamless reconnection without losing conversation context. |
| **Poison counter** | Detects stale resumption handles — 2 consecutive short-lived sessions (<30s) triggers a fresh start. |
| **Proactive rotation** | Sessions rotate every 10 minutes to prevent server-side context staleness before it causes errors. |
| **Vision oracle circuit breaker** | After 3 consecutive failures, the Gemini 3 Flash vision fallback disables for 30s to avoid cascading timeouts. |
| **Stale frame draining** | Before reconnect, buffered audio/video frames are drained to prevent protocol violations. Tool responses are never drained. |
| **Firestore retry queue** | Failed cloud syncs are queued to `~/Library/Application Support/aura/pending_sync/` and retried on next session start. |
| **Graceful degradation** | Missing permissions, unavailable mic/speaker, or cloud config are warned — never fatal. Aura runs with whatever is available. |

## Google Search grounding

Aura includes Google Search as a Gemini tool, enabling live web answers mid-conversation. When you ask "what's the weather" or "who won the game last night," Gemini grounds its response in real-time search results — no browser needed, no tab switching. The answer comes back as speech, naturally woven into the conversation.

## Google Cloud integration

Aura uses Google Cloud for persistent memory and network resilience:

```
┌──────────────────┐    ┌──────────────────────┐    ┌───────────────┐
│  aura-proxy      │    │  memory-agent        │    │  Firestore    │
│  Cloud Run       │    │  Cloud Run           │    │  (GCP)        │
│                  │    │                      │    │               │
│  WebSocket relay │    │  FastAPI + Gemini    │    │  Facts &      │
│  for Gemini API  │    │  session consol-     │    │  sessions     │
│  Per-device auth │    │  idation via ADK     │    │  per device   │
│  Token registry  │    │                      │    │               │
└──────────────────┘    └──────────────────────┘    └───────────────┘
```

- **Proxy relay (Cloud Run):** WebSocket bridge to Gemini Live API for restricted networks. Handles per-device registration, token-based auth (constant-time comparison), and concurrent connection limiting. Bidirectional frame relay with 1 MiB message limits.

- **Memory agent (Cloud Run):** Python FastAPI service using Gemini for session consolidation. After each conversation, transcripts are distilled into structured facts (category, entities, importance). On session start, relevant facts are queried and injected as context — giving Aura memory that persists across sessions and devices.

- **Firestore:** Document store for facts and session summaries, scoped per device ID. Deterministic document IDs enable idempotent writes. Anonymous Firebase auth with token caching and proactive refresh.

## What makes this different

Aura isn't a chatbot with extra features. It's an operating system agent that happens to communicate through voice.

- **No text box.** Voice is the primary (and only) input. The screen is the output.
- **Multimodal in, multimodal out.** Audio and vision stream continuously to Gemini. Responses come as speech or direct actions on the desktop.
- **The OS is the UI.** Aura doesn't render its own interface for tasks. It uses _your_ apps — clicking buttons, typing text, running scripts in the apps you're already working in.
- **Native performance.** 18K lines of Rust, compiled to a single binary. No runtime overhead, no garbage collector, no web engine. Audio latency is measured in milliseconds.
- **Real interruption.** You can cut Gemini off mid-sentence and it adapts. Conversations flow like talking to a person, not waiting for a loading spinner.
