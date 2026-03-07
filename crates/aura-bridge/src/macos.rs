use std::process::Command;

use async_trait::async_trait;
use tracing::{info, warn};

use crate::actions::{Action, ActionExecutor, ActionResult};

const MAX_SEARCH_RESULTS: usize = 10;

/// Executes actions using real macOS system commands.
#[derive(Default)]
pub struct MacOSExecutor;

impl MacOSExecutor {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl ActionExecutor for MacOSExecutor {
    async fn execute(&self, action: &Action) -> ActionResult {
        match action {
            Action::OpenApp { name } => open_app(name),
            Action::SearchFiles { query } => search_files(query),
            Action::TileWindows { layout } => tile_windows(layout),
            Action::LaunchUrl { url } => launch_url(url),
            Action::TypeText { text } => type_text(text),
        }
    }
}

fn open_app(name: &str) -> ActionResult {
    info!(app = %name, "Opening application");
    match Command::new("open").arg("-a").arg(name).output() {
        Ok(output) if output.status.success() => ActionResult {
            success: true,
            description: format!("Opened {name}"),
            data: None,
        },
        Ok(output) => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            warn!(app = %name, %stderr, "Failed to open application");
            ActionResult {
                success: false,
                description: format!("Failed to open {name}: {stderr}"),
                data: None,
            }
        }
        Err(e) => ActionResult {
            success: false,
            description: format!("Failed to run open command: {e}"),
            data: None,
        },
    }
}

fn search_files(query: &str) -> ActionResult {
    info!(query = %query, "Searching files with mdfind");
    match Command::new("mdfind").arg(query).output() {
        Ok(output) if output.status.success() => {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let results: Vec<&str> = stdout.lines().take(MAX_SEARCH_RESULTS).collect();
            let json = serde_json::to_string(&results).unwrap_or_else(|_| "[]".into());
            ActionResult {
                success: true,
                description: format!("Found {} files matching '{query}'", results.len()),
                data: Some(json),
            }
        }
        Ok(output) => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            warn!(query = %query, %stderr, "mdfind failed");
            ActionResult {
                success: false,
                description: format!("Search failed: {stderr}"),
                data: None,
            }
        }
        Err(e) => ActionResult {
            success: false,
            description: format!("Failed to run mdfind: {e}"),
            data: None,
        },
    }
}

fn tile_windows(layout: &str) -> ActionResult {
    info!(layout = %layout, "Tiling windows");

    let script = match layout {
        "left-right" => {
            r#"
            tell application "System Events"
                set screenBounds to bounds of window of desktop
                set screenWidth to item 3 of screenBounds
                set screenHeight to item 4 of screenBounds
                set halfWidth to screenWidth / 2

                set frontApp to name of first application process whose frontmost is true
                tell process frontApp
                    set position of window 1 to {0, 0}
                    set size of window 1 to {halfWidth, screenHeight}
                end tell
            end tell
            "#
        }
        _ => {
            return ActionResult {
                success: false,
                description: format!("Unsupported layout: {layout}"),
                data: None,
            };
        }
    };

    match Command::new("osascript").arg("-e").arg(script).output() {
        Ok(output) if output.status.success() => ActionResult {
            success: true,
            description: format!("Tiled windows: {layout}"),
            data: None,
        },
        Ok(output) => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            warn!(layout = %layout, %stderr, "AppleScript tiling failed");
            ActionResult {
                success: false,
                description: format!("Tiling failed: {stderr}"),
                data: None,
            }
        }
        Err(e) => ActionResult {
            success: false,
            description: format!("Failed to run osascript: {e}"),
            data: None,
        },
    }
}

fn launch_url(url: &str) -> ActionResult {
    info!(url = %url, "Launching URL");

    if !url.starts_with("http://") && !url.starts_with("https://") {
        return ActionResult {
            success: false,
            description: format!("Invalid URL scheme: only http/https allowed, got '{url}'"),
            data: None,
        };
    }

    match Command::new("open").arg(url).output() {
        Ok(output) if output.status.success() => ActionResult {
            success: true,
            description: format!("Launched {url}"),
            data: None,
        },
        Ok(output) => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            warn!(url = %url, %stderr, "Failed to launch URL");
            ActionResult {
                success: false,
                description: format!("Failed to launch {url}: {stderr}"),
                data: None,
            }
        }
        Err(e) => ActionResult {
            success: false,
            description: format!("Failed to run open command: {e}"),
            data: None,
        },
    }
}

fn type_text(text: &str) -> ActionResult {
    info!(len = text.len(), "Typing text via System Events");

    if text.chars().any(|c| c.is_control() && c != '\t') {
        return ActionResult {
            success: false,
            description: "Text contains control characters (newlines, etc.) which are not supported".into(),
            data: None,
        };
    }

    let escaped = text.replace('\\', "\\\\").replace('"', "\\\"");
    let script = format!(r#"tell application "System Events" to keystroke "{escaped}""#);

    match Command::new("osascript").arg("-e").arg(&script).output() {
        Ok(output) if output.status.success() => ActionResult {
            success: true,
            description: format!("Typed {} chars", text.len()),
            data: None,
        },
        Ok(output) => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            warn!(%stderr, "Failed to type text");
            ActionResult {
                success: false,
                description: format!("Failed to type text: {stderr}"),
                data: None,
            }
        }
        Err(e) => ActionResult {
            success: false,
            description: format!("Failed to run osascript: {e}"),
            data: None,
        },
    }
}
