# Getting Started with Aura

Aura is a voice-first AI desktop companion for macOS. Built with Rust, Swift, and the Gemini Live API, it listens for your voice commands and controls your Mac through AppleScript, keyboard/mouse automation, and screen analysis.

---

## Prerequisites

- **macOS 14+** (Sonoma or later)
- **A Gemini API key** -- get one free at [Google AI Studio](https://aistudio.google.com/apikey)

To build from source, you also need:
- **Rust 1.85+** (edition 2024) -- install via [rustup](https://rustup.rs/)
- **Xcode Command Line Tools** -- `xcode-select --install`

---

## Installation

### Option 1: Download the DMG (recommended)

Download the latest release from the [Releases page](https://github.com/abdul-abdi/aura/releases/latest). Open the DMG, drag Aura to Applications.

> **macOS Gatekeeper:** Since Aura is not notarized with Apple, macOS will block the first launch. Right-click `Aura.app` → **Open** → click **Open** in the dialog. Or run: `xattr -dr com.apple.quarantine /Applications/Aura.app`

### Option 2: From source

```bash
git clone https://github.com/abdul-abdi/aura.git
cd aura
bash scripts/bundle.sh
cp -r target/release/Aura.app /Applications/
open /Applications/Aura.app
```

`bundle.sh` compiles a release build and packages everything into a macOS `.app` bundle.

---

## First Launch

1. **Launch Aura** from `/Applications` or via Spotlight (Cmd+Space, type "Aura").

2. **Enter your Gemini API key** in the Welcome screen. Alternatively, set it beforehand:
   ```bash
   mkdir -p ~/.config/aura
   echo 'api_key = "your-gemini-api-key"' > ~/.config/aura/config.toml
   ```

3. **Grant permissions** -- Aura needs three macOS permissions to function:

   | Permission | How to grant | Why |
   |---|---|---|
   | **Microphone** | Inline system prompt on first use | Voice capture |
   | **Screen Recording** | System Settings > Privacy & Security > Screen Recording | Screen analysis and context |
   | **Accessibility** | System Settings > Privacy & Security > Accessibility | Keyboard and mouse automation |

   Aura checks for Accessibility at startup and prompts you if it is missing. Microphone and Screen Recording permissions must be granted or Aura will fail silently.

4. **The green dot appears in your menu bar** -- Aura is listening and ready.

### Menu Bar Status

| Dot color | Meaning |
|---|---|
| Green | Listening and connected |
| Green (pulsing) | Connected and listening |
| Amber | Reconnecting or running a tool |
| Red | Error (usually a permission issue) |
| Gray | Disconnected |

---

## Configuration

### Config file

Aura checks two locations for its config file (first match wins):
1. `~/Library/Application Support/aura/config.toml` (macOS default)
2. `~/.config/aura/config.toml` (fallback, also used by the Welcome screen)

```toml
api_key = "your-gemini-api-key"
proxy_url = "wss://your-proxy.run.app/ws"  # optional — route through your own relay
```

### Environment variables

Environment variables override the config file:

| Variable | Purpose |
|---|---|
| `GEMINI_API_KEY` | Gemini API key (required if not in config file) |
| `AURA_PROXY_URL` | WebSocket relay URL (optional) |
| `AURA_PROXY_AUTH_TOKEN` | Auth token for the proxy |
| `RUST_LOG` | Log verbosity (e.g. `debug`, `aura_gemini=trace`) |
| `PORT` | Proxy server listen port (default 8080) |
| `AURA_CLOUD_REGION` | GCP region for deploy-proxy.sh (default `us-central1`) |

### Data directory

Aura stores its data in `~/Library/Application Support/aura/`:
- `aura.db` -- SQLite database (sessions, messages, settings)
- `models/` -- Wake word model files
- `logs/` -- Application logs

---

## What You Can Say

Once the green dot is active, just speak naturally. Here are some examples:

- **"Open Safari and go to github.com"** -- launches apps and navigates
- **"Take a look at my screen and summarize what you see"** -- captures and analyzes your screen
- **"Move the mouse to the search bar and type 'Rust programming'"** -- controls mouse and keyboard
- **"Press Cmd+Tab to switch apps"** -- sends keyboard shortcuts
- **"What windows do I have open?"** -- queries your desktop state
- **"Close this window"** -- executes actions on the current app

Aura uses 9 tools under the hood: AppleScript execution, screen capture, mouse movement, clicking, typing, key presses, scrolling, dragging, and shutdown. It decides which tools to invoke based on your natural language request.

---

## Troubleshooting

| Symptom | Likely cause | Fix |
|---|---|---|
| Green dot but no response | Microphone permission not granted | System Settings > Privacy & Security > Microphone -- enable Aura |
| Amber dot stays on | Network issue or invalid API key | Check your internet connection and verify your `GEMINI_API_KEY` |
| Red dot | Permission error | Check all three permissions in System Settings (Mic, Screen Recording, Accessibility) |
| Voice not working | Audio configuration issue | Run with `RUST_LOG=debug cargo run -p aura-daemon -- --verbose` and check logs |
| Screen capture fails | Screen Recording permission missing | Look for the error "is Screen Recording permission granted?" in logs |
| No menu bar icon | App crashed on launch | Run from terminal to see the error: `cargo run -p aura-daemon -- --verbose` |

---

## Development

### Quick iteration

```bash
bash scripts/dev.sh                     # Build + install + relaunch
```

### Build and test

```bash
cargo check --workspace                 # Fast compilation check
cargo test --workspace                  # Run all tests
cargo clippy --workspace                # Lint
cargo fmt --all                         # Format code
```

### Run in dev mode

```bash
GEMINI_API_KEY=your-key cargo run -p aura-daemon -- --verbose
```

Add `--headless` to run without the menu bar (terminal-only mode):

```bash
GEMINI_API_KEY=your-key cargo run -p aura-daemon -- --headless
```

### Smoke test

```bash
bash scripts/smoke-test.sh
```

### Deploy the proxy (optional)

If you want to route WebSocket traffic through your own Cloud Run relay:

```bash
bash scripts/deploy-proxy.sh
```

---

## Uninstall

To remove Aura completely:

```bash
bash scripts/uninstall.sh
```

This removes the app bundle, configuration, and application data.

---

## Next Steps

- Read the [Architecture](ARCHITECTURE.md) doc for a deep dive into crates, threading, and data flow
- See the [Tools Reference](TOOLS.md) for details on all 9 tools
- See the [Security Model](SECURITY.md) for threat model and safety mechanisms
