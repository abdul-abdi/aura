pub mod firestore;
pub mod relay;

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::Result;
use axum::{
    Json, Router,
    extract::{Query, State, WebSocketUpgrade},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
};

const GEMINI_WS_BASE: &str = "wss://generativelanguage.googleapis.com/ws/google.ai.generativelanguage.v1beta.GenerativeService.BidiGenerateContent";

/// Max WebSocket message/frame size: 1 MiB.
const WS_MAX_SIZE: usize = 1_048_576;

// ── AppState ──────────────────────────────────────────────────────────────────

/// Shared application state injected into all handlers via Axum's `State` extractor.
#[derive(Clone)]
pub struct AppState {
    /// The shared legacy auth token (read from `AURA_PROXY_AUTH_TOKEN`).
    pub legacy_auth_token: Option<String>,
    /// Whether legacy shared-token auth is enabled (env `LEGACY_AUTH_ENABLED`, default `true`).
    pub legacy_auth_enabled: bool,
    /// Device registry (in-memory or Firestore-backed).
    pub device_store: firestore::DeviceStore,
    /// Limits concurrent WebSocket connections.
    pub semaphore: Arc<tokio::sync::Semaphore>,
}

// ── /register request / response types ───────────────────────────────────────

#[derive(serde::Deserialize)]
struct RegisterRequest {
    device_id: String,
    gemini_api_key: String,
}

#[derive(serde::Serialize)]
struct RegisterResponse {
    device_token: String,
}

// ── ConnectParams ─────────────────────────────────────────────────────────────

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

// ── Helpers ───────────────────────────────────────────────────────────────────

async fn health() -> Json<serde_json::Value> {
    Json(serde_json::json!({ "status": "ok" }))
}

/// Constant-time auth check. Returns `true` when the provided token matches.
/// Auth is always required — both `token` and `expected` must be present and match.
pub fn check_auth(token: Option<&str>, expected: &str) -> bool {
    match token {
        Some(token) => {
            use subtle::ConstantTimeEq;
            // Hash both to fixed length to avoid length oracle
            let token_hash = hash_token(token.as_bytes());
            let expected_hash = hash_token(expected.as_bytes());
            token_hash.ct_eq(&expected_hash).unwrap_u8() == 1
        }
        None => false,
    }
}

/// Hash token to fixed-size output for constant-time comparison.
fn hash_token(input: &[u8]) -> [u8; 32] {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(input);
    hasher.finalize().into()
}

/// Validate that a device_id is safe: non-empty, ≤128 chars, alphanumeric + `-` + `_` only.
fn validate_device_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

// ── Handlers ──────────────────────────────────────────────────────────────────

async fn ws_handler(
    State(state): State<AppState>,
    ws: WebSocketUpgrade,
    headers: HeaderMap,
    Query(query): Query<HashMap<String, String>>,
) -> Response {
    let params = extract_connect_params(&headers, &query);

    // Legacy auth check. If legacy auth is enabled a matching token is required.
    if state.legacy_auth_enabled {
        let expected = match &state.legacy_auth_token {
            Some(t) => t.as_str(),
            None => {
                return (StatusCode::INTERNAL_SERVER_ERROR, "Auth not configured").into_response()
            }
        };
        if !check_auth(params.auth_token.as_deref(), expected) {
            return (StatusCode::UNAUTHORIZED, "Invalid auth token").into_response();
        }
    }

    let api_key = match params.api_key {
        Some(key) if !key.is_empty() => key,
        _ => return (StatusCode::BAD_REQUEST, "Missing api_key").into_response(),
    };

    let permit = match state.semaphore.try_acquire_owned() {
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
async fn ws_auth_preflight(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<HashMap<String, String>>,
) -> Response {
    let params = extract_connect_params(&headers, &query);

    if state.legacy_auth_enabled {
        let expected = match &state.legacy_auth_token {
            Some(t) => t.as_str(),
            None => {
                return (StatusCode::INTERNAL_SERVER_ERROR, "Auth not configured").into_response()
            }
        };
        if !check_auth(params.auth_token.as_deref(), expected) {
            return (StatusCode::UNAUTHORIZED, "Invalid auth token").into_response();
        }
    }

    StatusCode::OK.into_response()
}

/// `POST /register` — register a device and receive a per-device auth token.
///
/// Validates the `device_id`, optionally validates the Gemini API key against
/// the Gemini REST API (skipped when `SKIP_GEMINI_VALIDATION=true`), hashes the
/// key, and delegates to `DeviceStore::register_device`.
async fn register_handler(
    State(state): State<AppState>,
    Json(req): Json<RegisterRequest>,
) -> Response {
    // 1. Validate device_id.
    if !validate_device_id(&req.device_id) {
        return (StatusCode::BAD_REQUEST, "Invalid device_id").into_response();
    }

    // 2. Validate Gemini key (skip in test environments).
    let skip_validation = std::env::var("SKIP_GEMINI_VALIDATION")
        .map(|v| v.eq_ignore_ascii_case("true") || v == "1")
        .unwrap_or(false);

    if !skip_validation {
        let validation_url = format!(
            "https://generativelanguage.googleapis.com/v1beta/models?key={}",
            req.gemini_api_key
        );
        let client = reqwest::Client::new();
        match client.get(&validation_url).send().await {
            Ok(resp) if resp.status().is_success() => {} // key is valid
            Ok(_) => {
                return (StatusCode::UNAUTHORIZED, "Invalid Gemini API key").into_response();
            }
            Err(e) => {
                tracing::error!(error = %e, "Gemini key validation request failed");
                return (StatusCode::INTERNAL_SERVER_ERROR, "Key validation failed")
                    .into_response();
            }
        }
    }

    // 3. Hash the Gemini key — never store or log the plaintext.
    let gemini_key_hash = hex::encode(hash_token(req.gemini_api_key.as_bytes()));

    // 4. Register with the device store.
    match state
        .device_store
        .register_device(&req.device_id, gemini_key_hash)
        .await
    {
        Ok(device_token) => Json(RegisterResponse { device_token }).into_response(),
        Err(firestore::RegisterError::KeyMismatch) => {
            (StatusCode::FORBIDDEN, "Gemini key mismatch for existing device").into_response()
        }
        Err(firestore::RegisterError::BackendError(msg)) => {
            tracing::error!(device_id = %req.device_id, error = %msg, "device store backend error");
            (StatusCode::INTERNAL_SERVER_ERROR, "Internal error").into_response()
        }
    }
}

// ── Server entrypoint ─────────────────────────────────────────────────────────

pub async fn run_server(port: u16) -> Result<()> {
    // Legacy shared auth token — optional; panicked on in previous version.
    let legacy_auth_token: Option<String> = std::env::var("AURA_PROXY_AUTH_TOKEN").ok();

    // Whether legacy shared-token auth is active (default: true if token is set).
    let legacy_auth_enabled: bool = std::env::var("LEGACY_AUTH_ENABLED")
        .map(|v| !v.eq_ignore_ascii_case("false") && v != "0")
        .unwrap_or(true);

    // Device store — Firestore if GCP_PROJECT_ID is set, otherwise in-memory.
    let device_store = match std::env::var("GCP_PROJECT_ID").ok() {
        Some(project_id) => {
            tracing::info!(%project_id, "using Firestore device store");
            firestore::DeviceStore::new_with_firestore(project_id)
        }
        None => {
            tracing::info!("using in-memory device store");
            firestore::DeviceStore::new_in_memory()
        }
    };

    // Require at least one auth mechanism.
    if legacy_auth_enabled && legacy_auth_token.is_none() && std::env::var("GCP_PROJECT_ID").is_err()
    {
        panic!(
            "FATAL: Neither AURA_PROXY_AUTH_TOKEN nor GCP_PROJECT_ID is set. \
             Refusing to start without an authentication mechanism."
        );
    }

    let state = AppState {
        legacy_auth_token,
        legacy_auth_enabled,
        device_store,
        semaphore: Arc::new(tokio::sync::Semaphore::new(10)),
    };

    let app = Router::new()
        .route("/health", get(health))
        .route("/ws", get(ws_handler))
        .route("/ws/auth", get(ws_auth_preflight))
        .route("/register", post(register_handler))
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{port}")).await?;
    tracing::info!("Proxy listening on port {port}");
    axum::serve(listener, app).await?;
    Ok(())
}
