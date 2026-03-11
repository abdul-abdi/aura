use axum::extract::ws::{Message, WebSocket};
use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::tungstenite::{self, protocol::WebSocketConfig};

/// Max WebSocket message/frame size for the Gemini upstream: 1 MiB.
const WS_MAX_SIZE: usize = 1_048_576;

/// Redact the API key from a Gemini WebSocket URL for safe logging.
fn redact_url(url: &str) -> String {
    if let Some(pos) = url.find("key=") {
        let key_start = pos + 4;
        let key_end = url[key_start..]
            .find('&')
            .map(|i| key_start + i)
            .unwrap_or(url.len());
        format!("{}key=REDACTED{}", &url[..key_start], &url[key_end..])
    } else {
        url.to_string()
    }
}

/// Relay WebSocket frames between client and Gemini.
pub async fn relay_websocket(mut client_ws: WebSocket, gemini_url: String) {
    let ws_config = WebSocketConfig {
        max_message_size: Some(WS_MAX_SIZE),
        max_frame_size: Some(WS_MAX_SIZE),
        ..Default::default()
    };

    // disable_nagle=true for lower latency on real-time audio relay
    let gemini_conn = match tokio::time::timeout(
        std::time::Duration::from_secs(15),
        tokio_tungstenite::connect_async_with_config(&gemini_url, Some(ws_config), true),
    )
    .await
    {
        Ok(Ok((ws, _))) => ws,
        Ok(Err(e)) => {
            tracing::error!(url = %redact_url(&gemini_url), "Failed to connect to Gemini: {e}");
            let _ = client_ws.close().await;
            return;
        }
        Err(_) => {
            tracing::error!(url = %redact_url(&gemini_url), "Timed out connecting to Gemini");
            let _ = client_ws.close().await;
            return;
        }
    };

    let (mut client_tx, mut client_rx) = client_ws.split();
    let (mut gemini_tx, mut gemini_rx) = gemini_conn.split();

    let client_to_gemini = async {
        loop {
            match client_rx.next().await {
                Some(Ok(msg)) => {
                    let tung_msg = match msg {
                        Message::Text(t) => tungstenite::Message::Text(t.to_string()),
                        Message::Binary(b) => tungstenite::Message::Binary(b.to_vec()),
                        Message::Ping(p) => tungstenite::Message::Ping(p.to_vec()),
                        Message::Pong(p) => tungstenite::Message::Pong(p.to_vec()),
                        Message::Close(_) => break,
                    };
                    if gemini_tx.send(tung_msg).await.is_err() {
                        break;
                    }
                }
                Some(Err(e)) => {
                    tracing::warn!("Client WebSocket error: {e}");
                    break;
                }
                None => break,
            }
        }
    };

    let gemini_to_client = async {
        loop {
            match gemini_rx.next().await {
                Some(Ok(msg)) => {
                    let axum_msg = match msg {
                        tungstenite::Message::Text(t) => Message::text(t),
                        tungstenite::Message::Binary(b) => Message::binary(b),
                        tungstenite::Message::Ping(p) => Message::Ping(p.into()),
                        tungstenite::Message::Pong(p) => Message::Pong(p.into()),
                        tungstenite::Message::Close(_) => break,
                        tungstenite::Message::Frame(_) => continue,
                    };
                    if client_tx.send(axum_msg).await.is_err() {
                        break;
                    }
                }
                Some(Err(e)) => {
                    tracing::warn!("Gemini WebSocket error: {e}");
                    break;
                }
                None => break,
            }
        }
    };

    tokio::select! {
        _ = client_to_gemini => {
            let _ = gemini_tx.close().await;
        },
        _ = gemini_to_client => {
            let _ = client_tx.close().await;
        },
    }

    tracing::info!("WebSocket relay closed");
}
