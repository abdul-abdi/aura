//! End-of-session memory consolidation via Gemini REST API.

use anyhow::{Context, Result};
use serde::Deserialize;

use crate::store::{Message, MessageRole};

const CONSOLIDATION_MODEL: &str = "gemini-2.0-flash-lite";
const GEMINI_REST_URL: &str = "https://generativelanguage.googleapis.com/v1beta/models";

/// Maximum characters in the consolidation prompt (≈12K tokens).
const MAX_PROMPT_CHARS: usize = 50_000;

/// Extracted fact from a session.
#[derive(Debug, Deserialize)]
pub struct ExtractedFact {
    pub category: String,
    pub content: String,
    #[serde(default)]
    pub entities: Vec<String>,
    #[serde(default = "default_importance")]
    pub importance: f64,
}

fn default_importance() -> f64 {
    0.5
}

/// Response from the consolidation model.
#[derive(Debug, Deserialize)]
pub struct ConsolidationResponse {
    pub summary: String,
    #[serde(default)]
    pub facts: Vec<ExtractedFact>,
}

/// Filter session messages to tool_call + user messages only.
pub fn filter_messages_for_consolidation(messages: &[Message]) -> Vec<&Message> {
    messages
        .iter()
        .filter(|m| matches!(m.role, MessageRole::ToolCall | MessageRole::User))
        .collect()
}

/// Build the consolidation prompt from filtered messages.
pub fn build_consolidation_prompt(messages: &[&Message]) -> String {
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

    let mut lines: Vec<String> = Vec::with_capacity(messages.len());
    for msg in messages {
        let role_label = match msg.role {
            MessageRole::User => "USER",
            MessageRole::ToolCall => "TOOL_CALL",
            _ => "OTHER",
        };
        lines.push(format!("[{role_label}] {}", msg.content));
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

/// Call Gemini REST API to consolidate a session's messages into structured facts.
/// If a Cloud Run URL is provided, uses that instead of calling Gemini directly.
pub async fn consolidate_session(
    api_key: &str,
    messages: &[Message],
    cloud_run_url: Option<&str>,
    cloud_run_auth_token: Option<&str>,
    device_id: Option<&str>,
    session_id: Option<&str>,
) -> Result<ConsolidationResponse> {
    let filtered = filter_messages_for_consolidation(messages);
    if filtered.is_empty() {
        return Ok(ConsolidationResponse {
            summary: String::new(),
            facts: Vec::new(),
        });
    }

    if let (Some(url), Some(token), Some(did), Some(sid)) =
        (cloud_run_url, cloud_run_auth_token, device_id, session_id)
    {
        match consolidate_via_cloud_run(url, token, did, sid, &filtered).await {
            Ok(resp) => return Ok(resp),
            Err(e) => {
                tracing::warn!("Cloud Run consolidation failed, falling back to local: {e}");
            }
        }
    }

    // Fallback: call Gemini directly
    consolidate_locally(api_key, &filtered).await
}

/// Call the Cloud Run consolidation service.
async fn consolidate_via_cloud_run(
    url: &str,
    auth_token: &str,
    device_id: &str,
    session_id: &str,
    messages: &[&Message],
) -> Result<ConsolidationResponse> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(60))
        .build()
        .context("Failed to build HTTP client")?;

    let msg_json: Vec<serde_json::Value> = messages
        .iter()
        .map(|m| {
            serde_json::json!({
                "role": match m.role {
                    MessageRole::User => "user",
                    MessageRole::ToolCall => "tool_call",
                    _ => "other",
                },
                "content": m.content,
                "timestamp": m.timestamp,
            })
        })
        .collect();

    let body = serde_json::json!({
        "device_id": device_id,
        "session_id": session_id,
        "messages": msg_json,
    });

    let resp = client
        .post(&format!("{url}/consolidate"))
        .bearer_auth(auth_token)
        .json(&body)
        .send()
        .await
        .context("Cloud Run consolidation request failed")?;

    let status = resp.status();
    if !status.is_success() {
        let text = resp.text().await.unwrap_or_default();
        anyhow::bail!("Cloud Run returned {status}: {text}");
    }

    resp.json()
        .await
        .context("Failed to parse Cloud Run response")
}

/// Call Gemini REST API directly to consolidate session messages.
async fn consolidate_locally(api_key: &str, filtered: &[&Message]) -> Result<ConsolidationResponse> {
    let prompt = build_consolidation_prompt(filtered);
    let url = format!("{GEMINI_REST_URL}/{CONSOLIDATION_MODEL}:generateContent");

    let body = serde_json::json!({
        "contents": [{
            "parts": [{ "text": prompt }]
        }],
        "generationConfig": {
            "temperature": 0.2,
            "responseMimeType": "application/json"
        }
    });

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .context("Failed to build HTTP client")?;
    let resp = client
        .post(&url)
        .header("x-goog-api-key", api_key)
        .json(&body)
        .send()
        .await
        .context("Failed to call Gemini REST API for consolidation")?;

    let status = resp.status();
    if !status.is_success() {
        let text = resp.text().await.unwrap_or_default();
        anyhow::bail!("Gemini consolidation API returned {status}: {text}");
    }

    let resp_json: serde_json::Value = resp
        .json()
        .await
        .context("Failed to parse Gemini consolidation response")?;

    // Extract the text from the response
    let text = resp_json["candidates"][0]["content"]["parts"][0]["text"]
        .as_str()
        .context("No text in consolidation response")?;

    // Parse the JSON from the response text
    let consolidation: ConsolidationResponse = serde_json::from_str(text)
        .context("Failed to parse consolidation JSON from model response")?;

    Ok(consolidation)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_message(role: MessageRole, content: &str) -> Message {
        Message {
            id: 0,
            session_id: "test".into(),
            role,
            content: content.into(),
            timestamp: "2026-01-01T00:00:00Z".into(),
            metadata: None,
        }
    }

    #[test]
    fn filter_keeps_user_and_tool_call_only() {
        let messages = vec![
            make_message(MessageRole::User, "open Safari"),
            make_message(MessageRole::Assistant, "Sure, opening Safari."),
            make_message(MessageRole::ToolCall, "click: {x: 512, y: 760}"),
            make_message(MessageRole::ToolResult, "ok"),
        ];
        let filtered = filter_messages_for_consolidation(&messages);
        assert_eq!(filtered.len(), 2);
        assert_eq!(filtered[0].content, "open Safari");
        assert_eq!(filtered[1].content, "click: {x: 512, y: 760}");
    }

    #[test]
    fn filter_empty_messages_returns_empty() {
        let filtered = filter_messages_for_consolidation(&[]);
        assert!(filtered.is_empty());
    }

    #[test]
    fn prompt_contains_conversation_markers() {
        let messages = vec![
            make_message(MessageRole::User, "search for Rust docs"),
            make_message(MessageRole::ToolCall, "type_text: Rust docs"),
        ];
        let refs: Vec<&Message> = messages.iter().collect();
        let prompt = build_consolidation_prompt(&refs);
        assert!(prompt.contains("[USER] search for Rust docs"));
        assert!(prompt.contains("[TOOL_CALL] type_text: Rust docs"));
        assert!(prompt.contains("memory extraction agent"));
        assert!(prompt.contains("\"preference\""));
    }

    #[test]
    fn parse_consolidation_response() {
        let json = r#"{"summary":"User searched for Rust docs","facts":[{"category":"task","content":"User searched for Rust documentation","entities":["Rust"],"importance":0.6}]}"#;
        let resp: ConsolidationResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.summary, "User searched for Rust docs");
        assert_eq!(resp.facts.len(), 1);
        assert_eq!(resp.facts[0].category, "task");
        assert_eq!(resp.facts[0].entities, vec!["Rust"]);
    }

    #[test]
    fn parse_empty_facts_response() {
        let json = r#"{"summary":"Just a greeting","facts":[]}"#;
        let resp: ConsolidationResponse = serde_json::from_str(json).unwrap();
        assert!(resp.facts.is_empty());
    }

    #[test]
    fn prompt_truncates_long_conversations() {
        let messages: Vec<Message> = (0..1000)
            .map(|i| make_message(MessageRole::User, &format!("Message number {i} with some padding text to make it longer than a short msg")))
            .collect();
        let refs: Vec<&Message> = messages.iter().collect();
        let prompt = build_consolidation_prompt(&refs);
        assert!(
            prompt.len() < 60_000,
            "Prompt was {} chars, expected < 60K",
            prompt.len()
        );
        assert!(prompt.contains("Message number 999"));
    }
}
