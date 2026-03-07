use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WindowBounds {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WindowInfo {
    pub title: String,
    pub app_name: String,
    pub is_focused: bool,
    pub bounds: WindowBounds,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ScreenContext {
    windows: Vec<WindowInfo>,
    focused_text: Option<String>,
}

impl ScreenContext {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_windows(windows: Vec<WindowInfo>, focused_text: Option<String>) -> Self {
        Self {
            windows,
            focused_text,
        }
    }

    pub fn windows(&self) -> &[WindowInfo] {
        &self.windows
    }

    pub fn focused_window(&self) -> Option<&WindowInfo> {
        self.windows.iter().find(|w| w.is_focused)
    }

    pub fn focused_text(&self) -> Option<&str> {
        self.focused_text.as_deref()
    }

    pub fn summary(&self) -> String {
        let focused = self
            .focused_window()
            .map(|w| format!("{} - {}", w.app_name, w.title))
            .unwrap_or_else(|| "No focused window".into());

        if self.windows.is_empty() {
            return format!("Focused: {}\nNo open windows", focused);
        }

        let window_list: Vec<String> = self
            .windows
            .iter()
            .map(|w| format!("  {} - {}", w.app_name, w.title))
            .collect();

        format!(
            "Focused: {}\nOpen windows:\n{}",
            focused,
            window_list.join("\n")
        )
    }
}
