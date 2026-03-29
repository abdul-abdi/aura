# Handoff: Aura v2 Ground-Up Rewrite

## Goal
Rebuild Aura from "cool demo" to "daily driver" — a Mac companion that notices things, remembers preferences, and shuts up when it should.

## Branch
Create `v2` from `main` before starting: `git checkout main && git checkout -b v2`

## What's Done (Planning)
- Full design spec: `docs/superpowers/specs/2026-03-29-aura-v2-design.md`
- Phase 1+2 plan (Voice MVP): `docs/superpowers/plans/2026-03-29-aura-v2-phase1-voice-mvp.md`
- Phase 3 plan (Eyes & Hands): `docs/superpowers/plans/2026-03-29-aura-v2-phase3-eyes-and-hands.md`
- Phase 4 plan (Intelligence): `docs/superpowers/plans/2026-03-29-aura-v2-phase4-intelligence.md`
- Roundtable debate: `~/Developer/roundtables/2026-03-29-aura-v2-architecture.md`
- Model already upgraded to `gemini-3.1-flash-live-preview` in `config.rs`
- Custom barge-in already removed from `orchestrator.rs` and `processor.rs`

## What's Already Changed on `main` (Uncommitted-safe, committed)
- `crates/aura-gemini/src/config.rs` — model ID updated to 3.1 Flash Live
- `crates/aura-daemon/src/orchestrator.rs` — custom barge-in removed, mic bridge simplified
- `crates/aura-daemon/src/processor.rs` — rms_energy, calibrate_barge_in_threshold, and tests removed
- All tests pass (`cargo test` — 385 tests)

## What Needs to Happen
Execute the plans in order. Each phase produces working software:

### Phase 1+2: Voice Round-Trip MVP (START HERE)
7 tasks. Build the end-to-end voice pipeline:
1. Swift AudioEngine with AVAudioEngine + voice processing (native echo cancellation)
2. Extended IPC protocol (AudioFrame, PlaybackAudio, mode messages)
3. Wire audio engine into DaemonConnection
4. Gemini 3.1 Flash Live setup (proactivity, VAD, affective dialog)
5. Rewrite daemon orchestrator for Swift-native audio
6. End-to-end smoke test (< 300ms latency target)
6.5. Profile base64 IPC overhead (decide if binary IPC needed)
7. Remove dead v1 code (aura-voice, aura-menubar crates)

**Milestone:** User speaks, Gemini responds with voice, barge-in works naturally.

### Phase 3: Eyes & Hands
9 tasks. Screen capture + Navigator planner + 20+ tools.

**Milestone:** Aura can see the screen and take actions (click, type, scroll, etc.)

### Phase 4: Intelligence Layer
13 tasks. Memory (append-only facts + three-tier profile), proactive scorer, autonomy slider.

**Milestone:** Aura remembers preferences, speaks unprompted when useful, respects Ghost/Copilot/Autopilot modes.

## Key Architecture Decisions
- **Swift + Rust hybrid** at the natural seam (Swift = Apple APIs, Rust = computation)
- **No gateway** — direct WebSocket to Gemini 3.1 Flash Live
- **Local SQLite only** — no Firestore, no Cloud Run for intelligence layer
- **Append-only facts** — never update, never delete. Profile is a derived projection.
- **Three-tier memory injection** — 400t stable profile + 200t corrections + 300t last session = 900 tokens
- **Single proactive scorer** — one function, mode-specific thresholds, dismissal backoff
- **Clock trait in BackoffState** — for testable time (Hickey's review fix)
- **ScreenCaptureKit permission detection** — 5 consecutive failures = surface error (Carmack's review fix)

## Expert Reviews
All three approved (Karpathy, Carmack, Hickey):
- Karpathy: "Ship it. The audio harness is your zero-to-one."
- Carmack: "Go. Phase 3 is the shipping risk — budget extra time."
- Hickey: "Go. Memory architecture is correct. Inject a clock for testability."

## Execution Strategy
Use `superpowers:subagent-driven-development` skill. Fresh subagent per task, review between tasks. Or use `superpowers:executing-plans` for inline batch execution.

## Important Notes
- v1 line numbers in plans may have drifted — always verify before editing
- Audio pipeline is the foundation — prove it works before anything else
- Test on multiple audio devices (built-in mic, AirPods, external USB)
- The proactive scorer thresholds (0.85/0.60) are starting points — tune with real usage
