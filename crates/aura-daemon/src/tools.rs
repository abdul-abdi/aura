use std::time::Duration;

use aura_bridge::script::{ScriptExecutor, ScriptLanguage};
use aura_screen::macos::MacOSScreenReader;

use super::is_automation_denied;

/// Maximum characters allowed in a single type_text tool call.
const TYPE_TEXT_MAX_CHARS: usize = 10_000;

/// Maximum click count for click tool (1 = single, 2 = double, 3 = triple).
const CLICK_COUNT_MAX: u32 = 3;

/// Maximum absolute scroll amount in either axis.
const SCROLL_MAX: i32 = 1000;

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

pub(crate) async fn execute_tool(
    name: &str,
    args: &serde_json::Value,
    executor: &ScriptExecutor,
    screen_reader: &MacOSScreenReader,
    dims: FrameDims,
) -> serde_json::Value {
    match name {
        "run_applescript" => {
            let script = args.get("script").and_then(|v| v.as_str()).unwrap_or("");
            let language = match args.get("language").and_then(|v| v.as_str()) {
                Some("javascript") => ScriptLanguage::JavaScript,
                _ => ScriptLanguage::AppleScript,
            };

            // Pre-check Automation permission for the target app (if identifiable).
            // This avoids running scripts that will definitely fail because the user
            // previously denied Automation access. Scripts targeting apps where
            // permission hasn't been decided yet proceed normally (macOS shows the
            // one-time consent popup).
            if let Some(target_app) = aura_bridge::automation::extract_target_app(script)
                && let Some(bundle_id) = aura_bridge::automation::app_name_to_bundle_id(&target_app)
            {
                let perm = aura_bridge::automation::check_automation_permission(bundle_id);
                if perm == aura_bridge::automation::AutomationPermission::Denied {
                    tracing::warn!(
                        target_app = %target_app,
                        "Automation permission denied for {target_app} — skipping script"
                    );
                    return serde_json::json!({
                        "success": false,
                        "error": format!(
                            "Automation permission for {target_app} is denied. \
                             The user must grant it in System Settings > Privacy & Security > Automation, \
                             then toggle Aura's access to {target_app} on."
                        ),
                        "error_kind": "automation_denied",
                    });
                }
            }

            let timeout = args
                .get("timeout_secs")
                .and_then(|v| v.as_u64())
                .unwrap_or(30);
            let result = executor.run(script, language, timeout).await;

            // Detect Automation denial from osascript stderr (covers cases where
            // the preflight couldn't identify the target app or bundle ID).
            if !result.success && is_automation_denied(&result.stderr) {
                let target = aura_bridge::automation::extract_target_app(script)
                    .unwrap_or_else(|| "the target app".to_string());
                return serde_json::json!({
                    "success": false,
                    "error": format!(
                        "Automation permission for {target} was denied by the user. \
                         Tell the user to grant it in System Settings > Privacy & Security > Automation, \
                         then toggle Aura's access to {target} on. Do not retry this script."
                    ),
                    "error_kind": "automation_denied",
                    "stderr": result.stderr,
                });
            }

            serde_json::json!({
                "success": result.success,
                "stdout": result.stdout,
                "stderr": result.stderr,
            })
        }
        "get_screen_context" => match screen_reader.capture_context() {
            Ok(ctx) => serde_json::json!({ "success": true, "context": ctx.summary() }),
            Err(e) => serde_json::json!({ "success": false, "error": format!("{e}") }),
        },
        // All input tools (mouse/keyboard) require Accessibility permission.
        // CGEvent.post() silently drops events without it — check before executing
        // so Gemini gets an honest failure instead of a fake success.
        "move_mouse" | "click" | "type_text" | "press_key" | "scroll" | "drag"
            if !aura_input::accessibility::check_accessibility(false) =>
        {
            serde_json::json!({
                "success": false,
                "error": "Accessibility permission is not granted. \
                          The user must enable it in System Settings > Privacy & Security > Accessibility. \
                          Without it, mouse and keyboard actions are silently ignored by macOS.",
                "error_kind": "accessibility_denied",
            })
        }
        "click_element" => {
            if !aura_input::accessibility::check_accessibility(false) {
                return serde_json::json!({
                    "success": false,
                    "error": "Accessibility permission is not granted. \
                              Required for click_element to read UI elements and click. \
                              Enable in System Settings > Privacy & Security > Accessibility.",
                    "error_kind": "accessibility_denied",
                });
            }

            let label = args.get("label").and_then(|v| v.as_str()).map(String::from);
            let role = args.get("role").and_then(|v| v.as_str()).map(String::from);
            let index = args.get("index").and_then(|v| v.as_u64()).unwrap_or(0) as usize;

            // Run AX tree walk + click on blocking thread (FFI calls are synchronous)
            match tokio::task::spawn_blocking(move || {
                click_element_inner(label.as_deref(), role.as_deref(), index)
            })
            .await
            {
                Ok(result) => result,
                Err(e) => serde_json::json!({
                    "success": false,
                    "error": format!("Task panicked: {e}"),
                }),
            }
        }
        "move_mouse" => {
            let raw_x = args.get("x").and_then(|v| v.as_f64()).unwrap_or(0.0);
            let raw_y = args.get("y").and_then(|v| v.as_f64()).unwrap_or(0.0);
            let x = dims.to_logical_x(raw_x);
            let y = dims.to_logical_y(raw_y);
            run_with_pid_fallback(
                move |pid| aura_input::mouse::move_mouse_pid(x, y, pid),
                "pid_move",
                move || aura_input::mouse::move_mouse(x, y),
                "hid_move",
            )
            .await
        }
        "click" => {
            let raw_x = args.get("x").and_then(|v| v.as_f64()).unwrap_or(0.0);
            let raw_y = args.get("y").and_then(|v| v.as_f64()).unwrap_or(0.0);
            let x = dims.to_logical_x(raw_x);
            let y = dims.to_logical_y(raw_y);
            let button = args
                .get("button")
                .and_then(|v| v.as_str())
                .unwrap_or("left")
                .to_string();
            // S7: Clamp click_count to 1..=3
            let count = args
                .get("click_count")
                .and_then(|v| v.as_u64())
                .unwrap_or(1) as u32;
            let count = count.clamp(1, CLICK_COUNT_MAX);
            // Pre-move cursor to click target so apps register hover state
            // before receiving the click event.
            let pre_x = x;
            let pre_y = y;
            run_input_blocking(
                move || aura_input::mouse::move_mouse(pre_x, pre_y),
                "pre_click_move",
            )
            .await;
            let btn = button.clone();
            run_with_pid_fallback(
                move |pid| aura_input::mouse::click_pid(x, y, &btn, count, pid),
                "pid_click",
                move || aura_input::mouse::click(x, y, &button, count),
                "hid_click",
            )
            .await
        }
        "type_text" => {
            let text = args
                .get("text")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            // S6: Cap type_text at 10,000 characters (char-aware to avoid UTF-8 panics)
            let text = if text.chars().count() > TYPE_TEXT_MAX_CHARS {
                tracing::warn!(
                    len = text.chars().count(),
                    max = TYPE_TEXT_MAX_CHARS,
                    "type_text input truncated"
                );
                text.chars().take(TYPE_TEXT_MAX_CHARS).collect::<String>()
            } else {
                text
            };

            // If label/role provided, focus the target element first via AX
            let label = args.get("label").and_then(|v| v.as_str()).map(String::from);
            let role = args.get("role").and_then(|v| v.as_str()).map(String::from);
            if label.is_some() || role.is_some() {
                let focus_label = label.clone();
                let focus_role = role.clone();
                let _ = tokio::task::spawn_blocking(move || {
                    let result = aura_screen::accessibility::ax_set_focused(
                        focus_label.as_deref(),
                        focus_role.as_deref(),
                    );
                    if !result.success {
                        tracing::debug!(
                            error = ?result.error,
                            "ax_set_focused failed, will type at current focus"
                        );
                    }
                })
                .await;
                // Pause for focus to settle (60ms covers Electron/browser apps)
                tokio::time::sleep(Duration::from_millis(60)).await;
            }

            // Type via keyboard synthesis (triggers onChange/validation in target apps)
            // PID-targeted first, then HID fallback
            let pid_text = text.clone();
            run_with_pid_fallback(
                move |pid| aura_input::keyboard::type_text_pid(&pid_text, pid),
                "pid_type",
                move || aura_input::keyboard::type_text(&text),
                "hid_type",
            )
            .await
        }
        "press_key" => {
            let key_name = args
                .get("key")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let modifiers: Vec<String> = args
                .get("modifiers")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str().map(String::from))
                        .collect()
                })
                .unwrap_or_default();
            match aura_input::keyboard::keycode_from_name(&key_name) {
                Some(keycode) => {
                    let mods = modifiers.clone();
                    run_with_pid_fallback(
                        move |pid| {
                            let mod_refs: Vec<&str> = mods.iter().map(|s| s.as_str()).collect();
                            aura_input::keyboard::press_key_pid(keycode, &mod_refs, pid)
                        },
                        "pid_key",
                        move || {
                            let mod_refs: Vec<&str> =
                                modifiers.iter().map(|s| s.as_str()).collect();
                            aura_input::keyboard::press_key(keycode, &mod_refs)
                        },
                        "hid_key",
                    )
                    .await
                }
                None => {
                    serde_json::json!({ "success": false, "error": format!("Unknown key: {key_name}") })
                }
            }
        }
        "scroll" => {
            // S7: Clamp scroll amounts to -1000..=1000 (clamp at i64 before cast to avoid wrap)
            let dx = args
                .get("dx")
                .and_then(|v| v.as_i64())
                .unwrap_or(0)
                .clamp(-(SCROLL_MAX as i64), SCROLL_MAX as i64) as i32;
            let dy = args
                .get("dy")
                .and_then(|v| v.as_i64())
                .unwrap_or(0)
                .clamp(-(SCROLL_MAX as i64), SCROLL_MAX as i64) as i32;
            run_with_pid_fallback(
                move |pid| aura_input::mouse::scroll_pid(dx, dy, pid),
                "pid_scroll",
                move || aura_input::mouse::scroll(dx, dy),
                "hid_scroll",
            )
            .await
        }
        "drag" => {
            let raw_fx = args.get("from_x").and_then(|v| v.as_f64()).unwrap_or(0.0);
            let raw_fy = args.get("from_y").and_then(|v| v.as_f64()).unwrap_or(0.0);
            let raw_tx = args.get("to_x").and_then(|v| v.as_f64()).unwrap_or(0.0);
            let raw_ty = args.get("to_y").and_then(|v| v.as_f64()).unwrap_or(0.0);
            let fx = dims.to_logical_x(raw_fx);
            let fy = dims.to_logical_y(raw_fy);
            let tx = dims.to_logical_x(raw_tx);
            let ty = dims.to_logical_y(raw_ty);
            run_with_pid_fallback(
                move |pid| aura_input::mouse::drag_pid(fx, fy, tx, ty, pid),
                "pid_drag",
                move || aura_input::mouse::drag(fx, fy, tx, ty),
                "hid_drag",
            )
            .await
        }
        "activate_app" => {
            let name = args.get("name").and_then(|v| v.as_str()).unwrap_or("");
            if name.is_empty() {
                return serde_json::json!({
                    "success": false,
                    "error": "name parameter is required"
                });
            }
            // Sanitize app name to prevent AppleScript injection
            let safe_name = name.replace(['\\', '"'], "");
            let script = format!(r#"tell application "{safe_name}" to activate"#);

            // Pre-check automation permission if we know the bundle ID
            if let Some(bundle_id) = aura_bridge::automation::app_name_to_bundle_id(&safe_name) {
                let perm = aura_bridge::automation::check_automation_permission(bundle_id);
                if perm == aura_bridge::automation::AutomationPermission::Denied {
                    return serde_json::json!({
                        "success": false,
                        "error": format!(
                            "Automation permission for {safe_name} is denied. \
                             Grant in System Settings > Privacy & Security > Automation."
                        ),
                        "error_kind": "automation_denied",
                    });
                }
            }

            let result = executor.run(&script, ScriptLanguage::AppleScript, 10).await;
            // Invalidate PID/app cache since frontmost app changed
            aura_screen::macos::clear_frontmost_cache();
            if result.success {
                serde_json::json!({
                    "success": true,
                    "app": safe_name,
                })
            } else {
                serde_json::json!({
                    "success": false,
                    "app": safe_name,
                    "error": result.stderr,
                })
            }
        }
        "click_menu_item" => {
            let menu_path: Vec<String> = args
                .get("menu_path")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str().map(String::from))
                        .collect()
                })
                .unwrap_or_default();

            if menu_path.len() < 2 {
                return serde_json::json!({
                    "success": false,
                    "error": "menu_path requires at least 2 items: [\"MenuBarItem\", \"MenuItem\", ...\"SubmenuItem\"]"
                });
            }

            // Determine target app
            let target_app = if let Some(app) = args.get("app").and_then(|v| v.as_str()) {
                app.to_string()
            } else {
                match screen_reader.capture_context() {
                    Ok(ctx) => ctx.frontmost_app().to_string(),
                    Err(_) => {
                        return serde_json::json!({
                            "success": false,
                            "error": "Could not determine frontmost app. Specify 'app' parameter."
                        });
                    }
                }
            };

            // Pre-check automation permission for System Events
            if let Some(bundle_id) = aura_bridge::automation::app_name_to_bundle_id("System Events")
            {
                let perm = aura_bridge::automation::check_automation_permission(bundle_id);
                if perm == aura_bridge::automation::AutomationPermission::Denied {
                    return serde_json::json!({
                        "success": false,
                        "error": "Automation permission for System Events is denied. \
                                 Grant in System Settings > Privacy & Security > Automation.",
                        "error_kind": "automation_denied",
                    });
                }
            }

            let script = build_menu_click_script(&target_app, &menu_path);
            let result = executor.run(&script, ScriptLanguage::AppleScript, 10).await;
            if result.success {
                serde_json::json!({
                    "success": true,
                    "clicked": menu_path.join(" > "),
                })
            } else {
                serde_json::json!({
                    "success": false,
                    "error": format!("Menu item not found or click failed: {}", result.stderr),
                    "stderr": result.stderr,
                })
            }
        }
        other => serde_json::json!({
            "success": false,
            "error": format!("Unknown tool: {other}"),
        }),
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

    let bounds = match &target.bounds {
        Some(b) => b,
        None => {
            return serde_json::json!({
                "success": false,
                "error": "Element found but has no bounds (may be offscreen or hidden)",
                "element": {
                    "role": target.role,
                    "label": target.label,
                },
            });
        }
    };

    // AX bounds are already in logical screen coordinates — no FrameDims conversion needed
    let (center_x, center_y) = bounds.center();

    // Pre-move cursor and attempt focus before coordinate-based click fallback.
    // Some apps need hover state and/or focus to register clicks.
    let _ = aura_input::mouse::move_mouse(center_x, center_y);
    std::thread::sleep(std::time::Duration::from_millis(15));
    let _ = aura_screen::accessibility::ax_set_focused(label, role);

    // Try 2: PID-targeted click
    if let Some(pid) = aura_screen::macos::get_frontmost_pid() {
        if aura_input::mouse::click_pid(center_x, center_y, "left", 1, pid).is_ok() {
            return serde_json::json!({
                "success": true,
                "method": "pid_click",
                "element": {
                    "role": target.role,
                    "label": target.label,
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
    match aura_input::mouse::click(center_x, center_y, "left", 1) {
        Ok(()) => serde_json::json!({
            "success": true,
            "method": "hid_click",
            "element": {
                "role": target.role,
                "label": target.label,
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

/// Truncate a tool response to stay within context budget.
const MAX_TOOL_RESPONSE_CHARS: usize = 8000;

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
        s.to_string()
    } else {
        format!("{}...[truncated]", &s[..max_chars])
    }
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
}
