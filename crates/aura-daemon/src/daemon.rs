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

        let ctrl_c = tokio::signal::ctrl_c();
        tokio::pin!(ctrl_c);

        loop {
            tokio::select! {
                event = rx.recv() => {
                    match event {
                        Ok(AuraEvent::Shutdown) => {
                            tracing::info!("Shutdown signal received");
                            break;
                        }
                        Ok(event) => {
                            tracing::debug!(?event, "Event received");
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                            tracing::warn!("Event bus lagged, skipped {n} events");
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                            tracing::error!("Event bus closed unexpectedly");
                            break;
                        }
                    }
                }
                _ = &mut ctrl_c => {
                    tracing::info!("Ctrl+C received, shutting down");
                    break;
                }
            }
        }

        Ok(())
    }
}
