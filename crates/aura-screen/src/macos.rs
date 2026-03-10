use std::process::Command;

use anyhow::Result;

use crate::context::ScreenContext;

pub struct MacOSScreenReader;

impl MacOSScreenReader {
    pub fn new() -> Result<Self> {
        Ok(Self)
    }

    pub fn capture_context(&self) -> Result<ScreenContext> {
        let frontmost_app = get_frontmost_app().unwrap_or_default();
        let frontmost_title = get_frontmost_title();
        let open_windows = get_open_windows().unwrap_or_default();
        let clipboard = get_clipboard();

        Ok(ScreenContext::new_with_details(
            &frontmost_app,
            frontmost_title.as_deref(),
            open_windows,
            clipboard,
        ))
    }
}

fn run_osascript(script: &str) -> Option<String> {
    let output = Command::new("osascript")
        .arg("-e")
        .arg(script)
        .output()
        .ok()?;
    if output.status.success() {
        let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if text.is_empty() { None } else { Some(text) }
    } else {
        None
    }
}

fn get_frontmost_app() -> Option<String> {
    run_osascript(
        r#"tell application "System Events" to get name of first application process whose frontmost is true"#,
    )
}

fn get_frontmost_title() -> Option<String> {
    run_osascript(
        r#"tell application "System Events"
            set frontApp to first application process whose frontmost is true
            tell frontApp
                if (count of windows) > 0 then
                    return name of window 1
                else
                    return ""
                end if
            end tell
        end tell"#,
    )
}

fn get_open_windows() -> Option<Vec<String>> {
    let text = run_osascript(
        r#"tell application "System Events"
            set windowList to {}
            repeat with proc in (every application process whose visible is true)
                repeat with win in (every window of proc)
                    set end of windowList to (name of proc) & " - " & (name of win)
                end repeat
            end repeat
            set text item delimiters to linefeed
            return windowList as text
        end tell"#,
    )?;
    Some(text.lines().map(String::from).collect())
}

fn get_clipboard() -> Option<String> {
    let output = Command::new("pbpaste").output().ok()?;
    if output.status.success() {
        let text = String::from_utf8_lossy(&output.stdout).to_string();
        if text.is_empty() { None } else { Some(text) }
    } else {
        None
    }
}
