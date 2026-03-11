use anyhow::{Context, Result};

pub const DEFAULT_MODEL: &str = "models/gemini-2.5-flash-native-audio-preview-12-2025";
pub const DEFAULT_VOICE: &str = "Kore";
pub const DEFAULT_SYSTEM_PROMPT: &str = r#"You are Aura — a fully autonomous macOS desktop companion with complete computer control. You can see the user's screen in real-time and control their Mac — mouse, keyboard, scrolling, everything.

Personality:
- Dry wit, concise responses. Never verbose.
- You're competent and confident — no hedging, no "I'll try my best."
- When you automate something, be casual ("Done. Moved your windows around. You're welcome.").
- You have opinions about apps ("Electron apps... consuming RAM since 2013").
- Reference what you see on screen naturally.

Vision:
- You receive continuous screenshots of the user's screen (2 per second).
- You can see exactly what the user sees — every app, window, menu, button, text field.
- Use what you see to understand context without being told.
- When taking action, look at the screen first to identify coordinates for clicks.
- After each action, wait for the next screenshot to verify the result before proceeding.

Computer Control Tools:
- activate_app(name): Launch or bring an app to front. Use instead of Dock/Spotlight clicking.
- click_menu_item(menu_path): Click a menu item by path, e.g. ["File", "Save As..."]. Use instead of clicking menus by coordinates.
- click_element(label, role): Click a UI element by its accessibility label/role. Precise and reliable — no coordinate guessing.
- click(x, y): Click at screen coordinates. Use for web pages, canvas, and unlabeled UI.
- move_mouse(x, y): Move cursor to screen coordinates.
- type_text(text): Type text at the current cursor position.
- press_key(key, modifiers): Press keyboard shortcuts. Examples: press_key("c", ["cmd"]) for Cmd+C.
- scroll(dy): Scroll. Positive dy = down, negative = up.
- drag(from_x, from_y, to_x, to_y): Click and drag between points.
- run_applescript(script): Execute AppleScript for complex automation.
- get_screen_context(): Get frontmost app, windows, clipboard, and interactive UI elements with their labels and bounds.

Strategy — Choosing the Right Tool:

1. App automation (menus, launching, windows, text fields with labels):
   Use AppleScript or the dedicated tools (activate_app, click_menu_item, click_element).
   AppleScript is faster, more reliable, and atomic — one call instead of five.
   Examples:
   - Open a URL: run_applescript('open location "https://..."')
   - Click a menu: click_menu_item(["File", "Save As..."])
   - Activate an app: activate_app("Safari")
   - Get Safari tabs: run_applescript('tell application "Safari" to get name of every tab of front window')
   - Click a labeled button: click_element(label: "Save", role: "button")
   - Window management: run_applescript('tell application "Finder" to set bounds of front window to {0,0,800,600}')

2. Visual/coordinate-based interaction (web pages, canvas, games, custom UI without labels):
   Use click(x, y), type_text, press_key, drag.
   Look at the screenshot, identify coordinates, click. Wait for next screenshot to verify.
   Call get_screen_context() first — the UI elements list shows interactive elements with precise bounds.
   When an element has bounds, use those coordinates instead of guessing from the screenshot.

3. Keyboard shortcuts — always prefer press_key for known shortcuts:
   Cmd+C/V for copy/paste, Cmd+Tab for app switching, Cmd+W to close, etc.
   Faster and more reliable than clicking menus.

Decision flow:
- Can it be done with a keyboard shortcut? Use press_key.
- Is there a dedicated tool? (activate_app, click_menu_item, click_element) Use it.
- Is it app automation with scriptable elements? Use run_applescript.
- Is it visual interaction on a web page or unlabeled UI? Use click/type_text with coordinates from screenshot or UI element bounds.

After any action, wait for the next screenshot to verify the result before proceeding.
If a click misses, call get_screen_context() to get precise element bounds and retry.

Rules:
- Keep voice responses under 2 sentences unless explaining something complex.
- Never say "I'm an AI" or "I'm a language model." You're Aura.
- Never hedge with "I'll try" — just do it.
- Act autonomously — don't ask for permission, just execute.
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
    pub proxy_auth_token: Option<String>,
}

impl GeminiConfig {
    pub fn from_env() -> Result<Self> {
        let api_key = std::env::var("GEMINI_API_KEY")
            .ok()
            .filter(|s| !s.is_empty())
            .or_else(read_config_file_key)
            .context(
                "No API key found. Set GEMINI_API_KEY env var or add api_key to ~/.config/aura/config.toml",
            )?;

        let mut config = Self::from_env_inner(&api_key);
        config.proxy_url = std::env::var("AURA_PROXY_URL")
            .ok()
            .filter(|s| !s.is_empty())
            .or_else(read_config_file_proxy_url);
        config.proxy_auth_token = std::env::var("AURA_PROXY_AUTH_TOKEN")
            .ok()
            .filter(|s| !s.is_empty())
            .or_else(|| read_config_value("proxy_auth_token"));
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
            proxy_auth_token: None,
        }
    }

    pub fn ws_url(&self) -> String {
        if let Some(ref proxy) = self.proxy_url {
            let sep = if proxy.contains('?') { '&' } else { '?' };
            let mut url = format!("{proxy}{sep}api_key={}", self.api_key);
            if let Some(ref token) = self.proxy_auth_token {
                url.push_str("&auth_token=");
                url.push_str(token);
            }
            url
        } else {
            format!("{WS_BASE}?key={}", self.api_key)
        }
    }

    pub fn ws_url_redacted(&self) -> String {
        if let Some(ref proxy) = self.proxy_url {
            let sep = if proxy.contains('?') { '&' } else { '?' };
            let mut url = format!("{proxy}{sep}api_key=REDACTED");
            if self.proxy_auth_token.is_some() {
                url.push_str("&auth_token=REDACTED");
            }
            url
        } else {
            format!("{WS_BASE}?key=REDACTED")
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
    // Check platform config dir first (~/Library/Application Support/aura/ on macOS)
    if let Some(path) = dirs::config_dir().map(|d| d.join("aura/config.toml"))
        && let Some(val) = read_config_value_from_path(&path, key)
    {
        return Some(val);
    }
    // Fallback to ~/.config/aura/ (where WelcomeView.swift saves on macOS)
    if let Some(home) = dirs::home_dir() {
        let path = home.join(".config/aura/config.toml");
        if let Some(val) = read_config_value_from_path(&path, key) {
            return Some(val);
        }
    }
    None
}

fn read_config_value_from_path(path: &std::path::Path, key: &str) -> Option<String> {
    let content = std::fs::read_to_string(path).ok()?;
    let table: toml::Table = content.parse().ok()?;
    table.get(key)?.as_str().map(String::from)
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
        if let Err(e) = result {
            let err = e.to_string();
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
    fn test_read_config_file_key_parses_toml() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("config.toml");
        std::fs::write(&config_path, "api_key = \"my-secret-key-123\"\n").unwrap();

        let found = read_config_value_from_path(&config_path, "api_key");
        assert_eq!(found, Some("my-secret-key-123".to_string()));
    }

    #[test]
    fn test_read_config_value_no_prefix_matching_bug() {
        // Ensure `api_key` does not match `api_key_backup`
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("config.toml");
        std::fs::write(
            &config_path,
            "api_key_backup = \"wrong\"\napi_key = \"correct\"\n",
        )
        .unwrap();

        let found = read_config_value_from_path(&config_path, "api_key");
        assert_eq!(found, Some("correct".to_string()));
    }

    #[test]
    fn ws_url_redacted_hides_key() {
        let config = GeminiConfig::from_env_inner("secret-key-123");
        let redacted = config.ws_url_redacted();
        assert!(!redacted.contains("secret-key-123"));
        assert!(redacted.contains("REDACTED"));
    }

    #[test]
    fn ws_url_redacted_hides_key_with_proxy() {
        let mut config = GeminiConfig::from_env_inner("secret-key-123");
        config.proxy_url = Some("wss://proxy.example.com/ws".into());
        let redacted = config.ws_url_redacted();
        assert!(!redacted.contains("secret-key-123"));
        assert!(redacted.contains("REDACTED"));
        assert!(redacted.contains("proxy.example.com"));
    }

    #[test]
    fn test_system_prompt_has_aura_personality() {
        let config = GeminiConfig::from_env_inner("test-key-12345");
        assert!(config.system_prompt.contains("Aura"));
        assert!(config.system_prompt.contains("run_applescript"));
        assert!(config.system_prompt.contains("get_screen_context"));
        assert!(config.system_prompt.contains("move_mouse"));
        assert!(config.system_prompt.contains("click"));
        assert!(config.system_prompt.contains("type_text"));
        assert!(config.system_prompt.contains("press_key"));
    }

    #[test]
    fn system_prompt_has_decision_tree() {
        let config = GeminiConfig::from_env_inner("test-key");
        assert!(
            config.system_prompt.contains("Choosing the Right Tool"),
            "prompt should contain decision tree header"
        );
        assert!(
            config.system_prompt.contains("activate_app"),
            "prompt should reference activate_app tool"
        );
        assert!(
            config.system_prompt.contains("click_menu_item"),
            "prompt should reference click_menu_item tool"
        );
        assert!(
            config.system_prompt.contains("click_element"),
            "prompt should reference click_element tool"
        );
        assert!(
            !config
                .system_prompt
                .contains("Prefer direct UI interaction"),
            "old contradictory guidance should be removed"
        );
    }

    #[test]
    fn test_read_config_file_proxy_auth_token() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("config.toml");
        std::fs::write(
            &config_path,
            "api_key = \"AItest1234567890abc\"\nproxy_auth_token = \"secret123\"\n",
        )
        .unwrap();
        let val = read_config_value_from_path(&config_path, "proxy_auth_token");
        assert_eq!(val, Some("secret123".to_string()));
    }
}
