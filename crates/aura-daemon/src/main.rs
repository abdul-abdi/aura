use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Default RMS energy threshold for mic gating while Aura is speaking.
/// Used as the initial value before adaptive calibration completes.
/// Direct speech into the mic is typically 0.05-0.3 RMS; speaker bleed-through
/// from laptop speakers is usually 0.005-0.02.
const BARGE_IN_ENERGY_THRESHOLD_DEFAULT: f32 = 0.04;

/// Number of initial audio chunks to collect for ambient noise calibration.
/// At ~5ms per chunk this is roughly 500ms of audio.
const CALIBRATION_CHUNK_COUNT: usize = 100;

/// Minimum allowed calibrated threshold — prevents the gate from being set
/// so low that any noise triggers a barge-in.
const CALIBRATION_THRESHOLD_MIN: f32 = 0.02;

/// Maximum allowed calibrated threshold — prevents the gate from being set
/// so high that real speech is suppressed.
const CALIBRATION_THRESHOLD_MAX: f32 = 0.15;

/// Maximum characters allowed in a single type_text tool call.
const TYPE_TEXT_MAX_CHARS: usize = 10_000;

/// Maximum click count for click tool (1 = single, 2 = double, 3 = triple).
const CLICK_COUNT_MAX: u32 = 3;

/// Maximum absolute scroll amount in either axis.
const SCROLL_MAX: i32 = 1000;

/// Bounded mic bridge channel capacity — prevents unbounded memory growth
/// during backpressure from the Gemini WebSocket.
const MIC_BRIDGE_CAPACITY: usize = 256;

use anyhow::{Context, Result};
use chrono::Local;
use clap::Parser;
use tokio::sync::{broadcast, mpsc};
mod deploy;
use tokio_util::sync::CancellationToken;

use aura_bridge::script::{ScriptExecutor, ScriptLanguage};
use aura_daemon::bus::EventBus;
use aura_daemon::event::AuraEvent;
use aura_daemon::ipc;
use aura_daemon::protocol::{ConnectionState, DaemonEvent, UICommand};
use aura_daemon::setup::AuraSetup;
use aura_gemini::config::GeminiConfig;
use aura_gemini::session::{GeminiEvent, GeminiLiveSession};
use aura_memory::{MessageRole, SessionMemory};
use aura_menubar::app::{MenuBarApp, MenuBarMessage};
use aura_menubar::status_item::DotColor;
use aura_screen::capture::CaptureTrigger;
use aura_screen::macos::MacOSScreenReader;
use aura_voice::audio::AudioCapture;
use aura_voice::playback::AudioPlayer;
use tracing_subscriber::EnvFilter;

const EVENT_BUS_CAPACITY: usize = 256;
const IPC_BROADCAST_CAPACITY: usize = 256;
const OUTPUT_SAMPLE_RATE: u32 = 24_000;

/// Destructive action guardrail injected into the system prompt.
const DESTRUCTIVE_ACTION_GUARDRAIL: &str = "\n\nSafety — Destructive Actions:\
\n- Before deleting files, emptying trash, quitting unsaved apps, reformatting drives, \
or any action that permanently destroys data, ALWAYS confirm with the user first.\
\n- Phrase it briefly: \"Delete ~/Documents/report.pdf — sure?\" and wait for confirmation.\
\n- Non-destructive actions (opening apps, clicking, typing, moving files) do NOT require confirmation.";

#[derive(Parser)]
#[command(name = "aura", about = "Voice-first AI desktop companion")]
struct Cli {
    /// Run without the menu bar UI (headless mode)
    #[arg(long, global = true)]
    headless: bool,

    /// Enable verbose logging
    #[arg(short, long, global = true)]
    verbose: bool,

    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(clap::Subcommand)]
enum Command {
    /// Deploy aura-proxy to Google Cloud Run
    Deploy {
        /// Accept all defaults without prompting (for non-interactive use)
        #[arg(short, long)]
        yes: bool,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    // Initialize tracing
    let filter = if cli.verbose { "debug" } else { "info" };
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(filter)),
        )
        .init();

    // Handle subcommands
    if let Some(Command::Deploy { yes }) = cli.command {
        return deploy::run_deploy(yes);
    }

    // Validate GEMINI_API_KEY — prompt user if missing
    let gemini_config = match GeminiConfig::from_env() {
        Ok(config) => config,
        Err(_) => {
            tracing::info!("No API key found, prompting user...");
            match prompt_api_key() {
                Some(_) => {
                    tracing::info!("API key saved to config");
                    // Re-read config to pick up proxy settings alongside the new key
                    GeminiConfig::from_env()
                        .context("Failed to load config after saving API key")?
                }
                None => {
                    anyhow::bail!(
                        "Aura requires a Gemini API key. Get one at https://aistudio.google.com/apikey\n\
                         Then set it: echo 'api_key = \"YOUR_KEY\"' > ~/.config/aura/config.toml"
                    );
                }
            }
        }
    };
    tracing::info!("Gemini API key validated");

    // First-run setup
    let data_dir = AuraSetup::default_data_dir();
    let setup = AuraSetup::new(data_dir.clone());
    setup.ensure_dirs()?;
    setup.print_status();

    // Check Screen Recording permission at startup
    check_screen_recording_permission();

    // Initialize session memory
    let db_path = data_dir.join("aura.db");
    let memory = SessionMemory::open(&db_path).context("Failed to open session memory database")?;
    let memory = Arc::new(Mutex::new(memory));

    let bus = EventBus::new(EVENT_BUS_CAPACITY);
    let cancel = CancellationToken::new();

    let (ipc_tx, _) = broadcast::channel::<DaemonEvent>(IPC_BROADCAST_CAPACITY);

    if cli.headless {
        // No menu bar — run tokio directly
        let session_id = {
            let mem = memory
                .lock()
                .map_err(|e| anyhow::anyhow!("Memory lock poisoned: {e}"))?;
            mem.start_session()
                .context("Failed to start memory session")?
        };
        tracing::info!(session_id = %session_id, "Session memory initialized");
        let has_permission_error = Arc::new(AtomicBool::new(false));
        let rt = tokio::runtime::Runtime::new()?;
        rt.block_on(run_daemon(
            gemini_config,
            bus,
            cancel,
            Arc::clone(&memory),
            session_id,
            None,
            has_permission_error,
            ipc_tx.clone(),
        ))?;
    } else {
        // Create menu bar app (must run on main thread)
        let (menu_app, menu_tx, mut reconnect_rx, mut shutdown_rx) = MenuBarApp::new();

        // Spawn tokio runtime on a background thread
        let bg_bus = bus.clone();
        let bg_cancel = cancel.clone();
        let bg_ipc_tx = ipc_tx.clone();
        std::thread::spawn(move || {
            let rt = match tokio::runtime::Runtime::new() {
                Ok(rt) => rt,
                Err(e) => {
                    tracing::error!("Failed to create tokio runtime: {e}");
                    return;
                }
            };
            rt.block_on(async {
                // Persist permission error flag across reconnection attempts so the
                // "Connecting..." status doesn't overwrite a mic permission error.
                let has_permission_error = Arc::new(AtomicBool::new(false));
                let mut first_error = true;

                // Spawn a dedicated task that listens for the menu Quit signal.
                // This fires even while run_daemon is active, triggering graceful
                // shutdown via the cancellation token.
                let shutdown_cancel = bg_cancel.clone();
                let shutdown_bus = bg_bus.clone();
                let shutdown_menu_tx = menu_tx.clone();
                tokio::spawn(async move {
                    if shutdown_rx.recv().await.is_some() {
                        tracing::info!("Shutdown requested via menu");
                        shutdown_cancel.cancel();
                        shutdown_bus.send(AuraEvent::Shutdown);
                        let _ = shutdown_menu_tx
                            .send(MenuBarMessage::Shutdown)
                            .await;
                    }
                });

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

                    if let Err(e) = run_daemon(
                        gemini_config.clone(),
                        bg_bus.clone(),
                        bg_cancel.clone(),
                        Arc::clone(&memory),
                        session_id,
                        Some(menu_tx.clone()),
                        Arc::clone(&has_permission_error),
                        bg_ipc_tx.clone(),
                    )
                    .await
                    {
                        tracing::error!("Daemon error: {e}");

                        // U5: Show native dialog on first connection failure only
                        if first_error {
                            show_error_dialog(&format!(
                                "Aura failed to connect:\n\n{e}\n\nCheck your API key and network connection."
                            ));
                            first_error = false;
                        }
                    }

                    // If shutdown was requested, stop reconnecting
                    if bg_cancel.is_cancelled() {
                        break;
                    }

                    // Wait for reconnect signal or auto-reconnect after 3s
                    let _ = menu_tx.send(MenuBarMessage::SetColor(DotColor::Gray)).await;
                    let _ = menu_tx.send(MenuBarMessage::SetPulsing(false)).await;
                    let _ = menu_tx
                        .send(MenuBarMessage::SetStatus {
                            text: "Disconnected — right-click to reconnect".into(),
                        })
                        .await;

                    tokio::select! {
                        Some(()) = reconnect_rx.recv() => {
                            tracing::info!("Reconnecting via menu...");
                            let _ = menu_tx.send(MenuBarMessage::SetStatus {
                                text: "Reconnecting...".into(),
                            }).await;
                        }
                        _ = bg_cancel.cancelled() => {
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
                }
            });
        });

        // Run menu bar on main thread (blocks forever)
        menu_app.run();
    }

    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn run_daemon(
    mut gemini_config: GeminiConfig,
    bus: EventBus,
    cancel: CancellationToken,
    memory: Arc<Mutex<SessionMemory>>,
    session_id: String,
    menubar_tx: Option<mpsc::Sender<MenuBarMessage>>,
    has_permission_error: Arc<AtomicBool>,
    ipc_tx: broadcast::Sender<DaemonEvent>,
) -> Result<()> {
    // U9: Inject destructive action confirmation guardrail into system prompt (once)
    if !gemini_config
        .system_prompt
        .contains(DESTRUCTIVE_ACTION_GUARDRAIL)
    {
        gemini_config
            .system_prompt
            .push_str(DESTRUCTIVE_ACTION_GUARDRAIL);
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
    let _ = ipc_tx.send(DaemonEvent::ConnectionState {
        state: ConnectionState::Connecting,
        message: "Connecting...".into(),
    });

    // Load persisted session resumption handle (if any) for cross-restart continuity.
    let resumption_handle: Option<String> =
        memory_op(&memory, |mem| mem.get_setting("resumption_handle"))
            .await
            .flatten();
    if resumption_handle.is_some() {
        tracing::info!("Loaded persisted resumption handle for session continuity");
    }

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

    let _mic_available = match AudioCapture::new(None) {
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
            let _ = ipc_tx.send(DaemonEvent::ConnectionState {
                state: ConnectionState::Error,
                message: "Mic access needed — check System Settings > Privacy".into(),
            });
            false
        }
    };

    // Check accessibility permission (needed for input control tools)
    if !aura_input::accessibility::check_accessibility(true) {
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
    // Applies energy gating while Aura is speaking to prevent echo-triggered barge-in.
    let audio_session = Arc::clone(&session);
    let audio_cancel = cancel.clone();
    let bridge_shutdown = Arc::clone(&mic_shutdown);
    let bridge_speaking = Arc::clone(&is_speaking);
    let bridge_threshold = Arc::clone(&barge_in_threshold);
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

        loop {
            tokio::select! {
                _ = audio_cancel.cancelled() => break,
                Some(samples) = tokio_rx.recv() => {
                    // Collect RMS samples for ambient noise calibration
                    if !calibration_done {
                        let rms = rms_energy(&samples);
                        calibration_samples.push(rms);
                        if calibration_samples.len() >= CALIBRATION_CHUNK_COUNT {
                            let calibrated = calibrate_barge_in_threshold(&calibration_samples);
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

                    // When Aura is speaking, gate the mic to prevent echo-triggered barge-in.
                    // Only send audio chunks with enough energy to be intentional speech.
                    if bridge_speaking.load(Ordering::Acquire) {
                        let threshold = f32::from_bits(bridge_threshold.load(Ordering::Acquire));
                        let rms = rms_energy(&samples);
                        if rms < threshold {
                            continue; // Skip this chunk — likely speaker bleed-through
                        }
                        tracing::debug!(rms, threshold, "Barge-in detected while speaking");
                    }

                    if let Err(e) = audio_session.send_audio(&samples).await {
                        tracing::warn!("Failed to send audio to Gemini: {e}");
                        break;
                    }
                }
            }
        }
    });

    // Spawn event processor (Gemini events -> tool handling + audio playback)
    let proc_session = Arc::clone(&session);
    let proc_bus = bus.clone();
    let proc_cancel = cancel.clone();
    let proc_speaking = Arc::clone(&is_speaking);
    let proc_permission_error = Arc::clone(&has_permission_error);
    let proc_ipc_tx = ipc_tx.clone();
    let is_interrupted = Arc::new(AtomicBool::new(false));
    let processor_handle = tokio::spawn(async move {
        if let Err(e) = run_processor(
            proc_session,
            proc_bus,
            proc_cancel,
            memory,
            session_id,
            menubar_tx,
            proc_speaking,
            is_interrupted,
            proc_permission_error,
            proc_ipc_tx,
        )
        .await
        {
            tracing::error!("Processor error: {e}");
        }
    });

    // Run until either processor or daemon exits
    let daemon = aura_daemon::daemon::Daemon::new(bus.clone());
    let ipc_session = Arc::clone(&session);
    let ipc_bus = bus.clone();
    let ipc_cancel = cancel.clone();
    tokio::select! {
        _ = processor_handle => {
            tracing::info!("Processor finished, ending session");
        }
        result = daemon.run() => {
            if let Err(e) = result {
                tracing::error!("Daemon error: {e}");
            }
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
                        if let Err(e) = ipc_session.send_text(&text).await {
                            tracing::warn!("Failed to send IPC text to Gemini: {e}");
                        }
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

#[allow(clippy::too_many_arguments)]
async fn run_processor(
    session: Arc<GeminiLiveSession>,
    bus: EventBus,
    cancel: CancellationToken,
    memory: Arc<Mutex<SessionMemory>>,
    session_id: String,
    menubar_tx: Option<mpsc::Sender<MenuBarMessage>>,
    is_speaking: Arc<AtomicBool>,
    is_interrupted: Arc<AtomicBool>,
    has_permission_error: Arc<AtomicBool>,
    ipc_tx: broadcast::Sender<DaemonEvent>,
) -> Result<()> {
    let mut events = session.subscribe();

    // Audio playback (optional — warn if unavailable)
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

    // Script executor for tool calls
    let executor = ScriptExecutor::new();

    // Screen reader for context gathering
    let screen_reader = MacOSScreenReader::new().context("Failed to initialize screen reader")?;

    // Screen capture loop: 1 FPS JPEG screenshots with change detection
    let capture_trigger = CaptureTrigger::new();
    let cap_notify = Arc::new(tokio::sync::Notify::new());
    let tool_semaphore = Arc::new(tokio::sync::Semaphore::new(8));

    // Shared frame dimensions for coordinate mapping (image pixels -> logical points).
    // Updated by the capture loop after each successful capture.
    let frame_img_w = Arc::new(AtomicU32::new(1920));
    let frame_img_h = Arc::new(AtomicU32::new(1080));
    let frame_logical_w = Arc::new(AtomicU32::new(1920));
    let frame_logical_h = Arc::new(AtomicU32::new(1080));
    let cap_session = Arc::clone(&session);
    let cap_cancel = cancel.clone();
    let cap_trigger = capture_trigger.clone();
    let cap_notify_loop = cap_notify.clone();
    let cap_img_w = Arc::clone(&frame_img_w);
    let cap_img_h = Arc::clone(&frame_img_h);
    let cap_logical_w = Arc::clone(&frame_logical_w);
    let cap_logical_h = Arc::clone(&frame_logical_h);
    tokio::spawn(async move {
        let mut last_hash: u64 = 0;
        let mut interval = tokio::time::interval(Duration::from_secs(1));
        interval.tick().await; // skip first immediate tick

        loop {
            tokio::select! {
                _ = cap_cancel.cancelled() => break,
                _ = interval.tick() => {},
                _ = cap_notify_loop.notified() => {},
            }

            // Clear trigger flag (may have been set alongside notify)
            let _ = cap_trigger.check_and_clear();

            // Capture in a blocking task (JPEG encoding is CPU-bound)
            let frame =
                match tokio::task::spawn_blocking(aura_screen::capture::capture_screen).await {
                    Ok(Ok(frame)) => frame,
                    Ok(Err(e)) => {
                        tracing::warn!("Screen capture failed: {e}");
                        continue;
                    }
                    Err(e) => {
                        tracing::error!("Screen capture task panicked: {e}");
                        continue;
                    }
                };

            // Skip if screen hasn't changed
            if frame.hash == last_hash {
                continue;
            }
            last_hash = frame.hash;

            // Store frame dimensions for coordinate mapping in tool handlers.
            cap_img_w.store(frame.width, Ordering::Release);
            cap_img_h.store(frame.height, Ordering::Release);
            cap_logical_w.store(frame.logical_width, Ordering::Release);
            cap_logical_h.store(frame.logical_height, Ordering::Release);

            // Send coordinate metadata alongside the frame so Gemini knows the
            // image resolution and can specify coordinates in image pixel space.
            let coord_meta = format!(
                "Image resolution: {}x{}. When specifying coordinates for tools (click, move_mouse, drag), \
                 use pixel coordinates within this image (0,0 is top-left, {},{}  is bottom-right).",
                frame.width, frame.height, frame.width, frame.height
            );
            if let Err(e) = cap_session.send_text(&coord_meta).await {
                tracing::warn!("Failed to send frame metadata: {e}");
            }

            // Send to Gemini
            if let Err(e) = cap_session.send_video(&frame.jpeg_base64).await {
                tracing::warn!("Failed to send screen frame: {e}");
                break;
            }
            tracing::debug!(
                width = frame.width,
                height = frame.height,
                scale_factor = frame.scale_factor,
                size_kb = frame.jpeg_base64.len() / 1024,
                "Sent screen frame"
            );
        }
    });

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
                    Ok(GeminiEvent::Connected) => {
                        tracing::info!("Gemini session connected");
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
                        let _ = ipc_tx.send(DaemonEvent::ConnectionState {
                            state: ConnectionState::Connected,
                            message: "Connected — Listening".into(),
                        });

                        let is_first = session.is_first_connect();

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

                            if let Err(e) = session.send_text(&context_msg).await {
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

                            if let Err(e) = session.send_text(&context_msg).await {
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
                        // Ignore audio that arrived after barge-in
                        if is_interrupted.load(Ordering::Acquire) {
                            continue;
                        }
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
                        let _ = ipc_tx.send(DaemonEvent::ToolStarted {
                            name: name.clone(),
                            id: id.clone(),
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
                        let tool_tokens = Arc::clone(&active_tool_tokens);
                        let tool_inflight = Arc::clone(&tools_in_flight);
                        let tool_permission_error = Arc::clone(&has_permission_error);
                        let tool_ipc_tx = ipc_tx.clone();
                        let tool_dims = FrameDims {
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
                            let _permit = match tool_semaphore.acquire().await {
                                Ok(permit) => permit,
                                Err(_) => {
                                    tracing::error!("Tool semaphore closed");
                                    return;
                                }
                            };

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

                            let response = tokio::select! {
                                result = execute_tool(&name, &args, &tool_executor, &tool_screen_reader, tool_dims) => result,
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

                            let tool_success = response
                                .get("success")
                                .and_then(|v| v.as_bool())
                                .unwrap_or(false);

                            // Notify the popover that the tool completed
                            if let Some(ref tx) = tool_menubar_tx {
                                let status_msg = if tool_success {
                                    format!("\u{2705} Done: {name}")
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
                            let _ = tool_ipc_tx.send(DaemonEvent::ToolFinished {
                                name: name.clone(),
                                id: id.clone(),
                                success: tool_success,
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

                            // Trigger immediate screen capture so Gemini sees the result
                            tool_capture_trigger.trigger();
                            tool_cap_notify.notify_one();

                            // Send tool response back to Gemini
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
                    }
                    Ok(GeminiEvent::Transcription { text }) => {
                        // Native audio models generate text and audio independently.
                        // The text is the model's internal reasoning — NOT a transcript
                        // of the spoken audio. It's always longer/different from what's
                        // actually said. Log it for debugging but don't display it.
                        tracing::debug!(transcription = text.lines().next().unwrap_or(""), "Gemini text (not displayed)");
                    }
                    Ok(GeminiEvent::TurnComplete) => {
                        is_speaking.store(false, Ordering::Release);
                        is_interrupted.store(false, Ordering::Release);
                        tracing::debug!("Turn complete");
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
                        let _ = ipc_tx.send(DaemonEvent::ConnectionState {
                            state: ConnectionState::Error,
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
                        let _ = ipc_tx.send(DaemonEvent::ConnectionState {
                            state: ConnectionState::Reconnecting,
                            message: format!("Reconnecting (attempt {attempt})..."),
                        });
                    }
                    Ok(GeminiEvent::Disconnected) => {
                        tracing::info!("Gemini session disconnected");

                        if let Some(ref tx) = menubar_tx {
                            let _ = tx.send(MenuBarMessage::SetPulsing(false)).await;
                            let _ = tx.send(MenuBarMessage::SetColor(DotColor::Gray)).await;
                            let _ = tx.send(MenuBarMessage::SetStatus {
                                text: "Disconnected".into(),
                            }).await;
                        }
                        let _ = ipc_tx.send(DaemonEvent::ConnectionState {
                            state: ConnectionState::Disconnected,
                            message: "Disconnected".into(),
                        });

                        // End the memory session
                        let es_sid = session_id.clone();
                        memory_op(&memory, move |mem| mem.end_session(&es_sid, None)).await;
                        break;
                    }
                    Ok(GeminiEvent::SessionHandle { handle }) => {
                        let prefix_len = handle.len().min(12);
                        tracing::debug!(handle_prefix = &handle[..prefix_len], "Received session resumption handle, persisting");
                        memory_op(&memory, move |mem| {
                            mem.set_setting("resumption_handle", &handle)
                        }).await;
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

/// Show a native macOS dialog prompting for the Gemini API key.
/// Returns the key if entered, None if cancelled.
fn prompt_api_key() -> Option<String> {
    let script = r#"
        set dialogResult to display dialog "Welcome to Aura!" & return & return & "Enter your Gemini API key to get started." & return & "Get one free at aistudio.google.com/apikey" with title "Aura Setup" default answer "" buttons {"Cancel", "Save"} default button "Save" with icon note
        set apiKey to text returned of dialogResult
        return apiKey
    "#;

    let output = std::process::Command::new("osascript")
        .arg("-e")
        .arg(script)
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let key = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if key.is_empty() {
        return None;
    }

    // U4: Validate API key format before saving
    if !validate_api_key(&key) {
        tracing::warn!("Invalid API key format entered");
        show_error_dialog(
            "Invalid API key format.\n\nA valid Gemini API key is at least 20 characters long, \
             ASCII-only, and contains no whitespace.\n\n\
             Get a key at aistudio.google.com/apikey",
        );
        return None;
    }

    // Save to config file
    if let Some(config_dir) = dirs::config_dir() {
        let aura_dir = config_dir.join("aura");
        let _ = std::fs::create_dir_all(&aura_dir);
        let config_path = aura_dir.join("config.toml");
        let content = format!("api_key = \"{key}\"\n");
        if std::fs::write(&config_path, &content).is_ok() {
            // Secure the file (owner read/write only)
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ =
                    std::fs::set_permissions(&config_path, std::fs::Permissions::from_mode(0o600));
            }
        }
    }

    Some(key)
}

/// Validate that an API key has a plausible format.
/// Checks length, ASCII-only, and no whitespace.
fn validate_api_key(key: &str) -> bool {
    key.len() >= 20 && key.is_ascii() && !key.chars().any(|c| c.is_whitespace())
}

/// Show a native macOS error dialog via osascript.
/// Escapes backslashes and double-quotes to prevent AppleScript injection.
fn show_error_dialog(message: &str) {
    let escaped = message
        .replace('\\', "\\\\")
        .replace('\"', "\\\"")
        .replace('\n', "\" & return & \"");
    let script = format!(
        "display dialog \"{escaped}\" with title \"Aura\" buttons {{\"OK\"}} default button \"OK\" with icon stop"
    );
    let _ = std::process::Command::new("osascript")
        .arg("-e")
        .arg(&script)
        .output();
}

/// Check Screen Recording permission at startup and warn if not granted.
fn check_screen_recording_permission() {
    // CGPreflightScreenCaptureAccess returns true if permission is granted.
    // It does NOT prompt the user — we use CGRequestScreenCaptureAccess for that.
    #[link(name = "CoreGraphics", kind = "framework")]
    unsafe extern "C" {
        fn CGPreflightScreenCaptureAccess() -> bool;
        fn CGRequestScreenCaptureAccess() -> bool;
    }

    let has_permission = unsafe { CGPreflightScreenCaptureAccess() };
    if !has_permission {
        tracing::warn!("Screen Recording permission not granted — requesting");
        let granted = unsafe { CGRequestScreenCaptureAccess() };
        if !granted {
            tracing::warn!(
                "Screen Recording permission denied. Screen capture will return blank frames."
            );
            show_error_dialog(
                "Aura needs Screen Recording permission to see your screen.\n\n\
                 Go to System Settings > Privacy & Security > Screen Recording and enable Aura.\n\n\
                 You may need to restart Aura after granting permission.",
            );
        }
    }
}

/// Frame dimension snapshot used to map image-pixel coordinates to logical macOS points.
#[derive(Clone, Copy)]
struct FrameDims {
    img_w: u32,
    img_h: u32,
    logical_w: u32,
    logical_h: u32,
}

impl FrameDims {
    /// Map an x coordinate from image pixels to logical screen points.
    fn to_logical_x(self, x: f64) -> f64 {
        if self.img_w == 0 {
            return x;
        }
        x * (self.logical_w as f64 / self.img_w as f64)
    }

    /// Map a y coordinate from image pixels to logical screen points.
    fn to_logical_y(self, y: f64) -> f64 {
        if self.img_h == 0 {
            return y;
        }
        y * (self.logical_h as f64 / self.img_h as f64)
    }
}

async fn execute_tool(
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
            let timeout = args
                .get("timeout_secs")
                .and_then(|v| v.as_u64())
                .unwrap_or(30);
            let result = executor.run(script, language, timeout).await;
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
        "move_mouse" => {
            let raw_x = args.get("x").and_then(|v| v.as_f64()).unwrap_or(0.0);
            let raw_y = args.get("y").and_then(|v| v.as_f64()).unwrap_or(0.0);
            let x = dims.to_logical_x(raw_x);
            let y = dims.to_logical_y(raw_y);
            run_input_blocking(move || aura_input::mouse::move_mouse(x, y)).await
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
            run_input_blocking(move || aura_input::mouse::click(x, y, &button, count)).await
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
            run_input_blocking(move || aura_input::keyboard::type_text(&text)).await
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
                    run_input_blocking(move || {
                        let mod_refs: Vec<&str> = modifiers.iter().map(|s| s.as_str()).collect();
                        aura_input::keyboard::press_key(keycode, &mod_refs)
                    })
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
            run_input_blocking(move || aura_input::mouse::scroll(dx, dy)).await
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
            run_input_blocking(move || aura_input::mouse::drag(fx, fy, tx, ty)).await
        }
        other => serde_json::json!({
            "success": false,
            "error": format!("Unknown tool: {other}"),
        }),
    }
}

/// Run a blocking input operation on a dedicated thread to avoid blocking tokio.
async fn run_input_blocking<F>(f: F) -> serde_json::Value
where
    F: FnOnce() -> anyhow::Result<()> + Send + 'static,
{
    match tokio::task::spawn_blocking(f).await {
        Ok(Ok(())) => serde_json::json!({ "success": true }),
        Ok(Err(e)) => serde_json::json!({ "success": false, "error": format!("{e}") }),
        Err(e) => serde_json::json!({ "success": false, "error": format!("Task panicked: {e}") }),
    }
}

/// Run a memory operation on a blocking thread to avoid holding the Mutex
/// across await points or blocking the tokio runtime.
/// Logs errors with `tracing::warn!` before converting to `None`.
async fn memory_op<F, T>(memory: &Arc<Mutex<SessionMemory>>, f: F) -> Option<T>
where
    F: FnOnce(&SessionMemory) -> anyhow::Result<T> + Send + 'static,
    T: Send + 'static,
{
    let mem = Arc::clone(memory);
    match tokio::task::spawn_blocking(move || {
        let guard = match mem.lock() {
            Ok(g) => g,
            Err(e) => {
                tracing::warn!("Memory lock poisoned: {e}");
                return None;
            }
        };
        match f(&guard) {
            Ok(val) => Some(val),
            Err(e) => {
                tracing::warn!("Memory operation failed: {e}");
                None
            }
        }
    })
    .await
    {
        Ok(result) => result,
        Err(e) => {
            tracing::error!("Memory operation panicked: {e}");
            None
        }
    }
}

/// Compute root-mean-square energy of an audio buffer.
fn rms_energy(samples: &[f32]) -> f32 {
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
fn calibrate_barge_in_threshold(rms_samples: &[f32]) -> f32 {
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
        // Mean=0.01, stddev=0.005 → threshold = 0.01 + 0.015 = 0.025
        let mut samples = Vec::with_capacity(100);
        for _ in 0..50 {
            samples.push(0.005);
        }
        for _ in 0..50 {
            samples.push(0.015);
        }
        let threshold = calibrate_barge_in_threshold(&samples);
        // mean = 0.01, stddev = 0.005, expected = 0.025
        assert!(threshold > CALIBRATION_THRESHOLD_MIN);
        assert!(threshold < CALIBRATION_THRESHOLD_MAX);
        assert!((threshold - 0.025).abs() < 1e-5);
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
