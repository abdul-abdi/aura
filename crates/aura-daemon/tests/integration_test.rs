use std::time::Duration;

use aura_daemon::bus::EventBus;
use aura_daemon::daemon::Daemon;
use aura_daemon::event::{AuraEvent, OverlayContent};

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
        events.iter().any(|e| matches!(
            e,
            AuraEvent::ShowOverlay {
                content: OverlayContent::Processing
            }
        )),
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
async fn test_gemini_tool_call_to_action_mapping() {
    use aura_gemini::tools::function_call_to_action;
    use serde_json::json;

    // open_app maps correctly
    let action = function_call_to_action("open_app", &json!({"app_name": "Safari"}));
    assert!(action.is_some());

    // search_files maps correctly
    let action = function_call_to_action("search_files", &json!({"query": "readme"}));
    assert!(action.is_some());

    // summarize_screen returns None (handled specially)
    let action = function_call_to_action("summarize_screen", &json!({}));
    assert!(action.is_none());

    // unknown function returns None
    let action = function_call_to_action("unknown_func", &json!({}));
    assert!(action.is_none());
}
