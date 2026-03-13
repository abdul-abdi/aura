# Pipeline Overhaul: Making Aura Work With All Apps

**Date:** 2026-03-13
**Status:** Approved
**Goal:** Address the three core frustrations — Electron app blindness, slow multi-step actions, and unreliable voice interaction — through 5 targeted improvements.

## Problem Statement

Aura's core pipeline has three compounding pain points:
1. **Can't control Electron apps** (Slack, VS Code, Notion, Discord) — empty AX trees, no AppleScript support
2. **Too slow** — 5-action sequences take 14+ seconds due to serial verification (2.75s worst case per action)
3. **Unreliable voice** — energy-based gating triggers on ambient noise, barge-in detection is fragile

## Implementation Order (by ROI)

| Phase | Improvement | Effort | Impact |
|-------|------------|--------|--------|
| 1 | Reduce verification overhead | Low | Highest — 60-80% overhead reduction |
| 2 | Add VAD (webrtc-vad) | Low | High — eliminates false audio triggers |
| 3 | Vision fallback v1 (coordinate fallback) | Low-Med | Critical — unblocks Electron apps |
| 4 | Adaptive AX depth | Low-Med | Medium — helps complex native apps |
| 5 | Safe-to-pipeline action pairs | Medium | High — faster multi-step tasks |
| 6 | Vision SoM v2 (structured overlay) | High | Critical — full visual targeting |

---

## Phase 1: Adaptive Verification

### Current State
- **Settle delay:** 150ms (hardcoded)
- **Poll loop:** 10 iterations × 200ms = 2000ms max
- **Post-state AX:** 600ms timeout, blocking
- **Total worst case:** 2750ms per state-changing tool
- **Location:** `processor.rs` lines 516-540, `tool_helpers.rs`

### Design

**1a. Early-exit polling**
Replace fixed 10×200ms with adaptive polling:
- Poll every **50ms** instead of 200ms
- Exit immediately on first screen hash change
- Max timeout reduced to **1000ms** (from 2000ms)
- Common case (UI updates in <100ms): 50ms settle + 50ms poll = **100ms** (was 350ms+)

**1b. Action-type settle delays**
| Action Type | Settle Delay | Rationale |
|-------------|-------------|-----------|
| `type_text`, `press_key` | 30ms | Keystroke processing is near-instant |
| `click`, `click_element` | 100ms | Click handlers may trigger animations |
| `activate_app`, `click_menu_item` | 150ms | App activation/menu opening is slower |
| `run_applescript` | 200ms | Script effects are unpredictable |
| `scroll`, `drag` | 50ms | Continuous actions, fast UI response |

**1c. Async post-state AX capture**
- Don't block tool response on AX capture
- Send basic `{success: true, settled: true/false}` immediately
- Fire AX capture in background, send as supplementary context via the next screen frame's metadata
- If AX capture completes within 200ms (common for simple apps), include it in the response

### Expected Impact
- Common case per-action: **100-250ms** (from 350-2750ms)
- 5-action sequence: **0.5-1.25s** (from 1.75-13.75s)

---

## Phase 2: Voice Activity Detection

### Current State
- **Energy threshold:** 0.05 RMS (calibrated to 0.05-0.15 range)
- **Calibration:** mean + 3σ over 100 chunks (~500ms)
- **Barge-in:** 1.8× multiplier during playback, 3 consecutive frames required
- **Location:** `orchestrator.rs` lines 29-48, `audio.rs`

### Design

**Use `webrtc-vad` crate** (pure Rust, ~50KB, Google's production VAD)

**Integration point:** Replace the energy check in the mic bridge loop (`orchestrator.rs`):

```
Current: rms_energy(&samples) > threshold → forward to Gemini
New:     vad.is_voice_segment(&samples) → forward to Gemini
```

**Configuration:**
- VAD mode: `Quality` (most accurate, still <1ms per frame)
- Frame size: 480 samples at 16kHz = 30ms frames (VAD requires 10/20/30ms)
- Speech padding: 150ms pre-roll (buffer last 5 frames, include them when speech starts)
- Hangover: 300ms after last speech frame before stopping (prevents clipping end of utterance)

**Barge-in improvement:**
- During playback: VAD replaces energy × 1.8 multiplier
- VAD is inherently better at ignoring speaker output (it models speech characteristics, not just energy)
- Keep consecutive-frame requirement (3 frames = 90ms of sustained speech) as safety gate

**Fallback:** If VAD initialization fails (shouldn't happen with pure Rust), fall back to current energy-based gating. Log warning.

### Expected Impact
- Eliminate ~90% of false audio sends from ambient noise
- More reliable barge-in during playback
- Reduced token costs (less noise sent to Gemini)

---

## Phase 3: Vision Fallback v1

### Current State
- `click_element` searches AX tree by label/role → fails silently on Electron apps (empty tree)
- `click` works by coordinates but Gemini doesn't always know precise pixel positions
- Gemini already receives screen JPEGs with `frame_dims` coordinate mapping
- **Location:** `tools.rs` (click_element), `tool_helpers.rs` (click fallback tiers)

### Design

**When `click_element` finds 0 AX matches:**
1. Instead of returning `{success: false, error: "Element not found"}`, return:
```json
{
  "success": false,
  "error": "No accessibility elements found — this app may not support accessibility. Use the 'click' tool with coordinates based on what you see in the screenshot.",
  "hint": "look_at_screenshot",
  "frame_dims": { "width": 1920, "height": 1080, "scale": 2 }
}
```
2. Include the current `frame_dims` so Gemini can map visual positions to click coordinates
3. Update Gemini system prompt to explicitly teach this fallback pattern

**System prompt addition:**
```
When click_element fails with hint "look_at_screenshot", examine the most recent
screenshot to visually locate the target element. Use the 'click' tool with the
pixel coordinates of the element's center. The frame_dims in the error tell you
the coordinate space.
```

**Why this works:** Gemini is already a vision model receiving screenshots. It can identify UI elements visually. The gap was that `click_element` failure was a dead end — now it's a redirect to coordinate-based clicking.

### Expected Impact
- Electron apps become partially controllable via visual targeting
- No new dependencies or major code changes
- Gemini's coordinate accuracy is imperfect (~85-90%) but far better than "can't do it at all"

---

## Phase 4: Adaptive AX Depth

### Current State
- **MAX_ELEMENTS:** 50
- **MAX_DEPTH:** 5
- **TIMEOUT_MS:** 500
- **MAX_CHILDREN_PER_INTERACTIVE:** 10
- **Location:** `accessibility.rs` lines 18-20

### Design

**Density-based adaptive depth:**

```
Phase 1 (fast probe): Walk to depth 3, 200ms timeout
  → If >= 20 interactive elements found: tree is rich
      Continue to depth 7, 150 max elements, 800ms total timeout
  → If 5-19 elements found: tree is moderate
      Continue to depth 5, 80 max elements, 600ms total timeout (current-like)
  → If < 5 elements found: tree is sparse/broken
      Stop. Flag as "sparse_ax_tree" in response metadata
      This signals Phase 3's vision fallback should be preferred
```

**Why density-based, not per-app profiles:**
- No app list to maintain
- Automatically adapts to apps we've never seen
- Handles the same app having rich trees in some views and sparse in others (e.g., VS Code editor view vs. settings)

**Additional: Focused subtree walk**
When `click_element` is called with a target near specific coordinates (from a prior click or screen region), walk only the AX subtree under the window at those coordinates. This avoids walking irrelevant windows/UI.

### Expected Impact
- Complex native apps (VS Code native views, Xcode): 50→150 visible elements
- Broken Electron apps: faster failure (200ms instead of 500ms), clean handoff to vision
- Average AX walk time roughly unchanged (fast probe exits early for simple/broken apps)

---

## Phase 5: Safe-to-Pipeline Action Pairs

### Current State
- All state-changing tools serialized through `state_change_mutex`
- Each tool gets full verification cycle before the next starts
- **Location:** `processor.rs` lines 428-614

### Design

**Define "continuation" pairs that skip intermediate verification:**

| First Action | Continuation | Rationale |
|-------------|-------------|-----------|
| `type_text` | `press_key` | Type then Enter/Tab — natural sequence |
| `press_key` | `press_key` | Key combos (Cmd+C, Cmd+V) |
| `click` / `click_element` | `type_text` | Click field then type — field is already focused |
| `activate_app` | `click` / `click_element` | Open app then interact |

**Implementation:**
- After executing a state-changing tool, check if the next queued tool forms a continuation pair
- If yes: skip verification on the first tool, execute the second immediately with only a 30ms micro-settle
- Apply full verification only after the last action in the continuation chain
- Max chain length: 3 (safety cap — don't let errors cascade too far)

**Safety rail:** If any action in a chain returns an error, break the chain and verify immediately.

**Mutex behavior:** The chain holds the mutex for its full duration. Other tools still wait.

### Expected Impact
- "Type text and press Enter": 2 × 2.75s → ~300ms
- "Click field, type, submit": 3 × 2.75s → ~500ms
- Perceived responsiveness transforms from "sluggish robot" to "fast assistant"

---

## Phase 6: Vision SoM v2 (Future)

**Deferred — design separately after Phases 1-5 ship.**

Set-of-Mark overlay: number visually distinct interactive regions on the screenshot before sending to Gemini. Gemini says "click mark 7" instead of estimating coordinates. This requires:
- Region detection (edge detection or segmentation model)
- Overlay rendering on the JPEG
- Coordinate mapping from mark ID to screen position
- Significant new code and possibly a vision model dependency

**Trigger for starting Phase 6:** After Phases 1-5, if coordinate-based clicking (Phase 3) has >15% miss rate in real usage.

---

## Risks and Mitigations

| Risk | Mitigation |
|------|-----------|
| Faster verification misses slow UI changes | Keep 1s max timeout; apps that need more are rare |
| VAD clips beginning of speech | 150ms pre-roll buffer; conservative threshold |
| Vision coordinate clicks miss target | Include frame_dims for coordinate mapping; user can retry |
| Deep AX walks hang on pathological apps | Density probe exits in 200ms for sparse trees; total timeout capped |
| Continuation chains cascade errors | Max chain length 3; break on any error; full verify at chain end |

## Testing Strategy

- **Phase 1:** Benchmark 10-action sequences before/after. Measure p50/p95 total time.
- **Phase 2:** Record audio sessions with ambient noise. Measure false-trigger rate before/after.
- **Phase 3:** Test click_element on 5 Electron apps (Slack, VS Code, Notion, Discord, Figma). Measure success rate.
- **Phase 4:** Compare AX element counts on Xcode, VS Code, Finder before/after.
- **Phase 5:** Time multi-step sequences (type+enter, click+type+enter) before/after.
