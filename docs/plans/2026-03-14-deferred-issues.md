# Deferred Issues

Issues identified during the core pipeline audit that are deferred due to low impact, high complexity, or requiring design decisions.

## Issues

| ID | Severity | Crate | Description | Reason Deferred |
|----|----------|-------|-------------|-----------------|
| N8 | HIGH | aura-daemon | Verification accepts any hash change — clock tick or notification could count as "screen changed" | Requires design work: region-of-interest hashing, UI-aware diffing, or screenshot comparison. Current approach works 95%+ of the time. |
| N12 | HIGH | aura-daemon | Tool response lost on WebSocket disconnect — no pending-ID tracking or reconnect replay | Requires reconnect architecture: persistent pending-tool queue, idempotency checks, session resumption coordination. Large scope. |
| B1 | CRITICAL | aura-input | Double-click sends 1 down/up pair with click_count flag instead of looping — some Electron apps ignore the flag | Already fixed on fix/core-flow-audit branch (merged). Verify no regression. |
| N2 | HIGH | aura-input | type_text bypasses IME — emoji and CJK characters may not render correctly in some apps | Fix requires clipboard-paste fallback for non-ASCII characters. Needs testing across apps (some apps handle CGEvent Unicode fine). |
| N3 | MEDIUM | aura-input | type_text 10ms inter-keystroke delay is hardcoded, no configurable delay_ms parameter | Low impact — 10ms works for most apps. Adding a parameter increases tool schema complexity for minimal gain. |
| N5 | MEDIUM | aura-input | Numpad keys missing from keycode map (numpad 0-9, enter, operators) | Rarely needed for desktop automation. Add when a user reports needing numpad input. |
| N6 | MEDIUM | aura-daemon | Drag has no pre-move hover settle (click has 40ms) — drag start position may be imprecise | Low reports of drag issues. Add 40ms settle if drag accuracy problems are reported. |
| N37 | LOW | aura-screen | Hash stride too sparse on 5K displays — samples only 8192 pixels from 14.7M | Only affects 5K displays. Increase sample count to 16384 or scale with resolution if change detection issues are reported. |
| N33 | MEDIUM | aura-screen | collect_menu_items premature return on AXMenuItem — no submenu recursion | Affects deeply nested context menus. Most menus are flat. Fix when submenu navigation is needed. |
| N35 | MEDIUM | aura-bridge | Third-party apps skip Automation permission preflight when bundle ID is unknown | Graceful degradation — script runs and fails with a clear error if permission is missing. Preflight is an optimization, not a requirement. |
| N36 | LOW | aura-bridge | contains_shell_atoms checks full script text, not just do shell script blocks | False positives are conservative (blocks more than needed). False negatives are unlikely. Low risk. |
| N15 | LOW | aura-screen | parent_label collected but never surfaced in summary output | Adds context but increases token cost. Evaluate if Gemini needs parent context for disambiguation. |
| B6 | MEDIUM | aura-input | No count/repeat parameter on press_key | Gemini can call press_key multiple times. Adding count is a convenience, not a necessity. |

## When to Revisit

- **N8, N12**: Next major architecture iteration (reconnect overhaul)
- **N2**: When CJK/emoji support is reported broken by users
- **B1**: Already merged — monitor for Electron app regressions
- **N5, N6, N37**: When specific user reports indicate these are causing problems
- **N33, N35, N36, N15, B6, N3**: Low priority, fix opportunistically
