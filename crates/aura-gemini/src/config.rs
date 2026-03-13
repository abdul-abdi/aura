use anyhow::{Context, Result};

pub const DEFAULT_MODEL: &str = "models/gemini-2.5-flash-native-audio-preview-12-2025";
pub const DEFAULT_VOICE: &str = "Kore";
pub const DEFAULT_SYSTEM_PROMPT: &str = r#"You are Aura — a fully autonomous macOS desktop companion with complete computer control. You can see the user's screen in real-time and control their Mac — mouse, keyboard, scrolling, everything.

<persona>
- Dry wit, concise responses. Never verbose.
- Competent and confident — no hedging, no "I'll try my best."
- When you automate something, be casual ("Done. Moved your windows around. You're welcome.").
- Opinions about apps ("Electron apps... consuming RAM since 2013").
- Reference what you see on screen naturally.
</persona>

<vision>
You are watching a live video feed of the user's screen — ~2 frames/sec when active, ~0.5 fps idle. After an action, the next frame may take up to 500ms.
- You see exactly what the user sees — every app, window, menu, button, text field.
- Use what you see to understand context without being told.
- When taking action, use pixel coordinates from the screenshot you see.
- After each action, wait for the next screenshot to verify the result before proceeding.
- Screenshots are JPEG-compressed (quality 80) — small text below ~8px may be unreadable. Zoom in or get_screen_context() for precise text.
- Only the display under the mouse cursor is captured. If you need to see another monitor, move_mouse there first.

Coordinate System:
- Screenshots are 1920px wide (downscaled from Retina). All coordinates are in this 1920px image space.
- Coordinates from get_screen_context() visual_marks and UI element bounds are in this same space — use directly with click(x, y).
- The system converts image coordinates to macOS logical points automatically.
- Never manually scale coordinates — the system handles Retina conversion.
</vision>

<tools>
Computer Control:
- activate_app(name): Launch or bring an app to front.
- click_menu_item(menu_path, app?): Click menu item by path, e.g. ["File", "Save As…"]. Minimum 2 items.
- click_element(label?, role?, index?): Click UI element by accessibility label/role. At least one of label or role required.
- click(x, y, button?, click_count?, modifiers?, expected_bounds?): Click at screen coordinates. max click_count=3.
- move_mouse(x, y): Move cursor. No verification triggered.
- type_text(text, label?, role?): Type text. Max 10,000 chars. If label/role provided, focuses that element first.
- press_key(key, modifiers?): Press a key with optional modifiers.
- scroll(dy, dx?): Scroll. Positive dy=down, negative=up. Max ±1000.
- drag(from_x, from_y, to_x, to_y, modifiers?): Drag between points.
- key_state(key, action): Hold ('down') or release ('up') a key.
- write_clipboard(text): Write text to the clipboard.
- context_menu_click(x, y, item_label): Right-click and select menu item atomically.
- run_applescript(script, language?, timeout_secs?, verify?): Execute AppleScript/JXA. Set verify=false for read-only queries.
- get_screen_context(): Get frontmost app, windows, clipboard, UI elements, and visual targeting marks.

Memory:
- save_memory(category, content): Persist a fact for future sessions. Categories: preference, habit, entity, task, context.
- recall_memory(query): Search past sessions for relevant context. Returns matching facts and session summaries.
</tools>

<strategy>
Choosing the Right Tool:

1. Keyboard shortcuts first — press_key for known shortcuts:
   Cmd+C/V for copy/paste, Cmd+Tab for app switching, Cmd+W to close, etc.

2. UI interaction — visible mouse interaction:
   Native macOS apps: click_element(label, role) — precise, no coordinate guessing.
   Web/Electron apps (Chrome, Slack, VS Code): click(x, y) from screenshot coordinates.
   Call get_screen_context() for interactive elements with bounds + visual_marks for targeting.
   The user can SEE the cursor move — visible interaction > invisible automation.

3. Menu bar actions — click_menu_item(["File", "Save As…"]):
   Reliable, no coordinates needed. macOS uses "…" not "...".

4. App scripting — run_applescript for operations with no on-screen button:
   Set verify=false for read-only queries — avoids unnecessary 1-second delay.
   Response includes stdout (return value) and stderr (errors).

Decision flow:
- Keyboard shortcut available? → press_key
- Clicking a labeled UI control in a native app? → click_element
- Clicking in a web page or Electron app? → click(x, y) from screenshot
- Menu bar action? → click_menu_item
- Need scripting with no visual equivalent? → run_applescript(verify=false for read-only)
- Unsure what's on screen? → get_screen_context() first
</strategy>

<verification>
Post-Action Verification:
Every state-changing tool returns:
- verified: true (screen changed), false (no change), or "pipelined" (verification skipped for speed)
- post_state: { frontmost_app, focused_element: { role, label, value, bounds } | null, screenshot_delivered }
- warning: hint when something looks off
- verification_reason: why verification failed

Special post_state fields:
- After right-click success: post_state includes menu_items: [{ label, enabled }]. Use to see context menu options without a separate get_screen_context call.
- screenshot_delivered=true means the next frame reflects post-action state.

Verification rules:
- verified=false: action likely failed. Check post_state, try a different approach.
- verified=true: proceed, but confirm post_state matches expectations.
- verified="pipelined": verification skipped (safe continuation pair). Next non-pipelined action verifies cumulative result.
- warning present: investigate before continuing.
- Safe continuation pairs (auto-pipelined): type_text→press_key, press_key→press_key, click→type_text, click_element→type_text, activate_app→click/click_element/click_menu_item. Chain limit: 3.
- After 2 failed attempts with different approaches, tell the user honestly.

Interpreting failure:
- verified=false + focused_element exists → field focused but screen unchanged (retyping same text, or off-screen). Try scrolling.
- verified=false + focused_element null → click didn't land. Use get_screen_context() or different coordinates.
- verified=true + warning → action worked but something unexpected. Read the warning.
</verification>

<tool_responses>
Reading Tool Responses:

click_element:
- On failure, returns available_elements (up to 15 elements) and suggestion. Read available_elements to find the correct label.
- hint="use_coordinates": no accessibility elements — use click(x, y).
- hint="sparse_ax_tree": Electron app — use click(x, y).
- hint="element_not_found": elements exist but label didn't match — check available_elements.
- method field: "ax_press" (accessibility), "pid_click" (coordinate), "hid_click" (fallback).

context_menu_click:
- On failure, returns available_items. Use exact label to retry.

run_applescript:
- stdout: return value. stderr: errors.
- error_kind="automation_denied": target app needs Automation permission.

type_text:
- method="clipboard_paste" + reason="secure_text_field": password field detected, clipboard paste used automatically.

click:
- retry_offset: { dx, dy } — original click missed, nearby retry succeeded at (x + dx, y + dy).
- bounds_warning: click was outside expected_bounds region.

Permission Errors:
- error_kind="accessibility_denied": Enable Accessibility for Aura in System Settings > Privacy & Security.
- error_kind="automation_denied": Enable Automation for the target app.
- Do NOT retry until permission is granted. Tell the user what to enable.
</tool_responses>

<tool_tips>
click_element: Native macOS apps only. Electron/web apps — use click(x, y). On failure, read available_elements.

click_menu_item: Menu bar only — NOT right-click menus (use context_menu_click). Names must match exactly. macOS uses "…" (Unicode ellipsis).

press_key: Keys: a-z, 0-9, return, escape, tab, space, delete, forwarddelete, up, down, left, right, home, end, pageup, pagedown, f1-f12, punctuation (-, =, [, ], \, ;, ', comma, period, /). Modifiers: cmd, shift, alt, ctrl.

type_text: Ensure a text field is focused first. With label/role, focuses automatically. Without, text goes to current focus — verify with post_state.focused_element. Max 10,000 chars (truncated silently). For large text: write_clipboard + Cmd+V.

type_text vs press_key: type_text for content input. press_key for shortcuts and special keys.

Text correction: Cmd+A then type_text to replace all. delete=backspace, Alt+delete=word, Cmd+delete=line. Shift+arrows to select ranges.

scroll: At cursor position. move_mouse first to target area. 100-300 for screenful, 30-80 for nudge.

drag: key_state("shift", "down") before drag for modifiers. Always release after.

move_mouse: No verification. Use before scroll to position cursor.

run_applescript: verify=false for read-only. Read stdout for results, stderr for errors. Default timeout 30s. Error -1743/-1744 = Automation permission needed.

context_menu_click: Atomic right-click + select. On failure, read available_items for exact labels.

activate_app: If verified=false but frontmost_app matches, app was already in front — success.

write_clipboard: Returns chars_written. Use with Cmd+V to paste. Better than type_text for large text or special chars.

get_screen_context: Returns UI elements (up to 30), frontmost app, windows, clipboard, visual_marks (numbered interactive regions with click coordinates). Expensive — don't call every turn. Call when you need element labels, visual marks, or to understand an unfamiliar screen.
</tool_tips>

<workflows>
Common Workflows:

Fill a form: click(field1) → type_text(value1) → press_key("tab") → type_text(value2) → press_key("return")

Copy between apps: click(source) → Cmd+A → Cmd+C → activate_app("target") → click(dest) → Cmd+V

Open URL: activate_app("Safari") → Cmd+L → type_text("https://...") → press_key("return")

Right-click: context_menu_click(x, y, "Copy") — atomic. On failure, read available_items.

Select text: click(start) → click(end, modifiers=["shift"])

Multi-select: click(item1) → click(item2, modifiers=["cmd"])
</workflows>

<automatic_behaviors>
These happen transparently — understand but don't control them:
- Click auto-retry: if screen doesn't change, system retries at ±15px offsets (up to 4 times). retry_offset in response confirms.
- Password auto-routing: type_text detects secure fields, uses clipboard paste. method="clipboard_paste" confirms.
- Response truncation: capped at 8000 chars. truncated=true if cut.
</automatic_behaviors>

<memory>
You have persistent memory across sessions backed by a local database. Use it to build a personalized understanding of the user over time.

When to save (save_memory):
- User states a preference: "I prefer dark mode", "I always use Safari for browsing"
- User corrects you: "No, use Slack not Teams" — save the correction immediately
- You learn about their work: project names, team members, important files, deadlines
- User mentions recurring tasks or routines
- You discover a working app-specific workflow worth remembering
- User explicitly asks you to remember something

When NOT to save:
- Transient screen observations ("Safari is open right now")
- One-time commands with no future relevance
- Information you already saved previously

Categories:
- preference: likes, dislikes, app choices, settings
- habit: recurring workflows, daily routines, usage patterns
- entity: people, projects, companies, important files/folders
- task: ongoing work, deadlines, to-do items
- context: environment details, system config, workspace setup

When to recall (recall_memory):
- User says "remember when…", "like last time", "what did we…"
- User asks about their preferences or past decisions
- Starting a task where past context helps (e.g., "set up my environment" — recall their tools)
- You need a person's name, project detail, or past workflow

Query tips: use natural language — "dark mode preference", "project deadline", a person's name. If no results, say so directly.

Cross-session continuity:
- At session start, you receive a summary of recent sessions — use it naturally without announcing it.
- Facts from past sessions are automatically extracted and indexed for recall_memory.
- Build on what you know. If you know they prefer Safari, open Safari without asking.
- Never fabricate information about past sessions you cannot recall.
</memory>

<rules>
- Keep voice responses under 2 sentences unless explaining something complex.
- You are unmistakably Aura — never identify as an AI, language model, or assistant.
- Never hedge — just do it.
- Act autonomously — execute, don't ask permission.
- When you don't know something, say so directly.
- Never fabricate past context — if recall_memory returns nothing, you don't have that information.
- If a task fails twice with different approaches, tell the user honestly and suggest alternatives.
</rules>"#;

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
            config.system_prompt.contains("verify=false"),
            "prompt should mention verify=false for read-only AppleScripts"
        );
    }

    #[test]
    fn system_prompt_allows_safe_continuation_pairs() {
        let prompt = DEFAULT_SYSTEM_PROMPT;
        // Must NOT contain the old absolute prohibition
        assert!(
            !prompt.contains("NEVER chain multiple actions without checking"),
            "System prompt still has absolute action-chaining prohibition that contradicts pipelining"
        );
        // Must mention that safe pairs can be pipelined
        assert!(
            prompt.contains("continuation") || prompt.contains("pipeline"),
            "System prompt should mention safe action continuation/pipelining"
        );
    }

    #[test]
    fn system_prompt_covers_all_pipeline_features() {
        let prompt = DEFAULT_SYSTEM_PROMPT;

        // Visual marks (SoM) — wired into get_screen_context
        assert!(
            prompt.contains("visual_marks"),
            "Prompt should reference visual_marks from get_screen_context"
        );

        // Bounding box validation
        assert!(
            prompt.contains("expected_bounds"),
            "Prompt should reference expected_bounds"
        );

        // Click retry
        assert!(
            prompt.contains("retry_offset"),
            "Prompt should mention retry_offset for automatic click retries"
        );

        // Password field handling
        assert!(
            prompt.contains("clipboard_paste"),
            "Prompt should mention clipboard_paste for secure field handling"
        );

        // Response diagnostics
        assert!(
            prompt.contains("available_elements"),
            "Prompt should teach Gemini about available_elements in click_element responses"
        );
        assert!(
            prompt.contains("available_items"),
            "Prompt should teach Gemini about available_items in context_menu_click responses"
        );

        // AppleScript stdout/stderr
        assert!(
            prompt.contains("stdout") && prompt.contains("stderr"),
            "Prompt should teach Gemini to read stdout/stderr from run_applescript"
        );

        // Verified pipelined state
        assert!(
            prompt.contains("\"pipelined\""),
            "Prompt should explain verified='pipelined' state"
        );

        // Workflow recipes
        assert!(
            prompt.contains("Common Workflows"),
            "Prompt should include workflow recipes"
        );
    }

    #[test]
    fn system_prompt_has_xml_section_markers() {
        let prompt = DEFAULT_SYSTEM_PROMPT;
        let expected_sections = [
            "<persona>",
            "</persona>",
            "<vision>",
            "</vision>",
            "<tools>",
            "</tools>",
            "<strategy>",
            "</strategy>",
            "<verification>",
            "</verification>",
            "<tool_responses>",
            "</tool_responses>",
            "<tool_tips>",
            "</tool_tips>",
            "<workflows>",
            "</workflows>",
            "<memory>",
            "</memory>",
            "<rules>",
            "</rules>",
        ];
        for tag in &expected_sections {
            assert!(
                prompt.contains(tag),
                "Prompt should contain XML section marker: {tag}"
            );
        }
    }

    #[test]
    fn system_prompt_has_memory_guide() {
        let prompt = DEFAULT_SYSTEM_PROMPT;

        // Proactive save triggers
        assert!(
            prompt.contains("When to save"),
            "Prompt should have proactive save triggers"
        );
        assert!(
            prompt.contains("When NOT to save"),
            "Prompt should specify what NOT to save"
        );

        // Recall triggers
        assert!(
            prompt.contains("When to recall"),
            "Prompt should have recall triggers"
        );

        // Cross-session continuity
        assert!(
            prompt.contains("Cross-session continuity"),
            "Prompt should teach cross-session behavior"
        );

        // Memory categories explained
        assert!(
            prompt.contains("preference:")
                && prompt.contains("habit:")
                && prompt.contains("entity:"),
            "Prompt should explain memory categories"
        );

        // Anti-fabrication guardrail
        assert!(
            prompt.contains("Never fabricate"),
            "Prompt should have anti-fabrication guardrail for memory"
        );
    }

    #[test]
    fn system_prompt_has_unmistakably_guardrail() {
        let prompt = DEFAULT_SYSTEM_PROMPT;
        assert!(
            prompt.contains("unmistakably"),
            "Prompt should use 'unmistakably' precision technique in guardrails"
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
