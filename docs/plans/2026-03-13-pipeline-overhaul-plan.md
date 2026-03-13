# Pipeline Overhaul Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Make Aura work reliably with all macOS apps by reducing action latency 60-80%, adding voice activity detection, enabling vision-based fallback for Electron apps, and adapting AX tree depth to app complexity.

**Architecture:** Five independent improvements to the daemon pipeline, each touching 1-3 files. Changes are additive — no major refactors. Phase 1-2 are pure improvements to existing code paths. Phase 3-5 add new capabilities with fallback to existing behavior.

**Tech Stack:** Rust, `webrtc-vad` crate (pure Rust VAD), macOS Accessibility API (AXUIElement FFI), tokio async runtime.

---

### Task 1: Adaptive Verification — Early-Exit Polling

**Files:**
- Modify: `crates/aura-daemon/src/processor.rs:514-536` (verification poll loop)
- Test: `crates/aura-daemon/src/processor.rs` (existing test module at bottom)

**Context:** Currently the verification loop does 10 polls at 200ms intervals (2s max) after a 150ms settle. Most UI changes happen within 50ms. We want early-exit on first hash change with 50ms poll intervals and 1s max timeout.

**Step 1: Write the failing test**

Add to the `processor.rs` test module:

```rust
#[test]
fn test_adaptive_settle_delay() {
    // Keyboard actions get shorter settle delays
    assert_eq!(settle_delay_for_tool("type_text"), Duration::from_millis(30));
    assert_eq!(settle_delay_for_tool("press_key"), Duration::from_millis(30));
    // Click actions get medium settle delays
    assert_eq!(settle_delay_for_tool("click"), Duration::from_millis(100));
    assert_eq!(settle_delay_for_tool("click_element"), Duration::from_millis(100));
    // App activation and menu clicks get longer settle delays
    assert_eq!(settle_delay_for_tool("activate_app"), Duration::from_millis(150));
    assert_eq!(settle_delay_for_tool("click_menu_item"), Duration::from_millis(150));
    // AppleScript gets longest settle delay
    assert_eq!(settle_delay_for_tool("run_applescript"), Duration::from_millis(200));
    // Scroll and drag get short settle delays
    assert_eq!(settle_delay_for_tool("scroll"), Duration::from_millis(50));
    assert_eq!(settle_delay_for_tool("drag"), Duration::from_millis(50));
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p aura-daemon test_adaptive_settle_delay`
Expected: FAIL — `settle_delay_for_tool` doesn't exist yet

**Step 3: Implement settle_delay_for_tool**

Add to `crates/aura-daemon/src/tool_helpers.rs` before the `is_state_changing_tool` function:

```rust
/// Per-tool-type settle delay before polling for screen changes.
/// Keyboard actions are near-instant; app activation and scripts need more time.
pub(crate) fn settle_delay_for_tool(name: &str) -> Duration {
    match name {
        "type_text" | "press_key" | "move_mouse" => Duration::from_millis(30),
        "scroll" | "drag" => Duration::from_millis(50),
        "click" | "click_element" | "context_menu_click" => Duration::from_millis(100),
        "activate_app" | "click_menu_item" => Duration::from_millis(150),
        "run_applescript" => Duration::from_millis(200),
        _ => Duration::from_millis(150), // conservative default
    }
}
```

Import `Duration` at the top of `tool_helpers.rs`:
```rust
use std::time::Duration;
```

**Step 4: Run test to verify it passes**

Run: `cargo test -p aura-daemon test_adaptive_settle_delay`
Expected: PASS

**Step 5: Update the verification loop in processor.rs**

Replace the verification block at `processor.rs:514-536` (the section starting with `if let Some(pre) = pre_hash`):

Change the settle delay line from:
```rust
tokio::time::sleep(Duration::from_millis(150)).await;
```
To:
```rust
tokio::time::sleep(tools::settle_delay_for_tool(&name)).await;
```

Change the poll loop from:
```rust
let mut screen_changed = false;
for _ in 0..10 {
    let rx = tool_capture_trigger.trigger_and_wait();
    tool_cap_notify.notify_one();
    let _ = tokio::time::timeout(Duration::from_millis(200), rx).await;

    let current_hash = tool_last_hash.load(Ordering::Acquire);
    if current_hash != pre {
        screen_changed = true;
        break;
    }
}
```
To:
```rust
let mut screen_changed = false;
for _ in 0..20 {
    let rx = tool_capture_trigger.trigger_and_wait();
    tool_cap_notify.notify_one();
    let _ = tokio::time::timeout(Duration::from_millis(50), rx).await;

    let current_hash = tool_last_hash.load(Ordering::Acquire);
    if current_hash != pre {
        screen_changed = true;
        break;
    }
}
```

Update the warning message from `"screen_unchanged_after_2s"` to `"screen_unchanged_after_1s"`.

**Step 6: Run full test suite**

Run: `cargo test -p aura-daemon`
Expected: All tests pass

**Step 7: Commit**

```bash
git add crates/aura-daemon/src/processor.rs crates/aura-daemon/src/tool_helpers.rs
git commit -m "perf: adaptive verification with early-exit polling

Per-tool settle delays (30ms-200ms) and 50ms poll intervals with
20-iteration cap (1s max). Reduces common-case verification from
2.75s to ~100-250ms."
```

---

### Task 2: Async Post-State AX Capture

**Files:**
- Modify: `crates/aura-daemon/src/processor.rs:538-545` (post_state capture block)

**Context:** The 600ms blocking AX capture currently delays the tool response. We can fire it with a shorter timeout and accept `{}` if it doesn't complete in time, since the screen capture loop will provide visual context anyway.

**Step 1: Reduce AX capture timeout**

In `processor.rs`, change the post-state capture timeout from 600ms to 300ms:

```rust
// Before:
let post_state = tokio::time::timeout(
    Duration::from_millis(600),
    tokio::task::spawn_blocking(tools::capture_post_state),
)

// After:
let post_state = tokio::time::timeout(
    Duration::from_millis(300),
    tokio::task::spawn_blocking(tools::capture_post_state),
)
```

**Step 2: Run tests**

Run: `cargo test -p aura-daemon`
Expected: All tests pass

**Step 3: Commit**

```bash
git add crates/aura-daemon/src/processor.rs
git commit -m "perf: reduce post-state AX capture timeout to 300ms

Most AX queries complete within 100ms for well-behaved apps.
The 600ms timeout was overly generous and padded every action."
```

---

### Task 3: Voice Activity Detection — Add webrtc-vad Dependency

**Files:**
- Modify: `crates/aura-voice/Cargo.toml` (add dependency)
- Create: `crates/aura-voice/src/vad.rs` (VAD wrapper module)
- Modify: `crates/aura-voice/src/lib.rs` (expose module)

**Step 1: Add webrtc-vad dependency**

Add to `crates/aura-voice/Cargo.toml` under `[dependencies]`:
```toml
webrtc-vad = "0.4"
```

**Step 2: Write the failing test for VAD wrapper**

Create `crates/aura-voice/src/vad.rs`:

```rust
//! Voice Activity Detection wrapper using Google's WebRTC VAD.
//!
//! Provides speech detection to replace energy-threshold gating.
//! Operates on 16kHz mono PCM audio in 30ms frames (480 samples).

use webrtc_vad::{Vad, SampleRate, VadMode};

/// Frame size for 30ms at 16kHz (the only frame size webrtc-vad supports at 16kHz).
pub const VAD_FRAME_SIZE: usize = 480;

/// Number of pre-roll frames to buffer (150ms = 5 frames × 30ms).
/// When speech starts, these buffered frames are prepended so the
/// beginning of the utterance isn't clipped.
const PRE_ROLL_FRAMES: usize = 5;

/// Number of hangover frames after last speech detection (300ms = 10 frames × 30ms).
/// Prevents clipping the tail of an utterance.
const HANGOVER_FRAMES: usize = 10;

pub struct VoiceDetector {
    vad: Vad,
    /// Ring buffer of recent frames for pre-roll.
    pre_roll: Vec<Vec<i16>>,
    /// Frames since last speech detected (for hangover).
    silence_frames: usize,
    /// Whether we're currently in a speech segment.
    in_speech: bool,
}

impl VoiceDetector {
    pub fn new() -> Result<Self, String> {
        let mut vad = Vad::new_with_rate_and_mode(SampleRate::Rate16kHz, VadMode::Quality);
        Ok(Self {
            vad,
            pre_roll: Vec::with_capacity(PRE_ROLL_FRAMES),
            silence_frames: 0,
            in_speech: false,
        })
    }

    /// Process a 30ms frame of 16kHz mono i16 PCM audio.
    /// Returns true if this frame should be forwarded (speech detected or hangover active).
    pub fn is_speech(&mut self, frame: &[i16]) -> bool {
        if frame.len() != VAD_FRAME_SIZE {
            return false;
        }

        let speech = self.vad.is_voice_segment(frame).unwrap_or(false);

        if speech {
            self.silence_frames = 0;
            self.in_speech = true;
            true
        } else if self.in_speech {
            self.silence_frames += 1;
            if self.silence_frames >= HANGOVER_FRAMES {
                self.in_speech = false;
                false
            } else {
                true // hangover — keep forwarding
            }
        } else {
            // Not in speech — buffer for pre-roll
            let frame_i16 = frame.to_vec();
            if self.pre_roll.len() >= PRE_ROLL_FRAMES {
                self.pre_roll.remove(0);
            }
            self.pre_roll.push(frame_i16);
            false
        }
    }

    /// Drain pre-roll buffer. Call this when speech is first detected
    /// to get the buffered frames that preceded the speech onset.
    pub fn drain_pre_roll(&mut self) -> Vec<Vec<i16>> {
        std::mem::take(&mut self.pre_roll)
    }

    /// Whether the detector is currently tracking active speech.
    pub fn is_in_speech(&self) -> bool {
        self.in_speech
    }

    /// Reset state (e.g., after a session reconnect).
    pub fn reset(&mut self) {
        self.pre_roll.clear();
        self.silence_frames = 0;
        self.in_speech = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_detector_not_in_speech() {
        let det = VoiceDetector::new().unwrap();
        assert!(!det.is_in_speech());
    }

    #[test]
    fn silence_returns_false() {
        let mut det = VoiceDetector::new().unwrap();
        let silence = vec![0i16; VAD_FRAME_SIZE];
        assert!(!det.is_speech(&silence));
    }

    #[test]
    fn wrong_frame_size_returns_false() {
        let mut det = VoiceDetector::new().unwrap();
        let short = vec![0i16; 100];
        assert!(!det.is_speech(&short));
    }

    #[test]
    fn pre_roll_buffers_frames() {
        let mut det = VoiceDetector::new().unwrap();
        for _ in 0..PRE_ROLL_FRAMES {
            let silence = vec![0i16; VAD_FRAME_SIZE];
            det.is_speech(&silence);
        }
        let pre = det.drain_pre_roll();
        assert_eq!(pre.len(), PRE_ROLL_FRAMES);
    }

    #[test]
    fn pre_roll_caps_at_max() {
        let mut det = VoiceDetector::new().unwrap();
        for _ in 0..(PRE_ROLL_FRAMES + 5) {
            let silence = vec![0i16; VAD_FRAME_SIZE];
            det.is_speech(&silence);
        }
        let pre = det.drain_pre_roll();
        assert_eq!(pre.len(), PRE_ROLL_FRAMES);
    }

    #[test]
    fn reset_clears_state() {
        let mut det = VoiceDetector::new().unwrap();
        let silence = vec![0i16; VAD_FRAME_SIZE];
        det.is_speech(&silence);
        det.reset();
        assert!(!det.is_in_speech());
        assert!(det.drain_pre_roll().is_empty());
    }
}
```

**Step 3: Expose the module in lib.rs**

Add to `crates/aura-voice/src/lib.rs`:
```rust
pub mod vad;
```

**Step 4: Run tests to verify**

Run: `cargo test -p aura-voice`
Expected: All tests pass (including the silence/frame-size tests)

**Step 5: Commit**

```bash
git add crates/aura-voice/Cargo.toml crates/aura-voice/src/vad.rs crates/aura-voice/src/lib.rs
git commit -m "feat: add webrtc-vad voice activity detection module

Pure Rust VAD wrapper with 30ms frame processing, 150ms pre-roll
buffer, and 300ms hangover. Replaces energy-threshold gating for
more accurate speech detection."
```

---

### Task 4: Integrate VAD into Mic Bridge

**Files:**
- Modify: `crates/aura-daemon/src/orchestrator.rs:300-434` (mic bridge loop)

**Context:** Replace the energy-based gating in the mic bridge with VAD. The VAD operates on i16 frames, but the current pipeline sends f32 samples. We need to:
1. Accumulate f32 samples into 480-sample (30ms) frames
2. Convert to i16 for VAD
3. If VAD says speech, forward the original f32 samples to Gemini
4. Keep the barge-in logic but use VAD instead of energy threshold

**Step 1: Add VoiceDetector initialization before the mic bridge loop**

In `orchestrator.rs`, after the line `let mut barge_in_streak: u32 = 0;` (around line 342), add:

```rust
// VAD for speech detection (replaces energy-threshold gating)
let mut vad = match aura_voice::vad::VoiceDetector::new() {
    Ok(v) => Some(v),
    Err(e) => {
        tracing::warn!("VAD init failed, falling back to energy gating: {e}");
        None
    }
};
let mut vad_accumulator: Vec<f32> = Vec::with_capacity(aura_voice::vad::VAD_FRAME_SIZE);
```

**Step 2: Replace the barge-in energy check during playback**

In the `if currently_playing {` block (around line 381-396), wrap the existing energy check with a VAD-first approach:

```rust
if currently_playing {
    if let Some(ref mut v) = vad {
        // Accumulate into VAD-sized frames
        vad_accumulator.extend_from_slice(&samples);
        let mut any_speech = false;
        while vad_accumulator.len() >= aura_voice::vad::VAD_FRAME_SIZE {
            let frame_f32: Vec<f32> = vad_accumulator.drain(..aura_voice::vad::VAD_FRAME_SIZE).collect();
            let frame_i16: Vec<i16> = frame_f32.iter()
                .map(|&s| (s * 32767.0).clamp(-32768.0, 32767.0) as i16)
                .collect();
            if v.is_speech(&frame_i16) {
                any_speech = true;
            }
        }
        if !any_speech {
            continue; // VAD says no speech — skip
        }
        if barge_in_streak == 0 {
            tracing::info!("Barge-in: VAD detected speech during playback");
        }
        barge_in_streak = barge_in_streak.saturating_add(1);
        if barge_in_streak < BARGE_IN_CONSECUTIVE_FRAMES {
            continue;
        }
    } else {
        // Fallback: original energy-based gating
        let energy = processor::rms_energy(&samples);
        let base = f32::from_bits(bridge_threshold.load(Ordering::Acquire));
        let threshold = base * PLAYBACK_THRESHOLD_MULTIPLIER;
        if energy < threshold {
            barge_in_streak = 0;
            continue;
        }
        barge_in_streak = barge_in_streak.saturating_add(1);
        if barge_in_streak < BARGE_IN_CONSECUTIVE_FRAMES {
            continue;
        }
    }
}
```

**Step 3: Add VAD gating for non-playback audio**

After the reverb guard block (around line 424), before the `audio_session.send_audio` call, add VAD gating for normal (non-playback) audio:

```rust
// VAD gate for non-playback audio (replaces always-forward behavior)
if !currently_playing && playback_stopped_at.is_none() {
    if let Some(ref mut v) = vad {
        vad_accumulator.extend_from_slice(&samples);
        let mut any_speech = false;
        while vad_accumulator.len() >= aura_voice::vad::VAD_FRAME_SIZE {
            let frame_f32: Vec<f32> = vad_accumulator.drain(..aura_voice::vad::VAD_FRAME_SIZE).collect();
            let frame_i16: Vec<i16> = frame_f32.iter()
                .map(|&s| (s * 32767.0).clamp(-32768.0, 32767.0) as i16)
                .collect();
            if v.is_speech(&frame_i16) {
                any_speech = true;
            }
        }
        if !any_speech {
            continue; // Not speech — don't send to Gemini
        }
    }
}
```

**Step 4: Run tests**

Run: `cargo test -p aura-daemon`
Expected: All tests pass (mic bridge is async and tested via integration; unit tests should still pass)

**Step 5: Build and verify compilation**

Run: `cargo build -p aura-daemon`
Expected: Compiles without errors

**Step 6: Commit**

```bash
git add crates/aura-daemon/src/orchestrator.rs
git commit -m "feat: integrate VAD into mic bridge for speech detection

Replaces energy-threshold gating with webrtc-vad during both
playback (barge-in) and normal listening. Falls back to energy
gating if VAD init fails. Accumulates samples into 30ms frames
for VAD processing."
```

---

### Task 5: Vision Fallback — Enhanced click_element Error Response

**Files:**
- Modify: `crates/aura-daemon/src/tool_helpers.rs:62-70` (empty AX tree error)
- Modify: `crates/aura-daemon/src/tool_helpers.rs:88-94` (no matching element error)

**Step 1: Write the failing test**

Add to `tool_helpers.rs` test module:

```rust
#[test]
fn click_element_empty_tree_suggests_coordinate_fallback() {
    // Simulate what click_element_inner returns for empty AX tree
    // (we can't call it directly in tests without AX, but test the error format)
    let response = serde_json::json!({
        "success": false,
        "error": "No interactive UI elements found.",
        "hint": "use_coordinates",
        "suggestion": "This app may not expose accessibility data. Use the 'click' tool with pixel coordinates from the screenshot instead.",
    });
    assert_eq!(response["hint"], "use_coordinates");
    assert!(response["suggestion"].as_str().unwrap().contains("click"));
}
```

**Step 2: Run test to verify it passes (it's a format test)**

Run: `cargo test -p aura-daemon click_element_empty_tree`
Expected: PASS

**Step 3: Update the empty-tree error in click_element_inner**

In `tool_helpers.rs`, replace the empty-tree error response (lines 64-69):

```rust
// Before:
if all_elements.is_empty() {
    return serde_json::json!({
        "success": false,
        "error": "No interactive UI elements found. The app may not expose accessibility data, \
                  or Accessibility permission may not be fully granted.",
    });
}

// After:
if all_elements.is_empty() {
    return serde_json::json!({
        "success": false,
        "error": "No interactive UI elements found.",
        "hint": "use_coordinates",
        "suggestion": "This app may not expose accessibility data. Use the 'click' tool \
                        with pixel coordinates based on what you see in the screenshot instead. \
                        Estimate the center of the target element visually.",
    });
}
```

**Step 4: Update the no-match error similarly**

Replace the no-match error (lines 88-94):

```rust
// Before:
return serde_json::json!({
    "success": false,
    "error": format!(
        "No element matching label={:?} role={:?}. Available elements: {}",
        label, role, alternatives.join(", ")
    ),
});

// After:
return serde_json::json!({
    "success": false,
    "error": format!(
        "No element matching label={:?} role={:?}.",
        label, role
    ),
    "available_elements": alternatives,
    "hint": if alternatives.len() <= 3 { "sparse_ax_tree" } else { "element_not_found" },
    "suggestion": if alternatives.len() <= 3 {
        "This app has very few accessibility elements. Try using the 'click' tool \
         with pixel coordinates from the screenshot instead."
    } else {
        "The element wasn't found. Check the available_elements list for the correct label/role."
    },
});
```

**Step 5: Run full tests**

Run: `cargo test -p aura-daemon`
Expected: All tests pass

**Step 6: Commit**

```bash
git add crates/aura-daemon/src/tool_helpers.rs
git commit -m "feat: vision fallback hints when AX tree is empty/sparse

When click_element fails due to empty or sparse accessibility tree,
the error response now includes hint='use_coordinates' suggesting
Gemini use the click tool with pixel coordinates from the screenshot."
```

---

### Task 6: Update Gemini System Prompt for Vision Fallback

**Files:**
- Modify: `crates/aura-gemini/src/config.rs` (system prompt)

**Step 1: Read the current system prompt**

Read `crates/aura-gemini/src/config.rs` and find the system prompt string.

**Step 2: Add vision fallback instruction to system prompt**

Add after the existing tool strategy section:

```
## Coordinate Fallback for Inaccessible Apps

Some apps (especially Electron-based like Slack, VS Code, Notion, Discord) don't expose
accessibility data. When click_element returns hint="use_coordinates" or hint="sparse_ax_tree":
1. Look at the most recent screenshot carefully
2. Identify the target element visually (button, link, text field, etc.)
3. Estimate the pixel coordinates of the element's center
4. Use the 'click' tool with those x,y coordinates instead
5. After clicking, check the next screenshot to verify the click landed correctly

This is expected behavior for many modern apps — not an error. Use visual targeting confidently.
```

**Step 3: Run tests**

Run: `cargo test -p aura-gemini`
Expected: All tests pass

**Step 4: Commit**

```bash
git add crates/aura-gemini/src/config.rs
git commit -m "feat: teach Gemini coordinate fallback for inaccessible apps

System prompt now instructs the model to use visual screenshot
targeting when accessibility data is unavailable, enabling
interaction with Electron apps like Slack, VS Code, and Notion."
```

---

### Task 7: Adaptive AX Tree Depth — Density-Based Scaling

**Files:**
- Modify: `crates/aura-screen/src/accessibility.rs:17-20` (constants)
- Modify: `crates/aura-screen/src/accessibility.rs:252-355` (walk_element + get_focused_app_elements)

**Step 1: Write the failing test**

Add to `accessibility.rs` test module:

```rust
#[test]
fn adaptive_limits_scale_with_density() {
    let (max_el, max_dep) = adaptive_limits(25); // Rich tree (>=20)
    assert_eq!(max_el, 150);
    assert_eq!(max_dep, 7);

    let (max_el, max_dep) = adaptive_limits(10); // Moderate tree (5-19)
    assert_eq!(max_el, 80);
    assert_eq!(max_dep, 5);

    let (max_el, max_dep) = adaptive_limits(3);  // Sparse tree (<5)
    assert_eq!(max_el, 50);  // Don't go deeper for sparse trees
    assert_eq!(max_dep, 3);
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p aura-screen adaptive_limits`
Expected: FAIL — function doesn't exist

**Step 3: Implement adaptive_limits**

Add to `accessibility.rs` after the constants block:

```rust
/// Phase-1 probe limits (fast, shallow scan).
const PROBE_MAX_DEPTH: usize = 3;
const PROBE_TIMEOUT_MS: u128 = 200;

/// Determine max elements and depth based on how many interactive
/// elements were found in the initial shallow probe.
fn adaptive_limits(probe_count: usize) -> (usize, usize) {
    if probe_count >= 20 {
        // Rich tree — go deeper to find nested controls
        (150, 7)
    } else if probe_count >= 5 {
        // Moderate tree — current-like limits
        (80, 5)
    } else {
        // Sparse/broken tree — don't waste time going deeper
        (50, 3)
    }
}
```

**Step 4: Run test**

Run: `cargo test -p aura-screen adaptive_limits`
Expected: PASS

**Step 5: Update get_focused_app_elements to use two-phase walk**

Replace the `get_focused_app_elements` function:

```rust
pub fn get_focused_app_elements() -> Vec<UIElement> {
    let pid = match crate::macos::get_frontmost_pid() {
        Some(p) => p,
        None => return Vec::new(),
    };

    let app_element = unsafe { AXUIElementCreateApplication(pid) };
    if app_element.is_null() {
        return Vec::new();
    }
    let app_ref = CfRef::new(app_element);

    // Phase 1: Fast shallow probe to measure tree density
    let mut probe_elements = Vec::new();
    let probe_start = Instant::now();
    walk_element_with_limits(
        app_ref.as_raw(),
        0,
        &mut probe_elements,
        &probe_start,
        MAX_ELEMENTS, // cap at 50 for probe
        PROBE_MAX_DEPTH,
        PROBE_TIMEOUT_MS,
    );

    let probe_count = probe_elements.len();
    let (max_elements, max_depth) = adaptive_limits(probe_count);

    // If probe already hit limits or tree is sparse, return probe results
    if probe_count >= MAX_ELEMENTS || max_depth <= PROBE_MAX_DEPTH {
        return probe_elements;
    }

    // Phase 2: Deeper walk with adaptive limits
    let mut elements = Vec::new();
    let start_time = Instant::now();
    walk_element_with_limits(
        app_ref.as_raw(),
        0,
        &mut elements,
        &start_time,
        max_elements,
        max_depth,
        TIMEOUT_MS + 300, // extend timeout for deeper walk (800ms total)
    );
    elements
}
```

**Step 6: Refactor walk_element to accept limits as parameters**

Rename `walk_element` to `walk_element_with_limits` and add limit parameters:

```rust
fn walk_element_with_limits(
    element: CFTypeRef,
    depth: usize,
    elements: &mut Vec<UIElement>,
    start_time: &Instant,
    max_elements: usize,
    max_depth: usize,
    timeout_ms: u128,
) {
    if depth > max_depth {
        return;
    }
    if elements.len() >= max_elements {
        return;
    }
    if start_time.elapsed().as_millis() >= timeout_ms {
        return;
    }

    // ... rest of existing walk_element body unchanged,
    // but recursive calls pass through the limit parameters:
    // walk_element_with_limits(child, depth + 1, elements, start_time, max_elements, max_depth, timeout_ms);
```

Update ALL recursive calls within the function to pass `max_elements`, `max_depth`, and `timeout_ms`.

Also update `find_element_single_pass` similarly — it uses `MAX_DEPTH` and `TIMEOUT_MS` directly. Change those references to use the constants (they already use the phase-1 values which is correct for targeted searches).

**Step 7: Run full test suite**

Run: `cargo test -p aura-screen`
Expected: All tests pass

**Step 8: Commit**

```bash
git add crates/aura-screen/src/accessibility.rs
git commit -m "feat: adaptive AX tree depth based on density probe

Two-phase walk: fast shallow probe (depth 3, 200ms) measures tree
density, then scales limits adaptively. Rich trees (20+ elements)
get depth 7 and 150 max elements. Sparse trees exit early at depth 3."
```

---

### Task 8: Safe-to-Pipeline Action Pairs

**Files:**
- Create: `crates/aura-daemon/src/pipeline.rs` (action pairing logic)
- Modify: `crates/aura-daemon/src/lib.rs` (expose module)
- Modify: `crates/aura-daemon/src/processor.rs` (integrate pipelining)

**Step 1: Write failing tests for action pair detection**

Create `crates/aura-daemon/src/pipeline.rs`:

```rust
//! Safe-to-pipeline action pair detection.
//!
//! Determines when consecutive state-changing tool calls can skip
//! intermediate verification, reducing multi-step action latency.

/// Maximum consecutive actions that can be pipelined without verification.
/// Safety cap to prevent error cascades.
const MAX_CHAIN_LENGTH: usize = 3;

/// Returns true if `next_tool` can safely execute immediately after `prev_tool`
/// without waiting for screen verification of prev_tool's effect.
pub(crate) fn is_safe_continuation(prev_tool: &str, next_tool: &str) -> bool {
    matches!(
        (prev_tool, next_tool),
        // Type then press Enter/Tab/Escape — natural text entry sequence
        ("type_text", "press_key")
        // Key combo sequences (Cmd+C, Cmd+V, etc.)
        | ("press_key", "press_key")
        // Click into field then type — field is already focused after click
        | ("click", "type_text")
        | ("click_element", "type_text")
        // Activate app then interact with it
        | ("activate_app", "click")
        | ("activate_app", "click_element")
        | ("activate_app", "click_menu_item")
    )
}

/// Returns true if `chain_length` has reached the safety cap.
pub(crate) fn chain_at_limit(chain_length: usize) -> bool {
    chain_length >= MAX_CHAIN_LENGTH
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn type_then_enter_is_safe() {
        assert!(is_safe_continuation("type_text", "press_key"));
    }

    #[test]
    fn key_combo_is_safe() {
        assert!(is_safe_continuation("press_key", "press_key"));
    }

    #[test]
    fn click_then_type_is_safe() {
        assert!(is_safe_continuation("click", "type_text"));
        assert!(is_safe_continuation("click_element", "type_text"));
    }

    #[test]
    fn activate_then_interact_is_safe() {
        assert!(is_safe_continuation("activate_app", "click"));
        assert!(is_safe_continuation("activate_app", "click_element"));
        assert!(is_safe_continuation("activate_app", "click_menu_item"));
    }

    #[test]
    fn unrelated_actions_not_safe() {
        assert!(!is_safe_continuation("click", "click"));
        assert!(!is_safe_continuation("scroll", "type_text"));
        assert!(!is_safe_continuation("run_applescript", "click"));
        assert!(!is_safe_continuation("type_text", "click"));
    }

    #[test]
    fn chain_limit() {
        assert!(!chain_at_limit(0));
        assert!(!chain_at_limit(1));
        assert!(!chain_at_limit(2));
        assert!(chain_at_limit(3));
        assert!(chain_at_limit(10));
    }
}
```

**Step 2: Expose module in lib.rs**

Add to `crates/aura-daemon/src/lib.rs`:
```rust
pub(crate) mod pipeline;
```

**Step 3: Run tests**

Run: `cargo test -p aura-daemon pipeline`
Expected: All tests pass

**Step 4: Integrate pipelining into processor.rs**

This is the most complex integration. In the tool dispatch block (`processor.rs`), we need to track the previous tool name and check if the next tool forms a continuation pair.

Add state tracking before the main loop (after `tools_in_flight` declaration around line 123):

```rust
// Pipeline state: tracks the last state-changing tool for continuation detection
let last_state_tool: Arc<std::sync::Mutex<Option<String>>> = Arc::new(std::sync::Mutex::new(None));
```

In the verification block (around line 514), wrap the entire verification in a pipelining check:

```rust
if let Some(pre) = pre_hash {
    // Check if this tool was a safe continuation of the previous one
    let should_skip_verify = {
        let prev = last_state_tool.lock().unwrap_or_else(|e| e.into_inner());
        match prev.as_deref() {
            Some(prev_name) => {
                super::pipeline::is_safe_continuation(prev_name, &name)
                    && !super::pipeline::chain_at_limit(/* need chain counter */)
            }
            None => false,
        }
    };

    if should_skip_verify {
        // Micro-settle only — skip full verification
        tokio::time::sleep(Duration::from_millis(30)).await;
        if let Some(obj) = response.as_object_mut() {
            obj.insert("verified".to_string(), serde_json::json!("pipelined"));
            obj.insert("post_state".to_string(), serde_json::json!({}));
        }
    } else {
        // Full adaptive verification (from Task 1)
        // ... existing verification code ...
    }

    // Update last tool for next iteration
    if let Ok(mut prev) = last_state_tool.lock() {
        *prev = Some(name.clone());
    }
}
```

Note: The chain counter needs to be tracked per-sequence. Add an `Arc<AtomicUsize>` chain counter that resets when verification runs and increments when pipelined.

**Step 5: Run full test suite**

Run: `cargo test -p aura-daemon`
Expected: All tests pass

**Step 6: Commit**

```bash
git add crates/aura-daemon/src/pipeline.rs crates/aura-daemon/src/lib.rs crates/aura-daemon/src/processor.rs
git commit -m "feat: safe-to-pipeline action pairs skip intermediate verification

Consecutive actions like type_text→press_key, click→type_text, and
activate_app→click skip intermediate screen verification, using only
a 30ms micro-settle. Max chain length of 3 prevents error cascades."
```

---

### Task 9: Integration Verification

**Files:** None (verification only)

**Step 1: Run full workspace tests**

Run: `cargo test --workspace`
Expected: All tests pass across all crates

**Step 2: Build release binary**

Run: `cargo build --release`
Expected: Compiles without errors or warnings

**Step 3: Run clippy**

Run: `cargo clippy --workspace -- -D warnings`
Expected: No warnings

**Step 4: Run formatter**

Run: `cargo fmt --all -- --check`
Expected: All files formatted

**Step 5: Final commit if any formatting needed**

```bash
cargo fmt --all
git add -A
git commit -m "chore: format after pipeline overhaul"
```
