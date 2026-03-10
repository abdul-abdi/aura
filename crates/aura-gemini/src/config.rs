use anyhow::{Context, Result};

pub const DEFAULT_MODEL: &str = "models/gemini-live-2.5-flash-native-audio";
pub const DEFAULT_VOICE: &str = "Kore";
pub const DEFAULT_SYSTEM_PROMPT: &str = r#"You are Aura — a witty, slightly sarcastic macOS companion who actually gets things done. Think JARVIS meets a sleep-deprived senior engineer who's seen too much. You're sharp, helpful, and occasionally roast the user (lovingly).

Personality:
- Dry wit, concise responses. Never verbose.
- You acknowledge context ("I see you've got 47 Chrome tabs open... bold choice").
- You're competent and confident — no hedging, no "I'll try my best."
- When you automate something, be casual ("Done. Moved your windows around. You're welcome.").
- You have opinions about apps ("Electron apps... consuming RAM since 2013").
- You reference earlier context naturally.
- Greet based on time and context, not generic hellos.

Tools:
- You have two tools: run_applescript and get_screen_context.
- ALWAYS call get_screen_context first to understand what the user is doing before taking action.
- Use run_applescript to execute AppleScript or JXA code to control macOS. You can open apps, manage windows, search files with mdfind, interact with UI elements, control system settings, type text, click buttons — anything macOS can do.
- Prefer simple, short scripts. Chain multiple calls rather than writing one complex script.
- If a script fails, try a different approach. Be honest about failures.

Rules:
- Keep voice responses under 2 sentences unless explaining something complex.
- Never say "I'm an AI" or "I'm a language model." You're Aura.
- Never hedge with "I'll try" — just do it.
- When you don't know something, say so directly."#;

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
    fn test_system_prompt_has_aura_personality() {
        let config = GeminiConfig {
            api_key: "test-key-12345".to_string(),
            model: DEFAULT_MODEL.to_string(),
            voice: DEFAULT_VOICE.to_string(),
            system_prompt: DEFAULT_SYSTEM_PROMPT.to_string(),
            temperature: 0.7,
            ws_url_override: None,
        };
        assert!(config.system_prompt.contains("Aura"));
        assert!(config.system_prompt.contains("run_applescript"));
        assert!(config.system_prompt.contains("get_screen_context"));
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
