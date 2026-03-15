# Oracle-First Click Targeting Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Replace AX-based click targeting with Vision Oracle as the primary coordinate refinement system, with circuit breaker, delta guard, multi-monitor fix, and 4s click budget.

**Architecture:** Remove `element_at_position()` from click path. Oracle becomes primary with hard timeout. Circuit breaker auto-disables on repeated failure. Spiral retries stay fast (no oracle). Multi-monitor fixed by adding display origin offset.

**Tech Stack:** Rust, Gemini 3 Flash REST API, macOS Accessibility (context-only), tokio, reqwest

---

### Task 1: Add Display Origin to CapturedFrame

**Files:**
- Modify: `crates/aura-screen/src/capture.rs:20-36` (CapturedFrame struct)
- Modify: `crates/aura-screen/src/capture.rs:86-170` (capture_screen_with_params)

**Step 1: Write the failing test**

Add to `crates/aura-screen/src/capture.rs` tests (or create inline test module):

```rust
#[test]
fn captured_frame_has_display_origin() {
    // CapturedFrame should have display_origin_x and display_origin_y fields
    let frame = CapturedFrame {
        jpeg_base64: String::new(),
        hash: 0,
        width: 1920,
        height: 1080,
        scale_factor: 2.0,
        logical_width: 1440,
        logical_height: 900,
        display_origin_x: 0.0,
        display_origin_y: 0.0,
    };
    assert_eq!(frame.display_origin_x, 0.0);
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p aura-screen captured_frame_has_display_origin`
Expected: FAIL — `display_origin_x` not a field of `CapturedFrame`

**Step 3: Add origin fields to CapturedFrame and populate them**

In `capture.rs`, add to the `CapturedFrame` struct:

```rust
/// Display origin X in global macOS coordinate space (for multi-monitor).
/// Primary display is (0, 0). Secondary displays have non-zero origins.
pub display_origin_x: f64,
/// Display origin Y in global macOS coordinate space.
pub display_origin_y: f64,
```

In `capture_screen_with_params`, after `let display_bounds = display.bounds();` (line 98), the origin is already available. Add to the `CapturedFrame` construction:

```rust
display_origin_x: display_bounds.origin.x,
display_origin_y: display_bounds.origin.y,
```

**Step 4: Fix all existing CapturedFrame construction sites**

Search for other places `CapturedFrame` is constructed (tests, mocks). Add `display_origin_x: 0.0, display_origin_y: 0.0` to each.

**Step 5: Run tests**

Run: `cargo test -p aura-screen`
Expected: PASS

**Step 6: Commit**

```bash
git add crates/aura-screen/src/capture.rs
git commit -m "feat: add display origin to CapturedFrame for multi-monitor support"
```

---

### Task 2: Add Circuit Breaker to VisionOracle

**Files:**
- Modify: `crates/aura-gemini/src/vision_oracle.rs:20-37` (struct + new method)

**Step 1: Write the failing tests**

Add to `vision_oracle.rs` test module:

```rust
#[test]
fn circuit_breaker_initially_available() {
    let oracle = VisionOracle::new("fake-key");
    assert!(oracle.is_available());
}

#[test]
fn circuit_breaker_trips_after_threshold() {
    let oracle = VisionOracle::new("fake-key");
    // Simulate 3 consecutive failures
    oracle.record_failure();
    oracle.record_failure();
    oracle.record_failure();
    assert!(!oracle.is_available());
}

#[test]
fn circuit_breaker_resets_on_success() {
    let oracle = VisionOracle::new("fake-key");
    oracle.record_failure();
    oracle.record_failure();
    oracle.record_success();
    assert!(oracle.is_available());
    // After reset, need 3 more failures to trip
    oracle.record_failure();
    assert!(oracle.is_available());
}
```

**Step 2: Run tests to verify they fail**

Run: `cargo test -p aura-gemini circuit_breaker`
Expected: FAIL — `is_available`, `record_failure`, `record_success` don't exist

**Step 3: Implement circuit breaker on VisionOracle**

```rust
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

const CIRCUIT_BREAKER_THRESHOLD: u32 = 3;
const CIRCUIT_BREAKER_COOLDOWN_SECS: u64 = 30;

pub struct VisionOracle {
    client: reqwest::Client,
    api_key: String,
    model: String,
    consecutive_failures: AtomicU32,
    tripped_until: AtomicU64, // epoch millis
}

impl VisionOracle {
    pub fn new(api_key: &str) -> Self {
        let client = reqwest::Client::builder()
            .timeout(DEFAULT_TIMEOUT)
            .build()
            .expect("failed to build reqwest client");
        Self {
            client,
            api_key: api_key.to_string(),
            model: DEFAULT_MODEL.to_string(),
            consecutive_failures: AtomicU32::new(0),
            tripped_until: AtomicU64::new(0),
        }
    }

    pub fn is_available(&self) -> bool {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        now >= self.tripped_until.load(Ordering::Acquire)
    }

    pub fn record_failure(&self) {
        let prev = self.consecutive_failures.fetch_add(1, Ordering::AcqRel);
        if prev + 1 >= CIRCUIT_BREAKER_THRESHOLD {
            let trip_until = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64
                + CIRCUIT_BREAKER_COOLDOWN_SECS * 1000;
            self.tripped_until.store(trip_until, Ordering::Release);
            tracing::warn!(
                cooldown_secs = CIRCUIT_BREAKER_COOLDOWN_SECS,
                "Vision oracle circuit breaker tripped"
            );
        }
    }

    pub fn record_success(&self) {
        let prev = self.consecutive_failures.swap(0, Ordering::AcqRel);
        if prev >= CIRCUIT_BREAKER_THRESHOLD {
            tracing::info!("Vision oracle circuit breaker recovered");
        }
    }

    // ... existing find_element_coordinates unchanged for now
}
```

**Step 4: Run tests**

Run: `cargo test -p aura-gemini circuit_breaker`
Expected: PASS

**Step 5: Commit**

```bash
git add crates/aura-gemini/src/vision_oracle.rs
git commit -m "feat: add circuit breaker to vision oracle"
```

---

### Task 3: Update Oracle Prompt, Signature, and API Fields

**Files:**
- Modify: `crates/aura-gemini/src/vision_oracle.rs:39-143`

**Step 1: Write failing tests**

```rust
#[tokio::test]
async fn find_element_coordinates_accepts_target_and_origin() {
    let oracle = VisionOracle::new("fake-key");
    let result = oracle
        .find_element_coordinates(
            "base64data",
            100.0, 200.0,       // hint
            1920, 1080,         // img dims
            1920, 1080,         // screen dims
            Some("blue Submit button"),  // target
            0.0, 0.0,          // display origin
        )
        .await;
    assert!(result.is_err()); // fake key = HTTP error, but it compiled
}

#[test]
fn normalize_hint_coords() {
    // hint (960, 540) on 1920x1080 screen -> (500, 500) in 0-1000
    let norm_x = (960.0 / 1920.0 * 1000.0) as u32;
    let norm_y = (540.0 / 1080.0 * 1000.0) as u32;
    assert_eq!(norm_x, 500);
    assert_eq!(norm_y, 500);
}
```

**Step 2: Run tests to verify they fail**

Run: `cargo test -p aura-gemini find_element_coordinates_accepts`
Expected: FAIL — wrong number of arguments

**Step 3: Update `find_element_coordinates`**

New signature:

```rust
#[allow(clippy::too_many_arguments)]
pub async fn find_element_coordinates(
    &self,
    screenshot_b64: &str,
    hint_x: f64,
    hint_y: f64,
    img_w: u32,
    img_h: u32,
    screen_w: u32,
    screen_h: u32,
    target: Option<&str>,
    display_origin_x: f64,
    display_origin_y: f64,
) -> Result<(f64, f64)> {
    if screen_w == 0 || screen_h == 0 {
        anyhow::bail!("Invalid screen dimensions: {}x{}", screen_w, screen_h);
    }

    // Normalize hint coords to 0-1000 (same space as output)
    let norm_hint_x = (hint_x / screen_w as f64 * 1000.0) as u32;
    let norm_hint_y = (hint_y / screen_h as f64 * 1000.0) as u32;

    let prompt = match target {
        Some(desc) => format!(
            "You are a precise UI click targeting system for macOS.\n\n\
             Target: {desc}\n\
             Hint: approximately [{norm_hint_y}, {norm_hint_x}] (0-1000 normalized)\n\n\
             Rules:\n\
             - Find the element matching the target description, not just the nearest element to the hint\n\
             - Return the CENTER of the element, not an edge\n\
             - If multiple elements match, prefer the one closest to the hint\n\
             - If the target is on a canvas or content area (not a UI control), return the hint unchanged\n\
             - If the target is not visible or is covered by another element, return [0, 0]\n\
             - Return ONLY [y, x] normalized to 0-1000. No other text."
        ),
        None => format!(
            "You are a precise UI click targeting system for macOS.\n\n\
             Hint: approximately [{norm_hint_y}, {norm_hint_x}] (0-1000 normalized)\n\n\
             Find the nearest clickable UI element to the hint coordinates.\n\
             Return ONLY the center point as [y, x] normalized to 0-1000. No other text."
        ),
    };

    let body = serde_json::json!({
        "contents": [{
            "parts": [
                { "text": prompt },
                {
                    "inline_data": { "mime_type": "image/jpeg", "data": screenshot_b64 }
                }
            ]
        }],
        "generationConfig": {
            "temperature": 0.0,
            "maxOutputTokens": 100,
            "mediaResolution": "MEDIA_RESOLUTION_ULTRA_HIGH"
        }
    });

    // ... HTTP call unchanged ...

    // After denormalize, add display origin for global coords
    let (local_x, local_y) = denormalize(norm_x, norm_y, screen_w, screen_h);
    let global_x = local_x + display_origin_x;
    let global_y = local_y + display_origin_y;

    Ok((global_x, global_y))
}
```

Key changes:
- `target: Option<&str>` parameter
- `display_origin_x/y` parameters, added to output
- Hint coords normalized to 0-1000 (same space as output)
- Part-level `media_resolution` removed (was invalid API field)
- `maxOutputTokens`: 50 -> 100
- New prompt with failure signal, canvas rule, occlusion rule
- Zero-dimension guard

**Step 4: Run tests**

Run: `cargo test -p aura-gemini`
Expected: PASS

**Step 5: Commit**

```bash
git add crates/aura-gemini/src/vision_oracle.rs
git commit -m "feat: update oracle prompt, add target param, fix multi-monitor coords"
```

---

### Task 4: Add `target` to Click Tool Schema

**Files:**
- Modify: `crates/aura-gemini/src/tools.rs:94-122`

**Step 1: Write the failing test**

```rust
#[test]
fn click_tool_has_target_parameter() {
    let tools = build_tool_declarations();
    let decls = tools[0].function_declarations.as_ref().unwrap();
    let click = decls.iter().find(|fd| fd.name == "click").unwrap();
    let props = click.parameters["properties"].as_object().unwrap();
    assert!(props.contains_key("target"), "click tool should have 'target' parameter");
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p aura-gemini click_tool_has_target_parameter`
Expected: FAIL

**Step 3: Add `target` to click tool declaration**

Update the click `FunctionDeclaration` description and properties:

```rust
FunctionDeclaration {
    name: "click".into(),
    description: "Click at the specified screen coordinates. Always include a \
        target description of what you're clicking so the targeting system can \
        visually locate the exact element. Defaults to single left click."
        .into(),
    parameters: json!({
        "type": "object",
        "properties": {
            "x": { "type": "number", "description": "X coordinate" },
            "y": { "type": "number", "description": "Y coordinate" },
            "target": { "type": "string", "description": "Short UNIQUE description of the UI element you're clicking (e.g. 'blue Submit button at bottom of form', 'Safari address bar'). Include label text, color, or position. Used by the vision targeting system." },
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
                "description": "Optional bounding box [y0, x0, y1, x1] (normalized 0-1000) of the expected target element."
            }
        },
        "required": ["x", "y"]
    }),
    behavior: Some("NON_BLOCKING".into()),
},
```

**Step 4: Run tests**

Run: `cargo test -p aura-gemini`
Expected: All pass (17 declarations unchanged, just modified one)

**Step 5: Commit**

```bash
git add crates/aura-gemini/src/tools.rs
git commit -m "feat: add target description to click tool schema"
```

---

### Task 5: Replace AX Targeting with Oracle-First in tools.rs

**Files:**
- Modify: `crates/aura-daemon/src/tools.rs:235-359`

This is the core change. Replace the AX hit-test + oracle-fallback block with oracle-first logic, 4s budget, and delta guard.

**Step 1: Replace the click targeting block**

In `tools.rs`, replace lines 235-359 (from `// AX hit-test:` through the end of the `else` block) with:

```rust
            // Extract target description for vision oracle
            let target = args
                .get("target")
                .and_then(|v| v.as_str())
                .map(String::from);

            // Oracle-first coordinate refinement with 4s total budget
            let mut targeting_info = serde_json::json!({});
            let raw_x = x;
            let raw_y = y;

            /// Maximum distance (logical px) the oracle can move coords from the hint.
            /// Beyond this, the oracle result is discarded as unreliable.
            const MAX_ORACLE_DELTA: f64 = 150.0;

            /// Total time budget for oracle refinement (capture + API call + retry).
            const ORACLE_BUDGET: Duration = Duration::from_secs(4);

            if let Some(oracle) = vision_oracle.filter(|o| o.is_available()) {
                let oracle_start = std::time::Instant::now();
                tracing::info!(x, y, target = ?target, "Querying vision oracle");

                let oracle_result = tokio::time::timeout(ORACLE_BUDGET, async {
                    let frame = tokio::task::spawn_blocking(|| {
                        aura_screen::capture::capture_screen_high_res()
                    })
                    .await
                    .map_err(|e| anyhow::anyhow!("Screenshot task panicked: {e}"))?
                    .map_err(|e| anyhow::anyhow!("Screenshot capture failed: {e}"))?;

                    let target_ref = target.as_deref();
                    let mut result = oracle
                        .find_element_coordinates(
                            &frame.jpeg_base64,
                            x, y,
                            frame.width, frame.height,
                            frame.logical_width, frame.logical_height,
                            target_ref,
                            frame.display_origin_x, frame.display_origin_y,
                        )
                        .await;

                    // Single retry on failure
                    if result.is_err() {
                        tracing::warn!("Vision oracle attempt 1 failed, retrying");
                        result = oracle
                            .find_element_coordinates(
                                &frame.jpeg_base64,
                                x, y,
                                frame.width, frame.height,
                                frame.logical_width, frame.logical_height,
                                target_ref,
                                frame.display_origin_x, frame.display_origin_y,
                            )
                            .await;
                    }

                    result
                })
                .await;

                let elapsed_ms = oracle_start.elapsed().as_millis() as u64;

                match oracle_result {
                    Ok(Ok((ox, oy))) => {
                        let delta = ((ox - raw_x).powi(2) + (oy - raw_y).powi(2)).sqrt();
                        if delta > MAX_ORACLE_DELTA {
                            tracing::warn!(
                                delta = format!("{:.1}", delta),
                                max = MAX_ORACLE_DELTA,
                                "Oracle delta too large — discarding, using raw coords"
                            );
                            oracle.record_success(); // API worked, just result was weird
                            targeting_info = serde_json::json!({
                                "vision_oracle": false,
                                "raw_coords": [raw_x, raw_y],
                                "oracle_coords": [ox, oy],
                                "delta_px": (delta * 10.0).round() / 10.0,
                                "elapsed_ms": elapsed_ms,
                                "targeting_hint": "Oracle delta exceeded threshold — using raw coords",
                            });
                        } else {
                            oracle.record_success();
                            x = ox;
                            y = oy;
                            targeting_info = serde_json::json!({
                                "vision_oracle": true,
                                "raw_coords": [raw_x, raw_y],
                                "oracle_coords": [ox, oy],
                                "delta_px": (delta * 10.0).round() / 10.0,
                                "elapsed_ms": elapsed_ms,
                            });
                        }
                    }
                    Ok(Err(e)) => {
                        oracle.record_failure();
                        tracing::warn!(error = %e, elapsed_ms, "Vision oracle failed");
                        targeting_info = serde_json::json!({
                            "vision_oracle": false,
                            "raw_coords": [raw_x, raw_y],
                            "oracle_error": format!("{e}"),
                            "elapsed_ms": elapsed_ms,
                            "targeting_hint": "Oracle failed — using raw coordinates",
                        });
                    }
                    Err(_) => {
                        // Timeout
                        oracle.record_failure();
                        tracing::warn!("Vision oracle timed out after 4s");
                        targeting_info = serde_json::json!({
                            "vision_oracle": false,
                            "raw_coords": [raw_x, raw_y],
                            "oracle_error": "timeout",
                            "elapsed_ms": elapsed_ms,
                            "targeting_hint": "Oracle timed out — using raw coordinates",
                        });
                    }
                }
            } else {
                // Oracle unavailable (not configured or circuit breaker tripped)
                let reason = if vision_oracle.is_some() { "circuit_breaker" } else { "not_configured" };
                targeting_info = serde_json::json!({
                    "vision_oracle": false,
                    "raw_coords": [raw_x, raw_y],
                    "oracle_skipped": reason,
                    "targeting_hint": "Oracle unavailable — using raw coordinates",
                });
            }
```

**Step 2: Handle the `.filter()` on `Option<&VisionOracle>`**

The line `vision_oracle.filter(|o| o.is_available())` requires `Option<&VisionOracle>`. Since the function signature already provides `vision_oracle: Option<&VisionOracle>`, `.filter()` works directly.

For the `record_success`/`record_failure` calls: these use atomics so `&self` is sufficient (no `&mut`).

**Step 3: Verify compilation**

Run: `cargo check -p aura-daemon`
Expected: PASS

**Step 4: Commit**

```bash
git add crates/aura-daemon/src/tools.rs
git commit -m "feat: replace AX targeting with oracle-first + 4s budget + delta guard"
```

---

### Task 6: Update Aura System Prompt

**Files:**
- Modify: `crates/aura-gemini/src/config.rs:5-236`

**Step 1: Write failing tests**

```rust
#[test]
fn system_prompt_has_vision_targeting_guidance() {
    let prompt = DEFAULT_SYSTEM_PROMPT;
    assert!(
        prompt.contains("approximate"),
        "Prompt should tell Gemini coords are approximate hints"
    );
    assert!(
        prompt.contains("target"),
        "Prompt should reference the click target parameter"
    );
}

#[test]
fn system_prompt_click_tool_has_target() {
    let prompt = DEFAULT_SYSTEM_PROMPT;
    assert!(
        prompt.contains("click(x, y, target?"),
        "Click tool definition should show target parameter"
    );
}
```

**Step 2: Run tests to verify they fail**

Run: `cargo test -p aura-gemini system_prompt_has_vision_targeting`
Expected: FAIL

**Step 3: Update DEFAULT_SYSTEM_PROMPT**

Make these specific edits to the prompt string:

1. **`<vision>` section** (after "Never manually scale coordinates" line) — add:
```
Click Targeting:
- Your click coordinates are approximate hints — a vision targeting system refines them to the exact element center.
- Focus on clicking the RIGHT area, not the exact pixel.
- ALWAYS include a target description in click() calls — this dramatically improves accuracy.
```

2. **`<tools>` section** — replace click line with:
```
- click(x, y, target?, button?, click_count?, modifiers?, expected_bounds?): Click at screen coordinates. ALWAYS include target — a short UNIQUE description of what you're clicking (e.g. "blue Submit button at bottom of form", "Safari address bar"). Include label text, color, or position to disambiguate. Max click_count=3.
```

3. **`<automatic_behaviors>` section** — replace click auto-retry line with:
```
- Click targeting: A vision system refines your coordinates to the exact UI element using the target description you provide. More descriptive targets = more accurate clicks. If the vision system is unavailable, your raw coordinates are used as-is.
- Click auto-retry: if screen doesn't change after click, system retries at nearby offsets (up to 4 times). retry_offset in response confirms.
```

**Step 4: Run all prompt tests**

Run: `cargo test -p aura-gemini system_prompt`
Expected: All pass. Verify each existing test still holds (XML markers, tool names, etc.)

**Step 5: Commit**

```bash
git add crates/aura-gemini/src/config.rs
git commit -m "feat: update system prompt for oracle-first targeting"
```

---

### Task 7: Fix All Callers of Updated Signatures

**Files:**
- Any file that constructs `CapturedFrame` or calls `find_element_coordinates`

**Step 1: Check compilation**

Run: `cargo check --workspace 2>&1 | head -40`

The compiler will report every call site that needs updating. Common fixes:
- `CapturedFrame` construction: add `display_origin_x: 0.0, display_origin_y: 0.0`
- `find_element_coordinates` calls: add `None` (target), `0.0, 0.0` (origin) for any remaining callers

**Step 2: Fix each reported error**

**Step 3: Full workspace build**

Run: `cargo build --workspace`
Expected: PASS

**Step 4: Full test suite**

Run: `cargo test --workspace`
Expected: All tests pass

**Step 5: Commit**

```bash
git add -A
git commit -m "fix: update all callers for new oracle and CapturedFrame signatures"
```

---

### Task 8: Integration Verification

**Files:** None (verification only)

**Step 1: Run with dev script**

Run: `./scripts/dev.sh`

**Step 2: Verify oracle fires for clicks**

Test in log output:
- `"Querying vision oracle"` should appear on every click
- `"vision_oracle": true` in targeting_info
- `delta_px` values present
- `elapsed_ms` values present

**Step 3: Test circuit breaker**

1. Disconnect network
2. Click 3 times — should see "circuit breaker tripped" in logs
3. Next clicks should show `"oracle_skipped": "circuit_breaker"` (instant, no timeout)
4. Reconnect network, wait 30s
5. Next click should show oracle firing again

**Step 4: Test multi-monitor (if available)**

1. Move a window to secondary display
2. Move mouse to secondary display
3. Click an element — oracle should return global coordinates
4. Verify click lands correctly

**Step 5: Test delta guard**

Watch logs for `delta_px` values. If any click shows delta > 150, the guard should fire:
`"Oracle delta exceeded threshold — using raw coords"`

**Step 6: Commit any fixes**

```bash
git add -A
git commit -m "fix: address issues found in integration testing"
```

---

## Task Dependency Graph

```
Task 1 (CapturedFrame origin) ──┐
Task 2 (Circuit breaker)  ──────┤
Task 3 (Oracle prompt/sig) ─────┼── Task 5 (tools.rs core change) ── Task 7 (fix callers) ── Task 8 (verify)
Task 4 (Tool schema) ───────────┘
Task 6 (System prompt) ─── independent, can run in parallel
```

Tasks 1, 2, 3, 4, 6 are independent of each other.
Task 5 depends on 1, 2, 3, 4.
Task 7 depends on all.
Task 8 depends on 7.

## Worst-Case Latency Budget

| Scenario | Time |
|----------|------|
| Happy path | ~1.75s |
| Oracle fails + retries | ~4s (capped by budget) |
| Circuit breaker tripped | ~50ms |
| + Spiral retry (no oracle) | +640ms |
| **Absolute worst case** | **~5.8s** |

vs previous plan's 84s worst case.
