pub mod relay;

use std::sync::Arc;

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

async fn ws_handler_with_sem(
    ws: WebSocketUpgrade,
    params: ConnectParams,
    expected: Option<String>,
    semaphore: Arc<tokio::sync::Semaphore>,
) -> Response {
    if !check_auth(params.auth_token.as_deref(), expected.as_deref()) {
        return (StatusCode::UNAUTHORIZED, "Invalid auth token").into_response();
    }

    let permit = match semaphore.try_acquire_owned() {
        Ok(permit) => permit,
        Err(_) => return (StatusCode::SERVICE_UNAVAILABLE, "Too many connections").into_response(),
    };

    let gemini_url = format!("{GEMINI_WS_BASE}?key={}", params.api_key);
    ws.max_message_size(WS_MAX_SIZE)
        .max_frame_size(WS_MAX_SIZE)
        .on_upgrade(move |socket| async move {
            relay::relay_websocket(socket, gemini_url).await;
            drop(permit); // released when WS closes
        })
}

/// Auth-only endpoint for testing — validates token without requiring WS upgrade headers.
async fn ws_auth_preflight_with_token(
    params: ConnectParams,
    expected: Option<String>,
) -> Response {
    if !check_auth(params.auth_token.as_deref(), expected.as_deref()) {
        return (StatusCode::UNAUTHORIZED, "Invalid auth token").into_response();
    }
    StatusCode::OK.into_response()
}

pub async fn run_server(port: u16) -> Result<()> {
    if std::env::var("AURA_PROXY_AUTH_TOKEN").is_err() {
        tracing::warn!("AURA_PROXY_AUTH_TOKEN not set — proxy accepts unauthenticated connections");
    }

    let auth_token: Option<String> = std::env::var("AURA_PROXY_AUTH_TOKEN").ok();
    let ws_semaphore = Arc::new(tokio::sync::Semaphore::new(10));

    let app = Router::new()
        .route("/health", get(health))
        .route("/ws", get({
            let sem = Arc::clone(&ws_semaphore);
            let token = auth_token.clone();
            move |ws: WebSocketUpgrade, Query(params): Query<ConnectParams>| {
                ws_handler_with_sem(ws, params, token, sem)
            }
        }))
        .route("/ws/auth", get({
            let token = auth_token;
            move |Query(params): Query<ConnectParams>| {
                ws_auth_preflight_with_token(params, token)
            }
        }));

    let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{port}")).await?;
    tracing::info!("Proxy listening on port {port}");
    axum::serve(listener, app).await?;
    Ok(())
}
