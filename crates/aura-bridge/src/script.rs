use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

use serde::{Deserialize, Serialize};

/// Blocked shell patterns — never allowed inside `do shell script`.
const BLOCKED_SHELL_PATTERNS: &[&str] = &[
    "rm -rf",
    "rm -r",
    "rmdir",
    "sudo",
    "mkfs",
    "dd if=",
    "chmod 777",
    ":(){ :|:",
    "> /dev/sd",
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

#[derive(Clone)]
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

        let handle = tokio::task::spawn_blocking(move || {
            let mut cmd = Command::new("sandbox-exec");
            cmd.arg("-f").arg(sandbox_profile_path());
            cmd.arg("osascript");
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
                        stderr: format!("Failed to spawn sandbox-exec: {e}"),
                    };
                }
            };

            // Wait with timeout using a polling loop
            let start = std::time::Instant::now();
            loop {
                match child.try_wait() {
                    Ok(Some(status)) => {
                        // Process finished — read output
                        let stdout = child
                            .stdout
                            .take()
                            .map(|mut s| {
                                let mut buf = String::new();
                                std::io::Read::read_to_string(&mut s, &mut buf).ok();
                                buf
                            })
                            .unwrap_or_default();
                        let stderr = child
                            .stderr
                            .take()
                            .map(|mut s| {
                                let mut buf = String::new();
                                std::io::Read::read_to_string(&mut s, &mut buf).ok();
                                buf
                            })
                            .unwrap_or_default();

                        return ScriptResult {
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
                            return ScriptResult {
                                success: false,
                                stdout: String::new(),
                                stderr: "Script timed out".into(),
                            };
                        }
                        std::thread::sleep(Duration::from_millis(50));
                    }
                    Err(e) => {
                        return ScriptResult {
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

/// Locate the sandbox profile to pass to `sandbox-exec -f`.
///
/// Search order:
/// 1. Next to the current binary (production install).
/// 2. Next to `CARGO_MANIFEST_DIR` (development / cargo test).
/// 3. `~/.config/aura/sandbox.sb` (user-level override).
fn sandbox_profile_path() -> PathBuf {
    // 1. Beside the running binary
    if let Ok(exe) = std::env::current_exe() {
        let candidate = exe
            .parent()
            .unwrap_or(std::path::Path::new("."))
            .join("sandbox.sb");
        if candidate.exists() {
            return candidate;
        }
    }

    // 2. Relative to the crate's source directory (cargo test / dev builds)
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let dev_path = manifest_dir.join("sandbox.sb");
    if dev_path.exists() {
        return dev_path;
    }

    // 3. User config directory fallback
    if let Some(home) = std::env::var_os("HOME") {
        let cfg = PathBuf::from(home)
            .join(".config")
            .join("aura")
            .join("sandbox.sb");
        if cfg.exists() {
            return cfg;
        }
    }

    // Return the dev path even if missing — sandbox-exec will produce a clear error.
    dev_path
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn sandbox_blocks_network() {
        let executor = ScriptExecutor::new();
        let result = executor
            .run(
                r#"do shell script "curl -s https://example.com""#,
                ScriptLanguage::AppleScript,
                5,
            )
            .await;
        // Either the script fails (sandbox killed curl / denied network) or
        // it succeeds but returns no output (blocked silently). Both are acceptable.
        assert!(
            !result.success || result.stdout.is_empty(),
            "Expected sandbox to block network access, but got output: {}",
            result.stdout
        );
    }
}
