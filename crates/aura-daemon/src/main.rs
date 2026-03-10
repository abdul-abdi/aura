use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// RMS energy threshold for mic gating while Aura is speaking.
/// Direct speech into the mic is typically 0.05–0.3 RMS; speaker bleed-through
/// from laptop speakers is usually 0.005–0.02. This threshold filters echo
/// while still allowing intentional barge-in.
const BARGE_IN_ENERGY_THRESHOLD: f32 = 0.04;

use anyhow::{Context, Result};
use chrono::Local;
use clap::Parser;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use aura_bridge::script::{ScriptExecutor, ScriptLanguage};
use aura_daemon::bus::EventBus;
use aura_daemon::event::AuraEvent;
use aura_daemon::setup::AuraSetup;
use aura_gemini::config::GeminiConfig;
use aura_gemini::session::{GeminiEvent, GeminiLiveSession};
use aura_memory::{MessageRole, SessionMemory};
use aura_menubar::app::{MenuBarApp, MenuBarMessage};
use aura_menubar::status_item::DotColor;
use aura_screen::macos::MacOSScreenReader;
use aura_voice::audio::AudioCapture;
use aura_voice::playback::AudioPlayer;
use tracing_subscriber::EnvFilter;

const EVENT_BUS_CAPACITY: usize = 64;
const OUTPUT_SAMPLE_RATE: u32 = 24_000;

#[derive(Parser)]
#[command(name = "aura", about = "Voice-first AI desktop companion")]
struct Cli {
    /// Run without the menu bar UI (headless mode)
    #[arg(long)]
    headless: bool,

    /// Enable verbose logging
    #[arg(short, long)]
    verbose: bool,
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

    // Validate GEMINI_API_KEY — prompt user if missing
    let gemini_config = match GeminiConfig::from_env() {
        Ok(config) => config,
        Err(_) => {
            tracing::info!("No API key found, prompting user...");
            match prompt_api_key() {
                Some(key) => {
                    tracing::info!("API key saved to config");
                    GeminiConfig::from_env_inner(&key)
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

    if cli.headless {
        // No menu bar — run tokio directly
        let session_id = {
            let mem = memory.lock().unwrap();
            mem.start_session().context("Failed to start memory session")?
        };
        tracing::info!(session_id = %session_id, "Session memory initialized");
        let rt = tokio::runtime::Runtime::new()?;
        rt.block_on(run_daemon(
            gemini_config,
            bus,
            cancel,
            Arc::clone(&memory),
            session_id,
            None,
        ))?;
    } else {
        // Create menu bar app (must run on main thread)
        let (menu_app, menu_tx, mut reconnect_rx) = MenuBarApp::new();

        // Spawn tokio runtime on a background thread
        let bg_bus = bus.clone();
        let bg_cancel = cancel.clone();
        std::thread::spawn(move || {
            let rt = tokio::runtime::Runtime::new().expect("Failed to create tokio runtime");
            rt.block_on(async {
                loop {
                    // Create a new session for each connection attempt
                    let session_id = {
                        let mem = memory.lock().unwrap();
                        match mem.start_session() {
                            Ok(id) => id,
                            Err(e) => {
                                tracing::error!("Failed to start memory session: {e}");
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
                    )
                    .await
                    {
                        tracing::error!("Daemon error: {e}");
                    }

                    // Wait for reconnect signal or auto-reconnect after 3s
                    let _ = menu_tx.send(MenuBarMessage::SetColor(DotColor::Gray)).await;
                    let _ = menu_tx.send(MenuBarMessage::SetPulsing(false)).await;
                    let _ = menu_tx.send(MenuBarMessage::SetStatus {
                        text: "Disconnected — right-click to reconnect".into(),
                    }).await;

                    tokio::select! {
                        Some(()) = reconnect_rx.recv() => {
                            tracing::info!("Reconnecting via menu...");
                            let _ = menu_tx.send(MenuBarMessage::SetStatus {
                                text: "Reconnecting...".into(),
                            }).await;
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

async fn run_daemon(
    gemini_config: GeminiConfig,
    bus: EventBus,
    cancel: CancellationToken,
    memory: Arc<Mutex<SessionMemory>>,
    session_id: String,
    menubar_tx: Option<mpsc::Sender<MenuBarMessage>>,
) -> Result<()> {
    // Connect to Gemini Live API
    if let Some(ref tx) = menubar_tx {
        let _ = tx.send(MenuBarMessage::SetStatus {
            text: "Connecting...".into(),
        }).await;
    }

    let session = GeminiLiveSession::connect(gemini_config)
        .await
        .context("Failed to connect Gemini Live session")?;

    let session = Arc::new(session);

    // Set up mic capture on a dedicated std::thread (cpal's Stream is !Send)
    let (std_tx, std_rx) = std::sync::mpsc::channel::<Vec<f32>>();
    let mic_shutdown = Arc::new(AtomicBool::new(false));
    let is_speaking = Arc::new(AtomicBool::new(false));

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
                    while !mic_shutdown_flag.load(Ordering::Relaxed) {
                        std::thread::park_timeout(Duration::from_millis(500));
                    }
                    tracing::info!("Mic capture stopped");
                })?;
            true
        }
        Err(e) => {
            tracing::warn!("Mic unavailable: {e}");
            if let Some(ref tx) = menubar_tx {
                let _ = tx.send(MenuBarMessage::SetColor(DotColor::Red)).await;
                let _ = tx.send(MenuBarMessage::SetStatus {
                    text: "Mic access needed — check System Settings > Privacy".into(),
                }).await;
            }
            false
        }
    };

    // Bridge std::sync::mpsc -> tokio -> session.send_audio()
    // Applies energy gating while Aura is speaking to prevent echo-triggered barge-in.
    let audio_session = Arc::clone(&session);
    let audio_cancel = cancel.clone();
    let bridge_shutdown = Arc::clone(&mic_shutdown);
    let bridge_speaking = Arc::clone(&is_speaking);
    tokio::spawn(async move {
        let (tokio_tx, mut tokio_rx) = tokio::sync::mpsc::channel::<Vec<f32>>(64);

        // Spawn a blocking task to drain the std::sync::mpsc channel
        let bridge_tx = tokio_tx.clone();
        tokio::task::spawn_blocking(move || {
            loop {
                match std_rx.recv_timeout(Duration::from_millis(200)) {
                    Ok(samples) => {
                        if bridge_tx.blocking_send(samples).is_err() {
                            break;
                        }
                    }
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                        if bridge_shutdown.load(Ordering::Relaxed) {
                            break;
                        }
                    }
                    Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
                }
            }
        });

        loop {
            tokio::select! {
                _ = audio_cancel.cancelled() => break,
                Some(samples) = tokio_rx.recv() => {
                    // When Aura is speaking, gate the mic to prevent echo-triggered barge-in.
                    // Only send audio chunks with enough energy to be intentional speech.
                    if bridge_speaking.load(Ordering::Relaxed) {
                        let rms = rms_energy(&samples);
                        if rms < BARGE_IN_ENERGY_THRESHOLD {
                            continue; // Skip this chunk — likely speaker bleed-through
                        }
                        tracing::debug!(rms, "Barge-in detected while speaking");
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
    let processor_handle = tokio::spawn(async move {
        if let Err(e) = run_processor(
            proc_session,
            proc_bus,
            proc_cancel,
            memory,
            session_id,
            menubar_tx,
            proc_speaking,
        )
        .await
        {
            tracing::error!("Processor error: {e}");
        }
    });

    // Run until either processor or daemon exits
    let daemon = aura_daemon::daemon::Daemon::new(bus);
    tokio::select! {
        _ = processor_handle => {
            tracing::info!("Processor finished, ending session");
        }
        result = daemon.run() => {
            if let Err(e) = result {
                tracing::error!("Daemon error: {e}");
            }
        }
    }

    // Shutdown
    cancel.cancel();
    mic_shutdown.store(true, Ordering::Relaxed);
    session.disconnect();

    Ok(())
}

async fn run_processor(
    session: Arc<GeminiLiveSession>,
    bus: EventBus,
    cancel: CancellationToken,
    memory: Arc<Mutex<SessionMemory>>,
    session_id: String,
    menubar_tx: Option<mpsc::Sender<MenuBarMessage>>,
    is_speaking: Arc<AtomicBool>,
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

    // Spawn periodic screen context updates (every 15s)
    let ctx_session = Arc::clone(&session);
    let ctx_cancel = cancel.clone();
    tokio::spawn(async move {
        let reader = match MacOSScreenReader::new() {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!("Screen reader for periodic context failed: {e}");
                return;
            }
        };
        let mut last_summary = String::new();
        let mut interval = tokio::time::interval(Duration::from_secs(15));
        interval.tick().await; // skip first immediate tick

        loop {
            tokio::select! {
                _ = ctx_cancel.cancelled() => break,
                _ = interval.tick() => {
                    let summary = match reader.capture_context() {
                        Ok(ctx) => ctx.summary(),
                        Err(_) => continue,
                    };

                    // Only send if context actually changed
                    if summary == last_summary {
                        continue;
                    }
                    last_summary = summary.clone();

                    let now = Local::now();
                    let msg = format!(
                        "[Screen update — {}]\n{}",
                        now.format("%I:%M %p"),
                        summary,
                    );
                    tracing::debug!("Sending periodic screen context");
                    if let Err(e) = ctx_session.send_text(&msg).await {
                        tracing::warn!("Failed to send periodic context: {e}");
                        break;
                    }
                }
            }
        }
    });

    tracing::info!("Gemini event processor running");

    loop {
        tokio::select! {
            _ = cancel.cancelled() => break,
            event = events.recv() => {
                match event {
                    Ok(GeminiEvent::Connected) => {
                        tracing::info!("Gemini session connected");
                        let _ = bus.send(AuraEvent::GeminiConnected);

                        // Enable pulsing dot + status
                        if let Some(ref tx) = menubar_tx {
                            let _ = tx.send(MenuBarMessage::SetColor(DotColor::Green)).await;
                            let _ = tx.send(MenuBarMessage::SetPulsing(true)).await;
                            let _ = tx.send(MenuBarMessage::SetStatus {
                                text: "Connected — Listening".into(),
                            }).await;
                        }

                        // Gather screen context for initial greeting
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

                        // Get rich time/location context
                        let now = Local::now();
                        let time_context = format!(
                            "Current time: {} ({}). Date: {}.",
                            now.format("%I:%M %p"),
                            now.format("%Z"),
                            now.format("%A, %B %-d, %Y"),
                        );

                        let context_msg = format!(
                            "[System: User just activated Aura. {time_context} Current screen context:\n{greeting_context}]"
                        );

                        // Log initial context to memory
                        if let Err(e) = memory.lock().unwrap().add_message(
                            &session_id,
                            MessageRole::User,
                            &context_msg,
                            None,
                        ) {
                            tracing::warn!("Failed to log greeting context to memory: {e}");
                        }

                        // Send greeting context to Gemini
                        if let Err(e) = session.send_text(&context_msg).await {
                            tracing::warn!("Failed to send greeting context to Gemini: {e}");
                        }

                        // Start the audio stream so incoming chunks queue seamlessly
                        if let Some(ref p) = player {
                            if let Err(e) = p.start_stream(OUTPUT_SAMPLE_RATE) {
                                tracing::error!("Failed to start audio stream: {e}");
                            }
                        }
                    }
                    Ok(GeminiEvent::AudioResponse { samples }) => {
                        is_speaking.store(true, Ordering::Relaxed);
                        if let Some(ref p) = player
                            && let Err(e) = p.append(samples)
                        {
                            tracing::error!("Audio playback failed: {e}");
                        }
                    }
                    Ok(GeminiEvent::ToolCall { id, name, args }) => {
                        tracing::info!(name = %name, "Tool call");
                        if let Err(e) = memory.lock().unwrap().add_message(
                            &session_id,
                            MessageRole::ToolCall,
                            &format!("{name}: {args}"),
                            None,
                        ) {
                            tracing::warn!("Failed to log tool call to memory: {e}");
                        }

                        // Pause pulsing, set amber while executing
                        if let Some(ref tx) = menubar_tx {
                            let _ = tx.send(MenuBarMessage::SetPulsing(false)).await;
                            let _ = tx.send(MenuBarMessage::SetColor(DotColor::Amber)).await;
                            let _ = tx.send(MenuBarMessage::SetStatus {
                                text: format!("Running {name}..."),
                            }).await;
                        }

                        let response = match name.as_str() {
                            "run_applescript" => {
                                let script = args.get("script")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("");
                                let language = match args.get("language").and_then(|v| v.as_str()) {
                                    Some("javascript") => ScriptLanguage::JavaScript,
                                    _ => ScriptLanguage::AppleScript,
                                };
                                let timeout = args.get("timeout_secs")
                                    .and_then(|v| v.as_u64())
                                    .unwrap_or(30);
                                let result = executor.run(script, language, timeout).await;

                                let _ = bus.send(AuraEvent::ToolExecuted {
                                    name: name.clone(),
                                    success: result.success,
                                    output: result.stdout.clone(),
                                });

                                serde_json::json!({
                                    "success": result.success,
                                    "stdout": result.stdout,
                                    "stderr": result.stderr,
                                })
                            }
                            "get_screen_context" => {
                                match screen_reader.capture_context() {
                                    Ok(ctx) => {
                                        let summary = ctx.summary();
                                        let _ = bus.send(AuraEvent::ToolExecuted {
                                            name: name.clone(),
                                            success: true,
                                            output: summary.clone(),
                                        });
                                        serde_json::json!({
                                            "success": true,
                                            "context": summary,
                                        })
                                    }
                                    Err(e) => {
                                        let error = format!("{e}");
                                        let _ = bus.send(AuraEvent::ToolExecuted {
                                            name: name.clone(),
                                            success: false,
                                            output: error.clone(),
                                        });
                                        serde_json::json!({
                                            "success": false,
                                            "error": error,
                                        })
                                    }
                                }
                            }
                            "shutdown_aura" => {
                                tracing::info!("Shutdown requested via voice command");
                                let _ = bus.send(AuraEvent::ToolExecuted {
                                    name: name.clone(),
                                    success: true,
                                    output: "Shutting down Aura".into(),
                                });

                                // Send response before shutting down
                                let response = serde_json::json!({
                                    "success": true,
                                    "message": "Aura is shutting down. Goodbye!",
                                });
                                if let Err(e) = session.send_tool_response(id, name, response).await {
                                    tracing::error!("Failed to send shutdown tool response: {e}");
                                }

                                // Brief delay to let Gemini's goodbye audio play
                                tokio::time::sleep(Duration::from_secs(3)).await;

                                // Trigger shutdown
                                if let Some(ref tx) = menubar_tx {
                                    let _ = tx.send(MenuBarMessage::Shutdown).await;
                                }
                                let _ = bus.send(AuraEvent::Shutdown);
                                break;
                            }
                            other => serde_json::json!({
                                "success": false,
                                "error": format!("Unknown tool: {other}"),
                            }),
                        };

                        // Log tool result to memory
                        if let Err(e) = memory.lock().unwrap().add_message(
                            &session_id,
                            MessageRole::ToolResult,
                            &response.to_string(),
                            None,
                        ) {
                            tracing::warn!("Failed to log tool result to memory: {e}");
                        }

                        // Resume pulsing + green
                        if let Some(ref tx) = menubar_tx {
                            let _ = tx.send(MenuBarMessage::SetColor(DotColor::Green)).await;
                            let _ = tx.send(MenuBarMessage::SetPulsing(true)).await;
                            let _ = tx.send(MenuBarMessage::SetStatus {
                                text: "Connected — Listening".into(),
                            }).await;
                        }

                        if let Err(e) = session.send_tool_response(id, name, response).await {
                            tracing::error!("Failed to send tool response: {e}");
                        }
                    }
                    Ok(GeminiEvent::ToolCallCancellation { ids }) => {
                        tracing::info!(?ids, "Tool call(s) cancelled");
                    }
                    Ok(GeminiEvent::Interrupted) => {
                        tracing::info!("Gemini interrupted — stopping playback");
                        is_speaking.store(false, Ordering::Relaxed);
                        if let Some(ref p) = player {
                            p.stop();
                        }
                        let _ = bus.send(AuraEvent::BargeIn);
                    }
                    Ok(GeminiEvent::Transcription { text }) => {
                        // Native audio models generate text and audio independently.
                        // The text is the model's internal reasoning — NOT a transcript
                        // of the spoken audio. It's always longer/different from what's
                        // actually said. Log it for debugging but don't display it.
                        tracing::debug!(transcription = %text.lines().next().unwrap_or(""), "Gemini text (not displayed)");
                    }
                    Ok(GeminiEvent::TurnComplete) => {
                        is_speaking.store(false, Ordering::Relaxed);
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
                    }
                    Ok(GeminiEvent::Reconnecting { attempt }) => {
                        tracing::warn!(attempt, "Gemini reconnecting");
                        let _ = bus.send(AuraEvent::GeminiReconnecting { attempt });

                        if let Some(ref tx) = menubar_tx {
                            let _ = tx.send(MenuBarMessage::SetPulsing(false)).await;
                            let _ = tx.send(MenuBarMessage::SetColor(DotColor::Amber)).await;
                            let _ = tx.send(MenuBarMessage::SetStatus {
                                text: format!("Reconnecting (attempt {attempt})..."),
                            }).await;
                        }
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

                        // End the memory session
                        let _ = memory.lock().unwrap().end_session(&session_id, None);
                        break;
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        tracing::warn!("Processor lagged by {n} Gemini events");
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
                let _ = std::fs::set_permissions(
                    &config_path,
                    std::fs::Permissions::from_mode(0o600),
                );
            }
        }
    }

    Some(key)
}

/// Compute root-mean-square energy of an audio buffer.
fn rms_energy(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    let sum_sq: f32 = samples.iter().map(|s| s * s).sum();
    (sum_sq / samples.len() as f32).sqrt()
}
