use std::time::Duration;

use aura_bridge::script::{ScriptExecutor, ScriptLanguage};
use aura_screen::macos::MacOSScreenReader;

use super::is_automation_denied;
use super::tool_helpers::{
    build_menu_click_script, click_element_inner, run_input_blocking, run_with_pid_fallback,
};

// Re-export helpers used by processor.rs via `tools::` path
pub(crate) use super::tool_helpers::{
    FrameDims, capture_post_state, is_state_changing_tool, truncate_tool_response,
};

/// Maximum characters allowed in a single type_text tool call.
const TYPE_TEXT_MAX_CHARS: usize = 10_000;

/// Maximum click count for click tool (1 = single, 2 = double, 3 = triple).
const CLICK_COUNT_MAX: u32 = 3;

/// Maximum absolute scroll amount in either axis.
const SCROLL_MAX: i32 = 1000;

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
            // Brief delay to let apps register hover state before clicking
            tokio::time::sleep(std::time::Duration::from_millis(40)).await;
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
