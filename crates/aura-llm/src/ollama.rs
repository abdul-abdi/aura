use crate::provider::LlmProvider;
use anyhow::{Context, Result};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

const DEFAULT_BASE_URL: &str = "http://localhost:11434";
const DEFAULT_MODEL: &str = "qwen3.5:4b";
const DEFAULT_TIMEOUT_SECS: u64 = 30;

#[derive(Debug, Clone)]
pub struct OllamaConfig {
    pub base_url: String,
    pub model: String,
    pub timeout_secs: u64,
}

impl Default for OllamaConfig {
    fn default() -> Self {
        Self {
            base_url: DEFAULT_BASE_URL.into(),
            model: DEFAULT_MODEL.into(),
            timeout_secs: DEFAULT_TIMEOUT_SECS,
        }
    }
}

/// Chat API request body — uses /api/chat with thinking disabled for fast responses.
#[derive(Serialize)]
struct ChatRequest<'a> {
    model: &'a str,
    messages: Vec<ChatMessage<'a>>,
    stream: bool,
    /// Disable chain-of-thought for thinking models (qwen3.5, etc.)
    think: bool,
}

#[derive(Serialize)]
struct ChatMessage<'a> {
    role: &'a str,
    content: &'a str,
}

#[derive(Deserialize)]
struct ChatResponse {
    message: ChatResponseMessage,
}

#[derive(Deserialize)]
struct ChatResponseMessage {
    content: String,
}

pub struct OllamaProvider {
    client: reqwest::Client,
    config: OllamaConfig,
}

impl OllamaProvider {
    pub fn new(config: OllamaConfig) -> Result<Self> {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(config.timeout_secs))
            .build()
            .context("Failed to create HTTP client")?;
        Ok(Self { client, config })
    }

    /// Check if Ollama is reachable and the model is available.
    pub async fn health_check(&self) -> Result<()> {
        let url = format!("{}/api/tags", self.config.base_url);
        let resp = self
            .client
            .get(&url)
            .send()
            .await
            .context("Cannot reach Ollama — is it running? Start with: ollama serve")?;

        if !resp.status().is_success() {
            anyhow::bail!("Ollama health check failed with status {}", resp.status());
        }

        // Check if our model is available
        let body: serde_json::Value = resp.json().await.context("Failed to parse Ollama tags")?;
        let empty = Vec::new();
        let models = body["models"]
            .as_array()
            .unwrap_or(&empty)
            .iter()
            .filter_map(|m| m["name"].as_str())
            .collect::<Vec<_>>();

        if !models
            .iter()
            .any(|m| m.starts_with(self.config.model.split(':').next().unwrap_or("")))
        {
            tracing::warn!(
                model = %self.config.model,
                available = ?models,
                "Model not found in Ollama — pull it with: ollama pull {}",
                self.config.model
            );
        }

        Ok(())
    }

    async fn try_chat(&self, url: &str, body: &ChatRequest<'_>) -> Result<String> {
        let resp = self
            .client
            .post(url)
            .json(body)
            .send()
            .await
            .context("Failed to reach Ollama")?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            anyhow::bail!("Ollama returned {status}: {text}");
        }

        let parsed: ChatResponse = resp
            .json()
            .await
            .context("Failed to parse Ollama response")?;

        Ok(parsed.message.content)
    }
}

#[async_trait]
impl LlmProvider for OllamaProvider {
    async fn complete(&self, prompt: &str) -> Result<String> {
        let url = format!("{}/api/chat", self.config.base_url);
        let body = ChatRequest {
            model: &self.config.model,
            messages: vec![ChatMessage {
                role: "user",
                content: prompt,
            }],
            stream: false,
            think: false,
        };

        let mut last_error = None;
        for attempt in 0..3u64 {
            if attempt > 0 {
                let delay = std::time::Duration::from_millis(500 * attempt);
                tracing::debug!(attempt, "Retrying Ollama request after {delay:?}");
                tokio::time::sleep(delay).await;
            }

            match self.try_chat(&url, &body).await {
                Ok(response) => return Ok(response),
                Err(e) => {
                    tracing::warn!(attempt, "Ollama request failed: {e}");
                    last_error = Some(e);
                }
            }
        }

        Err(last_error.unwrap_or_else(|| anyhow::anyhow!("Ollama request failed")))
    }
}
