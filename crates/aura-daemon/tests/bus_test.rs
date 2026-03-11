use aura_daemon::bus::EventBus;
use aura_daemon::event::AuraEvent;

#[tokio::test]
async fn test_event_bus_send_receive() {
    let bus = EventBus::new(16);
    let mut rx = bus.subscribe();

    bus.send(AuraEvent::Shutdown);

    let event = rx.recv().await.unwrap();
    assert!(matches!(event, AuraEvent::Shutdown));
}

#[tokio::test]
async fn test_event_bus_multiple_subscribers() {
    let bus = EventBus::new(16);
    let mut rx1 = bus.subscribe();
    let mut rx2 = bus.subscribe();

    bus.send(AuraEvent::ToolExecuted {
        name: "run_applescript".into(),
        success: true,
        output: "done".into(),
    });

    let e1 = rx1.recv().await.unwrap();
    let e2 = rx2.recv().await.unwrap();
    assert!(matches!(e1, AuraEvent::ToolExecuted { .. }));
    assert!(matches!(e2, AuraEvent::ToolExecuted { .. }));
}
