# Aura

**Voice-first AI desktop companion for macOS, powered by Gemini Live.**

Talk to your Mac. Aura listens, understands what's on your screen, and takes action — opening apps, managing windows, searching files, running scripts — all through natural conversation. No typing. No clicking. Just talk.

https://github.com/user-attachments/assets/demo-placeholder

## What It Does

Aura lives in your menu bar as a small colored dot. Speak naturally and it will:

- **See your screen** — knows what app you're using, what windows are open, what's on your clipboard
- **Run AppleScript & JXA** — opens apps, moves windows, searches with Spotlight, types text, clicks buttons, controls system settings
- **Remember context** — references what you said earlier in the conversation
- **Greet you intelligently** — "I see you've got 47 Chrome tabs open... bold choice"
- **Interrupt and resume** — barge in mid-sentence and it stops immediately

### Example Interactions

> "Open Spotify and play my liked songs"

> "What app am I using right now?"

> "Move this window to the left half of the screen"

> "Find all PDFs on my desktop from this week"

> "Set a timer for 10 minutes"

> "Quit Aura" *(says goodbye and exits cleanly)*

## Quick Start

### One-Line Install

```bash
curl -fsSL https://raw.githubusercontent.com/abdul-abdi/aura/main/scripts/install.sh | bash
```

This installs Rust (if needed), builds Aura, creates the `.app` bundle in `/Applications`, and prompts for your Gemini API key.

### Manual Install

```bash
git clone https://github.com/abdul-abdi/aura.git && cd aura
cargo build --release -p aura-daemon
bash scripts/bundle.sh
cp -r target/release/Aura.app /Applications/
open /Applications/Aura.app
```

On first launch, Aura will ask for your Gemini API key via a native dialog. Get one free at [aistudio.google.com/apikey](https://aistudio.google.com/apikey).

### Requirements

- macOS 13 (Ventura) or later
- Microphone access (prompted on first launch)
- Accessibility permission for AppleScript automation (prompted on first use)

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
        |                    |              shutdown_aura
        |                    |                    |
        |                    v                    v
        |               [Speaker]          [osascript/pbpaste]
        |                                        |
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
| **aura-bridge** | AppleScript/JXA executor with safety checks |
| **aura-menubar** | Native NSStatusItem + NSPopover via cocoa/objc FFI |
| **aura-memory** | SQLite session persistence (conversations, tool calls) |
| **aura-proxy** | Cloud Run WebSocket relay for region-restricted APIs |
| **aura-overlay** | Skia-rendered transparent floating UI (experimental) |

### Key Technical Decisions

- **Native Cocoa FFI** — No frameworks, no Swift bridging. Direct `objc::msg_send!` calls to AppKit for the menu bar, popover, and status item. Minimal memory footprint.
- **Streaming audio pipeline** — Single `rodio::Sink` per session. Gemini sends ~100ms audio chunks; they're appended to the sink for gapless playback. Barge-in calls `sink.stop()` instantly.
- **Session resumption** — On disconnect, Aura stores a resumption token and reconnects with exponential backoff (1s → 30s, max 10 attempts). Context window compression enabled.
- **Tool safety** — The AppleScript executor blocks dangerous patterns (`rm -rf`, `sudo`, `dd`, `chmod 777`, fork bombs) before execution.

## Configuration

Config lives at `~/.config/aura/config.toml`:

```toml
api_key = "your-gemini-api-key"
```

Environment variables override the config file:

| Variable | Purpose |
|----------|---------|
| `GEMINI_API_KEY` | Gemini API key (required) |
| `AURA_PROXY_URL` | WebSocket relay URL (optional, for region restrictions) |

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

## License

Apache-2.0
