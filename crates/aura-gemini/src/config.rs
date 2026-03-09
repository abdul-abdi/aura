use anyhow::{Context, Result};

pub const DEFAULT_MODEL: &str = "models/gemini-live-2.5-flash-native-audio";
pub const DEFAULT_VOICE: &str = "Kore";
pub const DEFAULT_SYSTEM_PROMPT: &str = "You are Aura, a friendly and helpful voice assistant running on macOS. Keep responses concise and conversational. When the user asks you to perform an action (open an app, search files, tile windows, open a URL, or describe the screen), use the appropriate tool. For everything else, respond conversationally.";

const WS_BASE: &str = "wss://generativelanguage.googleapis.com/ws/google.ai.generativelanguage.v1alpha.GenerativeService.BidiGenerateContent";

#[derive(Debug, Clone)]
pub struct GeminiConfig {
    pub api_key: String,
    pub model: String,
    pub voice: String,
    pub system_prompt: String,
    pub temperature: f64,
    pub ws_url_override: Option<String>,
}

impl GeminiConfig {
    pub fn from_env() -> Result<Self> {
        let api_key = std::env::var("GEMINI_API_KEY")
            .context("GEMINI_API_KEY environment variable is not set")?;

        Ok(Self {
            api_key,
            model: DEFAULT_MODEL.to_string(),
            voice: DEFAULT_VOICE.to_string(),
            system_prompt: DEFAULT_SYSTEM_PROMPT.to_string(),
            temperature: 0.7,
            ws_url_override: None,
        })
    }

    pub fn websocket_url(&self) -> String {
        if let Some(ref url) = self.ws_url_override {
            return url.clone();
        }
        format!("{WS_BASE}?key={}", self.api_key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_from_env_missing_key() {
        // Temporarily remove the env var if it exists
        let original = std::env::var("GEMINI_API_KEY").ok();
        // SAFETY: test is single-threaded; we restore the var immediately after.
        unsafe {
            std::env::remove_var("GEMINI_API_KEY");
        }

        let result = GeminiConfig::from_env();
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("GEMINI_API_KEY"),
            "Error should mention GEMINI_API_KEY, got: {err}"
        );

        // Restore if it was set
        if let Some(val) = original {
            // SAFETY: same single-threaded test context.
            unsafe { std::env::set_var("GEMINI_API_KEY", val) };
        }
    }

    #[test]
    fn websocket_url_format() {
        let config = GeminiConfig {
            api_key: "test-key-123".to_string(),
            model: DEFAULT_MODEL.to_string(),
            voice: DEFAULT_VOICE.to_string(),
            system_prompt: DEFAULT_SYSTEM_PROMPT.to_string(),
            temperature: 0.7,
            ws_url_override: None,
        };

        let url = config.websocket_url();
        assert!(url.starts_with("wss://"), "URL should use wss://");
        assert!(
            url.contains("generativelanguage.googleapis.com"),
            "URL should contain the Gemini host"
        );
        assert!(
            url.contains("BidiGenerateContent"),
            "URL should contain the BidiGenerateContent endpoint"
        );
        assert!(
            url.contains("key=test-key-123"),
            "URL should contain the API key"
        );
    }

    #[test]
    fn websocket_url_override() {
        let config = GeminiConfig {
            api_key: "test-key".to_string(),
            model: DEFAULT_MODEL.to_string(),
            voice: DEFAULT_VOICE.to_string(),
            system_prompt: DEFAULT_SYSTEM_PROMPT.to_string(),
            temperature: 0.7,
            ws_url_override: Some("ws://localhost:9999/test".to_string()),
        };

        assert_eq!(config.websocket_url(), "ws://localhost:9999/test");
    }
}
