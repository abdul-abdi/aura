use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use futures_util::{SinkExt, StreamExt};
use serde_json::json;
use tokio::net::TcpListener;
use tokio::time::timeout;
use tokio_tungstenite::accept_async;
use tokio_tungstenite::tungstenite::Message;

use aura_gemini::config::GeminiConfig;
use aura_gemini::session::{GeminiEvent, GeminiLiveSession};

const TEST_TIMEOUT: Duration = Duration::from_secs(2);

fn test_config(url: String) -> GeminiConfig {
    GeminiConfig {
        api_key: "test-key".into(),
        model: "models/gemini-test".into(),
        voice: "Kore".into(),
        system_prompt: "Test prompt".into(),
        temperature: 0.7,
        proxy_url: Some(format!("{url}/")),
        proxy_auth_token: None,
        firestore_project_id: None,
        firebase_api_key: None,
        device_id: None,
        cloud_run_url: None,
        cloud_run_auth_token: None,
    }
}

/// Start a mock WebSocket server that accepts a single connection.
/// Returns the `ws://` URL to connect to.
///
/// Note: Only accepts a single WebSocket connection. If the client reconnects,
/// the second connection will not be served.
async fn start_mock_server<F, Fut>(handler: F) -> String
where
    F: FnOnce(tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>) -> Fut + Send + 'static,
    Fut: std::future::Future<Output = ()> + Send,
{
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let ws = accept_async(stream).await.unwrap();
        handler(ws).await;
    });
    format!("ws://{addr}")
}

/// Complete the setup handshake on the server side:
/// receive the client's setup message, then send setupComplete.
async fn complete_setup(ws: &mut tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>) {
    // Read setup message from client
    let msg = ws.next().await.unwrap().unwrap();
    let text = match msg {
        Message::Text(t) => t,
        other => panic!("Expected Text message for setup, got: {other:?}"),
    };
    // Verify it contains a setup field
    let val: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert!(
        val.get("setup").is_some(),
        "Expected setup message, got: {val}"
    );

    // Send setupComplete
    ws.send(Message::Text(r#"{"setupComplete":{}}"#.into()))
        .await
        .unwrap();
}

/// Wait for a specific event from the subscriber, with timeout.
async fn expect_event<F>(
    rx: &mut tokio::sync::broadcast::Receiver<GeminiEvent>,
    predicate: F,
    description: &str,
) -> GeminiEvent
where
    F: Fn(&GeminiEvent) -> bool,
{
    timeout(TEST_TIMEOUT, async {
        loop {
            match rx.recv().await {
                Ok(event) if predicate(&event) => return event,
                Ok(_) => continue, // skip non-matching events
                Err(e) => panic!("Channel error waiting for {description}: {e}"),
            }
        }
    })
    .await
    .unwrap_or_else(|_| panic!("Timed out waiting for {description}"))
}

#[tokio::test]
async fn test_session_connect_and_receive_audio() {
    // Build a small PCM audio chunk: 4 samples of 16-bit LE
    let samples_i16: Vec<i16> = vec![0, 16383, -16384, 8191];
    let mut pcm_bytes = Vec::new();
    for s in &samples_i16 {
        pcm_bytes.extend_from_slice(&s.to_le_bytes());
    }
    let audio_b64 = BASE64.encode(&pcm_bytes);

    let url = start_mock_server(move |mut ws| async move {
        complete_setup(&mut ws).await;

        // Send an audio response
        let audio_msg = json!({
            "serverContent": {
                "modelTurn": {
                    "parts": [{
                        "inlineData": {
                            "mimeType": "audio/pcm;rate=24000",
                            "data": audio_b64
                        }
                    }]
                }
            }
        });
        ws.send(Message::Text(audio_msg.to_string())).await.unwrap();

        // Keep connection open until client disconnects
        while ws.next().await.is_some() {}
    })
    .await;

    let session = GeminiLiveSession::connect(test_config(url), None)
        .await
        .unwrap();
    let mut rx = session.subscribe();

    // Should receive Connected
    expect_event(
        &mut rx,
        |e| matches!(e, GeminiEvent::Connected { .. }),
        "Connected",
    )
    .await;

    // Should receive AudioResponse
    let event = expect_event(
        &mut rx,
        |e| matches!(e, GeminiEvent::AudioResponse { .. }),
        "AudioResponse",
    )
    .await;

    let GeminiEvent::AudioResponse { samples } = event else {
        panic!("Expected AudioResponse, got: {event:?}");
    };
    assert_eq!(samples.len(), 4);
    // Verify decoded values are close to originals
    let expected: Vec<f32> = samples_i16.iter().map(|&s| s as f32 / 32768.0).collect();
    for (i, (got, want)) in samples.iter().zip(expected.iter()).enumerate() {
        assert!(
            (got - want).abs() < 0.001,
            "Sample {i}: got {got}, expected {want}"
        );
    }

    session.disconnect();
}

#[tokio::test]
async fn test_session_send_audio() {
    let (verify_tx, mut verify_rx) = tokio::sync::mpsc::channel::<serde_json::Value>(1);

    let url = start_mock_server(move |mut ws| async move {
        complete_setup(&mut ws).await;

        // Wait for client to send audio
        let msg = ws.next().await.unwrap().unwrap();
        if let Message::Text(text) = msg {
            let val: serde_json::Value = serde_json::from_str(&text).unwrap();
            verify_tx.send(val).await.unwrap();
        }
    })
    .await;

    let session = GeminiLiveSession::connect(test_config(url), None)
        .await
        .unwrap();
    let mut rx = session.subscribe();

    // Wait for connection
    expect_event(
        &mut rx,
        |e| matches!(e, GeminiEvent::Connected { .. }),
        "Connected",
    )
    .await;

    // Send audio
    let pcm = vec![0.0_f32, 0.5, -0.5, 1.0];
    session.send_audio(&pcm).unwrap();

    // Verify server received a realtimeInput message
    let received = timeout(TEST_TIMEOUT, verify_rx.recv())
        .await
        .expect("Timed out waiting for server to receive audio")
        .expect("Channel closed");

    assert!(
        received.get("realtimeInput").is_some(),
        "Expected realtimeInput message, got: {received}"
    );

    let audio = &received["realtimeInput"]["audio"];
    assert!(audio.is_object(), "Expected audio object, got: {audio}");
    assert_eq!(audio["mimeType"], "audio/pcm;rate=16000");

    let data = audio["data"].as_str().unwrap();
    let decoded_bytes = BASE64.decode(data).unwrap();
    // 4 f32 samples -> 4 i16 samples -> 8 bytes
    assert_eq!(decoded_bytes.len(), 8);

    session.disconnect();
}

#[tokio::test]
async fn test_session_tool_call_and_response() {
    let (verify_tx, mut verify_rx) = tokio::sync::mpsc::channel::<serde_json::Value>(1);

    let url = start_mock_server(move |mut ws| async move {
        complete_setup(&mut ws).await;

        // Send a tool call
        let tool_call_msg = json!({
            "toolCall": {
                "functionCalls": [{
                    "id": "call_1",
                    "name": "open_app",
                    "args": {"app_name": "Safari"}
                }]
            }
        });
        ws.send(Message::Text(tool_call_msg.to_string()))
            .await
            .unwrap();

        // Wait for client to send tool response
        let msg = ws.next().await.unwrap().unwrap();
        if let Message::Text(text) = msg {
            let val: serde_json::Value = serde_json::from_str(&text).unwrap();
            verify_tx.send(val).await.unwrap();
        }
    })
    .await;

    let session = GeminiLiveSession::connect(test_config(url), None)
        .await
        .unwrap();
    let mut rx = session.subscribe();

    expect_event(
        &mut rx,
        |e| matches!(e, GeminiEvent::Connected { .. }),
        "Connected",
    )
    .await;

    // Wait for tool call event
    let event = expect_event(
        &mut rx,
        |e| matches!(e, GeminiEvent::ToolCall { .. }),
        "ToolCall",
    )
    .await;

    let GeminiEvent::ToolCall { id, name, args } = event else {
        panic!("Expected ToolCall, got: {event:?}");
    };
    assert_eq!(id, "call_1");
    assert_eq!(name, "open_app");
    assert_eq!(args["app_name"], "Safari");

    // Send tool response
    session
        .send_tool_response(
            id,
            name,
            json!({"status": "success", "message": "Safari opened"}),
        )
        .await
        .unwrap();

    // Verify server received the tool response
    let received = timeout(TEST_TIMEOUT, verify_rx.recv())
        .await
        .expect("Timed out waiting for server to receive tool response")
        .expect("Channel closed");

    assert!(
        received.get("toolResponse").is_some(),
        "Expected toolResponse message, got: {received}"
    );

    let responses = &received["toolResponse"]["functionResponses"];
    assert_eq!(responses[0]["id"], "call_1");
    assert_eq!(responses[0]["name"], "open_app");
    assert_eq!(responses[0]["response"]["status"], "success");

    session.disconnect();
}

#[tokio::test]
async fn test_session_interrupted() {
    let url = start_mock_server(|mut ws| async move {
        complete_setup(&mut ws).await;

        let msg = json!({"serverContent": {"interrupted": true}});
        ws.send(Message::Text(msg.to_string())).await.unwrap();

        // Keep connection open until client disconnects
        while ws.next().await.is_some() {}
    })
    .await;

    let session = GeminiLiveSession::connect(test_config(url), None)
        .await
        .unwrap();
    let mut rx = session.subscribe();

    expect_event(
        &mut rx,
        |e| matches!(e, GeminiEvent::Connected { .. }),
        "Connected",
    )
    .await;
    expect_event(
        &mut rx,
        |e| matches!(e, GeminiEvent::Interrupted),
        "Interrupted",
    )
    .await;

    session.disconnect();
}

#[tokio::test]
async fn test_session_turn_complete() {
    let url = start_mock_server(|mut ws| async move {
        complete_setup(&mut ws).await;

        let msg = json!({"serverContent": {"turnComplete": true}});
        ws.send(Message::Text(msg.to_string())).await.unwrap();

        // Keep connection open until client disconnects
        while ws.next().await.is_some() {}
    })
    .await;

    let session = GeminiLiveSession::connect(test_config(url), None)
        .await
        .unwrap();
    let mut rx = session.subscribe();

    expect_event(
        &mut rx,
        |e| matches!(e, GeminiEvent::Connected { .. }),
        "Connected",
    )
    .await;
    expect_event(
        &mut rx,
        |e| matches!(e, GeminiEvent::TurnComplete),
        "TurnComplete",
    )
    .await;

    session.disconnect();
}

#[tokio::test]
async fn test_audio_encoding_roundtrip() {
    // This test verifies the PCM f32 -> base64 -> PCM f32 roundtrip
    // at the protocol level, without needing a mock server.
    let original_samples: Vec<f32> = vec![0.0, 0.5, -0.5, 1.0, -1.0, 0.25, -0.75];

    // Encode: f32 -> i16 LE bytes -> base64 (simulating what the session does)
    let mut pcm_bytes = Vec::with_capacity(original_samples.len() * 2);
    for &sample in &original_samples {
        let clamped = sample.clamp(-1.0, 1.0);
        let i16_val = (clamped * 32767.0) as i16;
        pcm_bytes.extend_from_slice(&i16_val.to_le_bytes());
    }
    let encoded = BASE64.encode(&pcm_bytes);

    // Decode: base64 -> i16 LE bytes -> f32 (simulating what the session does on receive)
    let decoded_bytes = BASE64.decode(&encoded).unwrap();
    let decoded_samples: Vec<f32> = decoded_bytes
        .chunks_exact(2)
        .map(|chunk| {
            let sample = i16::from_le_bytes([chunk[0], chunk[1]]);
            sample as f32 / 32768.0
        })
        .collect();

    assert_eq!(decoded_samples.len(), original_samples.len());
    for (i, (original, decoded)) in original_samples
        .iter()
        .zip(decoded_samples.iter())
        .enumerate()
    {
        assert!(
            (original - decoded).abs() < 0.001,
            "Sample {i}: original={original}, decoded={decoded}"
        );
    }
}

#[tokio::test]
async fn test_session_tool_call_cancellation() {
    let url = start_mock_server(|mut ws| async move {
        complete_setup(&mut ws).await;

        let msg = json!({
            "toolCallCancellation": {
                "ids": ["call_1", "call_2"]
            }
        });
        ws.send(Message::Text(msg.to_string())).await.unwrap();

        // Keep connection open until client disconnects
        while ws.next().await.is_some() {}
    })
    .await;

    let session = GeminiLiveSession::connect(test_config(url), None)
        .await
        .unwrap();
    let mut rx = session.subscribe();

    expect_event(
        &mut rx,
        |e| matches!(e, GeminiEvent::Connected { .. }),
        "Connected",
    )
    .await;

    let event = expect_event(
        &mut rx,
        |e| matches!(e, GeminiEvent::ToolCallCancellation { .. }),
        "ToolCallCancellation",
    )
    .await;

    let GeminiEvent::ToolCallCancellation { ids } = event else {
        panic!("Expected ToolCallCancellation, got: {event:?}");
    };
    assert_eq!(ids, vec!["call_1", "call_2"]);

    session.disconnect();
}

#[tokio::test]
async fn test_session_transcription() {
    let url = start_mock_server(|mut ws| async move {
        complete_setup(&mut ws).await;

        let msg = json!({
            "serverContent": {
                "modelTurn": {
                    "parts": [{"text": "Hello there"}]
                }
            }
        });
        ws.send(Message::Text(msg.to_string())).await.unwrap();

        // Keep connection open until client disconnects
        while ws.next().await.is_some() {}
    })
    .await;

    let session = GeminiLiveSession::connect(test_config(url), None)
        .await
        .unwrap();
    let mut rx = session.subscribe();

    expect_event(
        &mut rx,
        |e| matches!(e, GeminiEvent::Connected { .. }),
        "Connected",
    )
    .await;

    let event = expect_event(
        &mut rx,
        |e| matches!(e, GeminiEvent::Transcription { .. }),
        "Transcription",
    )
    .await;

    let GeminiEvent::Transcription { text } = event else {
        panic!("Expected Transcription, got: {event:?}");
    };
    assert_eq!(text, "Hello there");

    session.disconnect();
}

#[tokio::test]
async fn test_session_reconnect_on_server_close() {
    let url = start_mock_server(|mut ws| async move {
        complete_setup(&mut ws).await;

        // Immediately drop the WebSocket to simulate server-side close
        drop(ws);
    })
    .await;

    let session = GeminiLiveSession::connect(test_config(url), None)
        .await
        .unwrap();
    let mut rx = session.subscribe();

    expect_event(
        &mut rx,
        |e| matches!(e, GeminiEvent::Connected { .. }),
        "Connected",
    )
    .await;

    // Use a longer timeout since there's a 1-second backoff before reconnecting
    let event = timeout(Duration::from_secs(5), async {
        loop {
            match rx.recv().await {
                Ok(event) if matches!(event, GeminiEvent::Reconnecting { .. }) => return event,
                Ok(_) => continue,
                Err(e) => panic!("Channel error waiting for Reconnecting: {e}"),
            }
        }
    })
    .await
    .expect("Timed out waiting for Reconnecting event");

    let GeminiEvent::Reconnecting { attempt } = event else {
        panic!("Expected Reconnecting, got: {event:?}");
    };
    assert_eq!(attempt, 1);

    session.disconnect();
}

#[tokio::test]
async fn test_session_connect_failure() {
    // Bind a listener, get its address, then drop it so nothing is listening
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    drop(listener);

    let url = format!("ws://{addr}");
    let session = GeminiLiveSession::connect(test_config(url), None)
        .await
        .unwrap();
    let mut rx = session.subscribe();

    // The session spawns background tasks, so connect() succeeds.
    // But the background connection will fail and we should see a Reconnecting or Error event.
    let event = timeout(Duration::from_secs(5), async {
        loop {
            match rx.recv().await {
                Ok(event)
                    if matches!(
                        event,
                        GeminiEvent::Reconnecting { .. } | GeminiEvent::Error { .. }
                    ) =>
                {
                    return event;
                }
                Ok(_) => continue,
                Err(e) => panic!("Channel error waiting for Reconnecting/Error: {e}"),
            }
        }
    })
    .await
    .expect("Timed out waiting for Reconnecting or Error event");

    assert!(
        matches!(
            event,
            GeminiEvent::Reconnecting { .. } | GeminiEvent::Error { .. }
        ),
        "Expected Reconnecting or Error, got: {event:?}"
    );

    session.disconnect();
}

/// Start a mock WebSocket server that accepts multiple connections.
/// Each connection is handled by the closure, which receives the stream and
/// a connection index (0-based).
async fn start_multi_mock_server<F, Fut>(max_connections: usize, handler: F) -> String
where
    F: Fn(tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>, usize) -> Fut
        + Send
        + Sync
        + 'static,
    Fut: std::future::Future<Output = ()> + Send,
{
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let handler = std::sync::Arc::new(handler);
    tokio::spawn(async move {
        for i in 0..max_connections {
            let (stream, _) = listener.accept().await.unwrap();
            let ws = accept_async(stream).await.unwrap();
            let h = handler.clone();
            tokio::spawn(async move {
                h(ws, i).await;
            });
        }
    });
    format!("ws://{addr}")
}

#[tokio::test]
async fn test_go_away_triggers_reconnect() {
    let url = start_multi_mock_server(2, |mut ws, conn_index| async move {
        complete_setup(&mut ws).await;
        if conn_index == 0 {
            // First connection: send goAway to trigger reconnect
            let msg = json!({"goAway": {"timeToTransfer": "5s"}});
            ws.send(Message::Text(msg.to_string())).await.unwrap();
            // Keep alive briefly
            let _ = tokio::time::timeout(Duration::from_secs(1), async {
                while ws.next().await.is_some() {}
            })
            .await;
        } else {
            // Second connection: just stay alive
            while ws.next().await.is_some() {}
        }
    })
    .await;

    let session = GeminiLiveSession::connect(test_config(url), None)
        .await
        .unwrap();
    let mut rx = session.subscribe();

    // First Connected
    expect_event(
        &mut rx,
        |e| matches!(e, GeminiEvent::Connected { .. }),
        "Connected (first)",
    )
    .await;

    // goAway should reconnect immediately without emitting Reconnecting.
    // The next event we care about is a second Connected.
    expect_event(
        &mut rx,
        |e| matches!(e, GeminiEvent::Connected { .. }),
        "Connected (second, after goAway)",
    )
    .await;

    session.disconnect();
}

#[tokio::test]
async fn test_binary_frame_handled_like_text() {
    let url = start_mock_server(|mut ws| async move {
        complete_setup(&mut ws).await;

        // Send a server message as a Binary frame instead of Text
        let msg = json!({
            "serverContent": {
                "modelTurn": {
                    "parts": [{"text": "Hello from binary"}]
                }
            }
        });
        ws.send(Message::Binary(msg.to_string().into_bytes()))
            .await
            .unwrap();

        // Keep connection open
        while ws.next().await.is_some() {}
    })
    .await;

    let session = GeminiLiveSession::connect(test_config(url), None)
        .await
        .unwrap();
    let mut rx = session.subscribe();

    expect_event(
        &mut rx,
        |e| matches!(e, GeminiEvent::Connected { .. }),
        "Connected",
    )
    .await;

    let event = expect_event(
        &mut rx,
        |e| matches!(e, GeminiEvent::Transcription { .. }),
        "Transcription from binary frame",
    )
    .await;

    let GeminiEvent::Transcription { text } = event else {
        panic!("Expected Transcription, got: {event:?}");
    };
    assert_eq!(text, "Hello from binary");

    session.disconnect();
}

#[tokio::test]
async fn test_setup_timeout_triggers_reconnect() {
    let url = start_multi_mock_server(2, |mut ws, _conn_index| async move {
        // Read setup but never send setupComplete — let the timeout fire.
        let _msg = ws.next().await;
        // Keep alive so the connection doesn't close before the timeout.
        tokio::time::sleep(Duration::from_secs(30)).await;
    })
    .await;

    let session = GeminiLiveSession::connect(test_config(url), None)
        .await
        .unwrap();
    let mut rx = session.subscribe();

    // The setup has a 15-second timeout. We should eventually see Reconnecting.
    let event = timeout(Duration::from_secs(20), async {
        loop {
            match rx.recv().await {
                Ok(event) if matches!(event, GeminiEvent::Reconnecting { .. }) => return event,
                Ok(_) => continue,
                Err(e) => panic!("Channel error: {e}"),
            }
        }
    })
    .await
    .expect("Timed out waiting for Reconnecting event after setup timeout");

    assert!(
        matches!(event, GeminiEvent::Reconnecting { attempt: 1 }),
        "Expected attempt 1, got: {event:?}"
    );

    session.disconnect();
}
