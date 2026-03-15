# New Tools Design: run_javascript, select_text, run_shell_command

**Date:** 2026-03-15
**Branch:** feat/oracle-first-targeting
**Goal:** Add three new tools to raise end-to-end control from 62% to ~75%, targeting the biggest gaps identified by expert validation.

## Motivation

Expert judges scored Aura's macOS control at:
- Mouse 66%, Keyboard 88%, AppleScript 72%, End-to-End 62%

The top three actionable improvements:
1. **Web interaction** — no DOM access means ~50% success on web tasks
2. **Text selection** — requires manual multi-step keyboard sequences
3. **System settings** — `do shell script` is blocked, so `defaults write` is unreachable

## Tool 1: `run_javascript`

### Purpose
Execute JavaScript in Safari or Chrome via AppleScript's `do JavaScript`. Provides a cleaner API than hand-writing AppleScript for web DOM interactions.

### Parameters
| Param | Type | Required | Default | Description |
|-------|------|----------|---------|-------------|
| `app` | string (enum: "Safari", "Chrome") | yes | — | Target browser |
| `code` | string | yes | — | JavaScript to execute in the active tab |
| `timeout_secs` | integer | no | 30 | Max execution time (max 120) |
| `verify` | boolean | no | false | Verify screen changed after execution |

### Implementation
Builds an AppleScript string internally:
- Safari: `tell application "Safari" to do JavaScript "<code>" in document 1`
- Chrome: `tell application "Google Chrome" to execute front window's active tab javascript "<code>"`

Routes through existing `ScriptExecutor` with `ScriptLanguage::AppleScript`. No bridge changes — `do JavaScript` is already allowed by the safety model. Pre-checks automation permission for the target browser.

### Safety
- JavaScript runs in the browser's sandbox, not the shell
- AppleScript safety checks still apply (JXA blocked, `do shell script` blocked)
- Quotes in JS code are escaped before embedding in AppleScript

## Tool 2: `select_text`

### Purpose
Intelligently select text using keyboard primitives. Composes existing `press_key`, `key_down`, `key_up`, and `click` functions.

### Parameters
| Param | Type | Required | Description |
|-------|------|----------|-------------|
| `method` | string (enum) | yes | Selection strategy |
| `x` | number | conditional | Image x-coordinate (required for word/line) |
| `y` | number | conditional | Image y-coordinate (required for word/line) |

### Methods
| Method | Action | Requires x,y? |
|--------|--------|---------------|
| `"all"` | Cmd+A | no |
| `"word"` | Double-click at (x,y) | yes |
| `"line"` | Triple-click at (x,y) | yes |
| `"to_start"` | Cmd+Shift+Up from cursor | no |
| `"to_end"` | Cmd+Shift+Down from cursor | no |

### Implementation
Pure composition of existing primitives — no new FFI code:
- `"all"` → `press_key(0, &["cmd"])` with keycode for 'a'
- `"word"` → `click(x, y, click_count=2)` via mouse::click
- `"line"` → `click(x, y, click_count=3)` via mouse::click
- `"to_start"` → `press_key(up_arrow, &["cmd", "shift"])`
- `"to_end"` → `press_key(down_arrow, &["cmd", "shift"])`

Uses PID+HID fallback on click paths. Oracle refinement for word/line methods (same pipeline as regular click). Settle delay: 50ms.

## Tool 3: `run_shell_command`

### Purpose
Execute allowlisted shell commands for system configuration that AppleScript can't reach.

### Parameters
| Param | Type | Required | Default | Description |
|-------|------|----------|---------|-------------|
| `command` | string | yes | — | Command name (must be in allowlist) |
| `args` | array of strings | yes | — | Command arguments |
| `timeout_secs` | integer | no | 15 | Max execution time (max 60) |
| `verify` | boolean | no | false | Verify screen changed after execution |

### Allowlist (5 commands)
| Command | Purpose | Example |
|---------|---------|---------|
| `defaults` | Read/write macOS preferences | `["write", "com.apple.dock", "autohide", "-bool", "true"]` |
| `open` | Open files, URLs, apps | `["-a", "Preview", "file.pdf"]` |
| `killall` | Terminate apps by name | `["Dock"]` |
| `say` | Text-to-speech | `["Hello world"]` |
| `launchctl` | Manage services | `["list"]` |

### Safety Model
**Allowlist enforcement:**
- Command name must exactly match one of the 5 allowed commands
- Command is resolved to absolute path (`/usr/bin/defaults`, etc.) — no PATH hijacking

**Blocked patterns (in any argument):**
- `sudo` as any argument
- Shell metacharacters: `|`, `;`, `` ` ``, `$(`, `>`, `<`, `&&`, `||`
- Null bytes

**Blocked `defaults` domains:**
- `com.apple.security` — security policies
- `com.apple.loginwindow` — login behavior
- `com.apple.screensaver` — password-related settings

**Execution:**
- Direct `std::process::Command::new(absolute_path)` with args as separate elements
- No shell interpretation (not `sh -c`)
- Output capped at 10KB
- Returns `{ success, stdout, stderr }`

## System Prompt Changes

1. Add `run_javascript` to the tools section with usage guidance
2. Replace buried `do JavaScript` example with `run_javascript` reference
3. Add to decision tree: "Web DOM interactions → `run_javascript`"
4. Add `select_text` to tool tips: "Use before copy operations"
5. Add `run_shell_command` to decision tree: "System settings → `run_shell_command` with `defaults write` + `killall`"

## Files Changed

| File | Changes |
|------|---------|
| `crates/aura-gemini/src/tools.rs` | +3 FunctionDeclarations (~120 lines) |
| `crates/aura-daemon/src/tools.rs` | +3 match arms (~180 lines) |
| `crates/aura-daemon/src/tool_helpers.rs` | +3 entries in settle_delay_for_tool |
| `crates/aura-gemini/src/config.rs` | System prompt updates (~20 lines) |
| Tests in tools.rs | Update counts (17→20), add name assertions |

**Total: ~350 lines across 4 files. No new crates, no new dependencies, no unsafe code.**
