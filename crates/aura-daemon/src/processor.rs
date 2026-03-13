use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, Result};
use chrono::Local;
use tokio_util::sync::CancellationToken;

use aura_bridge::script::ScriptExecutor;
use aura_daemon::context::{CloudConfig, DaemonContext, SharedFlags};
use aura_daemon::event::AuraEvent;
use aura_daemon::protocol::{DaemonEvent, DotColorName, Role, ToolRunStatus};
use aura_gemini::session::GeminiEvent;
use aura_memory::MessageRole;
use aura_menubar::app::MenuBarMessage;
use aura_menubar::status_item::DotColor;
use aura_screen::capture::CaptureTrigger;
use aura_screen::macos::MacOSScreenReader;

use super::cloud::memory_op;
use super::tools;

/// Minimum allowed calibrated threshold — prevents the gate from being set
/// so low that speaker bleed-through triggers a false barge-in.
/// Set above the typical speaker-to-mic bleed range (0.005-0.03 RMS at
/// moderate volume) so that only direct speech passes through.
pub(crate) const CALIBRATION_THRESHOLD_MIN: f32 = 0.05;

/// Maximum allowed calibrated threshold — prevents the gate from being set
/// so high that real speech is suppressed.
pub(crate) const CALIBRATION_THRESHOLD_MAX: f32 = 0.15;

const OUTPUT_SAMPLE_RATE: u32 = 24_000;

pub async fn run_processor(ctx: DaemonContext) -> Result<()> {
    let DaemonContext {
        session,
        bus,
        cancel,
        memory,
        session_id,
        menubar_tx,
        ipc_tx,
        player,
        cloud,
        flags,
    } = ctx;
    let SharedFlags {
        is_speaking,
        is_interrupted,
        has_permission_error,
    } = flags;
    let CloudConfig {
        gemini_api_key,
        cloud_run_url,
        cloud_run_auth_token,
        cloud_run_device_id,
        firestore_project_id,
        firebase_api_key,
    } = cloud;
    let mut events = session.subscribe();

    // Script executor for tool calls
    let executor = ScriptExecutor::new();

    // Screen reader for context gathering
    let screen_reader = MacOSScreenReader::new().context("Failed to initialize screen reader")?;

    // Screen capture loop: 1 FPS JPEG screenshots with change detection
    let capture_trigger = CaptureTrigger::new();
    let cap_notify = Arc::new(tokio::sync::Notify::new());
    let last_frame_hash = Arc::new(AtomicU64::new(0));
    let last_sent_hash = Arc::new(AtomicU64::new(0));
    let tool_semaphore = Arc::new(tokio::sync::Semaphore::new(8));
    let state_change_mutex = Arc::new(tokio::sync::Mutex::new(()));
    let held_keys: Arc<std::sync::Mutex<std::collections::HashSet<u16>>> =
        Arc::new(std::sync::Mutex::new(std::collections::HashSet::new()));

    // Startup cleanup: release all modifier keys in case a previous
    // daemon run was killed without releasing them (e.g. SIGKILL, panic).
    for &kc in &[
        56_u16, // kVK_Shift (left)
        60,     // kVK_RightShift
        59,     // kVK_Control (left)
        62,     // kVK_RightControl
        58,     // kVK_Option (left)
        61,     // kVK_RightOption
        55,     // kVK_Command (left)
        54,     // kVK_RightCommand
    ] {
        let _ = aura_input::keyboard::key_up(kc);
    }

    // Shared frame dimensions for coordinate mapping (image pixels -> logical points).
    // Updated by the capture loop after each successful capture.
    let frame_img_w = Arc::new(AtomicU32::new(1920));
    let frame_img_h = Arc::new(AtomicU32::new(1080));
    let frame_logical_w = Arc::new(AtomicU32::new(1920));
    let frame_logical_h = Arc::new(AtomicU32::new(1080));

    super::screen_capture::spawn_screen_capture(
        Arc::clone(&session),
        cancel.clone(),
        capture_trigger.clone(),
        cap_notify.clone(),
        Arc::clone(&last_frame_hash),
        Arc::clone(&last_sent_hash),
        Arc::clone(&frame_img_w),
        Arc::clone(&frame_img_h),
        Arc::clone(&frame_logical_w),
        Arc::clone(&frame_logical_h),
    );

    // Map from tool call ID -> CancellationToken for in-flight tool tasks.
    // Uses std::sync::Mutex since token insert/remove operations are fast and
    // never need to hold the lock across await points.
    let active_tool_tokens: Arc<Mutex<HashMap<String, CancellationToken>>> =
        Arc::new(Mutex::new(HashMap::new()));

    // A6: Atomic counter tracking how many tools are currently executing.
    // Drives the amber "busy" status in the UI.
    let tools_in_flight = Arc::new(AtomicUsize::new(0));

    tracing::info!("Gemini event processor running");

    loop {
        tokio::select! {
            _ = cancel.cancelled() => break,
            event = events.recv() => {
                match event {
                    Ok(GeminiEvent::Connected { is_first }) => {
                        tracing::info!(is_first, "Gemini session connected");
                        is_interrupted.store(false, Ordering::Release);
                        bus.send(AuraEvent::GeminiConnected);

                        // Enable pulsing dot + status (U2: don't clobber permission error)
                        if let Some(ref tx) = menubar_tx
                            && !has_permission_error.load(Ordering::Acquire)
                        {
                            let _ = tx.send(MenuBarMessage::SetColor(DotColor::Green)).await;
                            let _ = tx.send(MenuBarMessage::SetPulsing(true)).await;
                            let _ = tx.send(MenuBarMessage::SetStatus {
                                text: "Connected — Listening".into(),
                            }).await;
                        }
                        let _ = ipc_tx.send(DaemonEvent::DotColor {
                            color: DotColorName::Green,
                            pulsing: true,
                        });
                        let _ = ipc_tx.send(DaemonEvent::Status {
                            message: "Connected — Listening".into(),
                        });

                        if is_first {
                            // Inject recent session history for cross-session memory
                            let recent_summary: Option<String> =
                                memory_op(&memory, |mem| mem.get_recent_summary(3))
                                    .await
                                    .filter(|s| !s.is_empty());

                            // First connection: send greeting with screen context + time
                            let greeting_context = match screen_reader.capture_context() {
                                Ok(ctx) => {
                                    let summary = ctx.summary();
                                    tracing::info!(context = %summary, "Screen context for greeting");
                                    summary
                                }
                                Err(e) => {
                                    tracing::warn!("Screen context failed: {e}");
                                    "No screen context available".into()
                                }
                            };

                            let now = Local::now();
                            let time_context = format!(
                                "Current time: {} ({}). Date: {}.",
                                now.format("%I:%M %p"),
                                now.format("%Z"),
                                now.format("%A, %B %-d, %Y"),
                            );

                            let history_section = match recent_summary {
                                Some(ref summary) => format!("\n\n{summary}"),
                                None => String::new(),
                            };

                            let context_msg = format!(
                                "[System: User just activated Aura. {time_context} Current screen context:\n{greeting_context}{history_section}]"
                            );

                            let ctx_sid = session_id.clone();
                            let ctx_msg = context_msg.clone();
                            memory_op(&memory, move |mem| {
                                mem.add_message(&ctx_sid, MessageRole::User, &ctx_msg, None)
                            })
                            .await;

                            if let Err(e) = session.send_text(&context_msg) {
                                tracing::warn!("Failed to send greeting context to Gemini: {e}");
                            }
                        } else {
                            // Reconnection: send brief context restoration
                            let now = Local::now();
                            let context_msg = format!(
                                "[System: Session reconnected at {}. Continuing previous conversation. Do not re-greet the user.]",
                                now.format("%I:%M %p"),
                            );
                            tracing::info!("Reconnection — sending context restoration");

                            if let Err(e) = session.send_text(&context_msg) {
                                tracing::warn!("Failed to send reconnection context: {e}");
                            }
                        }

                        // Always start audio stream
                        if let Some(ref p) = player
                            && let Err(e) = p.start_stream(OUTPUT_SAMPLE_RATE)
                        {
                            tracing::error!("Failed to start audio stream: {e}");
                        }
                    }
                    Ok(GeminiEvent::AudioResponse { samples }) => {
                        // New audio from Gemini means the model is speaking again —
                        // clear any stale interruption flag so the audio actually plays.
                        is_interrupted.store(false, Ordering::Release);
                        is_speaking.store(true, Ordering::Release);
                        if let Some(ref p) = player
                            && let Err(e) = p.append(samples)
                        {
                            tracing::error!("Audio playback failed: {e}");
                        }
                    }
                    Ok(GeminiEvent::ToolCall { id, name, args }) => {
                        tracing::info!(name = %name, "Tool call");
                        {
                            let tc_sid = session_id.clone();
                            let tc_content = format!("{name}: {args}");
                            memory_op(&memory, move |mem| {
                                mem.add_message(&tc_sid, MessageRole::ToolCall, &tc_content, None)
                            })
                            .await;
                        }

                        // Notify the popover that a tool is starting
                        if let Some(ref tx) = menubar_tx {
                            let _ = tx
                                .send(MenuBarMessage::AddMessage {
                                    text: format!("\u{1f527} Running: {name}"),
                                    is_user: false,
                                })
                                .await;
                        }
                        let _ = ipc_tx.send(DaemonEvent::ToolStatus {
                            name: name.clone(),
                            status: ToolRunStatus::Running,
                            output: None,
                            summary: None,
                        });

                        // shutdown_aura stays inline — needs to break event loop
                        if name == "shutdown_aura" {
                            tracing::info!("Shutdown requested via voice command");
                            bus.send(AuraEvent::ToolExecuted {
                                name: name.clone(),
                                success: true,
                                output: "Shutting down Aura".into(),
                            });
                            let response = serde_json::json!({
                                "success": true,
                                "message": "Aura is shutting down. Goodbye!",
                            });
                            if let Err(e) = session.send_tool_response(id, name, response).await {
                                tracing::error!("Failed to send shutdown tool response: {e}");
                            }
                            tokio::time::sleep(Duration::from_secs(3)).await;
                            if let Some(ref tx) = menubar_tx {
                                let _ = tx.send(MenuBarMessage::Shutdown).await;
                            }
                            let _ = ipc_tx.send(DaemonEvent::Shutdown);
                            bus.send(AuraEvent::Shutdown);
                            break;
                        }

                        // recall_memory stays inline — just a fast SQLite query
                        if name == "recall_memory" {
                            let query = args["query"].as_str().unwrap_or("").to_string();
                            let response = if query.is_empty() {
                                serde_json::json!({"error": "query parameter is required", "facts": [], "sessions": []})
                            } else {
                                match memory_op(&memory, move |mem| mem.search_memory_with_sessions(&query)).await {
                                    Some(results) => results,
                                    None => serde_json::json!({"error": "Memory search failed", "facts": [], "sessions": []}),
                                }
                            };

                            let tool_success = response.get("error").is_none();
                            bus.send(AuraEvent::ToolExecuted {
                                name: name.clone(),
                                success: tool_success,
                                output: response.to_string(),
                            });
                            let _ = ipc_tx.send(DaemonEvent::ToolStatus {
                                name: name.clone(),
                                status: if tool_success { ToolRunStatus::Completed } else { ToolRunStatus::Failed },
                                output: Some(response.to_string()),
                                summary: None,
                            });

                            {
                                let tr_sid = session_id.clone();
                                let tr_content = response.to_string();
                                memory_op(&memory, move |mem| {
                                    mem.add_message(&tr_sid, aura_memory::MessageRole::ToolResult, &tr_content, None)
                                }).await;
                            }

                            if let Err(e) = session.send_tool_response(id, name, response).await {
                                tracing::error!("Failed to send recall_memory tool response: {e}");
                            }
                            continue;  // Skip the background tool spawn
                        }

                        // save_memory stays inline — persists facts immediately
                        if name == "save_memory" {
                            let category = args.get("category").and_then(|v| v.as_str()).unwrap_or("context");
                            let content = args.get("content").and_then(|v| v.as_str()).unwrap_or("");

                            const VALID_CATEGORIES: &[&str] = &["preference", "habit", "entity", "task", "context"];

                            if content.is_empty() {
                                let response = serde_json::json!({"success": false, "error": "content is required"});
                                if let Err(e) = session.send_tool_response(id, name, response).await {
                                    tracing::error!("Failed to send save_memory response: {e}");
                                }
                                continue;
                            }

                            if !VALID_CATEGORIES.contains(&category) {
                                let response = serde_json::json!({
                                    "success": false,
                                    "error": format!("Invalid category '{}'. Must be one of: {}", category, VALID_CATEGORIES.join(", "))
                                });
                                if let Err(e) = session.send_tool_response(id, name, response).await {
                                    tracing::error!("Failed to send save_memory response: {e}");
                                }
                                continue;
                            }

                            let response = {
                                let cat = category.to_string();
                                let cont = content.to_string();
                                let sid = session_id.clone();

                                match memory_op(&memory, move |mem| {
                                    mem.add_fact(&sid, &cat, &cont, None, 0.7)
                                }).await {
                                    Some(()) => {
                                        serde_json::json!({
                                            "success": true,
                                            "saved": { "category": category, "content_length": content.len() }
                                        })
                                    }
                                    None => serde_json::json!({"success": false, "error": "Failed to save fact"}),
                                }
                            };

                            let tool_success = response.get("success").and_then(|v| v.as_bool()).unwrap_or(false);
                            bus.send(AuraEvent::ToolExecuted {
                                name: name.clone(),
                                success: tool_success,
                                output: response.to_string(),
                            });
                            let _ = ipc_tx.send(DaemonEvent::ToolStatus {
                                name: name.clone(),
                                status: if tool_success { ToolRunStatus::Completed } else { ToolRunStatus::Failed },
                                output: Some(response.to_string()),
                                summary: None,
                            });

                            {
                                let tr_sid = session_id.clone();
                                let tr_content = response.to_string();
                                memory_op(&memory, move |mem| {
                                    mem.add_message(&tr_sid, aura_memory::MessageRole::ToolResult, &tr_content, None)
                                }).await;
                            }

                            if let Err(e) = session.send_tool_response(id, name, response).await {
                                tracing::error!("Failed to send save_memory tool response: {e}");
                            }
                            continue;  // Skip the background tool spawn
                        }

                        // All other tools: spawn in background so audio keeps flowing
                        let tool_session = Arc::clone(&session);
                        let tool_bus = bus.clone();
                        let tool_memory = Arc::clone(&memory);
                        let tool_session_id = session_id.clone();
                        let tool_menubar_tx = menubar_tx.clone();
                        let tool_executor = executor.clone();
                        let tool_screen_reader = screen_reader.clone();
                        let tool_capture_trigger = capture_trigger.clone();
                        let tool_cap_notify = cap_notify.clone();
                        let tool_semaphore = Arc::clone(&tool_semaphore);
                        let tool_state_mutex = state_change_mutex.clone();
                        let tool_tokens = Arc::clone(&active_tool_tokens);
                        let tool_inflight = Arc::clone(&tools_in_flight);
                        let tool_permission_error = Arc::clone(&has_permission_error);
                        let tool_ipc_tx = ipc_tx.clone();
                        let tool_held_keys = Arc::clone(&held_keys);
                        let tool_last_hash = Arc::clone(&last_frame_hash);
                        let tool_dims = tools::FrameDims {
                            img_w: frame_img_w.load(Ordering::Acquire),
                            img_h: frame_img_h.load(Ordering::Acquire),
                            logical_w: frame_logical_w.load(Ordering::Acquire),
                            logical_h: frame_logical_h.load(Ordering::Acquire),
                        };

                        // Create a cancellation token and register it before spawning
                        let tool_cancel = CancellationToken::new();
                        if let Ok(mut guard) = active_tool_tokens.lock() {
                            guard.insert(id.clone(), tool_cancel.clone());
                        } else {
                            tracing::error!("active_tool_tokens lock poisoned");
                        }

                        tokio::spawn(async move {
                            // State-changing tools run sequentially; others use the semaphore
                            let _state_guard;
                            let _permit;
                            if tools::is_state_changing_tool(&name) {
                                _state_guard = Some(tool_state_mutex.lock().await);
                                _permit = None;
                            } else {
                                _state_guard = None;
                                _permit = Some(match tool_semaphore.acquire().await {
                                    Ok(permit) => permit,
                                    Err(_) => {
                                        tracing::error!("Tool semaphore closed");
                                        return;
                                    }
                                });
                            }

                            // Increment tools-in-flight counter AFTER acquiring semaphore.
                            // This prevents counter leak when semaphore is closed.
                            let prev = tool_inflight.fetch_add(1, Ordering::AcqRel);
                            if prev == 0 {
                                // Transition from idle to busy — set amber
                                if let Some(ref tx) = tool_menubar_tx {
                                    let _ = tx.send(MenuBarMessage::SetPulsing(false)).await;
                                    let _ = tx.send(MenuBarMessage::SetColor(DotColor::Amber)).await;
                                    let _ = tx.send(MenuBarMessage::SetStatus {
                                        text: format!("Running {name}..."),
                                    }).await;
                                }
                            }

                            let pre_hash = if tools::is_state_changing_tool(&name) {
                                // Allow tools to opt out of verification (e.g. run_applescript with verify: false)
                                let verify = args
                                    .get("verify")
                                    .and_then(|v| v.as_bool())
                                    .unwrap_or(true);
                                if verify {
                                    Some(tool_last_hash.load(Ordering::Acquire))
                                } else {
                                    None
                                }
                            } else {
                                None
                            };

                            let mut response = tokio::select! {
                                result = tools::execute_tool(&name, &args, &tool_executor, &tool_screen_reader, tool_dims) => result,
                                _ = tool_cancel.cancelled() => {
                                    tracing::info!(tool = %name, "Tool execution cancelled");
                                    serde_json::json!({
                                        "success": false,
                                        "error": "Tool execution was cancelled",
                                    })
                                }
                            };

                            // Remove our token now that the task is done
                            if let Ok(mut guard) = tool_tokens.lock() {
                                guard.remove(&id);
                            } else {
                                tracing::error!("active_tool_tokens lock poisoned on remove");
                            }

                            // Track held keys for key_state tool to enable auto-release on disconnect
                            if name == "key_state"
                                && response.get("success").and_then(|v| v.as_bool()).unwrap_or(false)
                            {
                                let action = args.get("action").and_then(|v| v.as_str()).unwrap_or("");
                                let key_name = args.get("key").and_then(|v| v.as_str()).unwrap_or("");
                                if let Some(kc) = aura_input::keyboard::keycode_from_name(key_name)
                                    && let Ok(mut keys) = tool_held_keys.lock()
                                {
                                    match action {
                                        "down" => { keys.insert(kc); }
                                        "up" => { keys.remove(&kc); }
                                        _ => {}
                                    }
                                }
                            }

                            // For state-changing tools: verify screen actually changed before reporting success
                            let verified;
                            let mut verification_reason: Option<&str> = None;

                            if let Some(pre) = pre_hash {
                                // Brief delay to let UI settle after the input action
                                tokio::time::sleep(tools::settle_delay_for_tool(&name)).await;

                                // Poll for screen hash change: 50ms intervals, 1s timeout, 20 checks max
                                let mut screen_changed = false;
                                for _ in 0..20 {
                                    let rx = tool_capture_trigger.trigger_and_wait();
                                    tool_cap_notify.notify_one();
                                    let _ = tokio::time::timeout(Duration::from_millis(50), rx).await;

                                    let current_hash = tool_last_hash.load(Ordering::Acquire);
                                    if current_hash != pre {
                                        screen_changed = true;
                                        break;
                                    }
                                }

                                verified = screen_changed;
                                if !screen_changed {
                                    verification_reason = Some("screen_unchanged_after_1s");
                                    tracing::warn!(tool = %name, "Screen unchanged after action — verification failed");
                                }

                                // Capture post-action state on a blocking thread (AX FFI)
                                let post_state = tokio::time::timeout(
                                    Duration::from_millis(600),
                                    tokio::task::spawn_blocking(tools::capture_post_state),
                                )
                                .await
                                .unwrap_or(Ok(serde_json::json!({})))
                                .unwrap_or_else(|_| serde_json::json!({}));

                                // Check for post_state mismatch warning
                                let warning: Option<&str> = if !verified {
                                    let has_focus = post_state
                                        .get("focused_element")
                                        .map(|e| !e.is_null())
                                        .unwrap_or(false);
                                    if has_focus {
                                        Some("screen_unchanged_but_element_focused — check post_state")
                                    } else {
                                        Some("screen_unchanged_and_no_focused_element")
                                    }
                                } else {
                                    None
                                };

                                if let Some(obj) = response.as_object_mut() {
                                    obj.insert("verified".to_string(), serde_json::Value::Bool(verified));
                                    if let Some(reason) = verification_reason {
                                        obj.insert("verification_reason".to_string(), reason.into());
                                    }
                                    if let Some(warn) = warning {
                                        obj.insert("warning".to_string(), warn.into());
                                    }
                                    let mut ps = post_state;
                                    if let Some(ps_obj) = ps.as_object_mut() {
                                        // screenshot_delivered == verified: if the hash changed,
                                        // a fresh frame was captured during the poll loop and
                                        // will be delivered with the next Gemini message.
                                        ps_obj.insert(
                                            "screenshot_delivered".to_string(),
                                            serde_json::Value::Bool(verified),
                                        );
                                        // Enrich right-click post_state with context menu items
                                        let is_right_click = name == "click"
                                            && args.get("button").and_then(|v| v.as_str())
                                                == Some("right");
                                        if is_right_click && verified {
                                            // Brief delay for context menu to appear in AX tree
                                            tokio::time::sleep(Duration::from_millis(100)).await;
                                            let menu_items = tokio::task::spawn_blocking(
                                                aura_screen::accessibility::get_menu_items,
                                            )
                                            .await
                                            .unwrap_or_default();
                                            if !menu_items.is_empty() {
                                                let items_json: Vec<serde_json::Value> =
                                                    menu_items
                                                        .iter()
                                                        .map(|el| {
                                                            serde_json::json!({
                                                                "label": el.label,
                                                                "enabled": el.enabled,
                                                            })
                                                        })
                                                        .collect();
                                                ps_obj.insert(
                                                    "menu_items".to_string(),
                                                    serde_json::json!(items_json),
                                                );
                                            }
                                        }
                                    }
                                    obj.insert("post_state".to_string(), ps);
                                }
                            } else {
                                verified = true; // non-state-changing tools are inherently "verified"
                            }

                            let tool_success = response
                                .get("success")
                                .and_then(|v| v.as_bool())
                                .unwrap_or(false);

                            // Notify the popover that the tool completed
                            if let Some(ref tx) = tool_menubar_tx {
                                let status_msg = if tool_success && verified {
                                    format!("\u{2705} Done: {name}")
                                } else if tool_success && !verified {
                                    format!("\u{26a0}\u{fe0f} Unverified: {name}")
                                } else {
                                    format!("\u{274c} Failed: {name}")
                                };
                                let _ = tx
                                    .send(MenuBarMessage::AddMessage {
                                        text: status_msg,
                                        is_user: false,
                                    })
                                    .await;
                            }
                            let tool_output = response.get("stdout")
                                .or(response.get("context"))
                                .or(response.get("error"))
                                .and_then(|v| v.as_str())
                                .map(|s| s.to_string());
                            let _ = tool_ipc_tx.send(DaemonEvent::ToolStatus {
                                name: name.clone(),
                                status: if tool_success { ToolRunStatus::Completed } else { ToolRunStatus::Failed },
                                output: tool_output,
                                summary: None,
                            });

                            tool_bus.send(AuraEvent::ToolExecuted {
                                name: name.clone(),
                                success: tool_success,
                                output: response.get("stdout")
                                    .or(response.get("context"))
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("")
                                    .to_string(),
                            });

                            // Log result
                            {
                                let tr_sid = tool_session_id.clone();
                                let tr_content = response.to_string();
                                memory_op(&tool_memory, move |mem| {
                                    mem.add_message(&tr_sid, MessageRole::ToolResult, &tr_content, None)
                                })
                                .await;
                            }

                            // Decrement tools-in-flight counter (saturating to avoid underflow)
                            let prev = tool_inflight.fetch_update(Ordering::AcqRel, Ordering::Acquire, |n| {
                                Some(n.saturating_sub(1))
                            }).unwrap_or(0);
                            let remaining = prev.saturating_sub(1);
                            if remaining == 0
                                && let Some(ref tx) = tool_menubar_tx
                                && !tool_permission_error.load(Ordering::Acquire)
                            {
                                // All tools done — restore green status
                                let _ = tx.send(MenuBarMessage::SetColor(DotColor::Green)).await;
                                let _ = tx.send(MenuBarMessage::SetPulsing(true)).await;
                                let _ = tx.send(MenuBarMessage::SetStatus {
                                    text: "Connected — Listening".into(),
                                }).await;
                            }

                            // For non-state-changing tools, fire-and-forget screen capture
                            // (state-changing tools already triggered + awaited above)
                            if pre_hash.is_none() {
                                tool_capture_trigger.trigger();
                                tool_cap_notify.notify_one();
                            }

                            // Send tool response back to Gemini
                            tools::truncate_tool_response(&mut response);
                            if let Err(e) = tool_session.send_tool_response(id, name, response).await {
                                tracing::error!("Failed to send tool response: {e}");
                            }

                            // Show thinking indicator while Gemini processes the tool result
                            if let Some(ref tx) = tool_menubar_tx {
                                let _ = tx
                                    .send(MenuBarMessage::AddMessage {
                                        text: "\u{1f4ad} Thinking...".into(),
                                        is_user: false,
                                    })
                                    .await;
                            }
                        });
                    }
                    Ok(GeminiEvent::ToolCallCancellation { ids }) => {
                        tracing::info!(?ids, "Tool call(s) cancelled");
                        if let Ok(mut guard) = active_tool_tokens.lock() {
                            for id in &ids {
                                if let Some(token) = guard.remove(id) {
                                    token.cancel();
                                }
                            }
                        } else {
                            tracing::error!("active_tool_tokens lock poisoned on cancellation");
                        }
                    }
                    Ok(GeminiEvent::Interrupted) => {
                        tracing::info!("Gemini interrupted — stopping playback");
                        is_speaking.store(false, Ordering::Release);
                        is_interrupted.store(true, Ordering::Release);
                        if let Some(ref p) = player {
                            p.stop();
                        }
                        bus.send(AuraEvent::BargeIn);
                        // Notify UI that we're back to listening
                        let _ = ipc_tx.send(DaemonEvent::Status {
                            message: "Listening...".into(),
                        });
                    }
                    Ok(GeminiEvent::Transcription { text }) => {
                        // Native audio models generate text and audio independently.
                        // The text is the model's internal reasoning — NOT a transcript
                        // of the spoken audio. It's always longer/different from what's
                        // actually said. Log it for debugging but still forward to IPC
                        // clients for display in the floating panel.
                        tracing::debug!(transcription = text.lines().next().unwrap_or(""), "Gemini text");

                        // Filter out markdown artifacts (lines that are just bold markers)
                        let filtered: String = text
                            .lines()
                            .filter(|line| {
                                let trimmed = line.trim();
                                !trimmed.is_empty() && trimmed != "**"
                            })
                            .collect::<Vec<_>>()
                            .join("\n");

                        if !filtered.is_empty() {
                            let _ = ipc_tx.send(DaemonEvent::Transcript {
                                role: Role::Assistant,
                                text: filtered,
                                done: false,
                                source: "voice".into(),
                            });
                        }
                    }
                    Ok(GeminiEvent::TurnComplete) => {
                        is_speaking.store(false, Ordering::Release);
                        is_interrupted.store(false, Ordering::Release);
                        tracing::debug!("Turn complete");

                        // Notify UI that assistant turn is done
                        let _ = ipc_tx.send(DaemonEvent::Transcript {
                            role: Role::Assistant,
                            text: String::new(),
                            done: true,
                            source: "voice".into(),
                        });
                    }
                    Ok(GeminiEvent::Error { message }) => {
                        tracing::error!(%message, "Gemini error");

                        if let Some(ref tx) = menubar_tx {
                            let _ = tx.send(MenuBarMessage::SetPulsing(false)).await;
                            let _ = tx.send(MenuBarMessage::SetColor(DotColor::Red)).await;
                            let _ = tx.send(MenuBarMessage::SetStatus {
                                text: format!("Error: {message}"),
                            }).await;
                        }
                        let _ = ipc_tx.send(DaemonEvent::DotColor {
                            color: DotColorName::Red,
                            pulsing: false,
                        });
                        let _ = ipc_tx.send(DaemonEvent::Status {
                            message: format!("Error: {message}"),
                        });
                    }
                    Ok(GeminiEvent::Reconnecting { attempt }) => {
                        tracing::warn!(attempt, "Gemini reconnecting");
                        bus.send(AuraEvent::GeminiReconnecting { attempt });

                        if let Some(ref tx) = menubar_tx {
                            // U2: Don't clobber permission error status
                            if !has_permission_error.load(Ordering::Acquire) {
                                let _ = tx.send(MenuBarMessage::SetPulsing(false)).await;
                                let _ = tx.send(MenuBarMessage::SetColor(DotColor::Amber)).await;
                                let _ = tx.send(MenuBarMessage::SetStatus {
                                    text: format!("Reconnecting (attempt {attempt})..."),
                                }).await;
                            }
                        }
                        let _ = ipc_tx.send(DaemonEvent::DotColor {
                            color: DotColorName::Amber,
                            pulsing: true,
                        });
                        let _ = ipc_tx.send(DaemonEvent::Status {
                            message: format!("Reconnecting (attempt {attempt})..."),
                        });
                    }
                    Ok(GeminiEvent::Disconnected) => {
                        tracing::info!("Gemini session disconnected");

                        // Release all held keys to prevent system-wide stuck modifiers
                        let keys_to_release: Vec<u16> = held_keys
                            .lock()
                            .map(|mut g| g.drain().collect())
                            .unwrap_or_default();
                        for kc in keys_to_release {
                            if let Err(e) = aura_input::keyboard::key_up(kc) {
                                tracing::warn!(keycode = kc, "Failed to release held key on disconnect: {e}");
                            }
                        }

                        if let Some(ref tx) = menubar_tx {
                            let _ = tx.send(MenuBarMessage::SetPulsing(false)).await;
                            let _ = tx.send(MenuBarMessage::SetColor(DotColor::Gray)).await;
                            let _ = tx.send(MenuBarMessage::SetStatus {
                                text: "Disconnected".into(),
                            }).await;
                        }
                        let _ = ipc_tx.send(DaemonEvent::DotColor {
                            color: DotColorName::Gray,
                            pulsing: false,
                        });
                        let _ = ipc_tx.send(DaemonEvent::Status {
                            message: "Disconnected".into(),
                        });

                        // Wait for in-flight tool tasks to finish before consolidation
                        // so their results are included in fact extraction.
                        {
                            let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
                            let mut warned = false;
                            loop {
                                let remaining = tools_in_flight.load(Ordering::Acquire);
                                if remaining == 0 {
                                    break;
                                }
                                if tokio::time::Instant::now() >= deadline {
                                    tracing::warn!(
                                        remaining,
                                        "Timed out waiting for in-flight tools, proceeding with consolidation"
                                    );
                                    break;
                                }
                                if !warned {
                                    tracing::info!(remaining, "Waiting for in-flight tools to complete before consolidation");
                                    warned = true;
                                }
                                tokio::time::sleep(Duration::from_millis(50)).await;
                            }
                        }

                        // Run end-of-session consolidation (extracts facts + sets summary)
                        {
                            let es_sid = session_id.clone();
                            let es_key = gemini_api_key.clone();

                            // Fetch messages with lock, then drop lock for async work
                            let messages = memory_op(&memory, {
                                let sid = es_sid.clone();
                                move |mem| mem.get_messages(&sid)
                            }).await;

                            if let Some(messages) = messages {
                                match aura_memory::consolidate::consolidate_session(
                                    &es_key,
                                    &messages,
                                    cloud_run_url.as_deref(),
                                    cloud_run_auth_token.as_deref(),
                                    cloud_run_device_id.as_deref(),
                                    Some(&es_sid),
                                ).await {
                                    Ok(response) => {
                                        if !response.summary.is_empty() || !response.facts.is_empty() {
                                            let summary = response.summary.clone();
                                            let facts_json: Vec<(String, String, Option<String>, f64)> = response
                                                .facts
                                                .iter()
                                                .map(|f| {
                                                    let entities = if f.entities.is_empty() {
                                                        None
                                                    } else {
                                                        serde_json::to_string(&f.entities).ok()
                                                    };
                                                    (f.category.clone(), f.content.clone(), entities, f.importance)
                                                })
                                                .collect();

                                            // Clone es_sid before it's moved into memory_op closure
                                            let fs_sid = es_sid.clone();

                                            memory_op(&memory, move |mem| {
                                                if !summary.is_empty() {
                                                    mem.end_session(&es_sid, Some(&summary))?;
                                                } else {
                                                    mem.end_session(&es_sid, None)?;
                                                }
                                                for (cat, content, entities, importance) in &facts_json {
                                                    mem.add_fact(&es_sid, cat, content, entities.as_deref(), *importance)?;
                                                }
                                                Ok(())
                                            }).await;
                                            tracing::info!("Session consolidation complete");

                                            // Sync facts to Firestore if config is available
                                            if let (Some(project_id), Some(device_id), Some(fb_key)) =
                                                (&firestore_project_id, &cloud_run_device_id, &firebase_api_key)
                                                && let Err(e) = super::cloud::sync_session_to_firestore(
                                                    &response.facts,
                                                    &response.summary,
                                                    &fs_sid,
                                                    project_id,
                                                    device_id,
                                                    fb_key,
                                                ).await
                                            {
                                                tracing::warn!("Firestore sync failed, queuing for retry: {e}");
                                                super::cloud::queue_pending_sync(
                                                    &response.facts,
                                                    &response.summary,
                                                    &fs_sid,
                                                );
                                            }
                                        } else {
                                            // No facts extracted — just end session normally
                                            let sid = es_sid.clone();
                                            memory_op(&memory, move |mem| mem.end_session(&sid, None)).await;
                                        }
                                    }
                                    Err(e) => {
                                        tracing::warn!("Session consolidation failed: {e}");
                                        // Fallback: end session without summary
                                        let sid = es_sid.clone();
                                        memory_op(&memory, move |mem| mem.end_session(&sid, None)).await;
                                    }
                                }
                            } else {
                                // Couldn't fetch messages — end session normally
                                let sid = es_sid.clone();
                                memory_op(&memory, move |mem| mem.end_session(&sid, None)).await;
                            }
                        }
                        break;
                    }
                    Ok(GeminiEvent::SessionHandle { handle }) => {
                        if handle.is_empty() {
                            tracing::info!("Clearing stale resumption handle from storage");
                            memory_op(&memory, move |mem| {
                                mem.delete_setting("resumption_handle")
                            }).await;
                        } else {
                            let prefix_len = handle.len().min(12);
                            tracing::debug!(handle_prefix = &handle[..prefix_len], "Received session resumption handle, persisting");
                            memory_op(&memory, move |mem| {
                                mem.set_setting("resumption_handle", &handle)
                            }).await;
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        tracing::warn!(skipped = n, "Event bus receiver lagged — events were dropped");
                        continue;
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                }
            }
        }
    }

    Ok(())
}

/// Compute root-mean-square energy of an audio buffer.
pub(crate) fn rms_energy(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    let sum_sq: f32 = samples.iter().map(|s| s * s).sum();
    (sum_sq / samples.len() as f32).sqrt()
}

/// Calibrate the barge-in energy threshold from ambient noise RMS samples.
///
/// Computes mean + 3*stddev of the collected RMS values, clamped to
/// `[CALIBRATION_THRESHOLD_MIN, CALIBRATION_THRESHOLD_MAX]`.
pub(crate) fn calibrate_barge_in_threshold(rms_samples: &[f32]) -> f32 {
    let n = rms_samples.len() as f32;
    let mean = rms_samples.iter().sum::<f32>() / n;
    let variance = rms_samples.iter().map(|s| (s - mean).powi(2)).sum::<f32>() / n;
    let stddev = variance.sqrt();
    let threshold = mean + 3.0 * stddev;
    threshold.clamp(CALIBRATION_THRESHOLD_MIN, CALIBRATION_THRESHOLD_MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rms_energy_empty() {
        assert_eq!(rms_energy(&[]), 0.0);
    }

    #[test]
    fn test_rms_energy_known_signal() {
        // Constant signal of 0.5 should give RMS of 0.5
        let samples = vec![0.5_f32; 100];
        let rms = rms_energy(&samples);
        assert!((rms - 0.5).abs() < 1e-5);
    }

    #[test]
    fn test_calibrate_uniform_quiet_noise() {
        // Uniform low noise — stddev is ~0, threshold ≈ mean, clamped to min
        let samples = vec![0.005_f32; 100];
        let threshold = calibrate_barge_in_threshold(&samples);
        assert_eq!(threshold, CALIBRATION_THRESHOLD_MIN);
    }

    #[test]
    fn test_calibrate_moderate_ambient_noise() {
        // Mean=0.03, stddev=0.01 → threshold = 0.03 + 0.03 = 0.06
        let mut samples = Vec::with_capacity(100);
        for _ in 0..50 {
            samples.push(0.02);
        }
        for _ in 0..50 {
            samples.push(0.04);
        }
        let threshold = calibrate_barge_in_threshold(&samples);
        // mean = 0.03, stddev = 0.01, expected = 0.06
        assert!(threshold > CALIBRATION_THRESHOLD_MIN);
        assert!(threshold < CALIBRATION_THRESHOLD_MAX);
        assert!((threshold - 0.06).abs() < 1e-4);
    }

    #[test]
    fn test_calibrate_clamps_to_max() {
        // Very high noise should clamp to max
        let samples = vec![0.2_f32; 100];
        let threshold = calibrate_barge_in_threshold(&samples);
        assert_eq!(threshold, CALIBRATION_THRESHOLD_MAX);
    }

    #[test]
    fn test_calibrate_clamps_to_min() {
        // Near-silence should clamp to min
        let samples = vec![0.001_f32; 100];
        let threshold = calibrate_barge_in_threshold(&samples);
        assert_eq!(threshold, CALIBRATION_THRESHOLD_MIN);
    }
}
