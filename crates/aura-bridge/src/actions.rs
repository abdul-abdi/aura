use async_trait::async_trait;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Action {
    OpenApp { name: String },
    SearchFiles { query: String },
    TileWindows { layout: String },
    LaunchUrl { url: String },
    TypeText { text: String },
}

#[derive(Debug, Clone, PartialEq)]
pub struct ActionResult {
    pub success: bool,
    pub description: String,
    pub data: Option<String>,
}

#[async_trait]
pub trait ActionExecutor: Send + Sync {
    async fn execute(&self, action: Action) -> ActionResult;
}

/// Mock executor for testing.
#[cfg(any(test, feature = "test-support"))]
pub struct MockExecutor;

#[cfg(any(test, feature = "test-support"))]
impl MockExecutor {
    pub fn new() -> Self {
        Self
    }
}

#[cfg(any(test, feature = "test-support"))]
#[async_trait]
impl ActionExecutor for MockExecutor {
    async fn execute(&self, action: Action) -> ActionResult {
        match action {
            Action::OpenApp { name } => ActionResult {
                success: true,
                description: format!("Opened {name}"),
                data: None,
            },
            Action::SearchFiles { query } => ActionResult {
                success: true,
                description: format!("Found files matching '{query}'"),
                data: Some("[]".into()),
            },
            Action::TileWindows { layout } => ActionResult {
                success: true,
                description: format!("Tiled windows: {layout}"),
                data: None,
            },
            Action::LaunchUrl { url } => ActionResult {
                success: true,
                description: format!("Launched {url}"),
                data: None,
            },
            Action::TypeText { text } => ActionResult {
                success: true,
                description: format!("Typed {} chars", text.len()),
                data: None,
            },
        }
    }
}
