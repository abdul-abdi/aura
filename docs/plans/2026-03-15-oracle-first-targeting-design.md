# Oracle-First Click Targeting Redesign (v2 — Expert-Reviewed)

**Date:** 2026-03-15
**Status:** Approved
**Approach:** B — Oracle-Only (AX removed from click targeting path)

## Problem

AX fires ~100% of the time but is wrong ~90%. The Vision Oracle fires ~0% (gated behind AX miss). The oracle is untested in production.

## Design Decisions

- Accuracy over speed — 1-2s per click is acceptable
- Oracle-Only — AX removed from targeting, kept for context gathering
- Hard 4s click budget — no single click exceeds this regardless of oracle state
- Circuit breaker — oracle auto-disables after repeated failures
- Spiral retries stay fast — no oracle in retry path

## Section 1: Click Targeting Pipeline

```
click(x, y, target?) from Gemini
  |
  +-- 0. Circuit breaker check — if oracle tripped, skip to raw coords
  |
  +-- 1. Capture high-res screenshot (2560px, Q92)
  |      - Include display origin offset for multi-monitor
  |
  +-- 2. 4-second timeout wraps steps 2-3:
  |     +-- Send to Vision Oracle:
  |     |     - screenshot + hint coords (normalized to 0-1000)
  |     |     - target description for semantic matching
  |     |     - single retry on failure
  |     |     - oracle returns refined (ox, oy)
  |     |
  |     +-- Sanity check: if delta > MAX_ORACLE_DELTA (150px), discard
  |
  +-- 3. On oracle success -> click at (ox, oy) + display origin offset
  |
  +-- 4. On oracle failure/timeout/tripped -> click at raw (x, y)
  |     - NO AX snap fallback
  |     - Log warning with failure reason
  |
  +-- 5. Verification + spiral retry (fast, no oracle)
```

### What's removed from click path:
- `element_at_position()` call
- AX center-snapping logic
- The `ax_hit` if/else branching in `tools.rs`

### What's kept:
- AX tree walking for screen context
- Screenshot hash verification
- Spiral retry (fast, no oracle — keeps current `None` behavior)
- `targeting_info` JSON (updated with new fields)

## Section 2: Spiral Retry — No Oracle

Expert review found that oracle + offset hints on the same screenshot is ineffective
and adds 4-64s of latency for no benefit. Spiral retries stay fast:

```
Verification fails (screen unchanged)
  |
  +-- For each spiral offset (up to 4):
  |     - Compute offset coords: (orig_x + dx, orig_y + dy)
  |     - Click at raw offset coords (NO oracle)
  |     - 80ms settle + hash check
  |     - Break on change
  |
  +-- If all retries fail -> return unverified
```

Rationale: If the oracle found the right element and the click didn't register,
the problem is click delivery, not coordinate targeting. Offsets address delivery.

## Section 3: Circuit Breaker

```rust
pub struct VisionOracle {
    client: reqwest::Client,
    api_key: String,
    model: String,
    consecutive_failures: AtomicU32,     // NEW
    tripped_until: AtomicU64,            // NEW — epoch millis
}

const CIRCUIT_BREAKER_THRESHOLD: u32 = 3;
const CIRCUIT_BREAKER_COOLDOWN_SECS: u64 = 30;
```

- After 3 consecutive failures, set `tripped_until` to now + 30s
- `is_available()` checks: `now < tripped_until ? false : true`
- On success, reset `consecutive_failures` to 0
- Logged when circuit opens/closes

## Section 4: Multi-Monitor Fix

The oracle's `denormalize` produces display-local coords (0-relative).
CGEvent expects global coords. On secondary displays, this is wrong.

Fix: `CapturedFrame` already stores `display_bounds` origin info (via `CGDisplay::bounds()`).
Add `display_origin_x` and `display_origin_y` to `CapturedFrame`, then add them after denormalization:

```rust
let (local_x, local_y) = denormalize(norm_x, norm_y, screen_w, screen_h);
let global_x = local_x + frame.display_origin_x;
let global_y = local_y + frame.display_origin_y;
```

This also fixes the pre-existing multi-monitor bug for raw coordinates.

## Section 5: Oracle Prompt (Revised)

### Problems with current prompt:
- Asks for "nearest clickable element" — too vague
- Hint coords in image-space pixels but output in 0-1000 — mixed systems
- 50 max tokens risks truncation if model reasons first
- No failure signal for invisible/occluded elements
- No canvas/empty-space handling
- Part-level `media_resolution` field is invalid API placement

### New prompt:

```
You are a precise UI click targeting system for macOS.

Target: {target_description}
Hint: approximately [{norm_hint_y}, {norm_hint_x}] (0-1000 normalized)

Rules:
- Find the element matching the target description, not just the nearest element to the hint
- Return the CENTER of the element, not an edge
- If multiple elements match, prefer the one closest to the hint
- If the target is on a canvas or content area (not a UI control), return the hint unchanged
- If the target is not visible or is covered by another element, return [0, 0]
- Return ONLY [y, x] normalized to 0-1000. No other text.
```

When no target description: fall back to "find the nearest clickable UI element to the hint"

### Config changes:
- `maxOutputTokens`: 50 -> 100
- Remove part-level `media_resolution` (invalid); keep only `generationConfig.mediaResolution`
- Normalize hint coords to 0-1000 before sending (same coordinate system as output)

## Section 6: MAX_ORACLE_DELTA Guard

```rust
const MAX_ORACLE_DELTA: f64 = 150.0; // logical pixels
```

After oracle returns coords, compute delta from raw hint:
```rust
let delta = ((ox - raw_x).powi(2) + (oy - raw_y).powi(2)).sqrt();
if delta > MAX_ORACLE_DELTA {
    tracing::warn!(delta, "Oracle moved coords too far — discarding");
    // Use raw coords instead
}
```

Prevents the oracle from "teleporting" clicks to unrelated screen areas.

## Section 7: Targeting Info & Telemetry

```json
{
  "vision_oracle": true,
  "raw_coords": [800, 600],
  "oracle_coords": [815.3, 612.7],
  "delta_px": 19.4,
  "elapsed_ms": 1340
}
```

Always includes `elapsed_ms`. On oracle skip (circuit breaker), includes `"oracle_skipped": "circuit_breaker"`.

## Section 8: System Prompt Changes

### Aura Main Prompt (Gemini Live)

**`<vision>`** — add:
```
Click Targeting:
- Your click coordinates are approximate hints — a vision targeting system refines them.
- Focus on clicking the RIGHT area, not the exact pixel.
- ALWAYS include a target description in click() calls.
```

**`click` tool** — updated:
```
click(x, y, target?, button?, click_count?, modifiers?, expected_bounds?):
  Click at screen coordinates. ALWAYS include target — a short UNIQUE description
  of what you're clicking (e.g. "blue Submit button at bottom of form",
  "Safari address bar"). Include label text, color, or position to disambiguate.
  The targeting system uses this to visually locate the exact element.
```

**`<automatic_behaviors>`** — updated:
```
- Click targeting: A vision system refines your coordinates using the target
  description you provide. More descriptive targets = more accurate clicks.
  If unavailable, raw coordinates are used as-is.
- Click auto-retry: if screen doesn't change, system retries at nearby offsets
  (up to 4 times). retry_offset in response confirms.
```

## Section 9: Worst-Case Latency

```
Happy path:
  screenshot (200ms) + oracle (1.5s) + click (50ms) = ~1.75s

Oracle fails once, retries:
  screenshot (200ms) + attempt 1 (fail, up to 4s) + retry (1.5s) + click = ~5.7s
  BUT: 4s total timeout caps this at 4.2s

Oracle circuit-tripped:
  click at raw coords = ~50ms (instant)

Verification fails + spiral retry (no oracle):
  4 retries x (click + 80ms settle + hash check) = ~640ms

ABSOLUTE WORST CASE:
  4s timeout + 50ms click + 100ms settle + 1s verification + 640ms spiral = ~5.8s
```

vs previous worst case of 84s. The 4s budget is the key fix.

## Files to Modify

| File | Change |
|------|--------|
| `aura-gemini/src/vision_oracle.rs` | Add `target`, circuit breaker, normalized hints, fix media_resolution, bump tokens |
| `aura-gemini/src/tools.rs` | Add `target` to click tool schema |
| `aura-gemini/src/config.rs` | Update system prompt |
| `aura-daemon/src/tools.rs` | Replace AX targeting with oracle-first + 4s budget + delta guard |
| `aura-screen/src/capture.rs` | Add `display_origin_x/y` to CapturedFrame |

## What's NOT Changing
- `aura-screen/src/accessibility.rs` — untouched
- `aura-daemon/src/processor.rs` — spiral retry stays as-is (keeps `None` for oracle)
- Non-click tools (move_mouse, type_text, drag, etc.)
- Screenshot capture pipeline (except adding origin fields)

## Known Limitations (Documented, Not Solved)

1. **Cursor placement in text fields** — oracle snaps to field center, not character position
2. **Games/fullscreen media** — no standard UI for oracle to identify
3. **Canvas areas** — mitigated by "return hint unchanged" prompt rule
4. **Very small targets (<12px)** — oracle precision is tight; system prompt steers to `click_element` for native apps
5. **Notifications** — may auto-dismiss during oracle latency (~1.5s)
6. **Secondary display capture** — only captures display under mouse cursor (pre-existing)
