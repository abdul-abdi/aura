use aura_daemon::bus::EventBus;
use aura_daemon::daemon::Daemon;
use aura_daemon::event::AuraEvent;
use std::time::Duration;

#[tokio::test]
async fn test_daemon_starts_and_shuts_down() {
    let bus = EventBus::new(64);
    let daemon = Daemon::new(bus.clone());

    let handle = tokio::spawn(async move {
        daemon.run().await.unwrap();
    });

    // Give it time to start
    tokio::time::sleep(Duration::from_millis(50)).await;

    // Send shutdown
    bus.send(AuraEvent::Shutdown);

    // Should exit cleanly
    let result = tokio::time::timeout(Duration::from_secs(2), handle).await;
    assert!(result.is_ok(), "Daemon should shut down within 2 seconds");
}
