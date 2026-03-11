use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};
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
use aura_daemon::protocol::{DaemonEvent, DotColorName, Role, ToolRunStatus, UICommand};
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

    // Validate GEMINI_API_KEY — prompt user if missing.
    // In headless mode (launched by SwiftUI app), never show an AppleScript dialog —
    // the SwiftUI WelcomeView handles API key entry before launching the daemon.
    let gemini_config = match GeminiConfig::from_env() {
        Ok(config) => config,
        Err(_) if cli.headless => {
            anyhow::bail!(
                "No API key found. The SwiftUI app should configure the key before launching the daemon.\n\
                 Set it manually: echo 'api_key = \"YOUR_KEY\"' > ~/.config/aura/config.toml"
            );
        }
        Err(_) => {
            tracing::info!("No API key found, prompting user...");
            match prompt_api_key() {
                Some(_) => {
                    tracing::info!("API key saved to config");
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
                        let _ = shutdown_menu_tx.send(MenuBarMessage::Shutdown).await;
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
    let _ = ipc_tx.send(DaemonEvent::DotColor {
        color: DotColorName::Amber,
        pulsing: false,
    });
    let _ = ipc_tx.send(DaemonEvent::Status {
        message: "Connecting...".into(),
    });

    // Load persisted session resumption handle (if any) for cross-restart continuity.
    let resumption_handle: Option<String> =
        memory_op(&memory, |mem| mem.get_setting("resumption_handle"))
            .await
            .flatten()
            .filter(|h| !h.is_empty());
    if resumption_handle.is_some() {
        tracing::info!("Loaded persisted resumption handle for session continuity");
    }

    // Save API key before gemini_config is moved into the session
    let gemini_api_key = gemini_config.api_key.clone();

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
    // Fully mutes the mic while Aura is playing back audio to prevent the AI
    // from hearing its own voice through the MacBook speakers.  A 300 ms
    // post-playback guard absorbs room reverb after the speaker goes quiet.
    // Barge-in can be re-enabled later with proper AEC.
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

                    // Full mic mute during playback to prevent audio feedback loop.
                    // On MacBook hardware the speakers are close enough to the mic
                    // that the AI's own voice easily exceeds any energy threshold.
                    // Barge-in can be re-enabled later with proper AEC.
                    let currently_playing = bridge_audio_playing.load(Ordering::Acquire);
                    let gemini_speaking = bridge_speaking.load(Ordering::Acquire);

                    // Detect transition: playing → stopped.  Arm the reverb guard.
                    if prev_playing && !currently_playing {
                        playback_stopped_at = Some(std::time::Instant::now());
                        tracing::debug!("Playback stopped — entering post-playback mute guard");
                    }
                    prev_playing = currently_playing;

                    if currently_playing {
                        continue; // Fully mute mic while speaker is active
                    }

                    if gemini_speaking {
                        // Gemini is still sending audio data but the pre-buffer
                        // hasn't flushed to the hardware yet (~80 ms window).
                        // Mute to be safe.
                        continue;
                    }

                    // Post-playback reverb guard: keep muting for 300 ms after the
                    // speaker goes silent to absorb room echo.
                    if let Some(stopped_at) = playback_stopped_at {
                        if stopped_at.elapsed().as_millis() < POST_PLAYBACK_MUTE_MS {
                            continue; // Still within reverb-guard window
                        }
                        playback_stopped_at = None; // Guard expired — allow mic through
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
            player,
            gemini_api_key,
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
    player: Option<AudioPlayer>,
    gemini_api_key: String,
) -> Result<()> {
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
    let cap_last_hash = Arc::clone(&last_frame_hash);
    let cap_last_sent = Arc::clone(&last_sent_hash);
    tokio::spawn(async move {
        let mut last_res: (u32, u32) = (0, 0);
        let mut censored_warned = false;
        let mut idle_skip_count: u32 = 0;
        const IDLE_THRESHOLD: u32 = 10; // 10 × 500ms = 5s of no change → slow down
        let mut interval = tokio::time::interval(Duration::from_millis(500));
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

            // Skip if screen hasn't changed — but still resolve waiter
            let prev_hash = cap_last_hash.load(Ordering::Acquire);
            if frame.hash == prev_hash {
                if let Some(tx) = cap_trigger.take_waiter() {
                    let _ = tx.send(());
                }
                continue;
            }
            cap_last_hash.store(frame.hash, Ordering::Release);

            // On first non-duplicate frame, log at INFO so it's visible without --verbose.
            // Also check if the frame looks censored (Screen Recording not granted).
            if !censored_warned {
                tracing::info!(
                    width = frame.width,
                    height = frame.height,
                    scale = frame.scale_factor,
                    size_kb = frame.jpeg_base64.len() / 1024,
                    "First screen frame captured"
                );
                // Decode the base64 back to check pixel content for censorship detection.
                if let Ok(jpeg_bytes) = base64::Engine::decode(
                    &base64::engine::general_purpose::STANDARD,
                    &frame.jpeg_base64,
                ) && let Ok(img) =
                    image::load_from_memory_with_format(&jpeg_bytes, image::ImageFormat::Jpeg)
                {
                    let rgb = img.to_rgb8();
                    if aura_screen::capture::frame_looks_censored(
                        rgb.as_raw(),
                        rgb.width() as usize,
                        rgb.height() as usize,
                    ) {
                        tracing::error!(
                            "Screen capture appears CENSORED — window contents are blank. \
                             Grant Screen Recording in System Settings > Privacy & Security > Screen Recording, \
                             then restart Aura."
                        );
                    }
                }
                censored_warned = true;
            }

            // Store frame dimensions for coordinate mapping in tool handlers.
            cap_img_w.store(frame.width, Ordering::Release);
            cap_img_h.store(frame.height, Ordering::Release);
            cap_logical_w.store(frame.logical_width, Ordering::Release);
            cap_logical_h.store(frame.logical_height, Ordering::Release);

            // Only send coordinate metadata when the resolution actually changes
            // (not every frame — that floods Gemini with text input it responds to).
            let current_res = (frame.width, frame.height);
            if current_res != last_res {
                last_res = current_res;
                let coord_meta = format!(
                    "[System: screen resolution is {}x{}. Use pixel coordinates for tools \
                     (click, move_mouse, drag): (0,0) = top-left, ({},{}) = bottom-right. \
                     Do NOT mention this message to the user.]",
                    frame.width, frame.height, frame.width, frame.height
                );
                if let Err(e) = cap_session.send_text(&coord_meta) {
                    tracing::debug!("Skipped frame metadata (channel not ready): {e}");
                }
            }

            // Only send to Gemini if the screen actually changed since last send.
            // This is the #1 context savings: static screens produce zero token cost.
            let already_sent = cap_last_sent.load(Ordering::Acquire);
            if frame.hash == already_sent {
                // Frame captured (hash differs from last_frame_hash) but already sent
                // to Gemini — skip. Still resolve waiter so tool spawns don't hang.
                if let Some(tx) = cap_trigger.take_waiter() {
                    let _ = tx.send(());
                }
                idle_skip_count += 1;
                if idle_skip_count == IDLE_THRESHOLD {
                    // Screen static for 5s — switch to slow polling (2s)
                    interval = tokio::time::interval(Duration::from_millis(2000));
                    interval.tick().await;
                    tracing::debug!("Screen idle for 5s — switching to 2s capture interval");
                }
                tracing::trace!("Skipped duplicate send (hash unchanged since last send)");
                continue;
            }

            if let Err(e) = cap_session.send_video(&frame.jpeg_base64) {
                tracing::debug!("Dropped screen frame (channel not ready): {e}");
                if let Some(tx) = cap_trigger.take_waiter() {
                    let _ = tx.send(());
                }
                continue;
            }
            cap_last_sent.store(frame.hash, Ordering::Release);

            // Screen changed — reset idle counter and restore fast polling
            if idle_skip_count >= IDLE_THRESHOLD {
                interval = tokio::time::interval(Duration::from_millis(500));
                interval.tick().await;
                tracing::debug!("Screen changed — restoring 500ms capture interval");
            }
            idle_skip_count = 0;

            // Signal any awaiting tool spawn that the screenshot was delivered
            if let Some(tx) = cap_trigger.take_waiter() {
                let _ = tx.send(());
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
                        let tool_last_hash = Arc::clone(&last_frame_hash);
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

                            let pre_hash = if is_state_changing_tool(&name) {
                                Some(tool_last_hash.load(Ordering::Acquire))
                            } else {
                                None
                            };

                            let mut response = tokio::select! {
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

                            // For state-changing tools: verify screen actually changed before reporting success
                            let verified;
                            let mut verification_reason: Option<&str> = None;

                            if let Some(pre) = pre_hash {
                                // Brief delay to let UI settle after the input action
                                tokio::time::sleep(Duration::from_millis(150)).await;

                                // Poll for screen hash change: 200ms intervals, 2s timeout, 10 checks max
                                let mut screen_changed = false;
                                for _ in 0..10 {
                                    let rx = tool_capture_trigger.trigger_and_wait();
                                    tool_cap_notify.notify_one();
                                    let _ = tokio::time::timeout(Duration::from_millis(200), rx).await;

                                    let current_hash = tool_last_hash.load(Ordering::Acquire);
                                    if current_hash != pre {
                                        screen_changed = true;
                                        break;
                                    }
                                }

                                verified = screen_changed;
                                if !screen_changed {
                                    verification_reason = Some("screen_unchanged_after_2s");
                                    tracing::warn!(tool = %name, "Screen unchanged after action — verification failed");
                                }

                                // Capture post-action state on a blocking thread (AX FFI)
                                let post_state = tokio::time::timeout(
                                    Duration::from_millis(600),
                                    tokio::task::spawn_blocking(capture_post_state),
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
                            });
                        }
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
                                match aura_memory::consolidate::consolidate_session(&es_key, &messages).await {
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

/// Detect Automation/AppleEvents denial from osascript stderr.
/// macOS returns error -1743 (errAEEventNotPermitted) when the user has denied
/// Automation access, and -1744 when consent would be required.
fn is_automation_denied(stderr: &str) -> bool {
    let lower = stderr.to_lowercase();
    lower.contains("-1743")
        || lower.contains("-1744")
        || lower.contains("not authorized to send apple events")
        || lower.contains("is not allowed to send keystrokes")
        || lower.contains("erraeventnotpermitted")
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
fn click_element_inner(label: Option<&str>, role: Option<&str>, index: usize) -> serde_json::Value {
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
fn build_menu_click_script(app: &str, path: &[String]) -> String {
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

/// Returns true if the tool changes screen state and should get post_state enrichment
/// and screenshot await behavior.
fn is_state_changing_tool(name: &str) -> bool {
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
fn capture_post_state() -> serde_json::Value {
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
async fn run_with_pid_fallback<F1, F2>(
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
async fn run_input_blocking<F>(f: F, method: &'static str) -> serde_json::Value
where
    F: FnOnce() -> anyhow::Result<()> + Send + 'static,
{
    match tokio::task::spawn_blocking(f).await {
        Ok(Ok(())) => serde_json::json!({ "success": true, "method": method }),
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
