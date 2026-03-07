use anyhow::Result;
use async_trait::async_trait;

#[async_trait]
pub trait LlmProvider: Send + Sync {
    async fn complete(&self, prompt: &str) -> Result<String>;
}

/// Mock provider for testing
pub struct MockProvider {
    responses: Vec<(String, String)>,
}

impl MockProvider {
    pub fn new(responses: Vec<(&str, &str)>) -> Self {
        Self {
            responses: responses
                .into_iter()
                .map(|(q, a)| (q.to_string(), a.to_string()))
                .collect(),
        }
    }
}

#[async_trait]
impl LlmProvider for MockProvider {
    async fn complete(&self, prompt: &str) -> Result<String> {
        for (query, response) in &self.responses {
            if prompt.contains(query) {
                return Ok(response.clone());
            }
        }
        Ok(r#"{"type":"unknown","raw":"no match"}"#.to_string())
    }
}
