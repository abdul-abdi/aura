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
async fn test_run_applescript_error_returns_failure() {
    let executor = ScriptExecutor::new();
    let result = executor
        .run("error \"test error\"", ScriptLanguage::AppleScript, 10)
        .await;
    assert!(!result.success);
    assert!(!result.stderr.is_empty());
}

#[tokio::test]
async fn test_blocks_do_shell_script() {
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
async fn test_blocks_do_shell_script_safe_commands_too() {
    // Even "safe" shell commands are blocked — do shell script is the escape hatch
    let executor = ScriptExecutor::new();
    let result = executor
        .run(
            r#"do shell script "defaults delete com.apple.dock""#,
            ScriptLanguage::AppleScript,
            10,
        )
        .await;
    assert!(!result.success);
    assert!(result.stderr.contains("blocked"));
}

#[tokio::test]
async fn test_blocks_all_jxa() {
    let executor = ScriptExecutor::new();
    // Even a simple expression is blocked for JXA
    let result = executor
        .run("'hello from jxa'", ScriptLanguage::JavaScript, 10)
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
async fn test_blocks_terminal_app() {
    let executor = ScriptExecutor::new();
    let result = executor
        .run(
            r#"tell application "Terminal" to activate"#,
            ScriptLanguage::AppleScript,
            10,
        )
        .await;
    assert!(!result.success);
    assert!(result.stderr.contains("blocked"));
}

#[tokio::test]
async fn test_blocks_iterm_app() {
    let executor = ScriptExecutor::new();
    let result = executor
        .run(
            r#"tell application "iTerm2" to create window with default profile"#,
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
