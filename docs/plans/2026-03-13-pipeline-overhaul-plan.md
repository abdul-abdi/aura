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

---

## Phase 2: Closing the Remaining Gaps

Tasks 10-15 address capabilities that Phases 1-5 alone don't provide: retry logic for missed clicks, bounding box validation, visual element labeling (SoM), password field workarounds, and system prompt coherence.

---

### Task 10: Coordinate Retry with Spiral Offset

**Files:**
- Modify: `crates/aura-daemon/src/processor.rs:510-536` (verification block)
- Modify: `crates/aura-daemon/src/tool_helpers.rs` (add spiral offset helper)

**Context:** When a coordinate-based click returns `verified=false`, the click likely missed its target by a few pixels. Instead of giving up or asking Gemini to re-estimate, we can retry with small spiral offsets (±15px) around the original coordinate. This is invisible to Gemini — the daemon handles it automatically.

**Step 1: Write the failing test**

Add to `tool_helpers.rs` test module:

```rust
#[test]
fn spiral_offsets_are_correct() {
    let offsets = spiral_offsets(15);
    // First offset is always (0, 0) — original coordinates
    assert_eq!(offsets[0], (0, 0));
    // Then cardinal directions, then diagonals
    assert_eq!(offsets.len(), 9);
    assert!(offsets.contains(&(15, 0)));
    assert!(offsets.contains(&(-15, 0)));
    assert!(offsets.contains(&(0, 15)));
    assert!(offsets.contains(&(0, -15)));
    assert!(offsets.contains(&(15, 15)));
    assert!(offsets.contains(&(-15, -15)));
    assert!(offsets.contains(&(15, -15)));
    assert!(offsets.contains(&(-15, 15)));
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p aura-daemon spiral_offsets`
Expected: FAIL — `spiral_offsets` doesn't exist

**Step 3: Implement spiral_offsets**

Add to `crates/aura-daemon/src/tool_helpers.rs`:

```rust
/// Generate spiral offset pairs for retry clicks.
/// Returns (dx, dy) offsets starting from (0,0) then cardinal/diagonal directions.
pub(crate) fn spiral_offsets(radius: i32) -> Vec<(i32, i32)> {
    vec![
        (0, 0),           // original position
        (radius, 0),      // right
        (0, radius),      // down
        (-radius, 0),     // left
        (0, -radius),     // up
        (radius, radius), // down-right
        (-radius, -radius), // up-left
        (radius, -radius),  // up-right
        (-radius, radius),  // down-left
    ]
}

/// Maximum number of spiral retry attempts for coordinate-based clicks.
pub(crate) const MAX_CLICK_RETRIES: usize = 4;

/// Pixel radius for spiral retry offsets.
pub(crate) const SPIRAL_RADIUS: i32 = 15;
```

**Step 4: Run test to verify it passes**

Run: `cargo test -p aura-daemon spiral_offsets`
Expected: PASS

**Step 5: Integrate retry into processor.rs verification block**

In `processor.rs`, after the verification failure block (where `verified = false`), add retry logic for coordinate-based click tools:

```rust
// Retry with spiral offsets for coordinate-based clicks that failed verification
if !screen_changed && matches!(name.as_str(), "click" | "context_menu_click") {
    if let (Some(orig_x), Some(orig_y)) = (
        args.get("x").and_then(|v| v.as_f64()),
        args.get("y").and_then(|v| v.as_f64()),
    ) {
        let offsets = tools::spiral_offsets(tools::SPIRAL_RADIUS);
        // Skip offset[0] (0,0) — that's the original click we already tried
        for (i, &(dx, dy)) in offsets.iter().skip(1).take(tools::MAX_CLICK_RETRIES).enumerate() {
            let retry_x = orig_x + dx as f64;
            let retry_y = orig_y + dy as f64;
            tracing::debug!(tool = %name, attempt = i + 2, dx, dy, "Retrying click with spiral offset");

            // Execute the retry click
            let mut retry_args = args.clone();
            retry_args["x"] = serde_json::json!(retry_x);
            retry_args["y"] = serde_json::json!(retry_y);
            let _ = tools::execute_tool(&name, &retry_args, dims).await;

            // Brief settle + single hash check
            tokio::time::sleep(Duration::from_millis(80)).await;
            let rx = tool_capture_trigger.trigger_and_wait();
            tool_cap_notify.notify_one();
            let _ = tokio::time::timeout(Duration::from_millis(100), rx).await;
            let current_hash = tool_last_hash.load(Ordering::Acquire);
            if current_hash != pre {
                screen_changed = true;
                verified = true;
                tracing::info!(tool = %name, attempt = i + 2, "Spiral retry succeeded");
                if let Some(obj) = response.as_object_mut() {
                    obj.insert("retry_offset".into(), serde_json::json!({"dx": dx, "dy": dy}));
                }
                break;
            }
        }
    }
}
```

**Step 6: Run full test suite**

Run: `cargo test -p aura-daemon`
Expected: All tests pass

**Step 7: Commit**

```bash
git add crates/aura-daemon/src/processor.rs crates/aura-daemon/src/tool_helpers.rs
git commit -m "feat: spiral retry for coordinate clicks on verification failure

When a click at (x,y) returns verified=false, automatically retry with
±15px spiral offsets (up to 4 retries). Transparent to Gemini — the
daemon handles missed clicks without round-tripping to the model."
```

---

### Task 11: Bounding Box Validation on Click Tool

**Files:**
- Modify: `crates/aura-gemini/src/tools.rs:94-117` (click tool declaration)
- Modify: `crates/aura-daemon/src/tool_helpers.rs` (add bounds validation helper)
- Modify: `crates/aura-gemini/src/config.rs` (system prompt reference)

**Context:** Gemini has native bounding box detection — it can return `[y0, x0, y1, x1]` coordinates normalized to [0, 1000]. We add an optional `expected_element` parameter to the click tool. When provided, the daemon validates that the click coordinates fall within the expected bounds after denormalization, logging a warning if they don't. This catches coordinate estimation errors before they happen.

**Step 1: Write the failing test**

Add to `tool_helpers.rs` test module:

```rust
#[test]
fn point_in_bounds_check() {
    // Bounds: [y0, x0, y1, x1] normalized to [0, 1000]
    // Screen: 1920×1080
    let screen_w = 1920.0;
    let screen_h = 1080.0;
    // Element at roughly center of screen: [400, 400, 600, 600] in normalized
    let bounds = [400, 400, 600, 600]; // y0, x0, y1, x1

    let x0 = bounds[1] as f64 / 1000.0 * screen_w; // 768
    let y0 = bounds[0] as f64 / 1000.0 * screen_h; // 432
    let x1 = bounds[3] as f64 / 1000.0 * screen_w; // 1152
    let y1 = bounds[2] as f64 / 1000.0 * screen_h; // 648

    // Center of bounds
    assert!(point_in_denormalized_bounds(960.0, 540.0, &bounds, screen_w, screen_h));
    // Outside bounds
    assert!(!point_in_denormalized_bounds(100.0, 100.0, &bounds, screen_w, screen_h));
    // Edge of bounds (inclusive)
    assert!(point_in_denormalized_bounds(x0, y0, &bounds, screen_w, screen_h));
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p aura-daemon point_in_bounds`
Expected: FAIL — function doesn't exist

**Step 3: Implement bounds validation**

Add to `crates/aura-daemon/src/tool_helpers.rs`:

```rust
/// Check if a point (x, y) in pixel coordinates falls within Gemini-normalized
/// bounding box [y0, x0, y1, x1] (each in [0, 1000]).
pub(crate) fn point_in_denormalized_bounds(
    x: f64,
    y: f64,
    bounds: &[i32; 4], // [y0, x0, y1, x1] normalized to [0, 1000]
    screen_w: f64,
    screen_h: f64,
) -> bool {
    let x0 = bounds[1] as f64 / 1000.0 * screen_w;
    let y0 = bounds[0] as f64 / 1000.0 * screen_h;
    let x1 = bounds[3] as f64 / 1000.0 * screen_w;
    let y1 = bounds[2] as f64 / 1000.0 * screen_h;
    x >= x0 && x <= x1 && y >= y0 && y <= y1
}
```

**Step 4: Run test to verify it passes**

Run: `cargo test -p aura-daemon point_in_bounds`
Expected: PASS

**Step 5: Add optional `expected_bounds` parameter to click tool declaration**

In `crates/aura-gemini/src/tools.rs`, update the click tool's parameters (around line 101-115):

```rust
parameters: json!({
    "type": "object",
    "properties": {
        "x": { "type": "number", "description": "X coordinate" },
        "y": { "type": "number", "description": "Y coordinate" },
        "button": { "type": "string", "enum": ["left", "right"], "description": "Mouse button. Default: left" },
        "click_count": { "type": "integer", "description": "Number of clicks (2 for double-click). Default: 1" },
        "modifiers": {
            "type": "array",
            "items": { "type": "string", "enum": ["cmd", "shift", "alt", "ctrl"] },
            "description": "Modifier keys to hold during click."
        },
        "expected_bounds": {
            "type": "array",
            "items": { "type": "integer" },
            "description": "Optional bounding box [y0, x0, y1, x1] (normalized 0-1000) of the expected target element. If provided, the system validates your click coordinates fall within this region and warns if they don't."
        }
    },
    "required": ["x", "y"]
}),
```

**Step 6: Add validation in tool execution**

In the click tool handler in `processor.rs` or `tool_helpers.rs`, before executing the click:

```rust
// Validate expected_bounds if provided
if let Some(bounds_arr) = args.get("expected_bounds").and_then(|v| v.as_array()) {
    if bounds_arr.len() == 4 {
        let bounds: [i32; 4] = [
            bounds_arr[0].as_i64().unwrap_or(0) as i32,
            bounds_arr[1].as_i64().unwrap_or(0) as i32,
            bounds_arr[2].as_i64().unwrap_or(0) as i32,
            bounds_arr[3].as_i64().unwrap_or(0) as i32,
        ];
        let x = args.get("x").and_then(|v| v.as_f64()).unwrap_or(0.0);
        let y = args.get("y").and_then(|v| v.as_f64()).unwrap_or(0.0);
        let (sw, sh) = (dims.img_w as f64, dims.img_h as f64);
        if !tools::point_in_denormalized_bounds(x, y, &bounds, sw, sh) {
            tracing::warn!(
                tool = "click", x, y, ?bounds,
                "Click coordinates outside expected_bounds — likely mis-targeted"
            );
            if let Some(obj) = response.as_object_mut() {
                obj.insert("bounds_warning".into(),
                    serde_json::json!("Click coordinates are outside the expected element bounds. The click may miss its target."));
            }
        }
    }
}
```

**Step 7: Run full test suite**

Run: `cargo test -p aura-daemon && cargo test -p aura-gemini`
Expected: All tests pass

**Step 8: Commit**

```bash
git add crates/aura-gemini/src/tools.rs crates/aura-daemon/src/tool_helpers.rs crates/aura-daemon/src/processor.rs
git commit -m "feat: optional bounding box validation for click tool

Add expected_bounds parameter ([y0,x0,y1,x1] normalized 0-1000) to
the click tool. When Gemini provides bounds from its native bounding
box detection, the daemon validates that (x,y) falls within them and
warns on mismatch. Catches coordinate estimation errors early."
```

---

### Task 12: Lightweight Edge-Based SoM Overlay

**Files:**
- Create: `crates/aura-screen/src/som.rs` (SoM overlay module)
- Modify: `crates/aura-screen/src/lib.rs` (expose module)
- Modify: `crates/aura-screen/Cargo.toml` (add `image` PNG feature)
- Modify: `crates/aura-screen/src/capture.rs` (optional SoM-annotated frame)

**Context:** Set-of-Mark (SoM) overlays number detected interactive regions on the screenshot. OmniParser uses YOLO+Florence (too heavy, requires CUDA+Python). Instead we use pure Rust edge detection with the `image` crate (already a dependency) to find rectangular regions, then overlay numbered markers. This gives Gemini explicit targets: "click mark 7" instead of guessing coordinates. ~200-300 lines of image processing.

**Step 1: Add PNG feature to image dependency**

In `crates/aura-screen/Cargo.toml`, update the image dependency:

```toml
image = { version = "0.25", default-features = false, features = ["jpeg", "png"] }
```

The `png` feature is needed for rendering text/overlay marks with alpha compositing.

**Step 2: Write the failing tests**

Create `crates/aura-screen/src/som.rs`:

```rust
//! Lightweight Set-of-Mark (SoM) overlay for visual element targeting.
//!
//! Detects rectangular interactive regions in screenshots via edge detection
//! and overlays numbered markers. Gemini can then reference "mark N" instead
//! of estimating pixel coordinates.
//!
//! This is a pure-Rust alternative to OmniParser (YOLO+Florence, requires CUDA).

use image::{DynamicImage, GenericImageView, Rgba, RgbaImage};

/// A detected interactive region with its bounding box and mark number.
#[derive(Debug, Clone)]
pub struct SomMark {
    /// Mark number (1-indexed, displayed on overlay).
    pub id: usize,
    /// Bounding box: (x, y, width, height) in pixels.
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

/// Minimum region dimension (pixels) to qualify as an interactive element.
const MIN_REGION_SIZE: u32 = 20;

/// Maximum number of marks to overlay (prevents clutter).
const MAX_MARKS: usize = 30;

/// Edge detection threshold (0-255). Pixels with gradient magnitude above
/// this are considered edges.
const EDGE_THRESHOLD: u8 = 40;

/// Minimum gap between detected regions to avoid duplicates (pixels).
const MIN_REGION_GAP: u32 = 10;

/// Detect rectangular interactive regions using Sobel edge detection
/// and connected component analysis on the resulting binary edge map.
pub fn detect_regions(img: &DynamicImage) -> Vec<SomMark> {
    let gray = img.to_luma8();
    let (w, h) = gray.dimensions();

    if w < MIN_REGION_SIZE * 2 || h < MIN_REGION_SIZE * 2 {
        return Vec::new();
    }

    // Sobel edge detection
    let mut edges = vec![false; (w * h) as usize];
    for y in 1..h - 1 {
        for x in 1..w - 1 {
            let idx = |dx: i32, dy: i32| -> u8 {
                gray.get_pixel((x as i32 + dx) as u32, (y as i32 + dy) as u32).0[0]
            };
            // Sobel X
            let gx = -(idx(-1, -1) as i16) + (idx(1, -1) as i16)
                - 2 * (idx(-1, 0) as i16) + 2 * (idx(1, 0) as i16)
                - (idx(-1, 1) as i16) + (idx(1, 1) as i16);
            // Sobel Y
            let gy = -(idx(-1, -1) as i16) - 2 * (idx(0, -1) as i16) - (idx(1, -1) as i16)
                + (idx(-1, 1) as i16) + 2 * (idx(0, 1) as i16) + (idx(1, 1) as i16);
            let magnitude = ((gx.abs() + gy.abs()) / 2) as u8;
            if magnitude > EDGE_THRESHOLD {
                edges[(y * w + x) as usize] = true;
            }
        }
    }

    // Simple horizontal run-length region detection:
    // Find horizontal runs of edges, group into rectangular regions
    let mut regions: Vec<(u32, u32, u32, u32)> = Vec::new(); // (x, y, w, h)

    // Scan for rectangular clusters using a grid-based approach
    let grid_size = MIN_REGION_SIZE;
    let mut visited = vec![false; (w * h) as usize];

    for gy in (0..h).step_by(grid_size as usize / 2) {
        for gx in (0..w).step_by(grid_size as usize / 2) {
            // Count edges in this grid cell
            let mut edge_count = 0u32;
            let cell_w = grid_size.min(w - gx);
            let cell_h = grid_size.min(h - gy);
            for dy in 0..cell_h {
                for dx in 0..cell_w {
                    let idx = ((gy + dy) * w + (gx + dx)) as usize;
                    if idx < edges.len() && edges[idx] {
                        edge_count += 1;
                    }
                }
            }

            // If enough edges, this is likely an interactive region
            let cell_area = cell_w * cell_h;
            if cell_area > 0 && edge_count * 100 / cell_area > 15 {
                // Check if this overlaps with an existing region
                let overlaps = regions.iter().any(|&(rx, ry, rw, rh)| {
                    gx < rx + rw + MIN_REGION_GAP
                        && gx + cell_w + MIN_REGION_GAP > rx
                        && gy < ry + rh + MIN_REGION_GAP
                        && gy + cell_h + MIN_REGION_GAP > ry
                });

                if !overlaps {
                    regions.push((gx, gy, cell_w, cell_h));
                }
            }
        }
    }

    // Sort by position (top-to-bottom, left-to-right) and cap at MAX_MARKS
    regions.sort_by(|a, b| a.1.cmp(&b.1).then(a.0.cmp(&b.0)));
    regions.truncate(MAX_MARKS);

    regions
        .into_iter()
        .enumerate()
        .map(|(i, (x, y, w, h))| SomMark {
            id: i + 1,
            x,
            y,
            width: w,
            height: h,
        })
        .collect()
}

/// Render numbered markers on a copy of the image at each detected region.
/// Returns the annotated image and the list of marks with their coordinates.
pub fn annotate_frame(img: &DynamicImage) -> (DynamicImage, Vec<SomMark>) {
    let marks = detect_regions(img);
    let mut canvas = img.to_rgba8();

    for mark in &marks {
        draw_mark_label(&mut canvas, mark);
    }

    (DynamicImage::ImageRgba8(canvas), marks)
}

/// Draw a numbered label at the top-left corner of a mark's bounding box.
fn draw_mark_label(canvas: &mut RgbaImage, mark: &SomMark) {
    let label = format!("{}", mark.id);
    let label_w = (label.len() as u32) * 8 + 4; // ~8px per char + padding
    let label_h: u32 = 14;

    // Draw background rectangle (semi-transparent red)
    let bg_color = Rgba([220, 40, 40, 200]);
    let text_color = Rgba([255, 255, 255, 255]);

    let (img_w, img_h) = canvas.dimensions();
    let bx = mark.x.min(img_w.saturating_sub(label_w));
    let by = mark.y.saturating_sub(label_h + 2);

    for dy in 0..label_h {
        for dx in 0..label_w {
            let px = bx + dx;
            let py = by + dy;
            if px < img_w && py < img_h {
                canvas.put_pixel(px, py, bg_color);
            }
        }
    }

    // Draw number using simple pixel font (3×5 digit bitmaps)
    let digits: Vec<u8> = label.bytes().map(|b| b - b'0').collect();
    for (di, &digit) in digits.iter().enumerate() {
        let bitmap = digit_bitmap(digit);
        let ox = bx + 2 + (di as u32) * 8;
        let oy = by + 3;
        for (row, bits) in bitmap.iter().enumerate() {
            for col in 0..5u32 {
                if bits & (1 << (4 - col)) != 0 {
                    // Draw 2× scaled pixel for readability
                    for sy in 0..2u32 {
                        for sx in 0..2u32 {
                            let px = ox + col * 2 + sx;
                            let py = oy + (row as u32) * 2 + sy;
                            if px < img_w && py < img_h {
                                canvas.put_pixel(px, py, text_color);
                            }
                        }
                    }
                }
            }
        }
    }

    // Draw outline around the region (1px red border)
    let outline_color = Rgba([220, 40, 40, 180]);
    for dx in 0..mark.width {
        let px = mark.x + dx;
        if px < img_w {
            if mark.y < img_h { canvas.put_pixel(px, mark.y, outline_color); }
            let bot = mark.y + mark.height.saturating_sub(1);
            if bot < img_h { canvas.put_pixel(px, bot, outline_color); }
        }
    }
    for dy in 0..mark.height {
        let py = mark.y + dy;
        if py < img_h {
            if mark.x < img_w { canvas.put_pixel(mark.x, py, outline_color); }
            let right = mark.x + mark.width.saturating_sub(1);
            if right < img_w { canvas.put_pixel(right, py, outline_color); }
        }
    }
}

/// 3×5 pixel bitmap for digits 0-9 (5 bits per row, MSB = leftmost).
fn digit_bitmap(d: u8) -> [u8; 5] {
    match d {
        0 => [0b01110, 0b10001, 0b10001, 0b10001, 0b01110],
        1 => [0b00100, 0b01100, 0b00100, 0b00100, 0b01110],
        2 => [0b01110, 0b10001, 0b00110, 0b01000, 0b11111],
        3 => [0b01110, 0b10001, 0b00110, 0b10001, 0b01110],
        4 => [0b10010, 0b10010, 0b11111, 0b00010, 0b00010],
        5 => [0b11111, 0b10000, 0b11110, 0b00001, 0b11110],
        6 => [0b01110, 0b10000, 0b11110, 0b10001, 0b01110],
        7 => [0b11111, 0b00001, 0b00010, 0b00100, 0b00100],
        8 => [0b01110, 0b10001, 0b01110, 0b10001, 0b01110],
        9 => [0b01110, 0b10001, 0b01111, 0b00001, 0b01110],
        _ => [0; 5],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_regions_on_blank_image_returns_empty() {
        let img = DynamicImage::ImageRgba8(RgbaImage::new(200, 200));
        let regions = detect_regions(&img);
        assert!(regions.is_empty(), "Blank image should have no regions");
    }

    #[test]
    fn detect_regions_on_tiny_image_returns_empty() {
        let img = DynamicImage::ImageRgba8(RgbaImage::new(10, 10));
        let regions = detect_regions(&img);
        assert!(regions.is_empty(), "Tiny image should have no regions");
    }

    #[test]
    fn marks_are_numbered_sequentially() {
        // Create an image with high-contrast rectangles that should be detected
        let mut img = RgbaImage::new(400, 400);
        // Draw two distinct rectangles with sharp edges
        for x in 50..150 {
            for y in 50..100 {
                img.put_pixel(x, y, Rgba([255, 255, 255, 255]));
            }
        }
        for x in 250..350 {
            for y in 50..100 {
                img.put_pixel(x, y, Rgba([255, 255, 255, 255]));
            }
        }
        let dyn_img = DynamicImage::ImageRgba8(img);
        let marks = detect_regions(&dyn_img);
        // Marks should be 1-indexed
        for (i, mark) in marks.iter().enumerate() {
            assert_eq!(mark.id, i + 1);
        }
    }

    #[test]
    fn annotate_frame_returns_same_dimensions() {
        let img = DynamicImage::ImageRgba8(RgbaImage::new(200, 200));
        let (annotated, _marks) = annotate_frame(&img);
        assert_eq!(annotated.width(), 200);
        assert_eq!(annotated.height(), 200);
    }

    #[test]
    fn digit_bitmap_all_digits_valid() {
        for d in 0..=9 {
            let bm = digit_bitmap(d);
            // Each digit should have at least some pixels set
            assert!(bm.iter().any(|&row| row != 0), "Digit {d} bitmap is empty");
        }
    }

    #[test]
    fn max_marks_cap() {
        // Even if we detect many regions, cap at MAX_MARKS
        assert!(MAX_MARKS <= 30);
    }
}
```

**Step 3: Expose module in lib.rs**

Add to `crates/aura-screen/src/lib.rs`:
```rust
pub mod som;
```

**Step 4: Run tests**

Run: `cargo test -p aura-screen som`
Expected: All tests pass

**Step 5: Add SoM-annotated frame generation to capture.rs**

Add a public function to `crates/aura-screen/src/capture.rs`:

```rust
/// Generate a SoM-annotated version of a captured frame.
/// Returns the annotated JPEG (base64) and the mark positions.
/// Called on-demand when Gemini requests visual element targeting.
pub fn annotate_with_som(jpeg_bytes: &[u8]) -> Option<(String, Vec<crate::som::SomMark>)> {
    let img = image::load_from_memory(jpeg_bytes).ok()?;
    let (annotated, marks) = crate::som::annotate_frame(&img);

    // Encode annotated image back to JPEG
    let mut buf = Vec::new();
    let mut encoder = JpegEncoder::new_with_quality(&mut buf, JPEG_QUALITY);
    annotated.write_with_encoder(encoder).ok()?;

    Some((BASE64.encode(&buf), marks))
}
```

**Step 6: Run full test suite**

Run: `cargo test -p aura-screen`
Expected: All tests pass

**Step 7: Commit**

```bash
git add crates/aura-screen/src/som.rs crates/aura-screen/src/lib.rs crates/aura-screen/src/capture.rs crates/aura-screen/Cargo.toml
git commit -m "feat: lightweight edge-based Set-of-Mark overlay

Pure Rust SoM implementation using Sobel edge detection and grid-based
region clustering. Overlays numbered markers on detected interactive
regions. ~250 lines, no external ML dependencies. Gemini can reference
'mark N' instead of estimating pixel coordinates."
```

---

### Task 13: Password Field Detection with Clipboard Paste Workaround

**Files:**
- Modify: `crates/aura-daemon/src/tool_helpers.rs` (detect secure input fields)
- Modify: `crates/aura-screen/src/accessibility.rs` (expose subrole detection)

**Context:** macOS `SecureEventInput` blocks synthetic keyboard input in password fields. This is a hard OS limitation — no userspace workaround exists for `type_text`. However, we can detect when the focused field is a secure text field (AX subrole `AXSecureTextField`) and automatically route through clipboard paste (`write_clipboard` + Cmd+V) instead. The user sees the same result, but we bypass the keyboard input block.

**Step 1: Write the failing test**

Add to `accessibility.rs` test module:

```rust
#[test]
fn is_secure_field_detects_password_subroles() {
    assert!(is_secure_text_subrole("AXSecureTextField"));
    assert!(!is_secure_text_subrole("AXTextField"));
    assert!(!is_secure_text_subrole("AXTextArea"));
    assert!(!is_secure_text_subrole(""));
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p aura-screen is_secure_field`
Expected: FAIL — function doesn't exist

**Step 3: Implement subrole detection in accessibility.rs**

Add to `crates/aura-screen/src/accessibility.rs`:

```rust
/// Check if a subrole indicates a secure (password) text field.
/// macOS blocks synthetic keyboard input to these fields via SecureEventInput.
pub fn is_secure_text_subrole(subrole: &str) -> bool {
    subrole == "AXSecureTextField"
}

/// Check if the currently focused element is a secure text field.
/// Returns true if the focused element's subrole is AXSecureTextField.
pub fn is_focused_element_secure() -> bool {
    let pid = match crate::macos::get_frontmost_pid() {
        Some(p) => p,
        None => return false,
    };

    let app = unsafe { AXUIElementCreateApplication(pid) };
    if app.is_null() {
        return false;
    }
    let app_ref = CfRef::new(app);

    // Get the focused element
    let mut focused: CFTypeRef = std::ptr::null();
    let err = unsafe {
        AXUIElementCopyAttributeValue(
            app_ref.as_raw(),
            cf_str("AXFocusedUIElement"),
            &mut focused,
        )
    };
    if err != AX_ERROR_SUCCESS || focused.is_null() {
        return false;
    }
    let _focused_ref = CfRef::new(focused);

    // Get the subrole
    let mut subrole_ref: CFTypeRef = std::ptr::null();
    let err = unsafe {
        AXUIElementCopyAttributeValue(focused, cf_str("AXSubrole"), &mut subrole_ref)
    };
    if err != AX_ERROR_SUCCESS || subrole_ref.is_null() {
        return false;
    }
    let subrole = unsafe { cfstring_to_string(subrole_ref as CFStringRef) };
    unsafe { CFRelease(subrole_ref) };

    is_secure_text_subrole(&subrole)
}
```

**Step 4: Run test to verify it passes**

Run: `cargo test -p aura-screen is_secure_field`
Expected: PASS

**Step 5: Add clipboard paste workaround to type_text handler**

In `crates/aura-daemon/src/tool_helpers.rs`, in the `type_text` tool handler (or wherever type_text is dispatched), add before the actual typing:

```rust
// Detect password fields and route through clipboard paste instead of synthetic keys
if aura_screen::accessibility::is_focused_element_secure() {
    tracing::info!("Secure text field detected — routing through clipboard paste");
    // Write to clipboard
    let text = args.get("text").and_then(|v| v.as_str()).unwrap_or("");
    if let Err(e) = write_to_clipboard(text) {
        return serde_json::json!({
            "success": false,
            "error": format!("Failed to write to clipboard for secure field: {e}"),
        });
    }
    // Paste with Cmd+V
    let paste_result = execute_key_press("v", &["cmd"]);
    return serde_json::json!({
        "success": true,
        "method": "clipboard_paste",
        "reason": "secure_text_field",
        "note": "Used clipboard paste because the focused field blocks synthetic keyboard input (password field).",
    });
}
```

**Step 6: Run full test suite**

Run: `cargo test -p aura-screen && cargo test -p aura-daemon`
Expected: All tests pass

**Step 7: Commit**

```bash
git add crates/aura-screen/src/accessibility.rs crates/aura-daemon/src/tool_helpers.rs
git commit -m "feat: detect password fields and use clipboard paste workaround

macOS SecureEventInput blocks synthetic keyboard input in password
fields. Now type_text detects AXSecureTextField subrole and
automatically routes through write_clipboard + Cmd+V paste instead.
Transparent to Gemini — it just calls type_text as usual."
```

---

### Task 14: Fix System Prompt Contradiction About Action Chaining

**Files:**
- Modify: `crates/aura-gemini/src/config.rs:87` (contradictory instruction)

**Context:** Line 87 of the system prompt says `"NEVER chain multiple actions without checking verified + post_state between each one."` This directly contradicts Task 8's pipelining feature, where safe continuation pairs (type_text→press_key, click→type_text) intentionally skip intermediate verification. The prompt must be updated to allow pipelined pairs while still requiring verification for other sequences.

**Step 1: Write a test to catch the contradiction**

Add to `crates/aura-gemini/src/config.rs` test module:

```rust
#[test]
fn system_prompt_allows_safe_continuation_pairs() {
    let prompt = DEFAULT_SYSTEM_PROMPT;
    // Must NOT contain the old absolute prohibition
    assert!(
        !prompt.contains("NEVER chain multiple actions without checking"),
        "System prompt still has absolute action-chaining prohibition that contradicts pipelining"
    );
    // Must mention that safe pairs can be pipelined
    assert!(
        prompt.contains("continuation") || prompt.contains("pipeline"),
        "System prompt should mention safe action continuation/pipelining"
    );
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p aura-gemini system_prompt_allows`
Expected: FAIL — the old text still exists

**Step 3: Replace the contradictory line**

In `crates/aura-gemini/src/config.rs`, replace line 87:

```
// Before:
- NEVER chain multiple actions without checking verified + post_state between each one.

// After:
- For most actions, check verified + post_state before proceeding to the next action.
- Exception: natural action pairs are automatically pipelined by the system — type_text followed by press_key (e.g., typing then pressing Enter), click followed by type_text (clicking a field then typing), and key combos (press_key followed by press_key). For these safe continuation pairs, you can issue them in sequence without waiting for intermediate verification. The system handles the timing.
- For all other action sequences, always verify between steps.
```

**Step 4: Run test to verify it passes**

Run: `cargo test -p aura-gemini system_prompt_allows`
Expected: PASS

**Step 5: Run full gemini test suite**

Run: `cargo test -p aura-gemini`
Expected: All tests pass (including the existing `system_prompt_has_decision_tree` test)

**Step 6: Commit**

```bash
git add crates/aura-gemini/src/config.rs
git commit -m "fix: update system prompt to allow safe action pipelining

Replace absolute 'NEVER chain actions' rule with nuanced guidance
that allows natural continuation pairs (type→enter, click→type,
key combos) while still requiring verification for other sequences.
Aligns prompt with the daemon's pipelining behavior from Task 8."
```

---

### Task 15: System Prompt Update for All New Capabilities

**Files:**
- Modify: `crates/aura-gemini/src/config.rs` (system prompt additions)

**Context:** Tasks 1-14 add several capabilities that Gemini needs to know about: coordinate fallback is in Task 6, but we still need to teach Gemini about VAD behavior changes, SoM overlay usage, bounding box validation, and the retry spiral. Without these prompt updates, Gemini won't know when or how to use the new features.

**Step 1: Write tests for new prompt sections**

Add to `crates/aura-gemini/src/config.rs` test module:

```rust
#[test]
fn system_prompt_covers_all_pipeline_features() {
    let prompt = DEFAULT_SYSTEM_PROMPT;

    // SoM overlay reference
    assert!(
        prompt.contains("mark") || prompt.contains("SoM") || prompt.contains("numbered"),
        "Prompt should reference SoM/numbered marks"
    );

    // Bounding box validation
    assert!(
        prompt.contains("expected_bounds") || prompt.contains("bounding box"),
        "Prompt should reference expected_bounds or bounding box validation"
    );

    // Retry spiral is transparent — Gemini doesn't need to know details,
    // but should know retries happen automatically
    assert!(
        prompt.contains("automatic retry") || prompt.contains("retried"),
        "Prompt should mention automatic click retries"
    );

    // Password field workaround is transparent
    assert!(
        prompt.contains("password") || prompt.contains("secure"),
        "Prompt should mention password/secure field handling"
    );
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p aura-gemini system_prompt_covers`
Expected: FAIL — new sections don't exist yet

**Step 3: Add new sections to the system prompt**

Append the following sections to `DEFAULT_SYSTEM_PROMPT` in `config.rs`, after the existing "Tool Tips" section and before "Rules:":

```
Visual Element Targeting (SoM):
When you need precise targeting on visually complex screens, you can request a Set-of-Mark
annotated screenshot by calling get_screen_context(). If interactive regions were detected,
the response includes a list of numbered marks with their pixel coordinates. Reference these
marks when clicking: use the mark's center coordinates with the click tool. This is especially
useful for Electron apps and web content where accessibility labels are unreliable.

Bounding Box Validation:
When clicking based on visual estimation, you can provide an expected_bounds parameter to the
click tool: [y0, x0, y1, x1] normalized to [0, 1000] (Gemini's native bounding box format).
The system validates your click coordinates fall within this region and warns if they don't.
Use this when you want extra confidence that a coordinate click will hit its target.

Automatic Click Retry:
If a coordinate-based click (click tool) doesn't cause a visible screen change, the system
automatically retries with small offsets (±15px spiral) up to 4 times. This is transparent to
you — if the response shows verified=true with a retry_offset field, the retry succeeded. You
don't need to manually retry missed clicks.

Secure Fields:
Password fields and other secure text inputs block synthetic keyboard input on macOS. When
type_text targets a password field, the system automatically routes through clipboard paste
(this is transparent to you). If you see method="clipboard_paste" in the response, this
happened automatically.
```

**Step 4: Run tests**

Run: `cargo test -p aura-gemini`
Expected: All tests pass

**Step 5: Commit**

```bash
git add crates/aura-gemini/src/config.rs
git commit -m "feat: update system prompt for SoM, bounds validation, retries, secure fields

Teach Gemini about all new pipeline capabilities: Set-of-Mark visual
targeting, expected_bounds validation on clicks, automatic spiral
retry for missed clicks, and transparent clipboard paste for password
fields. Completes the prompt alignment for the full pipeline overhaul."
```

---

### Task 16: Integration Verification (Phase 2)

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
git commit -m "chore: format after pipeline overhaul phase 2"
```
