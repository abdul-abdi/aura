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

#[test]
fn test_clipboard_truncation_ascii() {
    let long_clip = "x".repeat(300);
    let ctx = ScreenContext::new_with_details("App", None, vec![], Some(long_clip));
    let summary = ctx.summary();
    assert!(
        summary.contains("..."),
        "Long clipboard should be truncated with ellipsis"
    );
    // The truncated content should have at most 200 chars before "..."
    let clip_line = summary
        .lines()
        .find(|l| l.starts_with("Clipboard:"))
        .unwrap();
    assert!(
        clip_line.len() < 220,
        "Truncated clipboard line should be short"
    );
}

#[test]
fn test_clipboard_truncation_multibyte() {
    // Use multi-byte characters to verify we don't panic on byte-boundary slicing
    let long_clip: String = std::iter::repeat_n('\u{1F600}', 300).collect();
    let ctx = ScreenContext::new_with_details("App", None, vec![], Some(long_clip));
    let summary = ctx.summary();
    assert!(
        summary.contains("..."),
        "Long multi-byte clipboard should be truncated"
    );
}

#[test]
fn test_clipboard_short_no_truncation() {
    let ctx = ScreenContext::new_with_details("App", None, vec![], Some("hello".into()));
    let summary = ctx.summary();
    assert!(summary.contains("Clipboard: hello"));
    assert!(
        !summary.contains("..."),
        "Short clipboard should not be truncated"
    );
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
