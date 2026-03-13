# Production Hardening Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Make Aura's core observe-act-verify pipeline reliable for mundane Mac control tasks and add missing input primitives.

**Architecture:** Two parallel tracks — Track 1 fixes the core loop (verification, concurrency, truncation, system prompt), Track 2 adds missing primitives (modifier+click, drag interpolation, hold/release key, clipboard write, context menus, save_memory, bounded AX recursion). All changes are in existing files; no new crates.

**Tech Stack:** Rust, tokio, core_graphics CGEvent FFI, macOS AXUIElement FFI, serde_json

**Design doc:** `docs/plans/2026-03-12-production-hardening-design.md`

---

## Task 1: Fix truncation to preserve post_state

**Files:**
- Modify: `crates/aura-daemon/src/tool_helpers.rs:225-280`
- Test: `crates/aura-daemon/src/tool_helpers.rs:372-451` (existing test module)

**Step 1: Write the failing test**

Add to the existing `mod tests` in `tool_helpers.rs`:

```rust
#[test]
fn truncate_preserves_post_state_and_warning() {
    let large_output = "x".repeat(10_000);
    let mut response = serde_json::json!({
        "success": true,
        "verified": false,
        "warning": "screen_unchanged_but_element_focused",
        "stdout": large_output,
        "post_state": {
            "frontmost_app": "Safari",
            "focused_element": {
                "role": "AXTextField",
                "label": "Address and Search",
            },
            "screenshot_delivered": false,
        }
    });
    truncate_tool_response(&mut response);
    let obj = response.as_object().unwrap();
    assert!(obj.contains_key("post_state"), "post_state must survive truncation");
    assert!(obj.contains_key("warning"), "warning must survive truncation");
    assert_eq!(
        obj["post_state"]["frontmost_app"].as_str().unwrap(),
        "Safari"
    );
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p aura-daemon truncate_preserves_post_state_and_warning`
Expected: FAIL — post_state is dropped by current truncation logic

**Step 3: Write minimal implementation**

In `truncate_tool_response()` (line 246-280), modify the general size cap branch to also preserve `post_state` and `warning`:

```rust
// After line 259 (stdout extraction), add:
let warning = obj
    .get("warning")
    .and_then(|v| v.as_str())
    .map(|s| truncate_str(s, 200));
let post_state = obj.get("post_state").cloned().map(|mut ps| {
    // Trim focused_element details if too large
    if let Some(fe) = ps.get_mut("focused_element") {
        if let Some(v) = fe.get("value").and_then(|v| v.as_str()) {
            if v.len() > 200 {
                fe.as_object_mut().unwrap().insert(
                    "value".to_string(),
                    serde_json::Value::String(truncate_str(v, 200)),
                );
            }
        }
    }
    ps
});
```

After the existing `stdout` insert (line 271), add:

```rust
if let Some(w) = warning {
    obj.insert("warning".to_string(), serde_json::Value::String(w));
}
if let Some(ps) = post_state {
    obj.insert("post_state".to_string(), ps);
}
```

**Step 4: Run test to verify it passes**

Run: `cargo test -p aura-daemon truncate_preserves_post_state`
Expected: PASS

**Step 5: Commit**

```bash
git add crates/aura-daemon/src/tool_helpers.rs
git commit -m "fix: preserve post_state and warning in truncated tool responses"
```

---

## Task 2: Add run_applescript to verification with opt-out

**Files:**
- Modify: `crates/aura-daemon/src/tool_helpers.rs:292-305` (is_state_changing_tool)
- Modify: `crates/aura-daemon/src/processor.rs:358-362` (pre_hash logic)
- Modify: `crates/aura-daemon/src/tools.rs:33-94` (run_applescript handler — pass verify flag)
- Modify: `crates/aura-gemini/src/tools.rs:15-46` (run_applescript schema)

**Step 1: Write the failing test**

Add to `tool_helpers.rs` tests:

```rust
#[test]
fn run_applescript_is_state_changing() {
    assert!(is_state_changing_tool("run_applescript"));
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p aura-daemon run_applescript_is_state_changing`
Expected: FAIL

**Step 3: Add run_applescript to is_state_changing_tool**

In `tool_helpers.rs:292-305`, add `| "run_applescript"` to the matches! macro:

```rust
pub(crate) fn is_state_changing_tool(name: &str) -> bool {
    matches!(
        name,
        "move_mouse"
            | "click"
            | "type_text"
            | "press_key"
            | "scroll"
            | "drag"
            | "click_element"
            | "activate_app"
            | "click_menu_item"
            | "run_applescript"
            | "context_menu_click"
    )
}
```

**Step 4: Run test to verify it passes**

Run: `cargo test -p aura-daemon run_applescript_is_state_changing`
Expected: PASS

**Step 5: Add verify opt-out in processor**

In `processor.rs`, modify the `pre_hash` assignment (line 358-362). The args are available as `&args` (a `serde_json::Value`). Add a check for `verify: false`:

```rust
let pre_hash = if tools::is_state_changing_tool(&name) {
    // Allow tools to opt out of verification (e.g. run_applescript with verify: false)
    let verify = args
        .get("verify")
        .and_then(|v| v.as_bool())
        .unwrap_or(true);
    if verify {
        Some(tool_last_hash.load(Ordering::Acquire))
    } else {
        None
    }
} else {
    None
};
```

**Step 6: Add verify param to run_applescript schema**

In `aura-gemini/src/tools.rs`, in the `run_applescript` declaration (around line 15-46), add a `verify` property:

```rust
// Add to the properties object for run_applescript:
"verify": {
    "type": "boolean",
    "description": "Whether to verify screen changed after execution. Default true. Set false for read-only queries that don't modify the screen."
}
```

**Step 7: Commit**

```bash
git add crates/aura-daemon/src/tool_helpers.rs crates/aura-daemon/src/processor.rs crates/aura-gemini/src/tools.rs
git commit -m "feat: add run_applescript to verification loop with verify opt-out"
```

---

## Task 3: Sequential mutex for state-changing tools

**Files:**
- Modify: `crates/aura-daemon/src/processor.rs:73` (semaphore creation)
- Modify: `crates/aura-daemon/src/processor.rs:335-342` (permit acquisition in spawn)

**Step 1: Add mutex alongside semaphore**

At line 73 in `processor.rs`, after the existing semaphore:

```rust
let tool_semaphore = Arc::new(tokio::sync::Semaphore::new(8));
let state_change_mutex = Arc::new(tokio::sync::Mutex::new(()));
```

**Step 2: Clone mutex into the spawn closure**

Where `tool_semaphore` is cloned for the spawn (search for the clone block before `tokio::spawn`), add:

```rust
let tool_state_mutex = state_change_mutex.clone();
```

**Step 3: Replace permit acquisition with branching logic**

Replace the semaphore acquire block (lines 336-342) inside the `tokio::spawn`:

```rust
// State-changing tools run sequentially; others use the semaphore
let _state_guard;
let _permit;
if tools::is_state_changing_tool(&name) {
    _state_guard = Some(tool_state_mutex.lock().await);
    _permit = None;
} else {
    _state_guard = None;
    _permit = Some(match tool_semaphore.acquire().await {
        Ok(permit) => permit,
        Err(_) => {
            tracing::error!("Tool semaphore closed");
            return;
        }
    });
}
```

**Step 4: Build and test**

Run: `cargo build -p aura-daemon`
Run: `cargo test -p aura-daemon`
Expected: All pass, no compilation errors

**Step 5: Commit**

```bash
git add crates/aura-daemon/src/processor.rs
git commit -m "fix: enforce sequential execution for state-changing tools via mutex"
```

---

## Task 4: Video channel capacity increase

**Files:**
- Modify: `crates/aura-gemini/src/session.rs:82`

**Step 1: Change capacity**

Line 82: `mpsc::channel::<String>(8)` → `mpsc::channel::<String>(32)`

**Step 2: Build**

Run: `cargo build -p aura-gemini`
Expected: PASS

**Step 3: Commit**

```bash
git add crates/aura-gemini/src/session.rs
git commit -m "fix: increase video channel capacity from 8 to 32 to reduce frame drops"
```

---

## Task 5: Pre-click hover delay

**Files:**
- Modify: `crates/aura-daemon/src/tools.rs:154-187` (click handler)

**Step 1: Add 40ms delay after pre-move**

In the click handler, after the pre-move `run_input_blocking` call (around line 178) and before `run_with_pid_fallback`, add:

```rust
// Brief delay to let apps register hover state before clicking
tokio::time::sleep(std::time::Duration::from_millis(40)).await;
```

**Step 2: Build and test**

Run: `cargo build -p aura-daemon`
Expected: PASS

**Step 3: Commit**

```bash
git add crates/aura-daemon/src/tools.rs
git commit -m "fix: add 40ms hover settle delay between pre-move and click"
```

---

## Task 6: Modifier+click support

**Files:**
- Modify: `crates/aura-input/src/mouse.rs:29-72` (click), `142-183` (click_pid)
- Modify: `crates/aura-daemon/src/tools.rs:154-187` (click handler)
- Modify: `crates/aura-daemon/src/tool_helpers.rs:335-357` (run_with_pid_fallback — needs modifier passthrough)
- Modify: `crates/aura-gemini/src/tools.rs:90-108` (click schema)
- Test: `crates/aura-input/src/mouse.rs:232-259` (existing test module)

**Step 1: Write the failing test**

Add to `mouse.rs` tests:

```rust
#[test]
fn click_with_empty_modifiers_succeeds() {
    // Empty modifiers should work identically to no modifiers
    let result = click(100.0, 100.0, "left", 1, &[]);
    assert!(result.is_ok());
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p aura-input click_with_empty_modifiers`
Expected: FAIL — click doesn't take a modifiers param yet

**Step 3: Add modifiers param to click() and click_pid()**

Update `click` signature (line 29):

```rust
pub fn click(x: f64, y: f64, button: &str, click_count: u32, modifiers: &[&str]) -> Result<()> {
```

After creating `down` and `up` events (before posting), add flag setting:

```rust
let flags = modifier_flags(modifiers);
if !flags.is_empty() {
    down.set_flags(flags);
    up.set_flags(flags);
}
```

Similarly update `click_pid` (line 142):

```rust
pub fn click_pid(x: f64, y: f64, button: &str, click_count: u32, modifiers: &[&str], pid: i32) -> Result<()> {
```

Add a shared `modifier_flags` helper at the top of `mouse.rs`:

```rust
use core_graphics::event::CGEventFlags;

fn modifier_flags(modifiers: &[&str]) -> CGEventFlags {
    let mut flags = CGEventFlags::empty();
    for m in modifiers {
        match *m {
            "cmd" | "command" => flags |= CGEventFlags::CGEventFlagCommand,
            "shift" => flags |= CGEventFlags::CGEventFlagShift,
            "alt" | "option" => flags |= CGEventFlags::CGEventFlagAlternate,
            "ctrl" | "control" => flags |= CGEventFlags::CGEventFlagControl,
            _ => {}
        }
    }
    flags
}
```

**Step 4: Update all callers**

In `tool_helpers.rs`:
- `click_element_inner` (line 147): `click_pid(center_x, center_y, "left", 1, &[], pid)`
- `click_element_inner` (line 165): `click(center_x, center_y, "left", 1, &[])`
- `run_with_pid_fallback` calls in `tools.rs` click handler: pass modifiers through closures

In `tools.rs` click handler (line 154-187):
- Parse modifiers from args: `let modifiers: Vec<String> = args.get("modifiers").and_then(|v| v.as_array()).map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect()).unwrap_or_default();`
- Pass to click closures

**Step 5: Update click schema**

In `aura-gemini/src/tools.rs`, add to click properties:

```rust
"modifiers": {
    "type": "array",
    "items": { "type": "string", "enum": ["cmd", "shift", "alt", "ctrl"] },
    "description": "Modifier keys to hold during click. Use for Cmd+click (multi-select, new tab), Shift+click (range select)."
}
```

**Step 6: Run tests**

Run: `cargo test -p aura-input`
Run: `cargo test -p aura-daemon`
Expected: All pass (update existing tests that call click() to pass `&[]`)

**Step 7: Commit**

```bash
git add crates/aura-input/src/mouse.rs crates/aura-daemon/src/tools.rs crates/aura-daemon/src/tool_helpers.rs crates/aura-gemini/src/tools.rs
git commit -m "feat: add modifier+click support (Cmd+click, Shift+click)"
```

---

## Task 7: Interpolated drag with modifiers

**Files:**
- Modify: `crates/aura-input/src/mouse.rs:89-125` (drag), `194-230` (drag_pid)
- Modify: `crates/aura-daemon/src/tools.rs:298-314` (drag handler)
- Modify: `crates/aura-gemini/src/tools.rs:167-185` (drag schema)

**Step 1: Write the failing test**

Add to `mouse.rs` tests:

```rust
#[test]
fn drag_with_modifiers_accepts_empty() {
    let result = drag(100.0, 100.0, 200.0, 200.0, &[]);
    assert!(result.is_ok());
}
```

**Step 2: Rewrite drag() with interpolation + modifiers**

```rust
pub fn drag(from_x: f64, from_y: f64, to_x: f64, to_y: f64, modifiers: &[&str]) -> Result<()> {
    anyhow::ensure!(
        from_x.is_finite() && from_y.is_finite() && to_x.is_finite() && to_y.is_finite(),
        "Invalid drag coordinates"
    );
    let source = event_source()?;
    let from = CGPoint::new(from_x, from_y);
    let to = CGPoint::new(to_x, to_y);
    let flags = modifier_flags(modifiers);

    // Mouse down at source
    let down = CGEvent::new_mouse_event(
        source.clone(),
        CGEventType::LeftMouseDown,
        from,
        CGMouseButton::Left,
    )
    .map_err(|_| anyhow::anyhow!("Failed to create drag down event"))?;
    if !flags.is_empty() {
        down.set_flags(flags);
    }
    down.post(CGEventTapLocation::HID);
    std::thread::sleep(std::time::Duration::from_millis(50));

    // Interpolate intermediate points every 20px
    let dx = to_x - from_x;
    let dy = to_y - from_y;
    let distance = (dx * dx + dy * dy).sqrt();
    let steps = ((distance / 20.0).ceil() as usize).max(1);
    for i in 1..=steps {
        let t = i as f64 / steps as f64;
        let ix = from_x + dx * t;
        let iy = from_y + dy * t;
        let point = CGPoint::new(ix, iy);
        let drag_ev = CGEvent::new_mouse_event(
            source.clone(),
            CGEventType::LeftMouseDragged,
            point,
            CGMouseButton::Left,
        )
        .map_err(|_| anyhow::anyhow!("Failed to create drag move event"))?;
        if !flags.is_empty() {
            drag_ev.set_flags(flags);
        }
        drag_ev.post(CGEventTapLocation::HID);
        std::thread::sleep(std::time::Duration::from_millis(5));
    }

    // Mouse up at destination
    let up = CGEvent::new_mouse_event(source, CGEventType::LeftMouseUp, to, CGMouseButton::Left)
        .map_err(|_| anyhow::anyhow!("Failed to create drag up event"))?;
    up.post(CGEventTapLocation::HID);

    Ok(())
}
```

Apply the same pattern to `drag_pid()`, using `post_to_pid(pid)` instead of `post(HID)`.

**Step 3: Update callers**

In `tools.rs` drag handler: parse optional `modifiers` from args, pass to drag closures.
Update existing tests to pass `&[]` for modifiers.

**Step 4: Update drag schema**

In `aura-gemini/src/tools.rs`, add `modifiers` to drag properties (same format as click).

**Step 5: Run tests**

Run: `cargo test -p aura-input`
Run: `cargo test -p aura-daemon`
Expected: All pass

**Step 6: Commit**

```bash
git add crates/aura-input/src/mouse.rs crates/aura-daemon/src/tools.rs crates/aura-gemini/src/tools.rs
git commit -m "feat: interpolated drag with modifier support (20px steps, 5ms delay)"
```

---

## Task 8: Hold/release key (key_state tool)

**Files:**
- Modify: `crates/aura-input/src/keyboard.rs` (add key_down, key_up)
- Modify: `crates/aura-daemon/src/tools.rs` (add key_state handler)
- Modify: `crates/aura-gemini/src/tools.rs` (add key_state schema)

**Step 1: Write the failing test**

Add to `keyboard.rs` tests:

```rust
#[test]
fn key_down_creates_event_without_panic() {
    // key_down for shift (keycode 56) should not panic
    let result = key_down(56, &[]);
    assert!(result.is_ok());
}

#[test]
fn key_up_creates_event_without_panic() {
    let result = key_up(56, &[]);
    assert!(result.is_ok());
}
```

**Step 2: Implement key_down and key_up**

Add to `keyboard.rs`:

```rust
/// Press a key down without releasing. Caller must call key_up later.
pub fn key_down(key: CGKeyCode, modifiers: &[&str]) -> Result<()> {
    let source = event_source()?;
    let mut flags = CGEventFlags::empty();
    for m in modifiers {
        match *m {
            "cmd" | "command" => flags |= CGEventFlags::CGEventFlagCommand,
            "shift" => flags |= CGEventFlags::CGEventFlagShift,
            "alt" | "option" => flags |= CGEventFlags::CGEventFlagAlternate,
            "ctrl" | "control" => flags |= CGEventFlags::CGEventFlagControl,
            _ => {}
        }
    }
    let down = CGEvent::new_keyboard_event(source, key, true)
        .map_err(|_| anyhow::anyhow!("Failed to create key down event"))?;
    down.set_flags(flags);
    down.post(CGEventTapLocation::HID);
    Ok(())
}

/// Release a previously held key.
pub fn key_up(key: CGKeyCode, modifiers: &[&str]) -> Result<()> {
    let source = event_source()?;
    let _ = modifiers; // Modifiers on up are typically null
    let up = CGEvent::new_keyboard_event(source, key, false)
        .map_err(|_| anyhow::anyhow!("Failed to create key up event"))?;
    up.set_flags(CGEventFlags::CGEventFlagNull);
    up.post(CGEventTapLocation::HID);
    Ok(())
}
```

**Step 3: Add key_state tool handler**

In `tools.rs`, add a new match arm (before the `other` catch-all):

```rust
"key_state" => {
    let key_name = args.get("key").and_then(|v| v.as_str()).unwrap_or("");
    let action = args.get("action").and_then(|v| v.as_str()).unwrap_or("down");
    let keycode = match aura_input::keyboard::keycode_from_name(key_name) {
        Some(k) => k,
        None => return serde_json::json!({ "success": false, "error": format!("Unknown key: {key_name}") }),
    };
    let modifiers: Vec<String> = args.get("modifiers")
        .and_then(|v| v.as_array())
        .map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect())
        .unwrap_or_default();
    let mod_refs: Vec<&str> = modifiers.iter().map(|s| s.as_str()).collect();
    match action {
        "down" => {
            run_input_blocking(move || aura_input::keyboard::key_down(keycode, &mod_refs), "key_down").await
        }
        "up" => {
            run_input_blocking(move || aura_input::keyboard::key_up(keycode, &mod_refs), "key_up").await
        }
        _ => serde_json::json!({ "success": false, "error": format!("Unknown action: {action}. Use 'down' or 'up'.") }),
    }
}
```

Note: The `mod_refs` lifetime issue — since `run_input_blocking` takes a `FnOnce + Send + 'static`, you'll need to clone modifiers into the closure. Use `modifiers.clone()` and convert inside.

**Step 4: Add key_state to is_state_changing_tool**

Add `| "key_state"` to the matches! in `tool_helpers.rs`.

**Step 5: Add key_state schema**

In `aura-gemini/src/tools.rs`, add a new function declaration:

```rust
FunctionDeclaration {
    name: "key_state".into(),
    description: "Hold or release a key. Use before drag to hold Shift/Option during drag. Always release keys after use. Held keys are auto-released on session end.".into(),
    parameters: Some(Schema {
        schema_type: "object".into(),
        properties: Some(indexmap! {
            "key".into() => json!({"type": "string", "description": "Key name (e.g., 'shift', 'a', 'cmd')"}),
            "action".into() => json!({"type": "string", "enum": ["down", "up"], "description": "'down' to hold, 'up' to release"}),
            "modifiers".into() => json!({"type": "array", "items": {"type": "string", "enum": ["cmd", "shift", "alt", "ctrl"]}, "description": "Additional modifier keys"}),
        }),
        required: Some(vec!["key".into(), "action".into()]),
    }),
    behavior: Some(ToolBehavior::NON_BLOCKING),
}
```

**Step 6: Add held-key tracking and auto-release**

In `processor.rs`, add a `held_keys: Arc<Mutex<HashSet<CGKeyCode>>>` that tracks which keys are held. On `GeminiEvent::Disconnected`, release all held keys. Wire the set into the `key_state` handler to add/remove entries.

**Step 7: Run tests and commit**

Run: `cargo test -p aura-input && cargo test -p aura-daemon`

```bash
git add crates/aura-input/src/keyboard.rs crates/aura-daemon/src/tools.rs crates/aura-daemon/src/tool_helpers.rs crates/aura-daemon/src/processor.rs crates/aura-gemini/src/tools.rs
git commit -m "feat: add key_state tool for hold/release key support"
```

---

## Task 9: Clipboard write

**Files:**
- Modify: `crates/aura-screen/src/macos.rs` (add set_clipboard)
- Modify: `crates/aura-daemon/src/tools.rs` (add write_clipboard handler)
- Modify: `crates/aura-gemini/src/tools.rs` (add write_clipboard schema)

**Step 1: Implement set_clipboard**

Add to `macos.rs` after `get_clipboard()` (line 163):

```rust
/// Write text to the system clipboard via pbcopy.
pub fn set_clipboard(text: &str) -> std::io::Result<()> {
    use std::io::Write;
    use std::process::{Command, Stdio};
    let mut child = Command::new("pbcopy")
        .stdin(Stdio::piped())
        .spawn()?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(text.as_bytes())?;
    }
    child.wait()?;
    Ok(())
}
```

**Step 2: Add write_clipboard handler**

In `tools.rs`, add match arm:

```rust
"write_clipboard" => {
    let text = args.get("text").and_then(|v| v.as_str()).unwrap_or("");
    match aura_screen::macos::set_clipboard(text) {
        Ok(()) => serde_json::json!({ "success": true, "chars_written": text.len() }),
        Err(e) => serde_json::json!({ "success": false, "error": format!("Clipboard write failed: {e}") }),
    }
}
```

**Step 3: Add schema**

```rust
FunctionDeclaration {
    name: "write_clipboard".into(),
    description: "Write text to the system clipboard. Use with Cmd+V to paste. Useful for large text blocks or special characters that are hard to type.".into(),
    parameters: Some(Schema {
        schema_type: "object".into(),
        properties: Some(indexmap! {
            "text".into() => json!({"type": "string", "description": "Text to place on the clipboard"}),
        }),
        required: Some(vec!["text".into()]),
    }),
    behavior: Some(ToolBehavior::NON_BLOCKING),
}
```

**Step 4: Run tests and commit**

```bash
git add crates/aura-screen/src/macos.rs crates/aura-daemon/src/tools.rs crates/aura-gemini/src/tools.rs
git commit -m "feat: add write_clipboard tool for programmatic clipboard write"
```

---

## Task 10: Scroll-to-element in click_element

**Files:**
- Modify: `crates/aura-screen/src/accessibility.rs` (add ax_scroll_to_visible)
- Modify: `crates/aura-daemon/src/tool_helpers.rs:122-133` (click_element_inner offscreen path)

**Step 1: Add ax_scroll_to_visible**

Add to `accessibility.rs` after `ax_set_focused` (line 720):

```rust
/// Attempt to scroll an element into view using kAXScrollToVisibleAction.
/// Returns true if the action succeeded.
pub fn ax_scroll_to_visible(element: CFTypeRef) -> bool {
    let action_key = cf_string_from_str("AXScrollToVisible");
    let ret = unsafe { AXUIElementPerformAction(element, action_key.as_raw()) };
    ret == AX_ERROR_SUCCESS
}
```

**Step 2: Modify click_element_inner offscreen path**

In `tool_helpers.rs`, replace lines 122-133 (the no-bounds error path):

```rust
let bounds = match &target.bounds {
    Some(b) => b.clone(),
    None => {
        // Element has no bounds — try scrolling it into view
        // We need the raw AX element ref for this
        if let Some((raw_ref, updated_el)) =
            aura_screen::accessibility::find_ax_raw_nth(label, role, index)
        {
            if aura_screen::accessibility::ax_scroll_to_visible(raw_ref.as_raw()) {
                // Re-query bounds after scroll
                std::thread::sleep(std::time::Duration::from_millis(200));
                if let Some((_, el_after)) =
                    aura_screen::accessibility::find_ax_raw_nth(label, role, index)
                {
                    match el_after.bounds {
                        Some(b) => b,
                        None => return serde_json::json!({
                            "success": false,
                            "error": "Element found and scrolled but still has no bounds",
                            "element": { "role": target.role, "label": target.label },
                        }),
                    }
                } else {
                    return serde_json::json!({
                        "success": false,
                        "error": "Element lost after scroll-to-visible",
                    });
                }
            } else {
                return serde_json::json!({
                    "success": false,
                    "error": "Element found but has no bounds and scroll-to-visible failed (may be offscreen or hidden)",
                    "element": { "role": target.role, "label": target.label },
                });
            }
        } else {
            return serde_json::json!({
                "success": false,
                "error": "Element found but has no bounds (may be offscreen or hidden)",
                "element": { "role": target.role, "label": target.label },
            });
        }
    }
};
```

Note: `find_ax_raw_nth` is `pub(crate)` — it's accessible from `tool_helpers.rs` since both are in aura-daemon's dependency tree. The function is in aura-screen, which aura-daemon depends on. It returns `Option<(CfRef, UIElement)>`. The `CfRef` holds the raw AX element needed for `ax_scroll_to_visible`.

**Step 3: Run tests and commit**

```bash
cargo test -p aura-screen && cargo test -p aura-daemon
git add crates/aura-screen/src/accessibility.rs crates/aura-daemon/src/tool_helpers.rs
git commit -m "feat: auto-scroll offscreen elements into view before clicking"
```

---

## Task 11: Context menu handling

**Files:**
- Modify: `crates/aura-daemon/src/processor.rs` (right-click AX enrichment)
- Modify: `crates/aura-daemon/src/tools.rs` (context_menu_click handler)
- Modify: `crates/aura-daemon/src/tool_helpers.rs` (add context_menu_click to state-changing)
- Modify: `crates/aura-screen/src/accessibility.rs` (add get_menu_items helper)
- Modify: `crates/aura-gemini/src/tools.rs` (context_menu_click schema)

**Step 1: Add get_menu_items to accessibility.rs**

```rust
/// Collect AXMenuItem elements from the frontmost app's AX tree.
/// Used to read context menu items after a right-click.
pub fn get_menu_items() -> Vec<UIElement> {
    let pid = match crate::macos::get_frontmost_pid() {
        Some(p) => p,
        None => return Vec::new(),
    };
    let app = unsafe { AXUIElementCreateApplication(pid) };
    if app.is_null() {
        return Vec::new();
    }
    let app_ref = CfRef::new(app);
    let mut elements = Vec::new();
    let start = Instant::now();
    collect_menu_items(app_ref.as_raw(), 0, &mut elements, &start);
    elements
}

fn collect_menu_items(
    element: CFTypeRef,
    depth: usize,
    elements: &mut Vec<UIElement>,
    start_time: &Instant,
) {
    if depth > 8 || elements.len() >= 30 || start_time.elapsed().as_millis() >= TIMEOUT_MS {
        return;
    }
    let role = match get_ax_string(element, "AXRole") {
        Some(r) => r,
        None => return,
    };
    if role == "AXMenuItem" {
        let label = get_ax_string(element, "AXTitle")
            .filter(|s| !s.is_empty())
            .or_else(|| get_ax_string(element, "AXDescription").filter(|s| !s.is_empty()));
        let enabled = get_ax_bool(element, "AXEnabled");
        let bounds = match (get_ax_position(element), get_ax_size(element)) {
            (Some((x, y)), Some((w, h))) => Some(ElementBounds { x, y, width: w, height: h }),
            _ => None,
        };
        elements.push(UIElement {
            role,
            label,
            value: None,
            bounds,
            enabled,
            focused: false,
        });
        return; // Don't recurse into menu item children
    }
    let children = get_ax_children(element);
    for child in &children {
        collect_menu_items(*child, depth + 1, elements, start_time);
    }
    for child in &children {
        unsafe { CFRelease(*child) };
    }
}
```

**Step 2: Enrich right-click post_state with menu items**

In `processor.rs`, in the post-action verification block (around line 434), after `post_state` is captured, detect right-click and add menu items:

```rust
// After capturing post_state, check if this was a right-click
let is_right_click = name == "click"
    && args.get("button").and_then(|v| v.as_str()) == Some("right");

if is_right_click && verified {
    // Brief delay for context menu to appear in AX tree
    tokio::time::sleep(Duration::from_millis(100)).await;
    let menu_items = tokio::task::spawn_blocking(aura_screen::accessibility::get_menu_items)
        .await
        .unwrap_or_default();
    if !menu_items.is_empty() {
        let items_json: Vec<serde_json::Value> = menu_items.iter().map(|el| {
            serde_json::json!({
                "label": el.label,
                "enabled": el.enabled,
                "bounds": el.bounds.as_ref().map(|b| serde_json::json!({"x": b.x, "y": b.y, "w": b.width, "h": b.height})),
            })
        }).collect();
        if let Some(ps_obj) = ps.as_object_mut() {
            ps_obj.insert("menu_items".to_string(), serde_json::json!(items_json));
        }
    }
}
```

**Step 3: Add context_menu_click handler**

In `tools.rs`:

```rust
"context_menu_click" => {
    let x = args.get("x").and_then(|v| v.as_f64()).unwrap_or(0.0);
    let y = args.get("y").and_then(|v| v.as_f64()).unwrap_or(0.0);
    let item_label = args.get("item_label").and_then(|v| v.as_str()).unwrap_or("");
    let lx = dims.to_logical_x(x);
    let ly = dims.to_logical_y(y);

    // Right-click at position
    if let Err(e) = aura_input::mouse::click(lx, ly, "right", 1, &[]) {
        return serde_json::json!({ "success": false, "error": format!("Right-click failed: {e}") });
    }

    // Poll for menu items to appear (up to 500ms)
    let mut found_item = None;
    for _ in 0..10 {
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        let items = aura_screen::accessibility::get_menu_items();
        if let Some(item) = items.iter().find(|el| {
            el.label.as_deref().map(|l| l.to_lowercase().contains(&item_label.to_lowercase())).unwrap_or(false)
        }) {
            found_item = Some(item.clone());
            break;
        }
    }

    match found_item {
        Some(item) => {
            // Click the menu item via AXPress or coordinate fallback
            let result = aura_screen::accessibility::ax_perform_action(
                item.label.as_deref(), Some("AXMenuItem"), "AXPress"
            );
            if result.success {
                serde_json::json!({
                    "success": true,
                    "method": "context_menu_ax_press",
                    "clicked_item": item.label,
                })
            } else if let Some(ref bounds) = item.bounds {
                let (cx, cy) = bounds.center();
                let _ = aura_input::mouse::click(cx, cy, "left", 1, &[]);
                serde_json::json!({
                    "success": true,
                    "method": "context_menu_coordinate_click",
                    "clicked_item": item.label,
                })
            } else {
                serde_json::json!({ "success": false, "error": "Found menu item but couldn't click it" })
            }
        }
        None => {
            // Return available items for diagnostic
            let items = aura_screen::accessibility::get_menu_items();
            let available: Vec<String> = items.iter()
                .filter_map(|el| el.label.clone())
                .collect();
            serde_json::json!({
                "success": false,
                "error": format!("Menu item '{}' not found in context menu", item_label),
                "available_items": available,
            })
        }
    }
}
```

**Step 4: Add schema**

```rust
FunctionDeclaration {
    name: "context_menu_click".into(),
    description: "Right-click at coordinates and click a menu item by label. Atomic — no timing gap. Use instead of separate right-click + click for context menus.".into(),
    parameters: Some(Schema {
        schema_type: "object".into(),
        properties: Some(indexmap! {
            "x".into() => json!({"type": "number", "description": "X pixel coordinate for right-click"}),
            "y".into() => json!({"type": "number", "description": "Y pixel coordinate for right-click"}),
            "item_label".into() => json!({"type": "string", "description": "Label of the menu item to click (case-insensitive substring match)"}),
        }),
        required: Some(vec!["x".into(), "y".into(), "item_label".into()]),
    }),
    behavior: Some(ToolBehavior::NON_BLOCKING),
}
```

**Step 5: Run tests and commit**

```bash
cargo test -p aura-screen && cargo test -p aura-daemon
git add crates/aura-screen/src/accessibility.rs crates/aura-daemon/src/processor.rs crates/aura-daemon/src/tools.rs crates/aura-daemon/src/tool_helpers.rs crates/aura-gemini/src/tools.rs
git commit -m "feat: add context_menu_click tool and right-click menu item enrichment"
```

---

## Task 12: save_memory tool

**Files:**
- Modify: `crates/aura-daemon/src/processor.rs` (add save_memory inline handler next to recall_memory)
- Modify: `crates/aura-gemini/src/tools.rs` (add save_memory schema)

**Step 1: Add save_memory handler**

In `processor.rs`, after the `recall_memory` inline handler (around line 302), add:

```rust
"save_memory" => {
    let category = args.get("category").and_then(|v| v.as_str()).unwrap_or("context");
    let content = args.get("content").and_then(|v| v.as_str()).unwrap_or("");
    if content.is_empty() {
        let response = serde_json::json!({ "success": false, "error": "content is required" });
        if let Err(e) = session.send_tool_response(id, name, response).await {
            tracing::error!("Failed to send save_memory response: {e}");
        }
        continue;
    }
    let cat = category.to_string();
    let cont = content.to_string();
    let sid = session_id.clone();
    memory_op(&memory, move |mem| {
        mem.add_fact(&sid, &cat, &cont, &[], 0.7)
    }).await;
    let response = serde_json::json!({
        "success": true,
        "saved": { "category": category, "content_length": content.len() }
    });
    if let Err(e) = session.send_tool_response(id, name, response).await {
        tracing::error!("Failed to send save_memory response: {e}");
    }
    continue;
}
```

Note: Check the `add_fact` signature in `aura-memory` — it likely takes `(session_id, category, content, entities, importance)`. Adjust params accordingly.

**Step 2: Add schema**

```rust
FunctionDeclaration {
    name: "save_memory".into(),
    description: "Save a fact to persistent memory for recall in future sessions. Use for user preferences, learned workflows, and app-specific knowledge. Don't save transient observations.".into(),
    parameters: Some(Schema {
        schema_type: "object".into(),
        properties: Some(indexmap! {
            "category".into() => json!({"type": "string", "enum": ["preference", "habit", "entity", "task", "context"], "description": "Category of the fact"}),
            "content".into() => json!({"type": "string", "description": "The fact to remember"}),
        }),
        required: Some(vec!["category".into(), "content".into()]),
    }),
    behavior: Some(ToolBehavior::NON_BLOCKING),
}
```

**Step 3: Run tests and commit**

```bash
cargo test -p aura-daemon
git add crates/aura-daemon/src/processor.rs crates/aura-gemini/src/tools.rs
git commit -m "feat: add save_memory tool for persistent fact storage"
```

---

## Task 13: Bounded AX recursion for dropdowns/tabs

**Files:**
- Modify: `crates/aura-screen/src/accessibility.rs:247-305` (walk_element)
- Modify: `crates/aura-screen/src/context.rs` (add parent_label to UIElement)

**Step 1: Add parent_label field to UIElement**

In `context.rs`, add to `UIElement` struct:

```rust
pub struct UIElement {
    pub role: String,
    pub label: Option<String>,
    pub value: Option<String>,
    pub bounds: Option<ElementBounds>,
    pub enabled: bool,
    pub focused: bool,
    pub parent_label: Option<String>,  // NEW: set for children of dropdowns/tabs
}
```

Update all `UIElement` constructors across the codebase to include `parent_label: None` (there are ~8 locations in accessibility.rs, tool_helpers.rs, and tests).

**Step 2: Define recursion-worthy roles**

Add constant in `accessibility.rs`:

```rust
/// Interactive roles whose children should be collected (bounded, 1 level, max 10).
const RECURSE_INTO_ROLES: &[&str] = &[
    "AXPopUpButton",
    "AXComboBox",
    "AXTabGroup",
    "AXMenuBar",
];
const MAX_CHILDREN_PER_INTERACTIVE: usize = 10;
```

**Step 3: Modify walk_element to recurse into select roles**

Replace the current interactive-element branch (lines 268-295):

```rust
if INTERACTIVE_ROLES.contains(&role.as_str()) {
    let label = get_ax_string(element, "AXTitle")
        .filter(|s| !s.is_empty())
        .or_else(|| get_ax_string(element, "AXDescription").filter(|s| !s.is_empty()));
    let value = get_ax_string(element, "AXValue").filter(|s| !s.is_empty());
    let enabled = get_ax_bool(element, "AXEnabled");
    let focused = get_ax_bool(element, "AXFocused");
    let bounds = match (get_ax_position(element), get_ax_size(element)) {
        (Some((x, y)), Some((w, h))) => Some(ElementBounds { x, y, width: w, height: h }),
        _ => None,
    };
    let parent_label_for_children = label.clone();
    elements.push(UIElement {
        role: role.clone(),
        label,
        value,
        bounds,
        enabled,
        focused,
        parent_label: None,
    });

    // Bounded recursion for select container roles
    if RECURSE_INTO_ROLES.contains(&role.as_str()) {
        let children = get_ax_children(element);
        let mut child_count = 0;
        for child in &children {
            if child_count >= MAX_CHILDREN_PER_INTERACTIVE || elements.len() >= MAX_ELEMENTS {
                break;
            }
            if let Some(child_role) = get_ax_string(*child, "AXRole") {
                let child_label = get_ax_string(*child, "AXTitle")
                    .filter(|s| !s.is_empty())
                    .or_else(|| get_ax_string(*child, "AXDescription").filter(|s| !s.is_empty()));
                let child_value = get_ax_string(*child, "AXValue").filter(|s| !s.is_empty());
                let child_enabled = get_ax_bool(*child, "AXEnabled");
                let child_bounds = match (get_ax_position(*child), get_ax_size(*child)) {
                    (Some((x, y)), Some((w, h))) => Some(ElementBounds { x, y, width: w, height: h }),
                    _ => None,
                };
                elements.push(UIElement {
                    role: child_role,
                    label: child_label,
                    value: child_value,
                    bounds: child_bounds,
                    enabled: child_enabled,
                    focused: false,
                    parent_label: parent_label_for_children.clone(),
                });
                child_count += 1;
            }
        }
        for child in &children {
            unsafe { CFRelease(*child) };
        }
    }
} else {
    // Not interactive — recurse into children
    let children = get_ax_children(element);
    for child in children {
        walk_element(child, depth + 1, elements, start_time);
        unsafe { CFRelease(child) };
    }
}
```

**Step 4: Update tests**

Update `make_element` helper and all UIElement constructors in tests to include `parent_label: None`.

**Step 5: Run tests and commit**

```bash
cargo test -p aura-screen && cargo test -p aura-daemon
git add crates/aura-screen/src/context.rs crates/aura-screen/src/accessibility.rs crates/aura-daemon/src/tool_helpers.rs
git commit -m "feat: bounded AX recursion into dropdowns, combo boxes, tabs, menu bars"
```

---

## Task 14: System prompt accuracy rewrite

**Files:**
- Modify: `crates/aura-gemini/src/config.rs:5-113` (DEFAULT_SYSTEM_PROMPT)

**Step 1: Read the current system prompt**

Read `config.rs` lines 5-113 carefully.

**Step 2: Apply all corrections**

Key changes to make in the system prompt:
1. **FPS claim**: Change "2 per second" to "~2 per second while the screen is changing, slower during idle periods"
2. **screenshot_delivered**: Remove as a separate field — fold into verified explanation: "When verified=true, a fresh screenshot has been captured and delivered"
3. **click_menu_item**: Add note: "For the macOS menu bar (File/Edit/View) only — NOT for right-click context menus. Use context_menu_click for those."
4. **scroll**: Add: "Scrolls at current cursor position. Use move_mouse first to position the cursor over the target area."
5. **activate_app guidance**: Add: "If activate_app returns verified=false but post_state.frontmost_app matches the app name, activation succeeded — the app was already frontmost."
6. **Remove overly restrictive guards**: Change "only after you have identified the target coordinates from screen context or user instruction" to "Use pixel coordinates from the screenshot."
7. **Add new tools to the tool list**: key_state, write_clipboard, context_menu_click, save_memory
8. **Add key_state guidance**: "Use key_state(key, action='down') before drag to hold Shift/Option during drag. Always call key_state(key, action='up') after."
9. **Add context_menu_click guidance**: "For right-click menus, prefer context_menu_click(x, y, item_label) over separate right-click + click — it's atomic with no timing gap."
10. **press_key schema description**: Expand key list to include "0-9, forwarddelete, home, end, pageup, pagedown, punctuation (-, =, [, ], \\, ;, ', comma, period, /)"

**Step 3: Build to verify syntax**

Run: `cargo build -p aura-gemini`
Expected: PASS

**Step 4: Commit**

```bash
git add crates/aura-gemini/src/config.rs
git commit -m "fix: rewrite system prompt for accuracy — FPS, new tools, scroll, activate_app guidance"
```

---

## Task 15: Integration build and format

**Step 1: Full workspace build**

Run: `cargo build --workspace`
Expected: PASS

**Step 2: Run clippy**

Run: `cargo clippy --workspace -- -D warnings`
Fix any warnings.

**Step 3: Run all tests**

Run: `cargo test --workspace`
Expected: All pass

**Step 4: Format**

Run: `cargo fmt --all`

**Step 5: Final commit**

```bash
git add -A
git commit -m "chore: fix clippy warnings and format after production hardening"
```

---

## Execution Order

Tasks 1-5 are independent core loop fixes — can be parallelized.
Tasks 6-7 depend on each other (modifier support shared between click and drag).
Tasks 8-12 are independent new primitives — can be parallelized.
Task 13 (AX recursion) is independent.
Task 14 (system prompt) should be done last since it references all new tools.
Task 15 is the final integration check.

```
Parallel group A (core fixes):  Tasks 1, 2, 3, 4, 5
Sequential group B (modifiers):  Task 6 → Task 7
Parallel group C (primitives):   Tasks 8, 9, 10, 11, 12, 13
Final:                           Task 14 → Task 15
```
