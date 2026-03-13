use std::time::Duration;

/// Frame dimension snapshot used to map image-pixel coordinates to logical macOS points.
#[derive(Clone, Copy)]
pub(crate) struct FrameDims {
    pub img_w: u32,
    pub img_h: u32,
    pub logical_w: u32,
    pub logical_h: u32,
}

impl FrameDims {
    /// Map an x coordinate from image pixels to logical screen points.
    pub fn to_logical_x(self, x: f64) -> f64 {
        if self.img_w == 0 {
            return x;
        }
        x * (self.logical_w as f64 / self.img_w as f64)
    }

    /// Map a y coordinate from image pixels to logical screen points.
    pub fn to_logical_y(self, y: f64) -> f64 {
        if self.img_h == 0 {
            return y;
        }
        y * (self.logical_h as f64 / self.img_h as f64)
    }
}

/// Per-tool-type settle delay before polling for screen changes.
/// Keyboard actions are near-instant; app activation and scripts need more time.
pub(crate) fn settle_delay_for_tool(name: &str) -> Duration {
    match name {
        "type_text" | "press_key" | "move_mouse" => Duration::from_millis(30),
        "scroll" | "drag" => Duration::from_millis(50),
        "click" | "click_element" | "context_menu_click" => Duration::from_millis(100),
        "activate_app" | "click_menu_item" => Duration::from_millis(150),
        "run_applescript" => Duration::from_millis(200),
        _ => Duration::from_millis(150), // conservative default
    }
}

/// Find a UI element by label/role in the frontmost app's accessibility tree and click it.
pub(crate) fn click_element_inner(
    label: Option<&str>,
    role: Option<&str>,
    index: usize,
) -> serde_json::Value {
    if label.is_none() && role.is_none() {
        return serde_json::json!({
            "success": false,
            "error": "At least one of 'label' or 'role' must be provided",
        });
    }

    // Try 1: AX press action via single-pass walk (finds the exact Nth match and presses it)
    let ax_result =
        aura_screen::accessibility::ax_perform_action_nth(label, role, "AXPress", index);
    if ax_result.success {
        let el = ax_result.element.as_ref();
        return serde_json::json!({
            "success": true,
            "method": "ax_press",
            "element": {
                "role": el.map(|e| &e.role),
                "label": el.and_then(|e| e.label.as_ref()),
            },
        });
    }

    // AXPress failed — need bounds for coordinate fallback.
    // Check if ax_perform_action_nth found the element but couldn't press it (has element),
    // or if the element wasn't found at all (no element).
    let target = match ax_result.element {
        Some(el) => el,
        None => {
            // Element not found — do a full walk for diagnostic alternatives
            let all_elements = aura_screen::accessibility::get_focused_app_elements();
            if all_elements.is_empty() {
                return serde_json::json!({
                    "success": false,
                    "error": "No interactive UI elements found. The app may not expose accessibility data, \
                              or Accessibility permission may not be fully granted.",
                });
            }

            let matches = aura_screen::accessibility::find_elements(&all_elements, label, role);
            if matches.is_empty() {
                let alternatives: Vec<String> = all_elements
                    .iter()
                    .map(|el| {
                        let label_str = el.label.as_deref().unwrap_or("(unlabeled)");
                        let role_short = el
                            .role
                            .strip_prefix("AX")
                            .unwrap_or(&el.role)
                            .to_lowercase();
                        format!("{role_short} \"{label_str}\"")
                    })
                    .take(15)
                    .collect();

                return serde_json::json!({
                    "success": false,
                    "error": format!(
                        "No element matching label={:?} role={:?}. Available elements: {}",
                        label, role, alternatives.join(", ")
                    ),
                });
            }

            return match matches.get(index) {
                Some(_target) => serde_json::json!({
                    "success": false,
                    "error": format!(
                        "AXPress failed on element ({}). {}",
                        ax_result.error.unwrap_or_default(),
                        "Element has no bounds for coordinate fallback.",
                    ),
                }),
                None => serde_json::json!({
                    "success": false,
                    "error": format!(
                        "Index {} out of range. Found {} matching elements.",
                        index, matches.len()
                    ),
                }),
            };
        }
    };

    tracing::debug!(
        error = ?ax_result.error,
        "AXPress failed, trying PID-targeted click"
    );

    // If the element has no bounds it may be offscreen — try AXScrollToVisible first.
    let scrolled_target: aura_screen::context::UIElement;
    let effective_target: &aura_screen::context::UIElement;
    if target.bounds.is_some() {
        effective_target = &target;
    } else {
        tracing::debug!("Element has no bounds — attempting AXScrollToVisible");
        match aura_screen::accessibility::scroll_to_visible_and_get_element(label, role, index) {
            Some(refreshed) if refreshed.bounds.is_some() => {
                scrolled_target = refreshed;
                effective_target = &scrolled_target;
            }
            _ => {
                return serde_json::json!({
                    "success": false,
                    "error": "Element found but has no bounds (offscreen or hidden). \
                              AXScrollToVisible was attempted but the element remains \
                              invisible. Try scrolling the element into view manually.",
                    "element": {
                        "role": target.role,
                        "label": target.label,
                    },
                });
            }
        }
    }
    let bounds = effective_target.bounds.as_ref().unwrap();

    // AX bounds are already in logical screen coordinates — no FrameDims conversion needed
    let (center_x, center_y) = bounds.center();

    // Pre-move cursor and attempt focus before coordinate-based click fallback.
    // Some apps need hover state and/or focus to register clicks.
    let _ = aura_input::mouse::move_mouse(center_x, center_y);
    std::thread::sleep(std::time::Duration::from_millis(15));
    let _ = aura_screen::accessibility::ax_set_focused(label, role);

    // Try 2: PID-targeted click
    if let Some(pid) = aura_screen::macos::get_frontmost_pid() {
        if aura_input::mouse::click_pid(center_x, center_y, "left", 1, &[], pid).is_ok() {
            return serde_json::json!({
                "success": true,
                "method": "pid_click",
                "element": {
                    "role": effective_target.role,
                    "label": effective_target.label,
                },
                "clicked_at": {
                    "x": center_x,
                    "y": center_y,
                },
            });
        }
        tracing::debug!("PID-targeted click_element failed, falling back to HID");
    }

    // Try 3: HID click fallback
    match aura_input::mouse::click(center_x, center_y, "left", 1, &[]) {
        Ok(()) => serde_json::json!({
            "success": true,
            "method": "hid_click",
            "element": {
                "role": effective_target.role,
                "label": effective_target.label,
            },
            "clicked_at": {
                "x": center_x,
                "y": center_y,
            },
        }),
        Err(e) => serde_json::json!({
            "success": false,
            "error": format!("Click failed (all methods exhausted): {e}"),
        }),
    }
}

/// Build an AppleScript to click a menu item via System Events.
/// Supports 2-level (menu bar > item) and 3+ level (menu bar > submenu > item) paths.
pub(crate) fn build_menu_click_script(app: &str, path: &[String]) -> String {
    let process = app.replace(['\\', '"'], "");
    let escaped: Vec<String> = path.iter().map(|s| s.replace(['\\', '"'], "")).collect();

    match escaped.len() {
        2 => format!(
            "tell application \"System Events\" to tell process \"{process}\"\n\
             \tclick menu item \"{}\" of menu 1 of menu bar item \"{}\" of menu bar 1\n\
             end tell",
            escaped[1], escaped[0]
        ),
        3 => format!(
            "tell application \"System Events\" to tell process \"{process}\"\n\
             \tclick menu item \"{}\" of menu 1 of menu item \"{}\" of menu 1 of menu bar item \"{}\" of menu bar 1\n\
             end tell",
            escaped[2], escaped[1], escaped[0]
        ),
        _ => {
            // Build nested chain for 4+ levels
            let leaf = escaped.last().unwrap();
            let mut chain = format!("menu item \"{leaf}\"");
            for item in escaped[1..escaped.len() - 1].iter().rev() {
                chain = format!("{chain} of menu 1 of menu item \"{item}\"");
            }
            chain = format!("{chain} of menu 1 of menu bar item \"{}\"", escaped[0]);
            format!(
                "tell application \"System Events\" to tell process \"{process}\"\n\
                 \tclick {chain} of menu bar 1\n\
                 end tell"
            )
        }
    }
}

/// Maximum characters a tool response may contain before truncation.
pub(crate) const MAX_TOOL_RESPONSE_CHARS: usize = 8000;

/// Truncate a tool response to stay within context budget.
pub(crate) fn truncate_tool_response(response: &mut serde_json::Value) {
    // For get_screen_context: trim the elements list
    if let Some(arr) = response
        .get_mut("context")
        .and_then(|c| c.get_mut("elements"))
        .and_then(|e| e.as_array_mut())
    {
        if arr.len() > 30 {
            let original_count = arr.len();
            arr.truncate(30);
            arr.push(serde_json::json!({"truncated": true, "original_count": original_count}));
        }
        // Strip verbose bounds from non-first elements
        for el in arr.iter_mut().skip(1) {
            if let Some(obj) = el.as_object_mut() {
                obj.remove("bounds");
            }
        }
    }

    // General size cap: if still too large, keep essential fields + truncated error/stdout
    let serialized_len = response.to_string().len();
    if serialized_len > MAX_TOOL_RESPONSE_CHARS
        && let Some(obj) = response.as_object_mut()
    {
        let success = obj.get("success").cloned();
        let verified = obj.get("verified").cloned();
        let error = obj
            .get("error")
            .and_then(|v| v.as_str())
            .map(|s| truncate_str(s, 500));
        let stdout = obj
            .get("stdout")
            .and_then(|v| v.as_str())
            .map(|s| truncate_str(s, 500));
        let warning = obj
            .get("warning")
            .and_then(|v| v.as_str())
            .map(|s| truncate_str(s, 200));
        let post_state = obj.get("post_state").cloned().map(|mut ps| {
            // Trim focused_element details if too large
            if let Some(fe) = ps.get_mut("focused_element") {
                let truncated = fe
                    .get("value")
                    .and_then(|v| v.as_str())
                    .filter(|v| v.len() > 200)
                    .map(|v| truncate_str(v, 200));
                if let Some(t) = truncated {
                    fe.as_object_mut()
                        .unwrap()
                        .insert("value".to_string(), serde_json::Value::String(t));
                }
            }
            ps
        });
        obj.clear();
        if let Some(s) = success {
            obj.insert("success".to_string(), s);
        }
        if let Some(v) = verified {
            obj.insert("verified".to_string(), v);
        }
        if let Some(e) = error {
            obj.insert("error".to_string(), serde_json::Value::String(e));
        }
        if let Some(o) = stdout {
            obj.insert("stdout".to_string(), serde_json::Value::String(o));
        }
        if let Some(w) = warning {
            obj.insert("warning".to_string(), serde_json::Value::String(w));
        }
        if let Some(ps) = post_state {
            obj.insert("post_state".to_string(), ps);
        }
        obj.insert(
            "truncated".to_string(),
            serde_json::json!(format!(
                "Response truncated from {serialized_len} chars to save context"
            )),
        );
    }
}

pub(crate) fn truncate_str(s: &str, max_chars: usize) -> String {
    if s.len() <= max_chars {
        return s.to_string();
    }
    let mut end = max_chars.min(s.len());
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}...[truncated]", &s[..end])
}

/// Parse optional modifier keys from tool args JSON.
pub(crate) fn parse_modifiers(args: &serde_json::Value) -> Vec<String> {
    args.get("modifiers")
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default()
}

/// Returns true if the tool changes screen state and should get post_state enrichment
/// and screenshot await behavior.
pub(crate) fn is_state_changing_tool(name: &str) -> bool {
    matches!(
        name,
        "move_mouse"
            | "click"
            | "type_text"
            | "press_key"
            | "scroll"
            | "drag"
            | "click_element"
            | "activate_app"
            | "click_menu_item"
            | "context_menu_click"
            | "run_applescript"
    )
}

/// Capture post-action state: frontmost app, focused element, screenshot_delivered flag.
/// Must be called from a blocking thread (AX FFI is synchronous).
pub(crate) fn capture_post_state() -> serde_json::Value {
    let frontmost_app = aura_screen::macos::get_frontmost_app().unwrap_or_default();
    let focused = aura_screen::accessibility::get_focused_element();
    let focused_json = match focused {
        Some(el) => {
            let mut m = serde_json::json!({
                "role": el.role,
                "label": el.label,
                "value": el.value,
            });
            if let Some(ref b) = el.bounds {
                m["bounds"] = serde_json::json!({
                    "x": b.x, "y": b.y, "width": b.width, "height": b.height,
                });
            }
            m
        }
        None => serde_json::Value::Null,
    };
    serde_json::json!({
        "frontmost_app": frontmost_app,
        "focused_element": focused_json,
    })
}

/// Try PID-targeted input first, fall back to global HID.
pub(crate) async fn run_with_pid_fallback<F1, F2>(
    pid_fn: F1,
    pid_method: &'static str,
    hid_fn: F2,
    hid_method: &'static str,
) -> serde_json::Value
where
    F1: FnOnce(i32) -> anyhow::Result<()> + Send + 'static,
    F2: FnOnce() -> anyhow::Result<()> + Send + 'static,
{
    if let Some(pid) = aura_screen::macos::get_frontmost_pid() {
        let result = run_input_blocking(move || pid_fn(pid), pid_method).await;
        if result
            .get("success")
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
        {
            return result;
        }
        tracing::debug!("{pid_method} failed, falling back to HID");
    }
    run_input_blocking(hid_fn, hid_method).await
}

/// Run a blocking input operation on a dedicated thread to avoid blocking tokio.
/// Returns `{ "success": true/false, "method": method }` on success.
pub(crate) async fn run_input_blocking<F>(f: F, method: &'static str) -> serde_json::Value
where
    F: FnOnce() -> anyhow::Result<()> + Send + 'static,
{
    match tokio::task::spawn_blocking(f).await {
        Ok(Ok(())) => serde_json::json!({ "success": true, "method": method }),
        Ok(Err(e)) => serde_json::json!({ "success": false, "error": format!("{e}") }),
        Err(e) => serde_json::json!({ "success": false, "error": format!("Task panicked: {e}") }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn test_adaptive_settle_delay() {
        assert_eq!(
            settle_delay_for_tool("type_text"),
            Duration::from_millis(30)
        );
        assert_eq!(
            settle_delay_for_tool("press_key"),
            Duration::from_millis(30)
        );
        assert_eq!(settle_delay_for_tool("click"), Duration::from_millis(100));
        assert_eq!(
            settle_delay_for_tool("click_element"),
            Duration::from_millis(100)
        );
        assert_eq!(
            settle_delay_for_tool("activate_app"),
            Duration::from_millis(150)
        );
        assert_eq!(
            settle_delay_for_tool("click_menu_item"),
            Duration::from_millis(150)
        );
        assert_eq!(
            settle_delay_for_tool("run_applescript"),
            Duration::from_millis(200)
        );
        assert_eq!(settle_delay_for_tool("scroll"), Duration::from_millis(50));
        assert_eq!(settle_delay_for_tool("drag"), Duration::from_millis(50));
    }

    #[test]
    fn run_applescript_is_state_changing() {
        assert!(is_state_changing_tool("run_applescript"));
    }

    #[test]
    fn truncate_preserves_error_and_stdout() {
        let large_output = "x".repeat(10_000);
        let mut response = serde_json::json!({
            "success": false,
            "error": format!("Something went wrong: {large_output}"),
            "stdout": format!("Output: {large_output}"),
            "verified": true
        });
        truncate_tool_response(&mut response);
        let obj = response.as_object().unwrap();
        assert!(obj.contains_key("error"), "error field should be preserved");
        assert!(
            obj.contains_key("stdout"),
            "stdout field should be preserved"
        );
        assert!(
            obj.contains_key("success"),
            "success field should be preserved"
        );
        let error_len = obj["error"].as_str().unwrap().len();
        let stdout_len = obj["stdout"].as_str().unwrap().len();
        assert!(
            error_len <= 520,
            "error should be truncated, got {error_len}"
        );
        assert!(
            stdout_len <= 520,
            "stdout should be truncated, got {stdout_len}"
        );
    }

    #[test]
    fn truncate_small_response_unchanged() {
        let mut response = serde_json::json!({
            "success": true,
            "stdout": "hello"
        });
        let original = response.clone();
        truncate_tool_response(&mut response);
        assert_eq!(response, original);
    }

    #[test]
    fn build_menu_click_script_two_level() {
        let script = build_menu_click_script("Safari", &["File".into(), "New Window".into()]);
        assert!(script.contains("tell process \"Safari\""));
        assert!(script.contains("click menu item \"New Window\""));
        assert!(script.contains("menu bar item \"File\""));
    }

    #[test]
    fn build_menu_click_script_three_level() {
        let script = build_menu_click_script(
            "Safari",
            &[
                "View".into(),
                "Developer".into(),
                "JavaScript Console".into(),
            ],
        );
        assert!(script.contains("tell process \"Safari\""));
        assert!(script.contains("click menu item \"JavaScript Console\""));
        assert!(script.contains("menu item \"Developer\""));
        assert!(script.contains("menu bar item \"View\""));
    }

    #[test]
    fn build_menu_click_script_sanitizes_quotes() {
        let script = build_menu_click_script("My\"App", &["Fi\\le".into(), "Sa\"ve".into()]);
        assert!(!script.contains(r#"My"App"#));
        assert!(script.contains("tell process \"MyApp\""));
        assert!(script.contains("menu item \"Save\""));
        assert!(script.contains("menu bar item \"File\""));
    }

    #[test]
    fn truncate_preserves_post_state_and_warning() {
        let large_output = "x".repeat(10_000);
        let mut response = serde_json::json!({
            "success": true,
            "verified": false,
            "warning": "screen_unchanged_but_element_focused",
            "stdout": large_output,
            "post_state": {
                "frontmost_app": "Safari",
                "focused_element": {
                    "role": "AXTextField",
                    "label": "Address and Search",
                },
                "screenshot_delivered": false,
            }
        });
        truncate_tool_response(&mut response);
        let obj = response.as_object().unwrap();
        assert!(
            obj.contains_key("post_state"),
            "post_state must survive truncation"
        );
        assert!(
            obj.contains_key("warning"),
            "warning must survive truncation"
        );
        assert_eq!(
            obj["post_state"]["frontmost_app"].as_str().unwrap(),
            "Safari"
        );
    }
}
