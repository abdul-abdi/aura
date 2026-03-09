# Aura v2 Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Redesign Aura into a macOS menu bar companion with dynamic AppleScript generation, witty personality, Cloud Run proxy, session memory, and wake word activation for the Gemini Live Agent Challenge (deadline: March 16, 2026).

**Architecture:** Native macOS menu bar app (NSStatusItem + NSPopover) using `cocoa`/`objc` crates. Gemini Live API streams audio through a Cloud Run WebSocket proxy. Instead of hardcoded tools, Gemini writes AppleScript on-the-fly via two tools: `run_applescript` and `get_screen_context`. Local SQLite stores conversation history per session.

**Tech Stack:** Rust 2024, tokio, cocoa/objc (Cocoa bindings), rusqlite (SQLite), axum + tokio-tungstenite (proxy), cpal (audio capture), rodio (playback), rustpotter (wake word), osascript (AppleScript execution), core-graphics (screen context)

---

## Task 1: Rewrite aura-bridge — Dynamic AppleScript Executor

Replace the hardcoded `Action` enum and `MacOSExecutor` with a general-purpose `ScriptExecutor` that runs arbitrary AppleScript/JXA code via `osascript`.

**Files:**
- Modify: `crates/aura-bridge/src/lib.rs`
- Rewrite: `crates/aura-bridge/src/actions.rs` → rename to `crates/aura-bridge/src/script.rs`
- Rewrite: `crates/aura-bridge/src/macos.rs`
- Create: `crates/aura-bridge/tests/script_test.rs`
- Modify: `crates/aura-bridge/Cargo.toml`

**Step 1: Write failing tests for ScriptExecutor**

Create `crates/aura-bridge/tests/script_test.rs`:

```rust
use aura_bridge::script::{ScriptExecutor, ScriptLanguage, ScriptResult};

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
        .run("delay 60", ScriptLanguage::AppleScript, 1)
        .await;
    assert!(!result.success);
    assert!(result.stderr.contains("timed out") || result.stderr.contains("timeout"));
}
```

**Step 2: Run tests to verify they fail**

Run: `cargo test -p aura-bridge --test script_test 2>&1 | head -20`
Expected: Compilation error — `script` module doesn't exist yet.

**Step 3: Implement ScriptExecutor**

Delete `crates/aura-bridge/src/actions.rs` and `crates/aura-bridge/src/macos.rs`. Create `crates/aura-bridge/src/script.rs`:

```rust
use std::process::Command;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::time::timeout;

/// Blocked shell patterns — these are never allowed inside `do shell script`.
const BLOCKED_SHELL_PATTERNS: &[&str] = &[
    "rm -rf", "rm -r", "rmdir", "sudo", "mkfs", "dd if=",
    "chmod 777", ":(){ :|:", "fork bomb", "> /dev/sd",
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
            cmd.output()
        });

        match timeout(Duration::from_secs(timeout_secs), handle).await {
            Ok(Ok(Ok(output))) => ScriptResult {
                success: output.status.success(),
                stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
                stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            },
            Ok(Ok(Err(e))) => ScriptResult {
                success: false,
                stdout: String::new(),
                stderr: format!("Failed to execute osascript: {e}"),
            },
            Ok(Err(e)) => ScriptResult {
                success: false,
                stdout: String::new(),
                stderr: format!("Script task panicked: {e}"),
            },
            Err(_) => ScriptResult {
                success: false,
                stdout: String::new(),
                stderr: "Script timed out".into(),
            },
        }
    }
}

/// Check if script contains dangerous patterns. Returns reason if blocked.
fn check_dangerous(script: &str) -> Option<String> {
    let lower = script.to_lowercase();

    // Check for dangerous shell commands inside do shell script
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

Update `crates/aura-bridge/src/lib.rs`:

```rust
//! Aura OS bridge: dynamic AppleScript execution and screen context

pub mod script;
```

Remove the `async-trait` dependency from `crates/aura-bridge/Cargo.toml` (no longer needed). The `test-support` feature can stay for now but the mock executor is gone.

Updated `Cargo.toml`:

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

**Step 4: Run tests to verify they pass**

Run: `cargo test -p aura-bridge --test script_test -- --nocapture`
Expected: All 5 tests pass. The timeout test may take ~1-2 seconds.

**Step 5: Fix compilation in dependent crates**

After deleting actions.rs and macos.rs, other crates that import them will fail. For now, just make `aura-bridge` compile. We'll fix dependents in later tasks.

Run: `cargo check -p aura-bridge`
Expected: OK

**Step 6: Commit**

```bash
git add crates/aura-bridge/
git commit -m "feat: rewrite aura-bridge with dynamic AppleScript executor"
```

---

## Task 2: Implement Screen Context (aura-screen)

Complete the stub `MacOSScreenReader` to actually parse CGWindowList, get clipboard contents, and detect the frontmost application. Add clipboard to `ScreenContext`.

**Files:**
- Modify: `crates/aura-screen/src/context.rs`
- Rewrite: `crates/aura-screen/src/macos.rs`
- Create: `crates/aura-screen/tests/context_test.rs`
- Modify: `crates/aura-screen/Cargo.toml`

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

#[cfg(target_os = "macos")]
#[test]
fn test_capture_context_returns_something() {
    let reader = aura_screen::macos::MacOSScreenReader::new().unwrap();
    let ctx = reader.capture_context().unwrap();
    // Should at least have the frontmost app
    assert!(
        !ctx.frontmost_app().is_empty(),
        "Should detect frontmost app"
    );
}

#[cfg(target_os = "macos")]
#[test]
fn test_capture_clipboard() {
    use std::process::Command;
    // Set clipboard to known value
    Command::new("pbcopy")
        .stdin(std::process::Stdio::piped())
        .spawn()
        .unwrap()
        .stdin
        .unwrap()
        .write_all(b"test_aura_clip")
        .unwrap();

    std::thread::sleep(std::time::Duration::from_millis(100));

    let reader = aura_screen::macos::MacOSScreenReader::new().unwrap();
    let ctx = reader.capture_context().unwrap();
    if let Some(clip) = ctx.clipboard() {
        assert!(clip.contains("test_aura_clip"));
    }
}
```

**Step 2: Run tests to verify they fail**

Run: `cargo test -p aura-screen --test context_test 2>&1 | head -20`
Expected: Compilation errors — `new_with_details`, `frontmost_app`, `clipboard` don't exist.

**Step 3: Update ScreenContext with new fields**

Rewrite `crates/aura-screen/src/context.rs`:

```rust
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScreenContext {
    pub frontmost_app: String,
    pub frontmost_title: Option<String>,
    pub open_windows: Vec<String>,
    pub clipboard: Option<String>,
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

**Step 4: Implement MacOSScreenReader with real CGWindowList + pbpaste**

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

fn get_frontmost_app() -> Option<String> {
    let output = Command::new("osascript")
        .arg("-e")
        .arg(r#"tell application "System Events" to get name of first application process whose frontmost is true"#)
        .output()
        .ok()?;
    if output.status.success() {
        Some(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        None
    }
}

fn get_frontmost_title() -> Option<String> {
    let output = Command::new("osascript")
        .arg("-e")
        .arg(r#"tell application "System Events"
            set frontApp to first application process whose frontmost is true
            tell frontApp
                if (count of windows) > 0 then
                    return name of window 1
                else
                    return ""
                end if
            end tell
        end tell"#)
        .output()
        .ok()?;
    if output.status.success() {
        let title = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if title.is_empty() { None } else { Some(title) }
    } else {
        None
    }
}

fn get_open_windows() -> Option<Vec<String>> {
    let output = Command::new("osascript")
        .arg("-e")
        .arg(r#"tell application "System Events"
            set windowList to {}
            repeat with proc in (every application process whose visible is true)
                repeat with win in (every window of proc)
                    set end of windowList to (name of proc) & " - " & (name of win)
                end repeat
            end repeat
            set text item delimiters to linefeed
            return windowList as text
        end tell"#)
        .output()
        .ok()?;
    if output.status.success() {
        let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if text.is_empty() {
            Some(Vec::new())
        } else {
            Some(text.lines().map(String::from).collect())
        }
    } else {
        None
    }
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

Add `serde_json` to `crates/aura-screen/Cargo.toml` dev-dependencies:

```toml
[dev-dependencies]
serde_json.workspace = true
```

**Step 5: Run tests**

Run: `cargo test -p aura-screen --test context_test -- --nocapture`
Expected: All tests pass (on macOS).

**Step 6: Commit**

```bash
git add crates/aura-screen/
git commit -m "feat: implement screen context with frontmost app, windows, and clipboard"
```

---

## Task 3: Rewrite Gemini Tools — Two Dynamic Tools

Replace the 5 hardcoded tool declarations with `run_applescript` and `get_screen_context`.

**Files:**
- Rewrite: `crates/aura-gemini/src/tools.rs`
- Modify: `crates/aura-gemini/Cargo.toml` (remove `aura-bridge` dependency)
- Modify: `crates/aura-gemini/src/lib.rs`

**Step 1: Write failing tests**

Add tests at the bottom of the new `tools.rs` (we'll write the module in step 3):

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tool_declarations_returns_two_tools() {
        let tools = tool_declarations();
        assert_eq!(tools.len(), 2);
    }

    #[test]
    fn test_tool_names() {
        let tools = tool_declarations();
        let names: Vec<&str> = tools
            .iter()
            .map(|t| t["name"].as_str().unwrap())
            .collect();
        assert!(names.contains(&"run_applescript"));
        assert!(names.contains(&"get_screen_context"));
    }

    #[test]
    fn test_run_applescript_has_script_param() {
        let tools = tool_declarations();
        let run = tools.iter().find(|t| t["name"] == "run_applescript").unwrap();
        let params = &run["parameters"]["properties"];
        assert!(params.get("script").is_some());
        assert!(params.get("language").is_some());
        assert!(params.get("timeout_secs").is_some());
    }

    #[test]
    fn test_get_screen_context_has_no_required_params() {
        let tools = tool_declarations();
        let ctx = tools.iter().find(|t| t["name"] == "get_screen_context").unwrap();
        let required = ctx["parameters"].get("required");
        assert!(required.is_none() || required.unwrap().as_array().unwrap().is_empty());
    }
}
```

**Step 2: Run tests to verify they fail**

Run: `cargo test -p aura-gemini tools::tests -- 2>&1 | head -20`
Expected: Compilation error — old tools.rs doesn't have `tool_declarations()`.

**Step 3: Rewrite tools.rs**

Replace `crates/aura-gemini/src/tools.rs` entirely:

```rust
use serde_json::{json, Value};

/// Returns the two dynamic tool declarations for Gemini function calling.
pub fn tool_declarations() -> Vec<Value> {
    vec![
        json!({
            "name": "run_applescript",
            "description": "Execute AppleScript or JXA code to control any macOS application or system feature. You can open apps, manage windows, interact with UI elements, automate workflows, manipulate files, control system settings, send keystrokes, and more. Write the script based on what the user needs. Prefer simple scripts — chain multiple calls over one complex script.",
            "parameters": {
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
            }
        }),
        json!({
            "name": "get_screen_context",
            "description": "Get the user's current screen context: frontmost application, window title, list of open windows, and clipboard contents. Always call this before taking action so you understand what the user is currently doing.",
            "parameters": {
                "type": "object",
                "properties": {}
            }
        }),
    ]
}

// Tests included inline — see Step 1
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tool_declarations_returns_two_tools() {
        let tools = tool_declarations();
        assert_eq!(tools.len(), 2);
    }

    #[test]
    fn test_tool_names() {
        let tools = tool_declarations();
        let names: Vec<&str> = tools
            .iter()
            .map(|t| t["name"].as_str().unwrap())
            .collect();
        assert!(names.contains(&"run_applescript"));
        assert!(names.contains(&"get_screen_context"));
    }

    #[test]
    fn test_run_applescript_has_script_param() {
        let tools = tool_declarations();
        let run = tools.iter().find(|t| t["name"] == "run_applescript").unwrap();
        let params = &run["parameters"]["properties"];
        assert!(params.get("script").is_some());
        assert!(params.get("language").is_some());
        assert!(params.get("timeout_secs").is_some());
    }

    #[test]
    fn test_get_screen_context_has_no_required_params() {
        let tools = tool_declarations();
        let ctx = tools.iter().find(|t| t["name"] == "get_screen_context").unwrap();
        let required = ctx["parameters"].get("required");
        assert!(required.is_none() || required.unwrap().as_array().unwrap().is_empty());
    }
}
```

Remove `aura-bridge` from `crates/aura-gemini/Cargo.toml` dependencies (no longer needed — tools.rs no longer imports Action):

```toml
[dependencies]
tokio.workspace = true
tracing.workspace = true
anyhow.workspace = true
serde.workspace = true
serde_json.workspace = true
tokio-tungstenite = { version = "0.24", features = ["native-tls"] }
base64 = "0.22"
futures-util = "0.3"
tokio-util = "0.7"
```

**Step 4: Update protocol.rs to use new tool declarations**

In `crates/aura-gemini/src/protocol.rs`, find where `build_tool_declarations()` or the old tools are referenced in the setup message and replace with `tools::tool_declarations()`. Specifically, the `SetupMessage` construction should use the new 2-tool list.

Search for usage:
```
grep -n "tool_declarations\|build_tool_declarations\|function_declarations" crates/aura-gemini/src/
```

Update accordingly — the setup message's `tools` field should contain:
```json
{ "function_declarations": tool_declarations() }
```

**Step 5: Run tests**

Run: `cargo test -p aura-gemini -- --nocapture`
Expected: All gemini tests pass (tools tests + existing session/protocol tests).

**Step 6: Commit**

```bash
git add crates/aura-gemini/
git commit -m "feat: replace hardcoded tools with dynamic run_applescript and get_screen_context"
```

---

## Task 4: Update System Prompt — Aura Personality

Update the default system prompt in `GeminiConfig` with the Aura personality.

**Files:**
- Modify: `crates/aura-gemini/src/config.rs`

**Step 1: Write failing test**

Add to existing tests in `config.rs`:

```rust
#[test]
fn test_system_prompt_contains_personality() {
    let config = GeminiConfig::from_env_inner("test-key-12345");
    assert!(config.system_prompt.contains("Aura"));
    assert!(config.system_prompt.contains("witty"));
    assert!(config.system_prompt.contains("run_applescript"));
    assert!(config.system_prompt.contains("get_screen_context"));
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p aura-gemini config::tests::test_system_prompt_contains_personality`
Expected: FAIL — current prompt doesn't mention Aura or witty.

**Step 3: Update system prompt**

In `crates/aura-gemini/src/config.rs`, replace the `system_prompt` default value in the constructor with:

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

**Step 4: Run tests**

Run: `cargo test -p aura-gemini config::tests`
Expected: All pass.

**Step 5: Commit**

```bash
git add crates/aura-gemini/src/config.rs
git commit -m "feat: add Aura personality system prompt"
```

---

## Task 5: Create aura-menubar Crate — NSStatusItem

Create the new `aura-menubar` crate with a native macOS menu bar status item (dot icon) using the `cocoa` and `objc` crates.

**Files:**
- Create: `crates/aura-menubar/Cargo.toml`
- Create: `crates/aura-menubar/src/lib.rs`
- Create: `crates/aura-menubar/src/status_item.rs`
- Create: `crates/aura-menubar/src/popover.rs`
- Create: `crates/aura-menubar/src/app.rs`
- Modify: `Cargo.toml` (workspace members)

**Step 1: Set up crate structure**

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

**Step 2: Create status_item.rs — NSStatusItem with dot icon**

Create `crates/aura-menubar/src/status_item.rs`:

```rust
use cocoa::appkit::{
    NSApp, NSApplication, NSApplicationActivationPolicyAccessory,
    NSImage, NSStatusBar, NSStatusItem, NSVariableStatusItemLength,
};
use cocoa::base::{id, nil, YES, NO};
use cocoa::foundation::{NSAutoreleasePool, NSSize, NSString, NSRect, NSPoint};
use objc::runtime::{Object, Sel};
use objc::{class, msg_send, sel, sel_impl};

use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Arc;

/// Status item colors representing Aura's state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum DotColor {
    Gray = 0,       // Disconnected
    Green = 1,      // Connected / listening
    Amber = 2,      // Processing
    Red = 3,        // Error
}

/// Handle to the macOS menu bar status item.
pub struct AuraStatusItem {
    status_item: id,
    color: Arc<AtomicU8>,
}

unsafe impl Send for AuraStatusItem {}

impl AuraStatusItem {
    /// Create a new status item in the system menu bar.
    /// MUST be called on the main thread.
    pub unsafe fn new() -> Self {
        let status_bar: id = msg_send![class!(NSStatusBar), systemStatusBar];
        let status_item: id = msg_send![status_bar, statusItemWithLength: NSVariableStatusItemLength];
        let _: () = msg_send![status_item, retain];

        let color = Arc::new(AtomicU8::new(DotColor::Gray as u8));

        let item = Self { status_item, color };
        item.update_icon(DotColor::Gray);
        item
    }

    /// Update the dot color.
    pub unsafe fn set_color(&self, color: DotColor) {
        self.color.store(color as u8, Ordering::Relaxed);
        self.update_icon(color);
    }

    /// Get current color.
    pub fn current_color(&self) -> DotColor {
        match self.color.load(Ordering::Relaxed) {
            1 => DotColor::Green,
            2 => DotColor::Amber,
            3 => DotColor::Red,
            _ => DotColor::Gray,
        }
    }

    /// Draw a colored dot as the status item icon.
    unsafe fn update_icon(&self, color: DotColor) {
        let size = NSSize::new(18.0, 18.0);

        let image: id = msg_send![class!(NSImage), alloc];
        let image: id = msg_send![image, initWithSize: size];
        let _: () = msg_send![image, lockFocus];

        // Draw circle
        let (r, g, b) = match color {
            DotColor::Gray => (0.6, 0.6, 0.6),
            DotColor::Green => (0.3, 0.9, 0.5),
            DotColor::Amber => (1.0, 0.8, 0.3),
            DotColor::Red => (0.9, 0.3, 0.3),
        };

        let ns_color: id = msg_send![class!(NSColor),
            colorWithRed: r
            green: g
            blue: b
            alpha: 1.0f64
        ];
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

    /// Get the raw status item for attaching click handlers.
    pub fn raw(&self) -> id {
        self.status_item
    }
}
```

**Step 3: Create popover.rs — NSPopover with conversation view**

Create `crates/aura-menubar/src/popover.rs`:

```rust
use cocoa::appkit::{NSView, NSTextField, NSFont};
use cocoa::base::{id, nil, YES, NO};
use cocoa::foundation::{NSString, NSRect, NSPoint, NSSize};
use objc::{class, msg_send, sel, sel_impl};

/// The popover panel that appears when clicking the menu bar dot.
pub struct AuraPopover {
    popover: id,
    content_view: id,
}

unsafe impl Send for AuraPopover {}

impl AuraPopover {
    /// Create a new popover. Must be called on main thread.
    pub unsafe fn new() -> Self {
        let popover: id = msg_send![class!(NSPopover), alloc];
        let popover: id = msg_send![popover, init];

        // Set size
        let _: () = msg_send![popover, setContentSize: NSSize::new(320.0, 480.0)];
        let _: () = msg_send![popover, setBehavior: 1i64]; // NSPopoverBehaviorTransient

        // Create content view controller
        let vc: id = msg_send![class!(NSViewController), alloc];
        let vc: id = msg_send![vc, init];

        let content_view: id = msg_send![class!(NSView), alloc];
        let frame = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(320.0, 480.0));
        let content_view: id = msg_send![content_view, initWithFrame: frame];

        // Dark background
        let _: () = msg_send![content_view, setWantsLayer: YES];
        let layer: id = msg_send![content_view, layer];
        let bg_color: id = msg_send![class!(NSColor),
            colorWithRed: 0.1f64
            green: 0.1f64
            blue: 0.12f64
            alpha: 1.0f64
        ];
        let cg_color: id = msg_send![bg_color, CGColor];
        let _: () = msg_send![layer, setBackgroundColor: cg_color];

        let _: () = msg_send![vc, setView: content_view];
        let _: () = msg_send![popover, setContentViewController: vc];

        Self {
            popover,
            content_view,
        }
    }

    /// Show the popover relative to the status item button.
    pub unsafe fn show(&self, relative_to: id) {
        let bounds: NSRect = msg_send![relative_to, bounds];
        let _: () = msg_send![self.popover, showRelativeToRect: bounds
            ofView: relative_to
            preferredEdge: 1u64]; // NSMinYEdge (below)
    }

    /// Close the popover.
    pub unsafe fn close(&self) {
        let shown: bool = msg_send![self.popover, isShown];
        if shown {
            let _: () = msg_send![self.popover, close];
        }
    }

    /// Toggle visibility.
    pub unsafe fn toggle(&self, relative_to: id) {
        let shown: bool = msg_send![self.popover, isShown];
        if shown {
            self.close();
        } else {
            self.show(relative_to);
        }
    }

    /// Add a text label to the popover (for conversation messages).
    pub unsafe fn add_message(&self, text: &str, is_user: bool) {
        let subviews: id = msg_send![self.content_view, subviews];
        let count: usize = msg_send![subviews, count];
        let y_offset = 480.0 - (count as f64 + 1.0) * 40.0;

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

**Step 4: Create app.rs — Main menu bar app runner**

Create `crates/aura-menubar/src/app.rs`:

```rust
use cocoa::appkit::{
    NSApp, NSApplication, NSApplicationActivationPolicyAccessory,
};
use cocoa::base::{id, nil};
use cocoa::foundation::NSAutoreleasePool;
use objc::declare::ClassDecl;
use objc::runtime::{Object, Sel, BOOL};
use objc::{class, msg_send, sel, sel_impl};

use std::cell::RefCell;
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;

use crate::popover::AuraPopover;
use crate::status_item::{AuraStatusItem, DotColor};

/// Messages from the async runtime to the menu bar UI.
#[derive(Debug, Clone)]
pub enum MenuBarMessage {
    SetColor(DotColor),
    AddMessage { text: String, is_user: bool },
    Shutdown,
}

/// The menu bar application.
pub struct MenuBarApp {
    pub message_tx: mpsc::Sender<MenuBarMessage>,
    message_rx: mpsc::Receiver<MenuBarMessage>,
}

impl MenuBarApp {
    pub fn new() -> (Self, mpsc::Sender<MenuBarMessage>) {
        let (tx, rx) = mpsc::channel(64);
        let tx_clone = tx.clone();
        (
            Self {
                message_tx: tx,
                message_rx: rx,
            },
            tx_clone,
        )
    }

    /// Run the menu bar app on the main thread. This blocks forever.
    /// Call from main() — Cocoa requires the main thread.
    pub fn run(self) {
        unsafe {
            let _pool = NSAutoreleasePool::new(nil);
            let app = NSApp();
            let _: () = msg_send![app, setActivationPolicy:
                NSApplicationActivationPolicyAccessory];

            let status_item = AuraStatusItem::new();
            let popover = AuraPopover::new();

            // Set up click handler using a target-action pattern
            // For simplicity, we'll use a timer to poll messages
            // A production app would use proper Cocoa run loop integration

            let button: id = msg_send![status_item.raw(), button];

            // Store in a static for the click callback
            // (Proper implementation would use objc class delegation)

            tracing::info!("Menu bar app running");

            // TODO: Wire click handler + message polling into NSRunLoop
            // For now, this starts the Cocoa run loop
            let _: () = msg_send![app, run];
        }
    }
}
```

**Step 5: Create lib.rs**

Create `crates/aura-menubar/src/lib.rs`:

```rust
//! Aura menu bar: native macOS status item and popover UI

pub mod app;
pub mod popover;
pub mod status_item;
```

**Step 6: Verify compilation**

Run: `cargo check -p aura-menubar`
Expected: Compiles (may have warnings about unused variables — that's fine).

**Step 7: Commit**

```bash
git add crates/aura-menubar/ Cargo.toml
git commit -m "feat: create aura-menubar crate with NSStatusItem and NSPopover"
```

---

## Task 6: Add Session Memory — SQLite Storage

Create a `memory` module in `aura-daemon` for local session persistence using SQLite.

**Files:**
- Create: `crates/aura-daemon/src/memory.rs`
- Modify: `crates/aura-daemon/src/lib.rs`
- Modify: `crates/aura-daemon/Cargo.toml`
- Create: `crates/aura-daemon/tests/memory_test.rs`

**Step 1: Add rusqlite dependency**

Add to `crates/aura-daemon/Cargo.toml`:

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
    assert_eq!(sessions[0].id, s2); // Most recent first
}
```

**Step 3: Run tests to verify they fail**

Run: `cargo test -p aura-daemon --test memory_test 2>&1 | head -20`
Expected: Compilation error — `memory` module doesn't exist.

**Step 4: Implement SessionMemory**

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
            "user" => Self::User,
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

**Step 5: Run tests**

Run: `cargo test -p aura-daemon --test memory_test -- --nocapture`
Expected: All 4 tests pass.

**Step 6: Commit**

```bash
git add crates/aura-daemon/
git commit -m "feat: add SQLite session memory for conversation persistence"
```

---

## Task 7: Create aura-proxy Crate — Cloud Run WebSocket Proxy

Create the Cloud Run WebSocket proxy that relays audio/messages between the Mac client and Gemini Live API.

**Files:**
- Create: `crates/aura-proxy/Cargo.toml`
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
uuid = { version = "1", features = ["v4"] }

[dev-dependencies]
reqwest = { version = "0.12", features = ["json"] }
```

Add `"crates/aura-proxy"` to workspace members in root `Cargo.toml`.

**Step 2: Write failing tests**

Create `crates/aura-proxy/tests/proxy_test.rs`:

```rust
use std::net::TcpListener;

#[tokio::test]
async fn test_health_endpoint() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);

    let handle = tokio::spawn(async move {
        aura_proxy::run_server(port).await.unwrap();
    });

    // Give server time to start
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;

    let resp = reqwest::get(format!("http://127.0.0.1:{port}/health"))
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["status"], "ok");

    handle.abort();
}
```

**Step 3: Run tests to verify they fail**

Run: `cargo test -p aura-proxy --test proxy_test 2>&1 | head -20`
Expected: Compilation error — `aura_proxy::run_server` doesn't exist.

**Step 4: Implement relay.rs**

Create `crates/aura-proxy/src/relay.rs`:

```rust
use anyhow::Result;
use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::{connect_async, tungstenite::Message};

/// Relay WebSocket frames between client and Gemini.
pub async fn relay_websocket(
    client_ws: axum::extract::ws::WebSocket,
    gemini_url: String,
) {
    let (gemini_ws, _) = match connect_async(&gemini_url).await {
        Ok(conn) => conn,
        Err(e) => {
            tracing::error!("Failed to connect to Gemini: {e}");
            return;
        }
    };

    let (mut client_tx, mut client_rx) = client_ws.split();
    let (mut gemini_tx, mut gemini_rx) = gemini_ws.split();

    // Client → Gemini
    let client_to_gemini = async {
        while let Some(Ok(msg)) = client_rx.next().await {
            let tung_msg = match msg {
                axum::extract::ws::Message::Text(t) => Message::Text(t),
                axum::extract::ws::Message::Binary(b) => Message::Binary(b),
                axum::extract::ws::Message::Close(_) => break,
                _ => continue,
            };
            if gemini_tx.send(tung_msg).await.is_err() {
                break;
            }
        }
    };

    // Gemini → Client
    let gemini_to_client = async {
        while let Some(Ok(msg)) = gemini_rx.next().await {
            let axum_msg = match msg {
                Message::Text(t) => axum::extract::ws::Message::Text(t),
                Message::Binary(b) => axum::extract::ws::Message::Binary(b),
                Message::Close(_) => break,
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

**Step 5: Implement main.rs**

Create `crates/aura-proxy/src/main.rs`:

```rust
use anyhow::Result;
use axum::{
    Router,
    extract::{Query, WebSocketUpgrade, ws::WebSocket},
    response::{IntoResponse, Json},
    routing::get,
};
use serde::Deserialize;
use tower_http::cors::CorsLayer;
use tracing_subscriber::EnvFilter;

mod relay;

const DEFAULT_PORT: u16 = 8080;
const GEMINI_WS_BASE: &str = "wss://generativelanguage.googleapis.com/ws/google.ai.generativelanguage.v1alpha.GenerativeService.BidiGenerateContent";

#[derive(Deserialize)]
struct ConnectParams {
    api_key: String,
    #[serde(default = "default_model")]
    model: String,
}

fn default_model() -> String {
    "models/gemini-live-2.5-flash-native-audio".into()
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

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .init();

    let port = std::env::var("PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(DEFAULT_PORT);

    run_server(port).await
}
```

Note: `run_server` must be `pub` and the module must be a lib+bin. Update `Cargo.toml`:

```toml
[lib]
name = "aura_proxy"
path = "src/lib.rs"
```

Create `crates/aura-proxy/src/lib.rs`:

```rust
pub mod relay;

// Re-export run_server for tests
use anyhow::Result;
use axum::{Router, extract::{Query, WebSocketUpgrade}, response::{IntoResponse, Json}, routing::get};
use serde::Deserialize;
use tower_http::cors::CorsLayer;

const GEMINI_WS_BASE: &str = "wss://generativelanguage.googleapis.com/ws/google.ai.generativelanguage.v1alpha.GenerativeService.BidiGenerateContent";

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

Simplify `main.rs` to just call the lib:

```rust
use anyhow::Result;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .init();
    let port = std::env::var("PORT").ok().and_then(|p| p.parse().ok()).unwrap_or(8080);
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
Expected: Health endpoint test passes.

**Step 8: Commit**

```bash
git add crates/aura-proxy/ Cargo.toml
git commit -m "feat: create Cloud Run WebSocket proxy for Gemini Live API"
```

---

## Task 8: Rewrite aura-daemon — Wire New Components

Rewrite `main.rs` to use the menu bar app instead of the overlay, wire dynamic AppleScript execution, screen context, and session memory.

**Files:**
- Rewrite: `crates/aura-daemon/src/main.rs`
- Modify: `crates/aura-daemon/src/event.rs`
- Modify: `crates/aura-daemon/src/daemon.rs`
- Modify: `crates/aura-daemon/src/lib.rs`
- Modify: `crates/aura-daemon/Cargo.toml`

**Step 1: Update Cargo.toml dependencies**

Replace aura-overlay with aura-menubar, add aura-screen:

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

**Step 2: Update event.rs — Remove overlay events, add new events**

Rewrite `crates/aura-daemon/src/event.rs`:

```rust
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AuraEvent {
    // Voice pipeline
    WakeWordDetected,
    ListeningStarted,
    ListeningStopped,

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

**Step 3: Rewrite main.rs — Menu bar + async pipeline**

Rewrite `crates/aura-daemon/src/main.rs`:

```rust
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use anyhow::{Context, Result};
use clap::Parser;
use tokio_util::sync::CancellationToken;

use aura_bridge::script::{ScriptExecutor, ScriptLanguage};
use aura_daemon::bus::EventBus;
use aura_daemon::event::AuraEvent;
use aura_daemon::memory::{MessageRole, SessionMemory};
use aura_daemon::setup::AuraSetup;
use aura_gemini::config::GeminiConfig;
use aura_gemini::session::{GeminiEvent, GeminiLiveSession};
use aura_menubar::app::{MenuBarApp, MenuBarMessage};
use aura_menubar::status_item::DotColor;
use aura_screen::macos::MacOSScreenReader;
use aura_voice::audio::AudioCapture;
use aura_voice::playback::AudioPlayer;
use tracing_subscriber::EnvFilter;

const EVENT_BUS_CAPACITY: usize = 64;
const OUTPUT_SAMPLE_RATE: u32 = 24_000;

#[derive(Parser)]
#[command(name = "aura", about = "Voice-first AI desktop companion")]
struct Cli {
    /// Run in headless mode (no menu bar UI)
    #[arg(long)]
    headless: bool,

    /// Enable verbose logging
    #[arg(short, long)]
    verbose: bool,
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    let filter = if cli.verbose { "debug" } else { "info" };
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(filter)),
        )
        .init();

    let gemini_config = GeminiConfig::from_env()
        .context("GEMINI_API_KEY must be set. Get one at https://aistudio.google.com/apikey")?;
    tracing::info!("Gemini API key validated");

    let setup = AuraSetup::new(AuraSetup::default_data_dir());
    setup.ensure_dirs()?;
    setup.print_status();

    // Initialize session memory
    let aura_dir = dirs::home_dir()
        .unwrap_or_default()
        .join(".aura");
    std::fs::create_dir_all(&aura_dir)?;
    let memory = Arc::new(SessionMemory::open(&aura_dir.join("sessions.db"))?);

    let bus = EventBus::new(EVENT_BUS_CAPACITY);
    let cancel = CancellationToken::new();

    if cli.headless {
        let rt = tokio::runtime::Runtime::new()?;
        rt.block_on(run_daemon(gemini_config, bus, cancel, memory, None))?;
    } else {
        let (menubar_app, menubar_tx) = MenuBarApp::new();

        // Spawn async runtime on background thread
        let bg_bus = bus.clone();
        let bg_cancel = cancel.clone();
        let bg_memory = Arc::clone(&memory);
        let bg_tx = menubar_tx.clone();
        std::thread::spawn(move || {
            let rt = tokio::runtime::Runtime::new().expect("Failed to create tokio runtime");
            rt.block_on(async {
                if let Err(e) = run_daemon(gemini_config, bg_bus, bg_cancel, bg_memory, Some(bg_tx)).await {
                    tracing::error!("Daemon error: {e}");
                }
            });
        });

        // Run menu bar on main thread (Cocoa requirement)
        menubar_app.run();
    }

    Ok(())
}

async fn run_daemon(
    gemini_config: GeminiConfig,
    bus: EventBus,
    cancel: CancellationToken,
    memory: Arc<SessionMemory>,
    menubar_tx: Option<tokio::sync::mpsc::Sender<MenuBarMessage>>,
) -> Result<()> {
    // Start a new session
    let session_id = memory.start_session()?;
    tracing::info!(session_id = %session_id, "Session started");

    // Connect to Gemini
    let session = GeminiLiveSession::connect(gemini_config)
        .await
        .context("Failed to connect Gemini Live session")?;
    let session = Arc::new(session);

    if let Some(ref tx) = menubar_tx {
        let _ = tx.send(MenuBarMessage::SetColor(DotColor::Green)).await;
    }

    // Mic capture
    let (std_tx, std_rx) = std::sync::mpsc::channel::<Vec<f32>>();
    let mic_shutdown = Arc::new(AtomicBool::new(false));
    let capture = AudioCapture::new(None).context("Failed to initialize audio capture")?;
    let mic_shutdown_flag = Arc::clone(&mic_shutdown);
    let mic_thread = std::thread::Builder::new()
        .name("aura-mic-capture".into())
        .spawn(move || {
            let _stream = match capture.start(std_tx) {
                Ok(stream) => stream,
                Err(e) => {
                    tracing::error!("Failed to start audio capture: {e}");
                    return;
                }
            };
            tracing::info!("Mic capture started");
            while !mic_shutdown_flag.load(Ordering::Relaxed) {
                std::thread::park_timeout(Duration::from_millis(500));
            }
            tracing::info!("Mic capture stopped");
        })?;

    // Bridge mic → Gemini
    let audio_session = Arc::clone(&session);
    let audio_cancel = cancel.clone();
    let bridge_shutdown = Arc::clone(&mic_shutdown);
    tokio::spawn(async move {
        let (tokio_tx, mut tokio_rx) = tokio::sync::mpsc::channel::<Vec<f32>>(64);
        let bridge_tx = tokio_tx.clone();
        tokio::task::spawn_blocking(move || {
            loop {
                match std_rx.recv_timeout(Duration::from_millis(200)) {
                    Ok(samples) => {
                        if bridge_tx.blocking_send(samples).is_err() { break; }
                    }
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                        if bridge_shutdown.load(Ordering::Relaxed) { break; }
                    }
                    Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
                }
            }
        });
        loop {
            tokio::select! {
                _ = audio_cancel.cancelled() => break,
                Some(samples) = tokio_rx.recv() => {
                    if let Err(e) = audio_session.send_audio(&samples).await {
                        tracing::warn!("Failed to send audio: {e}");
                        break;
                    }
                }
            }
        }
    });

    // Spawn event processor
    let proc_session = Arc::clone(&session);
    let proc_cancel = cancel.clone();
    let proc_memory = Arc::clone(&memory);
    let proc_sid = session_id.clone();
    let proc_tx = menubar_tx.clone();
    let processor_handle = tokio::spawn(async move {
        if let Err(e) = run_processor(proc_session, proc_cancel, proc_memory, proc_sid, proc_tx).await {
            tracing::error!("Processor error: {e}");
        }
    });

    // Wait for ctrl-c
    tokio::signal::ctrl_c().await?;
    tracing::info!("Shutting down...");

    // End session
    let _ = memory.end_session(&session_id, None);

    cancel.cancel();
    mic_shutdown.store(true, Ordering::Relaxed);
    mic_thread.thread().unpark();
    session.disconnect();
    let _ = processor_handle.await;
    let _ = mic_thread.join();

    if let Some(ref tx) = menubar_tx {
        let _ = tx.send(MenuBarMessage::Shutdown).await;
    }

    Ok(())
}

async fn run_processor(
    session: Arc<GeminiLiveSession>,
    cancel: CancellationToken,
    memory: Arc<SessionMemory>,
    session_id: String,
    menubar_tx: Option<tokio::sync::mpsc::Sender<MenuBarMessage>>,
) -> Result<()> {
    let mut events = session.subscribe();
    let executor = ScriptExecutor::new();
    let screen_reader = MacOSScreenReader::new()?;

    let player = match AudioPlayer::new() {
        Ok(p) => {
            tracing::info!("Audio playback ready");
            Some(p)
        }
        Err(e) => {
            tracing::warn!("Audio playback unavailable: {e}");
            None
        }
    };

    tracing::info!("Gemini event processor running");

    loop {
        tokio::select! {
            _ = cancel.cancelled() => break,
            event = events.recv() => {
                match event {
                    Ok(GeminiEvent::Connected) => {
                        tracing::info!("Gemini connected");
                    }
                    Ok(GeminiEvent::AudioResponse { samples }) => {
                        if let Some(ref p) = player
                            && let Err(e) = p.play(samples, OUTPUT_SAMPLE_RATE)
                        {
                            tracing::error!("Playback failed: {e}");
                        }
                    }
                    Ok(GeminiEvent::ToolCall { id, name, args }) => {
                        tracing::info!(name = %name, "Tool call");

                        // Log tool call
                        let _ = memory.add_message(
                            &session_id,
                            MessageRole::ToolCall,
                            &format!("{name}: {args}"),
                            None,
                        );

                        if let Some(ref tx) = menubar_tx {
                            let _ = tx.send(MenuBarMessage::SetColor(DotColor::Amber)).await;
                        }

                        let response = match name.as_str() {
                            "run_applescript" => {
                                let script = args.get("script")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("");
                                let language = match args.get("language").and_then(|v| v.as_str()) {
                                    Some("javascript") => ScriptLanguage::JavaScript,
                                    _ => ScriptLanguage::AppleScript,
                                };
                                let timeout = args.get("timeout_secs")
                                    .and_then(|v| v.as_u64())
                                    .unwrap_or(30);

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
                                        "error": format!("Screen context failed: {e}"),
                                    }),
                                }
                            }
                            other => {
                                serde_json::json!({
                                    "success": false,
                                    "error": format!("Unknown tool: {other}"),
                                })
                            }
                        };

                        // Log tool result
                        let _ = memory.add_message(
                            &session_id,
                            MessageRole::ToolResult,
                            &response.to_string(),
                            None,
                        );

                        if let Some(ref tx) = menubar_tx {
                            let _ = tx.send(MenuBarMessage::SetColor(DotColor::Green)).await;
                        }

                        if let Err(e) = session.send_tool_response(id, name, response).await {
                            tracing::error!("Failed to send tool response: {e}");
                        }
                    }
                    Ok(GeminiEvent::Interrupted) => {
                        tracing::info!("Interrupted — stopping playback");
                        if let Some(ref p) = player { p.stop(); }
                    }
                    Ok(GeminiEvent::Transcription { text }) => {
                        tracing::info!(text = %text, "Transcription");
                        let _ = memory.add_message(
                            &session_id,
                            MessageRole::Assistant,
                            &text,
                            None,
                        );
                        if let Some(ref tx) = menubar_tx {
                            let _ = tx.send(MenuBarMessage::AddMessage {
                                text: text.clone(),
                                is_user: false,
                            }).await;
                        }
                    }
                    Ok(GeminiEvent::TurnComplete) => {
                        tracing::debug!("Turn complete");
                    }
                    Ok(GeminiEvent::Error { message }) => {
                        tracing::error!(%message, "Gemini error");
                        if let Some(ref tx) = menubar_tx {
                            let _ = tx.send(MenuBarMessage::SetColor(DotColor::Red)).await;
                        }
                    }
                    Ok(GeminiEvent::Reconnecting { attempt }) => {
                        tracing::warn!(attempt, "Reconnecting");
                        if let Some(ref tx) = menubar_tx {
                            let _ = tx.send(MenuBarMessage::SetColor(DotColor::Amber)).await;
                        }
                    }
                    Ok(GeminiEvent::Disconnected) => {
                        tracing::info!("Disconnected");
                        if let Some(ref tx) = menubar_tx {
                            let _ = tx.send(MenuBarMessage::SetColor(DotColor::Gray)).await;
                        }
                        break;
                    }
                    Ok(GeminiEvent::ToolCallCancellation { ids }) => {
                        tracing::info!(?ids, "Tool calls cancelled");
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        tracing::warn!("Lagged by {n} events");
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                }
            }
        }
    }

    Ok(())
}
```

**Step 4: Simplify daemon.rs**

The `Daemon` struct and its event loop are replaced by the new `run_daemon` in main.rs. Simplify `daemon.rs` or remove it. If other code depends on it, keep it as a thin wrapper.

**Step 5: Verify compilation**

Run: `cargo check -p aura-daemon`
Expected: Compiles (fix any import errors as they arise).

**Step 6: Commit**

```bash
git add crates/aura-daemon/
git commit -m "feat: rewrite daemon with menu bar, dynamic AppleScript, and session memory"
```

---

## Task 9: Wake Word Materialization

Wire the wake word detector into the daemon pipeline so "Hey Aura" triggers: dot animation → screen context → context-aware greeting.

**Files:**
- Modify: `crates/aura-daemon/src/main.rs` (add wake word thread)
- Modify: `crates/aura-voice/src/wakeword.rs` (ensure it works with current audio pipeline)

**Step 1: Write failing test for wake word integration**

Create `crates/aura-daemon/tests/wakeword_integration_test.rs`:

```rust
// Integration test: verify that WakeWordDetector can be instantiated
// and processes silence without false positives.

#[test]
fn test_wakeword_detector_no_false_positive_on_silence() {
    // Skip if model file doesn't exist (CI environment)
    let model_path = dirs::data_local_dir()
        .unwrap_or_default()
        .join("aura/models/hey-aura.rpw");
    if !model_path.exists() {
        eprintln!("Skipping: wake word model not found at {}", model_path.display());
        return;
    }

    let detector = aura_voice::wakeword::WakeWordDetector::new(&model_path, 0.5, 0.5).unwrap();
    let silence = vec![0.0f32; 16000]; // 1 second of silence
    let detected = detector.process(&silence);
    assert!(!detected, "Should not detect wake word in silence");
}
```

**Step 2: Add wake word thread to daemon**

In `run_daemon()` in `main.rs`, after mic capture setup, add a wake word detection path:

```rust
// Wake word detection (optional — skip if model not found)
let wakeword_model = aura_dir.join("models/hey-aura.rpw");
if wakeword_model.exists() {
    let ww_cancel = cancel.clone();
    let ww_session = Arc::clone(&session);
    let ww_tx = menubar_tx.clone();
    let ww_memory = Arc::clone(&memory);
    let ww_sid = session_id.clone();

    // Wake word gets a copy of mic audio via a second channel
    tracing::info!("Wake word detection enabled");
    // Note: In the full implementation, fork the mic audio to both
    // Gemini and the wake word detector. For MVP, Gemini Live handles
    // voice activity detection natively, so wake word is for the
    // materialization UX only.
} else {
    tracing::info!("Wake word model not found — skipping (Gemini handles VAD)");
}
```

The key insight: Gemini Live API already handles voice activity detection. The wake word is primarily for the *materialization UX* — the dot appearing with a context-aware greeting. For MVP, we can trigger this on first audio detected by Gemini (GeminiEvent::Connected).

**Step 3: Add greeting generation on connect**

In the processor, when `GeminiEvent::Connected` is received, automatically call `get_screen_context` and generate a greeting:

```rust
Ok(GeminiEvent::Connected) => {
    tracing::info!("Gemini connected");
    // Gather context for greeting
    if let Ok(ctx) = screen_reader.capture_context() {
        tracing::info!(context = %ctx.summary(), "Screen context for greeting");
    }
    if let Some(ref tx) = menubar_tx {
        let _ = tx.send(MenuBarMessage::SetColor(DotColor::Green)).await;
    }
}
```

**Step 4: Test and commit**

Run: `cargo check -p aura-daemon`
Expected: Compiles.

```bash
git add crates/aura-daemon/ crates/aura-voice/
git commit -m "feat: wire wake word detection and context-aware greeting"
```

---

## Task 10: Remove Old Overlay Code

Remove `aura-overlay` from the workspace and clean up all references.

**Files:**
- Modify: `Cargo.toml` (remove aura-overlay from workspace members)
- Delete references in: `crates/aura-daemon/Cargo.toml`, `crates/aura-daemon/src/main.rs`
- Optionally: keep `crates/aura-overlay/` directory but remove from workspace

**Step 1: Remove aura-overlay from workspace members**

In root `Cargo.toml`, remove `"crates/aura-overlay"` from the `members` array.

**Step 2: Remove aura-overlay dependency from daemon**

In `crates/aura-daemon/Cargo.toml`, remove the `aura-overlay` dependency line. Remove any `use aura_overlay::*` imports from daemon source files.

**Step 3: Verify workspace compiles**

Run: `cargo check --workspace`
Expected: All crates compile without aura-overlay.

**Step 4: Commit**

```bash
git add Cargo.toml crates/aura-daemon/
git commit -m "chore: remove aura-overlay from workspace (replaced by aura-menubar)"
```

---

## Task 11: Update GeminiConfig for Proxy URL Support

Allow the daemon to connect through the Cloud Run proxy instead of directly to Gemini.

**Files:**
- Modify: `crates/aura-gemini/src/config.rs`

**Step 1: Write failing test**

```rust
#[test]
fn test_proxy_url_override() {
    let mut config = GeminiConfig::from_env_inner("test-key");
    config.set_proxy_url("wss://aura-proxy-xyz.run.app/ws");
    let url = config.ws_url();
    assert!(url.starts_with("wss://aura-proxy-xyz.run.app/ws"));
    assert!(url.contains("api_key=test-key"));
}
```

**Step 2: Implement proxy URL support**

Add a `proxy_url: Option<String>` field to `GeminiConfig` and modify `ws_url()` to use it when set:

```rust
pub fn set_proxy_url(&mut self, url: &str) {
    self.proxy_url = Some(url.to_string());
}

pub fn ws_url(&self) -> String {
    if let Some(ref proxy) = self.proxy_url {
        format!("{proxy}?api_key={}", self.api_key)
    } else {
        format!(
            "wss://generativelanguage.googleapis.com/ws/google.ai.generativelanguage.v1alpha.GenerativeService.BidiGenerateContent?key={}",
            self.api_key
        )
    }
}
```

**Step 3: Run tests and commit**

Run: `cargo test -p aura-gemini config::tests`

```bash
git add crates/aura-gemini/src/config.rs
git commit -m "feat: add proxy URL support to GeminiConfig"
```

---

## Task 12: Competition Deliverables

Prepare the submission artifacts: architecture diagram, README update, deploy script.

**Files:**
- Create: `docs/architecture.md` (ASCII diagram)
- Modify: `README.md`
- Create: `deploy.sh` (Cloud Run deployment script)

**Step 1: Create architecture diagram**

Create `docs/architecture.md`:

```markdown
# Aura Architecture

```
┌─────────────────────────────────────────────────────────┐
│                    macOS Client                          │
│                                                         │
│  ┌──────────┐  ┌──────────┐  ┌──────────┐             │
│  │ Menu Bar │  │   Mic    │  │  Audio   │             │
│  │  (dot)   │  │ Capture  │  │ Playback │             │
│  └────┬─────┘  └────┬─────┘  └────▲─────┘             │
│       │              │              │                    │
│  ┌────▼─────┐        │         ┌────┴─────┐            │
│  │ Popover  │   ┌────▼─────┐   │ Speaker  │            │
│  │   UI     │   │ 16kHz    │   │ 24kHz    │            │
│  └──────────┘   │ PCM f32  │   │ PCM f32  │            │
│                  └────┬─────┘   └────▲─────┘            │
│                       │              │                    │
│  ┌────────────────────▼──────────────┴─────────────────┐│
│  │              aura-daemon (orchestrator)              ││
│  │                                                     ││
│  │  ┌──────────┐  ┌──────────┐  ┌──────────────────┐ ││
│  │  │ Script   │  │ Screen   │  │ Session Memory   │ ││
│  │  │ Executor │  │ Context  │  │ (SQLite)         │ ││
│  │  │osascript │  │CGWindow  │  │~/.aura/sessions  │ ││
│  │  └──────────┘  └──────────┘  └──────────────────┘ ││
│  └──────────────────────┬──────────────────────────────┘│
│                          │ WSS                           │
└──────────────────────────┼───────────────────────────────┘
                           │
                    ┌──────▼──────┐
                    │  Cloud Run  │
                    │   Proxy     │
                    │  (axum)     │
                    └──────┬──────┘
                           │ WSS
                    ┌──────▼──────┐
                    │ Gemini Live │
                    │    API      │
                    │ (streaming  │
                    │  audio +    │
                    │  tools)     │
                    └─────────────┘

Tools:
  run_applescript ──► osascript ──► Any macOS app
  get_screen_context ──► CGWindowList + pbpaste + System Events
```
```

**Step 2: Update README.md**

Update with setup instructions, demo description, and architecture overview. Include:
- What Aura is (1 paragraph)
- Quick start (env var + cargo run)
- Architecture diagram reference
- Competition context
- Demo video link (placeholder)

**Step 3: Create deploy.sh**

```bash
#!/bin/bash
set -euo pipefail

PROJECT_ID="${GCP_PROJECT_ID:?Set GCP_PROJECT_ID}"
REGION="${GCP_REGION:-us-central1}"
SERVICE_NAME="aura-proxy"

echo "Building and deploying aura-proxy to Cloud Run..."

gcloud builds submit \
  --tag "gcr.io/${PROJECT_ID}/${SERVICE_NAME}" \
  --project "${PROJECT_ID}" \
  -f crates/aura-proxy/Dockerfile .

gcloud run deploy "${SERVICE_NAME}" \
  --image "gcr.io/${PROJECT_ID}/${SERVICE_NAME}" \
  --platform managed \
  --region "${REGION}" \
  --allow-unauthenticated \
  --port 8080 \
  --project "${PROJECT_ID}"

echo "Deployed! Service URL:"
gcloud run services describe "${SERVICE_NAME}" \
  --region "${REGION}" \
  --project "${PROJECT_ID}" \
  --format 'value(status.url)'
```

**Step 4: Commit**

```bash
git add docs/architecture.md README.md deploy.sh
git commit -m "docs: add architecture diagram, README, and Cloud Run deploy script"
```

---

## Verification Checklist

After all tasks, run:

```bash
# Full workspace check
cargo check --workspace

# All tests
cargo test --workspace

# Clippy
cargo clippy --workspace -- -D warnings

# Format
cargo fmt --all -- --check

# Specific test suites
cargo test -p aura-bridge --test script_test
cargo test -p aura-screen --test context_test
cargo test -p aura-gemini -- tools::tests
cargo test -p aura-daemon --test memory_test
cargo test -p aura-proxy --test proxy_test
```

All must pass before submission.

---

## Timeline

| Day | Tasks | Focus |
|-----|-------|-------|
| 1 | 1, 2, 3, 4 | Foundation: bridge + tools + personality + screen |
| 2 | 5, 6 | Menu bar + session memory |
| 3 | 7, 11 | Cloud Run proxy + config |
| 4 | 8 | Daemon rewrite (wire everything) |
| 5 | 9, 10 | Wake word + cleanup |
| 6 | 12 | Deliverables: diagram, README, deploy, demo video |
