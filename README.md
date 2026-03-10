# Aura

**Voice-first AI desktop companion for macOS, powered by Gemini Live.**

Talk to your Mac. Aura listens, understands what's on your screen, and takes action — opening apps, managing windows, searching files, running scripts — all through natural conversation. No typing. No clicking. Just talk.

https://github.com/user-attachments/assets/demo-placeholder

## About

Built for the **Gemini Live Agent Challenge**. Aura demonstrates what's possible when you combine Gemini's bidirectional audio streaming with real desktop automation: a native macOS assistant that sees your screen, remembers context across sessions, and operates your computer through voice.

No Electron. No web views. Pure Rust + native Cocoa.

## What It Does

Aura lives in your menu bar as a small colored dot. Speak naturally and it will:

- **See your screen** — knows what app you're using, what windows are open, what's on your clipboard
- **Run AppleScript & JXA** — opens apps, moves windows, searches with Spotlight, types text, clicks buttons, controls system settings
- **Control your cursor** — moves the mouse, clicks, scrolls, drags, types keystrokes
- **Remember context** — references what you said earlier in the conversation, persists across sessions
- **Greet you intelligently** — "I see you've got 47 Chrome tabs open... bold choice"
- **Interrupt and resume** — barge in mid-sentence and it stops immediately

### Tools Available to Gemini

| Tool | What It Does |
|------|-------------|
| `run_applescript` | Execute AppleScript or JXA to automate any macOS app or system feature |
| `get_screen_context` | Read frontmost app, window title, open windows, clipboard |
| `move_mouse` | Move cursor to screen coordinates |
| `click` | Click at coordinates (single, double, left, right) |
| `type_text` | Type text at the current cursor position |
| `press_key` | Send keyboard shortcuts and special keys (Cmd+C, Escape, arrows…) |
| `scroll` | Scroll up, down, left, right |
| `drag` | Click-and-drag between two coordinates |
| `shutdown_aura` | Quit cleanly with a goodbye message |

### Example Interactions

> "Open Spotify and play my liked songs"

> "What app am I using right now?"

> "Move this window to the left half of the screen"

> "Find all PDFs on my desktop from this week"

> "Set a timer for 10 minutes"

> "Quit Aura" *(says goodbye and exits cleanly)*

## Setup Guide

### Prerequisites

- macOS 13 (Ventura) or later
- [Rust](https://rustup.rs) (stable toolchain)
- A Gemini API key — get one free at [aistudio.google.com/apikey](https://aistudio.google.com/apikey)
- Microphone access (prompted on first launch)
- Accessibility permission for AppleScript automation (prompted on first use)

### Step-by-Step (from a fresh clone)

**1. Clone the repo**

```bash
git clone https://github.com/abdul-abdi/aura.git
cd aura
```

**2. Build the daemon**

```bash
cargo build --release -p aura-daemon
```

This compiles the full workspace (~2 min on first run, ~5s after that).

**3. Bundle the macOS app**

```bash
bash scripts/bundle.sh
```

This creates `target/release/Aura.app` with the correct `Info.plist` and icon.

**4. Install and launch**

```bash
cp -r target/release/Aura.app /Applications/
open /Applications/Aura.app
```

On first launch, Aura shows a native dialog asking for your Gemini API key. Enter it once — it's saved to `~/.config/aura/config.toml`.

**5. Grant permissions when prompted**

- Microphone: required for voice input
- Accessibility: required for AppleScript automation (System Settings → Privacy & Security → Accessibility)

**6. Talk to it**

A colored dot appears in your menu bar. Start speaking. The dot pulses green while listening and turns amber while running a tool.

### One-Line Install (alternative)

```bash
curl -fsSL https://raw.githubusercontent.com/abdul-abdi/aura/main/scripts/install.sh | bash
```

This installs Rust (if needed), builds Aura, creates the `.app` bundle in `/Applications`, and prompts for your Gemini API key.

### Run Tests

```bash
cargo test --workspace
```

All tests pass. The test suite covers the Gemini session protocol, tool declarations, AppleScript safety checks, memory persistence, and the proxy relay.

## How It Works

```
     You speak
        |
   [Microphone] ──16kHz PCM──> [Gemini Live WebSocket]
        |                              |
        |                     Gemini processes speech,
        |                     decides to respond or
        |                     call a tool
        |                              |
        |                    ┌─────────┴──────────┐
        |                    v                    v
        |              [Audio Response]     [Tool Call]
        |                    |                    |
        |              24kHz PCM            run_applescript
        |              via rodio            get_screen_context
        |                    |              move_mouse / click
        |                    v              type_text / scroll
        |               [Speaker]                 |
        |                                  Result sent back
        |                                  to Gemini
        |
   [Menu Bar Dot]
   Gray = disconnected
   Green (pulsing) = listening
   Amber = running a tool / reconnecting
   Red = error
```

Aura uses **bidirectional streaming** — audio flows both ways simultaneously over a single WebSocket connection. There's no turn-taking protocol; you can interrupt Aura mid-sentence (barge-in) and it stops immediately.

## Architecture

Rust workspace with 9 crates. No Electron. No web views. Pure native macOS.

| Crate | What It Does |
|-------|-------------|
| **aura-daemon** | Main orchestrator — connects everything together |
| **aura-gemini** | Gemini Live WebSocket client, protocol types, tool declarations |
| **aura-voice** | Audio capture (cpal) + streaming playback (rodio) with barge-in |
| **aura-screen** | Screen context — frontmost app, windows, clipboard via osascript |
| **aura-bridge** | AppleScript/JXA executor with sandbox and safety checks |
| **aura-menubar** | Native NSStatusItem + NSPopover via cocoa/objc FFI |
| **aura-memory** | SQLite session persistence (conversations, tool calls) |
| **aura-proxy** | Cloud Run WebSocket relay for region-restricted APIs |
| **aura-input** | Low-level mouse/keyboard control via macOS CGEvent API |

See [`docs/architecture.md`](docs/architecture.md) for detailed architecture diagrams (system overview, audio pipeline, tool execution, vision loop, WebSocket protocol, thread model, and more).

### Key Technical Decisions

- **Native Cocoa FFI** — No frameworks, no Swift bridging. Direct `objc::msg_send!` calls to AppKit for the menu bar, popover, and status item. Minimal memory footprint.
- **Streaming audio pipeline** — Single `rodio::Sink` per session. Gemini sends ~100ms audio chunks; they're appended to the sink for gapless playback. Barge-in calls `sink.stop()` instantly.
- **Session resumption** — On disconnect, Aura stores a resumption token and reconnects with exponential backoff (1s → 30s, max 10 attempts). Context window compression enabled.
- **Tool safety** — The AppleScript executor blocks dangerous patterns before execution (see Security section below).

## Cloud Deployment (Proxy)

`aura-proxy` is a lightweight WebSocket relay deployed to Google Cloud Run. It solves API region restrictions — if Gemini Live isn't available in your region, Aura routes through the proxy transparently.

### Deploy the proxy

```bash
# Authenticate with GCP
gcloud auth login

# Deploy (prompts for project ID if not set)
bash scripts/deploy-proxy.sh

# Or specify project directly
bash scripts/deploy-proxy.sh --project my-gcp-project
```

The script:
1. Enables the required GCP APIs (Cloud Run, Cloud Build, Artifact Registry)
2. Builds the container using Cloud Build (no local Docker required)
3. Deploys to Cloud Run with 0 min instances (scales to zero)
4. Saves the proxy URL to `~/.config/aura/config.toml` automatically

The proxy is optional. Without it, Aura connects directly to the Gemini Live endpoint.

## Configuration

Config lives at `~/.config/aura/config.toml`:

```toml
api_key = "your-gemini-api-key"
# proxy_url = "wss://your-proxy.run.app/ws"  # optional
```

Environment variables override the config file:

| Variable | Purpose |
|----------|---------|
| `GEMINI_API_KEY` | Gemini API key (required) |
| `AURA_PROXY_URL` | WebSocket relay URL (optional, for region restrictions) |

## Security

Scripts submitted to `run_applescript` are validated before execution:

- **Dangerous pattern blocking** — `rm -rf`, `sudo`, `mkfs`, `dd if=`, `chmod 777`, fork bombs, `diskutil erase`, and 10 other patterns are rejected outright, before any subprocess is spawned.
- **JXA escape blocking** — JXA-specific shell escape vectors (`$.system`, `ObjC.import`, `Application("Terminal").doScript`) are blocked separately.
- **macOS sandbox** — Scripts run under `sandbox-exec` with a deny-by-default profile. Network access and sensitive filesystem paths are blocked at the OS level even if a dangerous pattern slips through.
- **Output truncation** — Script output is capped at 10KB to prevent memory exhaustion.
- **Timeout enforcement** — All scripts have a max timeout (default 30s, hard cap 60s). The subprocess is killed on timeout — not just dropped.

## Menu Bar Controls

| Action | What Happens |
|--------|-------------|
| **Left click** dot | Toggle chat popover (shows conversation transcript) |
| **Right click** dot | Context menu: Reconnect, Quit Aura |
| **Say "quit aura"** | Aura says goodbye and exits cleanly |

## Personality

Aura isn't a generic assistant. It has opinions.

> *"I see you're using Electron... consuming RAM since 2013."*

> *"Done. Moved your windows around. You're welcome."*

> *"47 Chrome tabs? Bold choice."*

Built with the `Kore` voice on Gemini's native audio model (`gemini-2.5-flash-native-audio-preview-12-2025`). Responses are concise — usually under 2 sentences.

## Development

```bash
# Run in dev mode with verbose logging
GEMINI_API_KEY=your-key cargo run -p aura-daemon -- --verbose

# Run tests
cargo test --workspace

# Build release .app bundle
bash scripts/bundle.sh

# Headless mode (no menu bar, terminal only)
cargo run -p aura-daemon -- --headless
```

## Tech Stack

- **Language:** Rust
- **AI:** Gemini 2.5 Flash (native audio, Live API)
- **Audio:** cpal (capture), rodio (playback)
- **UI:** Native macOS Cocoa/AppKit via objc FFI
- **Storage:** SQLite via rusqlite
- **Networking:** tokio-tungstenite (WebSocket), axum (proxy)
- **Cloud:** Google Cloud Run (optional proxy)

## License

Apache-2.0
