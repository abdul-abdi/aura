use std::time::Duration;

use aura_daemon::bus::EventBus;
use aura_daemon::daemon::Daemon;
use aura_daemon::event::AuraEvent;

#[tokio::test]
async fn test_daemon_shuts_down_on_shutdown_event() {
    let bus = EventBus::new(64);
    let daemon = Daemon::new(bus.clone());

    let handle = tokio::spawn(async move {
        daemon.run().await.unwrap();
    });

    tokio::time::sleep(Duration::from_millis(20)).await;

    bus.send(AuraEvent::Shutdown);
    let result = tokio::time::timeout(Duration::from_secs(2), handle).await;
    assert!(result.is_ok(), "Daemon should shut down within 2 seconds");
}

#[tokio::test]
async fn test_daemon_processes_tool_executed_event() {
    let bus = EventBus::new(64);
    let daemon = Daemon::new(bus.clone());
    let mut rx = bus.subscribe();

    let handle = tokio::spawn(async move {
        daemon.run().await.unwrap();
    });

    tokio::time::sleep(Duration::from_millis(20)).await;

    // Send a ToolExecuted event — daemon should log it without crashing
    bus.send(AuraEvent::ToolExecuted {
        name: "run_applescript".into(),
        success: true,
        output: "Opened Safari".into(),
    });

    // Collect events briefly
    let mut events = Vec::new();
    let timeout = tokio::time::sleep(Duration::from_millis(100));
    tokio::pin!(timeout);

    loop {
        tokio::select! {
            _ = &mut timeout => break,
            event = rx.recv() => {
                if let Ok(event) = event {
                    events.push(event);
                }
            }
        }
    }

    // The ToolExecuted event should have been broadcast
    assert!(
        events.iter().any(
            |e| matches!(e, AuraEvent::ToolExecuted { name, .. } if name == "run_applescript")
        ),
        "Should receive ToolExecuted event. Got: {events:?}"
    );

    bus.send(AuraEvent::Shutdown);
    let _ = tokio::time::timeout(Duration::from_secs(2), handle).await;
}

