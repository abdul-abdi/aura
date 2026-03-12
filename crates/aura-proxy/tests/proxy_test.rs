use std::collections::HashMap;
use std::sync::Mutex;

/// Global mutex to serialize tests that read/write AURA_PROXY_AUTH_TOKEN env var.
/// SAFETY: All env var mutations are serialized by this mutex, so no data race
/// can occur with other threads reading the env at the same time.
static ENV_MUTEX: Mutex<()> = Mutex::new(());

fn free_port() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.local_addr().unwrap().port()
}

async fn start_server(port: u16) -> tokio::task::JoinHandle<()> {
    let handle = tokio::spawn(async move {
        aura_proxy::run_server(port).await.unwrap();
    });
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    handle
}

// ── Unit tests for check_auth ────────────────────────────────────────

#[test]
fn test_check_auth_no_expected() {
    // No auth required — any token (or none) should pass.
    assert!(aura_proxy::check_auth(None, None));
    assert!(aura_proxy::check_auth(Some("anything"), None));
}

#[test]
fn test_check_auth_valid_token() {
    assert!(aura_proxy::check_auth(Some("secret123"), Some("secret123")));
}

#[test]
fn test_check_auth_wrong_token() {
    assert!(!aura_proxy::check_auth(Some("wrong"), Some("secret123")));
}

#[test]
fn test_check_auth_missing_token() {
    assert!(!aura_proxy::check_auth(None, Some("secret123")));
}

#[test]
fn test_check_auth_length_mismatch() {
    assert!(!aura_proxy::check_auth(Some("short"), Some("longer_token")));
}

#[test]
fn test_check_auth_different_lengths_still_compared() {
    assert!(!aura_proxy::check_auth(
        Some("short"),
        Some("muchlongertoken")
    ));
    assert!(!aura_proxy::check_auth(
        Some("muchlongertoken"),
        Some("short")
    ));
}

// ── Header extraction unit tests ─────────────────────────────────────

#[test]
fn test_extract_connect_params_from_headers() {
    use axum::http::{HeaderMap, HeaderValue};

    let mut headers = HeaderMap::new();
    headers.insert("x-gemini-key", HeaderValue::from_static("my-api-key"));
    headers.insert("x-auth-token", HeaderValue::from_static("my-auth-token"));

    let params = aura_proxy::extract_connect_params(&headers, &HashMap::new());
    assert_eq!(params.api_key, Some("my-api-key".to_string()));
    assert_eq!(params.auth_token, Some("my-auth-token".to_string()));
}

#[test]
fn test_extract_connect_params_query_fallback() {
    use axum::http::HeaderMap;

    let headers = HeaderMap::new();
    let mut query = HashMap::new();
    query.insert("api_key".to_string(), "query-key".to_string());
    query.insert("auth_token".to_string(), "query-token".to_string());

    let params = aura_proxy::extract_connect_params(&headers, &query);
    assert_eq!(params.api_key, Some("query-key".to_string()));
    assert_eq!(params.auth_token, Some("query-token".to_string()));
}

#[test]
fn test_extract_connect_params_headers_take_precedence() {
    use axum::http::{HeaderMap, HeaderValue};

    let mut headers = HeaderMap::new();
    headers.insert("x-gemini-key", HeaderValue::from_static("header-key"));

    let mut query = HashMap::new();
    query.insert("api_key".to_string(), "query-key".to_string());
    query.insert("auth_token".to_string(), "query-token".to_string());

    let params = aura_proxy::extract_connect_params(&headers, &query);
    // Header takes precedence over query param
    assert_eq!(params.api_key, Some("header-key".to_string()));
    // auth_token falls back to query param
    assert_eq!(params.auth_token, Some("query-token".to_string()));
}

// ── Integration tests ────────────────────────────────────────────────
// These tests intentionally hold ENV_MUTEX across await points to serialize
// env var mutations. The lock is never contended across threads in practice
// because tokio::test runs each test on its own runtime.

#[tokio::test]
#[allow(clippy::await_holding_lock)]
async fn test_health_endpoint() {
    let _guard = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
    // SAFETY: Serialized by ENV_MUTEX.
    unsafe { std::env::remove_var("AURA_PROXY_AUTH_TOKEN") };

    let port = free_port();
    let handle = start_server(port).await;

    let resp = reqwest::get(format!("http://127.0.0.1:{port}/health"))
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["status"], "ok");

    handle.abort();
}

#[tokio::test]
#[allow(clippy::await_holding_lock)]
async fn test_auth_not_required() {
    let _guard = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
    // SAFETY: Serialized by ENV_MUTEX.
    unsafe { std::env::remove_var("AURA_PROXY_AUTH_TOKEN") };

    let port = free_port();
    let handle = start_server(port).await;

    // Without auth token env var, /ws/auth should return 200 (no auth needed).
    let resp = reqwest::get(format!("http://127.0.0.1:{port}/ws/auth?api_key=test_key"))
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    handle.abort();
}

#[tokio::test]
#[allow(clippy::await_holding_lock)]
async fn test_auth_valid_token_accepted() {
    let _guard = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
    // SAFETY: Serialized by ENV_MUTEX.
    unsafe { std::env::set_var("AURA_PROXY_AUTH_TOKEN", "test_secret") };

    let port = free_port();
    let handle = start_server(port).await;

    // Valid token — should get 200.
    let resp = reqwest::get(format!(
        "http://127.0.0.1:{port}/ws/auth?api_key=test_key&auth_token=test_secret"
    ))
    .await
    .unwrap();
    assert_eq!(resp.status(), 200);

    // SAFETY: Serialized by ENV_MUTEX.
    unsafe { std::env::remove_var("AURA_PROXY_AUTH_TOKEN") };
    handle.abort();
}

#[tokio::test]
#[allow(clippy::await_holding_lock)]
async fn test_auth_wrong_token_rejected() {
    let _guard = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
    // SAFETY: Serialized by ENV_MUTEX.
    unsafe { std::env::set_var("AURA_PROXY_AUTH_TOKEN", "test_secret") };

    let port = free_port();
    let handle = start_server(port).await;

    let resp = reqwest::get(format!(
        "http://127.0.0.1:{port}/ws/auth?api_key=test_key&auth_token=wrong"
    ))
    .await
    .unwrap();
    assert_eq!(resp.status(), 401);

    // SAFETY: Serialized by ENV_MUTEX.
    unsafe { std::env::remove_var("AURA_PROXY_AUTH_TOKEN") };
    handle.abort();
}

#[tokio::test]
#[allow(clippy::await_holding_lock)]
async fn test_auth_missing_token_rejected() {
    let _guard = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
    // SAFETY: Serialized by ENV_MUTEX.
    unsafe { std::env::set_var("AURA_PROXY_AUTH_TOKEN", "test_secret") };

    let port = free_port();
    let handle = start_server(port).await;

    let resp = reqwest::get(format!("http://127.0.0.1:{port}/ws/auth?api_key=test_key"))
        .await
        .unwrap();
    assert_eq!(resp.status(), 401);

    // SAFETY: Serialized by ENV_MUTEX.
    unsafe { std::env::remove_var("AURA_PROXY_AUTH_TOKEN") };
    handle.abort();
}

#[tokio::test]
#[allow(clippy::await_holding_lock)]
async fn test_auth_via_headers_accepted() {
    let _guard = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
    // SAFETY: Serialized by ENV_MUTEX.
    unsafe { std::env::set_var("AURA_PROXY_AUTH_TOKEN", "test_secret") };

    let port = free_port();
    let handle = start_server(port).await;

    // Auth via headers instead of query params — should get 200.
    let client = reqwest::Client::new();
    let resp = client
        .get(format!("http://127.0.0.1:{port}/ws/auth"))
        .header("x-gemini-key", "test_key")
        .header("x-auth-token", "test_secret")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    // SAFETY: Serialized by ENV_MUTEX.
    unsafe { std::env::remove_var("AURA_PROXY_AUTH_TOKEN") };
    handle.abort();
}

#[tokio::test]
#[allow(clippy::await_holding_lock)]
async fn test_auth_via_headers_wrong_token_rejected() {
    let _guard = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
    // SAFETY: Serialized by ENV_MUTEX.
    unsafe { std::env::set_var("AURA_PROXY_AUTH_TOKEN", "test_secret") };

    let port = free_port();
    let handle = start_server(port).await;

    let client = reqwest::Client::new();
    let resp = client
        .get(format!("http://127.0.0.1:{port}/ws/auth"))
        .header("x-gemini-key", "test_key")
        .header("x-auth-token", "wrong_token")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 401);

    // SAFETY: Serialized by ENV_MUTEX.
    unsafe { std::env::remove_var("AURA_PROXY_AUTH_TOKEN") };
    handle.abort();
}
