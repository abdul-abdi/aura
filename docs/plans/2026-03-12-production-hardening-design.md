# Production Hardening Design — "Hands, Feet, and Eyes"

**Date:** 2026-03-12
**Branch:** production-hardening
**Goal:** Make the core observe-act-verify pipeline reliable for mundane tasks. Fix the bugs that cause basic Mac control to fail, add missing primitives that block entire categories of tasks.

## Context

The architecture and security are solid. The core loop (Gemini sees screen → calls tool → daemon executes → verifies → feeds back) exists and is well-engineered. But rough edges cause basic tasks to fail intermittently:

- AppleScript actions get no verification — Gemini flies blind after window management
- Context menus dismiss before Gemini can click items (3-10s LLM latency)
- Drag is a single teleport event — breaks Finder drag-and-drop
- No modifier+click — can't Cmd+click or Shift+click
- Concurrent state-changing tools race on shared screen hash
- Tool response truncation drops the diagnostic data Gemini needs most
- System prompt makes inaccurate claims about FPS, screenshot delivery, and tool behavior

## Design Decisions

| Decision | Choice | Rationale |
|---|---|---|
| AppleScript verification | Default verify, opt-out via `verify: false` | Fails safe — worst case is unnecessary verification, never missed verification |
| Context menus | AX enrichment on right-click + atomic `context_menu_click` tool | Eliminates timing gap; enrichment allows exploration |
| Drag improvement | Interpolated path + modifier support (Level 2) | Covers vast majority of real drag use cases |
| Concurrency model | Sequential mutex for state-changing tools | System prompt already demands one-at-a-time; architecture should enforce it |
| save_memory storage | SQLite only, Firestore syncs at session end | Fast local write, no network dependency in tool path |
| AX recursion | Bounded: 4 roles, 1 level deep, 10 children cap | High value for dropdowns/tabs without performance explosion |

---

## Track 1: Core Loop Fixes

### 1.1 — AppleScript Verification (Default Verify, Opt-Out)

**Files:** `tool_helpers.rs`, `tools.rs`, `processor.rs`, `aura-gemini/src/tools.rs`

- Add `"run_applescript"` to `is_state_changing_tool()`
- Add `verify: bool` (default `true`) param to `run_applescript` tool schema
- In processor post-action path: skip verification loop when tool args contain `"verify": false`
- System prompt guidance: "Pass `verify: false` only for read-only queries that don't change the screen"

### 1.2 — Sequential Mutex for State-Changing Tools

**Files:** `processor.rs`

- Split concurrency control:
  - `tokio::sync::Mutex` for state-changing tools — one at a time
  - `Semaphore::new(8)` retained for non-state-changing tools — concurrent
- In spawned task: check `is_state_changing_tool()` → acquire mutex, else → acquire semaphore
- Eliminates hash-race between concurrent state-changing tools

### 1.3 — Truncation Preserves post_state

**Files:** `tool_helpers.rs`

- `truncate_tool_response()`: add `post_state` and `warning` to preserved fields
- Truncate `post_state.focused_element` content if oversized, but never drop the field
- New preserved set: `success`, `verified`, `error`, `stdout`, `post_state`, `warning`

### 1.4 — System Prompt Accuracy

**Files:** `aura-gemini/src/config.rs`

- FPS: "~2 per second while the screen is changing, slower during idle periods"
- Remove `screenshot_delivered` as separate signal — fold into `verified` explanation
- `click_menu_item`: "For the macOS menu bar only — not for right-click context menus"
- `press_key` schema: add full key list (0-9, punctuation, forwarddelete, home, end, pageup, pagedown)
- `scroll`: "Scrolls at current cursor position. Use move_mouse first to target a specific area"
- Remove "only after get_screen_context" guards — screenshot coordinates are valid
- `activate_app`: "If `verified: false` but `post_state.frontmost_app` matches the app name, activation succeeded — the app was already frontmost"

### 1.5 — Video Channel Capacity

**Files:** `aura-gemini/src/session.rs`

- `mpsc::channel::<String>(8)` → `mpsc::channel::<String>(32)`
- Memory impact: ~10MB max at 32 slots × 300KB average JPEG base64. Acceptable.

### 1.6 — Pre-Click Hover Delay

**Files:** `aura-daemon/src/tools.rs`

- Add `tokio::time::sleep(Duration::from_millis(40)).await` between pre-move and click dispatch
- Consistent with existing 50ms delays in drag implementation

---

## Track 2: New Primitives

### 2.1 — Modifier+Click

**Files:** `mouse.rs`, `tools.rs`, `aura-gemini/src/tools.rs`

- `click()` / `click_pid()`: add `modifiers: CGEventFlags` param, `set_flags()` on both down and up events
- `tools.rs`: parse optional `modifiers` array from click args (same `["cmd", "shift", "alt", "ctrl"]` format as `press_key`), convert to `CGEventFlags` bitmask
- Tool schema: add `modifiers` param to click declaration

### 2.2 — Hold/Release Key

**Files:** `keyboard.rs`, `tools.rs`, `aura-gemini/src/tools.rs`

- `keyboard.rs`: new `key_down(keycode, modifiers)` and `key_up(keycode, modifiers)` — post single event each
- Single tool: `key_state(key, action: "down" | "up", modifiers?)` — fewer tools for Gemini
- Safety: track held keys in `HashSet<CGKeyCode>`, auto-release all on session disconnect
- System prompt: "Use key_state before drag to hold Shift/Option. Always release after."

### 2.3 — Interpolated Drag with Modifiers

**Files:** `mouse.rs`, `tools.rs`, `aura-gemini/src/tools.rs`

- Replace single `LeftMouseDragged` with interpolated path: generate points every 20px along line, `LeftMouseDragged` per point with 5ms delay
- Add `modifiers: CGEventFlags` param, set on all down/dragged/up events
- Tool schema: add `modifiers` param to drag declaration
- System prompt: "Option+drag to copy files. Cmd+drag to move without spring-loading."

### 2.4 — Clipboard Write

**Files:** `macos.rs`, `tools.rs`, `aura-gemini/src/tools.rs`

- `macos.rs`: `set_clipboard(text: &str)` using `pbcopy` via stdin pipe (already in sandbox allowlist)
- New tool: `write_clipboard(text)` — returns success/error
- System prompt: "Use write_clipboard then Cmd+V for large text or special characters"

### 2.5 — Scroll-to-Element

**Files:** `accessibility.rs`, `tool_helpers.rs`

- New: `ax_scroll_to_visible(element: AXUIElementRef)` — `AXUIElementPerformAction` with `kAXScrollToVisibleAction`
- Wire into `click_element_inner`: when element found but no bounds (offscreen), attempt scroll first, re-query bounds
- No new tool — transparent enhancement to `click_element`

### 2.6 — Context Menu Handling

**Files:** `processor.rs`, `tools.rs`, `tool_helpers.rs`, `aura-gemini/src/tools.rs`

*Part A — AX enrichment on right-click:*
- In post-action verification, detect right-click (check tool args for `button: "right"`)
- Capture `AXMenuItem` elements from frontmost app AX tree
- Include in `post_state.menu_items: [{label, bounds}]`

*Part C — Compound tool:*
- New tool: `context_menu_click(x, y, item_label)`
- Implementation: right-click → poll AX up to 500ms for matching `AXMenuItem` → AXPress or coordinate click
- Error response includes available menu items if label not found

### 2.7 — save_memory

**Files:** `tools.rs`, `processor.rs`, `aura-gemini/src/tools.rs`

- New tool: `save_memory(category, content)` — category enum: `preference | habit | entity | task | context`
- Writes to SQLite `facts` table via `memory_op` → `add_fact`
- FTS5 auto-indexes, immediately searchable via `recall_memory`
- Firestore sync at session end (existing pipeline, no changes needed)
- System prompt: "Save important user preferences, learned workflows, app-specific knowledge. Don't save transient observations."

### 2.8 — Bounded AX Recursion

**Files:** `accessibility.rs`

- Modify `walk_element`: for 4 roles, recurse one level into children:
  - `AXPopUpButton`, `AXComboBox`, `AXTabGroup`, `AXMenuBar`
- Cap: 10 children per interactive element
- All other interactive roles: no recursion (unchanged)
- Child elements get `parent_label` field for Gemini context

---

## File Change Map

| File | Changes |
|---|---|
| `crates/aura-input/src/mouse.rs` | `modifiers` param on click/click_pid/drag/drag_pid, interpolated drag path |
| `crates/aura-input/src/keyboard.rs` | New `key_down()`, `key_up()` single-event functions |
| `crates/aura-screen/src/accessibility.rs` | Bounded recursion for 4 roles, `ax_scroll_to_visible()` |
| `crates/aura-screen/src/macos.rs` | `set_clipboard()` via pbcopy |
| `crates/aura-daemon/src/processor.rs` | Sequential mutex, right-click AX enrichment, run_applescript verify check |
| `crates/aura-daemon/src/tools.rs` | Modifier parsing for click/drag, 40ms hover delay, new handlers: key_state, write_clipboard, context_menu_click |
| `crates/aura-daemon/src/tool_helpers.rs` | `is_state_changing_tool` update, truncation fix, scroll-to-visible in click_element |
| `crates/aura-gemini/src/tools.rs` | Schema updates: click modifiers, drag modifiers, run_applescript verify, new tool schemas |
| `crates/aura-gemini/src/config.rs` | System prompt rewrite |
| `crates/aura-gemini/src/session.rs` | Channel capacity 8→32 |

**10 files modified, 0 new files, 4 new tools**

---

## What This Unlocks

| Scenario | Before | After |
|---|---|---|
| Open Safari, go to google.com | Fragile cold-start timing | activate_app verified correctly, no false alarm |
| Move Finder window left | AppleScript blind | Verified, Gemini confirms visually |
| Right-click → Get Info | Menu dismisses before click | `context_menu_click` — atomic, no timing gap |
| Cmd+click multiple files | Impossible | `click(x, y, modifiers: ["cmd"])` |
| Drag file to Desktop | Single teleport, breaks Finder | Interpolated path + modifier support |
| Shift+drag to constrain | Impossible | `hold_key → drag → release_key` |
| Complex app with dropdowns | Blind to contents | Bounded AX recursion shows options |
| Offscreen button in long form | Error: "no bounds" | Auto-scroll then click |
| "Remember I like dark mode" | Can't persist | `save_memory("preference", "...")` |
| Rapid 5-action sequence | Dropped screenshots, hash races | Channel 32, sequential mutex |
