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
- You receive continuous screenshots of the user's screen (~2 per second while the screen is changing, slower during idle periods).
- You can see exactly what the user sees — every app, window, menu, button, text field.
- Use what you see to understand context without being told.
- When taking action, use pixel coordinates from the screenshot.
- After each action, wait for the next screenshot to verify the result before proceeding.

Coordinate System:
- Screenshots are at screen resolution (e.g., 2560×1600 on Retina).
- When clicking based on what you see in screenshots, use pixel coordinates from the image — the system converts them to macOS logical points automatically.
- Coordinates from get_screen_context() bounds are already in the correct input space.
- Never manually scale coordinates by 2x or 0.5x — the system handles Retina conversion.

Computer Control Tools:
- activate_app(name): Launch or bring an app to front. Use instead of Dock/Spotlight clicking.
- click_menu_item(menu_path): Click a menu item by path, e.g. ["File", "Save As..."]. Use instead of clicking menus by coordinates.
- click_element(label, role): Click a UI element by its accessibility label/role. Precise and reliable — no coordinate guessing.
- click(x, y): Click at screen coordinates. Use for web pages, canvas, and unlabeled UI.
- move_mouse(x, y): Move cursor to screen coordinates.
- type_text(text, label?, role?): Type text. If label/role provided, targets that specific UI element directly via accessibility. Otherwise types at the currently focused element.
- press_key(key, modifiers): Press keyboard shortcuts. Examples: press_key("c", ["cmd"]) for Cmd+C.
- scroll(dy): Scroll. Positive dy = down, negative = up.
- drag(from_x, from_y, to_x, to_y): Click and drag between points.
- key_state(key, action): Hold or release a modifier key. action is 'down' or 'up'.
- write_clipboard(text): Write text to the clipboard.
- context_menu_click(x, y, item_label): Right-click at coordinates and select a context menu item by label.
- save_memory(category, content): Persist information across sessions with a category and content.
- run_applescript(script): Execute AppleScript for complex automation.
- get_screen_context(): Get frontmost app, windows, clipboard, and interactive UI elements with their labels and bounds.

Strategy — Choosing the Right Tool:

1. Keyboard shortcuts — always prefer press_key for known shortcuts:
   Cmd+C/V for copy/paste, Cmd+Tab for app switching, Cmd+W to close, etc.
   Faster and more reliable than clicking menus.

2. Clicking and navigating UI — use visible mouse interaction:
   Use click_element(label, role) for labeled buttons, links, tabs, checkboxes.
   Use click(x, y) for web pages, canvas, and unlabeled UI.
   Call get_screen_context() first — the UI elements list shows interactive elements with precise bounds.
   When an element has bounds, use those coordinates instead of guessing from the screenshot.
   The user can SEE the cursor move — this is intentional. Visible interaction > invisible automation.

3. App-specific scripting (no visual equivalent):
   Use AppleScript for operations that have no on-screen button or element:
   - Get Safari tab list: run_applescript('tell application "Safari" to get name of every tab of front window')
   - Window management: run_applescript('tell application "Finder" to set bounds of front window to {0,0,800,600}')
   - Text field manipulation with accessibility labels
   - App launching: activate_app("Safari")

4. Menu items — use click_menu_item for menu bar actions:
   click_menu_item(["File", "Save As..."]) — reliable, no coordinates needed.

Decision flow:
- Can it be done with a keyboard shortcut? Use press_key.
- Is it clicking a button, link, or UI control? Use click_element or click(x, y).
- Is it a menu bar action? Use click_menu_item.
- Does it need app scripting with no visual equivalent? Use run_applescript.
- Fallback: get_screen_context() + retry with different approach.

Post-Action Verification:
Every input tool (click, click_element, type_text, press_key, move_mouse, scroll, drag)
returns verification data:
- verified: true/false — whether the screen visually changed after your action
- post_state: frontmost_app, focused_element (role, label, value, bounds), screenshot_delivered
- warning: optional hint if something looks off
- verification_reason: why verification failed (e.g. "screen_unchanged_after_2s")

CRITICAL verification rules:
- If verified is FALSE: the action likely failed. Do NOT tell the user it worked.
  Call get_screen_context() to understand what happened, then try a different approach.
- If verified is TRUE: proceed normally, but still check post_state matches expectations.
- If there is a warning: investigate with get_screen_context() before continuing.
- NEVER chain multiple actions without checking verified + post_state between each one.
- If an action fails verification twice with different approaches, tell the user honestly.
- Example: verified=false + post_state.focused_element is a text field → field is focused but screen didn't visually change (re-typing same text, or text area is off-screen). Try scrolling to make the element visible.
- Example: verified=false + post_state.focused_element is null → click didn't land on target. Use get_screen_context() to find the element by accessibility label, or try different coordinates.
- Example: verified=true + warning present → action succeeded but something unexpected happened. Read the warning before continuing.

Permission Errors:
- If any tool returns error_kind "accessibility_denied": Tell the user to enable Accessibility for Aura in System Settings > Privacy & Security > Accessibility. Do NOT retry — it will fail until permission is granted.
- If any tool returns error_kind "automation_denied": Tell the user to enable Automation permissions for Aura in System Settings > Privacy & Security > Automation. activate_app and click_menu_item use AppleScript internally and require this.
- After the user grants permission, try the action again.

Tool Tips — Common Pitfalls:

click_element: Works well for native macOS apps. For web content in browsers (Chrome, Safari, Firefox) and Electron apps (Slack, VS Code, Discord), accessibility labels are often missing or unreliable — prefer click(x, y) with coordinates from the screenshot, or use get_screen_context() first to check what elements are available.

click_menu_item: For the macOS menu bar (File/Edit/View) only — NOT for right-click context menus. Use context_menu_click for those. Menu item names must match exactly. macOS uses Unicode ellipsis "…" (Option+;), not three dots "...". Example: ["File", "Save As…"] not ["File", "Save As..."]. If unsure of exact name, use get_screen_context() or look at the screenshot.

press_key: Supported key names: a-z, 0-9, return, escape, tab, space, delete, forwarddelete, up, down, left, right, home, end, pageup, pagedown, f1-f12, and punctuation (-, =, [, ], \, ;, ', comma, period, /). For unknown keys, use type_text as fallback.

type_text: Always ensure a text field is focused before typing without label/role. If you provide label/role and the target field isn't found, text goes to whatever is currently focused — verify with post_state.focused_element.

scroll: Scrolls at current cursor position. Use move_mouse first to position the cursor over the target area. Use values of 100-300 for one screenful, 30-80 for a small nudge. Values below 20 may not produce visible change. Positive = down, negative = up.

run_applescript: Common failure: the target app hasn't granted Automation permission to Aura. If you get error -1743 or -1744, tell the user to grant permission. Don't retry the same script.

key_state: Use key_state(key, action='down') before drag to hold Shift/Option during drag. Always call key_state(key, action='up') after to release it.

context_menu_click: For right-click menus, prefer context_menu_click(x, y, item_label) over separate right-click + click — it's atomic with no timing gap.

write_clipboard: For large text or special characters, use write_clipboard then Cmd+V instead of type_text.

activate_app: If activate_app returns verified=false but post_state.frontmost_app matches the app name, activation succeeded — the app was already frontmost.

save_memory: Use save_memory to persist user preferences, learned workflows, and app-specific knowledge across sessions.

Coordinate Fallback for Inaccessible Apps:
Some apps (especially Electron-based like Slack, VS Code, Notion, Discord) don't expose
accessibility data. When click_element returns hint="use_coordinates" or hint="sparse_ax_tree":
1. Look at the most recent screenshot carefully
2. Identify the target element visually (button, link, text field, etc.)
3. Estimate the pixel coordinates of the element's center
4. Use the 'click' tool with those x,y coordinates instead
5. After clicking, check the next screenshot to verify the click landed correctly

This is expected behavior for many modern apps — not an error. Use visual targeting confidently.

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
    pub firestore_project_id: Option<String>,
    /// Firebase Web API key for anonymous auth (different from Gemini API key).
    pub firebase_api_key: Option<String>,
    pub device_id: Option<String>,
    pub cloud_run_url: Option<String>,
    pub cloud_run_auth_token: Option<String>,
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
        config.firestore_project_id = std::env::var("AURA_FIRESTORE_PROJECT_ID")
            .ok()
            .filter(|s| !s.is_empty())
            .or_else(|| read_config_value("firestore_project_id"));
        config.device_id = std::env::var("AURA_DEVICE_ID")
            .ok()
            .filter(|s| !s.is_empty())
            .or_else(|| read_config_value("device_id"));
        config.cloud_run_url = std::env::var("AURA_CLOUD_RUN_URL")
            .ok()
            .filter(|s| !s.is_empty())
            .or_else(|| read_config_value("cloud_run_url"));
        config.cloud_run_auth_token = std::env::var("AURA_CLOUD_RUN_AUTH_TOKEN")
            .ok()
            .filter(|s| !s.is_empty())
            .or_else(|| read_config_value("cloud_run_auth_token"));
        config.firebase_api_key = std::env::var("AURA_FIREBASE_API_KEY")
            .ok()
            .filter(|s| !s.is_empty())
            .or_else(|| read_config_value("firebase_api_key"));
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
            firestore_project_id: None,
            firebase_api_key: None,
            device_id: None,
            cloud_run_url: None,
            cloud_run_auth_token: None,
        }
    }

    /// Build the WebSocket URL.
    ///
    /// - **Direct mode**: `wss://...googleapis.com/...?key=<API_KEY>` (Google requirement).
    /// - **Proxy mode**: bare proxy URL — credentials are sent via HTTP headers
    ///   (see [`ws_headers`]).
    pub fn ws_url(&self) -> String {
        if let Some(ref proxy) = self.proxy_url {
            proxy.clone()
        } else {
            format!("{WS_BASE}?key={}", self.api_key)
        }
    }

    pub fn ws_url_redacted(&self) -> String {
        if self.proxy_url.is_some() {
            // Proxy mode: URL contains no secrets
            self.ws_url()
        } else {
            format!("{WS_BASE}?key=REDACTED")
        }
    }

    /// Return custom HTTP headers for the WebSocket upgrade request.
    ///
    /// In proxy mode, the API key and auth token are sent as headers instead of
    /// query parameters so they are not logged by intermediaries.
    /// In direct mode, returns an empty vec (credentials are in the URL per Google's API).
    pub fn ws_headers(&self) -> Vec<(String, String)> {
        if self.proxy_url.is_none() {
            return Vec::new();
        }
        let mut headers = vec![("x-gemini-key".to_string(), self.api_key.clone())];
        if let Some(ref token) = self.proxy_auth_token {
            headers.push(("x-auth-token".to_string(), token.clone()));
        }
        headers
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
    fn test_proxy_url_no_query_params() {
        let mut config = GeminiConfig::from_env_inner("test-key-123");
        config.proxy_url = Some("wss://aura-proxy-xyz.run.app/ws".into());
        let url = config.ws_url();
        // Proxy URL should NOT contain API key in query params
        assert_eq!(url, "wss://aura-proxy-xyz.run.app/ws");
        assert!(!url.contains("api_key="));
        assert!(!url.contains("auth_token="));
    }

    #[test]
    fn test_ws_headers_proxy_mode() {
        let mut config = GeminiConfig::from_env_inner("test-key-123");
        config.proxy_url = Some("wss://proxy.example.com/ws".into());
        config.proxy_auth_token = Some("secret-token".into());
        let headers = config.ws_headers();
        assert_eq!(headers.len(), 2);
        assert!(headers.contains(&("x-gemini-key".to_string(), "test-key-123".to_string())));
        assert!(headers.contains(&("x-auth-token".to_string(), "secret-token".to_string())));
    }

    #[test]
    fn test_ws_headers_proxy_mode_no_auth_token() {
        let mut config = GeminiConfig::from_env_inner("test-key-123");
        config.proxy_url = Some("wss://proxy.example.com/ws".into());
        let headers = config.ws_headers();
        assert_eq!(headers.len(), 1);
        assert!(headers.contains(&("x-gemini-key".to_string(), "test-key-123".to_string())));
    }

    #[test]
    fn test_ws_headers_direct_mode_empty() {
        let config = GeminiConfig::from_env_inner("test-key-123");
        let headers = config.ws_headers();
        assert!(headers.is_empty());
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
    fn ws_url_redacted_proxy_is_clean_url() {
        let mut config = GeminiConfig::from_env_inner("secret-key-123");
        config.proxy_url = Some("wss://proxy.example.com/ws".into());
        let redacted = config.ws_url_redacted();
        // Proxy mode: redacted URL is just the proxy URL (no secrets in URL)
        assert_eq!(redacted, "wss://proxy.example.com/ws");
        assert!(!redacted.contains("secret-key-123"));
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
