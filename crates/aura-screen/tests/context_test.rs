use aura_screen::context::{ScreenContext, WindowBounds, WindowInfo};

fn make_window(app: &str, title: &str, focused: bool) -> WindowInfo {
    WindowInfo {
        title: title.into(),
        app_name: app.into(),
        is_focused: focused,
        bounds: WindowBounds {
            x: 0,
            y: 0,
            width: 1920,
            height: 1080,
        },
    }
}

#[test]
fn test_screen_context_creation() {
    let ctx = ScreenContext::new();
    assert!(ctx.windows().is_empty());
    assert!(ctx.focused_text().is_none());
    assert!(ctx.focused_window().is_none());
}

#[test]
fn test_window_info_fields() {
    let info = make_window("Code", "Visual Studio Code", true);
    assert_eq!(info.app_name, "Code");
    assert_eq!(info.title, "Visual Studio Code");
    assert!(info.is_focused);
    assert_eq!(info.bounds.width, 1920);
    assert_eq!(info.bounds.height, 1080);
}

#[test]
fn test_with_windows_and_focused() {
    let windows = vec![
        make_window("Safari", "Google", false),
        make_window("Code", "main.rs", true),
    ];
    let ctx = ScreenContext::with_windows(windows, Some("selected text".into()));

    assert_eq!(ctx.windows().len(), 2);
    assert_eq!(ctx.focused_text(), Some("selected text"));

    let focused = ctx.focused_window().expect("should have focused window");
    assert_eq!(focused.app_name, "Code");
}

#[test]
fn test_focused_window_none_when_unfocused() {
    let windows = vec![
        make_window("Safari", "Google", false),
        make_window("Code", "main.rs", false),
    ];
    let ctx = ScreenContext::with_windows(windows, None);
    assert!(ctx.focused_window().is_none());
}

#[test]
fn test_summary_with_windows() {
    let windows = vec![
        make_window("Safari", "Google", false),
        make_window("Code", "main.rs", true),
    ];
    let ctx = ScreenContext::with_windows(windows, None);
    let summary = ctx.summary();

    assert!(summary.contains("Focused: Code - main.rs"));
    assert!(summary.contains("Safari - Google"));
    assert!(summary.contains("Code - main.rs"));
}

#[test]
fn test_summary_empty() {
    let ctx = ScreenContext::new();
    let summary = ctx.summary();
    assert!(summary.contains("No focused window"));
    assert!(summary.contains("No open windows"));
}
