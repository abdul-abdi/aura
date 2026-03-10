use aura_daemon::bus::EventBus;
use aura_daemon::event::AuraEvent;

#[tokio::test]
async fn test_e2e_gemini_event_flow() {
    let bus = EventBus::new(64);
    let mut rx = bus.subscribe();

    // Gemini connected
    bus.send(AuraEvent::GeminiConnected).unwrap();

    // Assistant speaks (transcription from Gemini)
    bus.send(AuraEvent::AssistantSpeaking {
        text: "Hello! How can I help you?".into(),
    })
    .unwrap();

    // Tool executed (from tool call)
    bus.send(AuraEvent::ToolExecuted {
        name: "run_applescript".into(),
        success: true,
        output: "Opened Safari".into(),
    })
    .unwrap();

    let e1 = rx.recv().await.unwrap();
    assert!(matches!(e1, AuraEvent::GeminiConnected));

    let e2 = rx.recv().await.unwrap();
    assert!(matches!(e2, AuraEvent::AssistantSpeaking { .. }));

    let e3 = rx.recv().await.unwrap();
    assert!(matches!(e3, AuraEvent::ToolExecuted { .. }));
}

#[tokio::test]
async fn test_e2e_barge_in_event_flow() {
    let bus = EventBus::new(64);
    let mut rx = bus.subscribe();

    // Simulate barge-in: assistant speaking, then user interrupts
    bus.send(AuraEvent::AssistantSpeaking {
        text: "Let me tell you about...".into(),
    })
    .unwrap();
    bus.send(AuraEvent::BargeIn).unwrap();

    let e1 = rx.recv().await.unwrap();
    assert!(matches!(e1, AuraEvent::AssistantSpeaking { .. }));

    let e2 = rx.recv().await.unwrap();
    assert!(matches!(e2, AuraEvent::BargeIn));
}

#[tokio::test]
async fn test_e2e_gemini_reconnecting_flow() {
    let bus = EventBus::new(64);
    let mut rx = bus.subscribe();

    bus.send(AuraEvent::GeminiReconnecting { attempt: 1 })
        .unwrap();
    bus.send(AuraEvent::GeminiConnected).unwrap();

    let e1 = rx.recv().await.unwrap();
    assert!(matches!(e1, AuraEvent::GeminiReconnecting { attempt: 1 }));

    let e2 = rx.recv().await.unwrap();
    assert!(matches!(e2, AuraEvent::GeminiConnected));
}

#[tokio::test]
async fn test_e2e_tool_failure_flow() {
    let bus = EventBus::new(64);
    let mut rx = bus.subscribe();

    bus.send(AuraEvent::ToolExecuted {
        name: "run_applescript".into(),
        success: false,
        output: "Script execution failed".into(),
    })
    .unwrap();

    let e1 = rx.recv().await.unwrap();
    assert!(matches!(
        e1,
        AuraEvent::ToolExecuted { success: false, .. }
    ));
}
