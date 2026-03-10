use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScreenContext {
    frontmost_app: String,
    frontmost_title: Option<String>,
    open_windows: Vec<String>,
    clipboard: Option<String>,
}

impl ScreenContext {
    pub fn empty() -> Self {
        Self {
            frontmost_app: String::new(),
            frontmost_title: None,
            open_windows: Vec::new(),
            clipboard: None,
        }
    }

    pub fn new_with_details(
        frontmost_app: &str,
        frontmost_title: Option<&str>,
        open_windows: Vec<String>,
        clipboard: Option<String>,
    ) -> Self {
        Self {
            frontmost_app: frontmost_app.to_string(),
            frontmost_title: frontmost_title.map(String::from),
            open_windows,
            clipboard,
        }
    }

    pub fn frontmost_app(&self) -> &str {
        &self.frontmost_app
    }

    pub fn clipboard(&self) -> Option<&str> {
        self.clipboard.as_deref()
    }

    /// Human-readable summary for Gemini context injection.
    pub fn summary(&self) -> String {
        let mut parts = Vec::new();
        parts.push(format!("Frontmost app: {}", self.frontmost_app));
        if let Some(ref title) = self.frontmost_title {
            parts.push(format!("Window title: {title}"));
        }
        if !self.open_windows.is_empty() {
            parts.push(format!("Open windows: {}", self.open_windows.join(", ")));
        }
        if let Some(ref clip) = self.clipboard {
            let truncated: String = clip.chars().take(200).collect();
            if truncated.len() < clip.len() {
                parts.push(format!("Clipboard: {truncated}..."));
            } else {
                parts.push(format!("Clipboard: {truncated}"));
            }
        }
        parts.join("\n")
    }
}
