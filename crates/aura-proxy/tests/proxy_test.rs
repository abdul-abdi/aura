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
    assert!(!aura_proxy::check_auth(Some("short"), Some("muchlongertoken")));
    assert!(!aura_proxy::check_auth(Some("muchlongertoken"), Some("short")));
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
