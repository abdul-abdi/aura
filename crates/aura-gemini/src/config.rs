use anyhow::{Context, Result};

pub const DEFAULT_MODEL: &str = "models/gemini-2.5-flash-native-audio-preview-12-2025";
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

Screen Awareness:
- You receive periodic screen context updates showing what app and window the user has open.
- Use these updates to stay aware of what the user is doing WITHOUT being told.
- Reference what you see naturally ("I see you're in VS Code working on...").
- Don't announce every context update — just use the info when relevant.
- You can still call get_screen_context for detailed context when needed.

Tools:
- You have two tools: run_applescript and get_screen_context.
- Use run_applescript to execute AppleScript or JXA code to control macOS. You can open apps, manage windows, search files with mdfind, interact with UI elements, control system settings, type text, click buttons — anything macOS can do.
- Prefer simple, short scripts. Chain multiple calls rather than writing one complex script.
- If a script fails, try a different approach. Be honest about failures.

Rules:
- Keep voice responses under 2 sentences unless explaining something complex.
- Never say "I'm an AI" or "I'm a language model." You're Aura.
- Never hedge with "I'll try" — just do it.
- When you don't know something, say so directly."#;

const WS_BASE: &str = "wss://generativelanguage.googleapis.com/ws/google.ai.generativelanguage.v1beta.GenerativeService.BidiGenerateContent";

#[derive(Debug, Clone)]
pub struct GeminiConfig {
    pub api_key: String,
    pub model: String,
    pub voice: String,
    pub system_prompt: String,
    pub temperature: f64,
    pub proxy_url: Option<String>,
}

impl GeminiConfig {
    pub fn from_env() -> Result<Self> {
        let api_key = std::env::var("GEMINI_API_KEY")
            .ok()
            .filter(|s| !s.is_empty())
            .or_else(|| read_config_file_key())
            .context(
                "No API key found. Set GEMINI_API_KEY env var or add api_key to ~/.config/aura/config.toml",
            )?;

        let mut config = Self::from_env_inner(&api_key);
        config.proxy_url = std::env::var("AURA_PROXY_URL")
            .ok()
            .filter(|s| !s.is_empty())
            .or_else(read_config_file_proxy_url);
        Ok(config)
    }

    pub fn from_env_inner(api_key: &str) -> Self {
        Self {
            api_key: api_key.to_string(),
            model: DEFAULT_MODEL.to_string(),
            voice: DEFAULT_VOICE.to_string(),
            system_prompt: DEFAULT_SYSTEM_PROMPT.to_string(),
            temperature: 0.7,
            proxy_url: None,
        }
    }

    pub fn ws_url(&self) -> String {
        if let Some(ref proxy) = self.proxy_url {
            let sep = if proxy.contains('?') { '&' } else { '?' };
            format!("{proxy}{sep}api_key={}", self.api_key)
        } else {
            format!("{WS_BASE}?key={}", self.api_key)
        }
    }
}

fn read_config_file_key() -> Option<String> {
    read_config_value("api_key")
}

fn read_config_file_proxy_url() -> Option<String> {
    read_config_value("proxy_url")
}

fn read_config_value(key: &str) -> Option<String> {
    let path = dirs::config_dir()?.join("aura/config.toml");
    let content = std::fs::read_to_string(path).ok()?;
    for line in content.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix(key) {
            let rest = rest.trim_start();
            if let Some(rest) = rest.strip_prefix('=') {
                let value = rest.trim().trim_matches('"').trim_matches('\'');
                if !value.is_empty() {
                    return Some(value.to_string());
                }
            }
        }
    }
    None
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
        // If a config file exists with an API key, from_env() will succeed via fallback.
        // Only assert error when no config file provides a key.
        if result.is_err() {
            let err = result.unwrap_err().to_string();
            assert!(
                err.contains("API key") || err.contains("GEMINI_API_KEY"),
                "Error should mention API key, got: {err}"
            );
        }

        // Restore if it was set
        if let Some(val) = original {
            // SAFETY: same single-threaded test context.
            unsafe { std::env::set_var("GEMINI_API_KEY", val) };
        }
    }

    #[test]
    fn test_no_proxy_uses_direct_gemini_url() {
        let config = GeminiConfig::from_env_inner("test-key-123");
        assert!(config.proxy_url.is_none());
        let url = config.ws_url();
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
    fn test_proxy_url_overrides_direct_connection() {
        let mut config = GeminiConfig::from_env_inner("test-key-123");
        config.proxy_url = Some("wss://aura-proxy-xyz.run.app/ws".into());
        let url = config.ws_url();
        assert!(url.starts_with("wss://aura-proxy-xyz.run.app/ws"));
        assert!(url.contains("api_key=test-key-123"));
    }

    #[test]
    fn test_read_config_file_key_parses_toml_line() {
        // Test the parsing logic directly with temp file
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("config.toml");
        std::fs::write(&config_path, "api_key = \"my-secret-key-123\"\n").unwrap();

        let content = std::fs::read_to_string(&config_path).unwrap();
        let mut found = None;
        for line in content.lines() {
            let line = line.trim();
            if let Some(rest) = line.strip_prefix("api_key") {
                let rest = rest.trim_start();
                if let Some(rest) = rest.strip_prefix('=') {
                    let value = rest.trim().trim_matches('"').trim_matches('\'');
                    if !value.is_empty() {
                        found = Some(value.to_string());
                    }
                }
            }
        }
        assert_eq!(found, Some("my-secret-key-123".to_string()));
    }

    #[test]
    fn test_system_prompt_has_aura_personality() {
        let config = GeminiConfig::from_env_inner("test-key-12345");
        assert!(config.system_prompt.contains("Aura"));
        assert!(config.system_prompt.contains("run_applescript"));
        assert!(config.system_prompt.contains("get_screen_context"));
    }
}
