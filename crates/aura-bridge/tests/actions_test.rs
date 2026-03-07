use aura_bridge::actions::{Action, ActionExecutor, MockExecutor};

#[tokio::test]
async fn test_open_app_action() {
    let executor = MockExecutor::new();
    let result = executor
        .execute(Action::OpenApp {
            name: "Safari".into(),
        })
        .await;
    assert!(result.success);
    assert_eq!(result.description, "Opened Safari");
}

#[tokio::test]
async fn test_search_files_action() {
    let executor = MockExecutor::new();
    let result = executor
        .execute(Action::SearchFiles {
            query: "resume.pdf".into(),
        })
        .await;
    assert!(result.success);
    assert_eq!(result.data, Some("[]".into()));
}

#[tokio::test]
async fn test_tile_windows_action() {
    let executor = MockExecutor::new();
    let result = executor
        .execute(Action::TileWindows {
            layout: "left-right".into(),
        })
        .await;
    assert!(result.success);
    assert_eq!(result.description, "Tiled windows: left-right");
}

#[tokio::test]
async fn test_launch_url_action() {
    let executor = MockExecutor::new();
    let result = executor
        .execute(Action::LaunchUrl {
            url: "https://example.com".into(),
        })
        .await;
    assert!(result.success);
    assert_eq!(result.description, "Launched https://example.com");
}

#[tokio::test]
async fn test_type_text_action() {
    let executor = MockExecutor::new();
    let result = executor
        .execute(Action::TypeText {
            text: "hello".into(),
        })
        .await;
    assert!(result.success);
    assert_eq!(result.description, "Typed 5 chars");
    assert_eq!(result.data, None);
}
