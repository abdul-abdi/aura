# Aura v2 Handoff

## Goal
Redesign Aura into a macOS menu bar companion for the **Gemini Live Agent Challenge** (Devpost, deadline March 16, 2026). Dynamic AppleScript generation, witty personality, Cloud Run proxy, session memory, wake word.

## Plan
Full implementation plan: `docs/plans/2026-03-10-aura-v2-implementation.md`
Design doc: `docs/plans/2026-03-10-aura-v2-design.md`

## Execution Mode
Using **Subagent-Driven Development**: fresh subagent per task, spec compliance review after each task, then code quality review. User requested spec reviews for every task.

## Progress (10 of 11 tasks complete)

| Task | Status | Summary |
|------|--------|---------|
| 1. ScriptExecutor | DONE | `aura-bridge/src/script.rs` — runs AppleScript/JXA via osascript with timeout, safety checks. 5 tests. |
| 2. Screen Context | DONE | `aura-screen` rewritten — osascript + pbpaste for frontmost app, windows, clipboard. 4 tests. |
| 3. Rewrite Gemini Tools | DONE | `tools.rs` now has 2 tools (run_applescript, get_screen_context). 4 tests. |
| 4. Remove Old Bridge + Personality | DONE | Deleted actions.rs/macos.rs. Added Aura personality system prompt to GeminiConfig. |
| 5. Proxy URL in GeminiConfig | DONE | `proxy_url` field, `AURA_PROXY_URL` env var, `ws_url()` routing with separator logic. 4 tests. |
| 6. Create aura-menubar | DONE | NSStatusItem colored dot, NSPopover 320x480, ClassDecl click handler, NSTimer 50ms polling. Drop impls. |
| 7. SQLite Session Memory | DONE | `aura-memory` crate — rusqlite SessionMemory with sessions + messages tables. 4 tests. |
| 8. aura-proxy (Cloud Run) | DONE | axum 0.8 WebSocket relay with ping/pong forwarding, close frame handling. Dockerfile. 1 test. |
| 9. Rewrite Daemon | DONE | Full rewrite with menubar, ScriptExecutor, screen context, memory logging. Removed overlay. |
| 10. Wake Word + Greeting | DONE | Context-aware greeting on connect (screen context + time-of-day). Gemini VAD for MVP. |
| 11. Competition Deliverables | PENDING | Architecture doc, README, deploy.sh, demo script |

## Spec Reviews
- Tasks 1-4: PASSED
- Tasks 5-7: PASSED
- Task 8 (aura-proxy): PASSED — exact spec match, plus review fixes (close frames, ping/pong, removed permissive CORS)
- Task 9 (daemon rewrite): PASSED — beneficial deviation: aura-memory as separate crate
- Task 10 (greeting): PASSED — caveat: context logged to memory but not sent to Gemini (needs send_text())

## Code Quality Review Fixes Applied
- Relay now forwards ping/pong frames and sends close frames on disconnect
- Removed `CorsLayer::permissive()` (native app doesn't need CORS)
- Changed silent `let _ =` on memory writes to `if let Err(e) = ... { tracing::warn!(...) }`
- Removed unused `tower-http` dependency

## Key Files
- `crates/aura-bridge/src/script.rs` — ScriptExecutor
- `crates/aura-screen/src/context.rs` — ScreenContext
- `crates/aura-gemini/src/config.rs` — proxy URL + personality prompt
- `crates/aura-menubar/src/` — NSStatusItem, NSPopover, click handler
- `crates/aura-memory/src/store.rs` — SQLite session memory
- `crates/aura-proxy/src/` — Cloud Run WebSocket relay
- `crates/aura-daemon/src/main.rs` — full orchestrator (482 lines)

## Next Step
Task 11: Competition deliverables (architecture doc, README, deploy.sh, demo script).

## Commands
```bash
cargo check --workspace    # must pass after every task
cargo test --workspace     # run full suite — 64 tests, 0 failures
cargo fmt --all            # format before commit
```
