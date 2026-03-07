use crate::bus::EventBus;
use crate::event::{AuraEvent, OverlayContent};
use anyhow::Result;
use std::time::Duration;

/// Delay before auto-hiding the overlay after a successful action.
const OVERLAY_AUTO_HIDE_DELAY: Duration = Duration::from_secs(3);

pub struct Daemon {
    bus: EventBus,
}

impl Daemon {
    pub fn new(bus: EventBus) -> Self {
        Self { bus }
    }

    pub async fn run(&self) -> Result<()> {
        tracing::info!("Aura daemon running");

        let mut rx = self.bus.subscribe();
        let bus = self.bus.clone();

        loop {
            tokio::select! {
                event = rx.recv() => {
                    match event {
                        Ok(AuraEvent::Shutdown) => {
                            tracing::info!("Shutdown signal received");
                            break;
                        }
                        Ok(AuraEvent::WakeWordDetected) => {
                            tracing::info!("Wake word detected — listening");
                            let _ = bus.send(AuraEvent::ShowOverlay {
                                content: OverlayContent::Listening,
                            }).await;
                        }
                        Ok(AuraEvent::VoiceCommand { text }) => {
                            tracing::info!(command = %text, "Voice command received");
                            let _ = bus.send(AuraEvent::ShowOverlay {
                                content: OverlayContent::Processing,
                            }).await;
                            // Intent parsing will happen here
                        }
                        Ok(AuraEvent::IntentParsed { intent }) => {
                            tracing::info!(?intent, "Intent parsed");
                            // Action execution will happen here
                        }
                        Ok(AuraEvent::ActionExecuted { description }) => {
                            tracing::info!(%description, "Action executed");
                            let _ = bus.send(AuraEvent::ShowOverlay {
                                content: OverlayContent::Response {
                                    text: description,
                                },
                            }).await;
                            // Auto-hide after delay
                            let hide_bus = bus.clone();
                            tokio::spawn(async move {
                                tokio::time::sleep(OVERLAY_AUTO_HIDE_DELAY).await;
                                let _ = hide_bus.send(AuraEvent::HideOverlay).await;
                            });
                        }
                        Ok(AuraEvent::ActionFailed { description, error }) => {
                            tracing::warn!(%description, %error, "Action failed");
                            let _ = bus.send(AuraEvent::ShowOverlay {
                                content: OverlayContent::Error {
                                    message: error,
                                },
                            }).await;
                        }
                        Ok(event) => {
                            tracing::debug!(?event, "Unhandled event");
                        }
                        Err(e) => {
                            tracing::warn!("Event bus error: {e}");
                        }
                    }
                }
                _ = tokio::signal::ctrl_c() => {
                    tracing::info!("Ctrl+C received, shutting down");
                    let _ = bus.send(AuraEvent::Shutdown).await;
                    break;
                }
            }
        }

        Ok(())
    }
}
