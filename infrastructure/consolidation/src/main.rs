//! Aura Consolidation Service — Cloud Run HTTP server.
//!
//! POST /consolidate — extract facts from a session transcript and write to Firestore.
//! GET  /health      — liveness probe.

use std::sync::Arc;

use anyhow::{Context, Result};
use axum::{
    Router,
    extract::{DefaultBodyLimit, State},
    http::{HeaderMap, StatusCode},
    response::Json,
    routing::{get, post},
};
use chrono::Utc;
use futures::future::join_all;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio::sync::Semaphore;
use tracing::{error, info, warn};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

const DEFAULT_CONSOLIDATION_MODEL: &str = "gemini-2.5-flash-lite";
const GEMINI_REST_URL: &str = "https://generativelanguage.googleapis.com/v1beta/models";
const FIRESTORE_BASE: &str = "https://firestore.googleapis.com/v1/projects";
const METADATA_TOKEN_URL: &str =
    "http://metadata.google.internal/computeMetadata/v1/instance/service-accounts/default/token";
const MAX_PROMPT_CHARS: usize = 50_000;
/// Maximum request body size (1 MiB).
const MAX_BODY_BYTES: usize = 1_024 * 1_024;
/// Maximum concurrent consolidation requests.
const MAX_CONCURRENT_REQUESTS: usize = 10;

// ---------------------------------------------------------------------------
// App state
// ---------------------------------------------------------------------------

struct AppState {
    gemini_api_key: String,
    auth_token: Option<String>,
    legacy_auth_enabled: bool,
    gcp_project_id: Option<String>,
    consolidation_model: String,
    http: reqwest::Client,
    semaphore: Semaphore,
}

// ---------------------------------------------------------------------------
// Request / response types
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct Message {
    role: String,
    content: String,
    #[allow(dead_code)]
    timestamp: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ConsolidateRequest {
    device_id: String,
    session_id: String,
    messages: Vec<Message>,
}

#[derive(Debug, Serialize, Deserialize)]
struct ExtractedFact {
    category: String,
    content: String,
    #[serde(default)]
    entities: Vec<String>,
    #[serde(default = "default_importance")]
    importance: f64,
}

fn default_importance() -> f64 {
    0.5
}

#[derive(Debug, Serialize, Deserialize)]
struct ConsolidationResult {
    summary: String,
    #[serde(default)]
    facts: Vec<ExtractedFact>,
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    let gemini_api_key =
        std::env::var("GEMINI_API_KEY").context("GEMINI_API_KEY env var is required")?;
    let auth_token = std::env::var("AURA_AUTH_TOKEN").ok();
    let legacy_auth_enabled = std::env::var("LEGACY_AUTH_ENABLED")
        .unwrap_or_else(|_| "true".to_string())
        .to_lowercase()
        == "true";
    let gcp_project_id = std::env::var("GCP_PROJECT_ID").ok();
    let consolidation_model = std::env::var("CONSOLIDATION_MODEL")
        .unwrap_or_else(|_| DEFAULT_CONSOLIDATION_MODEL.to_string());
    let port: u16 = std::env::var("PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(8080);

    if auth_token.is_none() && gcp_project_id.is_none() {
        panic!("Either AURA_AUTH_TOKEN or GCP_PROJECT_ID must be set");
    }

    let state = Arc::new(AppState {
        gemini_api_key,
        auth_token,
        legacy_auth_enabled,
        gcp_project_id,
        consolidation_model,
        http: reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(60))
            .build()
            .context("Failed to build HTTP client")?,
        semaphore: Semaphore::new(MAX_CONCURRENT_REQUESTS),
    });

    let app = Router::new()
        .route("/health", get(health))
        .route("/consolidate", post(consolidate))
        .layer(DefaultBodyLimit::max(MAX_BODY_BYTES))
        .with_state(state);

    let addr = format!("0.0.0.0:{port}");
    info!("aura-consolidation listening on {addr}");
    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .with_context(|| format!("Failed to bind to {addr}"))?;
    axum::serve(listener, app).await.context("Server error")?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

async fn health() -> StatusCode {
    StatusCode::OK
}

async fn consolidate(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<ConsolidateRequest>,
) -> Result<Json<ConsolidationResult>, (StatusCode, String)> {
    // Rate limit: reject if too many concurrent requests.
    let _permit = state.semaphore.try_acquire().map_err(|_| {
        warn!("consolidate: rate limited (max {MAX_CONCURRENT_REQUESTS} concurrent)");
        (
            StatusCode::SERVICE_UNAVAILABLE,
            "Too many concurrent requests".to_string(),
        )
    })?;

    // Validate device_id and session_id to prevent Firestore path traversal
    // before the device_id is used in any backend lookup.
    validate_device_id(&req.device_id).map_err(|e| {
        warn!("consolidate: invalid device_id: {e}");
        (StatusCode::BAD_REQUEST, format!("Invalid device_id: {e}"))
    })?;
    validate_document_id(&req.session_id).map_err(|e| {
        warn!("consolidate: invalid session_id: {e}");
        (StatusCode::BAD_REQUEST, format!("Invalid session_id: {e}"))
    })?;

    // Auth check — try legacy shared token first, then device token via Firestore.
    let bearer_token = extract_bearer(&headers).unwrap_or_default();

    let mut authenticated = false;

    if state.legacy_auth_enabled
        && let Some(ref expected) = state.auth_token
        && constant_time_eq(bearer_token, expected)
    {
        authenticated = true;
    }

    if !authenticated {
        authenticated = validate_device_token(&state, &req.device_id, bearer_token).await;
    }

    if !authenticated {
        warn!("consolidate: unauthorized request");
        return Err((StatusCode::UNAUTHORIZED, "Unauthorized".into()));
    }

    info!(
        "consolidate: device={} session={} messages={}",
        req.device_id,
        req.session_id,
        req.messages.len()
    );

    // Build prompt and call Gemini.
    let result = call_gemini(&state, &req.messages).await.map_err(|e| {
        error!("Gemini call failed: {e:#}");
        (StatusCode::BAD_GATEWAY, "Upstream model error".to_string())
    })?;

    // Obtain GCP access token and write to Firestore.
    match get_gcp_token(&state.http).await {
        Ok(gcp_token) => {
            if let Err(e) =
                write_to_firestore(&state, &gcp_token, &req.device_id, &req.session_id, &result)
                    .await
            {
                // Non-fatal — log and continue so the caller still gets facts.
                error!("Firestore write failed: {e:#}");
            }
        }
        Err(e) => {
            warn!("Could not obtain GCP token (running locally?): {e:#}");
        }
    }

    Ok(Json(result))
}

// ---------------------------------------------------------------------------
// Auth helpers
// ---------------------------------------------------------------------------

fn extract_bearer(headers: &HeaderMap) -> Option<&str> {
    headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
}

/// Compare two strings in constant time by hashing both with SHA-256 first.
fn constant_time_eq(a: &str, b: &str) -> bool {
    use subtle::ConstantTimeEq;
    let ha = Sha256::digest(a.as_bytes());
    let hb = Sha256::digest(b.as_bytes());
    ha.ct_eq(&hb).into()
}

// ---------------------------------------------------------------------------
// Device token auth (Firestore-backed, with in-memory cache)
// ---------------------------------------------------------------------------

/// In-memory cache mapping device_id -> (token_hash, cached_at).
static DEVICE_CACHE: std::sync::LazyLock<
    tokio::sync::RwLock<std::collections::HashMap<String, (String, std::time::Instant)>>,
> = std::sync::LazyLock::new(|| tokio::sync::RwLock::new(std::collections::HashMap::new()));

const DEVICE_CACHE_TTL: std::time::Duration = std::time::Duration::from_secs(60);

async fn validate_device_token(state: &AppState, device_id: &str, token: &str) -> bool {
    let provided_hash = {
        let mut h = Sha256::new();
        h.update(token.as_bytes());
        hex::encode(h.finalize())
    };

    // Fast path: check in-memory cache.
    {
        let cache = DEVICE_CACHE.read().await;
        if let Some((cached_hash, cached_at)) = cache.get(device_id)
            && cached_at.elapsed() < DEVICE_CACHE_TTL
        {
            use subtle::ConstantTimeEq;
            return provided_hash
                .as_bytes()
                .ct_eq(cached_hash.as_bytes())
                .into();
        }
    }

    // Cache miss — look up in Firestore.
    let project_id = match &state.gcp_project_id {
        Some(p) => p,
        None => return false,
    };

    let access_token = match get_gcp_token(&state.http).await {
        Ok(t) => t,
        Err(e) => {
            tracing::error!("Failed to get GCP token for device auth: {e}");
            return false;
        }
    };

    let url = format!(
        "https://firestore.googleapis.com/v1/projects/{project_id}/databases/(default)/documents/devices/{device_id}"
    );

    let resp = match state.http.get(&url).bearer_auth(&access_token).send().await {
        Ok(r) => r,
        Err(e) => {
            tracing::error!("Firestore device lookup failed: {e}");
            return false;
        }
    };

    if !resp.status().is_success() {
        return false;
    }

    let json: serde_json::Value = match resp.json().await {
        Ok(j) => j,
        Err(_) => return false,
    };

    let stored_hash = json["fields"]["token_hash"]["stringValue"]
        .as_str()
        .unwrap_or("")
        .to_string();

    // Populate cache so subsequent requests within TTL skip Firestore.
    DEVICE_CACHE
        .write()
        .await
        .insert(device_id.to_string(), (stored_hash.clone(), std::time::Instant::now()));

    use subtle::ConstantTimeEq;
    provided_hash
        .as_bytes()
        .ct_eq(stored_hash.as_bytes())
        .into()
}

// ---------------------------------------------------------------------------
// Consolidation (Gemini REST)
// ---------------------------------------------------------------------------

fn build_prompt(messages: &[Message]) -> String {
    let preamble = "You are a memory extraction agent. Analyze this conversation history and extract key facts.\n\n\
         The conversation contains user messages and tool calls from an AI desktop assistant.\n\n\
         Extract facts in these categories:\n\
         - \"preference\": User preferences (apps, settings, workflows they like)\n\
         - \"habit\": Repeated behaviors or patterns\n\
         - \"entity\": Important files, apps, or things the user works with\n\
         - \"task\": What the user was trying to accomplish\n\
         - \"context\": Other useful context for future sessions\n\n\
         Respond ONLY with valid JSON matching this schema:\n\
         {\"summary\": \"string\", \"facts\": [{\"category\": \"string\", \"content\": \"string\", \"entities\": [\"string\"], \"importance\": number}]}\n\n\
         Categories: preference, habit, entity, task, context.\n\n\
         If the session was trivial (just a greeting or test), return an empty facts array.\n\n\
         --- CONVERSATION ---\n";

    // Only keep user and tool_call messages (mirrors aura-memory consolidation filter).
    let lines: Vec<String> = messages
        .iter()
        .filter(|m| {
            let r = m.role.as_str();
            r == "user" || r == "tool_call"
        })
        .map(|m| {
            let label = if m.role == "user" {
                "USER"
            } else {
                "TOOL_CALL"
            };
            format!("[{label}] {}", m.content)
        })
        .collect();

    if lines.is_empty() {
        return format!("{preamble}(no relevant messages)\n");
    }

    let budget = MAX_PROMPT_CHARS.saturating_sub(preamble.len());
    let mut total: usize = lines.iter().map(|l| l.len() + 1).sum();
    let mut start = 0;
    while total > budget && start < lines.len() {
        total -= lines[start].len() + 1;
        start += 1;
    }

    let mut prompt = String::with_capacity(preamble.len() + total);
    prompt.push_str(preamble);
    if start > 0 {
        prompt.push_str(&format!("[...truncated {start} older messages...]\n"));
    }
    for line in &lines[start..] {
        prompt.push_str(line);
        prompt.push('\n');
    }
    prompt
}

async fn call_gemini(state: &AppState, messages: &[Message]) -> Result<ConsolidationResult> {
    let prompt = build_prompt(messages);
    let url = format!(
        "{GEMINI_REST_URL}/{}:generateContent",
        state.consolidation_model
    );

    let body = json!({
        "contents": [{"parts": [{"text": prompt}]}],
        "generationConfig": {
            "temperature": 0.2,
            "responseMimeType": "application/json"
        }
    });

    let resp = state
        .http
        .post(&url)
        .header("x-goog-api-key", &state.gemini_api_key)
        .json(&body)
        .send()
        .await
        .context("Failed to call Gemini REST API")?;

    let status = resp.status();
    if !status.is_success() {
        let text = resp.text().await.unwrap_or_default();
        anyhow::bail!("Gemini returned {status}: {text}");
    }

    let resp_json: Value = resp
        .json()
        .await
        .context("Failed to parse Gemini response")?;

    let text = resp_json["candidates"][0]["content"]["parts"][0]["text"]
        .as_str()
        .context("No text in Gemini response")?;

    let result: ConsolidationResult =
        serde_json::from_str(text).context("Failed to parse consolidation JSON from model")?;

    Ok(result)
}

// ---------------------------------------------------------------------------
// GCP metadata token
// ---------------------------------------------------------------------------

async fn get_gcp_token(client: &reqwest::Client) -> Result<String> {
    let resp: Value = client
        .get(METADATA_TOKEN_URL)
        .header("Metadata-Flavor", "Google")
        .send()
        .await
        .context("Failed to reach GCP metadata server")?
        .error_for_status()
        .context("GCP metadata server returned error")?
        .json()
        .await
        .context("Failed to parse GCP token response")?;

    resp.get("access_token")
        .and_then(|t| t.as_str())
        .map(String::from)
        .context("access_token missing from GCP metadata response")
}

// ---------------------------------------------------------------------------
// Firestore writes
// ---------------------------------------------------------------------------

async fn write_to_firestore(
    state: &AppState,
    gcp_token: &str,
    device_id: &str,
    session_id: &str,
    result: &ConsolidationResult,
) -> Result<()> {
    let project_id = state
        .gcp_project_id
        .as_deref()
        .context("GCP_PROJECT_ID not configured")?;
    let base = format!(
        "{FIRESTORE_BASE}/{project_id}/databases/(default)/documents/users/{device_id}"
    );

    // Write all facts concurrently.
    let fact_futures: Vec<_> = result
        .facts
        .iter()
        .map(|fact| {
            let doc_id = fact_doc_id(&fact.category, &fact.content);
            let url = format!("{base}/facts/{doc_id}");
            let body = fact_to_firestore_doc(fact, session_id);
            let http = &state.http;
            async move {
                http.patch(&url)
                    .bearer_auth(gcp_token)
                    .json(&body)
                    .send()
                    .await
                    .context("write_fact: request failed")?
                    .error_for_status()
                    .context("write_fact: non-2xx response")?;
                Ok::<_, anyhow::Error>(())
            }
        })
        .collect();

    let results = join_all(fact_futures).await;
    for (i, r) in results.into_iter().enumerate() {
        if let Err(e) = r {
            warn!("Failed to write fact {i}: {e:#}");
        }
    }

    // Write session summary.
    let now = Utc::now().to_rfc3339();
    let session_url = format!("{base}/sessions/{session_id}");
    let session_body = json!({
        "fields": {
            "summary":    {"stringValue": result.summary},
            "session_id": {"stringValue": session_id},
            "created_at": {"timestampValue": now}
        }
    });

    state
        .http
        .patch(&session_url)
        .bearer_auth(gcp_token)
        .json(&session_body)
        .send()
        .await
        .context("write_session: request failed")?
        .error_for_status()
        .context("write_session: non-2xx response")?;

    info!(
        "Firestore: wrote {} facts and session summary for {session_id}",
        result.facts.len()
    );
    Ok(())
}

/// Deterministic document ID derived from category + content using FNV-1a.
/// Matches the algorithm in aura-firestore/src/client.rs for cross-binary consistency.
fn fact_doc_id(category: &str, content: &str) -> String {
    let raw = format!("{category}:{content}");
    let mut hash: u64 = 0xcbf29ce484222325;
    for byte in raw.as_bytes() {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{:016x}", hash)
}

/// Validate a string is safe for use as a Firestore document ID in URL paths.
/// Allows alphanumeric chars, hyphens, and underscores only. Max 128 chars.
fn validate_document_id(id: &str) -> Result<()> {
    if id.is_empty() || id.len() > 128 {
        anyhow::bail!("document ID must be 1-128 characters, got {}", id.len());
    }
    if !id
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        anyhow::bail!(
            "document ID must contain only alphanumeric characters, hyphens, and underscores"
        );
    }
    Ok(())
}

/// Validate device_id to prevent Firestore path traversal.
/// Canonical version lives in aura-firestore/src/client.rs — kept local here
/// to avoid pulling the full crate (with workspace deps) into the Docker build.
fn validate_device_id(id: &str) -> Result<()> {
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

fn fact_to_firestore_doc(fact: &ExtractedFact, session_id: &str) -> Value {
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
            "session_id": {"stringValue": session_id},
            "created_at": {"timestampValue": now}
        }
    })
}
