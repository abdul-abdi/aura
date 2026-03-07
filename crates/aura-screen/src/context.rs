use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WindowInfo {
    pub title: String,
    pub app_name: String,
    pub is_focused: bool,
    pub bounds: (i32, i32, i32, i32), // x, y, w, h
}

#[derive(Debug, Clone, Default)]
pub struct ScreenContext {
    windows: Vec<WindowInfo>,
    focused_text: Option<String>,
}

impl ScreenContext {
    pub fn new() -> Self {
        Self::default()
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

    pub fn update(&mut self, windows: Vec<WindowInfo>, focused_text: Option<String>) {
        self.windows = windows;
        self.focused_text = focused_text;
    }

    pub fn summary(&self) -> String {
        let focused = self
            .focused_window()
            .map(|w| format!("{} - {}", w.app_name, w.title))
            .unwrap_or_else(|| "No focused window".into());

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
