use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, Result};
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;

use aura_daemon::bus::EventBus;
use aura_daemon::context::{CloudConfig, DaemonContext, SharedFlags};
use aura_daemon::event::AuraEvent;
use aura_daemon::ipc;
use aura_daemon::protocol::{DaemonEvent, DotColorName, UICommand};
use aura_gemini::config::GeminiConfig;
use aura_gemini::session::GeminiLiveSession;
use aura_memory::SessionMemory;

use crate::cloud;
use crate::processor;

/// Destructive action guardrail injected into the system prompt.
pub(crate) const DESTRUCTIVE_ACTION_GUARDRAIL: &str = "\n\nSafety — Destructive Actions:\
\n- Before deleting files, emptying trash, quitting unsaved apps, reformatting drives, \
or any action that permanently destroys data, ALWAYS confirm with the user first.\
\n- Phrase it briefly: \"Delete ~/Documents/report.pdf — sure?\" and wait for confirmation.\
\n- Non-destructive actions (opening apps, clicking, typing, moving files) do NOT require confirmation.";

/// Whether this session resumes a previous Gemini context or starts fresh.
#[derive(Debug, Clone)]
#[allow(dead_code)] // Resume will be used when IPC-based reconnection is added
pub(crate) enum SessionMode {
    /// Resume previous session via handle — no greeting.
    Resume { handle: String },
    /// Fresh session — Aura greets and introduces itself.
    Fresh,
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn run_daemon(
    mut gemini_config: GeminiConfig,
    bus: EventBus,
    cancel: CancellationToken,
    memory: Arc<Mutex<SessionMemory>>,
    session_id: String,
    has_permission_error: Arc<AtomicBool>,
    ipc_tx: broadcast::Sender<DaemonEvent>,
    session_mode: SessionMode,
) -> Result<()> {
    // Inject session-mode-specific hint before the destructive action guardrail.
    let mode_hint = match &session_mode {
        SessionMode::Resume { .. } => {
            "\n\nThis is a resumed session. Continue naturally — no greeting, no reintroduction. Pick up where you left off."
        }
        SessionMode::Fresh => {
            "\n\nThis is a new session. Greet the user briefly and naturally introduce yourself. You're Aura, their Mac companion. Keep it casual and short."
        }
    };
    if !gemini_config.system_prompt.contains(mode_hint) {
        gemini_config.system_prompt.push_str(mode_hint);
    }

    // U9: Inject destructive action confirmation guardrail into system prompt (once)
    if !gemini_config
        .system_prompt
        .contains(DESTRUCTIVE_ACTION_GUARDRAIL)
    {
        gemini_config
            .system_prompt
            .push_str(DESTRUCTIVE_ACTION_GUARDRAIL);
    }

    // Flush any pending Firestore syncs from previous sessions that failed.
    if let (Some(project_id), Some(device_id)) = (
        &gemini_config.firestore_project_id,
        &gemini_config.device_id,
    ) && let Some(firebase_api_key) = &gemini_config.firebase_api_key
    {
        cloud::flush_pending_syncs(project_id, device_id, firebase_api_key).await;
    }

    // Start IPC server for UI clients (SwiftUI panel)
    let mut ipc_cmd_rx = ipc::start_ipc_server(ipc_tx.clone(), cancel.clone());

    // Connect to Gemini Live API
    let _ = ipc_tx.send(DaemonEvent::DotColor {
        color: DotColorName::Amber,
        pulsing: false,
    });
    let _ = ipc_tx.send(DaemonEvent::Status {
        message: "Connecting...".into(),
    });

    // Determine resumption handle from session mode.
    let resumption_handle: Option<String> = match &session_mode {
        SessionMode::Resume { handle } => {
            tracing::info!("Resuming session with persisted handle");
            Some(handle.clone())
        }
        SessionMode::Fresh => {
            tracing::info!("Starting fresh Gemini session (no resumption handle)");
            None
        }
    };

    // Save API key and Cloud Run fields before gemini_config is moved into the session
    let gemini_api_key = gemini_config.api_key.clone();
    let gemini_cloud_run_url = gemini_config.cloud_run_url.clone();
    let gemini_cloud_run_auth_token = gemini_config.device_token.clone();
    let gemini_device_id = gemini_config.device_id.clone();
    let gemini_firestore_project_id = gemini_config.firestore_project_id.clone();
    let gemini_firebase_api_key = gemini_config.firebase_api_key.clone();

    let session = GeminiLiveSession::connect(gemini_config, resumption_handle)
        .await
        .context("Failed to connect Gemini Live session")?;

    let session = Arc::new(session);

    let is_speaking = Arc::new(AtomicBool::new(false));

    // Notify UI if Screen Recording was denied at startup.
    if !check_screen_recording_permission() {
        has_permission_error.store(true, Ordering::Release);
        let _ = ipc_tx.send(DaemonEvent::DotColor {
            color: DotColorName::Red,
            pulsing: false,
        });
        let _ = ipc_tx.send(DaemonEvent::Status {
            message: "Screen Recording needed — grant in System Settings > Privacy & Security > Screen Recording".into(),
        });
    }

    // Check accessibility permission (needed for input control tools)
    if !aura_input::accessibility::check_accessibility(false) {
        tracing::warn!("Accessibility permission not granted — input tools will fail silently");
    }

    // v2: Audio capture and playback are handled by Swift AVAudioEngine via IPC.
    // Audio frames arrive as UICommand::AudioFrame and are forwarded to Gemini
    // via session.send_audio_base64(). Playback audio is sent back to Swift
    // as DaemonEvent::PlaybackAudio.

    // Spawn event processor (Gemini events -> tool handling + IPC forwarding)
    let is_interrupted = Arc::new(AtomicBool::new(false));
    let ctx = DaemonContext {
        session: Arc::clone(&session),
        bus: bus.clone(),
        cancel: cancel.clone(),
        memory,
        session_id,
        ipc_tx: ipc_tx.clone(),
        cloud: CloudConfig {
            gemini_api_key,
            cloud_run_url: gemini_cloud_run_url,
            device_token: gemini_cloud_run_auth_token,
            cloud_run_device_id: gemini_device_id,
            firestore_project_id: gemini_firestore_project_id,
            firebase_api_key: gemini_firebase_api_key,
        },
        flags: SharedFlags {
            is_speaking: Arc::clone(&is_speaking),
            is_interrupted: Arc::clone(&is_interrupted),
            has_permission_error: Arc::clone(&has_permission_error),
        },
    };
    let processor_handle = tokio::spawn(async move {
        if let Err(e) = processor::run_processor(ctx).await {
            tracing::error!("Processor error: {e}");
        }
    });

    // Run until either processor exits, Ctrl+C, or IPC shutdown.
    // We need processor_handle after the select to await graceful shutdown,
    // so track whether the processor already finished naturally.
    let ipc_session = Arc::clone(&session);
    let ipc_bus = bus.clone();
    let _ipc_cancel = cancel.clone();
    let mut processor_done = false;
    let mut processor_handle = processor_handle;

    // Listen for SIGTERM (sent by SwiftUI app's process.terminate())
    let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        .expect("failed to register SIGTERM handler");

    tokio::select! {
        _ = &mut processor_handle => {
            tracing::info!("Processor finished, ending session");
            processor_done = true;
        }
        _ = tokio::signal::ctrl_c() => {
            tracing::info!("Ctrl+C received, shutting down");
            bus.send(AuraEvent::Shutdown);
        }
        _ = sigterm.recv() => {
            tracing::info!("SIGTERM received, shutting down gracefully");
            bus.send(AuraEvent::Shutdown);
        }
        _ = async {
            while let Some(cmd) = ipc_cmd_rx.recv().await {
                match cmd {
                    UICommand::Shutdown => {
                        tracing::info!("Shutdown requested via IPC");
                        ipc_bus.send(AuraEvent::Shutdown);
                    }
                    UICommand::SendText { text } => {
                        tracing::info!(len = text.len(), "Text input via IPC");
                        if let Err(e) = ipc_session.send_text(&text) {
                            tracing::warn!("Failed to send IPC text to Gemini: {e}");
                        }
                    }
                    UICommand::Reconnect => {
                        tracing::info!("Reconnect requested via IPC");
                        ipc_session.reconnect().await;
                    }
                    UICommand::ToggleMic => {
                        tracing::info!("Toggle mic requested via IPC");
                    }
                    UICommand::AudioFrame { pcm } => {
                        if let Err(e) = ipc_session.send_audio_base64(&pcm) {
                            tracing::warn!("Failed to send audio to Gemini: {e}");
                        }
                    }
                    UICommand::SetMode { mode } => {
                        tracing::info!(mode = %mode, "Set autonomy mode via IPC");
                    }
                    UICommand::DismissIntervention { intervention_id } => {
                        tracing::info!(intervention_id = %intervention_id, "Dismiss intervention via IPC");
                    }
                }
            }
        } => {
            tracing::debug!("IPC command channel closed");
        }
    }

    // Graceful shutdown: disconnect the WebSocket FIRST so the processor
    // receives the Disconnected event and runs consolidation + ingest.
    // cancel.cancel() must come AFTER disconnect — the processor's select
    // loop has a cancel.cancelled() branch that would skip the Disconnected
    // handler if cancelled first.
    let _ = ipc_tx.send(DaemonEvent::Shutdown);
    session.disconnect();

    if !processor_done {
        tracing::info!("Waiting for processor to finish end-of-session consolidation...");
        match tokio::time::timeout(Duration::from_secs(15), processor_handle).await {
            Ok(_) => tracing::info!("Processor finished gracefully"),
            Err(_) => tracing::warn!("Processor timed out after 15s, exiting anyway"),
        }
    }

    // Now cancel everything else (screen capture, IPC, etc.)
    cancel.cancel();

    Ok(())
}

/// Check Screen Recording permission at startup and warn if not granted.
/// Uses CGPreflightScreenCaptureAccess — a silent read-only check that never
/// triggers a macOS popup dialog.
fn check_screen_recording_permission() -> bool {
    #[link(name = "CoreGraphics", kind = "framework")]
    unsafe extern "C" {
        fn CGPreflightScreenCaptureAccess() -> bool;
    }

    let has_permission = unsafe { CGPreflightScreenCaptureAccess() };
    if !has_permission {
        tracing::warn!(
            "Screen Recording permission not granted — screen capture will return blank/censored frames. \
             Grant in System Settings > Privacy & Security > Screen Recording."
        );
    }
    has_permission
}

