use crate::event::AuraEvent;
use tokio::sync::broadcast;

/// Event bus for distributing [`AuraEvent`]s to multiple consumers.
///
/// Uses `tokio::sync::broadcast` because the orchestrator, processor, and other
/// subscribers each receive every event independently. An `mpsc` channel would only
/// deliver each event to a single consumer, which would break the fan-out requirement.
#[derive(Clone)]
pub struct EventBus {
    tx: broadcast::Sender<AuraEvent>,
}

impl EventBus {
    pub fn new(capacity: usize) -> Self {
        let (tx, _) = broadcast::channel(capacity);
        Self { tx }
    }

    /// Send an event to all subscribers.
    ///
    /// Errors (e.g., no active receivers) are logged internally.
    /// Callers never need to handle send failures since the bus is
    /// best-effort — dropped events are acceptable.
    pub fn send(&self, event: AuraEvent) {
        if let Err(e) = self.tx.send(event) {
            tracing::debug!("EventBus send failed (no receivers): {e}");
        }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<AuraEvent> {
        self.tx.subscribe()
    }
}
