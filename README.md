# Aura

A voice-first, always-present, fully local AI companion.

"Hey Aura" — and it's there. No cloud. No browser. No app window.

## Prerequisites

- Rust 1.80+
- macOS 13+ (MVP platform)
- ~2GB disk for models

## Quick Start

```bash
# Clone and build
git clone <repo> && cd aura
cargo build --release

# Download models (first run)
./scripts/download-models.sh

# Run
cargo run --release
```

## Architecture

Aura is a Rust workspace with six crates:

| Crate | Purpose |
|-------|---------|
| `aura-daemon` | Main process, event bus, orchestration |
| `aura-voice` | Audio capture, wake word, STT, TTS |
| `aura-overlay` | GPU-rendered transparent overlay (Skia) |
| `aura-screen` | Screen context via macOS accessibility |
| `aura-llm` | Local LLM intent parsing (llama.cpp) |
| `aura-bridge` | macOS action execution (open apps, search, tile) |

See `docs/plans/2026-03-07-aura-design.md` for the full design document.

## License

Apache-2.0
