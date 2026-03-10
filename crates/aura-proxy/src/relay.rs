use axum::extract::ws::{Message, WebSocket};
use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::tungstenite;

/// Relay WebSocket frames between client and Gemini.
pub async fn relay_websocket(client_ws: WebSocket, gemini_url: String) {
    let gemini_conn = match tokio_tungstenite::connect_async(&gemini_url).await {
        Ok((ws, _)) => ws,
        Err(e) => {
            tracing::error!("Failed to connect to Gemini: {e}");
            return;
        }
    };

    let (mut client_tx, mut client_rx) = client_ws.split();
    let (mut gemini_tx, mut gemini_rx) = gemini_conn.split();

    let client_to_gemini = async {
        while let Some(Ok(msg)) = client_rx.next().await {
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
    };

    let gemini_to_client = async {
        while let Some(Ok(msg)) = gemini_rx.next().await {
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
    };

    tokio::select! {
        _ = client_to_gemini => {
            let _ = gemini_tx.send(tungstenite::Message::Close(None)).await;
        },
        _ = gemini_to_client => {
            let _ = client_tx.send(Message::Close(None)).await;
        },
    }

    tracing::info!("WebSocket relay closed");
}
