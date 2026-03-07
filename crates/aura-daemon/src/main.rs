use anyhow::{Context, Result};
use clap::Parser;
use tokio_util::sync::CancellationToken;
use winit::event_loop::EventLoopProxy;

use aura_bridge::actions::ActionExecutor;
use aura_bridge::mapper::intent_to_action;
use aura_daemon::bus::EventBus;
use aura_daemon::daemon::Daemon;
use aura_daemon::event::{AuraEvent, OverlayContent};
use aura_daemon::setup::AuraSetup;
use aura_llm::intent::{Intent, IntentParser};
use aura_llm::ollama::{OllamaConfig, OllamaProvider};
use aura_overlay::renderer::OverlayState;
use aura_overlay::window::{create_event_loop, OverlayMessage, OverlayWindow};
use aura_voice::pipeline::{run_voice_task, VoiceEvent};
use aura_voice::stt::SttConfig;
use aura_voice::vad::VadConfig;
use tracing_subscriber::EnvFilter;

const EVENT_BUS_CAPACITY: usize = 64;

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

    // First-run setup
    let setup = AuraSetup::new(AuraSetup::default_data_dir());
    setup.ensure_dirs()?;
    setup.print_status();

    let bus = EventBus::new(EVENT_BUS_CAPACITY);
    let cancel = CancellationToken::new();

    if cli.no_overlay {
        // No overlay — run tokio directly
        let rt = tokio::runtime::Runtime::new()?;
        rt.block_on(run_daemon(bus, cancel))?;
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

                if let Err(e) = run_daemon(bg_bus, bg_cancel).await {
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

async fn run_daemon(bus: EventBus, cancel: CancellationToken) -> Result<()> {
    // Start voice pipeline on a dedicated thread (cpal's Stream is !Send)
    let voice_cancel = cancel.clone();
    let (voice_tx, _) = tokio::sync::broadcast::channel::<VoiceEvent>(64);
    let voice_tx2 = voice_tx.clone();

    let voice_handle = std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("Failed to create voice runtime");
        rt.block_on(async move {
            if let Err(e) = run_voice_task(
                SttConfig::default(),
                VadConfig::default(),
                voice_tx2,
                voice_cancel,
            )
            .await
            {
                tracing::error!("Voice pipeline error: {e}");
            }
        });
    });

    // Start processor task (voice events -> intent -> action -> bus events)
    let proc_bus = bus.clone();
    let proc_cancel = cancel.clone();
    let mut voice_rx = voice_tx.subscribe();

    let processor_handle = tokio::spawn(async move {
        if let Err(e) = run_processor(proc_bus, &mut voice_rx, proc_cancel).await {
            tracing::error!("Processor error: {e}");
        }
    });

    // Run the daemon event loop
    let daemon = Daemon::new(bus);
    daemon.run().await?;

    // Shutdown
    cancel.cancel();
    let _ = processor_handle.await;
    let _ = voice_handle.join();

    Ok(())
}

async fn run_processor(
    bus: EventBus,
    voice_rx: &mut tokio::sync::broadcast::Receiver<VoiceEvent>,
    cancel: CancellationToken,
) -> Result<()> {
    let provider =
        OllamaProvider::new(OllamaConfig::default()).context("Failed to create Ollama provider")?;
    let parser = IntentParser::new(Box::new(provider));

    #[cfg(target_os = "macos")]
    let executor = aura_bridge::macos::MacOSExecutor::new();

    tracing::info!("Processor task running");

    loop {
        tokio::select! {
            _ = cancel.cancelled() => break,
            event = voice_rx.recv() => {
                match event {
                    Ok(VoiceEvent::ListeningStarted) => {
                        let _ = bus.send(AuraEvent::WakeWordDetected);
                    }
                    Ok(VoiceEvent::Transcription { text }) => {
                        let _ = bus.send(AuraEvent::VoiceCommand { text: text.clone() });

                        // Parse intent
                        match parser.parse(&text).await {
                            Ok(intent) => {
                                let _ = bus.send(AuraEvent::IntentParsed { intent: intent.clone() });

                                // Map intent to action and execute
                                if let Some(action) = intent_to_action(&intent) {
                                    #[cfg(target_os = "macos")]
                                    {
                                        let result = executor.execute(&action).await;
                                        if result.success {
                                            let _ = bus.send(AuraEvent::ActionExecuted {
                                                description: result.description,
                                            });
                                        } else {
                                            let _ = bus.send(AuraEvent::ActionFailed {
                                                description: result.description.clone(),
                                                error: result.description,
                                            });
                                        }
                                    }
                                    #[cfg(not(target_os = "macos"))]
                                    {
                                        let _ = bus.send(AuraEvent::ActionFailed {
                                            description: format!("{action:?}"),
                                            error: "Platform not supported".into(),
                                        });
                                    }
                                } else {
                                    match intent {
                                        Intent::Unknown { raw } => {
                                            let _ = bus.send(AuraEvent::ActionFailed {
                                                description: "Unknown command".into(),
                                                error: format!("Could not understand: {raw}"),
                                            });
                                        }
                                        Intent::SummarizeScreen => {
                                            let _ = bus.send(AuraEvent::ActionFailed {
                                                description: "Summarize screen".into(),
                                                error: "Screen summarization not yet implemented".into(),
                                            });
                                        }
                                        _ => {}
                                    }
                                }
                            }
                            Err(e) => {
                                tracing::error!("Intent parsing failed: {e}");
                                let _ = bus.send(AuraEvent::ActionFailed {
                                    description: "Intent parsing".into(),
                                    error: format!("Failed to parse intent: {e}"),
                                });
                            }
                        }
                    }
                    Ok(VoiceEvent::ListeningStopped) => {
                        let _ = bus.send(AuraEvent::ListeningStopped);
                    }
                    Ok(VoiceEvent::Error { message }) => {
                        let _ = bus.send(AuraEvent::ActionFailed {
                            description: "Voice pipeline".into(),
                            error: message,
                        });
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        tracing::warn!("Processor lagged by {n} voice events");
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
