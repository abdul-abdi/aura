pub mod relay;

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::Result;
use axum::{
    Router,
    extract::{Query, WebSocketUpgrade},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Json, Response},
    routing::get,
};
const GEMINI_WS_BASE: &str = "wss://generativelanguage.googleapis.com/ws/google.ai.generativelanguage.v1beta.GenerativeService.BidiGenerateContent";

/// Max WebSocket message/frame size: 1 MiB.
const WS_MAX_SIZE: usize = 1_048_576;

/// Parsed connection parameters. Fields are `Option` because they may come from
/// headers, query params, or neither.
pub struct ConnectParams {
    pub api_key: Option<String>,
    pub auth_token: Option<String>,
}

/// Extract connection parameters from headers (preferred) with query-param fallback.
///
/// Header precedence:
/// - `x-gemini-key` header  -> `api_key`   (falls back to `api_key` query param)
/// - `x-auth-token` header  -> `auth_token` (falls back to `auth_token` query param)
pub fn extract_connect_params(
    headers: &HeaderMap,
    query: &HashMap<String, String>,
) -> ConnectParams {
    let api_key = headers
        .get("x-gemini-key")
        .and_then(|v| v.to_str().ok())
        .map(String::from)
        .or_else(|| query.get("api_key").cloned());

    let auth_token = headers
        .get("x-auth-token")
        .and_then(|v| v.to_str().ok())
        .map(String::from)
        .or_else(|| query.get("auth_token").cloned());

    ConnectParams {
        api_key,
        auth_token,
    }
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
                use subtle::ConstantTimeEq;
                // Hash both to fixed length to avoid length oracle
                let token_hash = hash_token(token.as_bytes());
                let expected_hash = hash_token(expected.as_bytes());
                token_hash.ct_eq(&expected_hash).unwrap_u8() == 1
            }
            None => false,
        },
    }
}

/// Hash token to fixed-size output for constant-time comparison.
fn hash_token(input: &[u8]) -> [u8; 32] {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(input);
    hasher.finalize().into()
}

async fn ws_handler_with_sem(
    ws: WebSocketUpgrade,
    headers: HeaderMap,
    query: HashMap<String, String>,
    expected: Option<String>,
    semaphore: Arc<tokio::sync::Semaphore>,
) -> Response {
    let params = extract_connect_params(&headers, &query);

    if !check_auth(params.auth_token.as_deref(), expected.as_deref()) {
        return (StatusCode::UNAUTHORIZED, "Invalid auth token").into_response();
    }

    let api_key = match params.api_key {
        Some(key) if !key.is_empty() => key,
        _ => return (StatusCode::BAD_REQUEST, "Missing api_key").into_response(),
    };

    let permit = match semaphore.try_acquire_owned() {
        Ok(permit) => permit,
        Err(_) => return (StatusCode::SERVICE_UNAVAILABLE, "Too many connections").into_response(),
    };

    let gemini_url = format!("{GEMINI_WS_BASE}?key={api_key}");
    ws.max_message_size(WS_MAX_SIZE)
        .max_frame_size(WS_MAX_SIZE)
        .on_upgrade(move |socket| async move {
            relay::relay_websocket(socket, gemini_url).await;
            drop(permit); // released when WS closes
        })
}

/// Auth-only endpoint for testing — validates token without requiring WS upgrade headers.
async fn ws_auth_preflight_with_token(
    headers: HeaderMap,
    query: HashMap<String, String>,
    expected: Option<String>,
) -> Response {
    let params = extract_connect_params(&headers, &query);

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
        .route(
            "/ws",
            get({
                let sem = Arc::clone(&ws_semaphore);
                let token = auth_token.clone();
                move |ws: WebSocketUpgrade,
                      headers: HeaderMap,
                      Query(query): Query<HashMap<String, String>>| {
                    ws_handler_with_sem(ws, headers, query, token, sem)
                }
            }),
        )
        .route(
            "/ws/auth",
            get({
                let token = auth_token;
                move |headers: HeaderMap, Query(query): Query<HashMap<String, String>>| {
                    ws_auth_preflight_with_token(headers, query, token)
                }
            }),
        );

    let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{port}")).await?;
    tracing::info!("Proxy listening on port {port}");
    axum::serve(listener, app).await?;
    Ok(())
}
