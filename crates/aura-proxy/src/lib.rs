pub mod relay;

use anyhow::Result;
use axum::{
    Router,
    extract::{Query, WebSocketUpgrade},
    http::StatusCode,
    response::{IntoResponse, Json, Response},
    routing::get,
};
use serde::Deserialize;
use subtle::ConstantTimeEq;
use tower::limit::ConcurrencyLimitLayer;

const GEMINI_WS_BASE: &str = "wss://generativelanguage.googleapis.com/ws/\
    google.ai.generativelanguage.v1beta.GenerativeService.BidiGenerateContent";

/// Max WebSocket message/frame size: 1 MiB.
const WS_MAX_SIZE: usize = 1_048_576;

#[derive(Deserialize)]
pub struct ConnectParams {
    pub api_key: String,
    pub auth_token: Option<String>,
}

async fn health() -> Json<serde_json::Value> {
    Json(serde_json::json!({ "status": "ok" }))
}

/// Constant-time auth check. Returns `true` when access should be allowed.
/// If `expected` is `None`, auth is not required and always passes.
pub fn check_auth(token: Option<&str>, expected: Option<&str>) -> bool {
    match expected {
        None => true,
        Some(expected) => match token {
            Some(token) => {
                let token_bytes = token.as_bytes();
                let expected_bytes = expected.as_bytes();
                token_bytes.len() == expected_bytes.len()
                    && token_bytes.ct_eq(expected_bytes).unwrap_u8() == 1
            }
            None => false,
        },
    }
}

async fn ws_handler(ws: WebSocketUpgrade, Query(params): Query<ConnectParams>) -> Response {
    // Check auth token if AURA_PROXY_AUTH_TOKEN is set
    let expected = std::env::var("AURA_PROXY_AUTH_TOKEN").ok();
    if !check_auth(params.auth_token.as_deref(), expected.as_deref()) {
        return (StatusCode::UNAUTHORIZED, "Invalid auth token").into_response();
    }

    let gemini_url = format!("{GEMINI_WS_BASE}?key={}", params.api_key);
    ws.max_message_size(WS_MAX_SIZE)
        .max_frame_size(WS_MAX_SIZE)
        .on_upgrade(move |socket| relay::relay_websocket(socket, gemini_url))
}

/// Auth-only endpoint for testing — validates token without requiring WS upgrade headers.
async fn ws_auth_preflight(Query(params): Query<ConnectParams>) -> Response {
    let expected = std::env::var("AURA_PROXY_AUTH_TOKEN").ok();
    if !check_auth(params.auth_token.as_deref(), expected.as_deref()) {
        return (StatusCode::UNAUTHORIZED, "Invalid auth token").into_response();
    }
    StatusCode::OK.into_response()
}

pub async fn run_server(port: u16) -> Result<()> {
    if std::env::var("AURA_PROXY_AUTH_TOKEN").is_err() {
        tracing::warn!("AURA_PROXY_AUTH_TOKEN not set — proxy accepts unauthenticated connections");
    }

    let app = Router::new()
        .route("/health", get(health))
        .route("/ws", get(ws_handler))
        .route("/ws/auth", get(ws_auth_preflight))
        .layer(ConcurrencyLimitLayer::new(10));

    let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{port}")).await?;
    tracing::info!("Proxy listening on port {port}");
    axum::serve(listener, app).await?;
    Ok(())
}
