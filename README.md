# Aura

Local-only macOS voice assistant. Single Swift binary. You speak, Aura streams your voice + your screen to Gemini's Live API, and Gemini speaks back or directly controls the Mac (clicks, types, runs curated scripts).

No servers. No cloud infra. The only network egress is `generativelanguage.googleapis.com` (Live WebSocket + Flash REST). Your API key lives in the macOS login Keychain.

## Status

Plan 1 (Voice Loop Foundation) is complete. The app launches, hears you, sees your screen, Gemini replies, sessions rotate every 9 minutes, lid-close + network changes are handled. Tools land in Plan 2.

## Build & run

Requires macOS 14+. Tested on Apple Silicon.

```bash
swift build
bash scripts/dev-codesign.sh   # ad-hoc Hardened Runtime + entitlements
swift run Aura
```

First launch shows an onboarding window: enter a Gemini API key, grant mic / screen / accessibility permissions. The key is stored in your login Keychain — never written to disk.

## Tests

```bash
swift test                                          # 66 unit tests
AURA_LIVE_TEST=1 swift test --filter Gemini         # opt-in live integration
```

## Documentation

- Design spec: `docs/superpowers/specs/2026-04-29-aura-v3-design.md`
- Implementation plans: `docs/superpowers/plans/2026-04-29-aura-v3-plan-{1,2,3,4}-*.md`
- Project rules: `CLAUDE.md`

## License

See `LICENSE`.
