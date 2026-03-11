use std::process::Command;
use std::time::Duration;

use serde::{Deserialize, Serialize};

/// Maximum timeout in seconds — any requested timeout is clamped to this value.
const MAX_TIMEOUT_SECS: u64 = 60;

/// Maximum output size in bytes before truncation.
const MAX_OUTPUT_BYTES: usize = 10_240;

/// Defense-in-depth shell command blocklist. Primary safety mechanism —
/// dangerous patterns are blocked before execution.
const BLOCKED_SHELL_PATTERNS: &[&str] = &[
    "rm -rf",
    "rm -r",
    "sudo",
    "mkfs",
    "dd if=",
    "chmod 777",
    ":(){ :|:",
    "> /dev/sd",
    "unlink ",
    "diskutil erase",
];

/// Blocked JXA-specific patterns (checked case-insensitively).
const BLOCKED_JXA_PATTERNS: &[&str] = &["$.system", "objc.import", ".doscript("];

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
        // Safety check: block dangerous shell commands
        if let Some(reason) = check_dangerous(script) {
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

/// Multi-atom dangerous patterns for obfuscation detection.
/// Each entry is a set of atoms that, when ALL present in the script, indicate a
/// dangerous command even if split across string concatenation or variables.
/// Only multi-atom patterns are listed here — single-atom ones (e.g. "sudo") are
/// already caught by `BLOCKED_SHELL_PATTERNS`.
const OBFUSCATED_ATOM_PATTERNS: &[(&[&str], &str)] = &[
    (&["rm", "-rf"], "rm -rf (fragmented)"),
    (&["dd", "if="], "dd if= (fragmented)"),
    (&["chmod", "777"], "chmod 777 (fragmented)"),
];

/// Check if script contains dangerous patterns. Returns reason string if blocked.
fn check_dangerous(script: &str) -> Option<String> {
    let lower = script.to_lowercase();

    // Check ALL content against blocked shell patterns (not just inside `do shell script`)
    for pattern in BLOCKED_SHELL_PATTERNS {
        if lower.contains(pattern) {
            return Some(format!("Dangerous command blocked: contains '{pattern}'"));
        }
    }

    // Check JXA-specific shell escape patterns
    for pattern in BLOCKED_JXA_PATTERNS {
        if lower.contains(pattern) {
            return Some(format!(
                "Dangerous JXA pattern blocked: contains '{pattern}'"
            ));
        }
    }

    // Check for obfuscated dangerous commands (string concatenation, variable splitting)
    if let Some(reason) = check_obfuscated_patterns(&lower) {
        return Some(reason);
    }

    None
}

/// Detect dangerous commands split across string concatenation or variables.
///
/// Checks whether ALL atoms of a known dangerous command appear as standalone tokens
/// in the lowercased script. A token is considered standalone if it's surrounded by
/// non-alphanumeric characters (or is at the start/end of the string), which avoids
/// false positives like "rm_notes.txt" matching the "rm" atom.
fn check_obfuscated_patterns(lower: &str) -> Option<String> {
    for (atoms, label) in OBFUSCATED_ATOM_PATTERNS {
        if atoms
            .iter()
            .all(|atom| contains_standalone_token(lower, atom))
        {
            return Some(format!("Obfuscated dangerous command blocked: {label}"));
        }
    }
    None
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
            || !hay_bytes[abs_pos - 1].is_ascii_alphanumeric() && hay_bytes[abs_pos - 1] != b'_';
        let end_pos = abs_pos + token_bytes.len();
        let after_ok = end_pos >= hay_bytes.len()
            || !hay_bytes[end_pos].is_ascii_alphanumeric() && hay_bytes[end_pos] != b'_';
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

    #[test]
    fn check_dangerous_blocks_rm_rf() {
        assert!(check_dangerous("do shell script \"rm -rf /\"").is_some());
    }

    #[test]
    fn check_dangerous_blocks_sudo() {
        assert!(check_dangerous("sudo reboot").is_some());
    }

    #[test]
    fn check_dangerous_blocks_outside_do_shell_script() {
        // Previously only checked inside "do shell script" blocks
        assert!(check_dangerous("set cmd to \"sudo reboot\"").is_some());
    }

    #[test]
    fn check_dangerous_blocks_jxa_system() {
        assert!(check_dangerous("$.system(\"/bin/rm -rf /\")").is_some());
    }

    #[test]
    fn check_dangerous_blocks_jxa_objc_import() {
        assert!(check_dangerous("ObjC.import('Foundation')").is_some());
    }

    #[test]
    fn check_dangerous_blocks_jxa_doscript() {
        // The dangerous method .doScript( is blocked regardless of app reference
        assert!(check_dangerous("Application(\"Terminal\").doScript(\"ls\")").is_some());
        assert!(check_dangerous("Application(\"Finder\").doScript(\"rm -rf /\")").is_some());
    }

    #[test]
    fn check_dangerous_allows_terminal_reference_without_doscript() {
        // Referencing Terminal app without invoking doScript is legitimate
        assert!(check_dangerous("Application(\"Terminal\").activate()").is_none());
        assert!(check_dangerous("tell application \"Terminal\" to activate").is_none());
    }

    #[test]
    fn check_dangerous_allows_defaults_and_launchctl() {
        // defaults delete and launchctl unload are legitimate automation tasks
        assert!(check_dangerous("do shell script \"defaults delete com.apple.dock\"").is_none());
        assert!(
            check_dangerous(
                "do shell script \"launchctl unload ~/Library/LaunchAgents/com.example.plist\""
            )
            .is_none()
        );
    }

    #[test]
    fn check_dangerous_blocks_unlink() {
        assert!(check_dangerous("unlink /important/file").is_some());
        // Should NOT match "unlinked" as a substring
        assert!(check_dangerous("tell app \"Finder\" to get unlinked items").is_none());
    }

    #[test]
    fn check_dangerous_blocks_diskutil_erase() {
        assert!(check_dangerous("diskutil erase disk0").is_some());
    }

    #[test]
    fn check_dangerous_allows_safe_script() {
        assert!(check_dangerous("tell application \"Finder\" to get name of every disk").is_none());
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

    // --- Obfuscation bypass tests ---

    #[test]
    fn check_dangerous_blocks_concatenated_rm_rf() {
        // String concatenation with & in AppleScript
        let script = "set x to \"rm\" & \" -rf /\"\ndo shell script x";
        assert!(
            check_dangerous(script).is_some(),
            "Should block concatenated rm -rf"
        );
    }

    #[test]
    fn check_dangerous_blocks_variable_split_rm_rf() {
        // Atoms split across variable assignments
        let script = "set a to \"rm\"\nset b to \" -rf\"\ndo shell script a & b";
        assert!(
            check_dangerous(script).is_some(),
            "Should block variable-split rm -rf"
        );
    }

    #[test]
    fn check_dangerous_allows_rm_in_filename() {
        // "rm" appears as part of a filename — should NOT be blocked
        let script = "tell app \"Finder\" to move file \"rm_notes.txt\" to trash";
        assert!(
            check_dangerous(script).is_none(),
            "Should allow 'rm' when part of a filename like rm_notes.txt"
        );
    }

    #[test]
    fn check_dangerous_blocks_obfuscated_dd() {
        let script = "set a to \"dd\"\nset b to \"if=/dev/zero\"\ndo shell script a & \" \" & b";
        assert!(
            check_dangerous(script).is_some(),
            "Should block fragmented dd if="
        );
    }

    #[test]
    fn check_dangerous_blocks_obfuscated_chmod() {
        let script = "set cmd to \"chmod\" & \" 777 /etc\"\ndo shell script cmd";
        assert!(
            check_dangerous(script).is_some(),
            "Should block fragmented chmod 777"
        );
    }

    #[test]
    fn check_dangerous_allows_partial_atom_match() {
        // Contains "rm" standalone but no "-rf" — should NOT be blocked by obfuscation check
        // (the plain BLOCKED_SHELL_PATTERNS also won't match since "rm -rf" isn't present)
        let script = "do shell script \"rm myfile.txt\"";
        assert!(
            check_dangerous(script).is_none(),
            "Should allow 'rm' without '-rf'"
        );
    }

    #[test]
    fn standalone_token_rejects_embedded() {
        // "rm" embedded in "inform" — not standalone
        assert!(!contains_standalone_token("inform the user", "rm"));
        // "rm" embedded in "rm_backup" — not standalone
        assert!(!contains_standalone_token("rm_backup folder", "rm"));
        // "rm" standalone in quotes
        assert!(contains_standalone_token("set x to \"rm\"", "rm"));
        // "rm" at start of string
        assert!(contains_standalone_token("rm something", "rm"));
    }
}
