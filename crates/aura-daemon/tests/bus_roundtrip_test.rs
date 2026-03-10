use aura_daemon::bus::EventBus;
use aura_daemon::event::AuraEvent;

#[tokio::test]
async fn test_bus_roundtrip_gemini_event_flow() {
    let bus = EventBus::new(64);
    let mut rx = bus.subscribe();

    // Gemini connected
    bus.send(AuraEvent::GeminiConnected);

    // Tool executed (from tool call)
    bus.send(AuraEvent::ToolExecuted {
        name: "run_applescript".into(),
        success: true,
        output: "Opened Safari".into(),
    });

    let e1 = rx.recv().await.unwrap();
    assert!(matches!(e1, AuraEvent::GeminiConnected));

    let e2 = rx.recv().await.unwrap();
    assert!(matches!(e2, AuraEvent::ToolExecuted { .. }));
}

#[tokio::test]
async fn test_bus_roundtrip_barge_in_event_flow() {
    let bus = EventBus::new(64);
    let mut rx = bus.subscribe();

    // Simulate barge-in
    bus.send(AuraEvent::BargeIn);

    let e1 = rx.recv().await.unwrap();
    assert!(matches!(e1, AuraEvent::BargeIn));
}

#[tokio::test]
async fn test_bus_roundtrip_gemini_reconnecting_flow() {
    let bus = EventBus::new(64);
    let mut rx = bus.subscribe();

    bus.send(AuraEvent::GeminiReconnecting { attempt: 1 });
    bus.send(AuraEvent::GeminiConnected);

    let e1 = rx.recv().await.unwrap();
    assert!(matches!(e1, AuraEvent::GeminiReconnecting { attempt: 1 }));

    let e2 = rx.recv().await.unwrap();
    assert!(matches!(e2, AuraEvent::GeminiConnected));
}

#[tokio::test]
async fn test_bus_roundtrip_tool_failure_flow() {
    let bus = EventBus::new(64);
    let mut rx = bus.subscribe();

    bus.send(AuraEvent::ToolExecuted {
        name: "run_applescript".into(),
        success: false,
        output: "Script execution failed".into(),
    });

    let e1 = rx.recv().await.unwrap();
    assert!(matches!(e1, AuraEvent::ToolExecuted { success: false, .. }));
}
