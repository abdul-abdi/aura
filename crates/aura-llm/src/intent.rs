use crate::provider::LlmProvider;
use anyhow::Result;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Intent {
    OpenApp { name: String },
    SearchFiles { query: String },
    TileWindows { layout: String },
    SummarizeScreen,
    LaunchUrl { url: String },
    Unknown { raw: String },
}

const SYSTEM_PROMPT: &str = r#"You are Aura's intent parser. Given a user voice command, output JSON with the intent type and parameters.

Valid intents:
- {"type":"open_app","name":"<app name>"}
- {"type":"search_files","query":"<search query>"}
- {"type":"tile_windows","layout":"<left-right|grid|stack>"}
- {"type":"summarize_screen"}
- {"type":"launch_url","url":"<url>"}
- {"type":"unknown","raw":"<original text>"}

Output ONLY valid JSON. No explanation."#;

pub struct IntentParser {
    provider: Box<dyn LlmProvider>,
}

impl IntentParser {
    pub fn new(provider: Box<dyn LlmProvider>) -> Self {
        Self { provider }
    }

    pub async fn parse(&self, text: &str) -> Result<Intent> {
        const MAX_INPUT_LEN: usize = 500;
        let truncated = if text.len() > MAX_INPUT_LEN {
            &text[..text.floor_char_boundary(MAX_INPUT_LEN)]
        } else {
            text
        };
        let prompt = format!("{SYSTEM_PROMPT}\n\nUser command: {truncated}\n\nJSON:");
        let response = self.provider.complete(&prompt).await?;

        tracing::debug!(raw_response = %response, "LLM response for intent parsing");

        let intent: Intent = serde_json::from_str(response.trim()).unwrap_or_else(|err| {
            tracing::warn!(%err, "Failed to parse LLM response as Intent, falling back to Unknown");
            Intent::Unknown {
                raw: text.to_string(),
            }
        });
        Ok(intent)
    }
}
