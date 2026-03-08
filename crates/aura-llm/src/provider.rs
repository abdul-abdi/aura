use anyhow::Result;
use async_trait::async_trait;
use futures_core::Stream;
use std::pin::Pin;

pub type TokenStream = Pin<Box<dyn Stream<Item = Result<String>> + Send>>;

#[async_trait]
pub trait LlmProvider: Send + Sync {
    async fn complete(&self, prompt: &str) -> Result<String>;

    /// Stream tokens from the model. Default implementation collects the full response.
    async fn stream(&self, prompt: &str) -> Result<TokenStream> {
        let text = self.complete(prompt).await?;
        let stream = async_stream::stream! {
            yield Ok(text);
        };
        Ok(Box::pin(stream))
    }
}

/// Mock provider for testing — only available in test builds.
#[cfg(any(test, feature = "test-support"))]
pub struct MockProvider {
    responses: Vec<(String, String)>,
}

#[cfg(any(test, feature = "test-support"))]
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

#[cfg(any(test, feature = "test-support"))]
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
