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
    assert!(
        summary.contains("clipboard text"),
        "Should include clipboard"
    );
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
    assert!(
        !ctx.frontmost_app().is_empty(),
        "Should detect frontmost app"
    );
}
