# Per-Device Token Registration Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace shared auth tokens with per-device tokens issued during onboarding, stored in macOS Keychain, and validated against a Firestore device registry.

**Architecture:** The proxy gains a `/register` endpoint that validates a Gemini API key, generates a per-device token, and stores its hash in Firestore `devices/{device_id}`. The proxy, memory agent, and consolidation service all validate device tokens against this Firestore collection (with 60s in-memory cache). The client stores the token in macOS Keychain, with device ID in config.toml.

**Tech Stack:** Rust (proxy, daemon, gemini config), Swift (AuraApp onboarding + Keychain), Python (memory agent), Firestore REST API, macOS Security.framework, `security-framework` crate.

**Spec:** `docs/superpowers/specs/2026-03-15-device-registration-design.md`

---

## Chunk 1: Firestore Device Registry + Proxy Registration Endpoint

### Task 1: Add Firestore REST client to proxy

**Files:**
- Create: `crates/aura-proxy/src/firestore.rs`
- Modify: `crates/aura-proxy/Cargo.toml`
- Modify: `crates/aura-proxy/src/lib.rs:1` (add `mod firestore`)
- Test: `crates/aura-proxy/tests/proxy_test.rs`

- [ ] **Step 1: Add dependencies to Cargo.toml**

Add `reqwest`, `serde`, `serde_json`, `chrono` to `[dependencies]` in `crates/aura-proxy/Cargo.toml`:

```toml
reqwest = { version = "0.12", features = ["json"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
chrono = "0.4"
rand = "0.9"
hex = "0.4"
```

Move `reqwest` from `[dev-dependencies]` to `[dependencies]` (it's already there for tests).

Note: `sha2`, `subtle`, and `tracing` are already in the proxy's `Cargo.toml` — do not re-add them.

- [ ] **Step 2: Write failing test for Firestore device store**

In `crates/aura-proxy/tests/proxy_test.rs`, add a test that calls the future `DeviceStore` API:

```rust
#[tokio::test]
async fn test_device_store_register_and_validate() {
    use aura_proxy::firestore::DeviceStore;

    let store = DeviceStore::new_in_memory();
    let token = store.register_device("dev-123", "abc_gemini_key_hash").await.unwrap();
    assert_eq!(token.len(), 64); // 64 hex chars = 256 bits

    assert!(store.validate_token("dev-123", &token).await);
    assert!(!store.validate_token("dev-123", "wrong-token").await);
    assert!(!store.validate_token("dev-999", &token).await);
}
```

- [ ] **Step 3: Run test to verify it fails**

Run: `cargo test -p aura-proxy test_device_store_register_and_validate`
Expected: FAIL — `firestore` module not found.

- [ ] **Step 4: Implement DeviceStore with in-memory backend**

Create `crates/aura-proxy/src/firestore.rs`:

```rust
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use sha2::{Sha256, Digest};
use subtle::ConstantTimeEq;
use tokio::sync::RwLock;

/// Device record stored in Firestore (or in-memory for tests).
#[derive(Clone, Debug)]
struct DeviceRecord {
    token_hash: String,
    gemini_key_hash: String,
    created_at: String,
}

/// Cached device record with expiry.
struct CachedRecord {
    token_hash: String,
    fetched_at: Instant,
}

const CACHE_TTL: Duration = Duration::from_secs(60);

/// Abstraction over Firestore device registry.
/// Supports both real Firestore (production) and in-memory (tests).
#[derive(Clone)]
pub struct DeviceStore {
    backend: Arc<dyn DeviceBackend + Send + Sync>,
    cache: Arc<RwLock<HashMap<String, CachedRecord>>>,
}

#[async_trait::async_trait]
pub trait DeviceBackend: Send + Sync {
    async fn get_device(&self, device_id: &str) -> Option<DeviceRecord>;
    async fn set_device(&self, device_id: &str, record: DeviceRecord);
}

/// In-memory backend for tests.
struct InMemoryBackend {
    devices: Arc<RwLock<HashMap<String, DeviceRecord>>>,
}

#[async_trait::async_trait]
impl DeviceBackend for InMemoryBackend {
    async fn get_device(&self, device_id: &str) -> Option<DeviceRecord> {
        self.devices.read().await.get(device_id).cloned()
    }
    async fn set_device(&self, device_id: &str, record: DeviceRecord) {
        self.devices.write().await.insert(device_id.to_string(), record);
    }
}

fn hash_value(value: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(value.as_bytes());
    hex::encode(hasher.finalize())
}

impl DeviceStore {
    pub fn new_in_memory() -> Self {
        Self {
            backend: Arc::new(InMemoryBackend {
                devices: Arc::new(RwLock::new(HashMap::new())),
            }),
            cache: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Register a device, returning the plaintext token.
    /// If the device already exists AND the gemini_key_hash matches,
    /// generates a new token (invalidating the old one).
    pub async fn register_device(
        &self,
        device_id: &str,
        gemini_key_hash: &str,
    ) -> Result<String, RegisterError> {
        // Check for existing device with mismatched key.
        // Both sides are already SHA-256 hex strings — compare directly.
        if let Some(existing) = self.backend.get_device(device_id).await {
            if existing.gemini_key_hash.as_bytes()
                .ct_ne(gemini_key_hash.as_bytes())
                .into()
            {
                return Err(RegisterError::KeyMismatch);
            }
        }

        let token = generate_token();
        let record = DeviceRecord {
            token_hash: hash_value(&token),
            gemini_key_hash: gemini_key_hash.to_string(),
            created_at: chrono::Utc::now().to_rfc3339(),
        };
        self.backend.set_device(device_id, record).await;

        // Invalidate cache for this device
        self.cache.write().await.remove(device_id);

        Ok(token)
    }

    /// Validate a device token. Uses cache with 60s TTL.
    pub async fn validate_token(&self, device_id: &str, token: &str) -> bool {
        let provided_hash = hash_value(token);

        // Check cache first
        {
            let cache = self.cache.read().await;
            if let Some(cached) = cache.get(device_id) {
                if cached.fetched_at.elapsed() < CACHE_TTL {
                    return provided_hash.as_bytes()
                        .ct_eq(cached.token_hash.as_bytes())
                        .into();
                }
            }
        }

        // Cache miss — read from backend
        let record = match self.backend.get_device(device_id).await {
            Some(r) => r,
            None => return false,
        };

        // Update cache
        self.cache.write().await.insert(device_id.to_string(), CachedRecord {
            token_hash: record.token_hash.clone(),
            fetched_at: Instant::now(),
        });

        let valid: bool = provided_hash.as_bytes()
            .ct_eq(record.token_hash.as_bytes())
            .into();

        // Fire-and-forget last_seen update on cache miss (at most once per TTL)
        if valid {
            let backend = self.backend.clone();
            let did = device_id.to_string();
            let rec = record.clone();
            tokio::spawn(async move {
                let updated = DeviceRecord {
                    token_hash: rec.token_hash,
                    gemini_key_hash: rec.gemini_key_hash,
                    created_at: rec.created_at,
                };
                backend.set_device(&did, updated).await;
            });
        }

        valid
    }
}

fn generate_token() -> String {
    use rand::Rng;
    let bytes: [u8; 32] = rand::rng().random();
    hex::encode(bytes)
}

#[derive(Debug)]
pub enum RegisterError {
    KeyMismatch,
    BackendError(String),
}

impl std::fmt::Display for RegisterError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::KeyMismatch => write!(f, "Gemini key does not match original registration"),
            Self::BackendError(e) => write!(f, "Backend error: {e}"),
        }
    }
}
```

Add `mod firestore;` to `crates/aura-proxy/src/lib.rs` and add `async-trait` to `Cargo.toml`:

```toml
async-trait = "0.1"
```

- [ ] **Step 5: Run test to verify it passes**

Run: `cargo test -p aura-proxy test_device_store_register_and_validate`
Expected: PASS

- [ ] **Step 6: Write test for re-registration with key mismatch**

```rust
#[tokio::test]
async fn test_device_store_reregister_key_mismatch() {
    use aura_proxy::firestore::DeviceStore;

    let store = DeviceStore::new_in_memory();
    store.register_device("dev-123", "key_hash_1").await.unwrap();

    let result = store.register_device("dev-123", "key_hash_2").await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_device_store_reregister_same_key_invalidates_old() {
    use aura_proxy::firestore::DeviceStore;

    let store = DeviceStore::new_in_memory();
    let token1 = store.register_device("dev-123", "key_hash_1").await.unwrap();
    let token2 = store.register_device("dev-123", "key_hash_1").await.unwrap();

    assert_ne!(token1, token2);
    assert!(!store.validate_token("dev-123", &token1).await);
    assert!(store.validate_token("dev-123", &token2).await);
}
```

- [ ] **Step 7: Run tests to verify they pass**

Run: `cargo test -p aura-proxy test_device_store`
Expected: All 3 tests PASS

- [ ] **Step 8: Commit**

```bash
git add crates/aura-proxy/
git commit -m "feat: add device store with in-memory backend and token validation"
```

### Task 2: Add Firestore REST backend to DeviceStore

**Files:**
- Modify: `crates/aura-proxy/src/firestore.rs`

- [ ] **Step 1: Write the Firestore REST backend**

Add to `crates/aura-proxy/src/firestore.rs`:

```rust
/// Firestore REST API backend for production.
/// Uses Application Default Credentials (ADC) via the metadata server on Cloud Run.
pub struct FirestoreBackend {
    client: reqwest::Client,
    project_id: String,
}

impl FirestoreBackend {
    pub fn new(project_id: String) -> Self {
        Self {
            client: reqwest::Client::new(),
            project_id,
        }
    }

    async fn get_access_token(&self) -> Result<String, String> {
        let resp = self.client
            .get("http://metadata.google.internal/computeMetadata/v1/instance/service-accounts/default/token")
            .header("Metadata-Flavor", "Google")
            .send()
            .await
            .map_err(|e| e.to_string())?;

        let json: serde_json::Value = resp.json().await.map_err(|e| e.to_string())?;
        json.get("access_token")
            .and_then(|v| v.as_str())
            .map(String::from)
            .ok_or_else(|| "No access_token in metadata response".to_string())
    }

    fn doc_url(&self, device_id: &str) -> String {
        format!(
            "https://firestore.googleapis.com/v1/projects/{}/databases/(default)/documents/devices/{}",
            self.project_id, device_id
        )
    }
}

#[async_trait::async_trait]
impl DeviceBackend for FirestoreBackend {
    async fn get_device(&self, device_id: &str) -> Option<DeviceRecord> {
        let token = self.get_access_token().await.ok()?;
        let resp = self.client
            .get(&self.doc_url(device_id))
            .bearer_auth(&token)
            .send()
            .await
            .ok()?;

        if !resp.status().is_success() {
            return None;
        }

        let json: serde_json::Value = resp.json().await.ok()?;
        let fields = json.get("fields")?;

        Some(DeviceRecord {
            token_hash: fields.get("token_hash")?.get("stringValue")?.as_str()?.to_string(),
            gemini_key_hash: fields.get("gemini_key_hash")?.get("stringValue")?.as_str()?.to_string(),
            created_at: fields.get("created_at")?.get("stringValue")?.as_str()?.to_string(),
        })
    }

    async fn set_device(&self, device_id: &str, record: DeviceRecord) {
        let token = match self.get_access_token().await {
            Ok(t) => t,
            Err(e) => {
                tracing::error!("Failed to get access token: {e}");
                return;
            }
        };

        let body = serde_json::json!({
            "fields": {
                "token_hash": { "stringValue": record.token_hash },
                "gemini_key_hash": { "stringValue": record.gemini_key_hash },
                "created_at": { "stringValue": record.created_at },
                "last_seen": { "stringValue": chrono::Utc::now().to_rfc3339() },
            }
        });

        let url = format!("{}?updateMask.fieldPaths=token_hash&updateMask.fieldPaths=gemini_key_hash&updateMask.fieldPaths=created_at&updateMask.fieldPaths=last_seen",
            self.doc_url(device_id));

        if let Err(e) = self.client
            .patch(&url)
            .bearer_auth(&token)
            .json(&body)
            .send()
            .await
        {
            tracing::error!("Failed to write device {device_id}: {e}");
        }
    }
}

impl DeviceStore {
    pub fn new_with_firestore(project_id: String) -> Self {
        Self {
            backend: Arc::new(FirestoreBackend::new(project_id)),
            cache: Arc::new(RwLock::new(HashMap::new())),
        }
    }
}
```

- [ ] **Step 2: Verify compilation**

Run: `cargo check -p aura-proxy`
Expected: Compiles with no errors.

- [ ] **Step 3: Commit**

```bash
git add crates/aura-proxy/src/firestore.rs
git commit -m "feat: add Firestore REST backend for device store"
```

### Task 3: Add `/register` endpoint to proxy

**Files:**
- Modify: `crates/aura-proxy/src/lib.rs`
- Test: `crates/aura-proxy/tests/proxy_test.rs`

- [ ] **Step 1: Write failing test for `/register`**

In `crates/aura-proxy/tests/proxy_test.rs`:

```rust
#[tokio::test]
async fn test_register_missing_fields() {
    let _lock = ENV_MUTEX.lock().unwrap();
    let port = free_port();
    std::env::set_var("AURA_PROXY_AUTH_TOKEN", "legacy-token");
    let handle = start_server(port).await;

    let client = reqwest::Client::new();
    let resp = client
        .post(format!("http://127.0.0.1:{port}/register"))
        .json(&serde_json::json!({}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);

    std::env::remove_var("AURA_PROXY_AUTH_TOKEN");
    handle.abort();
}

#[tokio::test]
async fn test_register_invalid_device_id() {
    let _lock = ENV_MUTEX.lock().unwrap();
    let port = free_port();
    std::env::set_var("AURA_PROXY_AUTH_TOKEN", "legacy-token");
    let handle = start_server(port).await;

    let client = reqwest::Client::new();
    let resp = client
        .post(format!("http://127.0.0.1:{port}/register"))
        .json(&serde_json::json!({
            "device_id": "../bad-path",
            "gemini_api_key": "AIzaSyTestKey1234567890"
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);

    std::env::remove_var("AURA_PROXY_AUTH_TOKEN");
    handle.abort();
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p aura-proxy test_register`
Expected: FAIL — no `/register` route.

- [ ] **Step 3: Implement `/register` endpoint**

Modify `crates/aura-proxy/src/lib.rs`:

1. Add `use firestore::DeviceStore;` and `use axum::Json;`
2. Add `DeviceStore` to the router state
3. Add device_id validation function (alphanumeric + hyphens + underscores, max 128)
4. Add `/register` POST handler
5. Wire the route into `run_server`

The handler:
- Parses `{ device_id, gemini_api_key }` from JSON body
- Validates `device_id` format → 400 if invalid
- Validates Gemini key via `GET https://generativelanguage.googleapis.com/v1beta/models?key={key}` → 401 if invalid
- Calls `device_store.register_device(device_id, gemini_key_hash)` → 403 if key mismatch
- Returns `{ device_token }` with 200

```rust
#[derive(serde::Deserialize)]
struct RegisterRequest {
    device_id: String,
    gemini_api_key: String,
}

#[derive(serde::Serialize)]
struct RegisterResponse {
    device_token: String,
}

fn validate_device_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

async fn register_handler(
    State(state): State<AppState>,
    Json(req): Json<RegisterRequest>,
) -> impl axum::response::IntoResponse {
    use axum::http::StatusCode;

    if !validate_device_id(&req.device_id) {
        return (StatusCode::BAD_REQUEST, Json(serde_json::json!({"error": "Invalid device_id"}))).into_response();
    }

    // Validate Gemini key (do NOT log the full key)
    let key_valid = reqwest::get(format!(
        "https://generativelanguage.googleapis.com/v1beta/models?key={}",
        req.gemini_api_key
    ))
    .await
    .map(|r| r.status().is_success())
    .unwrap_or(false);

    if !key_valid {
        return (StatusCode::UNAUTHORIZED, Json(serde_json::json!({"error": "Invalid Gemini API key"}))).into_response();
    }

    let gemini_key_hash = hash_token(&req.gemini_api_key);

    match state.device_store.register_device(&req.device_id, &gemini_key_hash).await {
        Ok(token) => (StatusCode::OK, Json(serde_json::json!({"device_token": token}))).into_response(),
        Err(firestore::RegisterError::KeyMismatch) => {
            (StatusCode::FORBIDDEN, Json(serde_json::json!({"error": "Gemini key mismatch"}))).into_response()
        }
        Err(e) => {
            tracing::error!("Registration failed: {e}");
            (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": "Internal error"}))).into_response()
        }
    }
}
```

Add `AppState` struct to hold both the legacy auth token and device store:

```rust
#[derive(Clone)]
struct AppState {
    auth_token: Option<String>,
    device_store: DeviceStore,
    semaphore: Arc<tokio::sync::Semaphore>,
}
```

Update `run_server`:
- Read `AURA_PROXY_AUTH_TOKEN` as `Option<String>` (no panic if missing)
- Read `GCP_PROJECT_ID` env var — if set, use `DeviceStore::new_with_firestore(project_id)`, otherwise `DeviceStore::new_in_memory()`
- Read `LEGACY_AUTH_ENABLED` env var (default `true`)
- Panic only if neither legacy auth token nor GCP project is configured
- Create `AppState` with the store, add `.route("/register", post(register_handler))`

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p aura-proxy test_register`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add crates/aura-proxy/
git commit -m "feat: add /register endpoint with device_id validation and Gemini key check"
```

### Task 4: Update proxy auth to support device tokens

**Files:**
- Modify: `crates/aura-proxy/src/lib.rs`
- Test: `crates/aura-proxy/tests/proxy_test.rs`

- [ ] **Step 1: Write failing test for device token auth on `/ws`**

```rust
/// Helper: start server with device store but no legacy token.
async fn start_server_no_legacy(port: u16) -> tokio::task::JoinHandle<()> {
    // GCP_PROJECT_ID unset + AURA_PROXY_AUTH_TOKEN unset = in-memory device store, no legacy
    std::env::remove_var("AURA_PROXY_AUTH_TOKEN");
    std::env::remove_var("GCP_PROJECT_ID");
    std::env::set_var("LEGACY_AUTH_ENABLED", "false");
    let handle = tokio::spawn(async move {
        aura_proxy::run_server(port).await;
    });
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    handle
}

#[tokio::test]
async fn test_ws_auth_with_device_token() {
    let _lock = ENV_MUTEX.lock().unwrap();
    let port = free_port();
    let handle = start_server_no_legacy(port).await;

    let client = reqwest::Client::new();

    // Register a device (Gemini key validation will fail against real API,
    // so this test requires the validation to be mockable or skipped in test mode.
    // For integration, set SKIP_GEMINI_VALIDATION=true in test env.)
    std::env::set_var("SKIP_GEMINI_VALIDATION", "true");
    let resp = client
        .post(format!("http://127.0.0.1:{port}/register"))
        .json(&serde_json::json!({
            "device_id": "test-dev-1",
            "gemini_api_key": "AIzaSyTestKey1234567890abc"
        }))
        .send().await.unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let device_token = body["device_token"].as_str().unwrap();

    // Now test /ws/auth with device headers
    let resp = client
        .get(format!("http://127.0.0.1:{port}/ws/auth"))
        .header("x-device-id", "test-dev-1")
        .header("x-device-token", device_token)
        .header("x-gemini-key", "AIzaSyTestKey1234567890abc")
        .send().await.unwrap();
    assert_eq!(resp.status(), 200);

    std::env::remove_var("SKIP_GEMINI_VALIDATION");
    std::env::remove_var("LEGACY_AUTH_ENABLED");
    handle.abort();
}

#[tokio::test]
async fn test_ws_auth_rejects_invalid_device_token() {
    let _lock = ENV_MUTEX.lock().unwrap();
    let port = free_port();
    let handle = start_server_no_legacy(port).await;

    let client = reqwest::Client::new();
    let resp = client
        .get(format!("http://127.0.0.1:{port}/ws/auth"))
        .header("x-device-id", "nonexistent-device")
        .header("x-device-token", "bad-token")
        .header("x-gemini-key", "AIzaSyTestKey1234567890abc")
        .send().await.unwrap();
    assert_eq!(resp.status(), 401);

    std::env::remove_var("LEGACY_AUTH_ENABLED");
    handle.abort();
}
```

Note: Add `SKIP_GEMINI_VALIDATION` env var support to `register_handler` — when set to `"true"`, skip the `GET /v1beta/models` call. This is test-only; production never sets it.

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p aura-proxy test_ws_auth_with_device`
Expected: FAIL

- [ ] **Step 3: Update `check_auth` and `ws_handler` for dual auth**

Modify `check_auth` in `lib.rs` to accept either:
- Legacy: `x-auth-token` header matched against `AURA_PROXY_AUTH_TOKEN` (when `LEGACY_AUTH_ENABLED`)
- Device: `x-device-id` + `x-device-token` headers validated against DeviceStore

Update `extract_connect_params` to read `x-device-id` and `x-device-token` in addition to existing headers. Update `ws_handler_with_sem` to pass the new headers to a dual-auth `check_auth_dual` function.

Update `ws_auth_preflight_with_token` similarly.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p aura-proxy`
Expected: All tests PASS (legacy + device token tests)

- [ ] **Step 5: Commit**

```bash
git add crates/aura-proxy/
git commit -m "feat: support device token auth on /ws with legacy fallback"
```

### Task 5: Update Firestore rules

**Files:**
- Modify: `infrastructure/firestore.rules`

- [ ] **Step 1: Add devices collection rule**

```
rules_version = '2';
service cloud.firestore {
  match /databases/{database}/documents {
    // Default deny all
    match /{document=**} {
      allow read, write: if false;
    }

    // Device registry — server-side only (Admin SDK bypasses rules)
    match /devices/{deviceId} {
      allow read, write: if false;
    }

    // User data — any authenticated client
    match /users/{deviceId}/{document=**} {
      allow read, write: if request.auth != null;
    }
  }
}
```

- [ ] **Step 2: Commit**

```bash
git add infrastructure/firestore.rules
git commit -m "feat: add devices collection to Firestore rules"
```

---

## Chunk 2: Memory Agent + Consolidation Service Auth Update

### Task 6: Update memory agent for device token auth

**Files:**
- Modify: `infrastructure/memory-agent/server.py:87-106`
- Modify: `infrastructure/memory-agent/config.py:13`
- Test: `infrastructure/memory-agent/tests/test_server.py`

- [ ] **Step 1: Write failing test for device token auth**

In `tests/test_server.py`, add:

```python
def test_ingest_with_device_token(self):
    """Device token auth should work when validated against Firestore."""
    # This test uses a mock Firestore to validate device tokens
    with patch("server._validate_device_token") as mock_validate:
        mock_validate.return_value = True
        with patch("server._run_agent") as mock_run:
            mock_run.return_value = {"summary": "test", "facts": ["fact1"]}
            resp = client.post(
                "/ingest",
                headers={"Authorization": "Bearer device-token-123"},
                json={
                    "device_id": "dev-test",
                    "session_id": "sess-1",
                    "messages": [{"role": "user", "content": "hello", "timestamp": "2026-01-01T00:00:00Z"}],
                },
            )
            assert resp.status_code == 200

def test_legacy_auth_disabled_rejects_shared_token(self):
    """When LEGACY_AUTH_ENABLED=false, shared tokens should be rejected."""
    with patch("server.LEGACY_AUTH_ENABLED", False):
        with patch("server._validate_device_token") as mock_validate:
            mock_validate.return_value = False
            resp = client.post(
                "/ingest",
                headers=AUTH_HEADERS,  # shared token
                json={"device_id": "dev-test", "session_id": "s1", "messages": []},
            )
            assert resp.status_code == 401
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cd infrastructure/memory-agent && python -m pytest tests/test_server.py -k "device_token or legacy_auth_disabled" -v`
Expected: FAIL

- [ ] **Step 3: Implement dual auth in memory agent**

Modify `config.py` — add:
```python
LEGACY_AUTH_ENABLED = os.environ.get("LEGACY_AUTH_ENABLED", "true").lower() == "true"
GCP_PROJECT_ID = os.environ.get("GCP_PROJECT_ID", "")
```

Modify `server.py` — update `_check_auth`:
```python
import hashlib
import google.auth
import google.auth.transport.requests

_device_cache: dict[str, tuple[str, float]] = {}
CACHE_TTL = 60.0

async def _validate_device_token(device_id: str, token: str) -> bool:
    """Validate a device token against Firestore devices collection."""
    import time
    cache_key = device_id
    now = time.time()

    # Check cache
    if cache_key in _device_cache:
        cached_hash, cached_at = _device_cache[cache_key]
        if now - cached_at < CACHE_TTL:
            provided_hash = hashlib.sha256(token.encode()).hexdigest()
            return hmac.compare_digest(provided_hash, cached_hash)

    # Cache miss — read from Firestore
    try:
        credentials, project = google.auth.default()
        credentials.refresh(google.auth.transport.requests.Request())
        url = f"https://firestore.googleapis.com/v1/projects/{config.GCP_PROJECT_ID}/databases/(default)/documents/devices/{device_id}"
        resp = httpx.get(url, headers={"Authorization": f"Bearer {credentials.token}"}, timeout=5.0)
        if resp.status_code != 200:
            return False
        fields = resp.json().get("fields", {})
        stored_hash = fields.get("token_hash", {}).get("stringValue", "")
        _device_cache[cache_key] = (stored_hash, now)
        provided_hash = hashlib.sha256(token.encode()).hexdigest()
        return hmac.compare_digest(provided_hash, stored_hash)
    except Exception as e:
        logger.error(f"Device token validation failed: {e}")
        return False


def _check_auth(request: Request) -> str:
    """Validate auth. Returns device_id if device token, or 'legacy' if shared token."""
    auth = request.headers.get("authorization", "")
    if not auth.startswith("Bearer "):
        raise HTTPException(status_code=401, detail="Missing auth")
    token = auth[7:]

    # Try legacy auth first (if enabled)
    if config.LEGACY_AUTH_ENABLED and config.AUTH_TOKEN:
        provided = hashlib.sha256(token.encode()).hexdigest()
        expected = hashlib.sha256(config.AUTH_TOKEN.encode()).hexdigest()
        if hmac.compare_digest(provided, expected):
            return "legacy"

    # Try device token auth — need device_id from request body
    # This is called before body parsing, so we defer device validation
    # to the endpoint handlers which have access to device_id
    raise HTTPException(status_code=401, detail="Invalid token")
```

Since the memory agent needs `device_id` from the request body to validate device tokens, restructure auth into two phases:

1. `_extract_bearer_token(request)` — extracts the raw token from the Authorization header (no validation)
2. `_check_auth_with_device(token, device_id)` — tries legacy first, then device token via Firestore

Each endpoint handler calls `_extract_bearer_token`, parses the body to get `device_id`, then calls `_check_auth_with_device`. Example for `/ingest`:

```python
def _extract_bearer_token(request: Request) -> str:
    auth = request.headers.get("authorization", "")
    if not auth.startswith("Bearer "):
        raise HTTPException(status_code=401, detail="Missing auth")
    return auth[7:]

async def _check_auth_with_device(token: str, device_id: str) -> None:
    """Validate token. Tries legacy shared token first, then device token via Firestore."""
    # Legacy check
    if config.LEGACY_AUTH_ENABLED and config.AUTH_TOKEN:
        provided = hashlib.sha256(token.encode()).hexdigest()
        expected = hashlib.sha256(config.AUTH_TOKEN.encode()).hexdigest()
        if hmac.compare_digest(provided, expected):
            return  # Legacy auth passed

    # Device token check
    if await _validate_device_token(device_id, token):
        return

    raise HTTPException(status_code=401, detail="Invalid token")

@app.post("/ingest")
async def ingest(request: Request):
    token = _extract_bearer_token(request)
    body = await request.json()
    device_id = body.get("device_id", "")
    config.validate_id(device_id, "device_id")
    await _check_auth_with_device(token, device_id)
    # ... rest of handler
```

Apply the same pattern to `/query` and `/consolidate`.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cd infrastructure/memory-agent && python -m pytest tests/test_server.py -v`
Expected: All PASS

- [ ] **Step 5: Commit**

```bash
git add infrastructure/memory-agent/
git commit -m "feat: add device token auth to memory agent with legacy fallback"
```

### Task 7: Update consolidation service for device token auth

**Files:**
- Modify: `infrastructure/consolidation/src/main.rs:43-50,152-171,219-232`
- Modify: `infrastructure/consolidation/Cargo.toml`

- [ ] **Step 1: Add dependencies**

Add to `infrastructure/consolidation/Cargo.toml`:
```toml
hex = "0.4"
```

- [ ] **Step 2: Update AppState and auth logic**

Modify `infrastructure/consolidation/src/main.rs`:

1. Add `legacy_auth_enabled: bool` and `gcp_project_id: String` to `AppState` (line 43-50)
2. Read `LEGACY_AUTH_ENABLED` env var in `main` (line 97-142)
3. Update `consolidate` handler (line 152-171) to try legacy auth first, then device token via Firestore
4. Add `validate_device_token` function similar to proxy's approach (Firestore REST + cache)

Key changes in the `consolidate` handler:
```rust
// Try legacy auth
if state.legacy_auth_enabled {
    if constant_time_eq(&bearer_token, &state.auth_token) {
        // Legacy auth passed
    }
}
// Try device token auth
if !legacy_passed {
    let device_id = &req.device_id;
    if !validate_device_token(&state, device_id, &bearer_token).await {
        return (StatusCode::UNAUTHORIZED, "Invalid token").into_response();
    }
}
```

- [ ] **Step 3: Verify compilation**

Run: `cd infrastructure/consolidation && cargo check`
Expected: Compiles

- [ ] **Step 4: Commit**

```bash
git add infrastructure/consolidation/
git commit -m "feat: add device token auth to consolidation service with legacy fallback"
```

---

## Chunk 3: Client-Side — Swift Keychain + Registration

### Task 8: Create KeychainHelper in Swift

**Files:**
- Create: `AuraApp/Sources/KeychainHelper.swift`

- [ ] **Step 1: Implement KeychainHelper**

```swift
import Foundation
import Security

enum KeychainHelper {
    static let service = "com.aura.desktop"

    static func save(account: String, data: Data) -> Bool {
        // Delete existing item first
        let deleteQuery: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: account,
        ]
        SecItemDelete(deleteQuery as CFDictionary)

        let addQuery: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: account,
            kSecValueData as String: data,
            kSecAttrAccessible as String: kSecAttrAccessibleAfterFirstUnlock,
        ]
        return SecItemAdd(addQuery as CFDictionary, nil) == errSecSuccess
    }

    static func read(account: String) -> Data? {
        let query: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: account,
            kSecReturnData as String: true,
            kSecMatchLimit as String: kSecMatchLimitOne,
        ]
        var result: AnyObject?
        let status = SecItemCopyMatching(query as CFDictionary, &result)
        guard status == errSecSuccess else { return nil }
        return result as? Data
    }

    static func delete(account: String) -> Bool {
        let query: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: account,
        ]
        return SecItemDelete(query as CFDictionary) == errSecSuccess
    }

    // Convenience: save/read strings
    static func saveString(account: String, value: String) -> Bool {
        guard let data = value.data(using: .utf8) else { return false }
        return save(account: account, data: data)
    }

    static func readString(account: String) -> String? {
        guard let data = read(account: account) else { return nil }
        return String(data: data, encoding: .utf8)
    }
}
```

- [ ] **Step 2: Verify it compiles**

Run: `cd AuraApp && swift build 2>&1 | tail -3`
Expected: Build succeeds

- [ ] **Step 3: Commit**

```bash
git add AuraApp/Sources/KeychainHelper.swift
git commit -m "feat: add KeychainHelper for secure device token storage"
```

### Task 9: Add device registration to onboarding flow

**Files:**
- Modify: `AuraApp/Sources/WelcomeView.swift:240-270`
- Modify: `AuraApp/Sources/AppState.swift:88-95`

- [ ] **Step 1: Update saveAPIKey to generate device_id and register**

In `WelcomeView.swift`, modify `saveAPIKey()` (line 240):

After writing the API key to config.toml, add:
1. Generate UUID device_id
2. Write `device_id` to config.toml
3. Call `/register` on the proxy
4. Store returned token in Keychain

```swift
func saveAPIKey() {
    let trimmed = apiKey.trimmingCharacters(in: .whitespacesAndNewlines)
    guard !trimmed.isEmpty else { return }

    let configDir = FileManager.default.homeDirectoryForCurrentUser
        .appendingPathComponent(".config/aura")
    try? FileManager.default.createDirectory(at: configDir, withIntermediateDirectories: true,
                                              attributes: [.posixPermissions: 0o700])

    let configPath = configDir.appendingPathComponent("config.toml")

    // Generate device ID if not already present
    let deviceId = existingDeviceId(at: configPath) ?? UUID().uuidString.lowercased()

    var config = "api_key = \"\(trimmed)\"\ndevice_id = \"\(deviceId)\"\n"
    try? config.write(to: configPath, atomically: true, encoding: .utf8)
    try? FileManager.default.setAttributes([.posixPermissions: 0o600], ofItemAtPath: configPath.path)

    // Register device in background (non-blocking — onboarding continues even if this fails)
    Task {
        await registerDevice(apiKey: trimmed, deviceId: deviceId)
    }

    onContinue()
}

private func existingDeviceId(at path: URL) -> String? {
    guard let content = try? String(contentsOf: path, encoding: .utf8) else { return nil }
    for line in content.components(separatedBy: "\n") {
        let parts = line.split(separator: "=", maxSplits: 1)
        if parts.count == 2 && parts[0].trimmingCharacters(in: .whitespaces) == "device_id" {
            return parts[1].trimmingCharacters(in: .whitespaces).replacingOccurrences(of: "\"", with: "")
        }
    }
    return nil
}

private func registerDevice(apiKey: String, deviceId: String) async {
    // Read proxy URL from config.toml (proxy_url field), converting wss:// to https://
    // Falls back to compiled default if not in config
    let proxyBase = readProxyBaseURL() ?? "https://aura-proxy-prod-877110560858.us-central1.run.app"

    guard let url = URL(string: "\(proxyBase)/register") else { return }
    var request = URLRequest(url: url)
    request.httpMethod = "POST"
    request.setValue("application/json", forHTTPHeaderField: "Content-Type")
    request.httpBody = try? JSONSerialization.data(withJSONObject: [
        "device_id": deviceId,
        "gemini_api_key": apiKey,
    ])

    do {
        let (data, response) = try await URLSession.shared.data(for: request)
        guard let httpResp = response as? HTTPURLResponse, httpResp.statusCode == 200 else {
            print("[Aura] Device registration failed: HTTP \((response as? HTTPURLResponse)?.statusCode ?? 0)")
            return
        }
        guard let json = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
              let token = json["device_token"] as? String else { return }

        if KeychainHelper.saveString(account: "device_token", value: token) {
            print("[Aura] Device registered successfully")
        }
    } catch {
        print("[Aura] Device registration error: \(error.localizedDescription)")
    }
}
```

- [ ] **Step 2: Verify it compiles**

Run: `cd AuraApp && swift build 2>&1 | tail -3`
Expected: Build succeeds

- [ ] **Step 3: Commit**

```bash
git add AuraApp/Sources/WelcomeView.swift
git commit -m "feat: register device and store token in Keychain during onboarding"
```

---

## Chunk 4: Rust Daemon — Keychain Integration + Config Cleanup

### Task 10: Add Keychain reading to aura-gemini config

**Files:**
- Modify: `crates/aura-gemini/Cargo.toml`
- Modify: `crates/aura-gemini/src/config.rs:248-340`
- Test: `crates/aura-gemini/tests/session_test.rs` (or inline tests)

- [ ] **Step 1: Add security-framework dependency**

In `crates/aura-gemini/Cargo.toml`:
```toml
security-framework = "3"
```

- [ ] **Step 2: Write failing test for Keychain fallback**

In `crates/aura-gemini/src/config.rs` tests section:

```rust
#[test]
fn test_prod_default_filters_empty_strings() {
    assert_eq!(prod_default(Some("value")), Some("value".to_string()));
    assert_eq!(prod_default(Some("")), None);
    assert_eq!(prod_default(None), None);
}
```

- [ ] **Step 3: Run test to verify it passes** (this one should already pass from earlier fix)

Run: `cargo test -p aura-gemini test_prod_default_filters`
Expected: PASS

- [ ] **Step 4: Implement read_keychain_token**

Add to `config.rs`:

```rust
/// Read the device token from macOS Keychain.
fn read_keychain_token() -> Option<String> {
    use security_framework::passwords::get_generic_password;
    match get_generic_password("com.aura.desktop", "device_token") {
        Ok(bytes) => String::from_utf8(bytes.to_vec()).ok(),
        Err(_) => None,
    }
}
```

- [ ] **Step 5: Update GeminiConfig to use device_token**

Replace `proxy_auth_token` and `cloud_run_auth_token` with a single `device_token` field:

```rust
#[derive(Clone)]
pub struct GeminiConfig {
    pub api_key: String,
    pub model: String,
    pub voice: String,
    pub system_prompt: String,
    pub temperature: f64,
    pub proxy_url: Option<String>,
    pub device_id: Option<String>,
    pub device_token: Option<String>,
    pub cloud_run_url: Option<String>,
    pub firestore_project_id: Option<String>,
    pub firebase_api_key: Option<String>,
}
```

Update `from_env()`:
```rust
// Device token: env var > Keychain (config.toml not used for tokens)
config.device_token = std::env::var("AURA_DEVICE_TOKEN")
    .ok()
    .filter(|s| !s.is_empty())
    .or_else(read_keychain_token);
```

Update `ws_headers()` to send `x-device-id` + `x-device-token` instead of `x-auth-token`:
```rust
pub fn ws_headers(&self) -> Vec<(String, String)> {
    if self.proxy_url.is_none() {
        return Vec::new();
    }
    let mut headers = vec![("x-gemini-key".to_string(), self.api_key.clone())];
    if let Some(ref id) = self.device_id {
        headers.push(("x-device-id".to_string(), id.clone()));
    }
    if let Some(ref token) = self.device_token {
        headers.push(("x-device-token".to_string(), token.clone()));
    }
    headers
}
```

Update `Debug` impl to redact `device_token`.

Remove `prod_defaults::PROXY_AUTH_TOKEN` and `CLOUD_RUN_AUTH_TOKEN`. Keep URL defaults.

- [ ] **Step 6: Update existing tests**

Update tests that reference `proxy_auth_token` to use `device_token`. Update `test_ws_headers_proxy_mode` to check for `x-device-id` and `x-device-token` instead of `x-auth-token`.

- [ ] **Step 7: Run all tests**

Run: `cargo test -p aura-gemini`
Expected: All PASS

- [ ] **Step 8: Commit**

```bash
git add crates/aura-gemini/
git commit -m "feat: replace shared auth tokens with device token from Keychain"
```

### Task 11: Update daemon CloudConfig and processor

**Files:**
- Modify: `crates/aura-daemon/src/context.rs:16-23`
- Modify: `crates/aura-daemon/src/processor.rs:25-59,62-119`
- Modify: `crates/aura-daemon/src/main.rs` (where CloudConfig is populated)

- [ ] **Step 1: Update CloudConfig struct**

In `context.rs`, replace `cloud_run_auth_token` with `device_token`:

```rust
pub struct CloudConfig {
    pub gemini_api_key: String,
    pub cloud_run_url: Option<String>,
    pub device_token: Option<String>,
    pub device_id: Option<String>,
    pub firestore_project_id: Option<String>,
    pub firebase_api_key: Option<String>,
}
```

- [ ] **Step 2: Update processor to use device_token**

In `processor.rs`, update `query_memory_agent` and `ingest_to_memory_agent`:
- Change `cloud_run_auth_token` parameter to `device_token`
- The `.bearer_auth()` call stays the same — just uses `device_token` now

- [ ] **Step 3: Update main.rs where CloudConfig is populated**

Change the field mapping from `gemini_config.cloud_run_auth_token` to `gemini_config.device_token`.

- [ ] **Step 4: Add background registration retry**

In the daemon startup (after `GeminiConfig::from_env()`), add:

```rust
// If device_id exists but no device_token, try background registration
if gemini_config.device_id.is_some() && gemini_config.device_token.is_none() {
    if let Some(ref proxy_url) = gemini_config.proxy_url {
        let api_key = gemini_config.api_key.clone();
        let device_id = gemini_config.device_id.clone().unwrap();
        let proxy_base = proxy_url.replace("/ws", "").replace("wss://", "https://");
        tokio::spawn(async move {
            tracing::info!("Device token missing, attempting background registration");
            let client = reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(10))
                .build()
                .unwrap();
            let resp = match client
                .post(format!("{proxy_base}/register"))
                .json(&serde_json::json!({
                    "device_id": device_id,
                    "gemini_api_key": api_key,
                }))
                .send()
                .await
            {
                Ok(r) => r,
                Err(e) => {
                    tracing::warn!("Background registration failed: {e}");
                    return;
                }
            };
            if !resp.status().is_success() {
                tracing::warn!("Background registration returned {}", resp.status());
                return;
            }
            let json: serde_json::Value = match resp.json().await {
                Ok(j) => j,
                Err(e) => {
                    tracing::warn!("Background registration bad response: {e}");
                    return;
                }
            };
            if let Some(token) = json.get("device_token").and_then(|v| v.as_str()) {
                // Store in Keychain
                use security_framework::passwords::set_generic_password;
                match set_generic_password("com.aura.desktop", "device_token", token.as_bytes()) {
                    Ok(_) => tracing::info!("Device registered and token stored in Keychain"),
                    Err(e) => tracing::warn!("Failed to store device token in Keychain: {e}"),
                }
            }
        });
    }
}
```

- [ ] **Step 5: Verify compilation**

Run: `cargo check -p aura-daemon`
Expected: Compiles

- [ ] **Step 6: Commit**

```bash
git add crates/aura-daemon/
git commit -m "feat: update daemon to use device token from Keychain"
```

---

## Chunk 5: Cleanup + Release Workflow

### Task 12: Remove shared token secrets from CI

**Files:**
- Modify: `.github/workflows/release.yml`

- [ ] **Step 1: Remove auth token env vars from release build step**

In `.github/workflows/release.yml`, remove `AURA_PROD_PROXY_AUTH_TOKEN` and `AURA_PROD_CLOUD_RUN_AUTH_TOKEN` from the build step env vars. Keep `AURA_PROD_PROXY_URL` and `AURA_PROD_CLOUD_RUN_URL`.

```yaml
      - name: Build Rust daemon (release)
        env:
          AURA_PROD_PROXY_URL: ${{ secrets.AURA_PROD_PROXY_URL }}
          AURA_PROD_CLOUD_RUN_URL: ${{ secrets.AURA_PROD_CLOUD_RUN_URL }}
        run: cargo build --release --locked -p aura-daemon
```

- [ ] **Step 2: Commit**

```bash
git add .github/workflows/release.yml
git commit -m "chore: remove shared auth token secrets from release build"
```

### Task 13: Delete GitHub secrets for auth tokens

- [ ] **Step 1: Delete the 4 auth token secrets**

```bash
gh secret delete AURA_PROD_PROXY_AUTH_TOKEN
gh secret delete AURA_PROD_CLOUD_RUN_AUTH_TOKEN
gh secret delete AURA_STAGING_PROXY_AUTH_TOKEN
gh secret delete AURA_STAGING_CLOUD_RUN_AUTH_TOKEN
```

- [ ] **Step 2: Verify remaining secrets**

```bash
gh secret list
```

Expected: Only URL secrets and GCP credentials remain.

### Task 14: Update local dev config

- [ ] **Step 1: Update `~/.config/aura/config.toml`**

Remove `proxy_auth_token` and `cloud_run_auth_token`. Add `device_id`:

```toml
api_key = "AIzaSy..."
device_id = "dev-abdullahi"

# Staging cloud services (for local dev)
proxy_url = "wss://aura-proxy-staging-877110560858.us-central1.run.app/ws"
cloud_run_url = "https://aura-memory-agent-staging-877110560858.us-central1.run.app"
```

- [ ] **Step 2: Register dev device manually**

```bash
curl -X POST https://aura-proxy-staging-877110560858.us-central1.run.app/register \
  -H "Content-Type: application/json" \
  -d '{"device_id": "dev-abdullahi", "gemini_api_key": "AIzaSy..."}'
```

Take the returned `device_token` and store in Keychain:
```bash
security add-generic-password -s "com.aura.desktop" -a "device_token" -w "<TOKEN>" -U
```

### Task 15: Full integration test

- [ ] **Step 1: Run all Rust tests**

```bash
cargo test --workspace
```

Expected: All PASS

- [ ] **Step 2: Run memory agent tests**

```bash
cd infrastructure/memory-agent && python -m pytest tests/ -v
```

Expected: All PASS

- [ ] **Step 3: Build and test locally**

```bash
bash scripts/dev.sh
```

Verify in logs:
- Daemon reads device_token from Keychain
- Connects via proxy (staging)
- Memory agent queries/ingests succeed

- [ ] **Step 4: Final commit**

```bash
git add crates/ infrastructure/ AuraApp/ .github/ docs/
git commit -m "feat: per-device token registration — complete implementation"
```

---

## Implementation Notes

### Rate Limiting `/register`

The spec requires 3 requests per IP per hour on `/register`. Since the proxy runs on Cloud Run (potentially multiple instances), an in-memory rate limiter won't work across instances. For the current small user base, implement a simple in-memory `HashMap<IpAddr, Vec<Instant>>` rate limiter in the proxy. Add a comment noting it should be replaced with Cloud Armor or Redis when scaling. This is added in Task 3 as part of the `register_handler` — check the IP from `axum::extract::ConnectInfo<SocketAddr>` and reject with 429 if the limit is exceeded.

### Gemini Key Validation in Tests

The `/register` endpoint validates Gemini keys against Google's live API. For tests, the plan uses a `SKIP_GEMINI_VALIDATION` env var that bypasses this check. This is only read in the proxy code and must never be set in production. The deploy workflow does not set it.
