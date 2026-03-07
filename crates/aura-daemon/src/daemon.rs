use crate::bus::EventBus;
use crate::event::{AuraEvent, OverlayContent};
use anyhow::Result;
use std::time::Duration;
use tokio::task::AbortHandle;

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
        let mut auto_hide_handle: Option<AbortHandle> = None;

        loop {
            tokio::select! {
                event = rx.recv() => {
                    match event {
                        Ok(AuraEvent::Shutdown) => {
                            tracing::info!("Shutdown signal received");
                            if let Some(h) = auto_hide_handle.take() { h.abort(); }
                            break;
                        }
                        Ok(AuraEvent::WakeWordDetected) => {
                            tracing::info!("Wake word detected — listening");
                            if let Some(h) = auto_hide_handle.take() { h.abort(); }
                            send_event(&bus, AuraEvent::ShowOverlay {
                                content: OverlayContent::Listening,
                            }).await;
                        }
                        Ok(AuraEvent::VoiceCommand { text }) => {
                            tracing::info!(command = %text, "Voice command received");
                            if let Some(h) = auto_hide_handle.take() { h.abort(); }
                            send_event(&bus, AuraEvent::ShowOverlay {
                                content: OverlayContent::Processing,
                            }).await;
                        }
                        Ok(AuraEvent::IntentParsed { intent }) => {
                            tracing::info!(?intent, "Intent parsed");
                        }
                        Ok(AuraEvent::ActionExecuted { description }) => {
                            tracing::info!(%description, "Action executed");
                            send_event(&bus, AuraEvent::ShowOverlay {
                                content: OverlayContent::Response {
                                    text: description,
                                },
                            }).await;
                            if let Some(h) = auto_hide_handle.take() { h.abort(); }
                            let hide_bus = bus.clone();
                            auto_hide_handle = Some(tokio::spawn(async move {
                                tokio::time::sleep(OVERLAY_AUTO_HIDE_DELAY).await;
                                send_event(&hide_bus, AuraEvent::HideOverlay).await;
                            }).abort_handle());
                        }
                        Ok(AuraEvent::ActionFailed { description, error }) => {
                            tracing::warn!(%description, %error, "Action failed");
                            send_event(&bus, AuraEvent::ShowOverlay {
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
                    if let Some(h) = auto_hide_handle.take() { h.abort(); }
                    send_event(&bus, AuraEvent::Shutdown).await;
                    break;
                }
            }
        }

        Ok(())
    }
}

async fn send_event(bus: &EventBus, event: AuraEvent) {
    if let Err(e) = bus.send(event).await {
        tracing::warn!("Failed to send event: {e}");
    }
}
