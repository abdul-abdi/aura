# Aura Tool Reference

Aura declares 9 function tools to the Gemini Live API, plus two server-side capabilities (Google Search grounding and Code Execution). When Gemini decides to act, it calls one of these tools; the result is sent back so it can observe the outcome.

All tool declarations live in [`crates/aura-gemini/src/tools.rs`](../crates/aura-gemini/src/tools.rs). Handler dispatch is in [`crates/aura-daemon/src/main.rs`](../crates/aura-daemon/src/main.rs).

---

## Table of Contents

| # | Tool | Category |
|---|------|----------|
| 1 | [run_applescript](#1-run_applescript) | Automation |
| 2 | [get_screen_context](#2-get_screen_context) | Observation |
| 3 | [shutdown_aura](#3-shutdown_aura) | Lifecycle |
| 4 | [move_mouse](#4-move_mouse) | Input |
| 5 | [click](#5-click) | Input |
| 6 | [type_text](#6-type_text) | Input |
| 7 | [press_key](#7-press_key) | Input |
| 8 | [scroll](#8-scroll) | Input |
| 9 | [drag](#9-drag) | Input |
| -- | [Google Search](#google-search-grounding) | Server-side |
| -- | [Code Execution](#code-execution) | Server-side |

---

## 1. `run_applescript`

Execute AppleScript or JXA code to control any macOS application or system feature.

### Parameters

| Name | Type | Required | Default | Description |
|------|------|----------|---------|-------------|
| `script` | string | yes | -- | The AppleScript or JXA code to execute |
| `language` | string | no | `"applescript"` | `"applescript"` or `"javascript"` (JXA) |
| `timeout_secs` | integer | no | `30` | Max execution time in seconds (clamped to 60) |

### Example Call

```json
{
  "name": "run_applescript",
  "args": {
    "script": "tell application \"Safari\" to get URL of current tab of front window",
    "language": "applescript",
    "timeout_secs": 10
  }
}
```

### Response

```json
{
  "success": true,
  "stdout": "https://example.com",
  "stderr": ""
}
```

### Notes

- Executed via `osascript` in a `sandbox-exec` restrictive profile.
- Output is truncated to 10 KB (`MAX_OUTPUT_BYTES`).
- Timeout is enforced with `Child::kill()` -- the process is actually terminated, not just abandoned.
- **Dangerous pattern blocklist** -- scripts are checked against `BLOCKED_SHELL_PATTERNS` (e.g. `rm -rf`, `sudo`, `mkfs`, `dd if=`, `chmod 777`, `diskutil erase`) and `BLOCKED_JXA_PATTERNS` (e.g. `$.system`, `ObjC.import`, `.doScript(`). Blocked scripts return an error without executing.
- **Obfuscation detection** -- fragmented dangerous commands split across string concatenation or variables are caught (e.g. `set a to "rm"` + `set b to " -rf"`).
- **Automation permission preflight** -- before executing, the handler checks whether Aura has Automation access to the target app. If denied, it returns an `automation_denied` error immediately instead of running the script.

---

## 2. `get_screen_context`

Query the user's current screen state: frontmost application, window title, list of open windows, and clipboard contents.

### Parameters

None.

### Example Call

```json
{
  "name": "get_screen_context",
  "args": {}
}
```

### Response

```json
{
  "success": true,
  "context": "Frontmost app: Safari\nWindow title: GitHub - aura\nOpen windows: Safari, Terminal, Finder\nClipboard: some copied text"
}
```

### Notes

- Uses `osascript` for frontmost app/title, `CGWindowListCopyWindowInfo` for open windows, and `pbpaste` for clipboard.
- No Automation permission needed (reads system-level info only).
- Gemini is instructed to call this before taking any action so it understands the user's current context.

---

## 3. `shutdown_aura`

Gracefully shut down and quit Aura.

### Parameters

None.

### Example Call

```json
{
  "name": "shutdown_aura",
  "args": {}
}
```

### Notes

- Triggers the daemon shutdown sequence (cancellation token, WebSocket close, menu bar teardown).
- Gemini is instructed to say goodbye before calling this tool.

---

## 4. `move_mouse`

Move the mouse cursor to the specified screen coordinates.

### Parameters

| Name | Type | Required | Default | Description |
|------|------|----------|---------|-------------|
| `x` | number | yes | -- | X coordinate (pixels from left) |
| `y` | number | yes | -- | Y coordinate (pixels from top) |

### Example Call

```json
{
  "name": "move_mouse",
  "args": { "x": 500, "y": 300 }
}
```

### Response

```json
{ "success": true }
```

### Notes

- Requires **Accessibility** permission. Without it, `CGEvent.post()` silently drops events -- the handler checks permission before executing and returns an `accessibility_denied` error if not granted.
- Coordinates are converted from Gemini's raw values to logical (Retina-aware) coordinates via `FrameDims`.
- Uses `CGEvent` (Core Graphics synthetic events).

---

## 5. `click`

Click at the specified screen coordinates.

### Parameters

| Name | Type | Required | Default | Description |
|------|------|----------|---------|-------------|
| `x` | number | yes | -- | X coordinate |
| `y` | number | yes | -- | Y coordinate |
| `button` | string | no | `"left"` | `"left"` or `"right"` |
| `click_count` | integer | no | `1` | Number of clicks: 1 (single), 2 (double), 3 (triple). Clamped to 1..=3. |

### Example Call

```json
{
  "name": "click",
  "args": { "x": 200, "y": 150, "button": "right" }
}
```

```json
{
  "name": "click",
  "args": { "x": 200, "y": 150, "click_count": 2 }
}
```

### Notes

- Requires **Accessibility** permission.
- `click_count` is clamped to the range 1..=3 (`CLICK_COUNT_MAX`).

---

## 6. `type_text`

Type a string of text at the current cursor position.

### Parameters

| Name | Type | Required | Default | Description |
|------|------|----------|---------|-------------|
| `text` | string | yes | -- | The text to type (max 10,000 characters) |

### Example Call

```json
{
  "name": "type_text",
  "args": { "text": "Hello, world!" }
}
```

### Notes

- Requires **Accessibility** permission.
- Input is truncated to 10,000 characters (`TYPE_TEXT_MAX_CHARS`), using char-aware truncation to avoid splitting multi-byte UTF-8.
- Useful for entering text in fields, search bars, editors, and similar.

---

## 7. `press_key`

Press a key with optional modifier keys. Used for keyboard shortcuts and special keys.

### Parameters

| Name | Type | Required | Default | Description |
|------|------|----------|---------|-------------|
| `key` | string | yes | -- | Key name (see below) |
| `modifiers` | array of strings | no | `[]` | Modifier keys: `"cmd"`, `"shift"`, `"alt"`, `"ctrl"` |

### Supported Key Names

- **Letters**: `a`-`z`
- **Special**: `return`, `escape`, `tab`, `space`, `delete`
- **Arrows**: `up`, `down`, `left`, `right`
- **Function**: `f1`-`f12`

### Example Call

```json
{
  "name": "press_key",
  "args": { "key": "c", "modifiers": ["cmd"] }
}
```

```json
{
  "name": "press_key",
  "args": { "key": "return" }
}
```

### Notes

- Requires **Accessibility** permission.
- Unknown key names return `{ "success": false, "error": "Unknown key: ..." }`.
- Key name resolution happens via `keycode_from_name()` in `aura-input`.

---

## 8. `scroll`

Scroll the view horizontally and/or vertically.

### Parameters

| Name | Type | Required | Default | Description |
|------|------|----------|---------|-------------|
| `dx` | integer | no | `0` | Horizontal scroll (positive = right, negative = left) |
| `dy` | integer | yes | -- | Vertical scroll (positive = down, negative = up) |

### Example Call

```json
{
  "name": "scroll",
  "args": { "dy": -200 }
}
```

### Notes

- Requires **Accessibility** permission.
- Both `dx` and `dy` are clamped to the range -1000..=1000 (`SCROLL_MAX`).

---

## 9. `drag`

Click and drag from one point to another. Used for moving windows, selecting text, dragging files, etc.

### Parameters

| Name | Type | Required | Default | Description |
|------|------|----------|---------|-------------|
| `from_x` | number | yes | -- | Start X coordinate |
| `from_y` | number | yes | -- | Start Y coordinate |
| `to_x` | number | yes | -- | End X coordinate |
| `to_y` | number | yes | -- | End Y coordinate |

### Example Call

```json
{
  "name": "drag",
  "args": { "from_x": 100, "from_y": 200, "to_x": 400, "to_y": 200 }
}
```

### Notes

- Requires **Accessibility** permission.
- Coordinates are converted to logical (Retina-aware) values.

---

## Server-Side Tools

These are not function calls dispatched by Aura. They are capabilities enabled in the Gemini session setup that Gemini executes on Google's servers.

### Google Search Grounding

Lets Gemini answer questions about current events, weather, facts, and other information that requires up-to-date web data. Declared as `googleSearch: {}` in the tool list.

### Code Execution

Lets Gemini run Python code in a server-side sandbox for calculations, data analysis, and similar tasks. Declared as `codeExecution: {}` in the tool list.

---

## Security Gates

All tools pass through validation before execution:

| Gate | Applies To | Behavior |
|------|-----------|----------|
| Accessibility permission check | `move_mouse`, `click`, `type_text`, `press_key`, `scroll`, `drag` | Returns `accessibility_denied` error if not granted. Checked via `AXIsProcessTrustedWithOptions`. |
| Dangerous pattern blocklist | `run_applescript` | Blocks scripts containing dangerous shell commands (`rm -rf`, `sudo`, `mkfs`, etc.) or JXA escape hatches (`$.system`, `ObjC.import`, `.doScript(`). |
| Obfuscation detection | `run_applescript` | Catches dangerous commands split across string concatenation or variable assignments. |
| Automation permission preflight | `run_applescript` | Checks if Aura has Automation access to the target app before running the script. |
| Input clamping | `click` (`click_count`), `scroll` (`dx`, `dy`), `type_text` (`text` length) | Values are clamped to safe ranges to prevent abuse. |
| Output truncation | `run_applescript` | stdout/stderr capped at 10 KB. |
| Timeout enforcement | `run_applescript` | Process killed via `Child::kill()` after timeout (max 60s). |
| Destructive action guardrail | All tools (via system prompt) | Gemini is instructed to confirm with the user before any destructive action (deleting files, emptying trash, etc.). |
