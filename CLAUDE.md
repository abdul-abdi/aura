# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Build & Development Commands

```bash
# Build
cargo check --workspace                # Compilation check (objc macro warnings are harmless — ignore)
cargo build --release -p aura-daemon   # Release binary

# Test
cargo test --workspace                 # All tests (~183 tests)
cargo test -p aura-daemon              # Single crate
cargo test -p aura-bridge test_timeout_kills_script  # Single test by name
RUST_LOG=debug cargo test -- --nocapture             # With logging

# Lint & Format
cargo fmt --all                        # Format code
cargo fmt --all --check                # Check formatting (CI)
cargo clippy --workspace               # Lint

# Run
GEMINI_API_KEY=your-key cargo run -p aura-daemon -- --verbose  # Dev mode (-v also works)
cargo run -p aura-daemon -- --headless                          # No menu bar, terminal only

# Bundle & Deploy
bash scripts/bundle.sh                 # → target/release/Aura.app
bash scripts/deploy-proxy.sh           # Deploy aura-proxy to Cloud Run
```

**Requires Rust 1.85+** (edition 2024). No `rust-toolchain.toml` — verify with `rustc --version`.

## Architecture

See README.md for crate overview and data flow diagram. This section covers what the README doesn't.

### Threading Model

- **Main thread**: Cocoa run loop (`NSApplication`). `aura-menubar` owns the `NSStatusItem` and `NSPopover`. All AppKit calls must happen here.
- **Background thread**: `tokio::Runtime` runs the daemon async loop — Gemini WebSocket, audio capture, tool dispatch.
- **Event bus** (`crates/aura-daemon/src/bus.rs`): `tokio::sync::broadcast` channel carrying `AuraEvent` enum. Not mpsc — broadcast allows multiple subscribers.
- **Other channels**: `mpsc` is used for mic→daemon audio frames, tool responses, and menu bar message passing.

### Key Files for Common Tasks

| Task | Files to modify |
|------|----------------|
| Add a Gemini tool | `aura-gemini/src/tools.rs` (declaration + schema) AND `aura-daemon/src/main.rs` (handler dispatch) |
| Add a menu bar color | `aura-menubar/src/status_item.rs` (DotColor enum + RGB) → `app.rs` (message sends) → `main.rs` (event triggers) |
| Change reconnect logic | `aura-gemini/src/session.rs` (exponential backoff: 200ms→30s, max 5 attempts) |
| Modify system prompt | `aura-gemini/src/config.rs` (`build_system_prompt()`) |
| Add safety patterns | `aura-bridge/src/script.rs` (BLOCKED_SHELL_PATTERNS, BLOCKED_JXA_PATTERNS, OBFUSCATED_ATOM_PATTERNS) |
| Change audio format | `aura-voice/src/audio.rs` (capture: 16kHz) / `playback.rs` (output: 24kHz) |
| Modify SQLite schema | `aura-memory/src/store.rs` (3 tables: sessions, messages, settings) |

### Gemini Live API Specifics

- **Model**: `gemini-2.5-flash-native-audio-preview-12-2025` — only available on `v1beta` endpoint (not v1)
- **WebSocket frames**: Server sends both Text and Binary frames — code handles both identically via UTF-8 conversion
- **9 tools declared**: `run_applescript`, `get_screen_context`, `shutdown_aura`, `move_mouse`, `click`, `type_text`, `press_key`, `scroll`, `drag`
- **Native audio model** sends markdown-formatted thinking text (`**bold**`) as transcriptions — filter these out
- **Session resumption**: `newHandle` is optional in `SessionResumptionUpdate` — server sends updates without it initially. Handle stored in `settings` table.
- **`test-support` feature flag**: `aura-gemini` has this feature for mock WebSocket server in integration tests

### macOS Gotchas

**Cocoa FFI:**
- `NSApp` is a function (`NSApp()`), not a class — `class!(NSApp)` will crash at runtime
- `sendActionOn:` takes **event masks** (LeftMouseDown=2, RightMouseDown=8), not event types
- `NSEventType` is different: LeftMouseDown=1, RightMouseDown=3, RightMouseUp=4 — don't confuse with masks
- NSBox separator shows "Title" by default — set `setTitlePosition: 0` (NSNoTitle)
- `objc`/`cocoa` crate macros emit ~30 `unexpected_cfg` warnings on every build — these are upstream noise, not bugs

**Required permissions** (must grant manually in System Settings):
- **Microphone** — audio capture fails silently without it
- **Screen Recording** — `CGDisplay::image()` returns nil; check `capture.rs` error "is Screen Recording permission granted?"
- **Accessibility** — required for `aura-input` synthetic keyboard/mouse. Checked at startup via `AXIsProcessTrustedWithOptions`

**Bridge safety**: `ScriptExecutor` runs AppleScript/JXA through `sandbox-exec` with a restrictive profile. Dangerous patterns (`rm -rf`, `sudo`, `mkfs`, `dd if=`, `chmod 777`, `:(){ :|:`, `> /dev/sd`, `rmdir`, `unlink`, `diskutil erase`, etc.) are checked against ALL script content. JXA-specific patterns (`$.system`, `ObjC.import`, `Application("Terminal")`) are also blocked. Obfuscation detection catches fragmented dangerous commands split across string concatenation or variables.

### Menu Bar States

5 colors in `DotColor` enum (`aura-menubar/src/status_item.rs`):
- Gray (0) = disconnected
- Green (1) = listening
- Amber (2) = running tool / reconnecting
- Red (3) = error
- GreenDim (4) = pulsing dim phase (alternates with Green every 500ms)

## Configuration

Config file: `~/.config/aura/config.toml`
```toml
api_key = "your-gemini-api-key"
proxy_url = "wss://your-proxy.run.app/ws"  # optional
```

Environment variables (override config file):

| Variable | Purpose |
|----------|---------|
| `GEMINI_API_KEY` | Gemini API key (required) |
| `AURA_PROXY_URL` | WebSocket relay URL |
| `AURA_PROXY_AUTH_TOKEN` | Auth token for proxy (client and server side) |
| `PORT` | Proxy server listen port (default 8080) |
| `AURA_CLOUD_REGION` | GCP region for `deploy-proxy.sh` (default `us-central1`) |
| `RUST_LOG` | Log filter for both daemon and proxy (e.g. `debug`, `aura_gemini=trace`) |

Data directory: `~/Library/Application Support/aura/` — contains `aura.db` (SQLite), `models/` (wake word `.rpw` files), `logs/`.
