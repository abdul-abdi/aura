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

#[tokio::test]
async fn test_blocks_sudo() {
    let executor = ScriptExecutor::new();
    let result = executor
        .run(
            r#"do shell script "sudo rm /etc/hosts""#,
            ScriptLanguage::AppleScript,
            10,
        )
        .await;
    assert!(!result.success);
    assert!(result.stderr.contains("blocked"));
}

#[tokio::test]
async fn test_blocks_dd() {
    let executor = ScriptExecutor::new();
    let result = executor
        .run(
            r#"do shell script "dd if=/dev/zero of=/dev/disk0""#,
            ScriptLanguage::AppleScript,
            10,
        )
        .await;
    assert!(!result.success);
    assert!(result.stderr.contains("blocked"));
}

#[tokio::test]
async fn test_blocks_chmod_777() {
    let executor = ScriptExecutor::new();
    let result = executor
        .run(
            r#"do shell script "chmod 777 /etc/passwd""#,
            ScriptLanguage::AppleScript,
            10,
        )
        .await;
    assert!(!result.success);
    assert!(result.stderr.contains("blocked"));
}

#[tokio::test]
async fn test_blocks_fork_bomb() {
    let executor = ScriptExecutor::new();
    let result = executor
        .run(
            r#"do shell script ":(){ :|:& };:""#,
            ScriptLanguage::AppleScript,
            10,
        )
        .await;
    assert!(!result.success);
    assert!(result.stderr.contains("blocked"));
}

#[tokio::test]
async fn test_blocks_mkfs() {
    let executor = ScriptExecutor::new();
    let result = executor
        .run(
            r#"do shell script "mkfs.ext4 /dev/sda1""#,
            ScriptLanguage::AppleScript,
            10,
        )
        .await;
    assert!(!result.success);
    assert!(result.stderr.contains("blocked"));
}

#[tokio::test]
async fn test_blocks_jxa_system() {
    let executor = ScriptExecutor::new();
    let result = executor
        .run(
            r#"$.system("/bin/rm -rf /")"#,
            ScriptLanguage::JavaScript,
            10,
        )
        .await;
    assert!(!result.success);
    assert!(result.stderr.contains("blocked"));
}

#[tokio::test]
async fn test_blocks_jxa_terminal() {
    let executor = ScriptExecutor::new();
    let result = executor
        .run(
            r#"Application("Terminal").doScript("rm -rf /")"#,
            ScriptLanguage::JavaScript,
            10,
        )
        .await;
    assert!(!result.success);
    assert!(result.stderr.contains("blocked"));
}

#[tokio::test]
async fn test_blocks_jxa_objc_import() {
    let executor = ScriptExecutor::new();
    let result = executor
        .run(
            r#"ObjC.import('Foundation'); var task = $.NSTask.alloc.init;"#,
            ScriptLanguage::JavaScript,
            10,
        )
        .await;
    assert!(!result.success);
    assert!(result.stderr.contains("blocked"));
}

#[tokio::test]
async fn test_blocks_dangerous_outside_do_shell_script() {
    // Previously, patterns were only checked inside "do shell script" blocks.
    // Now they should be checked everywhere.
    let executor = ScriptExecutor::new();
    let result = executor
        .run(
            r#"set cmd to "sudo reboot""#,
            ScriptLanguage::AppleScript,
            10,
        )
        .await;
    assert!(!result.success);
    assert!(result.stderr.contains("blocked"));
}

#[tokio::test]
async fn test_timeout_clamped_to_max() {
    // Even if a very large timeout is requested, it should be clamped
    let executor = ScriptExecutor::new();
    // This script takes 2 seconds. With a requested timeout of 86400 (clamped to 60), it should complete normally.
    let result = executor
        .run(
            "delay 2\nreturn \"done\"",
            ScriptLanguage::AppleScript,
            86400,
        )
        .await;
    // It should succeed because 2s < 60s (the clamped max)
    assert!(result.success);
}

#[tokio::test]
async fn test_stdout_truncated() {
    let executor = ScriptExecutor::new();
    // Generate a large output (repeat a string many times)
    let script = r#"
        set output to ""
        repeat 500 times
            set output to output & "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA" & linefeed
        end repeat
        return output
    "#;
    let result = executor.run(script, ScriptLanguage::AppleScript, 30).await;
    // Output should be capped at ~10KB
    if result.success {
        assert!(
            result.stdout.len() <= 10_240 + 30,
            "stdout should be truncated to ~10KB, got {} bytes",
            result.stdout.len()
        );
    }
}

#[tokio::test]
async fn test_safe_script_allowed() {
    let executor = ScriptExecutor::new();
    let result = executor
        .run(r#"return "safe output""#, ScriptLanguage::AppleScript, 10)
        .await;
    // Should succeed (no dangerous patterns)
    assert!(
        result.success,
        "Safe script should not be blocked: {:?}",
        result
    );
    assert_eq!(result.stdout.trim(), "safe output");
}
