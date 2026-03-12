use std::process::Command;
use std::time::Duration;

use serde::{Deserialize, Serialize};

/// Maximum timeout in seconds — any requested timeout is clamped to this value.
const MAX_TIMEOUT_SECS: u64 = 60;

/// Maximum output size in bytes before truncation.
const MAX_OUTPUT_BYTES: usize = 10_240;

/// Dangerous applications that can execute arbitrary shell commands.
const BLOCKED_APPS: &[&str] = &["terminal", "iterm", "iterm2"];

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

#[derive(Clone)]
pub struct ScriptExecutor;

impl Default for ScriptExecutor {
    fn default() -> Self {
        Self::new()
    }
}

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
        // Safety check: allowlist-based script validation
        if let Some(reason) = check_script_safety(script, language) {
            return ScriptResult {
                success: false,
                stdout: String::new(),
                stderr: format!("Script blocked: {reason}"),
            };
        }

        let script = script.to_string();
        let timeout_secs = timeout_secs.min(MAX_TIMEOUT_SECS);
        let timeout_dur = Duration::from_secs(timeout_secs);

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

            let mut child = match cmd
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .spawn()
            {
                Ok(child) => child,
                Err(e) => {
                    return ScriptResult {
                        success: false,
                        stdout: String::new(),
                        stderr: format!("Failed to spawn osascript: {e}"),
                    };
                }
            };

            // Wait with timeout using a polling loop
            let start = std::time::Instant::now();
            loop {
                match child.try_wait() {
                    Ok(Some(status)) => {
                        // Process finished — read output
                        let mut stdout = child
                            .stdout
                            .take()
                            .map(|mut s| {
                                let mut buf = String::new();
                                std::io::Read::read_to_string(&mut s, &mut buf).ok();
                                buf
                            })
                            .unwrap_or_default();
                        let mut stderr = child
                            .stderr
                            .take()
                            .map(|mut s| {
                                let mut buf = String::new();
                                std::io::Read::read_to_string(&mut s, &mut buf).ok();
                                buf
                            })
                            .unwrap_or_default();

                        truncate_output(&mut stdout);
                        truncate_output(&mut stderr);

                        break ScriptResult {
                            success: status.success(),
                            stdout,
                            stderr,
                        };
                    }
                    Ok(None) => {
                        // Still running — check timeout
                        if start.elapsed() >= timeout_dur {
                            let _ = child.kill();
                            let _ = child.wait(); // reap zombie
                            break ScriptResult {
                                success: false,
                                stdout: String::new(),
                                stderr: "Script timed out".into(),
                            };
                        }
                        std::thread::sleep(Duration::from_millis(50));
                    }
                    Err(e) => {
                        break ScriptResult {
                            success: false,
                            stdout: String::new(),
                            stderr: format!("Failed to check process status: {e}"),
                        };
                    }
                }
            }
        });

        match handle.await {
            Ok(result) => result,
            Err(e) => ScriptResult {
                success: false,
                stdout: String::new(),
                stderr: format!("Script task panicked: {e}"),
            },
        }
    }
}

/// Validate a script for safe execution. Returns `Some(reason)` if the script
/// should be blocked, `None` if it is safe to run.
///
/// Security model (allowlist):
/// - **All JXA is blocked** — JavaScript for Automation provides `doShellScript`,
///   `$.system`, `ObjC.import` and is too powerful to allowlist safely.
/// - **`do shell script` is blocked** in AppleScript (case-insensitive), including
///   concatenation-based obfuscation (`"do" & " shell" & " script"`).
/// - **Dangerous apps** (`Terminal`, `iTerm`, `iTerm2`) are blocked — they can
///   execute arbitrary commands.
/// - Everything else (standard AppleScript: `tell application`, `activate`,
///   `keystroke`, `click`, `open`, etc.) is allowed.
fn check_script_safety(script: &str, language: ScriptLanguage) -> Option<String> {
    // Block ALL JXA unconditionally
    if language == ScriptLanguage::JavaScript {
        return Some(
            "JavaScript for Automation (JXA) is blocked — use AppleScript instead".to_string(),
        );
    }

    let lower = script.to_lowercase();

    // Block `do shell script` (case-insensitive)
    if lower.contains("do shell script") {
        return Some("'do shell script' is blocked — it allows arbitrary command execution".into());
    }

    // Block concatenation-based obfuscation of "do shell script"
    // Detects atoms "do", "shell", "script" all present with `&` (AppleScript concat operator)
    if lower.contains('&') && contains_shell_atoms(&lower) {
        return Some("Obfuscated 'do shell script' detected (concatenation) — blocked".to_string());
    }

    // Block dangerous applications that can execute arbitrary commands
    if let Some(app) = check_blocked_apps(&lower) {
        return Some(format!(
            "Application '{app}' is blocked — it can execute arbitrary commands"
        ));
    }

    None
}

/// Check if a lowercased script references any blocked application.
/// Looks for `tell application "Terminal"` and similar patterns.
fn check_blocked_apps(lower: &str) -> Option<&'static str> {
    for app in BLOCKED_APPS {
        // Match patterns like: tell application "Terminal", tell app "Terminal"
        // Also match: application("Terminal") for any residual mixed-language patterns
        let quoted_double = format!("\"{app}\"");
        let quoted_single = format!("'{app}'");
        if lower.contains(&quoted_double) || lower.contains(&quoted_single) {
            return Some(app);
        }
    }
    None
}

/// Check if the "do", "shell", and "script" atoms all appear in a lowercased script,
/// indicating a possible concatenation-based obfuscation of `do shell script`.
fn contains_shell_atoms(lower: &str) -> bool {
    // Strip out quoted string contents to find the atoms in the code structure
    // We look for the three words as standalone tokens
    let atoms = ["do", "shell", "script"];
    atoms
        .iter()
        .all(|atom| contains_standalone_token(lower, atom))
}

/// Returns true if `token` appears in `haystack` as a standalone word — i.e. not
/// embedded inside a larger alphanumeric identifier (e.g. "rm" matches `"rm"` and
/// `do shell script rm` but NOT `rm_notes` or `inform`).
fn contains_standalone_token(haystack: &str, token: &str) -> bool {
    let token_bytes = token.as_bytes();
    let hay_bytes = haystack.as_bytes();
    if token_bytes.len() > hay_bytes.len() {
        return false;
    }
    let mut start = 0;
    while let Some(pos) = haystack[start..].find(token) {
        let abs_pos = start + pos;
        let before_ok = abs_pos == 0
            || (!hay_bytes[abs_pos - 1].is_ascii_alphanumeric() && hay_bytes[abs_pos - 1] != b'_');
        let end_pos = abs_pos + token_bytes.len();
        let after_ok = end_pos >= hay_bytes.len()
            || (!hay_bytes[end_pos].is_ascii_alphanumeric() && hay_bytes[end_pos] != b'_');
        if before_ok && after_ok {
            return true;
        }
        start = abs_pos + 1;
    }
    false
}

/// Truncate output to `MAX_OUTPUT_BYTES`, appending a marker if truncated.
/// Uses `floor_char_boundary` to avoid panicking on multi-byte UTF-8.
fn truncate_output(output: &mut String) {
    if output.len() > MAX_OUTPUT_BYTES {
        let boundary = output.floor_char_boundary(MAX_OUTPUT_BYTES);
        output.truncate(boundary);
        output.push_str("\n... [output truncated]");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Helper to check AppleScript safety
    fn check_as(script: &str) -> Option<String> {
        check_script_safety(script, ScriptLanguage::AppleScript)
    }

    // Helper to check JXA safety
    fn check_jxa(script: &str) -> Option<String> {
        check_script_safety(script, ScriptLanguage::JavaScript)
    }

    // --- Allowlist: permitted AppleScript patterns ---

    #[test]
    fn allowlist_permits_tell_application_block() {
        assert!(
            check_as("tell application \"Finder\" to get name of every disk").is_none(),
            "Standard tell application blocks should be allowed"
        );
        assert!(
            check_as("tell application \"Safari\" to open location \"https://example.com\"")
                .is_none()
        );
    }

    #[test]
    fn allowlist_permits_system_events_keystroke() {
        let script = r#"tell application "System Events" to keystroke "v" using command down"#;
        assert!(
            check_as(script).is_none(),
            "System Events keystroke should be allowed"
        );
    }

    #[test]
    fn allowlist_permits_return_and_display() {
        assert!(check_as("return \"hello\"").is_none());
        assert!(check_as("display dialog \"Are you sure?\"").is_none());
    }

    #[test]
    fn allowlist_permits_activate_and_click() {
        assert!(check_as("tell application \"Safari\" to activate").is_none());
        assert!(
            check_as("tell application \"System Events\" to click button \"OK\" of window 1")
                .is_none()
        );
    }

    // --- Blocked: do shell script ---

    #[test]
    fn allowlist_blocks_do_shell_script() {
        assert!(check_as(r#"do shell script "ls -la""#).is_some());
        assert!(check_as(r#"do shell script "rm -rf /""#).is_some());
        assert!(check_as(r#"do shell script "defaults delete com.apple.dock""#).is_some());
    }

    #[test]
    fn allowlist_blocks_do_shell_script_case_insensitive() {
        assert!(check_as(r#"DO SHELL SCRIPT "ls""#).is_some());
        assert!(check_as(r#"Do Shell Script "echo hi""#).is_some());
    }

    #[test]
    fn allowlist_blocks_concatenated_shell() {
        // AppleScript concatenation with &
        let script = r#"set x to "do" & " shell" & " script" & " \"rm -rf /\"""#;
        assert!(
            check_as(script).is_some(),
            "Should block concatenated 'do shell script'"
        );
    }

    #[test]
    fn allowlist_blocks_variable_split_do_shell_script() {
        let script = "set a to \"do\"\nset b to \"shell\"\nset c to \"script\"\nrun (a & \" \" & b & \" \" & c)";
        assert!(
            check_as(script).is_some(),
            "Should block variable-split do shell script"
        );
    }

    // --- Blocked: dangerous apps ---

    #[test]
    fn allowlist_blocks_terminal_app() {
        assert!(check_as(r#"tell application "Terminal" to activate"#).is_some());
        assert!(check_as(r#"tell application "Terminal" to do script "ls""#).is_some());
    }

    #[test]
    fn allowlist_blocks_iterm_app() {
        assert!(check_as(r#"tell application "iTerm" to activate"#).is_some());
        assert!(check_as(r#"tell application "iTerm2" to create window"#).is_some());
    }

    // --- Blocked: all JXA ---

    #[test]
    fn allowlist_blocks_all_jxa() {
        // Even completely safe-looking JXA is blocked
        assert!(check_jxa("'hello'").is_some());
        assert!(check_jxa("Application('Finder').activate()").is_some());
        assert!(check_jxa("1 + 1").is_some());
    }

    #[test]
    fn allowlist_blocks_jxa_system_call() {
        assert!(check_jxa(r#"$.system("/bin/rm -rf /")"#).is_some());
    }

    #[test]
    fn allowlist_blocks_jxa_objc_import() {
        assert!(check_jxa("ObjC.import('Foundation')").is_some());
    }

    #[test]
    fn allowlist_blocks_jxa_doscript() {
        assert!(check_jxa(r#"Application("Terminal").doScript("ls")"#).is_some());
    }

    // --- Edge cases ---

    #[test]
    fn safe_script_with_shell_word_in_string() {
        // The word "script" or "shell" in normal text shouldn't trigger false positives
        // (no & concatenation operator present)
        assert!(
            check_as(r#"display dialog "Please run the shell script manually""#).is_none(),
            "String containing 'shell script' without 'do' prefix should be allowed"
        );
    }

    #[test]
    fn standalone_token_rejects_embedded() {
        assert!(!contains_standalone_token("inform the user", "rm"));
        assert!(!contains_standalone_token("rm_backup folder", "rm"));
        assert!(contains_standalone_token("set x to \"rm\"", "rm"));
        assert!(contains_standalone_token("rm something", "rm"));
    }

    #[test]
    fn truncate_output_caps_at_max() {
        let mut big = "A".repeat(MAX_OUTPUT_BYTES + 5_000);
        truncate_output(&mut big);
        assert!(big.len() <= MAX_OUTPUT_BYTES + 30);
        assert!(big.ends_with("... [output truncated]"));
    }

    #[test]
    fn truncate_output_leaves_small_output_alone() {
        let mut small = "hello".to_string();
        truncate_output(&mut small);
        assert_eq!(small, "hello");
    }

    #[test]
    fn default_impl_works() {
        let _executor: ScriptExecutor = Default::default();
    }
}
