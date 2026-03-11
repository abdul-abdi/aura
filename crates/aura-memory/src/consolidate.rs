//! End-of-session memory consolidation via Gemini REST API.

use anyhow::{Context, Result};
use serde::Deserialize;

use crate::store::{Message, MessageRole};

const CONSOLIDATION_MODEL: &str = "gemini-2.0-flash-lite";
const GEMINI_REST_URL: &str =
    "https://generativelanguage.googleapis.com/v1beta/models";

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
    let mut prompt = String::from(
        "You are a memory extraction agent. Analyze this conversation history and extract key facts.\n\n\
         The conversation contains user messages and tool calls from an AI desktop assistant.\n\n\
         Extract facts in these categories:\n\
         - \"preference\": User preferences (apps, settings, workflows they like)\n\
         - \"habit\": Repeated behaviors or patterns\n\
         - \"entity\": Important files, apps, or things the user works with\n\
         - \"task\": What the user was trying to accomplish\n\
         - \"context\": Other useful context for future sessions\n\n\
         Respond ONLY with valid JSON in this exact format:\n\
         ```json\n\
         {\n\
           \"summary\": \"One sentence describing what happened in this session\",\n\
           \"facts\": [\n\
             {\n\
               \"category\": \"preference\",\n\
               \"content\": \"Human-readable fact\",\n\
               \"entities\": [\"entity1\", \"entity2\"],\n\
               \"importance\": 0.8\n\
             }\n\
           ]\n\
         }\n\
         ```\n\n\
         If the session was trivial (just a greeting or test), return an empty facts array.\n\n\
         --- CONVERSATION ---\n",
    );

    for msg in messages {
        let role_label = match msg.role {
            MessageRole::User => "USER",
            MessageRole::ToolCall => "TOOL_CALL",
            _ => "OTHER",
        };
        prompt.push_str(&format!("[{role_label}] {}\n", msg.content));
    }

    prompt
}

/// Call Gemini REST API to consolidate a session's messages into structured facts.
pub async fn consolidate_session(
    api_key: &str,
    messages: &[Message],
) -> Result<ConsolidationResponse> {
    let filtered = filter_messages_for_consolidation(messages);
    if filtered.is_empty() {
        return Ok(ConsolidationResponse {
            summary: String::new(),
            facts: Vec::new(),
        });
    }

    let prompt = build_consolidation_prompt(&filtered);
    let url = format!(
        "{GEMINI_REST_URL}/{CONSOLIDATION_MODEL}:generateContent"
    );

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
}
