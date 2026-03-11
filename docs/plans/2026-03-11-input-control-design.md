# Input Control & AppleScript Integration Redesign

## Problem

Three issues severely limit Aura's ability to control the Mac:

**A) Coordinate inaccuracy** — Screenshots downscaled from retina (5120x2880) to 1920px max. Gemini estimates pixel coordinates on blurry images. Small errors amplify 1.3-2.7x when scaled back to logical points. No UI element data — purely vision-based guessing.

**B) Tool abstraction too low-level** — Opening a URL requires ~5 tool calls (click address bar, type, press return) vs 1 AppleScript. System prompt actively discourages AppleScript. No composite tools exist.

**C) No decision guidance** — One vague rule: "use AppleScript for things that can't be done with mouse/keyboard." No decision tree, no examples, no permission awareness. AI learns about failures only after trying.

## Design

Three coordinated changes:

### 1. System Prompt Rewrite

Replace the Strategy section in `aura-gemini/src/config.rs` with a clear decision tree:

```
Strategy — Choosing the Right Tool:

1. App automation (menus, launching, windows, text fields with labels):
   -> Use AppleScript or dedicated tools (activate_app, click_menu_item, click_element).
   AppleScript is faster, more reliable, and atomic — one call instead of five.
   Examples:
   - Open a URL: run_applescript('open location "https://..."')
   - Click a menu: click_menu_item(["File", "Save As..."])
   - Activate an app: activate_app("Safari")
   - Get Safari tabs: run_applescript('tell application "Safari" to get name of every tab of front window')
   - Set a text field: click_element(label: "Search", role: "textfield") then type_text(...)
   - Window management: run_applescript('tell application "Finder" to set bounds of front window to {0,0,800,600}')

2. Visual/coordinate-based interaction (web pages, canvas, games, custom UI without labels):
   -> Use click(x, y), type_text, press_key, drag.
   Look at the screenshot, identify coordinates, click. Wait for next screenshot to verify.
   Use get_screen_context() first — the ui_elements list shows interactive elements with precise bounds.
   When an element has bounds, use those coordinates instead of guessing from the screenshot.

3. Keyboard shortcuts — always prefer press_key for known shortcuts:
   -> Cmd+C/V for copy/paste, Cmd+Tab for app switching, Cmd+W to close, etc.
   Faster and more reliable than clicking menus.

Decision flow:
- Can it be done with a keyboard shortcut? -> press_key
- Is there a dedicated tool for it? (activate_app, click_menu_item, click_element) -> use it
- Is it app automation with scriptable elements? -> run_applescript
- Is it visual interaction on a web page or unlabeled UI? -> click/type_text with coordinates from screenshot or ui_elements bounds

After any action, wait for the next screenshot to verify the result before proceeding.
If a click misses, check ui_elements for the correct bounds and retry.
```

Remove contradictory lines:
- "Prefer direct UI interaction (click, type) over AppleScript when possible"
- "Prefer simple scripts -- chain multiple calls over one complex script"

### 2. Enriched get_screen_context with AX Tree

#### New types in `aura-screen/src/context.rs`:

```rust
pub struct UIElement {
    pub role: String,                  // "AXButton", "AXTextField", etc.
    pub label: Option<String>,         // AXTitle or AXDescription
    pub value: Option<String>,         // AXValue (text contents, checkbox state)
    pub bounds: Option<ElementBounds>, // {x, y, width, height} in logical points
    pub enabled: bool,
    pub focused: bool,
}

pub struct ElementBounds {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}
```

#### New module `aura-screen/src/accessibility.rs`:

- `get_focused_app_elements() -> Vec<UIElement>` — walks AX tree of frontmost app
- Uses `AXUIElementCreateApplication(pid)` -> recursively walks children
- Collects interactive elements: button, text field, menu item, checkbox, radio button, link, tab, popup button, slider
- Depth limit: 5 levels
- Max elements: 50 (prioritize interactive over static)
- Timeout: 500ms (bail early on unresponsive apps)
- Uses raw FFI to CoreFoundation + ApplicationServices (same pattern as existing aura-input/src/accessibility.rs)

#### Output format change:

```
Frontmost app: Safari
Window title: GitHub - aura
Open windows: Safari - GitHub, Terminal - bash
Clipboard: some text...
UI Elements (interactive):
  [0] button "Back" bounds={x:48, y:52, w:30, h:30} enabled
  [1] button "Forward" bounds={x:82, y:52, w:30, h:30} enabled
  [2] textfield "Address and Search Bar" bounds={x:200, y:48, w:600, h:36} enabled focused
  ...
```

Graceful degradation: if Accessibility permission not granted, returns `ui_elements: []` with note.

### 3. Three Keystone Tools

#### click_element(label?, role?, index?)

Find and click a UI element by accessibility properties.

- `label` (string, optional) — Substring match against AXTitle/AXDescription/AXValue, case-insensitive
- `role` (string, optional) — Filter: "button", "textfield", "checkbox", "menuitem", "link", "tab", "popupbutton"
- `index` (integer, optional) — Nth match (0-indexed, default 0)

Implementation:
1. Walk AX tree of frontmost app
2. Filter by label/role
3. No match -> return error with available alternatives
4. Match -> compute center from bounds -> click via CGEvent
5. Return `{ success: true, element: { role, label, bounds }, clicked_at: { x, y } }`

#### activate_app(name)

Launch or bring an app to front.

- `name` (string, required) — e.g. "Safari", "Terminal"

Implementation: `tell application "{name}" to activate` via existing ScriptExecutor.

#### click_menu_item(menu_path, app?)

Click a menu item by path.

- `menu_path` (array of strings, required) — e.g. `["File", "Save As..."]`
- `app` (string, optional) — Defaults to frontmost app

Implementation: Build System Events AppleScript dynamically. Supports 2-item and 3-item (submenu) paths.

### Tool Set (13 total)

| # | Tool | Category |
|---|------|----------|
| 1 | run_applescript | Automation |
| 2 | get_screen_context | Context (now with AX tree) |
| 3 | click_element | NEW — AX-based click |
| 4 | activate_app | NEW — app launch |
| 5 | click_menu_item | NEW — menu navigation |
| 6 | click | Coordinate-based input |
| 7 | move_mouse | Coordinate-based input |
| 8 | type_text | Keyboard input |
| 9 | press_key | Keyboard input |
| 10 | scroll | Scroll |
| 11 | drag | Drag |
| 12 | recall_memory | Memory |
| 13 | shutdown_aura | System |

## Error Handling

- **Accessibility denied (click_element, AX tree):** Return error with `error_kind: "accessibility_denied"`. AX tree in get_screen_context degrades gracefully to empty list.
- **AX tree timeout/failure:** Return whatever elements collected within 500ms. Skip elements without bounds.
- **click_element no match:** Return error listing available alternatives for self-correction.
- **click_menu_item not found:** AppleScript error reported with menu item path.
- **activate_app not installed:** osascript error reported.
- **Automation denied (activate_app, click_menu_item):** Caught by existing preflight.

## Testing

**Unit tests:**
- AX FFI type conversions, element filtering, max cap, timeout
- Tool count = 13, new names present, parameter schemas valid
- AppleScript generation for activate_app and click_menu_item
- click_element error includes alternatives

**Integration tests (macOS + permissions):**
- click_element on Apple menu bar item
- activate_app("Finder")
- get_screen_context returns non-empty ui_elements
- click_menu_item(["Finder", "File", "New Finder Window"])

## Files Modified

| File | Change |
|------|--------|
| `aura-gemini/src/config.rs` | Rewrite Strategy section |
| `aura-gemini/src/tools.rs` | Add 3 tool declarations, update descriptions |
| `aura-screen/src/context.rs` | Add UIElement, ElementBounds, extend ScreenContext |
| `aura-screen/src/accessibility.rs` | NEW — AX tree walking via FFI |
| `aura-screen/src/macos.rs` | Call AX tree walker in capture_context() |
| `aura-screen/src/lib.rs` | Export new module |
| `aura-daemon/src/main.rs` | Add handlers for 3 new tools |
| `aura-screen/Cargo.toml` | Add core-foundation-sys dep |
