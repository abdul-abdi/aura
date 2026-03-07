use anyhow::Result;
use aura_daemon::bus;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    tracing::info!("Aura daemon starting...");

    let _bus = bus::EventBus::new(64);

    tracing::info!("Aura daemon shutting down.");
    Ok(())
}
