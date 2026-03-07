use crate::event::AuraEvent;
use anyhow::Result;
use tokio::sync::broadcast;

#[derive(Clone)]
pub struct EventBus {
    tx: broadcast::Sender<AuraEvent>,
}

impl EventBus {
    pub fn new(capacity: usize) -> Self {
        let (tx, _) = broadcast::channel(capacity);
        Self { tx }
    }

    pub async fn send(&self, event: AuraEvent) -> Result<()> {
        self.tx.send(event)?;
        Ok(())
    }

    pub fn subscribe(&self) -> broadcast::Receiver<AuraEvent> {
        self.tx.subscribe()
    }
}
