use crate::bus::EventBus;
use crate::event::AuraEvent;
use anyhow::Result;

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
                        }
                        Ok(AuraEvent::ToolExecuted { name, success, output }) => {
                            tracing::info!(%name, %success, "Tool executed");
                            tracing::debug!(%output, "Tool output");
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
                    send_event(&bus, AuraEvent::Shutdown);
                    break;
                }
            }
        }

        Ok(())
    }
}

fn send_event(bus: &EventBus, event: AuraEvent) {
    if let Err(e) = bus.send(event) {
        tracing::warn!("Failed to send event: {e}");
    }
}
