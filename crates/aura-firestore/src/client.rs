//! Firestore REST client for reading/writing facts and session summaries.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
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

/// Thin Firestore REST client scoped to a single device's namespace.
pub struct FirestoreClient {
    project_id: String,
    device_id: String,
    client: reqwest::Client,
}

impl FirestoreClient {
    pub fn new(project_id: String, device_id: String) -> Self {
        Self {
            project_id,
            device_id,
            client: reqwest::Client::new(),
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
        let body = json!({
            "fields": {
                "summary": {"stringValue": summary},
                "session_id": {"stringValue": session_id}
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

        let resp: Value = self
            .client
            .get(&url)
            .bearer_auth(auth_token)
            .send()
            .await
            .context("read_facts: request failed")?
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
            .filter_map(|doc| firestore_doc_to_fact(doc).ok())
            .collect();

        Ok(facts)
    }
}

// ---------------------------------------------------------------------------
// Firestore document conversion helpers
// ---------------------------------------------------------------------------

/// Convert a `FirestoreFact` into a Firestore REST document body.
pub fn fact_to_firestore_doc(fact: &FirestoreFact) -> Value {
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
            "session_id": {"stringValue": fact.session_id}
        }
    })
}

/// Parse a Firestore REST document into a `FirestoreFact`.
pub fn firestore_doc_to_fact(doc: &Value) -> Result<FirestoreFact> {
    let fields = doc
        .get("fields")
        .context("document missing 'fields' key")?;

    let category = string_field(fields, "category")?;
    let content = string_field(fields, "content")?;
    let session_id = string_field(fields, "session_id")?;
    let importance = fields
        .get("importance")
        .and_then(|v| v.get("doubleValue"))
        .and_then(|v| v.as_f64())
        .unwrap_or(0.0);

    let entities = fields
        .get("entities")
        .and_then(|v| v.get("arrayValue"))
        .and_then(|v| v.get("values"))
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|e| e.get("stringValue").and_then(|s| s.as_str()).map(String::from))
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

/// Deterministic document ID derived from session + content.
fn fact_doc_id(fact: &FirestoreFact) -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    let mut h = DefaultHasher::new();
    fact.session_id.hash(&mut h);
    fact.content.hash(&mut h);
    format!("{}-{:x}", fact.session_id, h.finish())
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
        let doc = serde_json::json!({"name": "projects/x/databases/(default)/documents/users/d/facts/f"});
        assert!(firestore_doc_to_fact(&doc).is_err());
    }

    #[test]
    fn importance_defaults_to_zero_when_absent() {
        let doc = serde_json::json!({
            "fields": {
                "category":   {"stringValue": "note"},
                "content":    {"stringValue": "hello"},
                "session_id": {"stringValue": "s3"},
                "entities":   {"arrayValue": {"values": []}}
            }
        });
        let parsed = firestore_doc_to_fact(&doc).unwrap();
        assert_eq!(parsed.importance, 0.0);
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
