use std::sync::{Arc, Mutex};

use aura_memory::SessionMemory;
use aura_memory::consolidate::ExtractedFact;

/// Run a memory operation on a blocking thread to avoid holding the Mutex
/// across await points or blocking the tokio runtime.
/// Logs errors with `tracing::warn!` before converting to `None`.
pub(crate) async fn memory_op<F, T>(memory: &Arc<Mutex<SessionMemory>>, f: F) -> Option<T>
where
    F: FnOnce(&SessionMemory) -> anyhow::Result<T> + Send + 'static,
    T: Send + 'static,
{
    let mem = Arc::clone(memory);
    match tokio::task::spawn_blocking(move || {
        let guard = match mem.lock() {
            Ok(g) => g,
            Err(e) => {
                tracing::warn!("Memory lock poisoned: {e}");
                return None;
            }
        };
        match f(&guard) {
            Ok(val) => Some(val),
            Err(e) => {
                tracing::warn!("Memory operation failed: {e}");
                None
            }
        }
    })
    .await
    {
        Ok(result) => result,
        Err(e) => {
            tracing::error!("Memory operation panicked: {e}");
            None
        }
    }
}

/// Load facts from Firestore for a given device and format them as a context string.
/// Returns an empty string when there are no facts or Firestore is unavailable.
/// NOTE: Currently unused — memory agent /query endpoint replaces this at startup.
/// Kept as fallback if memory agent is removed.
#[allow(dead_code)]
pub(crate) async fn load_firestore_facts(
    project_id: &str,
    device_id: &str,
    firebase_api_key: &str,
) -> anyhow::Result<String> {
    #[allow(deprecated)]
    let token = aura_firestore::auth::get_anonymous_token(firebase_api_key).await?;
    let client = aura_firestore::client::FirestoreClient::new(
        project_id.to_string(),
        device_id.to_string(),
    )?;
    let facts = client.read_facts(&token).await?;

    if facts.is_empty() {
        return Ok(String::new());
    }

    let mut context = String::new();
    for fact in &facts {
        context.push_str(&format!("- [{}] {}\n", fact.category, fact.content));
    }
    Ok(context)
}

/// Sync consolidated facts and session summary to Firestore.
///
/// Authenticates anonymously via Firebase, then writes the session document
/// (if a non-empty summary exists) and all extracted facts. Errors are logged
/// as warnings — Firestore sync is best-effort and should never block the
/// local consolidation path.
pub(crate) async fn sync_session_to_firestore(
    facts: &[ExtractedFact],
    summary: &str,
    session_id: &str,
    project_id: &str,
    device_id: &str,
    firebase_api_key: &str,
) -> anyhow::Result<()> {
    let fs_client =
        aura_firestore::client::FirestoreClient::new(project_id.to_string(), device_id.to_string())
            .map_err(|e| anyhow::anyhow!("Invalid device_id for Firestore sync: {e}"))?;

    #[allow(deprecated)]
    let token = aura_firestore::auth::get_anonymous_token(firebase_api_key)
        .await
        .map_err(|e| anyhow::anyhow!("Firebase auth for Firestore sync failed: {e}"))?;

    if !summary.is_empty()
        && let Err(e) = fs_client.write_session(session_id, summary, &token).await
    {
        tracing::warn!("Firestore session write failed: {e}");
    }

    for fact in facts {
        let fs_fact = aura_firestore::client::FirestoreFact {
            category: fact.category.clone(),
            content: fact.content.clone(),
            entities: fact.entities.clone(),
            importance: fact.importance,
            session_id: session_id.to_string(),
        };
        if let Err(e) = fs_client.write_fact(&fs_fact, &token).await {
            tracing::warn!("Firestore fact write failed: {e}");
        }
    }

    tracing::info!("Local consolidation synced to Firestore");
    Ok(())
}

/// A pending Firestore sync entry, serialized to disk on failure.
#[derive(serde::Serialize, serde::Deserialize)]
struct PendingSyncEntry {
    session_id: String,
    summary: String,
    facts: Vec<PendingSyncFact>,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct PendingSyncFact {
    category: String,
    content: String,
    entities: Vec<String>,
    importance: f64,
}

/// Directory where pending Firestore sync files are stored.
fn pending_sync_dir() -> Option<std::path::PathBuf> {
    dirs::data_dir().map(|d| d.join("aura").join("pending_sync"))
}

/// Queue a failed Firestore sync for later retry.
pub(crate) fn queue_pending_sync(facts: &[ExtractedFact], summary: &str, session_id: &str) {
    let Some(dir) = pending_sync_dir() else {
        tracing::warn!("Cannot determine data directory for pending sync queue");
        return;
    };
    if let Err(e) = std::fs::create_dir_all(&dir) {
        tracing::warn!("Failed to create pending sync directory: {e}");
        return;
    }
    let entry = PendingSyncEntry {
        session_id: session_id.to_string(),
        summary: summary.to_string(),
        facts: facts
            .iter()
            .map(|f| PendingSyncFact {
                category: f.category.clone(),
                content: f.content.clone(),
                entities: f.entities.clone(),
                importance: f.importance,
            })
            .collect(),
    };
    let path = dir.join(format!("{session_id}.json"));
    match serde_json::to_string(&entry) {
        Ok(json) => {
            if let Err(e) = std::fs::write(&path, json) {
                tracing::warn!("Failed to write pending sync file: {e}");
            } else {
                tracing::info!(path = %path.display(), "Queued failed Firestore sync for retry");
            }
        }
        Err(e) => tracing::warn!("Failed to serialize pending sync entry: {e}"),
    }
}

/// Attempt to flush all pending Firestore syncs. Called at daemon startup
/// after a successful Firestore connection is confirmed.
pub(crate) async fn flush_pending_syncs(project_id: &str, device_id: &str, firebase_api_key: &str) {
    let Some(dir) = pending_sync_dir() else {
        return;
    };
    let entries = match std::fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(_) => return, // No pending sync directory — nothing to flush
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_some_and(|ext| ext == "json") {
            match std::fs::read_to_string(&path) {
                Ok(json) => match serde_json::from_str::<PendingSyncEntry>(&json) {
                    Ok(pending) => {
                        let facts: Vec<ExtractedFact> = pending
                            .facts
                            .into_iter()
                            .map(|f| ExtractedFact {
                                category: f.category,
                                content: f.content,
                                entities: f.entities,
                                importance: f.importance,
                            })
                            .collect();
                        match sync_session_to_firestore(
                            &facts,
                            &pending.summary,
                            &pending.session_id,
                            project_id,
                            device_id,
                            firebase_api_key,
                        )
                        .await
                        {
                            Ok(()) => {
                                let _ = std::fs::remove_file(&path);
                                tracing::info!(
                                    session_id = %pending.session_id,
                                    "Flushed pending Firestore sync"
                                );
                            }
                            Err(e) => {
                                tracing::warn!(
                                    session_id = %pending.session_id,
                                    "Retry of pending Firestore sync failed: {e}"
                                );
                            }
                        }
                    }
                    Err(e) => {
                        tracing::warn!(path = %path.display(), "Invalid pending sync file: {e}");
                        let _ = std::fs::remove_file(&path); // Remove corrupt file
                    }
                },
                Err(e) => {
                    tracing::warn!(path = %path.display(), "Failed to read pending sync: {e}")
                }
            }
        }
    }
}
