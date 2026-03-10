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
