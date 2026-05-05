# Aura — Project Rules

## What Aura is

A local-only macOS voice assistant. Single Swift target, single binary, no servers, no cloud infra beyond direct calls to `generativelanguage.googleapis.com`. You speak, Aura streams your voice + screen to Gemini's Live API, Gemini speaks back or controls the Mac.

The complete design rationale is in `docs/superpowers/specs/2026-04-29-aura-v3-design.md`.

## Stack

- Swift 5.10+ (Swift 6 strict concurrency on)
- SwiftPM, macOS 14+
- AVFoundation, ScreenCaptureKit, Network framework, ApplicationServices (AX)
- System SQLite (no GRDB)
- `URLSessionWebSocketTask` for the Gemini Live WebSocket

## Layout

- `AuraV3/Package.swift` — SwiftPM manifest. Library + thin app + CLI split.
- `AuraV3/Sources/AuraV3/` — library (everything testable lives here)
- `AuraV3/Sources/AuraV3App/main.swift` — thin app launcher
- `AuraV3/Sources/aura/main.swift` — CLI placeholder (Plan 4: `aura support-bundle`)
- `AuraV3/Tests/AuraV3Tests/` — unit + integration tests
- `AuraV3/Resources/` — Info.plist, Aura.entitlements
- `AuraV3/scripts/dev-codesign.sh` — ad-hoc HR + entitlements after every `swift build`
- `docs/superpowers/specs/` — design specs
- `docs/superpowers/plans/` — implementation plans (4 plans for v3)

## Build / test

```bash
cd AuraV3
swift build
swift test                                    # unit suite
bash scripts/dev-codesign.sh                  # ad-hoc HR + entitlements
swift run AuraV3                              # launch
AURA_LIVE_TEST=1 swift test --filter Gemini   # opt-in live integration tests
```

## Real-time pipeline discipline

- Make ONE change at a time to audio / streaming / VAD code. Verify it works before the next change.
- Never batch speculative fixes — they compound and become impossible to debug.
- If a fix makes things worse, revert it immediately before trying an alternative. Do not layer fixes on top of broken state.

## No paid Apple Developer Program

The user does not have a Developer ID. Ad-hoc codesigning only (`codesign --sign -`). No notarization. No DMG distribution to other machines. Hardened Runtime via `--options runtime` works ad-hoc and is required for `CGEvent.post` synthetic input on macOS 15+.

## Naming you'll see in the codebase

- `Log` — the SQLite event-log actor (`Memory/Log.swift`). Singular source of truth.
- `Loggers` — the OSLog category enum (`App/Logging.swift`). Plural; collection of category loggers. **Never use `Log.app.info(...)` for OSLog — that's `Loggers.app.info(...)`.** The plan author hit this collision; we resolved by renaming the OSLog wrapper.

## What's where

| Subsystem                                                               | Folder                        |
| ----------------------------------------------------------------------- | ----------------------------- |
| Event log + SQLite + writer Task                                        | `Sources/AuraV3/Memory/`      |
| Gemini Live transport + rotation + reconnect                            | `Sources/AuraV3/Transport/`   |
| Mic capture + playback + barge-in                                       | `Sources/AuraV3/Voice/`       |
| ScreenCaptureKit + AX tree + perceptual hash                            | `Sources/AuraV3/Vision/`      |
| Sleep/wake + network + audio-config + permission watchdogs + supervisor | `Sources/AuraV3/Observation/` |
| EffectLog (correlation_id discipline + orphan detector)                 | `Sources/AuraV3/Action/`      |
| AppCoordinator + AppState + Config + Keychain + Logging                 | `Sources/AuraV3/App/`         |
| Menu bar + floating panel + onboarding                                  | `Sources/AuraV3/UI/`          |
| AsyncChannel + MonotonicClock + JSONValue                               | `Sources/AuraV3/Util/`        |

## Plan status

- Plan 1 (Voice Loop Foundation) — **complete** (tag `plan-1-complete`)
- Plan 2 (Tools) — pending
- Plan 3 (Mind: memory lenses, Context.swift, proactivity) — pending
- Plan 4 (Surface: Mind area UI, support-bundle CLI, ship-gate fixtures) — pending
