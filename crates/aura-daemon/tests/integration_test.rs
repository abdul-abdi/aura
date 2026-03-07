use std::time::Duration;

use aura_bridge::actions::{ActionExecutor, MockExecutor};
use aura_bridge::mapper::intent_to_action;
use aura_daemon::bus::EventBus;
use aura_daemon::daemon::Daemon;
use aura_daemon::event::{AuraEvent, OverlayContent};
use aura_llm::intent::IntentParser;
use aura_llm::provider::MockProvider;

#[tokio::test]
async fn test_full_voice_to_action_flow() {
    // Simulate the full pipeline: voice command -> intent -> action -> result
    let bus = EventBus::new(64);
    let mut rx = bus.subscribe();

    // Create mock LLM that returns "open Safari" intent
    let provider = MockProvider::new(vec![(
        "open safari",
        r#"{"type":"open_app","name":"Safari"}"#,
    )]);
    let parser = IntentParser::new(Box::new(provider));
    let executor = MockExecutor::new();

    // Parse intent from voice command
    let intent = parser.parse("open safari").await.unwrap();
    bus.send(AuraEvent::IntentParsed {
        intent: intent.clone(),
    })
    .unwrap();

    // Map and execute
    let action = intent_to_action(&intent).unwrap();
    let result = executor.execute(&action).await;
    assert!(result.success);

    bus.send(AuraEvent::ActionExecuted {
        description: result.description,
    })
    .unwrap();

    // Verify events were published
    let event1 = rx.recv().await.unwrap();
    assert!(matches!(event1, AuraEvent::IntentParsed { .. }));

    let event2 = rx.recv().await.unwrap();
    assert!(matches!(event2, AuraEvent::ActionExecuted { .. }));
}

#[tokio::test]
async fn test_daemon_processes_voice_command() {
    let bus = EventBus::new(64);
    let daemon = Daemon::new(bus.clone());

    // Subscribe to get daemon's responses
    let mut rx = bus.subscribe();

    let handle = tokio::spawn(async move {
        daemon.run().await.unwrap();
    });

    // Give daemon time to start
    tokio::time::sleep(Duration::from_millis(20)).await;

    // Send a voice command -- daemon should emit ShowOverlay(Processing)
    bus.send(AuraEvent::VoiceCommand {
        text: "open safari".into(),
    })
    .unwrap();

    // Collect events
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

    // Should have received the VoiceCommand and a ShowOverlay(Processing)
    assert!(
        events.iter().any(|e| matches!(e, AuraEvent::ShowOverlay {
            content: OverlayContent::Processing
        })),
        "Daemon should show processing overlay on voice command. Got: {events:?}"
    );

    // Shutdown
    bus.send(AuraEvent::Shutdown).unwrap();
    let _ = tokio::time::timeout(Duration::from_secs(2), handle).await;
}

#[tokio::test]
async fn test_daemon_shows_response_on_action_executed() {
    let bus = EventBus::new(64);
    let daemon = Daemon::new(bus.clone());
    let mut rx = bus.subscribe();

    let handle = tokio::spawn(async move {
        daemon.run().await.unwrap();
    });

    tokio::time::sleep(Duration::from_millis(20)).await;

    bus.send(AuraEvent::ActionExecuted {
        description: "Opened Safari".into(),
    })
    .unwrap();

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

    assert!(
        events.iter().any(|e| matches!(
            e,
            AuraEvent::ShowOverlay {
                content: OverlayContent::Response { .. }
            }
        )),
        "Daemon should show response overlay. Got: {events:?}"
    );

    bus.send(AuraEvent::Shutdown).unwrap();
    let _ = tokio::time::timeout(Duration::from_secs(2), handle).await;
}

#[tokio::test]
async fn test_daemon_shows_error_on_action_failed() {
    let bus = EventBus::new(64);
    let daemon = Daemon::new(bus.clone());
    let mut rx = bus.subscribe();

    let handle = tokio::spawn(async move {
        daemon.run().await.unwrap();
    });

    tokio::time::sleep(Duration::from_millis(20)).await;

    bus.send(AuraEvent::ActionFailed {
        description: "open app".into(),
        error: "App not found".into(),
    })
    .unwrap();

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

    assert!(
        events.iter().any(|e| matches!(
            e,
            AuraEvent::ShowOverlay {
                content: OverlayContent::Error { .. }
            }
        )),
        "Daemon should show error overlay. Got: {events:?}"
    );

    bus.send(AuraEvent::Shutdown).unwrap();
    let _ = tokio::time::timeout(Duration::from_secs(2), handle).await;
}

#[tokio::test]
async fn test_unknown_intent_parse() {
    let provider = MockProvider::new(vec![]);
    let parser = IntentParser::new(Box::new(provider));
    let intent = parser.parse("gibberish command").await.unwrap();
    assert!(matches!(intent, aura_llm::intent::Intent::Unknown { .. }));
}
