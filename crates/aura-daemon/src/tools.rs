use std::time::Duration;

use aura_bridge::script::{ScriptExecutor, ScriptLanguage};
use aura_screen::macos::MacOSScreenReader;
use base64::Engine;

use super::is_automation_denied;
use super::tool_helpers::{
    build_menu_click_script, click_element_inner, run_input_blocking, run_with_pid_fallback,
};

// Re-export helpers used by processor.rs via `tools::` path
pub(crate) use super::tool_helpers::{
    FrameDims, MAX_CLICK_RETRIES, SPIRAL_RADIUS, capture_post_state, is_state_changing_tool,
    point_in_denormalized_bounds, settle_delay_for_tool, spiral_offsets, truncate_tool_response,
};

/// Maximum characters allowed in a single type_text tool call.
const TYPE_TEXT_MAX_CHARS: usize = 10_000;

/// Maximum click count for click tool (1 = single, 2 = double, 3 = triple).
const CLICK_COUNT_MAX: u32 = 3;

/// Maximum absolute scroll amount in either axis.
const SCROLL_MAX: i32 = 1000;

/// Maximum timeout allowed for a single run_applescript call.
const MAX_APPLESCRIPT_TIMEOUT_SECS: u64 = 120;

pub(crate) async fn execute_tool(
    name: &str,
    args: &serde_json::Value,
    executor: &ScriptExecutor,
    screen_reader: &MacOSScreenReader,
    dims: FrameDims,
    vision_oracle: Option<&aura_gemini::vision_oracle::VisionOracle>,
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
                let bundle = bundle_id.to_string();
                let perm = tokio::task::spawn_blocking(move || {
                    aura_bridge::automation::check_automation_permission(&bundle)
                })
                .await
                .unwrap_or(aura_bridge::automation::AutomationPermission::Unknown(-1));
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
                .unwrap_or(30)
                .min(MAX_APPLESCRIPT_TIMEOUT_SECS);
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
        "get_screen_context" => {
            let ctx = screen_reader.capture_context();
            let mut response = match ctx {
                Ok(ctx) => serde_json::json!({ "success": true, "context": ctx.summary() }),
                Err(e) => return serde_json::json!({ "success": false, "error": format!("{e}") }),
            };
            // Capture high-res frame and run SoM overlay for visual element targeting.
            // SoM runs on the 2560px high-res capture for better edge detection, but
            // the mark coordinates must be scaled to the streaming-frame space (1920px)
            // that Gemini uses for click(x,y) calls.
            if let Ok(Ok(frame)) =
                tokio::task::spawn_blocking(aura_screen::capture::capture_screen_high_res).await
                && let Ok(jpeg_bytes) =
                    base64::engine::general_purpose::STANDARD.decode(&frame.jpeg_base64)
                && let Some((_annotated_b64, marks)) =
                    aura_screen::capture::annotate_with_som(&jpeg_bytes)
                && !marks.is_empty()
            {
                // Scale from high-res (2560px) to streaming-frame space (dims.img_w, typically 1920px).
                // This ensures Gemini can pass mark coordinates directly to click(x, y).
                let scale_x = dims.img_w as f64 / frame.width.max(1) as f64;
                let scale_y = dims.img_h as f64 / frame.height.max(1) as f64;
                let marks_json: Vec<serde_json::Value> = marks
                    .iter()
                    .map(|m| {
                        let cx = ((m.x + m.width / 2) as f64 * scale_x) as u32;
                        let cy = ((m.y + m.height / 2) as f64 * scale_y) as u32;
                        serde_json::json!({
                            "mark": m.id,
                            "center_x": cx,
                            "center_y": cy,
                            "bounds": {
                                "x": (m.x as f64 * scale_x) as u32,
                                "y": (m.y as f64 * scale_y) as u32,
                                "w": (m.width as f64 * scale_x) as u32,
                                "h": (m.height as f64 * scale_y) as u32,
                            },
                        })
                    })
                    .collect();
                if let Some(obj) = response.as_object_mut() {
                    obj.insert("visual_marks".to_string(), serde_json::json!(marks_json));
                    obj.insert(
                        "visual_marks_note".to_string(),
                        "Numbered marks detected on screen. Use center_x/center_y with click tool for precise targeting.".into(),
                    );
                }
            }
            response
        }
        // All input tools (mouse/keyboard) require Accessibility permission.
        // CGEvent.post() silently drops events without it — check before executing
        // so Gemini gets an honest failure instead of a fake success.
        "move_mouse" | "click" | "type_text" | "press_key" | "scroll" | "drag" | "key_state"
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
            let raw_x = match args.get("x").and_then(|v| v.as_f64()) {
                Some(v) => v,
                None => return serde_json::json!({"error": "missing required parameter: x"}),
            };
            let raw_y = match args.get("y").and_then(|v| v.as_f64()) {
                Some(v) => v,
                None => return serde_json::json!({"error": "missing required parameter: y"}),
            };
            let mut x = dims.to_logical_x(raw_x);
            let mut y = dims.to_logical_y(raw_y);
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

            // Extract target description for vision oracle
            let target = args
                .get("target")
                .and_then(|v| v.as_str())
                .map(String::from);

            // Oracle-first coordinate refinement with 4s total budget
            let mut targeting_info = serde_json::json!({});
            let raw_x = x;
            let raw_y = y;

            /// Maximum distance (logical px) the oracle can move coords from the hint.
            /// Beyond this, the oracle result is discarded as unreliable.
            const MAX_ORACLE_DELTA: f64 = 150.0;

            /// Total time budget for oracle refinement (capture + API call + retry).
            const ORACLE_BUDGET: Duration = Duration::from_secs(4);

            // Get display origin for multi-monitor (used by BOTH oracle and fallback paths)
            let display_origin = tokio::task::spawn_blocking(|| {
                aura_screen::capture::get_active_display_origin()
            })
            .await
            .ok()
            .flatten()
            .unwrap_or((0.0, 0.0));

            // Bug 3: Capture frontmost app before oracle so we can detect an app switch
            let pre_oracle_app = tokio::task::spawn_blocking(|| {
                aura_screen::macos::get_frontmost_app().unwrap_or_default()
            })
            .await
            .unwrap_or_default();

            if let Some(oracle) = vision_oracle.filter(|o| o.is_available()) {
                let oracle_start = std::time::Instant::now();
                tracing::info!(x, y, target = ?target, "Querying vision oracle");

                let oracle_result = tokio::time::timeout(ORACLE_BUDGET, async {
                    let frame = tokio::task::spawn_blocking(|| {
                        aura_screen::capture::capture_screen_high_res()
                    })
                    .await
                    .map_err(|e| anyhow::anyhow!("Screenshot task panicked: {e}"))?
                    .map_err(|e| anyhow::anyhow!("Screenshot capture failed: {e}"))?;

                    // Bug 5: Check for censored screenshot before sending to oracle
                    if let Ok(jpeg_bytes) = base64::engine::general_purpose::STANDARD
                        .decode(&frame.jpeg_base64)
                    {
                        if let Ok(img) = image::load_from_memory_with_format(
                            &jpeg_bytes,
                            image::ImageFormat::Jpeg,
                        ) {
                            let rgb = img.to_rgb8();
                            if aura_screen::capture::frame_looks_censored(
                                rgb.as_raw(),
                                rgb.width() as usize,
                                rgb.height() as usize,
                            ) {
                                anyhow::bail!(
                                    "Screenshot appears censored — Screen Recording permission may be revoked"
                                );
                            }
                        }
                    }

                    // Capture frame's display origin now — same capture, no TOCTOU race.
                    let frame_origin = (frame.display_origin_x, frame.display_origin_y);

                    // Bug 8: Track elapsed time inside the oracle block to implement
                    // budget-aware retry (skip retry if less than 2s remain).
                    let oracle_inner_start = std::time::Instant::now();

                    let target_ref = target.as_deref();
                    let mut result = oracle
                        .find_element_coordinates(
                            &frame.jpeg_base64,
                            x, y,
                            frame.width, frame.height,
                            frame.logical_width, frame.logical_height,
                            target_ref,
                            frame.display_origin_x, frame.display_origin_y,
                        )
                        .await;

                    // Single retry on failure (Err only, not Ok(None))
                    // Bug 8: Only retry if enough budget remains (>= 2s elapsed means skip)
                    if result.is_err() {
                        let elapsed_so_far = oracle_inner_start.elapsed();
                        if elapsed_so_far < Duration::from_secs(2) {
                            tracing::warn!(
                                "Vision oracle attempt 1 failed, retrying ({:.1}s remaining)",
                                (Duration::from_secs(4) - elapsed_so_far).as_secs_f64()
                            );
                            result = oracle
                                .find_element_coordinates(
                                    &frame.jpeg_base64,
                                    x, y,
                                    frame.width, frame.height,
                                    frame.logical_width, frame.logical_height,
                                    target_ref,
                                    frame.display_origin_x, frame.display_origin_y,
                                )
                                .await;
                        } else {
                            tracing::warn!(
                                elapsed_ms = elapsed_so_far.as_millis() as u64,
                                "Vision oracle attempt 1 failed, skipping retry (insufficient budget)"
                            );
                        }
                    }

                    // Return the oracle result together with the frame's display origin so
                    // the delta comparison uses the same origin the oracle saw (Bug 2 fix).
                    Ok::<(anyhow::Result<Option<(f64, f64)>>, (f64, f64)), anyhow::Error>((result, frame_origin))
                })
                .await;

                let elapsed_ms = oracle_start.elapsed().as_millis() as u64;

                // Destructure the oracle result to separate the frame's display origin from
                // the oracle coordinate result. For error/timeout paths we fall back to the
                // separately-fetched display_origin (Bug 2 fix).
                enum OracleOutcome {
                    Found((f64, f64), (f64, f64)),      // (coords, frame_origin)
                    NotFound((f64, f64)),                 // frame_origin
                    Failed(anyhow::Error, (f64, f64)),   // (error, frame_origin)
                    TimedOut,
                }
                let outcome = match oracle_result {
                    Ok(Ok((Ok(Some(coords)), origin))) => OracleOutcome::Found(coords, origin),
                    Ok(Ok((Ok(None), origin)))          => OracleOutcome::NotFound(origin),
                    Ok(Ok((Err(e), origin)))             => OracleOutcome::Failed(e, origin),
                    Ok(Err(e))                           => OracleOutcome::Failed(e, display_origin),
                    Err(_elapsed)                        => OracleOutcome::TimedOut,
                };

                match outcome {
                    // Oracle found the target
                    OracleOutcome::Found((ox, oy), frame_display_origin) => {
                        // Use the frame's display origin for a fair delta comparison —
                        // this is the origin the oracle actually saw (Bug 2 fix).
                        let global_raw_x = raw_x + frame_display_origin.0;
                        let global_raw_y = raw_y + frame_display_origin.1;
                        let delta = ((ox - global_raw_x).powi(2) + (oy - global_raw_y).powi(2)).sqrt();

                        // Axis-swap detection: if swapping ox/oy produces a much closer match,
                        // Gemini likely returned [x,y] instead of [y,x] (Bug 1 fix).
                        let swapped_delta = ((oy - global_raw_x).powi(2) + (ox - global_raw_y).powi(2)).sqrt();
                        let likely_swapped = swapped_delta < delta * 0.5 && delta > 50.0;
                        if likely_swapped {
                            tracing::warn!(
                                delta = format!("{:.1}", delta),
                                swapped_delta = format!("{:.1}", swapped_delta),
                                "Oracle likely returned [x,y] instead of [y,x] — discarding"
                            );
                            oracle.record_success();
                            x = global_raw_x;
                            y = global_raw_y;
                            targeting_info = serde_json::json!({
                                "vision_oracle": false,
                                "raw_coords": [raw_x, raw_y],
                                "oracle_coords": [ox, oy],
                                "delta_px": (delta * 10.0).round() / 10.0,
                                "elapsed_ms": elapsed_ms,
                                "targeting_hint": "Oracle axis swap detected — using raw coords",
                            });
                        } else if delta > MAX_ORACLE_DELTA {
                            tracing::warn!(
                                delta = format!("{:.1}", delta),
                                max = MAX_ORACLE_DELTA,
                                "Oracle delta too large — discarding, using raw coords"
                            );
                            oracle.record_success(); // API worked, just result was weird
                            // Use frame's display origin (same as delta comparison, Bug 2 fix)
                            x = raw_x + frame_display_origin.0;
                            y = raw_y + frame_display_origin.1;
                            targeting_info = serde_json::json!({
                                "vision_oracle": false,
                                "raw_coords": [raw_x, raw_y],
                                "oracle_coords": [ox, oy],
                                "delta_px": (delta * 10.0).round() / 10.0,
                                "elapsed_ms": elapsed_ms,
                                "targeting_hint": "Oracle delta exceeded threshold — using raw coords",
                            });
                        } else {
                            oracle.record_success();
                            x = ox; // Already includes display origin from oracle
                            y = oy;
                            targeting_info = serde_json::json!({
                                "vision_oracle": true,
                                "raw_coords": [raw_x, raw_y],
                                "oracle_coords": [ox, oy],
                                "delta_px": (delta * 10.0).round() / 10.0,
                                "elapsed_ms": elapsed_ms,
                            });
                        }
                    }
                    // Oracle says target not visible (NotFound) — NOT a failure
                    OracleOutcome::NotFound(frame_display_origin) => {
                        oracle.record_success(); // API worked correctly
                        // Use frame's display origin (same capture the oracle saw, Bug 2 fix)
                        x = raw_x + frame_display_origin.0;
                        y = raw_y + frame_display_origin.1;
                        tracing::info!(elapsed_ms, "Vision oracle: target not visible, using raw coords");
                        targeting_info = serde_json::json!({
                            "vision_oracle": false,
                            "raw_coords": [raw_x, raw_y],
                            "oracle_not_found": true,
                            "elapsed_ms": elapsed_ms,
                            "targeting_hint": "Oracle: target not visible — using raw coordinates",
                        });
                    }
                    // Real API/parse failure — increments circuit breaker
                    OracleOutcome::Failed(e, frame_display_origin) => {
                        oracle.record_failure();
                        // Use frame's display origin when available, else fallback (Bug 2 fix)
                        x = raw_x + frame_display_origin.0;
                        y = raw_y + frame_display_origin.1;
                        tracing::warn!(error = %e, elapsed_ms, "Vision oracle failed");
                        targeting_info = serde_json::json!({
                            "vision_oracle": false,
                            "raw_coords": [raw_x, raw_y],
                            "oracle_error": format!("{e}"),
                            "elapsed_ms": elapsed_ms,
                            "targeting_hint": "Oracle failed — using raw coordinates",
                        });
                    }
                    OracleOutcome::TimedOut => {
                        // Timeout — increments circuit breaker
                        oracle.record_failure();
                        // Apply display origin to raw coords (multi-monitor fix)
                        x = raw_x + display_origin.0;
                        y = raw_y + display_origin.1;
                        tracing::warn!("Vision oracle timed out after 4s");
                        targeting_info = serde_json::json!({
                            "vision_oracle": false,
                            "raw_coords": [raw_x, raw_y],
                            "oracle_error": "timeout",
                            "elapsed_ms": elapsed_ms,
                            "targeting_hint": "Oracle timed out — using raw coordinates",
                        });
                    }
                }
            } else {
                // Oracle unavailable (not configured or circuit breaker tripped)
                // Apply display origin to raw coords (multi-monitor fix)
                x = raw_x + display_origin.0;
                y = raw_y + display_origin.1;
                let reason = if vision_oracle.is_some() { "circuit_breaker" } else { "not_configured" };
                targeting_info = serde_json::json!({
                    "vision_oracle": false,
                    "raw_coords": [raw_x, raw_y],
                    "oracle_skipped": reason,
                    "targeting_hint": "Oracle unavailable — using raw coordinates",
                });
            }

            // Bug 3: Detect app switch during oracle latency
            let post_oracle_app = tokio::task::spawn_blocking(|| {
                aura_screen::macos::get_frontmost_app().unwrap_or_default()
            })
            .await
            .unwrap_or_default();
            if !pre_oracle_app.is_empty()
                && !post_oracle_app.is_empty()
                && pre_oracle_app != post_oracle_app
            {
                tracing::warn!(
                    pre_app = %pre_oracle_app,
                    post_app = %post_oracle_app,
                    "Frontmost app changed during oracle — falling back to raw coords"
                );
                x = raw_x + display_origin.0;
                y = raw_y + display_origin.1;
                targeting_info = serde_json::json!({
                    "vision_oracle": false,
                    "raw_coords": [raw_x, raw_y],
                    "app_switch_detected": true,
                    "pre_app": pre_oracle_app,
                    "post_app": post_oracle_app,
                    "targeting_hint": "App changed during oracle — using raw coordinates",
                });
            }

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
            let modifiers = crate::tool_helpers::parse_modifiers(args);
            let btn = button.clone();
            let mods = modifiers.clone();
            let mut result = run_with_pid_fallback(
                move |pid| {
                    let mod_refs: Vec<&str> = mods.iter().map(|s| s.as_str()).collect();
                    aura_input::mouse::click_pid(x, y, &btn, count, &mod_refs, pid)
                },
                "pid_click",
                move || {
                    let mod_refs: Vec<&str> = modifiers.iter().map(|s| s.as_str()).collect();
                    aura_input::mouse::click(x, y, &button, count, &mod_refs)
                },
                "hid_click",
            )
            .await;

            // Merge targeting info into click result
            if let Some(obj) = result.as_object_mut()
                && let Some(ti) = targeting_info.as_object()
            {
                for (k, v) in ti {
                    obj.insert(k.clone(), v.clone());
                }
            }
            result
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

            // Detect password fields and route through clipboard paste instead of synthetic keys
            let is_secure =
                tokio::task::spawn_blocking(aura_screen::accessibility::is_focused_element_secure)
                    .await
                    .unwrap_or(false);

            if is_secure {
                tracing::info!("Secure text field detected — routing through clipboard paste");
                // Save current clipboard so we can restore it after pasting
                let prev_clipboard = aura_screen::macos::get_clipboard();
                if let Err(e) = aura_screen::macos::set_clipboard(&text) {
                    return serde_json::json!({
                        "success": false,
                        "error": format!("Failed to write to clipboard for secure field: {e}"),
                    });
                }
                // Paste with Cmd+V
                let paste_result = run_input_blocking(
                    || {
                        aura_input::keyboard::press_key(
                            aura_input::keyboard::keycode_from_name("v").unwrap(),
                            &["cmd"],
                        )
                    },
                    "clipboard_paste",
                )
                .await;
                // Restore previous clipboard after brief delay for paste to land
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                if let Some(ref prev) = prev_clipboard {
                    let _ = aura_screen::macos::set_clipboard(prev);
                } else {
                    // Clipboard was empty before — clear the password from clipboard
                    let _ = aura_screen::macos::set_clipboard("");
                }
                if paste_result
                    .get("success")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false)
                {
                    return serde_json::json!({
                        "success": true,
                        "method": "clipboard_paste",
                        "reason": "secure_text_field",
                        "note": "Used clipboard paste because the focused field blocks synthetic keyboard input (password field).",
                    });
                }
                return paste_result;
            }

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
            let modifiers = crate::tool_helpers::parse_modifiers(args);
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
            // Pre-move cursor to drag origin so apps register hover state
            let pre_x = fx;
            let pre_y = fy;
            run_input_blocking(
                move || aura_input::mouse::move_mouse(pre_x, pre_y),
                "pre_drag_move",
            )
            .await;
            tokio::time::sleep(std::time::Duration::from_millis(40)).await;
            let modifiers = crate::tool_helpers::parse_modifiers(args);
            let mods = modifiers.clone();
            run_with_pid_fallback(
                move |pid| {
                    let mod_refs: Vec<&str> = mods.iter().map(|s| s.as_str()).collect();
                    aura_input::mouse::drag_pid(fx, fy, tx, ty, &mod_refs, pid)
                },
                "pid_drag",
                move || {
                    let mod_refs: Vec<&str> = modifiers.iter().map(|s| s.as_str()).collect();
                    aura_input::mouse::drag(fx, fy, tx, ty, &mod_refs)
                },
                "hid_drag",
            )
            .await
        }
        "activate_app" => {
            const BLOCKED_APPS: &[&str] = &[
                "terminal",
                "iterm",
                "iterm2",
                "kitty",
                "alacritty",
                "warp",
                "hyper",
                "tabby",
                "rio",
                "wezterm",
                "ghostty",
            ];

            let name = args.get("name").and_then(|v| v.as_str()).unwrap_or("");
            if name.is_empty() {
                return serde_json::json!({
                    "success": false,
                    "error": "name parameter is required"
                });
            }

            let name_lower = name.to_lowercase();
            if BLOCKED_APPS
                .iter()
                .any(|b| name_lower == *b || name_lower.contains(b))
            {
                return serde_json::json!({
                    "error": "blocked_app",
                    "message": "Cannot activate terminal apps for safety — Aura could accidentally execute commands. Ask the user to switch to it manually."
                });
            }

            // Sanitize app name to prevent AppleScript injection
            let safe_name = name.replace(['\\', '"'], "");
            let script = format!(r#"tell application "{safe_name}" to activate"#);

            // Pre-check automation permission if we know the bundle ID
            if let Some(bundle_id) = aura_bridge::automation::app_name_to_bundle_id(&safe_name) {
                let bundle = bundle_id.to_string();
                let perm = tokio::task::spawn_blocking(move || {
                    aura_bridge::automation::check_automation_permission(&bundle)
                })
                .await
                .unwrap_or(aura_bridge::automation::AutomationPermission::Unknown(-1));
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
                let bundle = bundle_id.to_string();
                let perm = tokio::task::spawn_blocking(move || {
                    aura_bridge::automation::check_automation_permission(&bundle)
                })
                .await
                .unwrap_or(aura_bridge::automation::AutomationPermission::Unknown(-1));
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
        "write_clipboard" => {
            let text = args.get("text").and_then(|v| v.as_str()).unwrap_or("");
            match aura_screen::macos::set_clipboard(text) {
                Ok(()) => {
                    serde_json::json!({ "success": true, "chars_written": text.chars().count() })
                }
                Err(e) => {
                    serde_json::json!({ "success": false, "error": format!("Clipboard write failed: {e}") })
                }
            }
        }
        "key_state" => {
            let key_name = args.get("key").and_then(|v| v.as_str()).unwrap_or("");
            let action = args
                .get("action")
                .and_then(|v| v.as_str())
                .unwrap_or("down");
            let keycode = match aura_input::keyboard::keycode_from_name(key_name) {
                Some(k) => k,
                None => {
                    return serde_json::json!({ "success": false, "error": format!("Unknown key: {key_name}") });
                }
            };
            let modifiers = crate::tool_helpers::parse_modifiers(args);
            match action {
                "down" => {
                    let mods = modifiers.clone();
                    run_input_blocking(
                        move || {
                            let mod_refs: Vec<&str> = mods.iter().map(|s| s.as_str()).collect();
                            aura_input::keyboard::key_down(keycode, &mod_refs)
                        },
                        "key_down",
                    )
                    .await
                }
                "up" => {
                    run_input_blocking(move || aura_input::keyboard::key_up(keycode), "key_up")
                        .await
                }
                other => {
                    serde_json::json!({ "success": false, "error": format!("Unknown action: {other}. Use 'down' or 'up'.") })
                }
            }
        }
        "context_menu_click" => {
            if !aura_input::accessibility::check_accessibility(false) {
                return serde_json::json!({
                    "success": false,
                    "error": "Accessibility permission is not granted. \
                              Required for context_menu_click to read menu items. \
                              Enable in System Settings > Privacy & Security > Accessibility.",
                    "error_kind": "accessibility_denied",
                });
            }

            let raw_x = args.get("x").and_then(|v| v.as_f64()).unwrap_or(0.0);
            let raw_y = args.get("y").and_then(|v| v.as_f64()).unwrap_or(0.0);
            let item_label = args
                .get("item_label")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let lx = dims.to_logical_x(raw_x);
            let ly = dims.to_logical_y(raw_y);

            // Pre-move to target for hover registration
            let _ = aura_input::mouse::move_mouse(lx, ly);
            tokio::time::sleep(std::time::Duration::from_millis(40)).await;

            // Right-click at position
            if let Err(e) = aura_input::mouse::click(lx, ly, "right", 1, &[]) {
                return serde_json::json!({ "success": false, "error": format!("Right-click failed: {e}") });
            }

            // Initial delay to let the context menu render AX items
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;

            // Poll for menu items to appear (up to 500ms)
            let mut found_item = None;
            let mut last_seen_items: Vec<String> = Vec::new();
            for _ in 0..10 {
                let items = tokio::task::spawn_blocking(aura_screen::accessibility::get_menu_items)
                    .await
                    .unwrap_or_default();
                if !items.is_empty() {
                    last_seen_items = items.iter().filter_map(|el| el.label.clone()).collect();
                }
                let label_lower = item_label.to_lowercase();
                if let Some(item) = items.into_iter().find(|el| {
                    el.label
                        .as_deref()
                        .map(|l| l.to_lowercase().contains(&label_lower))
                        .unwrap_or(false)
                }) {
                    found_item = Some(item);
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            }

            match found_item {
                Some(item) => {
                    if let Some(ref bounds) = item.bounds {
                        let cx = bounds.x + bounds.width / 2.0;
                        let cy = bounds.y + bounds.height / 2.0;
                        let _ = aura_input::mouse::click(cx, cy, "left", 1, &[]);
                        serde_json::json!({
                            "success": true,
                            "method": "context_menu_coordinate_click",
                            "clicked_item": item.label,
                        })
                    } else {
                        // Dismiss the stale context menu before returning an error
                        let _ = aura_input::keyboard::press_key(
                            aura_input::keyboard::keycode_from_name("escape").unwrap_or(53),
                            &[],
                        );
                        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                        serde_json::json!({
                            "success": false,
                            "error": "Found menu item but it has no bounds",
                        })
                    }
                }
                None => {
                    // Dismiss the stale context menu before returning an error
                    let _ = aura_input::keyboard::press_key(
                        aura_input::keyboard::keycode_from_name("escape").unwrap_or(53),
                        &[],
                    );
                    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                    serde_json::json!({
                        "success": false,
                        "error": format!("Menu item '{}' not found in context menu", item_label),
                        "available_items": last_seen_items,
                    })
                }
            }
        }
        "select_text" => {
            let method = args.get("method").and_then(|v| v.as_str()).unwrap_or("all");

            match method {
                "all" => {
                    // Cmd+A
                    let keycode = aura_input::keyboard::keycode_from_name("a").unwrap();
                    run_with_pid_fallback(
                        move |pid| aura_input::keyboard::press_key_pid(keycode, &["cmd"], pid),
                        "select_all_pid",
                        move || aura_input::keyboard::press_key(keycode, &["cmd"]),
                        "select_all_hid",
                    )
                    .await
                }
                "word" => {
                    let raw_x = args.get("x").and_then(|v| v.as_f64()).unwrap_or(0.0);
                    let raw_y = args.get("y").and_then(|v| v.as_f64()).unwrap_or(0.0);
                    let lx = dims.to_logical_x(raw_x);
                    let ly = dims.to_logical_y(raw_y);
                    // Double-click to select word
                    run_with_pid_fallback(
                        move |pid| aura_input::mouse::click_pid(lx, ly, "left", 2, &[], pid),
                        "word_select_pid",
                        move || aura_input::mouse::click(lx, ly, "left", 2, &[]),
                        "word_select_hid",
                    )
                    .await
                }
                "line" => {
                    let raw_x = args.get("x").and_then(|v| v.as_f64()).unwrap_or(0.0);
                    let raw_y = args.get("y").and_then(|v| v.as_f64()).unwrap_or(0.0);
                    let lx = dims.to_logical_x(raw_x);
                    let ly = dims.to_logical_y(raw_y);
                    // Triple-click to select line
                    run_with_pid_fallback(
                        move |pid| aura_input::mouse::click_pid(lx, ly, "left", 3, &[], pid),
                        "line_select_pid",
                        move || aura_input::mouse::click(lx, ly, "left", 3, &[]),
                        "line_select_hid",
                    )
                    .await
                }
                "to_start" => {
                    // Cmd+Shift+Up to select from cursor to document start
                    let keycode = aura_input::keyboard::keycode_from_name("up").unwrap();
                    run_with_pid_fallback(
                        move |pid| {
                            aura_input::keyboard::press_key_pid(
                                keycode,
                                &["cmd", "shift"],
                                pid,
                            )
                        },
                        "select_to_start_pid",
                        move || aura_input::keyboard::press_key(keycode, &["cmd", "shift"]),
                        "select_to_start_hid",
                    )
                    .await
                }
                "to_end" => {
                    // Cmd+Shift+Down to select from cursor to document end
                    let keycode = aura_input::keyboard::keycode_from_name("down").unwrap();
                    run_with_pid_fallback(
                        move |pid| {
                            aura_input::keyboard::press_key_pid(
                                keycode,
                                &["cmd", "shift"],
                                pid,
                            )
                        },
                        "select_to_end_pid",
                        move || aura_input::keyboard::press_key(keycode, &["cmd", "shift"]),
                        "select_to_end_hid",
                    )
                    .await
                }
                other => {
                    serde_json::json!({
                        "success": false,
                        "error": format!("Unknown select_text method: {other}. Use: all, word, line, to_start, to_end"),
                    })
                }
            }
        }
        "run_javascript" => {
            let app = args.get("app").and_then(|v| v.as_str()).unwrap_or("Safari");
            let code = args.get("code").and_then(|v| v.as_str()).unwrap_or("");

            if code.is_empty() {
                return serde_json::json!({ "success": false, "error": "code parameter is required" });
            }

            // Resolve browser name for AppleScript targeting
            let (script_app, bundle_hint) = match app {
                "Chrome" => ("Google Chrome", "com.google.Chrome"),
                _ => ("Safari", "com.apple.Safari"),
            };

            // Pre-check Automation permission
            let bundle = bundle_hint.to_string();
            let perm = tokio::task::spawn_blocking(move || {
                aura_bridge::automation::check_automation_permission(&bundle)
            })
            .await
            .unwrap_or(aura_bridge::automation::AutomationPermission::Unknown(-1));
            if perm == aura_bridge::automation::AutomationPermission::Denied {
                return serde_json::json!({
                    "success": false,
                    "error": format!(
                        "Automation permission for {script_app} is denied. \
                         Grant it in System Settings > Privacy & Security > Automation."
                    ),
                    "error_kind": "automation_denied",
                });
            }

            // Escape backslashes and double quotes for AppleScript string embedding
            let escaped_code = code.replace('\\', "\\\\").replace('"', "\\\"");

            // Build the AppleScript that executes JS in the browser
            let script = if app == "Chrome" {
                format!(
                    "tell application \"Google Chrome\" to execute front window's active tab javascript \"{}\"",
                    escaped_code
                )
            } else {
                format!(
                    "tell application \"Safari\" to do JavaScript \"{}\" in document 1",
                    escaped_code
                )
            };

            let timeout = args
                .get("timeout_secs")
                .and_then(|v| v.as_u64())
                .unwrap_or(30)
                .min(MAX_APPLESCRIPT_TIMEOUT_SECS);
            let result = executor.run(&script, ScriptLanguage::AppleScript, timeout).await;

            if !result.success && is_automation_denied(&result.stderr) {
                return serde_json::json!({
                    "success": false,
                    "error": format!(
                        "Automation permission for {script_app} was denied. \
                         Grant it in System Settings > Privacy & Security > Automation."
                    ),
                    "error_kind": "automation_denied",
                    "stderr": result.stderr,
                });
            }

            serde_json::json!({
                "success": result.success,
                "result": result.stdout,
                "stderr": result.stderr,
            })
        }
        other => serde_json::json!({
            "success": false,
            "error": format!("Unknown tool: {other}"),
        }),
    }
}
