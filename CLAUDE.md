# Aura Project Rules

## Real-Time Pipeline Discipline
- Make ONE change at a time to audio/streaming/VAD pipeline code. Verify it works before making the next change.
- Never batch multiple speculative fixes — they compound and become impossible to debug.
- If a fix makes things worse, revert it immediately before trying an alternative. Do not layer fixes on top of broken state.

## Stack
- Rust crates in `crates/`, Swift UI in `AuraApp/`
- Run `cargo test` after Rust changes, `cargo clippy -- -D warnings` before committing
- Infrastructure in `infrastructure/` (Cloud Run, Docker)

## Key Docs
- Architecture: @ARCHITECTURE.md
- Current progress: @HANDOFF.md
