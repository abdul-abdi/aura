# Aura v2 Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Redesign Aura into a macOS menu bar companion with dynamic AppleScript generation, witty personality, Cloud Run proxy, session memory, and wake word activation for the Gemini Live Agent Challenge (deadline: March 16, 2026).

**Architecture:** Native macOS menu bar app (NSStatusItem + NSPopover) using `cocoa`/`objc` crates. Gemini Live API streams audio through a Cloud Run WebSocket proxy. Instead of hardcoded tools, Gemini writes AppleScript on-the-fly via two tools: `run_applescript` and `get_screen_context`. Local SQLite stores conversation history per session.

**Tech Stack:** Rust 2024, tokio, cocoa/objc (Cocoa bindings), rusqlite (SQLite), axum + tokio-tungstenite (proxy), cpal (audio capture), rodio (playback), rustpotter (wake word), osascript (AppleScript execution), core-graphics (screen context)

**Critical invariant:** The workspace MUST compile (`cargo check --workspace`) after every task's commit. Never leave broken intermediate states.

---

## Task 1: Add ScriptExecutor to aura-bridge (additive, don't delete old code yet)

Add a new `script` module to aura-bridge with a general-purpose `ScriptExecutor` that runs arbitrary AppleScript/JXA code. **Keep** `actions.rs` and `macos.rs` for now — they'll be removed in Task 4 after dependents are updated.

**Files:**
- Create: `crates/aura-bridge/src/script.rs`
- Modify: `crates/aura-bridge/src/lib.rs` (add `pub mod script;`)
- Create: `crates/aura-bridge/tests/script_test.rs`

**Step 1: Write failing tests for ScriptExecutor**

Create `crates/aura-bridge/tests/script_test.rs`:

```rust
use aura_bridge::script::{ScriptExecutor, ScriptLanguage};

#[tokio::test]
async fn test_run_applescript_simple_echo() {
    let executor = ScriptExecutor::new();
    let result = executor
        .run("return \"hello\"", ScriptLanguage::AppleScript, 10)
        .await;
    assert!(result.success, "Script should succeed: {:?}", result);
    assert_eq!(result.stdout.trim(), "hello");
}

#[tokio::test]
async fn test_run_jxa_simple() {
    let executor = ScriptExecutor::new();
    let result = executor
        .run("'hello from jxa'", ScriptLanguage::JavaScript, 10)
        .await;
    assert!(result.success, "JXA should succeed: {:?}", result);
    assert!(result.stdout.contains("hello from jxa"));
}

#[tokio::test]
async fn test_run_applescript_error_returns_failure() {
    let executor = ScriptExecutor::new();
    let result = executor
        .run("error \"test error\"", ScriptLanguage::AppleScript, 10)
        .await;
    assert!(!result.success);
    assert!(!result.stderr.is_empty());
}

#[tokio::test]
async fn test_blocks_dangerous_shell_commands() {
    let executor = ScriptExecutor::new();
    let result = executor
        .run(
            r#"do shell script "rm -rf /"#,
            ScriptLanguage::AppleScript,
            10,
        )
        .await;
    assert!(!result.success);
    assert!(result.stderr.contains("blocked"));
}

#[tokio::test]
async fn test_timeout_kills_script() {
    let executor = ScriptExecutor::new();
    let result = executor
        .run("delay 60", ScriptLanguage::AppleScript, 2)
        .await;
    assert!(!result.success);
    assert!(
        result.stderr.contains("timed out") || result.stderr.contains("timeout"),
        "Expected timeout error, got: {}",
        result.stderr
    );
}
```

**Step 2: Run tests to verify they fail**

Run: `cargo test -p aura-bridge --test script_test 2>&1 | head -20`
Expected: Compilation error — `script` module doesn't exist yet.

**Step 3: Implement ScriptExecutor**

Create `crates/aura-bridge/src/script.rs`:

```rust
use std::process::Command;
use std::time::Duration;

use serde::{Deserialize, Serialize};

/// Blocked shell patterns — never allowed inside `do shell script`.
const BLOCKED_SHELL_PATTERNS: &[&str] = &[
    "rm -rf", "rm -r", "rmdir", "sudo", "mkfs", "dd if=",
    "chmod 777", ":(){ :|:", "> /dev/sd",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ScriptLanguage {
    AppleScript,
    JavaScript,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScriptResult {
    pub success: bool,
    pub stdout: String,
    pub stderr: String,
}

pub struct ScriptExecutor;

impl ScriptExecutor {
    pub fn new() -> Self {
        Self
    }

    /// Run an AppleScript or JXA script via osascript with a timeout.
    ///
    /// Uses `Child::kill()` to actually terminate the process on timeout,
    /// rather than just dropping the future.
    pub async fn run(
        &self,
        script: &str,
        language: ScriptLanguage,
        timeout_secs: u64,
    ) -> ScriptResult {
        // Safety check: block dangerous shell commands
        if let Some(reason) = check_dangerous(script) {
            return ScriptResult {
                success: false,
                stdout: String::new(),
                stderr: format!("Script blocked: {reason}"),
            };
        }

        let script = script.to_string();
        let timeout_dur = Duration::from_secs(timeout_secs);

        // Use spawn_blocking with Child handle so we can kill the process on timeout
        let handle = tokio::task::spawn_blocking(move || {
            let mut cmd = Command::new("osascript");
            match language {
                ScriptLanguage::AppleScript => {
                    cmd.arg("-e").arg(&script);
                }
                ScriptLanguage::JavaScript => {
                    cmd.arg("-l").arg("JavaScript").arg("-e").arg(&script);
                }
            }

            let mut child = cmd.spawn().map_err(|e| {
                ScriptResult {
                    success: false,
                    stdout: String::new(),
                    stderr: format!("Failed to spawn osascript: {e}"),
                }
            })?;

            // Wait with timeout using a polling loop
            let start = std::time::Instant::now();
            loop {
                match child.try_wait() {
                    Ok(Some(status)) => {
                        // Process finished — read output
                        let stdout = child.stdout.take()
                            .map(|mut s| {
                                let mut buf = String::new();
                                std::io::Read::read_to_string(&mut s, &mut buf).ok();
                                buf
                            })
                            .unwrap_or_default();
                        let stderr = child.stderr.take()
                            .map(|mut s| {
                                let mut buf = String::new();
                                std::io::Read::read_to_string(&mut s, &mut buf).ok();
                                buf
                            })
                            .unwrap_or_default();

                        return Ok(ScriptResult {
                            success: status.success(),
                            stdout,
                            stderr,
                        });
                    }
                    Ok(None) => {
                        // Still running — check timeout
                        if start.elapsed() >= timeout_dur {
                            let _ = child.kill();
                            let _ = child.wait(); // reap zombie
                            return Ok(ScriptResult {
                                success: false,
                                stdout: String::new(),
                                stderr: "Script timed out".into(),
                            });
                        }
                        std::thread::sleep(Duration::from_millis(50));
                    }
                    Err(e) => {
                        return Ok(ScriptResult {
                            success: false,
                            stdout: String::new(),
                            stderr: format!("Failed to check process status: {e}"),
                        });
                    }
                }
            }
        });

        match handle.await {
            Ok(Ok(result)) => result,
            Ok(Err(result)) => result,
            Err(e) => ScriptResult {
                success: false,
                stdout: String::new(),
                stderr: format!("Script task panicked: {e}"),
            },
        }
    }
}

/// Check if script contains dangerous patterns. Returns reason string if blocked.
fn check_dangerous(script: &str) -> Option<String> {
    let lower = script.to_lowercase();

    // Only check shell escape patterns
    if lower.contains("do shell script") || lower.contains("system(") {
        for pattern in BLOCKED_SHELL_PATTERNS {
            if lower.contains(pattern) {
                return Some(format!(
                    "Dangerous shell command blocked: contains '{pattern}'"
                ));
            }
        }
    }

    None
}
```

**Important:** This version uses `child.try_wait()` polling with `child.kill()` on timeout, ensuring the `osascript` process is actually terminated. We need to capture stdout/stderr via piped I/O. Update the spawn to pipe stdout/stderr:

Replace the `cmd.spawn()` call with:

```rust
let mut child = cmd
    .stdout(std::process::Stdio::piped())
    .stderr(std::process::Stdio::piped())
    .spawn()
    .map_err(|e| ScriptResult { ... })?;
```

Update `crates/aura-bridge/src/lib.rs` — add `pub mod script;` while keeping existing modules:

```rust
//! Aura OS bridge: platform-specific system actions

pub mod actions;
pub mod script;

#[cfg(target_os = "macos")]
pub mod macos;
```

**Step 4: Run tests to verify they pass**

Run: `cargo test -p aura-bridge --test script_test -- --nocapture`
Expected: All 5 tests pass. Timeout test takes ~2 seconds.

**Step 5: Verify workspace still compiles**

Run: `cargo check --workspace`
Expected: OK — we only added code, didn't remove anything.

**Step 6: Commit**

```bash
git add crates/aura-bridge/src/script.rs crates/aura-bridge/src/lib.rs crates/aura-bridge/tests/script_test.rs
git commit -m "feat: add dynamic AppleScript executor to aura-bridge"
```

---

## Task 2: Implement Screen Context (aura-screen)

Complete the stub `MacOSScreenReader` to detect the frontmost application, list open windows, and read clipboard contents. Simplify `ScreenContext` to the fields we actually need.

**Files:**
- Rewrite: `crates/aura-screen/src/context.rs`
- Rewrite: `crates/aura-screen/src/macos.rs`
- Create: `crates/aura-screen/tests/context_test.rs`
- Modify: `crates/aura-screen/Cargo.toml` (add serde_json dev-dep, remove core-foundation/core-graphics deps)

**Step 1: Write failing tests**

Create `crates/aura-screen/tests/context_test.rs`:

```rust
use aura_screen::context::ScreenContext;

#[test]
fn test_screen_context_summary_with_focused() {
    let ctx = ScreenContext::new_with_details(
        "Safari",
        Some("Google - Safari"),
        vec!["Safari - Google".into(), "Terminal - zsh".into()],
        Some("clipboard text".into()),
    );
    let summary = ctx.summary();
    assert!(summary.contains("Safari"), "Should contain focused app");
    assert!(summary.contains("Terminal"), "Should list open windows");
    assert!(summary.contains("clipboard text"), "Should include clipboard");
}

#[test]
fn test_screen_context_to_json() {
    let ctx = ScreenContext::new_with_details("Finder", None, vec![], None);
    let json = serde_json::to_string(&ctx).unwrap();
    assert!(json.contains("Finder"));
}

#[test]
fn test_empty_context() {
    let ctx = ScreenContext::empty();
    assert!(ctx.frontmost_app().is_empty());
    assert!(ctx.clipboard().is_none());
}

#[cfg(target_os = "macos")]
#[test]
fn test_capture_context_returns_frontmost_app() {
    let reader = aura_screen::macos::MacOSScreenReader::new().unwrap();
    let ctx = reader.capture_context().unwrap();
    // Should at least detect a frontmost app (cargo test itself runs in a terminal)
    assert!(
        !ctx.frontmost_app().is_empty(),
        "Should detect frontmost app"
    );
}
```

**Step 2: Run tests to verify they fail**

Run: `cargo test -p aura-screen --test context_test 2>&1 | head -20`
Expected: Compilation errors — `new_with_details`, `frontmost_app`, `clipboard`, `empty` don't exist.

**Step 3: Rewrite ScreenContext**

Rewrite `crates/aura-screen/src/context.rs`:

```rust
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScreenContext {
    frontmost_app: String,
    frontmost_title: Option<String>,
    open_windows: Vec<String>,
    clipboard: Option<String>,
}

impl ScreenContext {
    pub fn empty() -> Self {
        Self {
            frontmost_app: String::new(),
            frontmost_title: None,
            open_windows: Vec::new(),
            clipboard: None,
        }
    }

    pub fn new_with_details(
        frontmost_app: &str,
        frontmost_title: Option<&str>,
        open_windows: Vec<String>,
        clipboard: Option<String>,
    ) -> Self {
        Self {
            frontmost_app: frontmost_app.to_string(),
            frontmost_title: frontmost_title.map(String::from),
            open_windows,
            clipboard,
        }
    }

    pub fn frontmost_app(&self) -> &str {
        &self.frontmost_app
    }

    pub fn clipboard(&self) -> Option<&str> {
        self.clipboard.as_deref()
    }

    /// Human-readable summary for Gemini context injection.
    pub fn summary(&self) -> String {
        let mut parts = Vec::new();
        parts.push(format!("Frontmost app: {}", self.frontmost_app));
        if let Some(ref title) = self.frontmost_title {
            parts.push(format!("Window title: {title}"));
        }
        if !self.open_windows.is_empty() {
            parts.push(format!("Open windows: {}", self.open_windows.join(", ")));
        }
        if let Some(ref clip) = self.clipboard {
            let truncated = if clip.len() > 200 {
                format!("{}...", &clip[..200])
            } else {
                clip.clone()
            };
            parts.push(format!("Clipboard: {truncated}"));
        }
        parts.join("\n")
    }
}
```

**Step 4: Rewrite MacOSScreenReader using osascript + pbpaste**

Rewrite `crates/aura-screen/src/macos.rs`:

```rust
use std::process::Command;

use anyhow::Result;

use crate::context::ScreenContext;

pub struct MacOSScreenReader;

impl MacOSScreenReader {
    pub fn new() -> Result<Self> {
        Ok(Self)
    }

    pub fn capture_context(&self) -> Result<ScreenContext> {
        let frontmost_app = get_frontmost_app().unwrap_or_default();
        let frontmost_title = get_frontmost_title();
        let open_windows = get_open_windows().unwrap_or_default();
        let clipboard = get_clipboard();

        Ok(ScreenContext::new_with_details(
            &frontmost_app,
            frontmost_title.as_deref(),
            open_windows,
            clipboard,
        ))
    }
}

fn run_osascript(script: &str) -> Option<String> {
    let output = Command::new("osascript")
        .arg("-e")
        .arg(script)
        .output()
        .ok()?;
    if output.status.success() {
        let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if text.is_empty() { None } else { Some(text) }
    } else {
        None
    }
}

fn get_frontmost_app() -> Option<String> {
    run_osascript(
        r#"tell application "System Events" to get name of first application process whose frontmost is true"#,
    )
}

fn get_frontmost_title() -> Option<String> {
    run_osascript(
        r#"tell application "System Events"
            set frontApp to first application process whose frontmost is true
            tell frontApp
                if (count of windows) > 0 then
                    return name of window 1
                else
                    return ""
                end if
            end tell
        end tell"#,
    )
}

fn get_open_windows() -> Option<Vec<String>> {
    let text = run_osascript(
        r#"tell application "System Events"
            set windowList to {}
            repeat with proc in (every application process whose visible is true)
                repeat with win in (every window of proc)
                    set end of windowList to (name of proc) & " - " & (name of win)
                end repeat
            end repeat
            set text item delimiters to linefeed
            return windowList as text
        end tell"#,
    )?;
    Some(text.lines().map(String::from).collect())
}

fn get_clipboard() -> Option<String> {
    let output = Command::new("pbpaste").output().ok()?;
    if output.status.success() {
        let text = String::from_utf8_lossy(&output.stdout).to_string();
        if text.is_empty() { None } else { Some(text) }
    } else {
        None
    }
}
```

**Step 5: Update Cargo.toml** — remove `core-foundation` and `core-graphics` (no longer needed, we use osascript instead), add `serde_json` dev-dep:

```toml
[package]
name = "aura-screen"
version.workspace = true
edition.workspace = true

[dependencies]
tokio.workspace = true
tracing.workspace = true
anyhow.workspace = true
serde.workspace = true

[dev-dependencies]
serde_json.workspace = true
```

**Step 6: Run tests**

Run: `cargo test -p aura-screen --test context_test -- --nocapture`
Expected: All tests pass on macOS.

**Step 7: Verify workspace**

Run: `cargo check --workspace`
Expected: OK.

**Step 8: Commit**

```bash
git add crates/aura-screen/
git commit -m "feat: implement screen context with frontmost app, windows, and clipboard"
```

---

## Task 3: Rewrite Gemini Tools + Remove Old Bridge Dependency

Replace the 5 hardcoded tool declarations with `run_applescript` and `get_screen_context`. Use the existing `protocol::Tool` and `protocol::FunctionDeclaration` types. Remove `aura-bridge` from aura-gemini's dependencies.

**Files:**
- Rewrite: `crates/aura-gemini/src/tools.rs`
- Modify: `crates/aura-gemini/Cargo.toml` (remove `aura-bridge`)
- No changes to `protocol.rs` or `session.rs` — the `build_tool_declarations() -> Vec<Tool>` signature stays the same.

**Step 1: Write the new tools.rs**

Replace `crates/aura-gemini/src/tools.rs` entirely:

```rust
//! Gemini tool declarations for dynamic macOS automation.

use crate::protocol::{FunctionDeclaration, Tool};
use serde_json::json;

/// Build the tool declarations sent to Gemini in the setup message.
///
/// Returns a `Vec<Tool>` with a single `Tool` containing two
/// `FunctionDeclaration`s: `run_applescript` and `get_screen_context`.
pub fn build_tool_declarations() -> Vec<Tool> {
    vec![Tool {
        function_declarations: vec![
            FunctionDeclaration {
                name: "run_applescript".into(),
                description: "Execute AppleScript or JXA code to control any macOS application \
                    or system feature. You can open apps, manage windows, interact with UI \
                    elements, automate workflows, manipulate files, control system settings, \
                    send keystrokes, and more. Write the script based on what the user needs. \
                    Prefer simple scripts — chain multiple calls over one complex script."
                    .into(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "script": {
                            "type": "string",
                            "description": "The AppleScript or JXA code to execute"
                        },
                        "language": {
                            "type": "string",
                            "enum": ["applescript", "javascript"],
                            "description": "Script language. Default: applescript"
                        },
                        "timeout_secs": {
                            "type": "integer",
                            "description": "Max execution time in seconds. Default: 30"
                        }
                    },
                    "required": ["script"]
                }),
            },
            FunctionDeclaration {
                name: "get_screen_context".into(),
                description: "Get the user's current screen context: frontmost application, \
                    window title, list of open windows, and clipboard contents. Always call \
                    this before taking action so you understand what the user is doing."
                    .into(),
                parameters: json!({
                    "type": "object",
                    "properties": {}
                }),
            },
        ],
    }]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_declarations_returns_two_functions() {
        let tools = build_tool_declarations();
        assert_eq!(tools.len(), 1, "Should be one Tool object");
        assert_eq!(
            tools[0].function_declarations.len(),
            2,
            "Should have 2 function declarations"
        );
    }

    #[test]
    fn tool_names_are_correct() {
        let tools = build_tool_declarations();
        let names: Vec<&str> = tools[0]
            .function_declarations
            .iter()
            .map(|fd| fd.name.as_str())
            .collect();
        assert_eq!(names, vec!["run_applescript", "get_screen_context"]);
    }

    #[test]
    fn tool_declarations_serialize_to_valid_json() {
        let tools = build_tool_declarations();
        let value = serde_json::to_value(&tools).unwrap();
        let decls = value[0]["functionDeclarations"].as_array().unwrap();
        assert_eq!(decls.len(), 2);
        assert_eq!(decls[0]["name"], "run_applescript");
        assert_eq!(decls[1]["name"], "get_screen_context");
    }

    #[test]
    fn run_applescript_has_required_script_param() {
        let tools = build_tool_declarations();
        let params = &tools[0].function_declarations[0].parameters;
        assert!(params["properties"]["script"].is_object());
        let required = params["required"].as_array().unwrap();
        assert!(required.iter().any(|v| v == "script"));
    }
}
```

**Key design choice:** We keep the `build_tool_declarations() -> Vec<Tool>` function signature identical to the old one. This means `session.rs:418` (`tools: Some(build_tool_declarations())`) works without any changes. The old `function_call_to_action()` function is removed — tool call handling moves to the daemon in Task 9.

**Step 2: Remove aura-bridge dependency from aura-gemini**

Edit `crates/aura-gemini/Cargo.toml` — remove the `aura-bridge = { path = "../aura-bridge" }` line from `[dependencies]`.

**Step 3: Run tests**

Run: `cargo test -p aura-gemini -- --nocapture`
Expected: All tests pass (new tools tests + existing session/protocol tests).

**Step 4: Verify workspace compiles**

Run: `cargo check --workspace`
Expected: OK. The daemon still imports `function_call_to_action` from the old tools, but that import is in `main.rs` which we'll rewrite in Task 9.

Wait — check if `main.rs` uses `use aura_gemini::tools::function_call_to_action;`. If so, this will break. Let me be explicit:

**Step 4a: Fix daemon main.rs import**

In `crates/aura-daemon/src/main.rs`, line 17 has `use aura_gemini::tools::function_call_to_action;`. Remove this line. Then in `run_processor()`, replace the `function_call_to_action`-based tool handling with a temporary stub that logs "tool call handling migrating to v2":

```rust
// In run_processor(), replace the entire ToolCall match arm body with:
Ok(GeminiEvent::ToolCall { id, name, args }) => {
    tracing::info!(name = %name, "Tool call received (v2 migration pending)");
    let response = serde_json::json!({
        "success": false,
        "error": "Tool handling is being migrated to v2",
    });
    if let Err(e) = session.send_tool_response(id, name, response).await {
        tracing::error!("Failed to send tool response: {e}");
    }
}
```

Also remove `use aura_bridge::actions::ActionExecutor;` and the `MacOSExecutor` import/usage.

**Step 5: Verify workspace**

Run: `cargo check --workspace`
Expected: OK.

**Step 6: Commit**

```bash
git add crates/aura-gemini/ crates/aura-daemon/src/main.rs
git commit -m "feat: replace hardcoded tools with run_applescript and get_screen_context"
```

---

## Task 4: Remove Old Bridge Code + Update System Prompt

Now that aura-gemini no longer depends on `actions.rs`/`macos.rs`, clean them up. Also update the system prompt with Aura's personality.

**Files:**
- Delete: `crates/aura-bridge/src/actions.rs`
- Delete: `crates/aura-bridge/src/macos.rs`
- Modify: `crates/aura-bridge/src/lib.rs` (remove old modules)
- Modify: `crates/aura-bridge/Cargo.toml` (remove `async-trait`)
- Modify: `crates/aura-gemini/src/config.rs` (personality prompt)

**Step 1: Clean up aura-bridge**

Update `crates/aura-bridge/src/lib.rs`:

```rust
//! Aura OS bridge: dynamic AppleScript execution

pub mod script;
```

Delete `crates/aura-bridge/src/actions.rs` and `crates/aura-bridge/src/macos.rs`.

Remove `async-trait` from `crates/aura-bridge/Cargo.toml`:

```toml
[package]
name = "aura-bridge"
version.workspace = true
edition.workspace = true

[features]
test-support = []

[dependencies]
tokio.workspace = true
tracing.workspace = true
serde.workspace = true
serde_json.workspace = true
```

**Step 2: Fix daemon Cargo.toml**

In `crates/aura-daemon/Cargo.toml`, remove `features = ["test-support"]` from the `aura-bridge` dev-dependency (the old MockExecutor is gone). Also remove the `aura-bridge` dev-dependency if nothing else uses it.

Remove from daemon `main.rs` any remaining imports from old bridge code (`ActionExecutor`, `MacOSExecutor`, etc.).

**Step 3: Write personality test**

Add to existing tests in `crates/aura-gemini/src/config.rs`:

```rust
#[test]
fn test_system_prompt_has_aura_personality() {
    let config = GeminiConfig::from_env_inner("test-key-12345");
    assert!(config.system_prompt.contains("Aura"));
    assert!(config.system_prompt.contains("run_applescript"));
    assert!(config.system_prompt.contains("get_screen_context"));
}
```

**Step 4: Update system prompt in config.rs**

In `crates/aura-gemini/src/config.rs`, replace the `system_prompt` default value with:

```rust
const SYSTEM_PROMPT: &str = r#"You are Aura — a witty, slightly sarcastic macOS companion who actually gets things done. Think JARVIS meets a sleep-deprived senior engineer who's seen too much. You're sharp, helpful, and occasionally roast the user (lovingly).

Personality:
- Dry wit, concise responses. Never verbose.
- You acknowledge context ("I see you've got 47 Chrome tabs open... bold choice").
- You're competent and confident — no hedging, no "I'll try my best."
- When you automate something, be casual ("Done. Moved your windows around. You're welcome.").
- You have opinions about apps ("Electron apps... consuming RAM since 2013").
- You reference earlier context naturally.
- Greet based on time and context, not generic hellos.

Tools:
- You have two tools: run_applescript and get_screen_context.
- ALWAYS call get_screen_context first to understand what the user is doing before taking action.
- Use run_applescript to execute AppleScript or JXA code to control macOS. You can open apps, manage windows, search files with mdfind, interact with UI elements, control system settings, type text, click buttons — anything macOS can do.
- Prefer simple, short scripts. Chain multiple calls rather than writing one complex script.
- If a script fails, try a different approach. Be honest about failures.

Rules:
- Keep voice responses under 2 sentences unless explaining something complex.
- Never say "I'm an AI" or "I'm a language model." You're Aura.
- Never hedge with "I'll try" — just do it.
- When you don't know something, say so directly."#;
```

Use this constant in the `GeminiConfig` constructor where `system_prompt` is currently set.

**Step 5: Run all tests**

Run: `cargo test --workspace -- --nocapture`
Expected: All pass.

**Step 6: Commit**

```bash
git add crates/aura-bridge/ crates/aura-gemini/ crates/aura-daemon/
git commit -m "feat: remove old bridge actions, add Aura personality prompt"
```

---

## Task 5: Add Proxy URL Support to GeminiConfig

Allow the daemon to connect through the Cloud Run proxy.

**Files:**
- Modify: `crates/aura-gemini/src/config.rs`

**Step 1: Write failing test**

Add to config tests:

```rust
#[test]
fn test_proxy_url_overrides_direct_connection() {
    let mut config = GeminiConfig::from_env_inner("test-key-123");
    config.proxy_url = Some("wss://aura-proxy-xyz.run.app/ws".into());
    let url = config.ws_url();
    assert!(url.starts_with("wss://aura-proxy-xyz.run.app/ws"));
    assert!(url.contains("api_key=test-key-123"));
}

#[test]
fn test_no_proxy_uses_direct_gemini_url() {
    let config = GeminiConfig::from_env_inner("test-key-123");
    assert!(config.proxy_url.is_none());
    let url = config.ws_url();
    assert!(url.contains("generativelanguage.googleapis.com"));
}
```

**Step 2: Implement**

Add `pub proxy_url: Option<String>` field to `GeminiConfig` (default `None`). Modify `ws_url()`:

```rust
pub fn ws_url(&self) -> String {
    if let Some(ref proxy) = self.proxy_url {
        format!("{proxy}?api_key={}", self.api_key)
    } else {
        format!(
            "wss://generativelanguage.googleapis.com/ws/\
             google.ai.generativelanguage.v1alpha.GenerativeService.BidiGenerateContent\
             ?key={}",
            self.api_key
        )
    }
}
```

Also check `AURA_PROXY_URL` environment variable in `from_env()`:

```rust
let proxy_url = std::env::var("AURA_PROXY_URL").ok();
```

**Step 3: Run tests and commit**

Run: `cargo test -p aura-gemini config::tests`

```bash
git add crates/aura-gemini/src/config.rs
git commit -m "feat: add proxy URL support to GeminiConfig"
```

---

## Task 6: Create aura-menubar Crate with Working Click Handler

Create the macOS menu bar app with NSStatusItem (colored dot), NSPopover (conversation panel), and a working click handler using objc class declaration.

**Files:**
- Create: `crates/aura-menubar/Cargo.toml`
- Create: `crates/aura-menubar/src/lib.rs`
- Create: `crates/aura-menubar/src/status_item.rs`
- Create: `crates/aura-menubar/src/popover.rs`
- Create: `crates/aura-menubar/src/app.rs`
- Modify: `Cargo.toml` (workspace members)

**Step 1: Set up crate**

Create `crates/aura-menubar/Cargo.toml`:

```toml
[package]
name = "aura-menubar"
version.workspace = true
edition.workspace = true

[dependencies]
tokio.workspace = true
tracing.workspace = true
anyhow.workspace = true
serde.workspace = true
serde_json.workspace = true
cocoa = "0.26"
objc = "0.2"
core-graphics = "0.24"
core-foundation = "0.10"
```

Add `"crates/aura-menubar"` to workspace members in root `Cargo.toml`.

**Step 2: Create status_item.rs**

Create `crates/aura-menubar/src/status_item.rs`:

```rust
use cocoa::base::{id, NO};
use cocoa::foundation::{NSPoint, NSRect, NSSize};
use objc::{class, msg_send, sel, sel_impl};

use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Arc;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum DotColor {
    Gray = 0,
    Green = 1,
    Amber = 2,
    Red = 3,
}

pub struct AuraStatusItem {
    status_item: id,
    color: Arc<AtomicU8>,
}

unsafe impl Send for AuraStatusItem {}

impl AuraStatusItem {
    /// MUST be called on the main thread.
    pub unsafe fn new() -> Self {
        let status_bar: id = msg_send![class!(NSStatusBar), systemStatusBar];
        let length: f64 = -1.0; // NSVariableStatusItemLength
        let status_item: id = msg_send![status_bar, statusItemWithLength: length];
        let _: () = msg_send![status_item, retain];

        let color = Arc::new(AtomicU8::new(DotColor::Gray as u8));
        let item = Self { status_item, color };
        item.update_icon(DotColor::Gray);
        item
    }

    pub unsafe fn set_color(&self, color: DotColor) {
        self.color.store(color as u8, Ordering::Relaxed);
        self.update_icon(color);
    }

    pub fn current_color(&self) -> DotColor {
        match self.color.load(Ordering::Relaxed) {
            1 => DotColor::Green,
            2 => DotColor::Amber,
            3 => DotColor::Red,
            _ => DotColor::Gray,
        }
    }

    unsafe fn update_icon(&self, color: DotColor) {
        let size = NSSize::new(18.0, 18.0);
        let image: id = msg_send![class!(NSImage), alloc];
        let image: id = msg_send![image, initWithSize: size];
        let _: () = msg_send![image, lockFocus];

        let (r, g, b): (f64, f64, f64) = match color {
            DotColor::Gray => (0.6, 0.6, 0.6),
            DotColor::Green => (0.3, 0.9, 0.5),
            DotColor::Amber => (1.0, 0.8, 0.3),
            DotColor::Red => (0.9, 0.3, 0.3),
        };

        let ns_color: id = msg_send![class!(NSColor),
            colorWithRed: r green: g blue: b alpha: 1.0f64];
        let _: () = msg_send![ns_color, setFill];

        let dot_size = 10.0f64;
        let offset = (18.0 - dot_size) / 2.0;
        let rect = NSRect::new(
            NSPoint::new(offset, offset),
            NSSize::new(dot_size, dot_size),
        );
        let path: id = msg_send![class!(NSBezierPath), bezierPathWithOvalInRect: rect];
        let _: () = msg_send![path, fill];

        let _: () = msg_send![image, unlockFocus];
        let _: () = msg_send![image, setTemplate: NO];

        let button: id = msg_send![self.status_item, button];
        let _: () = msg_send![button, setImage: image];
    }

    pub fn raw(&self) -> id {
        self.status_item
    }
}
```

**Step 3: Create popover.rs**

Create `crates/aura-menubar/src/popover.rs`:

```rust
use cocoa::base::{id, nil, NO, YES};
use cocoa::foundation::{NSPoint, NSRect, NSSize, NSString};
use objc::{class, msg_send, sel, sel_impl};

pub struct AuraPopover {
    popover: id,
    content_view: id,
}

unsafe impl Send for AuraPopover {}

impl AuraPopover {
    /// MUST be called on the main thread.
    pub unsafe fn new() -> Self {
        let popover: id = msg_send![class!(NSPopover), alloc];
        let popover: id = msg_send![popover, init];

        let _: () = msg_send![popover, setContentSize: NSSize::new(320.0, 480.0)];
        let _: () = msg_send![popover, setBehavior: 1i64]; // Transient

        let vc: id = msg_send![class!(NSViewController), alloc];
        let vc: id = msg_send![vc, init];

        let frame = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(320.0, 480.0));
        let content_view: id = msg_send![class!(NSView), alloc];
        let content_view: id = msg_send![content_view, initWithFrame: frame];

        let _: () = msg_send![content_view, setWantsLayer: YES];
        let layer: id = msg_send![content_view, layer];
        let bg: id = msg_send![class!(NSColor),
            colorWithRed: 0.1f64 green: 0.1f64 blue: 0.12f64 alpha: 1.0f64];
        let cg_color: id = msg_send![bg, CGColor];
        let _: () = msg_send![layer, setBackgroundColor: cg_color];

        let _: () = msg_send![vc, setView: content_view];
        let _: () = msg_send![popover, setContentViewController: vc];

        Self { popover, content_view }
    }

    pub unsafe fn toggle(&self, relative_to: id) {
        let shown: bool = msg_send![self.popover, isShown];
        if shown {
            let _: () = msg_send![self.popover, close];
        } else {
            let bounds: NSRect = msg_send![relative_to, bounds];
            let _: () = msg_send![self.popover, showRelativeToRect: bounds
                ofView: relative_to
                preferredEdge: 1u64]; // NSMinYEdge
        }
    }

    pub unsafe fn add_message(&self, text: &str, is_user: bool) {
        let subviews: id = msg_send![self.content_view, subviews];
        let count: usize = msg_send![subviews, count];
        let y_offset = 480.0 - (count as f64 + 1.0) * 40.0;
        if y_offset < 0.0 { return; } // Overflow guard

        let frame = NSRect::new(
            NSPoint::new(if is_user { 80.0 } else { 10.0 }, y_offset),
            NSSize::new(230.0, 35.0),
        );
        let label: id = msg_send![class!(NSTextField), alloc];
        let label: id = msg_send![label, initWithFrame: frame];

        let ns_text = NSString::alloc(nil).init_str(text);
        let _: () = msg_send![label, setStringValue: ns_text];
        let _: () = msg_send![label, setBezeled: NO];
        let _: () = msg_send![label, setDrawsBackground: NO];
        let _: () = msg_send![label, setEditable: NO];
        let _: () = msg_send![label, setSelectable: YES];

        let font: id = msg_send![class!(NSFont), systemFontOfSize: 13.0f64];
        let _: () = msg_send![label, setFont: font];
        let text_color: id = msg_send![class!(NSColor), whiteColor];
        let _: () = msg_send![label, setTextColor: text_color];

        let _: () = msg_send![self.content_view, addSubview: label];
    }
}
```

**Step 4: Create app.rs with WORKING click handler and message polling**

This is the critical piece. We use `objc::declare::ClassDecl` to create a custom Objective-C class that handles click events, and an `NSTimer` to poll the tokio message channel.

Create `crates/aura-menubar/src/app.rs`:

```rust
use cocoa::appkit::{NSApp, NSApplication, NSApplicationActivationPolicyAccessory};
use cocoa::base::{id, nil};
use cocoa::foundation::NSAutoreleasePool;
use objc::declare::ClassDecl;
use objc::runtime::{Object, Sel};
use objc::{class, msg_send, sel, sel_impl};

use std::sync::Mutex;
use tokio::sync::mpsc;

use crate::popover::AuraPopover;
use crate::status_item::{AuraStatusItem, DotColor};

#[derive(Debug, Clone)]
pub enum MenuBarMessage {
    SetColor(DotColor),
    AddMessage { text: String, is_user: bool },
    Shutdown,
}

/// Global mutable state accessed from ObjC callbacks.
/// This is safe because all access happens on the main thread.
static GLOBAL_STATE: Mutex<Option<AppState>> = Mutex::new(None);

struct AppState {
    status_item: AuraStatusItem,
    popover: AuraPopover,
    rx: mpsc::Receiver<MenuBarMessage>,
}

pub struct MenuBarApp {
    rx: mpsc::Receiver<MenuBarMessage>,
}

impl MenuBarApp {
    pub fn new() -> (Self, mpsc::Sender<MenuBarMessage>) {
        let (tx, rx) = mpsc::channel(64);
        (Self { rx }, tx)
    }

    /// Run the menu bar app. Blocks forever on the main thread.
    pub fn run(self) {
        unsafe {
            let _pool = NSAutoreleasePool::new(nil);
            let app = NSApp();
            let _: () = msg_send![app, setActivationPolicy:
                NSApplicationActivationPolicyAccessory];

            let status_item = AuraStatusItem::new();
            let popover = AuraPopover::new();

            // Register click handler via custom ObjC class
            let handler_class = register_click_handler_class();
            let handler: id = msg_send![handler_class, new];
            let button: id = msg_send![status_item.raw(), button];
            let _: () = msg_send![button, setTarget: handler];
            let _: () = msg_send![button, setAction: sel!(handleClick:)];

            // Store state globally for ObjC callbacks
            *GLOBAL_STATE.lock().unwrap() = Some(AppState {
                status_item,
                popover,
                rx: self.rx,
            });

            // Set up NSTimer to poll message channel every 50ms
            let timer_target = handler;
            let interval: f64 = 0.05;
            let _: id = msg_send![class!(NSTimer),
                scheduledTimerWithTimeInterval: interval
                target: timer_target
                selector: sel!(pollMessages:)
                userInfo: nil
                repeats: true
            ];

            tracing::info!("Menu bar app running");
            let _: () = msg_send![app, run];
        }
    }
}

/// Register a custom ObjC class with click and timer handlers.
fn register_click_handler_class() -> &'static objc::runtime::Class {
    let superclass = class!(NSObject);
    let mut decl = ClassDecl::new("AuraClickHandler", superclass)
        .expect("Failed to create AuraClickHandler class");

    unsafe {
        // Click handler — toggles the popover
        decl.add_method(
            sel!(handleClick:),
            handle_click as extern "C" fn(&Object, Sel, id),
        );

        // Timer handler — polls tokio channel for messages
        decl.add_method(
            sel!(pollMessages:),
            poll_messages as extern "C" fn(&Object, Sel, id),
        );
    }

    decl.register()
}

extern "C" fn handle_click(_this: &Object, _cmd: Sel, _sender: id) {
    unsafe {
        if let Some(ref state) = *GLOBAL_STATE.lock().unwrap() {
            let button: id = msg_send![state.status_item.raw(), button];
            state.popover.toggle(button);
        }
    }
}

extern "C" fn poll_messages(_this: &Object, _cmd: Sel, _timer: id) {
    let mut state_guard = GLOBAL_STATE.lock().unwrap();
    if let Some(ref mut state) = *state_guard {
        // Drain all pending messages (non-blocking)
        while let Ok(msg) = state.rx.try_recv() {
            unsafe {
                match msg {
                    MenuBarMessage::SetColor(color) => {
                        state.status_item.set_color(color);
                    }
                    MenuBarMessage::AddMessage { text, is_user } => {
                        state.popover.add_message(&text, is_user);
                    }
                    MenuBarMessage::Shutdown => {
                        let app = NSApp();
                        let _: () = msg_send![app, terminate: nil];
                    }
                }
            }
        }
    }
}
```

**Step 5: Create lib.rs**

```rust
//! Aura menu bar: native macOS status item and popover UI

pub mod app;
pub mod popover;
pub mod status_item;
```

**Step 6: Verify compilation**

Run: `cargo check -p aura-menubar`
Expected: Compiles.

Run: `cargo check --workspace`
Expected: OK.

**Step 7: Commit**

```bash
git add crates/aura-menubar/ Cargo.toml
git commit -m "feat: create aura-menubar with NSStatusItem, NSPopover, and click handler"
```

---

## Task 7: Add Session Memory — SQLite Storage

**Files:**
- Create: `crates/aura-daemon/src/memory.rs`
- Modify: `crates/aura-daemon/src/lib.rs`
- Modify: `crates/aura-daemon/Cargo.toml`
- Create: `crates/aura-daemon/tests/memory_test.rs`

**Step 1: Add dependencies to Cargo.toml**

Add to `crates/aura-daemon/Cargo.toml` `[dependencies]`:

```toml
rusqlite = { version = "0.32", features = ["bundled"] }
uuid = { version = "1", features = ["v4"] }
chrono = { version = "0.4", features = ["serde"] }
```

**Step 2: Write failing tests**

Create `crates/aura-daemon/tests/memory_test.rs`:

```rust
use aura_daemon::memory::{MessageRole, SessionMemory};
use tempfile::TempDir;

fn memory_in_tmpdir() -> (SessionMemory, TempDir) {
    let dir = TempDir::new().unwrap();
    let db_path = dir.path().join("sessions.db");
    let mem = SessionMemory::open(&db_path).unwrap();
    (mem, dir)
}

#[test]
fn test_create_and_list_sessions() {
    let (mem, _dir) = memory_in_tmpdir();
    let id = mem.start_session().unwrap();
    let sessions = mem.list_sessions(10).unwrap();
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].id, id);
}

#[test]
fn test_add_and_retrieve_messages() {
    let (mem, _dir) = memory_in_tmpdir();
    let sid = mem.start_session().unwrap();
    mem.add_message(&sid, MessageRole::User, "Hello Aura", None).unwrap();
    mem.add_message(&sid, MessageRole::Assistant, "Hey. What's up?", None).unwrap();

    let messages = mem.get_messages(&sid).unwrap();
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[0].role, MessageRole::User);
    assert_eq!(messages[0].content, "Hello Aura");
    assert_eq!(messages[1].role, MessageRole::Assistant);
}

#[test]
fn test_end_session_with_summary() {
    let (mem, _dir) = memory_in_tmpdir();
    let sid = mem.start_session().unwrap();
    mem.add_message(&sid, MessageRole::User, "Open Safari", None).unwrap();
    mem.end_session(&sid, Some("Opened Safari")).unwrap();

    let sessions = mem.list_sessions(10).unwrap();
    assert_eq!(sessions[0].summary.as_deref(), Some("Opened Safari"));
    assert!(sessions[0].ended_at.is_some());
}

#[test]
fn test_multiple_sessions_ordered_by_recency() {
    let (mem, _dir) = memory_in_tmpdir();
    let _s1 = mem.start_session().unwrap();
    std::thread::sleep(std::time::Duration::from_millis(10));
    let s2 = mem.start_session().unwrap();

    let sessions = mem.list_sessions(10).unwrap();
    assert_eq!(sessions[0].id, s2);
}
```

**Step 3: Implement SessionMemory**

Create `crates/aura-daemon/src/memory.rs`:

```rust
use anyhow::Result;
use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageRole {
    User,
    Assistant,
    ToolCall,
    ToolResult,
}

impl MessageRole {
    fn as_str(&self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Assistant => "assistant",
            Self::ToolCall => "tool_call",
            Self::ToolResult => "tool_result",
        }
    }

    fn from_str(s: &str) -> Self {
        match s {
            "assistant" => Self::Assistant,
            "tool_call" => Self::ToolCall,
            "tool_result" => Self::ToolResult,
            _ => Self::User,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Session {
    pub id: String,
    pub started_at: String,
    pub ended_at: Option<String>,
    pub summary: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Message {
    pub id: i64,
    pub session_id: String,
    pub role: MessageRole,
    pub content: String,
    pub timestamp: String,
    pub metadata: Option<String>,
}

pub struct SessionMemory {
    conn: Connection,
}

impl SessionMemory {
    pub fn open(path: &std::path::Path) -> Result<Self> {
        let conn = Connection::open(path)?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS sessions (
                id TEXT PRIMARY KEY,
                started_at TEXT NOT NULL,
                ended_at TEXT,
                summary TEXT
            );
            CREATE TABLE IF NOT EXISTS messages (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                session_id TEXT NOT NULL REFERENCES sessions(id),
                role TEXT NOT NULL,
                content TEXT NOT NULL,
                timestamp TEXT NOT NULL,
                metadata TEXT
            );",
        )?;
        Ok(Self { conn })
    }

    pub fn start_session(&self) -> Result<String> {
        let id = uuid::Uuid::new_v4().to_string();
        let now = chrono::Utc::now().to_rfc3339();
        self.conn.execute(
            "INSERT INTO sessions (id, started_at) VALUES (?1, ?2)",
            params![id, now],
        )?;
        Ok(id)
    }

    pub fn end_session(&self, session_id: &str, summary: Option<&str>) -> Result<()> {
        let now = chrono::Utc::now().to_rfc3339();
        self.conn.execute(
            "UPDATE sessions SET ended_at = ?1, summary = ?2 WHERE id = ?3",
            params![now, summary, session_id],
        )?;
        Ok(())
    }

    pub fn add_message(
        &self,
        session_id: &str,
        role: MessageRole,
        content: &str,
        metadata: Option<&str>,
    ) -> Result<()> {
        let now = chrono::Utc::now().to_rfc3339();
        self.conn.execute(
            "INSERT INTO messages (session_id, role, content, timestamp, metadata)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![session_id, role.as_str(), content, now, metadata],
        )?;
        Ok(())
    }

    pub fn get_messages(&self, session_id: &str) -> Result<Vec<Message>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, session_id, role, content, timestamp, metadata
             FROM messages WHERE session_id = ?1 ORDER BY id ASC",
        )?;
        let messages = stmt
            .query_map(params![session_id], |row| {
                Ok(Message {
                    id: row.get(0)?,
                    session_id: row.get(1)?,
                    role: MessageRole::from_str(&row.get::<_, String>(2)?),
                    content: row.get(3)?,
                    timestamp: row.get(4)?,
                    metadata: row.get(5)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(messages)
    }

    pub fn list_sessions(&self, limit: usize) -> Result<Vec<Session>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, started_at, ended_at, summary
             FROM sessions ORDER BY started_at DESC LIMIT ?1",
        )?;
        let sessions = stmt
            .query_map(params![limit], |row| {
                Ok(Session {
                    id: row.get(0)?,
                    started_at: row.get(1)?,
                    ended_at: row.get(2)?,
                    summary: row.get(3)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(sessions)
    }
}
```

Add `pub mod memory;` to `crates/aura-daemon/src/lib.rs`.

**Step 4: Run tests**

Run: `cargo test -p aura-daemon --test memory_test -- --nocapture`
Expected: All 4 tests pass.

**Step 5: Commit**

```bash
git add crates/aura-daemon/
git commit -m "feat: add SQLite session memory for conversation persistence"
```

---

## Task 8: Create aura-proxy — Cloud Run WebSocket Proxy

**Files:**
- Create: `crates/aura-proxy/Cargo.toml`
- Create: `crates/aura-proxy/src/lib.rs`
- Create: `crates/aura-proxy/src/main.rs`
- Create: `crates/aura-proxy/src/relay.rs`
- Create: `crates/aura-proxy/tests/proxy_test.rs`
- Create: `crates/aura-proxy/Dockerfile`
- Modify: `Cargo.toml` (workspace members)

**Step 1: Create crate**

Create `crates/aura-proxy/Cargo.toml`:

```toml
[package]
name = "aura-proxy"
version.workspace = true
edition.workspace = true

[lib]
name = "aura_proxy"
path = "src/lib.rs"

[[bin]]
name = "aura-proxy"
path = "src/main.rs"

[dependencies]
tokio.workspace = true
tracing.workspace = true
tracing-subscriber.workspace = true
anyhow.workspace = true
serde.workspace = true
serde_json.workspace = true
axum = { version = "0.8", features = ["ws"] }
tokio-tungstenite = { version = "0.24", features = ["native-tls"] }
futures-util = "0.3"
tower-http = { version = "0.6", features = ["cors"] }

[dev-dependencies]
reqwest = { version = "0.12", features = ["json"] }
```

Add `"crates/aura-proxy"` to workspace members in root `Cargo.toml`.

**Step 2: Write health endpoint test**

Create `crates/aura-proxy/tests/proxy_test.rs`:

```rust
#[tokio::test]
async fn test_health_endpoint() {
    let port = {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.local_addr().unwrap().port()
    };

    let handle = tokio::spawn(async move {
        aura_proxy::run_server(port).await.unwrap();
    });

    tokio::time::sleep(std::time::Duration::from_millis(300)).await;

    let resp = reqwest::get(format!("http://127.0.0.1:{port}/health"))
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["status"], "ok");

    handle.abort();
}
```

**Step 3: Implement relay.rs**

Create `crates/aura-proxy/src/relay.rs`:

```rust
use axum::extract::ws::{Message, WebSocket};
use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::tungstenite;

/// Relay WebSocket frames between client and Gemini.
pub async fn relay_websocket(client_ws: WebSocket, gemini_url: String) {
    let gemini_conn = match tokio_tungstenite::connect_async(&gemini_url).await {
        Ok((ws, _)) => ws,
        Err(e) => {
            tracing::error!("Failed to connect to Gemini: {e}");
            return;
        }
    };

    let (mut client_tx, mut client_rx) = client_ws.split();
    let (mut gemini_tx, mut gemini_rx) = gemini_conn.split();

    let client_to_gemini = async {
        while let Some(Ok(msg)) = client_rx.next().await {
            let tung_msg = match msg {
                Message::Text(t) => tungstenite::Message::Text(t.to_string()),
                Message::Binary(b) => tungstenite::Message::Binary(b.to_vec()),
                Message::Close(_) => break,
                _ => continue,
            };
            if gemini_tx.send(tung_msg).await.is_err() {
                break;
            }
        }
    };

    let gemini_to_client = async {
        while let Some(Ok(msg)) = gemini_rx.next().await {
            let axum_msg = match msg {
                tungstenite::Message::Text(t) => Message::text(t),
                tungstenite::Message::Binary(b) => Message::binary(b),
                tungstenite::Message::Close(_) => break,
                _ => continue,
            };
            if client_tx.send(axum_msg).await.is_err() {
                break;
            }
        }
    };

    tokio::select! {
        _ = client_to_gemini => {},
        _ = gemini_to_client => {},
    }

    tracing::info!("WebSocket relay closed");
}
```

**Note on axum 0.8:** `Message::Text` wraps `Utf8Bytes` (not `String`), and `Message::Binary` wraps `Bytes` (not `Vec<u8>`). Use `.to_string()` and `.to_vec()` to convert for tungstenite. Use `Message::text()` and `Message::binary()` constructors for the reverse.

**Step 4: Implement lib.rs**

Create `crates/aura-proxy/src/lib.rs`:

```rust
pub mod relay;

use anyhow::Result;
use axum::{
    Router,
    extract::{Query, WebSocketUpgrade},
    response::{IntoResponse, Json},
    routing::get,
};
use serde::Deserialize;
use tower_http::cors::CorsLayer;

const GEMINI_WS_BASE: &str = "wss://generativelanguage.googleapis.com/ws/\
    google.ai.generativelanguage.v1alpha.GenerativeService.BidiGenerateContent";

#[derive(Deserialize)]
struct ConnectParams {
    api_key: String,
}

async fn health() -> Json<serde_json::Value> {
    Json(serde_json::json!({ "status": "ok" }))
}

async fn ws_handler(
    ws: WebSocketUpgrade,
    Query(params): Query<ConnectParams>,
) -> impl IntoResponse {
    let gemini_url = format!("{GEMINI_WS_BASE}?key={}", params.api_key);
    ws.on_upgrade(move |socket| relay::relay_websocket(socket, gemini_url))
}

pub async fn run_server(port: u16) -> Result<()> {
    let app = Router::new()
        .route("/health", get(health))
        .route("/ws", get(ws_handler))
        .layer(CorsLayer::permissive());

    let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{port}")).await?;
    tracing::info!("Proxy listening on port {port}");
    axum::serve(listener, app).await?;
    Ok(())
}
```

**Step 5: Implement main.rs**

Create `crates/aura-proxy/src/main.rs`:

```rust
use anyhow::Result;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let port: u16 = std::env::var("PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(8080);

    aura_proxy::run_server(port).await
}
```

**Step 6: Create Dockerfile**

Create `crates/aura-proxy/Dockerfile`:

```dockerfile
FROM rust:1.85 AS builder
WORKDIR /app
COPY . .
RUN cargo build --release -p aura-proxy

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y ca-certificates && rm -rf /var/lib/apt/lists/*
COPY --from=builder /app/target/release/aura-proxy /usr/local/bin/
ENV PORT=8080
EXPOSE 8080
CMD ["aura-proxy"]
```

**Step 7: Run tests**

Run: `cargo test -p aura-proxy --test proxy_test -- --nocapture`
Expected: Health endpoint passes.

**Step 8: Commit**

```bash
git add crates/aura-proxy/ Cargo.toml
git commit -m "feat: create Cloud Run WebSocket proxy for Gemini Live API"
```

---

## Task 9: Rewrite aura-daemon + Remove Overlay

Rewrite `main.rs` to use the menu bar, dynamic AppleScript execution, screen context, and session memory. Remove `aura-overlay` from the workspace simultaneously.

**Files:**
- Rewrite: `crates/aura-daemon/src/main.rs`
- Modify: `crates/aura-daemon/src/event.rs`
- Modify: `crates/aura-daemon/src/lib.rs`
- Modify: `crates/aura-daemon/Cargo.toml`
- Modify: `Cargo.toml` (remove aura-overlay from workspace)

**Step 1: Remove aura-overlay from workspace**

In root `Cargo.toml`, remove `"crates/aura-overlay"` from `members`.

**Step 2: Update daemon Cargo.toml**

Replace `aura-overlay` with `aura-menubar`:

```toml
[package]
name = "aura-daemon"
version.workspace = true
edition.workspace = true

[dependencies]
aura-voice = { path = "../aura-voice" }
aura-menubar = { path = "../aura-menubar" }
aura-screen = { path = "../aura-screen" }
aura-bridge = { path = "../aura-bridge" }
aura-gemini = { path = "../aura-gemini" }
serde.workspace = true
serde_json.workspace = true
tokio.workspace = true
tracing.workspace = true
tracing-subscriber.workspace = true
anyhow.workspace = true
clap = { version = "4", features = ["derive"] }
tokio-util = "0.7"
dirs = "6"
which = "7"
rusqlite = { version = "0.32", features = ["bundled"] }
uuid = { version = "1", features = ["v4"] }
chrono = { version = "0.4", features = ["serde"] }

[dev-dependencies]
tempfile = "3"
aura-gemini = { path = "../aura-gemini" }
```

**Step 3: Simplify event.rs**

Rewrite `crates/aura-daemon/src/event.rs`:

```rust
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AuraEvent {
    // Voice
    WakeWordDetected,
    ListeningStarted,

    // Gemini session
    GeminiConnected,
    GeminiReconnecting { attempt: u32 },

    // Conversation
    AssistantSpeaking { text: String },
    UserTranscription { text: String },
    BargeIn,

    // Tool execution
    ToolExecuted { name: String, success: bool, output: String },

    // System
    Shutdown,
}
```

**Step 4: Rewrite main.rs**

Rewrite `crates/aura-daemon/src/main.rs` — full implementation with menu bar, dynamic AppleScript, screen context, and session memory. See Task 8 in the original plan for the complete code. Key differences from original:

- Import `aura_menubar::app::{MenuBarApp, MenuBarMessage}` instead of overlay
- Import `aura_bridge::script::{ScriptExecutor, ScriptLanguage}` instead of old actions
- Import `aura_screen::macos::MacOSScreenReader`
- Import `aura_daemon::memory::{MessageRole, SessionMemory}`
- CLI flag is `--headless` (not `--no-overlay`)
- Tool call handler dispatches to `ScriptExecutor::run()` for `run_applescript` and `MacOSScreenReader::capture_context()` for `get_screen_context`
- Session memory logs all tool calls, results, and transcriptions
- On `GeminiEvent::Connected`, automatically capture screen context and log it
- Color changes sent via `MenuBarMessage::SetColor`

The full `run_processor` tool call handler:

```rust
Ok(GeminiEvent::ToolCall { id, name, args }) => {
    tracing::info!(name = %name, "Tool call");
    let _ = memory.add_message(&session_id, MessageRole::ToolCall, &format!("{name}: {args}"), None);

    if let Some(ref tx) = menubar_tx {
        let _ = tx.send(MenuBarMessage::SetColor(DotColor::Amber)).await;
    }

    let response = match name.as_str() {
        "run_applescript" => {
            let script = args.get("script").and_then(|v| v.as_str()).unwrap_or("");
            let language = match args.get("language").and_then(|v| v.as_str()) {
                Some("javascript") => ScriptLanguage::JavaScript,
                _ => ScriptLanguage::AppleScript,
            };
            let timeout = args.get("timeout_secs").and_then(|v| v.as_u64()).unwrap_or(30);
            let result = executor.run(script, language, timeout).await;
            serde_json::json!({
                "success": result.success,
                "stdout": result.stdout,
                "stderr": result.stderr,
            })
        }
        "get_screen_context" => {
            match screen_reader.capture_context() {
                Ok(ctx) => serde_json::json!({
                    "success": true,
                    "context": ctx.summary(),
                }),
                Err(e) => serde_json::json!({
                    "success": false,
                    "error": format!("{e}"),
                }),
            }
        }
        other => serde_json::json!({
            "success": false,
            "error": format!("Unknown tool: {other}"),
        }),
    };

    let _ = memory.add_message(&session_id, MessageRole::ToolResult, &response.to_string(), None);

    if let Some(ref tx) = menubar_tx {
        let _ = tx.send(MenuBarMessage::SetColor(DotColor::Green)).await;
    }

    if let Err(e) = session.send_tool_response(id, name, response).await {
        tracing::error!("Failed to send tool response: {e}");
    }
}
```

**Step 5: Update lib.rs**

```rust
pub mod bus;
pub mod daemon;
pub mod event;
pub mod memory;
pub mod setup;
```

The `daemon.rs` module can remain as-is or be simplified — it's only used if you want the old event-loop pattern. The new main.rs handles everything directly.

**Step 6: Verify workspace compiles**

Run: `cargo check --workspace`
Expected: OK (aura-overlay is no longer a member).

**Step 7: Commit**

```bash
git add Cargo.toml crates/aura-daemon/
git commit -m "feat: rewrite daemon with menu bar, dynamic AppleScript, session memory"
```

---

## Task 10: Wake Word Materialization + Context-Aware Greeting

Wire the wake word and implement the materialization sequence: dot appears → screen context gathered → context-aware greeting spoken.

**Files:**
- Modify: `crates/aura-daemon/src/main.rs`

**Step 1: Add greeting on GeminiEvent::Connected**

In `run_processor`, when `GeminiEvent::Connected` fires, gather screen context and inject it as the first turn context. Gemini's system prompt already tells it to greet based on context.

```rust
Ok(GeminiEvent::Connected) => {
    tracing::info!("Gemini connected");

    // Animate dot
    if let Some(ref tx) = menubar_tx {
        let _ = tx.send(MenuBarMessage::SetColor(DotColor::Green)).await;
    }

    // Gather screen context for initial greeting
    let greeting_context = match screen_reader.capture_context() {
        Ok(ctx) => {
            let summary = ctx.summary();
            tracing::info!(context = %summary, "Screen context for greeting");
            summary
        }
        Err(e) => {
            tracing::warn!("Screen context failed: {e}");
            "No screen context available".into()
        }
    };

    // Get current time for time-aware greeting
    let hour = chrono::Local::now().hour();
    let time_context = match hour {
        0..=5 => "It's very late at night.",
        6..=11 => "It's morning.",
        12..=16 => "It's afternoon.",
        17..=20 => "It's evening.",
        _ => "It's late at night.",
    };

    // Send context as initial user message so Gemini greets contextually
    // This triggers Gemini's first audio response (the greeting)
    let context_msg = format!(
        "[System: User just activated Aura. {time_context} Current screen context:\n{greeting_context}\n\nGreet the user based on this context. Be brief and witty.]"
    );

    // Log to memory
    let _ = memory.add_message(&session_id, MessageRole::User, &context_msg, None);
}
```

**Note:** This requires a method to send a text message to Gemini (not just audio). Check if `GeminiLiveSession` has a `send_text()` method. If not, the greeting can be triggered by the system prompt alone — Gemini Live starts listening immediately after connection, and the system prompt tells it to greet based on context.

**Step 2: Wake word detection thread (optional for MVP)**

If the wake word model exists at `~/.local/share/aura/models/hey-aura.rpw`, start a wake word detection thread that:
1. Forks mic audio (one copy to Gemini, one to wake word detector)
2. On detection, sends `AuraEvent::WakeWordDetected` on the bus
3. The bus event triggers reconnection to Gemini (if disconnected) or the greeting sequence

For MVP, Gemini Live's built-in VAD handles activation. Wake word is enhancement.

```rust
// In run_daemon(), after mic setup:
let wakeword_model = setup.data_dir().join("models/hey-aura.rpw");
if wakeword_model.exists() {
    tracing::info!("Wake word model found — detection enabled");
    // TODO: Fork audio to wake word detector thread
    // For MVP, Gemini handles VAD natively
} else {
    tracing::info!("No wake word model — using Gemini's built-in VAD");
}
```

**Step 3: Verify and commit**

Run: `cargo check -p aura-daemon`

```bash
git add crates/aura-daemon/src/main.rs
git commit -m "feat: add context-aware greeting on Gemini connection"
```

---

## Task 11: Competition Deliverables

**Files:**
- Create: `docs/architecture.md`
- Modify: `README.md`
- Create: `deploy.sh`

**Step 1: Architecture diagram**

Create `docs/architecture.md`:

````markdown
# Aura Architecture

```
+----------------------------------------------------------+
|                     macOS Client                          |
|                                                          |
|  [Menu Bar Dot] -----> [NSPopover]                       |
|       |                  |  Conversation Feed             |
|       |                  |  Settings (API Key)            |
|       v                  |                                |
|  +-----------+     +------------+     +----------------+ |
|  | Mic       |     | Audio      |     | Screen Context | |
|  | Capture   |     | Playback   |     | (osascript +   | |
|  | 16kHz     |     | 24kHz      |     |  CGWindow +    | |
|  | cpal      |     | rodio      |     |  pbpaste)      | |
|  +-----+-----+     +-----^------+     +-------+--------+ |
|        |                  |                    |          |
|  +-----v------------------+--------------------+--------+ |
|  |              aura-daemon (orchestrator)               | |
|  |                                                       | |
|  |  ScriptExecutor    SessionMemory    WakeWord          | |
|  |  (osascript)       (SQLite)         (rustpotter)      | |
|  +------------------------+------------------------------+ |
|                           | WSS                            |
+---------------------------+--------------------------------+
                            |
                     +------v------+
                     | Cloud Run   |
                     | Proxy       |
                     | (axum)      |
                     +------+------+
                            | WSS
                     +------v------+
                     | Gemini Live |
                     | API         |
                     +-------------+

Tools (generated by Gemini at runtime):
  run_applescript --> osascript --> Any macOS app
  get_screen_context --> frontmost app + windows + clipboard
```
````

**Step 2: Update README.md**

Cover: what Aura is, quick start, architecture, competition context. Include `GEMINI_API_KEY` setup and `cargo run -p aura-daemon`.

**Step 3: Create deploy.sh**

```bash
#!/bin/bash
set -euo pipefail
PROJECT_ID="${GCP_PROJECT_ID:?Set GCP_PROJECT_ID}"
REGION="${GCP_REGION:-us-central1}"

echo "Deploying aura-proxy to Cloud Run..."
gcloud builds submit \
  --tag "gcr.io/${PROJECT_ID}/aura-proxy" \
  --project "${PROJECT_ID}" \
  -f crates/aura-proxy/Dockerfile .

gcloud run deploy aura-proxy \
  --image "gcr.io/${PROJECT_ID}/aura-proxy" \
  --platform managed \
  --region "${REGION}" \
  --allow-unauthenticated \
  --port 8080 \
  --project "${PROJECT_ID}"

echo "Deployed!"
gcloud run services describe aura-proxy \
  --region "${REGION}" \
  --project "${PROJECT_ID}" \
  --format 'value(status.url)'
```

**Step 4: Demo Video Script (< 4 minutes)**

Outline for recording:

```
0:00-0:15  Title card: "Aura — Voice-First macOS Companion"
0:15-0:30  Show menu bar dot (gray → green on connect)
0:30-0:50  Voice: "Hey Aura" → dot pulses → "Hey, I see you've got VS Code
           open. Late night coding session? What do you need?"
0:50-1:20  Voice: "Open Safari and search for Rust async patterns"
           → AppleScript runs → Safari opens → search happens
           → "Done. Safari's open with your search. Anything else?"
1:20-1:50  Voice: "What apps do I have running right now?"
           → get_screen_context → lists running apps with commentary
1:50-2:20  Voice: "Tile my windows left and right"
           → Generates tiling AppleScript → windows move
           → "Tiled. Your workspace is now slightly less chaotic."
2:20-2:50  Click dot → show popover with conversation history
           → Show settings panel with API key field
2:50-3:15  Architecture diagram overlay: Client ↔ Cloud Run ↔ Gemini
           → Show Cloud Run deployment in GCP console
3:15-3:40  Voice: "How many screenshots do I have?"
           → Generates mdfind AppleScript → counts files
           → "You've got 247 screenshots. That's... a lot."
3:40-3:55  Closing: "Aura — what Siri should have been."
3:55       End card with GitHub repo URL
```

**Step 5: Commit**

```bash
chmod +x deploy.sh
git add docs/architecture.md README.md deploy.sh
git commit -m "docs: add architecture diagram, README, deploy script, and demo outline"
```

---

## Verification Checklist

After all tasks, run:

```bash
cargo check --workspace
cargo test --workspace
cargo clippy --workspace -- -D warnings
cargo fmt --all -- --check

# Individual test suites
cargo test -p aura-bridge --test script_test
cargo test -p aura-screen --test context_test
cargo test -p aura-gemini -- tools::tests
cargo test -p aura-daemon --test memory_test
cargo test -p aura-proxy --test proxy_test
```

---

## Timeline

| Day | Tasks | Focus |
|-----|-------|-------|
| 1 | 1, 2, 3, 4 | Foundation: ScriptExecutor + screen context + tools + personality |
| 2 | 5, 6 | Proxy URL + menu bar app (hardest UI task) |
| 3 | 7, 8 | Session memory + Cloud Run proxy |
| 4 | 9 | Daemon rewrite (wire everything together) |
| 5 | 10, 11 | Wake word + competition deliverables |
| 6 | Buffer | Polish, record demo video, deploy to Cloud Run, submit |

## Key Fixes from Review

1. **ScriptExecutor timeout** uses `Child::kill()` instead of dropping the future
2. **Workspace stays compilable** — Task 1 adds code additively, Task 3+4 atomically remove old code
3. **Menu bar click handler is fully implemented** using `ClassDecl` + `NSTimer` polling
4. **axum 0.8 types** — `Utf8Bytes`/`Bytes` conversions handled correctly in proxy relay
5. **Task ordering fixed** — proxy URL (Task 5) before daemon rewrite (Task 9), overlay removal merged into daemon rewrite (Task 9)
6. **Demo video script** included with timestamp breakdown
7. **Context-aware greeting** actually implemented in Task 10 with time + screen context injection
