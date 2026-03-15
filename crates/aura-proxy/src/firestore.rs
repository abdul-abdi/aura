//! Device registration and token validation.
//!
//! Provides a `DeviceStore` backed by a pluggable `DeviceBackend` trait.
//! This module contains the in-memory backend; a Firestore REST backend
//! will be wired in as Task 2.

use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;
use tokio::sync::RwLock;

// ── Cache TTL ────────────────────────────────────────────────────────────────

const CACHE_TTL: Duration = Duration::from_secs(60);

// ── DeviceRecord ─────────────────────────────────────────────────────────────

/// Persisted device metadata. Fields contain hashed/non-sensitive values.
#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct DeviceRecord {
    /// SHA-256 hex of the device's current auth token.
    pub token_hash: String,
    /// SHA-256 hex of the device's Gemini API key (already hashed by caller).
    pub gemini_key_hash: String,
    /// ISO-8601 creation timestamp.
    pub created_at: String,
    /// ISO-8601 timestamp of last successful auth (updated on cache miss).
    pub last_seen: String,
}

/// Custom Debug that redacts the token hash.
impl fmt::Debug for DeviceRecord {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DeviceRecord")
            .field("token_hash", &"[REDACTED]")
            .field("gemini_key_hash", &"[REDACTED]")
            .field("created_at", &self.created_at)
            .finish()
    }
}

// ── DeviceBackend trait ───────────────────────────────────────────────────────

/// Storage backend for device records.
#[async_trait]
pub trait DeviceBackend: Send + Sync + 'static {
    /// Retrieve a device record by ID. Returns `None` if not found.
    async fn get_device(&self, device_id: &str) -> Option<DeviceRecord>;

    /// Persist (insert or overwrite) a device record.
    async fn set_device(&self, device_id: &str, record: DeviceRecord);
}

// ── InMemoryBackend ───────────────────────────────────────────────────────────

/// Thread-safe in-memory device store. Useful for tests and local development.
pub struct InMemoryBackend {
    store: Arc<RwLock<HashMap<String, DeviceRecord>>>,
}

impl InMemoryBackend {
    fn new() -> Self {
        Self {
            store: Arc::new(RwLock::new(HashMap::new())),
        }
    }
}

#[async_trait]
impl DeviceBackend for InMemoryBackend {
    async fn get_device(&self, device_id: &str) -> Option<DeviceRecord> {
        self.store.read().await.get(device_id).cloned()
    }

    async fn set_device(&self, device_id: &str, record: DeviceRecord) {
        self.store
            .write()
            .await
            .insert(device_id.to_string(), record);
    }
}

// ── FirestoreBackend ─────────────────────────────────────────────────────────

/// Firestore REST API backend for device storage. Production use on Cloud Run.
pub struct FirestoreBackend {
    client: reqwest::Client,
    project_id: String,
}

impl FirestoreBackend {
    /// Create a new Firestore backend with the given project ID.
    pub fn new(project_id: String) -> Self {
        Self {
            client: reqwest::Client::new(),
            project_id,
        }
    }

    /// Fetch the GCP service account access token from the metadata server.
    async fn get_access_token(&self) -> Result<String, String> {
        const METADATA_URL: &str =
            "http://metadata.google.internal/computeMetadata/v1/instance/service-accounts/default/token";

        let response = self
            .client
            .get(METADATA_URL)
            .header("Metadata-Flavor", "Google")
            .send()
            .await
            .map_err(|e| format!("metadata server request failed: {e}"))?;

        let json: serde_json::Value = response
            .json()
            .await
            .map_err(|e| format!("failed to parse metadata response: {e}"))?;

        json["access_token"]
            .as_str()
            .map(|s| s.to_string())
            .ok_or_else(|| "access_token field missing in metadata response".to_string())
    }

    /// Format the Firestore REST API document URL for a device.
    fn doc_url(&self, device_id: &str) -> String {
        format!(
            "https://firestore.googleapis.com/v1/projects/{}/databases/(default)/documents/devices/{}",
            self.project_id, device_id
        )
    }
}

#[async_trait]
impl DeviceBackend for FirestoreBackend {
    async fn get_device(&self, device_id: &str) -> Option<DeviceRecord> {
        let access_token = match self.get_access_token().await {
            Ok(token) => token,
            Err(e) => {
                tracing::error!(device_id, error = %e, "failed to get access token");
                return None;
            }
        };

        let url = self.doc_url(device_id);

        let response = match self
            .client
            .get(&url)
            .header("Authorization", format!("Bearer {access_token}"))
            .send()
            .await
        {
            Ok(r) => r,
            Err(e) => {
                tracing::error!(device_id, error = %e, "firestore get request failed");
                return None;
            }
        };

        let json: serde_json::Value = match response.json().await {
            Ok(j) => j,
            Err(e) => {
                tracing::error!(device_id, error = %e, "failed to parse firestore response");
                return None;
            }
        };

        // Extract fields from the Firestore document structure.
        let fields = json.get("fields")?;

        let token_hash = fields
            .get("token_hash")?
            .get("stringValue")?
            .as_str()?
            .to_string();

        let gemini_key_hash = fields
            .get("gemini_key_hash")?
            .get("stringValue")?
            .as_str()?
            .to_string();

        let created_at = fields
            .get("created_at")?
            .get("stringValue")?
            .as_str()?
            .to_string();

        let last_seen = fields
            .get("last_seen")?
            .get("stringValue")?
            .as_str()?
            .to_string();

        Some(DeviceRecord {
            token_hash,
            gemini_key_hash,
            created_at,
            last_seen,
        })
    }

    async fn set_device(&self, device_id: &str, record: DeviceRecord) {
        let access_token = match self.get_access_token().await {
            Ok(token) => token,
            Err(e) => {
                tracing::error!(device_id, error = %e, "failed to get access token for set");
                return;
            }
        };

        let url = self.doc_url(device_id);

        let body = serde_json::json!({
            "fields": {
                "token_hash": { "stringValue": record.token_hash },
                "gemini_key_hash": { "stringValue": record.gemini_key_hash },
                "created_at": { "stringValue": record.created_at },
                "last_seen": { "stringValue": record.last_seen }
            }
        });

        let query_url = format!(
            "{}?updateMask.fieldPaths=token_hash&updateMask.fieldPaths=gemini_key_hash&updateMask.fieldPaths=created_at&updateMask.fieldPaths=last_seen",
            url
        );

        if let Err(e) = self
            .client
            .patch(&query_url)
            .header("Authorization", format!("Bearer {access_token}"))
            .json(&body)
            .send()
            .await
        {
            tracing::error!(device_id, error = %e, "firestore patch request failed");
        }
    }
}

// ── CachedRecord ─────────────────────────────────────────────────────────────

/// A cached snapshot of the token hash for a device.
struct CachedRecord {
    token_hash: String,
    fetched_at: Instant,
}

/// Custom Debug that redacts the token hash.
impl fmt::Debug for CachedRecord {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CachedRecord")
            .field("token_hash", &"[REDACTED]")
            .field("fetched_at", &self.fetched_at)
            .finish()
    }
}

// ── RegisterError ─────────────────────────────────────────────────────────────

/// Errors returned by `DeviceStore::register_device`.
#[derive(Debug)]
pub enum RegisterError {
    /// The device exists but the supplied Gemini key hash does not match.
    KeyMismatch,
    /// An unexpected backend error occurred.
    BackendError(String),
}

impl fmt::Display for RegisterError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::KeyMismatch => write!(f, "gemini key hash mismatch for existing device"),
            Self::BackendError(msg) => write!(f, "backend error: {msg}"),
        }
    }
}

impl std::error::Error for RegisterError {}

// ── DeviceStore ───────────────────────────────────────────────────────────────

/// High-level device registry with an in-process cache.
///
/// Cheaply cloneable — all state lives behind `Arc`.
#[derive(Clone)]
pub struct DeviceStore {
    backend: Arc<dyn DeviceBackend>,
    cache: Arc<RwLock<HashMap<String, CachedRecord>>>,
}

impl fmt::Debug for DeviceStore {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DeviceStore").finish_non_exhaustive()
    }
}

impl DeviceStore {
    /// Create a `DeviceStore` backed by the in-memory backend.
    pub fn new_in_memory() -> Self {
        Self {
            backend: Arc::new(InMemoryBackend::new()),
            cache: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Create a `DeviceStore` with a custom backend (used by the Firestore backend in Task 2).
    pub fn new(backend: Arc<dyn DeviceBackend>) -> Self {
        Self {
            backend,
            cache: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Create a `DeviceStore` backed by Firestore REST API.
    pub fn new_with_firestore(project_id: String) -> Self {
        Self::new(Arc::new(FirestoreBackend::new(project_id)))
    }

    /// Register a device and return a new plaintext auth token.
    ///
    /// - If the device does not yet exist, it is created.
    /// - If the device exists and `gemini_key_hash` matches, a fresh token is issued
    ///   (the old token is immediately invalidated).
    /// - If the device exists but `gemini_key_hash` does **not** match, returns
    ///   `RegisterError::KeyMismatch`.
    pub async fn register_device(
        &self,
        device_id: &str,
        gemini_key_hash: String,
    ) -> Result<String, RegisterError> {
        if let Some(existing) = self.backend.get_device(device_id).await {
            // Constant-time comparison to prevent timing side-channels.
            let existing_bytes = existing.gemini_key_hash.as_bytes();
            let supplied_bytes = gemini_key_hash.as_bytes();
            let matches = existing_bytes.ct_eq(supplied_bytes).unwrap_u8() == 1;
            if !matches {
                return Err(RegisterError::KeyMismatch);
            }
        }

        let token = generate_token();
        let token_hash = hash_value(&token);

        let now = chrono::Utc::now().to_rfc3339();
        let record = DeviceRecord {
            token_hash,
            gemini_key_hash,
            created_at: now.clone(),
            last_seen: now,
        };

        self.backend.set_device(device_id, record).await;

        // Invalidate any cached entry so the next validate_token fetches fresh.
        self.cache.write().await.remove(device_id);

        Ok(token)
    }

    /// Return `true` iff `token` is the current valid token for `device_id`.
    ///
    /// Uses a 60-second in-process cache to minimise backend reads. On a
    /// successful cache miss the `last_seen` timestamp is updated
    /// fire-and-forget via `tokio::spawn`.
    pub async fn validate_token(&self, device_id: &str, token: &str) -> bool {
        let token_hash = hash_value(token);

        // ── cache look-up ────────────────────────────────────────────────────
        {
            let cache = self.cache.read().await;
            if let Some(cached) = cache.get(device_id)
                && cached.fetched_at.elapsed() < CACHE_TTL
            {
                return constant_time_eq_str(&token_hash, &cached.token_hash);
            }
        }

        // ── cache miss — read from backend ───────────────────────────────────
        let record = match self.backend.get_device(device_id).await {
            Some(r) => r,
            None => return false,
        };

        let is_valid = constant_time_eq_str(&token_hash, &record.token_hash);

        // Update cache regardless of validity (avoids re-hitting backend on
        // repeated bad tokens for a known device).
        {
            let mut cache = self.cache.write().await;
            cache.insert(
                device_id.to_string(),
                CachedRecord {
                    token_hash: record.token_hash.clone(),
                    fetched_at: Instant::now(),
                },
            );
        }

        // Fire-and-forget last_seen update (best effort).
        if is_valid {
            let backend = Arc::clone(&self.backend);
            let device_id_owned = device_id.to_string();
            let mut updated = record.clone();
            tokio::spawn(async move {
                // Re-read to avoid clobbering concurrent writes.
                if let Some(current) = backend.get_device(&device_id_owned).await {
                    // Only update if the token hash still matches (no rotation race).
                    if current.token_hash == updated.token_hash {
                        updated.last_seen = chrono::Utc::now().to_rfc3339();
                        backend.set_device(&device_id_owned, updated).await;
                    }
                }
            });
        }

        is_valid
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// SHA-256 hex digest of `value`.
pub fn hash_value(value: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(value.as_bytes());
    hex::encode(hasher.finalize())
}

/// Generate a 64-character hex token (32 random bytes).
fn generate_token() -> String {
    use rand::Rng;
    let bytes: [u8; 32] = rand::rng().random();
    hex::encode(bytes)
}

/// Constant-time equality check for two hex-encoded SHA-256 strings.
fn constant_time_eq_str(a: &str, b: &str) -> bool {
    a.as_bytes().ct_eq(b.as_bytes()).unwrap_u8() == 1
}
