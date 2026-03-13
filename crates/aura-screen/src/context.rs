use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ElementBounds {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl ElementBounds {
    pub fn center(&self) -> (f64, f64) {
        (self.x + self.width / 2.0, self.y + self.height / 2.0)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UIElement {
    pub role: String,
    pub label: Option<String>,
    pub value: Option<String>,
    pub bounds: Option<ElementBounds>,
    pub enabled: bool,
    pub focused: bool,
    pub parent_label: Option<String>,
}

impl UIElement {
    /// Returns a human-readable summary of this element.
    ///
    /// - Strips "AX" prefix from role and lowercases it (e.g. "AXButton" -> "button")
    /// - Shows label in quotes if present
    /// - Shows bounds as `bounds={x:N, y:N, w:N, h:N}` (as integers)
    /// - Appends "enabled" if enabled, "focused" if focused
    pub fn summary(&self) -> String {
        let role = self
            .role
            .strip_prefix("AX")
            .unwrap_or(&self.role)
            .to_lowercase();

        let mut parts = vec![role];

        if let Some(ref label) = self.label {
            parts.push(format!("\"{}\"", label));
        }

        if let Some(ref v) = self.value {
            if !v.is_empty() && self.label.as_deref() != Some(v.as_str()) {
                let truncated: String = v.chars().take(100).collect();
                if truncated.len() < v.len() {
                    parts.push(format!("value=\"{}...\"", truncated));
                } else {
                    parts.push(format!("value=\"{}\"", truncated));
                }
            }
        }

        if let Some(ref b) = self.bounds {
            parts.push(format!(
                "bounds={{x:{}, y:{}, w:{}, h:{}}}",
                b.x as i64, b.y as i64, b.width as i64, b.height as i64
            ));
        }

        if self.enabled {
            parts.push("enabled".to_string());
        }

        if self.focused {
            parts.push("focused".to_string());
        }

        parts.join(" ")
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScreenContext {
    frontmost_app: String,
    frontmost_title: Option<String>,
    open_windows: Vec<String>,
    clipboard: Option<String>,
    ui_elements: Vec<UIElement>,
}

impl ScreenContext {
    pub fn empty() -> Self {
        Self {
            frontmost_app: String::new(),
            frontmost_title: None,
            open_windows: Vec::new(),
            clipboard: None,
            ui_elements: Vec::new(),
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
            ui_elements: Vec::new(),
        }
    }

    pub fn with_ui_elements(mut self, elements: Vec<UIElement>) -> Self {
        self.ui_elements = elements;
        self
    }

    pub fn frontmost_app(&self) -> &str {
        &self.frontmost_app
    }

    pub fn clipboard(&self) -> Option<&str> {
        self.clipboard.as_deref()
    }

    pub fn ui_elements(&self) -> &[UIElement] {
        &self.ui_elements
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
        if !self.ui_elements.is_empty() {
            parts.push("UI Elements (interactive):".to_string());
            for (i, el) in self.ui_elements.iter().enumerate() {
                parts.push(format!("  [{}] {}", i, el.summary()));
            }
        }
        parts.join("\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_bounds(x: f64, y: f64, width: f64, height: f64) -> ElementBounds {
        ElementBounds {
            x,
            y,
            width,
            height,
        }
    }

    #[test]
    fn ui_element_display_with_all_fields() {
        let el = UIElement {
            role: "AXButton".to_string(),
            label: Some("Submit".to_string()),
            value: None,
            bounds: Some(make_bounds(10.0, 20.0, 100.0, 40.0)),
            enabled: true,
            focused: false,
            parent_label: None,
        };
        let s = el.summary();
        assert!(
            s.contains("button"),
            "role should be lowercased without AX prefix"
        );
        assert!(s.contains("\"Submit\""), "label should appear in quotes");
        assert!(
            s.contains("bounds={x:10, y:20, w:100, h:40}"),
            "bounds should appear as integers"
        );
        assert!(s.contains("enabled"), "enabled flag should appear");
    }

    #[test]
    fn ui_element_display_without_label() {
        let el = UIElement {
            role: "AXTextField".to_string(),
            label: None,
            value: Some("hello".to_string()),
            bounds: None,
            enabled: true,
            focused: false,
            parent_label: None,
        };
        let s = el.summary();
        assert!(!s.contains("\"\""), "no empty quotes when label is None");
        assert!(s.contains("textfield"), "role without AX prefix");
        assert!(
            s.contains("value=\"hello\""),
            "value should appear in summary"
        );
    }

    #[test]
    fn ui_element_value_shown_when_different_from_label() {
        let el = UIElement {
            role: "AXTextField".to_string(),
            label: Some("Search".to_string()),
            value: Some("rust lang".to_string()),
            bounds: None,
            enabled: true,
            focused: false,
            parent_label: None,
        };
        let s = el.summary();
        assert!(
            s.contains("value=\"rust lang\""),
            "value should appear when different from label"
        );
    }

    #[test]
    fn ui_element_value_omitted_when_same_as_label() {
        let el = UIElement {
            role: "AXButton".to_string(),
            label: Some("Save".to_string()),
            value: Some("Save".to_string()),
            bounds: None,
            enabled: true,
            focused: false,
            parent_label: None,
        };
        let s = el.summary();
        // value should not appear twice when it duplicates the label
        assert_eq!(
            s.matches("Save").count(),
            1,
            "value should not duplicate label"
        );
    }

    #[test]
    fn ui_element_value_truncated_at_100_chars() {
        let long_value = "a".repeat(150);
        let el = UIElement {
            role: "AXTextField".to_string(),
            label: None,
            value: Some(long_value),
            bounds: None,
            enabled: false,
            focused: false,
            parent_label: None,
        };
        let s = el.summary();
        assert!(s.contains("value=\""), "value should appear");
        assert!(
            s.contains("...\""),
            "long value should be truncated with ellipsis"
        );
        // the truncated portion should be exactly 100 'a' chars
        assert!(
            s.contains(&"a".repeat(100)),
            "first 100 chars should be present"
        );
        assert!(
            !s.contains(&"a".repeat(101)),
            "101st char should not be present"
        );
    }

    #[test]
    fn ui_element_value_omitted_when_empty() {
        let el = UIElement {
            role: "AXTextField".to_string(),
            label: None,
            value: Some(String::new()),
            bounds: None,
            enabled: false,
            focused: false,
            parent_label: None,
        };
        let s = el.summary();
        assert!(
            !s.contains("value="),
            "empty value should not appear in summary"
        );
    }

    #[test]
    fn ui_element_display_focused() {
        let el = UIElement {
            role: "AXTextField".to_string(),
            label: None,
            value: None,
            bounds: None,
            enabled: false,
            focused: true,
            parent_label: None,
        };
        let s = el.summary();
        assert!(s.contains("focused"), "focused flag should appear");
        assert!(
            !s.contains("enabled"),
            "enabled should not appear when false"
        );
    }

    #[test]
    fn screen_context_summary_includes_ui_elements() {
        let elements = vec![
            UIElement {
                role: "AXButton".to_string(),
                label: Some("OK".to_string()),
                value: None,
                bounds: None,
                enabled: true,
                focused: false,
                parent_label: None,
            },
            UIElement {
                role: "AXButton".to_string(),
                label: Some("Cancel".to_string()),
                value: None,
                bounds: None,
                enabled: true,
                focused: false,
                parent_label: None,
            },
        ];
        let ctx = ScreenContext::empty().with_ui_elements(elements);
        let s = ctx.summary();
        assert!(
            s.contains("UI Elements (interactive):"),
            "summary should include UI Elements section"
        );
        assert!(s.contains("[0]"), "summary should use [i] bracket format");
        assert!(s.contains("[1]"), "summary should index all elements");
        assert!(s.contains("OK"), "summary should include element labels");
        assert!(
            s.contains("Cancel"),
            "summary should include all element labels"
        );
    }

    #[test]
    fn screen_context_summary_empty_elements() {
        let ctx = ScreenContext::empty();
        let s = ctx.summary();
        assert!(
            !s.contains("UI Elements"),
            "no UI Elements section when empty"
        );
    }

    #[test]
    fn element_bounds_center() {
        let b = make_bounds(100.0, 200.0, 80.0, 30.0);
        let (cx, cy) = b.center();
        assert_eq!(cx, 140.0, "center x should be 140");
        assert_eq!(cy, 215.0, "center y should be 215");
    }
}
