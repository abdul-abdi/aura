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
    pub device_id: Option<String>,
    pub device_token: Option<String>,
}

/// Extract connection parameters from headers (preferred) with query-param fallback.
///
/// Header precedence:
/// - `x-gemini-key` header    -> `api_key`      (falls back to `api_key` query param)
/// - `x-auth-token` header    -> `auth_token`   (falls back to `auth_token` query param)
/// - `x-device-id` header     -> `device_id`    (falls back to `device_id` query param)
/// - `x-device-token` header  -> `device_token` (falls back to `device_token` query param)
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

    let device_id = headers
        .get("x-device-id")
        .and_then(|v| v.to_str().ok())
        .map(String::from)
        .or_else(|| query.get("device_id").cloned());

    let device_token = headers
        .get("x-device-token")
        .and_then(|v| v.to_str().ok())
        .map(String::from)
        .or_else(|| query.get("device_token").cloned());

    ConnectParams {
        api_key,
        auth_token,
        device_id,
        device_token,
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

/// Dual-mode auth check: tries legacy shared-token first, then per-device token.
///
/// Returns `true` when at least one of the following conditions holds:
/// - Legacy auth is enabled, a `legacy_token` was provided, and it matches
///   `state.legacy_auth_token`.
/// - Both `device_id` and `device_token` are provided and
///   `state.device_store.validate_token` succeeds.
///
/// Returns `false` when neither condition is satisfied (including when both
/// credential sets are absent — no anonymous access).
async fn check_auth_dual(
    state: &AppState,
    legacy_token: Option<&str>,
    device_id: Option<&str>,
    device_token: Option<&str>,
) -> bool {
    // Validate device_id format before any backend lookup.
    if let Some(did) = device_id
        && !validate_device_id(did)
    {
        return false;
    }

    // Try legacy auth first (if enabled and a token was provided).
    if state.legacy_auth_enabled
        && let (Some(provided), Some(expected)) = (legacy_token, state.legacy_auth_token.as_deref())
        && check_auth(Some(provided), expected)
    {
        return true;
    }

    // Try per-device token auth.
    if let (Some(did), Some(dtok)) = (device_id, device_token) {
        return state.device_store.validate_token(did, dtok).await;
    }

    false
}

/// Validate that a device_id is safe: non-empty, ≤128 chars, alphanumeric + `-` + `_` only.
fn validate_device_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

// ── Handlers ──────────────────────────────────────────────────────────────────

async fn ws_handler(
    State(state): State<AppState>,
    ws: WebSocketUpgrade,
    headers: HeaderMap,
    Query(query): Query<HashMap<String, String>>,
) -> Response {
    let params = extract_connect_params(&headers, &query);

    // Dual-mode auth: legacy shared token OR per-device token.
    if !check_auth_dual(
        &state,
        params.auth_token.as_deref(),
        params.device_id.as_deref(),
        params.device_token.as_deref(),
    )
    .await
    {
        return (StatusCode::UNAUTHORIZED, "Invalid auth token").into_response();
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

    // Dual-mode auth: legacy shared token OR per-device token.
    if !check_auth_dual(
        &state,
        params.auth_token.as_deref(),
        params.device_id.as_deref(),
        params.device_token.as_deref(),
    )
    .await
    {
        return (StatusCode::UNAUTHORIZED, "Invalid auth token").into_response();
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
        let key_valid = reqwest::Client::new()
            .get("https://generativelanguage.googleapis.com/v1beta/models")
            .header("x-goog-api-key", &req.gemini_api_key)
            .send()
            .await
            .map(|r| r.status().is_success())
            .unwrap_or(false);
        if !key_valid {
            return (StatusCode::UNAUTHORIZED, "Invalid Gemini API key").into_response();
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
        Err(firestore::RegisterError::KeyMismatch) => (
            StatusCode::FORBIDDEN,
            "Gemini key mismatch for existing device",
        )
            .into_response(),
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
    if legacy_auth_token.is_none() && std::env::var("GCP_PROJECT_ID").is_err() {
        panic!(
            "FATAL: Neither AURA_PROXY_AUTH_TOKEN nor GCP_PROJECT_ID is set. \
             At least one auth mechanism must be configured."
        );
    }

    // Prevent disabling Gemini key validation in production (when GCP is configured).
    if std::env::var("SKIP_GEMINI_VALIDATION").is_ok() && std::env::var("GCP_PROJECT_ID").is_ok() {
        panic!(
            "FATAL: SKIP_GEMINI_VALIDATION must not be set when GCP_PROJECT_ID is configured (production)"
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
