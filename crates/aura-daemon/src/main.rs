use anyhow::Result;
use aura_daemon::bus::EventBus;
use aura_daemon::daemon::Daemon;
use tracing_subscriber::EnvFilter;

const EVENT_BUS_CAPACITY: usize = 64;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    tracing::info!("Aura daemon starting...");

    let bus = EventBus::new(EVENT_BUS_CAPACITY);
    let daemon = Daemon::new(bus);
    daemon.run().await?;

    tracing::info!("Aura daemon shut down.");
    Ok(())
}
