use aura_bridge::mapper::intent_to_action;
use aura_daemon::bus::EventBus;
use aura_daemon::event::AuraEvent;
use aura_llm::conversation::Conversation;
use aura_llm::intent::IntentParser;
use aura_llm::ollama::{OllamaConfig, OllamaProvider};
use aura_llm::provider::LlmProvider;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn mock_chat_response(content: &str) -> serde_json::Value {
    serde_json::json!({
        "model": "qwen3.5:4b",
        "message": { "role": "assistant", "content": content },
        "done": true
    })
}

#[tokio::test]
async fn test_e2e_voice_command_to_action_with_mock_ollama() {
    let mock_server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/api/chat"))
        .respond_with(ResponseTemplate::new(200).set_body_json(
            mock_chat_response(r#"{"type":"open_app","name":"Safari"}"#),
        ))
        .mount(&mock_server)
        .await;

    let config = OllamaConfig {
        base_url: mock_server.uri(),
        model: "qwen3.5:4b".into(),
        timeout_secs: 5,
    };
    let provider = OllamaProvider::new(config).unwrap();

    let parser = IntentParser::new(Box::new(provider));
    let intent = parser.parse("open safari").await.unwrap();

    assert!(
        matches!(&intent, aura_llm::intent::Intent::OpenApp { name } if name == "Safari"),
        "Expected OpenApp(Safari), got: {intent:?}"
    );

    let action = intent_to_action(&intent);
    assert!(action.is_some(), "OpenApp intent should map to an action");

    let bus = EventBus::new(64);
    let mut rx = bus.subscribe();

    bus.send(AuraEvent::VoiceCommand {
        text: "open safari".into(),
    })
    .unwrap();
    bus.send(AuraEvent::IntentParsed {
        intent: intent.clone(),
    })
    .unwrap();
    bus.send(AuraEvent::ActionExecuted {
        description: "Opened Safari".into(),
    })
    .unwrap();

    let e1 = rx.recv().await.unwrap();
    assert!(matches!(e1, AuraEvent::VoiceCommand { .. }));

    let e2 = rx.recv().await.unwrap();
    assert!(matches!(e2, AuraEvent::IntentParsed { .. }));

    let e3 = rx.recv().await.unwrap();
    assert!(matches!(e3, AuraEvent::ActionExecuted { .. }));
}

#[tokio::test]
async fn test_e2e_ollama_returns_unknown_intent() {
    let mock_server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/api/chat"))
        .respond_with(ResponseTemplate::new(200).set_body_json(
            mock_chat_response(r#"{"type":"unknown","raw":"gibberish"}"#),
        ))
        .mount(&mock_server)
        .await;

    let config = OllamaConfig {
        base_url: mock_server.uri(),
        model: "qwen3.5:4b".into(),
        timeout_secs: 5,
    };
    let provider = OllamaProvider::new(config).unwrap();
    let parser = IntentParser::new(Box::new(provider));

    let intent = parser.parse("asdfjkl").await.unwrap();
    assert!(matches!(intent, aura_llm::intent::Intent::Unknown { .. }));

    assert!(intent_to_action(&intent).is_none());
}

#[tokio::test]
async fn test_e2e_ollama_server_error_is_handled() {
    let mock_server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/api/chat"))
        .respond_with(ResponseTemplate::new(500).set_body_string("Internal Server Error"))
        .mount(&mock_server)
        .await;

    let config = OllamaConfig {
        base_url: mock_server.uri(),
        model: "qwen3.5:4b".into(),
        timeout_secs: 5,
    };
    let provider = OllamaProvider::new(config).unwrap();

    let result = provider.complete("test").await;
    assert!(result.is_err(), "Should fail on 500 error");
}

#[tokio::test]
async fn test_e2e_ollama_malformed_response() {
    let mock_server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/api/chat"))
        .respond_with(ResponseTemplate::new(200).set_body_json(
            mock_chat_response("this is not valid json at all"),
        ))
        .mount(&mock_server)
        .await;

    let config = OllamaConfig {
        base_url: mock_server.uri(),
        model: "qwen3.5:4b".into(),
        timeout_secs: 5,
    };
    let provider = OllamaProvider::new(config).unwrap();
    let parser = IntentParser::new(Box::new(provider));

    let intent = parser.parse("test").await.unwrap();
    assert!(
        matches!(intent, aura_llm::intent::Intent::Unknown { .. }),
        "Malformed LLM response should fall back to Unknown. Got: {intent:?}"
    );
}

#[tokio::test]
async fn test_e2e_conversational_response() {
    let mock_server = MockServer::start().await;

    // First call: intent parser returns Unknown (not a command)
    // Second call: conversation chat returns a response
    Mock::given(method("POST"))
        .and(path("/api/chat"))
        .respond_with(ResponseTemplate::new(200).set_body_json(
            mock_chat_response(r#"{"type":"unknown","raw":"hello there"}"#),
        ))
        .up_to_n_times(1)
        .expect(1)
        .mount(&mock_server)
        .await;

    Mock::given(method("POST"))
        .and(path("/api/chat"))
        .respond_with(ResponseTemplate::new(200).set_body_json(
            mock_chat_response("Hello! I'm Aura, your local AI assistant. How can I help you today?"),
        ))
        .mount(&mock_server)
        .await;

    let config = OllamaConfig {
        base_url: mock_server.uri(),
        model: "qwen3.5:4b".into(),
        timeout_secs: 5,
    };

    // Test intent parsing routes to Unknown
    let intent_provider = OllamaProvider::new(config.clone()).unwrap();
    let parser = IntentParser::new(Box::new(intent_provider));
    let intent = parser.parse("hello there").await.unwrap();
    assert!(
        matches!(intent, aura_llm::intent::Intent::Unknown { .. }),
        "Casual speech should parse as Unknown. Got: {intent:?}"
    );
    assert!(intent_to_action(&intent).is_none(), "Unknown intent should not map to an action");

    // Test conversation responds
    let conv_provider = OllamaProvider::new(config).unwrap();
    let conversation = Conversation::new(Box::new(conv_provider));
    let response = conversation.chat("hello there").await.unwrap();
    assert!(!response.is_empty(), "Conversation should return a non-empty response");
    assert!(response.contains("Aura"), "Response should mention Aura: {response}");
}

#[tokio::test]
async fn test_e2e_conversation_history_persists() {
    let mock_server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/api/chat"))
        .respond_with(ResponseTemplate::new(200).set_body_json(
            mock_chat_response("I remember our conversation!"),
        ))
        .expect(2)
        .mount(&mock_server)
        .await;

    let config = OllamaConfig {
        base_url: mock_server.uri(),
        model: "qwen3.5:4b".into(),
        timeout_secs: 5,
    };

    let provider = OllamaProvider::new(config).unwrap();
    let conversation = Conversation::new(Box::new(provider));

    // Two sequential chats should build history
    let _r1 = conversation.chat("first message").await.unwrap();
    let r2 = conversation.chat("second message").await.unwrap();
    assert!(!r2.is_empty());

    // Clear and verify it doesn't panic
    conversation.clear_history();
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
    bus.send(AuraEvent::WakeWordDetected).unwrap();

    let e1 = rx.recv().await.unwrap();
    assert!(matches!(e1, AuraEvent::AssistantSpeaking { .. }));

    let e2 = rx.recv().await.unwrap();
    assert!(matches!(e2, AuraEvent::BargeIn));

    let e3 = rx.recv().await.unwrap();
    assert!(matches!(e3, AuraEvent::WakeWordDetected));
}

#[tokio::test]
async fn test_e2e_health_check_with_mock() {
    let mock_server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/api/tags"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "models": [
                {"name": "qwen3.5:4b", "size": 2500000000_u64}
            ]
        })))
        .mount(&mock_server)
        .await;

    let config = OllamaConfig {
        base_url: mock_server.uri(),
        model: "qwen3.5:4b".into(),
        timeout_secs: 5,
    };
    let provider = OllamaProvider::new(config).unwrap();
    let result = provider.health_check().await;
    assert!(
        result.is_ok(),
        "Health check should pass with mock: {result:?}"
    );
}
