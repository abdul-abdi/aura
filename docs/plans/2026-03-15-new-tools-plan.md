# New Tools Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Add three new tools (`run_javascript`, `select_text`, `run_shell_command`) to raise Aura's end-to-end macOS control from 62% to ~75%.

**Architecture:** Each tool follows the existing pattern: FunctionDeclaration in `aura-gemini/src/tools.rs`, match arm in `aura-daemon/src/tools.rs`, settle delay in `tool_helpers.rs`, and system prompt updates in `config.rs`. `run_javascript` wraps the existing AppleScript bridge. `select_text` composes existing keyboard/mouse primitives. `run_shell_command` uses direct `std::process::Command` with an allowlist.

**Tech Stack:** Rust, serde_json, tokio (spawn_blocking), CGEvent (via aura-input), AppleScript (via aura-bridge), std::process::Command.

---

### Task 1: Add `run_javascript` tool declaration

**Files:**
- Modify: `crates/aura-gemini/src/tools.rs:378` (insert before closing `]`)

**Step 1: Update test expectations first (RED)**

In `crates/aura-gemini/src/tools.rs`, update ALL test counts and name lists:

1. Line 401: change `17` to `20` in `tool_declarations_returns_two_tool_objects`
2. Line 447: change `17` to `20` in `tool_declarations_serialize_to_valid_json`
3. In `tool_names_are_correct` (line 411-430): add `"run_javascript"`, `"select_text"`, `"run_shell_command"` to the end of the names vec

**Step 2: Run tests to verify they fail**

Run: `cd /Users/abdullahiabdi/Developer/aura && cargo test -p aura-gemini -- tools`
Expected: FAIL — counts are 17 but tests expect 20, names vec doesn't match.

**Step 3: Add `run_javascript` FunctionDeclaration**

Insert BEFORE the closing `])` at line 378, after the `context_menu_click` declaration:

```rust
                FunctionDeclaration {
                    name: "run_javascript".into(),
                    description: "Execute JavaScript in Safari or Chrome's active tab. Returns the \
                        result of the last expression. Use for DOM queries, form filling, clicking \
                        web elements, reading page content, and any web interaction where coordinates \
                        are unreliable. \
                        Invoke this tool only after you have confirmed the user wants to interact \
                        with a web page and the target browser is open."
                        .into(),
                    parameters: json!({
                        "type": "object",
                        "properties": {
                            "app": {
                                "type": "string",
                                "enum": ["Safari", "Chrome"],
                                "description": "Target browser"
                            },
                            "code": {
                                "type": "string",
                                "description": "JavaScript code to execute in the active tab"
                            },
                            "timeout_secs": {
                                "type": "integer",
                                "description": "Max execution time in seconds. Default: 30"
                            },
                            "verify": {
                                "type": "boolean",
                                "description": "Whether to verify screen changed. Default: false (most JS is read-only)"
                            }
                        },
                        "required": ["app", "code"]
                    }),
                    behavior: Some("NON_BLOCKING".into()),
                },
```

**Step 4: Add `select_text` FunctionDeclaration**

Insert right after `run_javascript`:

```rust
                FunctionDeclaration {
                    name: "select_text".into(),
                    description: "Select text using the appropriate keyboard/mouse method. Use before \
                        copy operations. 'all' selects everything (Cmd+A), 'word' double-clicks at \
                        coordinates, 'line' triple-clicks at coordinates, 'to_start' selects from \
                        cursor to document start, 'to_end' selects from cursor to document end. \
                        Invoke this tool only after you know what text needs to be selected."
                        .into(),
                    parameters: json!({
                        "type": "object",
                        "properties": {
                            "method": {
                                "type": "string",
                                "enum": ["all", "word", "line", "to_start", "to_end"],
                                "description": "Selection strategy"
                            },
                            "x": {
                                "type": "number",
                                "description": "X coordinate for word/line selection (image pixels)"
                            },
                            "y": {
                                "type": "number",
                                "description": "Y coordinate for word/line selection (image pixels)"
                            }
                        },
                        "required": ["method"]
                    }),
                    behavior: Some("NON_BLOCKING".into()),
                },
```

**Step 5: Add `run_shell_command` FunctionDeclaration**

Insert right after `select_text`:

```rust
                FunctionDeclaration {
                    name: "run_shell_command".into(),
                    description: "Execute an allowlisted shell command for system configuration. \
                        Allowed commands: defaults (read/write macOS preferences), open (open files/URLs/apps), \
                        killall (terminate apps), say (text-to-speech), launchctl (manage services). \
                        Use defaults write + killall to apply system preference changes. \
                        Invoke this tool only after you have confirmed the user wants to change \
                        a system setting or perform an operation that requires shell access."
                        .into(),
                    parameters: json!({
                        "type": "object",
                        "properties": {
                            "command": {
                                "type": "string",
                                "enum": ["defaults", "open", "killall", "say", "launchctl"],
                                "description": "The command to run (must be in allowlist)"
                            },
                            "args": {
                                "type": "array",
                                "items": { "type": "string" },
                                "description": "Command arguments as separate strings"
                            },
                            "timeout_secs": {
                                "type": "integer",
                                "description": "Max execution time in seconds. Default: 15"
                            },
                            "verify": {
                                "type": "boolean",
                                "description": "Whether to verify screen changed. Default: false"
                            }
                        },
                        "required": ["command", "args"]
                    }),
                    behavior: Some("NON_BLOCKING".into()),
                },
```

**Step 6: Update doc comment**

Line 9: change `17` to `20` in the module doc comment.

**Step 7: Run tests to verify they pass**

Run: `cargo test -p aura-gemini -- tools`
Expected: ALL PASS (20 function declarations, names match, all NON_BLOCKING except shutdown_aura, all have "Invoke this tool only after").

**Step 8: Commit**

```bash
git add crates/aura-gemini/src/tools.rs
git commit -m "feat: add run_javascript, select_text, run_shell_command declarations"
```

---

### Task 2: Add `run_javascript` execution

**Files:**
- Modify: `crates/aura-daemon/src/tools.rs` (add match arm before `other =>` catch-all)

**Step 1: Add `run_javascript` match arm**

Insert before the `other =>` catch-all (line 1026 in daemon/tools.rs). This tool builds an AppleScript internally and routes through the existing executor:

```rust
        "run_javascript" => {
            let app = args.get("app").and_then(|v| v.as_str()).unwrap_or("Safari");
            let code = args.get("code").and_then(|v| v.as_str()).unwrap_or("");

            if code.is_empty() {
                return serde_json::json!({ "success": false, "error": "code parameter is required" });
            }

            // Resolve browser name for AppleScript targeting
            let (script_app, bundle_hint) = match app {
                "Chrome" => ("Google Chrome", "com.google.Chrome"),
                _ => ("Safari", "com.apple.Safari"),
            };

            // Pre-check Automation permission
            let bundle = bundle_hint.to_string();
            let perm = tokio::task::spawn_blocking(move || {
                aura_bridge::automation::check_automation_permission(&bundle)
            })
            .await
            .unwrap_or(aura_bridge::automation::AutomationPermission::Unknown(-1));
            if perm == aura_bridge::automation::AutomationPermission::Denied {
                return serde_json::json!({
                    "success": false,
                    "error": format!(
                        "Automation permission for {script_app} is denied. \
                         Grant it in System Settings > Privacy & Security > Automation."
                    ),
                    "error_kind": "automation_denied",
                });
            }

            // Escape single quotes in JS code for AppleScript embedding
            let escaped_code = code.replace('\\', "\\\\").replace('"', "\\\"");

            // Build the AppleScript that executes JS in the browser
            let script = if app == "Chrome" {
                format!(
                    "tell application \"Google Chrome\" to execute front window's active tab javascript \"{}\"",
                    escaped_code
                )
            } else {
                format!(
                    "tell application \"Safari\" to do JavaScript \"{}\" in document 1",
                    escaped_code
                )
            };

            let timeout = args
                .get("timeout_secs")
                .and_then(|v| v.as_u64())
                .unwrap_or(30)
                .min(MAX_APPLESCRIPT_TIMEOUT_SECS);
            let result = executor.run(&script, ScriptLanguage::AppleScript, timeout).await;

            if !result.success && is_automation_denied(&result.stderr) {
                return serde_json::json!({
                    "success": false,
                    "error": format!(
                        "Automation permission for {script_app} was denied. \
                         Grant it in System Settings > Privacy & Security > Automation."
                    ),
                    "error_kind": "automation_denied",
                    "stderr": result.stderr,
                });
            }

            serde_json::json!({
                "success": result.success,
                "result": result.stdout,
                "stderr": result.stderr,
            })
        }
```

**Step 2: Build to verify compilation**

Run: `cargo build -p aura-daemon 2>&1 | head -20`
Expected: Compiles without errors.

**Step 3: Commit**

```bash
git add crates/aura-daemon/src/tools.rs
git commit -m "feat: add run_javascript tool execution"
```

---

### Task 3: Add `select_text` execution

**Files:**
- Modify: `crates/aura-daemon/src/tools.rs` (add match arm after `run_javascript`)

**Step 1: Add `select_text` match arm**

Insert after the `run_javascript` match arm:

```rust
        "select_text" => {
            let method = args.get("method").and_then(|v| v.as_str()).unwrap_or("all");

            match method {
                "all" => {
                    // Cmd+A
                    let keycode = aura_input::keyboard::keycode_from_name("a").unwrap();
                    run_with_pid_fallback(
                        move |pid| aura_input::keyboard::press_key_pid(keycode, &["cmd"], pid),
                        "select_all_pid",
                        move || aura_input::keyboard::press_key(keycode, &["cmd"]),
                        "select_all_hid",
                    )
                    .await
                }
                "word" => {
                    let raw_x = args.get("x").and_then(|v| v.as_f64()).unwrap_or(0.0);
                    let raw_y = args.get("y").and_then(|v| v.as_f64()).unwrap_or(0.0);
                    let lx = dims.to_logical_x(raw_x);
                    let ly = dims.to_logical_y(raw_y);
                    // Double-click to select word
                    run_with_pid_fallback(
                        move |pid| aura_input::mouse::click_pid(lx, ly, "left", 2, &[], pid),
                        "word_select_pid",
                        move || aura_input::mouse::click(lx, ly, "left", 2, &[]),
                        "word_select_hid",
                    )
                    .await
                }
                "line" => {
                    let raw_x = args.get("x").and_then(|v| v.as_f64()).unwrap_or(0.0);
                    let raw_y = args.get("y").and_then(|v| v.as_f64()).unwrap_or(0.0);
                    let lx = dims.to_logical_x(raw_x);
                    let ly = dims.to_logical_y(raw_y);
                    // Triple-click to select line
                    run_with_pid_fallback(
                        move |pid| aura_input::mouse::click_pid(lx, ly, "left", 3, &[], pid),
                        "line_select_pid",
                        move || aura_input::mouse::click(lx, ly, "left", 3, &[]),
                        "line_select_hid",
                    )
                    .await
                }
                "to_start" => {
                    // Cmd+Shift+Up to select from cursor to document start
                    let keycode = aura_input::keyboard::keycode_from_name("up").unwrap();
                    run_with_pid_fallback(
                        move |pid| {
                            aura_input::keyboard::press_key_pid(
                                keycode,
                                &["cmd", "shift"],
                                pid,
                            )
                        },
                        "select_to_start_pid",
                        move || aura_input::keyboard::press_key(keycode, &["cmd", "shift"]),
                        "select_to_start_hid",
                    )
                    .await
                }
                "to_end" => {
                    // Cmd+Shift+Down to select from cursor to document end
                    let keycode = aura_input::keyboard::keycode_from_name("down").unwrap();
                    run_with_pid_fallback(
                        move |pid| {
                            aura_input::keyboard::press_key_pid(
                                keycode,
                                &["cmd", "shift"],
                                pid,
                            )
                        },
                        "select_to_end_pid",
                        move || aura_input::keyboard::press_key(keycode, &["cmd", "shift"]),
                        "select_to_end_hid",
                    )
                    .await
                }
                other => {
                    serde_json::json!({
                        "success": false,
                        "error": format!("Unknown select_text method: {other}. Use: all, word, line, to_start, to_end"),
                    })
                }
            }
        }
```

**Step 2: Build to verify compilation**

Run: `cargo build -p aura-daemon 2>&1 | head -20`
Expected: Compiles without errors.

**Step 3: Commit**

```bash
git add crates/aura-daemon/src/tools.rs
git commit -m "feat: add select_text tool execution"
```

---

### Task 4: Add `run_shell_command` execution

**Files:**
- Modify: `crates/aura-daemon/src/tools.rs` (add match arm, constants, and validation)

**Step 1: Add constants at the top of the file**

Insert after line 28 (`const MAX_APPLESCRIPT_TIMEOUT_SECS: u64 = 120;`):

```rust
/// Maximum timeout for shell commands.
const MAX_SHELL_TIMEOUT_SECS: u64 = 60;

/// Maximum output size from shell commands.
const MAX_SHELL_OUTPUT_BYTES: usize = 10_240;

/// Allowlisted shell commands with their absolute paths.
const ALLOWED_SHELL_COMMANDS: &[(&str, &str)] = &[
    ("defaults", "/usr/bin/defaults"),
    ("open", "/usr/bin/open"),
    ("killall", "/usr/bin/killall"),
    ("say", "/usr/bin/say"),
    ("launchctl", "/bin/launchctl"),
];

/// Blocked `defaults` domains that could compromise system security.
const BLOCKED_DEFAULTS_DOMAINS: &[&str] = &[
    "com.apple.security",
    "com.apple.loginwindow",
    "com.apple.screensaver",
];

/// Shell metacharacters that must not appear in any argument (injection prevention).
const SHELL_METACHARACTERS: &[&str] = &["|", ";", "`", "$(", ">", "<", "&&", "||"];
```

**Step 2: Add `run_shell_command` match arm**

Insert after the `select_text` match arm:

```rust
        "run_shell_command" => {
            let command = args.get("command").and_then(|v| v.as_str()).unwrap_or("");
            let cmd_args: Vec<String> = args
                .get("args")
                .and_then(|v| v.as_array())
                .map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect())
                .unwrap_or_default();

            // Validate command is in allowlist
            let abs_path = match ALLOWED_SHELL_COMMANDS.iter().find(|(name, _)| *name == command) {
                Some((_, path)) => *path,
                None => {
                    return serde_json::json!({
                        "success": false,
                        "error": format!(
                            "Command '{}' is not allowed. Allowed: {}",
                            command,
                            ALLOWED_SHELL_COMMANDS.iter().map(|(n, _)| *n).collect::<Vec<_>>().join(", ")
                        ),
                    });
                }
            };

            // Block sudo in any argument
            if cmd_args.iter().any(|a| a == "sudo") {
                return serde_json::json!({
                    "success": false,
                    "error": "sudo is not allowed in shell commands",
                });
            }

            // Block shell metacharacters in any argument
            for arg in &cmd_args {
                if let Some(meta) = SHELL_METACHARACTERS.iter().find(|m| arg.contains(*m)) {
                    return serde_json::json!({
                        "success": false,
                        "error": format!("Shell metacharacter '{}' is not allowed in arguments", meta),
                    });
                }
                if arg.contains('\0') {
                    return serde_json::json!({
                        "success": false,
                        "error": "Null bytes are not allowed in arguments",
                    });
                }
            }

            // Block dangerous defaults domains
            if command == "defaults" && cmd_args.len() >= 2 {
                let subcommand = cmd_args[0].as_str();
                let domain = &cmd_args[1];
                if subcommand == "write" || subcommand == "delete" {
                    if let Some(blocked) = BLOCKED_DEFAULTS_DOMAINS
                        .iter()
                        .find(|d| domain.starts_with(*d))
                    {
                        return serde_json::json!({
                            "success": false,
                            "error": format!("defaults domain '{}' is blocked for security", blocked),
                        });
                    }
                }
            }

            let timeout = args
                .get("timeout_secs")
                .and_then(|v| v.as_u64())
                .unwrap_or(15)
                .min(MAX_SHELL_TIMEOUT_SECS);

            let abs_path_owned = abs_path.to_string();
            let result = tokio::task::spawn_blocking(move || {
                let output = std::process::Command::new(&abs_path_owned)
                    .args(&cmd_args)
                    .stdout(std::process::Stdio::piped())
                    .stderr(std::process::Stdio::piped())
                    .output();

                match output {
                    Ok(out) => {
                        let mut stdout = String::from_utf8_lossy(&out.stdout).to_string();
                        let mut stderr = String::from_utf8_lossy(&out.stderr).to_string();
                        stdout.truncate(MAX_SHELL_OUTPUT_BYTES);
                        stderr.truncate(MAX_SHELL_OUTPUT_BYTES);
                        serde_json::json!({
                            "success": out.status.success(),
                            "exit_code": out.status.code(),
                            "stdout": stdout,
                            "stderr": stderr,
                        })
                    }
                    Err(e) => serde_json::json!({
                        "success": false,
                        "error": format!("Failed to execute command: {e}"),
                    }),
                }
            })
            .await
            .unwrap_or_else(|e| serde_json::json!({
                "success": false,
                "error": format!("Task panicked: {e}"),
            }));

            result
        }
```

Note: The timeout is not enforced with a `tokio::time::timeout` wrapper because `std::process::Command::output()` blocks. For a future improvement, consider using `tokio::process::Command` for async timeout support. For now the 60s max is sufficient since these are fast commands.

**Step 3: Build to verify compilation**

Run: `cargo build -p aura-daemon 2>&1 | head -20`
Expected: Compiles without errors.

**Step 4: Commit**

```bash
git add crates/aura-daemon/src/tools.rs
git commit -m "feat: add run_shell_command tool execution with allowlist"
```

---

### Task 5: Update tool_helpers.rs — settle delays and state-changing registration

**Files:**
- Modify: `crates/aura-daemon/src/tool_helpers.rs:32-41` (settle_delay_for_tool)
- Modify: `crates/aura-daemon/src/tool_helpers.rs:373-390` (is_state_changing_tool)

**Step 1: Update `settle_delay_for_tool`**

In the `settle_delay_for_tool` match, add the three new tools. Replace line 34-39:

```rust
    match name {
        "type_text" | "press_key" | "move_mouse" | "select_text" => Duration::from_millis(30),
        "scroll" | "drag" => Duration::from_millis(50),
        "click" | "click_element" | "context_menu_click" => Duration::from_millis(100),
        "activate_app" | "click_menu_item" => Duration::from_millis(150),
        "run_applescript" | "run_javascript" | "run_shell_command" => Duration::from_millis(200),
        _ => Duration::from_millis(150), // conservative default
    }
```

**Step 2: Update `is_state_changing_tool`**

Add `"run_javascript"`, `"select_text"`, and `"run_shell_command"` to the matches! macro:

```rust
    matches!(
        name,
        "click"
            | "type_text"
            | "press_key"
            | "key_state"
            | "scroll"
            | "drag"
            | "click_element"
            | "activate_app"
            | "click_menu_item"
            | "context_menu_click"
            | "run_applescript"
            | "run_javascript"
            | "select_text"
            | "run_shell_command"
    )
```

**Step 3: Update the settle delay test**

In the `test_adaptive_settle_delay` test, add assertions:

```rust
    assert_eq!(
        settle_delay_for_tool("select_text"),
        Duration::from_millis(30)
    );
    assert_eq!(
        settle_delay_for_tool("run_javascript"),
        Duration::from_millis(200)
    );
    assert_eq!(
        settle_delay_for_tool("run_shell_command"),
        Duration::from_millis(200)
    );
```

**Step 4: Add `is_state_changing_tool` assertions for new tools**

After the existing `run_applescript_is_state_changing` test:

```rust
    #[test]
    fn new_tools_are_state_changing() {
        assert!(is_state_changing_tool("run_javascript"));
        assert!(is_state_changing_tool("select_text"));
        assert!(is_state_changing_tool("run_shell_command"));
    }
```

**Step 5: Run tests**

Run: `cargo test -p aura-daemon -- tool_helpers`
Expected: ALL PASS.

**Step 6: Commit**

```bash
git add crates/aura-daemon/src/tool_helpers.rs
git commit -m "feat: register new tools in settle delay and state-changing maps"
```

---

### Task 6: Update processor.rs tool dispatch

**Files:**
- Modify: `crates/aura-daemon/src/processor.rs` (update the tool name match for input tools)

**Step 1: Find and update the input tool list**

The processor has a match at line ~160 that lists tools needing display_origin offset. Search for the pattern that lists `"move_mouse" | "click" | ...` and add the new tools.

Find the line (around line 160):
```rust
        "move_mouse" | "click" | "type_text" | "press_key" | "scroll" | "drag" | "key_state"
```

This is in `crates/aura-daemon/src/tools.rs` line 160, NOT processor.rs. This line determines which tools get display_origin_x/y offset. `select_text` needs it (for word/line methods with coordinates). `run_javascript` and `run_shell_command` do NOT need it.

Update to:
```rust
        "move_mouse" | "click" | "type_text" | "press_key" | "scroll" | "drag" | "key_state" | "select_text"
```

**Step 2: Build to verify**

Run: `cargo build -p aura-daemon 2>&1 | head -20`
Expected: Compiles without errors.

**Step 3: Commit**

```bash
git add crates/aura-daemon/src/tools.rs
git commit -m "feat: add select_text to display-origin-aware tool list"
```

---

### Task 7: Update system prompt

**Files:**
- Modify: `crates/aura-gemini/src/config.rs:36-56` (tools section)
- Modify: `crates/aura-gemini/src/config.rs:58-84` (strategy section)
- Modify: `crates/aura-gemini/src/config.rs:142-174` (tool_tips section)
- Modify: `crates/aura-gemini/src/config.rs:176-194` (workflows section)

**Step 1: Add new tools to the `<tools>` section**

After line 51 (`- get_screen_context(): ...`), add:

```
- run_javascript(app, code, timeout_secs?, verify?): Execute JavaScript in Safari or Chrome's active tab. Returns the JS expression result. Set verify=false for read-only DOM queries.
- select_text(method, x?, y?): Select text. Methods: 'all' (Cmd+A), 'word' (double-click at x,y), 'line' (triple-click at x,y), 'to_start' (select to document start), 'to_end' (select to document end).
- run_shell_command(command, args, timeout_secs?, verify?): Execute an allowlisted shell command. Commands: defaults, open, killall, say, launchctl.
```

**Step 2: Update the `<strategy>` section decision flow**

After line 82 (`- Need scripting with no visual equivalent? → run_applescript`), add:

```
- Web page DOM interaction? → run_javascript(app="Safari", code="...")
- System preferences (Dock, Finder, etc.)? → run_shell_command("defaults", ["write", ...]) + run_shell_command("killall", ["Dock"])
- Need to select text before copying? → select_text
```

**Step 3: Add tool tips for new tools**

After line 173 (end of `get_screen_context` tip), add:

```
run_javascript: Use for web interactions that are hard to click — form fills, DOM queries, scroll-to-element. Returns the last expression's value as a string. Example: run_javascript(app="Safari", code="document.title") returns the page title. For mutations (clicking buttons, filling forms), set verify=true.

select_text: Use before Cmd+C to copy. 'all' for entire field/document, 'word' to double-click a word, 'line' to triple-click a line. 'to_start'/'to_end' extend selection from current cursor. For word/line, provide x,y coordinates from the screenshot.

run_shell_command: For system preferences not accessible via UI. Common pattern: run_shell_command("defaults", ["write", "com.apple.dock", "autohide", "-bool", "true"]) then run_shell_command("killall", ["Dock"]) to apply. Use run_shell_command("defaults", ["read", "com.apple.dock"]) to check current settings first.
```

**Step 4: Update the `<workflows>` section**

Replace the `Web page interaction` workflow (lines 191-193) with:

```
Web page interaction: run_javascript(app="Safari", code="document.querySelector('#submit-btn').click()") — precise DOM targeting, no coordinate guessing. For reading: run_javascript(app="Safari", code="document.title", verify=false).

Select and copy: select_text(method="all") → press_key("c", modifiers=["cmd"]). Or for a specific word: select_text(method="word", x=500, y=300) → press_key("c", modifiers=["cmd"]).

System preferences: run_shell_command("defaults", ["write", "com.apple.dock", "autohide", "-bool", "true"]) → run_shell_command("killall", ["Dock"])
```

**Step 5: Build to verify**

Run: `cargo build -p aura-gemini 2>&1 | head -20`
Expected: Compiles without errors.

**Step 6: Commit**

```bash
git add crates/aura-gemini/src/config.rs
git commit -m "feat: update system prompt with new tool guidance"
```

---

### Task 8: Full build and test

**Files:** None (verification only)

**Step 1: Run full test suite**

Run: `cargo test --workspace 2>&1 | tail -30`
Expected: ALL PASS.

**Step 2: Run clippy**

Run: `cargo clippy --workspace 2>&1 | tail -20`
Expected: No errors (warnings acceptable).

**Step 3: Verify tool count consistency**

Run: `cargo test -p aura-gemini -- tools 2>&1`
Expected: All 8 tool tests pass with 20 function declarations.

**Step 4: Commit any formatting fixes**

```bash
cargo fmt --all
git add -u
git commit -m "chore: format" || echo "Nothing to format"
```
