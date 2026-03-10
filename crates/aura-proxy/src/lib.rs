pub mod relay;

use anyhow::Result;
use axum::{
    Router,
    extract::{Query, WebSocketUpgrade},
    response::{IntoResponse, Json},
    routing::get,
};
use serde::Deserialize;
const GEMINI_WS_BASE: &str = "wss://generativelanguage.googleapis.com/ws/\
    google.ai.generativelanguage.v1alpha.GenerativeService.BidiGenerateContent";

#[derive(Deserialize)]
struct ConnectParams {
    api_key: String,
}

async fn health() -> Json<serde_json::Value> {
    Json(serde_json::json!({ "status": "ok" }))
}

async fn ws_handler(
    ws: WebSocketUpgrade,
    Query(params): Query<ConnectParams>,
) -> impl IntoResponse {
    let gemini_url = format!("{GEMINI_WS_BASE}?key={}", params.api_key);
    ws.on_upgrade(move |socket| relay::relay_websocket(socket, gemini_url))
}

pub async fn run_server(port: u16) -> Result<()> {
    let app = Router::new()
        .route("/health", get(health))
        .route("/ws", get(ws_handler));

    let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{port}")).await?;
    tracing::info!("Proxy listening on port {port}");
    axum::serve(listener, app).await?;
    Ok(())
}
