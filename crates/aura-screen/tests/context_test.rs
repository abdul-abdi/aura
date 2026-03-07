use aura_screen::context::{ScreenContext, WindowInfo};

#[test]
fn test_screen_context_creation() {
    let ctx = ScreenContext::new();
    assert!(ctx.windows().is_empty());
}

#[test]
fn test_window_info_display() {
    let info = WindowInfo {
        title: "Visual Studio Code".into(),
        app_name: "Code".into(),
        is_focused: true,
        bounds: (0, 0, 1920, 1080),
    };
    assert_eq!(info.app_name, "Code");
    assert!(info.is_focused);
}
