# Aura v3 — In-Progress Handoff

> Read this BEFORE doing anything else if you're picking up where the previous session left off. The full source of truth for v3 is `docs/superpowers/specs/2026-04-29-aura-v3-design.md`. The 4 plans in `docs/superpowers/plans/` are the canonical task lists; what's below is the _scoped_ MVP we're aiming for in this work block.

## Goal of this work block

Get Aura genuinely working end-to-end: voice in → Gemini → voice out, with a small set of tools that prove the action loop, a minimal Mind module that proves memory, and enough surface that you can see what's happening. Then test it with a real Gemini API key.

The full plans (Plans 2/3/4 = ~7,300 lines of spec) describe months of work. **We are not implementing all of it in one session.** We're cutting hard to a credible MVP.

## Scope cuts (committed)

### Plan 2 — Tools (cut from 27 → 4)

**Shipping:** ToolRegistry actor, EffectLog wiring, `remember`, `recall` (FTS5-only), `forget`, `clipboard.read`.
**Deferred:** All 22 other tools. Specifically — find_ax / find_vision / find_ocr targeting (B), keyboard / mouse / scroll interaction (C), all 12 app-native tools (D), all 5 browser/CDP tools (E), AppleScript runner (F), run_playbook (H). The pattern proves; the count doesn't.

### Plan 3 — Mind (cut from full to Context.swift only)

**Shipping:** `Context.swift` with pure `sessionStartContext` + `rotationDigest` (deterministic, fixture-tested), versioned `Resources/persona.md`, IDPI defense block, FTS5-only recall lens.
**Deferred:** Embeddings (per amendment BB — eval first, default to FTS5), consolidation worker, self-model, all 7 policies (drift detector, calendar nudge, etc.), SubgoalInferrer, RollingSummary, DemonstrationRecorder.

### Plan 4 — Surface (cut to live transcript only)

**Shipping:** Event-log-driven live transcript view in the floating panel showing user turns, assistant turns, tool calls inline.
**Deferred:** Full Mind area (7 sections), dynamic prompt suffix view, replayable targeting view, conversation timeline shape, demonstration overlay, support-bundle CLI, all 4 ship gates (drift fixtures, context fixtures, rotation fuzz, torture test).

## What's done

- Plan 1 (Voice Loop Foundation) — complete (tag `plan-1-complete`). 83 unit tests pass.
- WIP voice-loop UX fixes committed (`debe654`):
  - Audio config feedback-loop fix (settle window in AudioCapture)
  - Edit menu wiring so Cmd+V works in NSTextField
  - .regular activation policy during onboarding
  - Onboarding paste button + URL hint
  - dev-bundle-and-run.sh script

## What's pending

- [ ] Live integration test (`AURA_LIVE_TEST=1`) — needs API key in Keychain
- [ ] Bundle + onboard + speak round-trip — same dependency
- [ ] Plan 2 MVP — ToolRegistry + 4 tools
- [ ] Plan 3 MVP — Context.swift
- [ ] Plan 4 MVP — live transcript view
- [ ] End-to-end demo path (see "Demo path" below)

## Demo path (the success criterion)

1. `swift build && bash scripts/dev-bundle-and-run.sh`
2. Onboard with Gemini API key (paste from clipboard).
3. Click menu bar dot, ⌘⇧A opens panel.
4. Speak: "Remember that I prefer concise responses."
5. Confirm in event log: `kind='fact'` row inserted via `remember` tool.
6. Quit, relaunch, ⌘⇧A.
7. Speak: "What do you remember about me?"
8. Aura voices back: "you prefer concise responses" — recalled via `recall` tool.

If step 8 produces voice output containing the fact, the MVP is done.

## Key user-context decisions

- **No paid Apple Developer Program** — ad-hoc codesign only (`scripts/dev-codesign.sh` for plain binary, `scripts/dev-bundle-and-run.sh` for full Aura.app bundle).
- **Discipline:** "Make ONE change at a time to audio / streaming / VAD code. Verify it works before the next change. Never batch speculative fixes." (CLAUDE.md)
- **TDD:** Where assumption isn't already covered by an existing test, write the failing test first, then the impl.
- **Subagents:** Serialize between phases (Plan 2 before Plan 3 before Plan 4); parallelize within phases. Every Agent call gets explicit `model` (Sonnet for bounded impl, Opus for design).

## Key files / where things live

| Subsystem                           | Folder                       |
| ----------------------------------- | ---------------------------- |
| Event log + SQLite + writer Task    | `Sources/Aura/Memory/`       |
| Gemini Live transport + rotation    | `Sources/Aura/Transport/`    |
| Mic capture + playback + barge-in   | `Sources/Aura/Voice/`        |
| ScreenCaptureKit + AX tree          | `Sources/Aura/Vision/`       |
| Sleep/wake + watchdogs + supervisor | `Sources/Aura/Observation/`  |
| EffectLog correlation_id            | `Sources/Aura/Action/`       |
| AppCoordinator + Config + Logging   | `Sources/Aura/App/`          |
| Menu bar + panel + onboarding       | `Sources/Aura/UI/`           |
| Conversation/Turn types             | `Sources/Aura/Conversation/` |
| AsyncChannel + JSONValue + Cosine   | `Sources/Aura/Util/`         |

New folders this work block will add:

- `Sources/Aura/Tools/` — ToolRegistry + tool implementations
- `Sources/Aura/Mind/` — Context.swift + Lenses

## Naming gotcha

`Log` (singular) = SQLite event-log actor (`Memory/Log.swift`).
`Loggers` (plural) = OSLog category enum (`App/Logging.swift`).
**Never use `Log.app.info(...)` for OSLog — that's `Loggers.app.info(...)`.**
