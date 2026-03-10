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
