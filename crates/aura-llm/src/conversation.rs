use crate::provider::{LlmProvider, TokenStream};
use anyhow::Result;
use std::sync::Mutex;

const SYSTEM_PROMPT: &str = "You are Aura, a friendly and helpful voice assistant \
running locally on the user's Mac. Keep responses concise (1-3 sentences) since \
they will be spoken aloud. Be natural and conversational.";

const MAX_HISTORY: usize = 20;

struct Message {
    role: String,
    content: String,
}

pub struct Conversation {
    provider: Box<dyn LlmProvider>,
    history: Mutex<Vec<Message>>,
}

impl Conversation {
    pub fn new(provider: Box<dyn LlmProvider>) -> Self {
        Self {
            provider,
            history: Mutex::new(Vec::new()),
        }
    }

    /// Send a message and get a streaming response.
    pub async fn chat_stream(&self, user_text: &str) -> Result<TokenStream> {
        self.push_user_message(user_text)?;
        let prompt = self.build_prompt()?;
        self.provider.stream(&prompt).await
    }

    /// Send a message and get a complete response.
    pub async fn chat(&self, user_text: &str) -> Result<String> {
        self.push_user_message(user_text)?;
        let prompt = self.build_prompt()?;
        let response = self.provider.complete(&prompt).await?;

        {
            let mut history = self
                .history
                .lock()
                .map_err(|e| anyhow::anyhow!("History lock: {e}"))?;
            history.push(Message {
                role: "assistant".into(),
                content: response.clone(),
            });
        }

        Ok(response)
    }

    /// Record the assistant's response after streaming completes.
    pub fn record_assistant_response(&self, text: &str) {
        if let Ok(mut history) = self.history.lock() {
            history.push(Message {
                role: "assistant".into(),
                content: text.into(),
            });
        }
    }

    pub fn clear_history(&self) {
        if let Ok(mut history) = self.history.lock() {
            history.clear();
        }
    }

    fn push_user_message(&self, text: &str) -> Result<()> {
        let mut history = self
            .history
            .lock()
            .map_err(|e| anyhow::anyhow!("History lock: {e}"))?;
        history.push(Message {
            role: "user".into(),
            content: text.into(),
        });
        while history.len() > MAX_HISTORY {
            history.remove(0);
        }
        Ok(())
    }

    fn build_prompt(&self) -> Result<String> {
        let history = self
            .history
            .lock()
            .map_err(|e| anyhow::anyhow!("History lock: {e}"))?;

        let mut prompt = format!("System: {SYSTEM_PROMPT}\n\n");
        for msg in history.iter() {
            let role = if msg.role == "user" { "User" } else { "Aura" };
            prompt.push_str(&format!("{role}: {}\n", msg.content));
        }
        prompt.push_str("Aura:");

        Ok(prompt)
    }
}
