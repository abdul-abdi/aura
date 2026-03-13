use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, Result};
use tokio::sync::{broadcast, mpsc};
use tokio_util::sync::CancellationToken;

use aura_daemon::bus::EventBus;
use aura_daemon::context::{CloudConfig, DaemonContext, SharedFlags};
use aura_daemon::event::AuraEvent;
use aura_daemon::ipc;
use aura_daemon::protocol::{DaemonEvent, DotColorName, UICommand};
use aura_gemini::config::GeminiConfig;
use aura_gemini::session::GeminiLiveSession;
use aura_memory::SessionMemory;
use aura_menubar::app::MenuBarMessage;
use aura_menubar::status_item::DotColor;
use aura_voice::audio::AudioCapture;
use aura_voice::playback::AudioPlayer;

use crate::cloud;
use crate::processor;

/// Default RMS energy threshold for mic gating while Aura is speaking.
/// Used as the initial value before adaptive calibration completes.
/// Direct speech into the mic is typically 0.05-0.3 RMS; speaker bleed-through
/// from laptop speakers is usually 0.005-0.03 (up to 0.05 at high volume).
const BARGE_IN_ENERGY_THRESHOLD_DEFAULT: f32 = 0.05;

/// Number of initial audio chunks to collect for ambient noise calibration.
/// At ~5ms per chunk this is roughly 500ms of audio.
const CALIBRATION_CHUNK_COUNT: usize = 100;

/// Bounded mic bridge channel capacity — prevents unbounded memory growth
/// during backpressure from the Gemini WebSocket.
const MIC_BRIDGE_CAPACITY: usize = 256;

/// Multiplier applied to the barge-in threshold while audio is actively
/// playing.  Speaker bleed-through on MacBook hardware can reach 0.03-0.05
/// RMS at high volume, so we inflate the threshold to avoid false triggers.
const PLAYBACK_THRESHOLD_MULTIPLIER: f32 = 1.8;

/// Number of consecutive audio chunks that must exceed the barge-in energy
/// threshold before we forward audio during playback.  Prevents transient
/// loud passages in the AI's speech from triggering a false barge-in.
/// At ~5 ms per chunk, 3 chunks ≈ 15 ms of sustained speech.
const BARGE_IN_CONSECUTIVE_FRAMES: u32 = 3;

/// Destructive action guardrail injected into the system prompt.
pub(crate) const DESTRUCTIVE_ACTION_GUARDRAIL: &str = "\n\nSafety — Destructive Actions:\
\n- Before deleting files, emptying trash, quitting unsaved apps, reformatting drives, \
or any action that permanently destroys data, ALWAYS confirm with the user first.\
\n- Phrase it briefly: \"Delete ~/Documents/report.pdf — sure?\" and wait for confirmation.\
\n- Non-destructive actions (opening apps, clicking, typing, moving files) do NOT require confirmation.";

/// Whether this session resumes a previous Gemini context or starts fresh.
#[derive(Debug, Clone)]
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
    menubar_tx: Option<mpsc::Sender<MenuBarMessage>>,
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

    // On fresh session start, load facts from Firestore and inject into system prompt.
    // This is optional — daemon works fine without Firestore configured.
    if matches!(session_mode, SessionMode::Fresh)
        && let (Some(project_id), Some(device_id)) = (
            &gemini_config.firestore_project_id,
            &gemini_config.device_id,
        )
    {
        if let Some(firebase_api_key) = &gemini_config.firebase_api_key {
            match cloud::load_firestore_facts(project_id, device_id, firebase_api_key).await {
                Ok(facts_context) if !facts_context.is_empty() => {
                    gemini_config
                        .system_prompt
                        .push_str("\n\nMemory from past sessions:\n");
                    gemini_config.system_prompt.push_str(&facts_context);
                    tracing::info!(
                        chars = facts_context.len(),
                        "Injected Firestore facts into system prompt"
                    );
                }
                Ok(_) => tracing::debug!("No Firestore facts found for this device"),
                Err(e) => tracing::warn!("Failed to load Firestore facts: {e}"),
            }
        } else {
            tracing::debug!("Skipping Firestore facts load: firebase_api_key not configured");
        }
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
    if let Some(ref tx) = menubar_tx {
        // U2: Don't overwrite permission error status
        if !has_permission_error.load(Ordering::Acquire) {
            let _ = tx
                .send(MenuBarMessage::SetStatus {
                    text: "Connecting...".into(),
                })
                .await;
        }
    }
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
    let gemini_cloud_run_auth_token = gemini_config.cloud_run_auth_token.clone();
    let gemini_device_id = gemini_config.device_id.clone();
    let gemini_firestore_project_id = gemini_config.firestore_project_id.clone();
    let gemini_firebase_api_key = gemini_config.firebase_api_key.clone();

    let session = GeminiLiveSession::connect(gemini_config, resumption_handle)
        .await
        .context("Failed to connect Gemini Live session")?;

    let session = Arc::new(session);

    // Set up mic capture on a dedicated std::thread (cpal's Stream is !Send)
    let (std_tx, std_rx) = std::sync::mpsc::channel::<Vec<f32>>();
    let mic_shutdown = Arc::new(AtomicBool::new(false));
    let is_speaking = Arc::new(AtomicBool::new(false));
    // Adaptive barge-in threshold stored as f32 bits in an AtomicU32.
    // Starts at the default and is updated after ambient noise calibration.
    let barge_in_threshold = Arc::new(AtomicU32::new(BARGE_IN_ENERGY_THRESHOLD_DEFAULT.to_bits()));

    // Audio playback (optional — warn if unavailable).
    // Created here so the mic bridge can share the player's playing flag and
    // fully mute itself during hardware playback (prevents feedback loop).
    let player = match AudioPlayer::new() {
        Ok(p) => {
            tracing::info!("Audio playback ready");
            Some(p)
        }
        Err(e) => {
            tracing::warn!("Audio playback unavailable: {e}");
            None
        }
    };
    // Arc<AtomicBool> that mirrors player.playing — false when no player.
    let audio_playing: Arc<AtomicBool> = player
        .as_ref()
        .map(|p| p.playing_arc())
        .unwrap_or_else(|| Arc::new(AtomicBool::new(false)));

    // Notify UI if Screen Recording was denied at startup.
    if !check_screen_recording_permission() {
        has_permission_error.store(true, Ordering::Release);
        if let Some(ref tx) = menubar_tx {
            let _ = tx.send(MenuBarMessage::SetColor(DotColor::Red)).await;
            let _ = tx
                .send(MenuBarMessage::SetStatus {
                    text: "Screen Recording needed — grant in System Settings > Privacy & Security > Screen Recording"
                        .into(),
                })
                .await;
        }
        let _ = ipc_tx.send(DaemonEvent::DotColor {
            color: DotColorName::Red,
            pulsing: false,
        });
        let _ = ipc_tx.send(DaemonEvent::Status {
            message: "Screen Recording needed — grant in System Settings > Privacy & Security > Screen Recording".into(),
        });
    }

    // Attempt audio capture directly. The SwiftUI app grants mic permission
    // during onboarding — the daemon inherits it via macOS's "responsible
    // process" mechanism. If permission wasn't granted, cpal will fail
    // gracefully and we report the error via IPC.
    let _mic_available = {
        match AudioCapture::new(None) {
            Ok(capture) => {
                let mic_shutdown_flag = Arc::clone(&mic_shutdown);
                std::thread::Builder::new()
                    .name("aura-mic-capture".into())
                    .spawn(move || {
                        let _stream = match capture.start(std_tx) {
                            Ok(stream) => stream,
                            Err(e) => {
                                tracing::error!("Failed to start audio capture: {e}");
                                return;
                            }
                        };
                        tracing::info!("Mic capture started");
                        while !mic_shutdown_flag.load(Ordering::Acquire) {
                            std::thread::park_timeout(Duration::from_millis(500));
                        }
                        tracing::info!("Mic capture stopped");
                    })?;
                true
            }
            Err(e) => {
                tracing::warn!("Mic unavailable: {e}");
                has_permission_error.store(true, Ordering::Release);
                if let Some(ref tx) = menubar_tx {
                    let _ = tx.send(MenuBarMessage::SetColor(DotColor::Red)).await;
                    let _ = tx
                        .send(MenuBarMessage::SetStatus {
                            text: "Mic access needed — check System Settings > Privacy".into(),
                        })
                        .await;
                }
                let _ = ipc_tx.send(DaemonEvent::DotColor {
                    color: DotColorName::Red,
                    pulsing: false,
                });
                let _ = ipc_tx.send(DaemonEvent::Status {
                    message: "Mic access needed — check System Settings > Privacy".into(),
                });
                false
            }
        }
    };

    // Check accessibility permission (needed for input control tools)
    if !aura_input::accessibility::check_accessibility(false) {
        tracing::warn!("Accessibility permission not granted — input tools will fail silently");
        if let Some(ref tx) = menubar_tx {
            let _ = tx
                .send(MenuBarMessage::AddMessage {
                    text: "Grant Accessibility permission in System Settings for input control."
                        .into(),
                    is_user: false,
                })
                .await;
        }
    }

    // Bridge std::sync::mpsc -> tokio -> session.send_audio()
    // During playback, mic audio is energy-gated rather than fully muted:
    // direct user speech (0.05-0.3 RMS) passes through while speaker
    // bleed-through (0.005-0.03 RMS) is filtered out, enabling barge-in.
    // A 300 ms post-playback reverb guard uses the same energy gating.
    let audio_session = Arc::clone(&session);
    let audio_cancel = cancel.clone();
    let bridge_shutdown = Arc::clone(&mic_shutdown);
    let bridge_speaking = Arc::clone(&is_speaking);
    let bridge_threshold = Arc::clone(&barge_in_threshold);
    let bridge_audio_playing = Arc::clone(&audio_playing);
    tokio::spawn(async move {
        let (tokio_tx, mut tokio_rx) = tokio::sync::mpsc::channel::<Vec<f32>>(MIC_BRIDGE_CAPACITY);

        // Spawn a blocking task to drain the std::sync::mpsc channel
        let bridge_tx = tokio_tx.clone();
        tokio::task::spawn_blocking(move || {
            loop {
                match std_rx.recv_timeout(Duration::from_millis(10)) {
                    Ok(samples) => {
                        if bridge_tx.blocking_send(samples).is_err() {
                            break;
                        }
                    }
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                        if bridge_shutdown.load(Ordering::Acquire) {
                            break;
                        }
                    }
                    Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
                }
            }
        });

        // Ambient noise calibration state
        let mut calibration_samples: Vec<f32> = Vec::with_capacity(CALIBRATION_CHUNK_COUNT);
        let mut calibration_done = false;

        // Tracks when hardware playback last stopped, for post-playback mute guard.
        let mut playback_stopped_at: Option<std::time::Instant> = None;
        // Whether audio was playing on the previous iteration (to detect stop edge).
        let mut prev_playing = false;
        // How long to keep muting after playback ends (absorbs room reverb).
        const POST_PLAYBACK_MUTE_MS: u128 = 300;
        // Consecutive frames above threshold during playback (prevents transients).
        let mut barge_in_streak: u32 = 0;

        // VAD for speech detection (replaces energy-threshold gating).
        // VoiceDetector wraps a raw C pointer and is not Send by default.
        // This task has sole ownership and never shares it across threads,
        // so the unsafe Send impl is sound.
        struct SendableVad(aura_voice::vad::VoiceDetector);
        // SAFETY: sole ownership, single-task access, no concurrent mutation.
        unsafe impl Send for SendableVad {}
        let mut vad: Option<SendableVad> = match aura_voice::vad::VoiceDetector::new() {
            Ok(v) => Some(SendableVad(v)),
            Err(e) => {
                tracing::warn!("VAD init failed, falling back to energy gating: {e}");
                None
            }
        };
        let mut vad_accumulator: Vec<f32> = Vec::with_capacity(aura_voice::vad::VAD_FRAME_SIZE);

        loop {
            tokio::select! {
                _ = audio_cancel.cancelled() => break,
                Some(samples) = tokio_rx.recv() => {
                    // Collect RMS samples for ambient noise calibration
                    if !calibration_done {
                        let rms = processor::rms_energy(&samples);
                        calibration_samples.push(rms);
                        if calibration_samples.len() >= CALIBRATION_CHUNK_COUNT {
                            let calibrated = processor::calibrate_barge_in_threshold(&calibration_samples);
                            bridge_threshold.store(calibrated.to_bits(), Ordering::Release);
                            tracing::info!(
                                threshold = calibrated,
                                samples = calibration_samples.len(),
                                "Adaptive barge-in threshold calibrated from ambient noise"
                            );
                            calibration_done = true;
                            // Free calibration buffer
                            calibration_samples = Vec::new();
                        }
                    }

                    // --- Barge-in: energy-gated mic during playback ---
                    // Forward audio to Gemini only when RMS energy exceeds the
                    // calibrated threshold, filtering out speaker bleed-through
                    // while allowing direct user speech to trigger interruption.
                    let currently_playing = bridge_audio_playing.load(Ordering::Acquire);
                    let gemini_speaking = bridge_speaking.load(Ordering::Acquire);

                    // Detect transition: playing → stopped.  Arm the reverb guard.
                    if prev_playing && !currently_playing {
                        playback_stopped_at = Some(std::time::Instant::now());
                        barge_in_streak = 0;
                        tracing::debug!("Playback stopped — entering post-playback reverb guard");
                    }
                    prev_playing = currently_playing;

                    if currently_playing {
                        if let Some(ref mut v) = vad {
                            // Accumulate into VAD-sized frames
                            vad_accumulator.extend_from_slice(&samples);
                            let mut any_speech = false;
                            while vad_accumulator.len() >= aura_voice::vad::VAD_FRAME_SIZE {
                                let frame_f32: Vec<f32> = vad_accumulator.drain(..aura_voice::vad::VAD_FRAME_SIZE).collect();
                                let frame_i16: Vec<i16> = frame_f32.iter()
                                    .map(|&s| (s * 32767.0).clamp(-32768.0, 32767.0) as i16)
                                    .collect();
                                if v.0.is_speech(&frame_i16) {
                                    any_speech = true;
                                }
                            }
                            if !any_speech {
                                continue; // VAD says no speech — skip
                            }
                            if barge_in_streak == 0 {
                                tracing::info!("Barge-in: VAD detected speech during playback");
                            }
                            barge_in_streak = barge_in_streak.saturating_add(1);
                            if barge_in_streak < BARGE_IN_CONSECUTIVE_FRAMES {
                                continue;
                            }
                        } else {
                            // Fallback: original energy-based gating
                            let energy = processor::rms_energy(&samples);
                            let base = f32::from_bits(bridge_threshold.load(Ordering::Acquire));
                            let threshold = base * PLAYBACK_THRESHOLD_MULTIPLIER;
                            if energy < threshold {
                                barge_in_streak = 0;
                                continue; // Below threshold — likely speaker echo
                            }
                            // Require sustained speech across consecutive frames
                            // to avoid transient spikes from loud AI passages.
                            barge_in_streak = barge_in_streak.saturating_add(1);
                            if barge_in_streak < BARGE_IN_CONSECUTIVE_FRAMES {
                                continue; // Not enough consecutive frames yet
                            }
                            if barge_in_streak == BARGE_IN_CONSECUTIVE_FRAMES {
                                tracing::info!(
                                    energy,
                                    threshold,
                                    "Barge-in: sustained user speech detected during playback"
                                );
                            }
                            // Don't reset streak — keep forwarding all above-threshold
                            // frames so Gemini receives continuous audio for VAD.
                        }
                    } else if gemini_speaking {
                        // Gemini is still sending audio data but the pre-buffer
                        // hasn't flushed to the hardware yet (~80 ms window).
                        // Mute to prevent premature interruption before the
                        // user hears anything.
                        continue;
                    } else if let Some(stopped_at) = playback_stopped_at {
                        // Post-playback reverb guard: energy-gate for 300 ms
                        // after the speaker stops to absorb room echo.
                        let in_reverb_window = stopped_at.elapsed().as_millis() < POST_PLAYBACK_MUTE_MS;
                        if in_reverb_window {
                            let energy = processor::rms_energy(&samples);
                            let threshold = f32::from_bits(bridge_threshold.load(Ordering::Acquire));
                            if energy < threshold {
                                continue; // Reverb guard: below threshold
                            }
                            tracing::debug!(energy, threshold, "Speech during reverb guard — forwarding");
                        }
                        playback_stopped_at = None;
                    }

                    // VAD gate for non-playback audio
                    if !currently_playing
                        && playback_stopped_at.is_none()
                        && let Some(ref mut v) = vad
                    {
                        vad_accumulator.extend_from_slice(&samples);
                        let mut any_speech = false;
                        while vad_accumulator.len() >= aura_voice::vad::VAD_FRAME_SIZE {
                            let frame_f32: Vec<f32> = vad_accumulator.drain(..aura_voice::vad::VAD_FRAME_SIZE).collect();
                            let frame_i16: Vec<i16> = frame_f32.iter()
                                .map(|&s| (s * 32767.0).clamp(-32768.0, 32767.0) as i16)
                                .collect();
                            if v.0.is_speech(&frame_i16) {
                                any_speech = true;
                            }
                        }
                        if !any_speech {
                            continue; // Not speech — don't send to Gemini
                        }
                    }

                    if let Err(e) = audio_session.send_audio(&samples) {
                        tracing::warn!("Gemini session closed, stopping audio bridge: {e}");
                        break;
                    }
                }
            }
        }
    });

    // Spawn event processor (Gemini events -> tool handling + audio playback)
    let is_interrupted = Arc::new(AtomicBool::new(false));
    let ctx = DaemonContext {
        session: Arc::clone(&session),
        bus: bus.clone(),
        cancel: cancel.clone(),
        memory,
        session_id,
        menubar_tx,
        ipc_tx: ipc_tx.clone(),
        player,
        cloud: CloudConfig {
            gemini_api_key,
            cloud_run_url: gemini_cloud_run_url,
            cloud_run_auth_token: gemini_cloud_run_auth_token,
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

    // Run until either processor exits, Ctrl+C, or IPC shutdown
    let ipc_session = Arc::clone(&session);
    let ipc_bus = bus.clone();
    let ipc_cancel = cancel.clone();
    tokio::select! {
        _ = processor_handle => {
            tracing::info!("Processor finished, ending session");
        }
        _ = tokio::signal::ctrl_c() => {
            tracing::info!("Ctrl+C received, shutting down");
            bus.send(AuraEvent::Shutdown);
            cancel.cancel();
        }
        _ = async {
            while let Some(cmd) = ipc_cmd_rx.recv().await {
                match cmd {
                    UICommand::Shutdown => {
                        tracing::info!("Shutdown requested via IPC");
                        ipc_bus.send(AuraEvent::Shutdown);
                        ipc_cancel.cancel();
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
                }
            }
        } => {
            tracing::debug!("IPC command channel closed");
        }
    }

    // Shutdown
    let _ = ipc_tx.send(DaemonEvent::Shutdown);
    cancel.cancel();
    mic_shutdown.store(true, Ordering::Release);
    session.disconnect();

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

/// Session reconnection loop for menubar mode.
///
/// Runs inside a background tokio runtime spawned by `main`. Manages the
/// session lifecycle: starts sessions, detects stale resumption handles,
/// and waits for reconnect signals or auto-reconnects on drop.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn run_reconnect_loop(
    gemini_config: GeminiConfig,
    bus: EventBus,
    cancel: CancellationToken,
    memory: Arc<Mutex<aura_memory::SessionMemory>>,
    menu_tx: mpsc::Sender<MenuBarMessage>,
    mut reconnect_rx: mpsc::Receiver<()>,
    mut shutdown_rx: mpsc::Receiver<()>,
    ipc_tx: broadcast::Sender<DaemonEvent>,
) {
    // Persist permission error flag across reconnection attempts so the
    // "Connecting..." status doesn't overwrite a mic permission error.
    let has_permission_error = Arc::new(AtomicBool::new(false));

    // Spawn a dedicated task that listens for the menu Quit signal.
    // This fires even while run_daemon is active, triggering graceful
    // shutdown via the cancellation token.
    let shutdown_cancel = cancel.clone();
    let shutdown_bus = bus.clone();
    let shutdown_menu_tx = menu_tx.clone();
    tokio::spawn(async move {
        if shutdown_rx.recv().await.is_some() {
            tracing::info!("Shutdown requested via menu");
            shutdown_cancel.cancel();
            shutdown_bus.send(AuraEvent::Shutdown);
            let _ = shutdown_menu_tx.send(MenuBarMessage::Shutdown).await;
        }
    });

    // Counters for smart reconnection logic.
    // reconnect_counter: number of successful (>=30s) sessions since last Fresh start.
    // poison_counter: number of consecutive Resume sessions that lasted <30s.
    let mut reconnect_counter: u32 = 0;
    let mut poison_counter: u32 = 0;

    loop {
        // session.rs connection_loop handles transient WebSocket drops with fast retries.
        // When it exhausts retries (permanent failure), run_daemon returns and this
        // outer loop creates a fresh session after user/auto-reconnect signal.

        // Create a new session for each connection attempt
        let session_id = {
            let mem = Arc::clone(&memory);
            match tokio::task::spawn_blocking(move || {
                mem.lock().ok().and_then(|g| g.start_session().ok())
            })
            .await
            {
                Ok(Some(id)) => id,
                Ok(None) => {
                    tracing::error!("Failed to start memory session");
                    tokio::time::sleep(Duration::from_secs(3)).await;
                    continue;
                }
                Err(e) => {
                    tracing::error!("Memory session task panicked: {e}");
                    tokio::time::sleep(Duration::from_secs(3)).await;
                    continue;
                }
            }
        };
        tracing::info!(session_id = %session_id, "Session memory initialized");

        // Determine session mode: Resume if handle exists AND not poisoned.
        let session_mode = {
            let handle: Option<String> =
                cloud::memory_op(&memory, |mem| mem.get_setting("resumption_handle"))
                    .await
                    .flatten()
                    .filter(|h| !h.is_empty());
            match handle {
                Some(h) if reconnect_counter < 3 => {
                    tracing::info!(reconnect_counter, "Resuming session with existing handle");
                    SessionMode::Resume { handle: h }
                }
                _ => {
                    tracing::info!("Starting fresh session (no handle or counter exceeded)");
                    SessionMode::Fresh
                }
            }
        };

        let session_start = std::time::Instant::now();

        if let Err(e) = run_daemon(
            gemini_config.clone(),
            bus.clone(),
            cancel.clone(),
            Arc::clone(&memory),
            session_id,
            Some(menu_tx.clone()),
            Arc::clone(&has_permission_error),
            ipc_tx.clone(),
            session_mode.clone(),
        )
        .await
        {
            tracing::error!("Daemon error: {e}");
        }

        // Track session health to detect a poisoned resume handle.
        let session_duration = session_start.elapsed().as_secs();
        if session_duration < 30 {
            if matches!(session_mode, SessionMode::Resume { .. }) {
                poison_counter += 1;
                tracing::warn!(
                    poison_counter,
                    session_duration,
                    "Short-lived Resume session — possible stale handle"
                );
                if poison_counter >= 2 {
                    tracing::warn!("Poison counter reached — clearing stale resumption handle");
                    let _ =
                        cloud::memory_op(&memory, |mem| mem.set_setting("resumption_handle", ""))
                            .await;
                    poison_counter = 0;
                    reconnect_counter = 0;
                }
            }
        } else {
            poison_counter = 0;
            reconnect_counter += 1;
        }

        // If shutdown was requested, stop reconnecting
        if cancel.is_cancelled() {
            break;
        }

        // Wait for reconnect signal or auto-reconnect after 3s.
        let _ = menu_tx
            .send(MenuBarMessage::SetColor(
                aura_menubar::status_item::DotColor::Gray,
            ))
            .await;
        let _ = menu_tx.send(MenuBarMessage::SetPulsing(false)).await;
        let _ = menu_tx
            .send(MenuBarMessage::SetStatus {
                text: "Disconnected — right-click to reconnect".into(),
            })
            .await;

        let mut user_triggered_reconnect = false;
        tokio::select! {
            Some(()) = reconnect_rx.recv() => {
                user_triggered_reconnect = true;
                tracing::info!("Reconnecting via menu...");
                let _ = menu_tx.send(MenuBarMessage::SetStatus {
                    text: "Reconnecting...".into(),
                }).await;
            }
            _ = cancel.cancelled() => {
                tracing::info!("Shutdown during reconnect wait");
                break;
            }
            _ = tokio::time::sleep(Duration::from_secs(3)) => {
                tracing::info!("Auto-reconnecting...");
                let _ = menu_tx.send(MenuBarMessage::SetStatus {
                    text: "Reconnecting...".into(),
                }).await;
            }
        }

        // Auto-reconnects always start fresh (reset counter so next iteration picks Fresh)
        if !user_triggered_reconnect {
            reconnect_counter = u32::MAX; // force Fresh on next loop iteration
        }
    }
}
