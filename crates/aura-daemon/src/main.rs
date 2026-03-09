use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use anyhow::{Context, Result};
use clap::Parser;
use tokio_util::sync::CancellationToken;
use winit::event_loop::EventLoopProxy;

use aura_bridge::actions::ActionExecutor;
use aura_daemon::bus::EventBus;
use aura_daemon::daemon::Daemon;
use aura_daemon::event::{AuraEvent, OverlayContent};
use aura_daemon::setup::AuraSetup;
use aura_gemini::config::GeminiConfig;
use aura_gemini::session::{GeminiEvent, GeminiLiveSession};
use aura_gemini::tools::function_call_to_action;
use aura_overlay::renderer::OverlayState;
use aura_overlay::window::{OverlayMessage, OverlayWindow, create_event_loop};
use aura_voice::audio::AudioCapture;
use aura_voice::playback::AudioPlayer;
use tracing_subscriber::EnvFilter;

const EVENT_BUS_CAPACITY: usize = 64;
const OUTPUT_SAMPLE_RATE: u32 = 24_000;

#[derive(Parser)]
#[command(name = "aura", about = "Voice-first AI desktop companion")]
struct Cli {
    /// Run without the overlay window
    #[arg(long)]
    no_overlay: bool,

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

    // Validate GEMINI_API_KEY early before doing anything else
    let gemini_config = GeminiConfig::from_env()
        .context("GEMINI_API_KEY must be set. Get one at https://aistudio.google.com/apikey")?;
    tracing::info!("Gemini API key validated");

    // First-run setup
    let setup = AuraSetup::new(AuraSetup::default_data_dir());
    setup.ensure_dirs()?;
    setup.print_status();

    let bus = EventBus::new(EVENT_BUS_CAPACITY);
    let cancel = CancellationToken::new();

    if cli.no_overlay {
        // No overlay — run tokio directly
        let rt = tokio::runtime::Runtime::new()?;
        rt.block_on(run_daemon(gemini_config, bus, cancel))?;
    } else {
        // Create the event loop BEFORE starting tokio (winit must own the main thread)
        let event_loop = create_event_loop().context("Failed to create overlay event loop")?;
        let proxy = event_loop.create_proxy();

        // Spawn tokio runtime on a background thread
        let bg_bus = bus.clone();
        let bg_cancel = cancel.clone();
        let bg_proxy = proxy.clone();
        std::thread::spawn(move || {
            let rt = tokio::runtime::Runtime::new().expect("Failed to create tokio runtime");
            rt.block_on(async {
                // Spawn overlay bridge (daemon events -> overlay messages)
                let bridge_bus = bg_bus.clone();
                let bridge_proxy = bg_proxy.clone();
                let bridge_cancel = bg_cancel.clone();
                tokio::spawn(async move {
                    run_overlay_bridge(bridge_bus, bridge_proxy, bridge_cancel).await;
                });

                if let Err(e) = run_daemon(gemini_config, bg_bus, bg_cancel).await {
                    tracing::error!("Daemon error: {e}");
                }

                // Tell overlay to shut down when daemon exits
                let _ = bg_proxy.send_event(OverlayMessage::Shutdown);
            });
        });

        // Run winit event loop on main thread (never returns)
        let mut overlay = OverlayWindow::new();
        event_loop.run_app(&mut overlay)?;
    }

    Ok(())
}

async fn run_daemon(
    gemini_config: GeminiConfig,
    bus: EventBus,
    cancel: CancellationToken,
) -> Result<()> {
    // Connect to Gemini Live API
    let session = GeminiLiveSession::connect(gemini_config)
        .await
        .context("Failed to connect Gemini Live session")?;

    let session = Arc::new(session);

    // Set up mic capture on a dedicated std::thread (cpal's Stream is !Send)
    let (std_tx, std_rx) = std::sync::mpsc::channel::<Vec<f32>>();
    let mic_shutdown = Arc::new(AtomicBool::new(false));

    let capture = AudioCapture::new(None).context("Failed to initialize audio capture")?;
    let mic_shutdown_flag = Arc::clone(&mic_shutdown);
    let mic_thread = std::thread::Builder::new()
        .name("aura-mic-capture".into())
        .spawn(move || {
            // start() returns a Stream that must be kept alive
            let _stream = match capture.start(std_tx) {
                Ok(stream) => stream,
                Err(e) => {
                    tracing::error!("Failed to start audio capture: {e}");
                    return;
                }
            };
            tracing::info!("Mic capture started");
            // Block until shutdown is signaled — dropping _stream stops capture
            while !mic_shutdown_flag.load(Ordering::Relaxed) {
                std::thread::park_timeout(Duration::from_millis(500));
            }
            tracing::info!("Mic capture stopped");
        })?;

    // Bridge std::sync::mpsc -> tokio -> session.send_audio()
    let audio_session = Arc::clone(&session);
    let audio_cancel = cancel.clone();
    let bridge_shutdown = Arc::clone(&mic_shutdown);
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
                    if let Err(e) = audio_session.send_audio(&samples).await {
                        tracing::warn!("Failed to send audio to Gemini: {e}");
                        break;
                    }
                }
            }
        }
    });

    // Spawn event processor
    let proc_session = Arc::clone(&session);
    let proc_bus = bus.clone();
    let proc_cancel = cancel.clone();
    let processor_handle = tokio::spawn(async move {
        if let Err(e) = run_processor(proc_session, proc_bus, proc_cancel).await {
            tracing::error!("Processor error: {e}");
        }
    });

    // Run the daemon event loop
    let daemon = Daemon::new(bus);
    daemon.run().await?;

    // Shutdown
    cancel.cancel();
    mic_shutdown.store(true, Ordering::Relaxed);
    mic_thread.thread().unpark();
    session.disconnect();
    let _ = processor_handle.await;
    let _ = mic_thread.join();

    Ok(())
}

async fn run_processor(
    session: Arc<GeminiLiveSession>,
    bus: EventBus,
    cancel: CancellationToken,
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

    // macOS action executor
    #[cfg(target_os = "macos")]
    let executor = aura_bridge::macos::MacOSExecutor::new();

    tracing::info!("Gemini event processor running");

    loop {
        tokio::select! {
            _ = cancel.cancelled() => break,
            event = events.recv() => {
                match event {
                    Ok(GeminiEvent::Connected) => {
                        tracing::info!("Gemini session connected");
                        let _ = bus.send(AuraEvent::GeminiConnected);
                    }
                    Ok(GeminiEvent::AudioResponse { samples }) => {
                        if let Some(ref p) = player
                            && let Err(e) = p.play(samples, OUTPUT_SAMPLE_RATE)
                        {
                            tracing::error!("Audio playback failed: {e}");
                        }
                    }
                    Ok(GeminiEvent::ToolCall { id, name, args }) => {
                        tracing::info!(name = %name, "Tool call received");

                        let response = if let Some(action) = function_call_to_action(&name, &args) {
                            #[cfg(target_os = "macos")]
                            {
                                let action_desc = format!("{action:?}");
                                let result = executor.execute(&action).await;
                                if result.success {
                                    let _ = bus.send(AuraEvent::ActionExecuted {
                                        description: result.description.clone(),
                                    });
                                } else {
                                    let _ = bus.send(AuraEvent::ActionFailed {
                                        description: action_desc,
                                        error: result.description.clone(),
                                    });
                                }
                                serde_json::json!({
                                    "success": result.success,
                                    "description": result.description,
                                    "data": result.data,
                                })
                            }
                            #[cfg(not(target_os = "macos"))]
                            {
                                let _ = bus.send(AuraEvent::ActionFailed {
                                    description: format!("{action:?}"),
                                    error: "Platform not supported".into(),
                                });
                                serde_json::json!({
                                    "success": false,
                                    "error": "Platform not supported",
                                })
                            }
                        } else if name == "summarize_screen" {
                            // TODO: implement screen capture + send to Gemini
                            let _ = bus.send(AuraEvent::ActionFailed {
                                description: "Summarize screen".into(),
                                error: "Screen summarization not yet implemented".into(),
                            });
                            serde_json::json!({
                                "success": false,
                                "error": "Screen summarization not yet implemented",
                            })
                        } else {
                            let error = format!("Unknown function: {name}");
                            let _ = bus.send(AuraEvent::ActionFailed {
                                description: format!("Tool call: {name}"),
                                error: error.clone(),
                            });
                            serde_json::json!({
                                "success": false,
                                "error": error,
                            })
                        };

                        if let Err(e) = session.send_tool_response(id, name, response).await {
                            tracing::error!("Failed to send tool response: {e}");
                        }
                    }
                    Ok(GeminiEvent::ToolCallCancellation { ids }) => {
                        tracing::info!(?ids, "Tool call(s) cancelled");
                    }
                    Ok(GeminiEvent::Interrupted) => {
                        tracing::info!("Gemini interrupted — stopping playback");
                        if let Some(ref p) = player {
                            p.stop();
                        }
                        let _ = bus.send(AuraEvent::BargeIn);
                    }
                    Ok(GeminiEvent::Transcription { text }) => {
                        let _ = bus.send(AuraEvent::AssistantSpeaking { text });
                    }
                    Ok(GeminiEvent::TurnComplete) => {
                        tracing::debug!("Turn complete");
                    }
                    Ok(GeminiEvent::Error { message }) => {
                        tracing::error!(%message, "Gemini error");
                        let _ = bus.send(AuraEvent::ActionFailed {
                            description: "Gemini session".into(),
                            error: message,
                        });
                    }
                    Ok(GeminiEvent::Reconnecting { attempt }) => {
                        tracing::warn!(attempt, "Gemini reconnecting");
                        let _ = bus.send(AuraEvent::GeminiReconnecting { attempt });
                    }
                    Ok(GeminiEvent::Disconnected) => {
                        tracing::info!("Gemini session disconnected");
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

/// Bridge daemon events to overlay messages via EventLoopProxy.
async fn run_overlay_bridge(
    bus: EventBus,
    proxy: EventLoopProxy<OverlayMessage>,
    cancel: CancellationToken,
) {
    let mut rx = bus.subscribe();

    loop {
        tokio::select! {
            _ = cancel.cancelled() => {
                let _ = proxy.send_event(OverlayMessage::Shutdown);
                break;
            }
            event = rx.recv() => {
                match event {
                    Ok(AuraEvent::ShowOverlay { content }) => {
                        let _ = proxy.send_event(OverlayMessage::Show);
                        let state = match content {
                            OverlayContent::Listening => OverlayState::Listening {
                                audio_levels: vec![0.5; 64],
                                phase: 0.0,
                                transition: 1.0,
                            },
                            OverlayContent::Processing => OverlayState::Processing {
                                phase: 0.0,
                                transition: 1.0,
                            },
                            OverlayContent::Response { text } => OverlayState::Response {
                                chars_revealed: text.len(),
                                text,
                                card_opacity: 1.0,
                            },
                            OverlayContent::Error { message } => OverlayState::Error {
                                message,
                                card_opacity: 1.0,
                                pulse_phase: 0.0,
                            },
                        };
                        let _ = proxy.send_event(OverlayMessage::SetState(state));
                    }
                    Ok(AuraEvent::HideOverlay) => {
                        let _ = proxy.send_event(OverlayMessage::Hide);
                    }
                    Ok(AuraEvent::AssistantSpeaking { text }) => {
                        let _ = proxy.send_event(OverlayMessage::Show);
                        let _ = proxy.send_event(OverlayMessage::SetState(OverlayState::Response {
                            chars_revealed: text.len(),
                            text,
                            card_opacity: 1.0,
                        }));
                    }
                    Ok(AuraEvent::BargeIn) => {
                        let _ = proxy.send_event(OverlayMessage::SetState(OverlayState::Listening {
                            audio_levels: vec![0.5; 64],
                            phase: 0.0,
                            transition: 1.0,
                        }));
                    }
                    Ok(AuraEvent::GeminiConnected) => {
                        tracing::info!("Overlay: Gemini connected");
                        let _ = proxy.send_event(OverlayMessage::Show);
                        let _ = proxy.send_event(OverlayMessage::SetState(OverlayState::Listening {
                            audio_levels: vec![0.3; 64],
                            phase: 0.0,
                            transition: 1.0,
                        }));
                    }
                    Ok(AuraEvent::GeminiReconnecting { attempt }) => {
                        tracing::warn!(attempt, "Overlay: Gemini reconnecting");
                        let _ = proxy.send_event(OverlayMessage::SetState(OverlayState::Error {
                            message: format!("Reconnecting to Gemini (attempt {attempt})..."),
                            card_opacity: 1.0,
                            pulse_phase: 0.0,
                        }));
                    }
                    Ok(AuraEvent::Shutdown) => {
                        let _ = proxy.send_event(OverlayMessage::Shutdown);
                        break;
                    }
                    Ok(_) => {}
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        tracing::warn!("Overlay bridge lagged by {n} events");
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                }
            }
        }
    }
}
