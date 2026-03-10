pub mod relay;

use anyhow::Result;
use axum::{
    Router,
    extract::{Query, WebSocketUpgrade},
    http::StatusCode,
    response::{IntoResponse, Json},
    routing::get,
};
use serde::Deserialize;
use tower::limit::ConcurrencyLimitLayer;

const GEMINI_WS_BASE: &str = "wss://generativelanguage.googleapis.com/ws/\
    google.ai.generativelanguage.v1beta.GenerativeService.BidiGenerateContent";

#[derive(Deserialize)]
struct ConnectParams {
    api_key: String,
    auth_token: Option<String>,
}

async fn health() -> Json<serde_json::Value> {
    Json(serde_json::json!({ "status": "ok" }))
}

async fn ws_handler(
    ws: WebSocketUpgrade,
    Query(params): Query<ConnectParams>,
) -> Result<impl IntoResponse, StatusCode> {
    // Check auth token if AURA_PROXY_AUTH_TOKEN is set
    if let Ok(expected) = std::env::var("AURA_PROXY_AUTH_TOKEN") {
        match &params.auth_token {
            Some(token) if token == &expected => {}
            _ => return Err(StatusCode::UNAUTHORIZED),
        }
    }

    let gemini_url = format!("{GEMINI_WS_BASE}?key={}", params.api_key);
    Ok(ws.on_upgrade(move |socket| relay::relay_websocket(socket, gemini_url)))
}

pub async fn run_server(port: u16) -> Result<()> {
    let app = Router::new()
        .route("/health", get(health))
        .route("/ws", get(ws_handler))
        .layer(ConcurrencyLimitLayer::new(10));

    let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{port}")).await?;
    tracing::info!("Proxy listening on port {port}");
    axum::serve(listener, app).await?;
    Ok(())
}
