use aura_proxy::firestore::DeviceStore;
use std::collections::HashMap;
use std::sync::Mutex;

/// Serializes tests that set SKIP_GEMINI_VALIDATION in addition to ENV_MUTEX.
static REGISTER_MUTEX: Mutex<()> = Mutex::new(());

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
fn test_check_auth_valid_token() {
    assert!(aura_proxy::check_auth(Some("secret123"), "secret123"));
}

#[test]
fn test_check_auth_wrong_token() {
    assert!(!aura_proxy::check_auth(Some("wrong"), "secret123"));
}

#[test]
fn test_check_auth_missing_token() {
    assert!(!aura_proxy::check_auth(None, "secret123"));
}

#[test]
fn test_check_auth_length_mismatch() {
    assert!(!aura_proxy::check_auth(Some("short"), "longer_token"));
}

#[test]
fn test_check_auth_different_lengths_still_compared() {
    assert!(!aura_proxy::check_auth(Some("short"), "muchlongertoken"));
    assert!(!aura_proxy::check_auth(Some("muchlongertoken"), "short"));
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
    unsafe { std::env::set_var("AURA_PROXY_AUTH_TOKEN", "test_secret") };

    let port = free_port();
    let handle = start_server(port).await;

    let resp = reqwest::get(format!("http://127.0.0.1:{port}/health"))
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["status"], "ok");

    // SAFETY: Serialized by ENV_MUTEX.
    unsafe { std::env::remove_var("AURA_PROXY_AUTH_TOKEN") };
    handle.abort();
}

#[tokio::test]
#[allow(clippy::await_holding_lock)]
async fn test_auth_required_without_token_rejected() {
    let _guard = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
    // SAFETY: Serialized by ENV_MUTEX.
    unsafe { std::env::set_var("AURA_PROXY_AUTH_TOKEN", "test_secret") };

    let port = free_port();
    let handle = start_server(port).await;

    // Without providing auth token, /ws/auth should return 401 (auth always required).
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

// ── DeviceStore unit tests ────────────────────────────────────────────

#[tokio::test]
async fn test_device_store_register_and_validate() {
    let store = DeviceStore::new_in_memory();
    let gemini_key_hash =
        "abcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890ab".to_string();

    let token = store
        .register_device("device-001", gemini_key_hash.clone())
        .await
        .expect("registration should succeed");

    // Correct token should validate
    assert!(
        store.validate_token("device-001", &token).await,
        "correct token should be valid"
    );

    // Wrong token should fail
    assert!(
        !store.validate_token("device-001", "wrong-token").await,
        "wrong token should be invalid"
    );

    // Unknown device should fail
    assert!(
        !store.validate_token("unknown-device", &token).await,
        "unknown device should be invalid"
    );
}

#[tokio::test]
async fn test_device_store_reregister_key_mismatch() {
    use aura_proxy::firestore::RegisterError;

    let store = DeviceStore::new_in_memory();
    let gemini_key_hash_a =
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_string();
    let gemini_key_hash_b =
        "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".to_string();

    store
        .register_device("device-002", gemini_key_hash_a)
        .await
        .expect("first registration should succeed");

    let err = store
        .register_device("device-002", gemini_key_hash_b)
        .await
        .expect_err("re-registration with different key should fail");

    assert!(
        matches!(err, RegisterError::KeyMismatch),
        "expected KeyMismatch, got {:?}",
        err
    );
}

#[tokio::test]
async fn test_device_store_reregister_same_key_invalidates_old() {
    let store = DeviceStore::new_in_memory();
    let gemini_key_hash =
        "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc".to_string();

    let old_token = store
        .register_device("device-003", gemini_key_hash.clone())
        .await
        .expect("first registration should succeed");

    let new_token = store
        .register_device("device-003", gemini_key_hash)
        .await
        .expect("re-registration with same key should succeed");

    // New token should work
    assert!(
        store.validate_token("device-003", &new_token).await,
        "new token should be valid"
    );

    // Old token should be invalid
    assert!(
        !store.validate_token("device-003", &old_token).await,
        "old token should be invalidated after re-registration"
    );
}

// ── /register endpoint tests ──────────────────────────────────────────

/// POST /register with an empty JSON body → 422 (Axum deserialization error).
#[tokio::test]
#[allow(clippy::await_holding_lock)]
async fn test_register_missing_fields() {
    let _guard = REGISTER_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
    let _env = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
    // SAFETY: Serialized by ENV_MUTEX + REGISTER_MUTEX.
    unsafe { std::env::set_var("AURA_PROXY_AUTH_TOKEN", "test_secret") };
    unsafe { std::env::set_var("SKIP_GEMINI_VALIDATION", "true") };

    let port = free_port();
    let handle = start_server(port).await;

    let client = reqwest::Client::new();
    let resp = client
        .post(format!("http://127.0.0.1:{port}/register"))
        .header("content-type", "application/json")
        .body("{}")
        .send()
        .await
        .unwrap();

    // Missing required fields → 400 (or 422 from Axum's Json extractor)
    assert!(
        resp.status() == 400 || resp.status() == 422,
        "expected 400 or 422, got {}",
        resp.status()
    );

    // SAFETY: Serialized by ENV_MUTEX + REGISTER_MUTEX.
    unsafe { std::env::remove_var("AURA_PROXY_AUTH_TOKEN") };
    unsafe { std::env::remove_var("SKIP_GEMINI_VALIDATION") };
    handle.abort();
}

/// POST /register with an invalid device_id (path traversal chars) → 400.
#[tokio::test]
#[allow(clippy::await_holding_lock)]
async fn test_register_invalid_device_id() {
    let _guard = REGISTER_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
    let _env = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
    // SAFETY: Serialized by ENV_MUTEX + REGISTER_MUTEX.
    unsafe { std::env::set_var("AURA_PROXY_AUTH_TOKEN", "test_secret") };
    unsafe { std::env::set_var("SKIP_GEMINI_VALIDATION", "true") };

    let port = free_port();
    let handle = start_server(port).await;

    let client = reqwest::Client::new();
    let resp = client
        .post(format!("http://127.0.0.1:{port}/register"))
        .json(&serde_json::json!({
            "device_id": "../bad",
            "gemini_api_key": "some-key"
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 400);

    // SAFETY: Serialized by ENV_MUTEX + REGISTER_MUTEX.
    unsafe { std::env::remove_var("AURA_PROXY_AUTH_TOKEN") };
    unsafe { std::env::remove_var("SKIP_GEMINI_VALIDATION") };
    handle.abort();
}

/// POST /register with a valid device_id and SKIP_GEMINI_VALIDATION=true → 200
/// with a `device_token` field in the response body.
#[tokio::test]
#[allow(clippy::await_holding_lock)]
async fn test_register_success() {
    let _guard = REGISTER_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
    let _env = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
    // SAFETY: Serialized by ENV_MUTEX + REGISTER_MUTEX.
    unsafe { std::env::set_var("AURA_PROXY_AUTH_TOKEN", "test_secret") };
    unsafe { std::env::set_var("SKIP_GEMINI_VALIDATION", "true") };

    let port = free_port();
    let handle = start_server(port).await;

    let client = reqwest::Client::new();
    let resp = client
        .post(format!("http://127.0.0.1:{port}/register"))
        .json(&serde_json::json!({
            "device_id": "test-device-001",
            "gemini_api_key": "AIzaSyTest1234"
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let token = body.get("device_token").and_then(|v| v.as_str());
    assert!(
        token.is_some() && !token.unwrap().is_empty(),
        "expected non-empty device_token in response, got: {body}"
    );

    // SAFETY: Serialized by ENV_MUTEX + REGISTER_MUTEX.
    unsafe { std::env::remove_var("AURA_PROXY_AUTH_TOKEN") };
    unsafe { std::env::remove_var("SKIP_GEMINI_VALIDATION") };
    handle.abort();
}
