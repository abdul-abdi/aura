//! Firestore REST client for reading/writing facts and session summaries.

use crate::auth::AuthCache;
use anyhow::{Context, Result};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::sync::Arc;
use tracing::debug;

/// A fact stored in Firestore under `users/{device_id}/facts`.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct FirestoreFact {
    pub category: String,
    pub content: String,
    pub entities: Vec<String>,
    pub importance: f64,
    pub session_id: String,
}

/// Validate device_id is safe for use in Firestore URL paths.
/// Allows alphanumeric chars, hyphens, and underscores only.
pub fn validate_device_id(id: &str) -> Result<()> {
    if id.is_empty() || id.len() > 128 {
        anyhow::bail!("device_id must be 1-128 characters, got {}", id.len());
    }
    if !id
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        anyhow::bail!(
            "device_id must contain only alphanumeric characters, hyphens, and underscores"
        );
    }
    Ok(())
}

/// Thin Firestore REST client scoped to a single device's namespace.
pub struct FirestoreClient {
    project_id: String,
    device_id: String,
    client: reqwest::Client,
    auth_cache: Option<Arc<AuthCache>>,
}

impl FirestoreClient {
    pub fn new(project_id: String, device_id: String) -> Result<Self> {
        validate_device_id(&device_id)?;
        Ok(Self {
            project_id,
            device_id,
            client: reqwest::Client::new(),
            auth_cache: None,
        })
    }

    /// Create a client with a shared [`AuthCache`] for automatic token management.
    pub fn with_auth(
        project_id: String,
        device_id: String,
        auth_cache: Arc<AuthCache>,
    ) -> Result<Self> {
        validate_device_id(&device_id)?;
        Ok(Self {
            project_id,
            device_id,
            client: reqwest::Client::new(),
            auth_cache: Some(auth_cache),
        })
    }

    /// Get a valid auth token, either from the shared cache or requiring an explicit token.
    pub async fn get_token(&self) -> Result<String> {
        match &self.auth_cache {
            Some(cache) => cache.get_token().await,
            None => anyhow::bail!("no AuthCache configured; pass an explicit auth_token instead"),
        }
    }

    fn base_url(&self) -> String {
        format!(
            "https://firestore.googleapis.com/v1/projects/{}/databases/(default)/documents/users/{}",
            self.project_id, self.device_id
        )
    }

    /// Write (create or overwrite) a fact document.  The document name is
    /// derived from `fact.session_id` + a hash of the content so that
    /// re-syncing the same fact is idempotent.
    pub async fn write_fact(&self, fact: &FirestoreFact, auth_token: &str) -> Result<()> {
        let doc_id = fact_doc_id(fact);
        let url = format!("{}/facts/{}", self.base_url(), doc_id);
        let body = fact_to_firestore_doc(fact);

        debug!("write_fact → PATCH {url}");
        self.client
            .patch(&url)
            .bearer_auth(auth_token)
            .json(&body)
            .send()
            .await
            .context("write_fact: request failed")?
            .error_for_status()
            .context("write_fact: non-2xx response")?;
        Ok(())
    }

    /// Write a session summary document under `users/{device_id}/sessions/{session_id}`.
    pub async fn write_session(
        &self,
        session_id: &str,
        summary: &str,
        auth_token: &str,
    ) -> Result<()> {
        let url = format!("{}/sessions/{}", self.base_url(), session_id);
        let now = Utc::now().to_rfc3339();
        let body = json!({
            "fields": {
                "summary": {"stringValue": summary},
                "session_id": {"stringValue": session_id},
                "created_at": {"timestampValue": now}
            }
        });

        debug!("write_session → PATCH {url}");
        self.client
            .patch(&url)
            .bearer_auth(auth_token)
            .json(&body)
            .send()
            .await
            .context("write_session: request failed")?
            .error_for_status()
            .context("write_session: non-2xx response")?;
        Ok(())
    }

    /// Read all facts for this device.
    pub async fn read_facts(&self, auth_token: &str) -> Result<Vec<FirestoreFact>> {
        let url = format!("{}/facts", self.base_url());
        debug!("read_facts → GET {url}");

        let resp = self
            .client
            .get(&url)
            .bearer_auth(auth_token)
            .send()
            .await
            .context("read_facts: request failed")?;

        // Empty collection or non-existent path returns 404 — treat as empty
        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            debug!("read_facts: 404 (collection not yet created), returning empty");
            return Ok(Vec::new());
        }

        let resp: Value = resp
            .error_for_status()
            .context("read_facts: non-2xx response")?
            .json()
            .await
            .context("read_facts: failed to parse response")?;

        let docs = resp
            .get("documents")
            .and_then(|d| d.as_array())
            .cloned()
            .unwrap_or_default();

        let facts = docs
            .iter()
            .filter_map(|doc| match firestore_doc_to_fact(doc) {
                Ok(fact) => Some(fact),
                Err(e) => {
                    tracing::warn!("Failed to parse Firestore fact document: {e}");
                    None
                }
            })
            .collect();

        Ok(facts)
    }
}

// ---------------------------------------------------------------------------
// Firestore document conversion helpers
// ---------------------------------------------------------------------------

/// Convert a `FirestoreFact` into a Firestore REST document body.
pub fn fact_to_firestore_doc(fact: &FirestoreFact) -> Value {
    let now = Utc::now().to_rfc3339();
    let entities_array: Vec<Value> = fact
        .entities
        .iter()
        .map(|e| json!({"stringValue": e}))
        .collect();

    json!({
        "fields": {
            "category":   {"stringValue": fact.category},
            "content":    {"stringValue": fact.content},
            "entities":   {"arrayValue": {"values": entities_array}},
            "importance": {"doubleValue": fact.importance},
            "session_id": {"stringValue": fact.session_id},
            "created_at": {"timestampValue": now}
        }
    })
}

/// Parse a Firestore REST document into a `FirestoreFact`.
pub fn firestore_doc_to_fact(doc: &Value) -> Result<FirestoreFact> {
    let fields = doc.get("fields").context("document missing 'fields' key")?;

    let category = string_field(fields, "category")?;
    let content = string_field(fields, "content")?;
    let session_id = string_field(fields, "session_id")?;
    let importance = fields
        .get("importance")
        .and_then(|v| {
            v.get("doubleValue").and_then(|d| d.as_f64()).or_else(|| {
                // Firestore REST encodes integerValue as a string, e.g. {"integerValue": "1"}
                v.get("integerValue")
                    .and_then(|i| i.as_str())
                    .and_then(|s| s.parse::<f64>().ok())
            })
        })
        .unwrap_or(0.5);

    let entities = fields
        .get("entities")
        .and_then(|v| v.get("arrayValue"))
        .and_then(|v| v.get("values"))
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|e| {
                    e.get("stringValue")
                        .and_then(|s| s.as_str())
                        .map(String::from)
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    Ok(FirestoreFact {
        category,
        content,
        entities,
        importance,
        session_id,
    })
}

// ---------------------------------------------------------------------------
// Private helpers
// ---------------------------------------------------------------------------

fn string_field(fields: &Value, key: &str) -> Result<String> {
    fields
        .get(key)
        .and_then(|v| v.get("stringValue"))
        .and_then(|v| v.as_str())
        .map(String::from)
        .with_context(|| format!("document missing string field '{key}'"))
}

/// Deterministic document ID derived from category + content using FNV-1a.
/// FNV-1a is stable across binaries and Rust versions (unlike DefaultHasher).
fn fact_doc_id(fact: &FirestoreFact) -> String {
    let raw = format!("{}:{}", fact.category, fact.content);
    let mut hash: u64 = 0xcbf29ce484222325;
    for byte in raw.as_bytes() {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{:016x}", hash)
}

// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_fact() -> FirestoreFact {
        FirestoreFact {
            category: "preference".into(),
            content: "User prefers dark mode".into(),
            entities: vec!["dark mode".into(), "UI".into()],
            importance: 0.8,
            session_id: "sess-001".into(),
        }
    }

    #[test]
    fn roundtrip_fact_to_doc_and_back() {
        let fact = sample_fact();
        let doc = fact_to_firestore_doc(&fact);
        let parsed = firestore_doc_to_fact(&doc).unwrap();

        assert_eq!(parsed.category, fact.category);
        assert_eq!(parsed.content, fact.content);
        assert_eq!(parsed.importance, fact.importance);
        assert_eq!(parsed.session_id, fact.session_id);
        assert_eq!(parsed.entities, fact.entities);
    }

    #[test]
    fn roundtrip_empty_entities() {
        let fact = FirestoreFact {
            category: "event".into(),
            content: "Meeting started".into(),
            entities: vec![],
            importance: 0.5,
            session_id: "sess-002".into(),
        };
        let doc = fact_to_firestore_doc(&fact);
        let parsed = firestore_doc_to_fact(&doc).unwrap();
        assert!(parsed.entities.is_empty());
    }

    #[test]
    fn missing_string_field_returns_err() {
        let doc = serde_json::json!({
            "fields": {
                "content": {"stringValue": "some content"},
                "importance": {"doubleValue": 0.5},
                "session_id": {"stringValue": "s1"},
                "entities": {"arrayValue": {"values": []}}
                // "category" intentionally missing
            }
        });
        assert!(firestore_doc_to_fact(&doc).is_err());
    }

    #[test]
    fn missing_fields_key_returns_err() {
        let doc =
            serde_json::json!({"name": "projects/x/databases/(default)/documents/users/d/facts/f"});
        assert!(firestore_doc_to_fact(&doc).is_err());
    }

    #[test]
    fn importance_defaults_to_half_when_absent() {
        let doc = serde_json::json!({
            "fields": {
                "category":   {"stringValue": "note"},
                "content":    {"stringValue": "hello"},
                "session_id": {"stringValue": "s3"},
                "entities":   {"arrayValue": {"values": []}}
            }
        });
        let parsed = firestore_doc_to_fact(&doc).unwrap();
        assert_eq!(parsed.importance, 0.5);
    }

    #[test]
    fn importance_parses_integer_value() {
        // Firestore REST encodes integerValue as a string
        let doc = serde_json::json!({
            "fields": {
                "category":   {"stringValue": "note"},
                "content":    {"stringValue": "hello"},
                "session_id": {"stringValue": "s4"},
                "entities":   {"arrayValue": {"values": []}},
                "importance": {"integerValue": "1"}
            }
        });
        let parsed = firestore_doc_to_fact(&doc).unwrap();
        assert_eq!(parsed.importance, 1.0);
    }

    #[test]
    fn fact_doc_id_is_deterministic() {
        let fact = sample_fact();
        assert_eq!(fact_doc_id(&fact), fact_doc_id(&fact));
    }

    #[test]
    fn fact_doc_id_differs_for_different_content() {
        let f1 = sample_fact();
        let f2 = FirestoreFact {
            content: "different content".into(),
            ..f1.clone()
        };
        assert_ne!(fact_doc_id(&f1), fact_doc_id(&f2));
    }
}
