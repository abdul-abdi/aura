use std::sync::{Arc, Mutex};

use aura_memory::consolidate::ExtractedFact;
use aura_memory::SessionMemory;

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
