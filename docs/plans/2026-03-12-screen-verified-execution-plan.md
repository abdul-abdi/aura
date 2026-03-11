# Screen-Verified Tool Execution — Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Gate tool responses on screen change confirmation so Gemini never claims success before the UI reflects it, and prefer visible mouse interactions over invisible AppleScript for clicks/navigation.

**Architecture:** Add a shared `AtomicU64` for the capture loop's last frame hash, readable by tool spawns. After executing a state-changing tool, poll for hash change (200ms intervals, 2s timeout). Enrich tool responses with `verified` and `warning` fields. Update system prompt to enforce verification behavior and mouse-first strategy.

**Tech Stack:** Rust, tokio, `std::sync::atomic::AtomicU64`, existing `CaptureTrigger` / `cap_notify` infrastructure.

---

### Task 1: Share frame hash between capture loop and tool spawns

**Files:**
- Modify: `crates/aura-daemon/src/main.rs:690-709` (capture loop setup + hash tracking)

**Step 1: Add shared AtomicU64 for last frame hash**

After line 691 (`let cap_notify = ...`), add a shared atomic:

```rust
let last_frame_hash = Arc::new(AtomicU64::new(0));
```

Clone it for the capture loop:

```rust
let cap_last_hash = Arc::clone(&last_frame_hash);
```

**Step 2: Update capture loop to write to shared hash**

In the capture loop (line 708 `tokio::spawn`), replace the local `let mut last_hash: u64 = 0;` (line 709) with reading/writing from the shared atomic:

```rust
// Replace line 709:
//   let mut last_hash: u64 = 0;
// With: (no local variable needed — use atomic)
```

Replace hash comparison at line 740:

```rust
// Replace lines 740-746:
let prev_hash = cap_last_hash.load(Ordering::Acquire);
if frame.hash == prev_hash {
    if let Some(tx) = cap_trigger.take_waiter() {
        let _ = tx.send(());
    }
    continue;
}
cap_last_hash.store(frame.hash, Ordering::Release);
```

**Step 3: Build and verify no regressions**

Run: `cargo build -p aura-daemon 2>&1 | head -20`
Expected: compiles with no errors

**Step 4: Commit**

```bash
git add crates/aura-daemon/src/main.rs
git commit -m "refactor: share frame hash via AtomicU64 for tool verification"
```

---

### Task 2: Add verification poll loop to state-changing tool execution

**Files:**
- Modify: `crates/aura-daemon/src/main.rs:1043-1142` (tool spawn, post-action block)

**Step 1: Capture pre-hash before tool execution**

Inside the `tokio::spawn` block, clone the shared hash for the tool spawn. In the clone section (lines 1036-1055), add:

```rust
let tool_last_hash = Arc::clone(&last_frame_hash);
```

Then right before `execute_tool` (before line 1088), capture the pre-action hash:

```rust
let pre_hash = if is_state_changing_tool(&name) {
    Some(tool_last_hash.load(Ordering::Acquire))
} else {
    None
};
```

**Step 2: Replace the current post-action block with verification poll loop**

Replace lines 1106-1142 (the current `if input_tool { ... }` block) with:

```rust
// For input tools: poll for screen change, then capture post_state
let verified;
let mut verification_reason: Option<&str> = None;

if let Some(pre) = pre_hash {
    // Brief delay to let UI settle after the input action
    tokio::time::sleep(Duration::from_millis(150)).await;

    // Poll for screen hash change: 200ms intervals, 2s timeout, 10 checks max
    let mut screen_changed = false;
    for _ in 0..10 {
        let rx = tool_capture_trigger.trigger_and_wait();
        tool_cap_notify.notify_one();
        let _ = tokio::time::timeout(
            Duration::from_millis(200),
            rx,
        ).await;

        let current_hash = tool_last_hash.load(Ordering::Acquire);
        if current_hash != pre {
            screen_changed = true;
            break;
        }
    }

    verified = screen_changed;
    if !screen_changed {
        verification_reason = Some("screen_unchanged_after_2s");
        tracing::warn!(tool = %name, "Screen unchanged after action — verification failed");
    }

    // Capture post-action state on a blocking thread (AX FFI)
    let post_state = tokio::time::timeout(
        Duration::from_millis(600),
        tokio::task::spawn_blocking(capture_post_state),
    )
    .await
    .unwrap_or(Ok(serde_json::json!({})))
    .unwrap_or_else(|_| serde_json::json!({}));

    // Check for post_state mismatch warning
    let warning = if verified {
        // Screen changed but focused element might not match expectations
        // (e.g., animation triggered hash change, not the action)
        None
    } else {
        // Screen didn't change — check if post_state at least looks right
        let has_focus = post_state
            .get("focused_element")
            .map(|e| !e.is_null())
            .unwrap_or(false);
        if has_focus {
            Some("screen_unchanged_but_element_focused — check post_state")
        } else {
            Some("screen_unchanged_and_no_focused_element")
        }
    };

    if let Some(obj) = response.as_object_mut() {
        obj.insert("verified".to_string(), serde_json::Value::Bool(verified));
        if let Some(reason) = verification_reason {
            obj.insert("verification_reason".to_string(), reason.into());
        }
        if let Some(warn) = warning {
            obj.insert("warning".to_string(), warn.into());
        }
        let mut ps = post_state;
        if let Some(ps_obj) = ps.as_object_mut() {
            ps_obj.insert(
                "screenshot_delivered".to_string(),
                serde_json::Value::Bool(verified), // only truly delivered if hash changed
            );
        }
        obj.insert("post_state".to_string(), ps);
    }
} else {
    verified = true; // non-state-changing tools are inherently "verified"
}
```

**Step 3: Build and verify**

Run: `cargo build -p aura-daemon 2>&1 | head -20`
Expected: compiles with no errors. The `verified` variable is used later (we'll use it in the status message next).

**Step 4: Update the status message to reflect verification**

At line 1151, update the status message to include verification:

```rust
let status_msg = if tool_success && verified {
    format!("\u{2705} Done: {name}")
} else if tool_success && !verified {
    format!("\u{26a0}\u{fe0f} Unverified: {name}")
} else {
    format!("\u{274c} Failed: {name}")
};
```

**Step 5: Build and verify**

Run: `cargo build -p aura-daemon 2>&1 | head -20`
Expected: compiles with no errors or warnings

**Step 6: Commit**

```bash
git add crates/aura-daemon/src/main.rs
git commit -m "feat: add screen verification gate for state-changing tools"
```

---

### Task 3: Update system prompt — verification behavior

**Files:**
- Modify: `crates/aura-gemini/src/config.rs:63-75` (Post-Action Verification section)

**Step 1: Replace the Post-Action Verification section**

Replace lines 63-75 (from `Post-Action Verification:` through `Never chain multiple actions...`) with:

```rust
Post-Action Verification:
Every input tool (click, click_element, type_text, press_key, move_mouse, scroll, drag)
returns verification data:
- verified: true/false — whether the screen visually changed after your action
- post_state: frontmost_app, focused_element (role, label, value, bounds), screenshot_delivered
- warning: optional hint if something looks off
- verification_reason: why verification failed (e.g. "screen_unchanged_after_2s")

CRITICAL verification rules:
- If verified is FALSE: the action likely failed. Do NOT tell the user it worked.
  Call get_screen_context() to understand what happened, then try a different approach.
- If verified is TRUE: proceed normally, but still check post_state matches expectations.
- If there is a warning: investigate with get_screen_context() before continuing.
- NEVER chain multiple actions without checking verified + post_state between each one.
- If an action fails verification twice with different approaches, tell the user honestly.
```

**Step 2: Build and verify**

Run: `cargo build -p aura-gemini 2>&1 | head -20`
Expected: compiles

**Step 3: Commit**

```bash
git add crates/aura-gemini/src/config.rs
git commit -m "feat: update system prompt with verification-aware behavior"
```

---

### Task 4: Update system prompt — mouse-first strategy for clicks/navigation

**Files:**
- Modify: `crates/aura-gemini/src/config.rs:34-61` (Strategy section)

**Step 1: Replace the Strategy and Decision flow sections**

Replace lines 34-61 (from `Strategy — Choosing the Right Tool:` through the decision flow) with:

```rust
Strategy — Choosing the Right Tool:

1. Keyboard shortcuts — always prefer press_key for known shortcuts:
   Cmd+C/V for copy/paste, Cmd+Tab for app switching, Cmd+W to close, etc.
   Faster and more reliable than clicking menus.

2. Clicking and navigating UI — use visible mouse interaction:
   Use click_element(label, role) for labeled buttons, links, tabs, checkboxes.
   Use click(x, y) for web pages, canvas, and unlabeled UI.
   Call get_screen_context() first — the UI elements list shows interactive elements with precise bounds.
   When an element has bounds, use those coordinates instead of guessing from the screenshot.
   The user can SEE the cursor move — this is intentional. Visible interaction > invisible automation.

3. App-specific scripting (no visual equivalent):
   Use AppleScript for operations that have no on-screen button or element:
   - Get Safari tab list: run_applescript('tell application "Safari" to get name of every tab of front window')
   - Window management: run_applescript('tell application "Finder" to set bounds of front window to {0,0,800,600}')
   - Text field manipulation with accessibility labels
   - App launching: activate_app("Safari")

4. Menu items — use click_menu_item for menu bar actions:
   click_menu_item(["File", "Save As..."]) — reliable, no coordinates needed.

Decision flow:
- Can it be done with a keyboard shortcut? Use press_key.
- Is it clicking a button, link, or UI control? Use click_element or click(x, y).
- Is it a menu bar action? Use click_menu_item.
- Does it need app scripting with no visual equivalent? Use run_applescript.
- Fallback: get_screen_context() + retry with different approach.
```

**Step 2: Build and verify**

Run: `cargo build -p aura-gemini 2>&1 | head -20`
Expected: compiles

**Step 3: Commit**

```bash
git add crates/aura-gemini/src/config.rs
git commit -m "feat: update system prompt to prefer visible mouse for clicks/navigation"
```

---

### Task 5: Add `use AtomicU64` import and create feature branch

**Files:**
- Modify: `crates/aura-daemon/src/main.rs` (imports, top of file)

**Note:** This task should be done FIRST before tasks 1-4 — it creates the branch and adds the import.

**Step 1: Create the feature branch**

```bash
git checkout -b feature/screen-verified-execution
```

**Step 2: Add the AtomicU64 import**

Check if `AtomicU64` is already imported. If not, add it alongside the existing atomic imports (near `AtomicU32`, `AtomicBool`):

```rust
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
```

**Step 3: Build and verify**

Run: `cargo build -p aura-daemon 2>&1 | head -20`
Expected: compiles (unused import warning is fine — tasks 1-2 will use it)

**Step 4: Commit**

```bash
git add crates/aura-daemon/src/main.rs
git commit -m "chore: add AtomicU64 import for screen verification"
```

---

## Execution Order

| Order | Task | Description |
|-------|------|-------------|
| 1 | Task 5 | Create branch + add import |
| 2 | Task 1 | Share frame hash via AtomicU64 |
| 3 | Task 2 | Add verification poll loop |
| 4 | Task 3 | Update system prompt — verification behavior |
| 5 | Task 4 | Update system prompt — mouse-first strategy |

Tasks 3 and 4 can be done in parallel (different sections of config.rs).
