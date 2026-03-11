use std::process::Command;

use anyhow::Result;

use crate::context::ScreenContext;

#[derive(Clone, Default)]
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

        // Walk the accessibility tree for interactive UI elements.
        // Returns empty Vec if Accessibility permission not granted or on timeout.
        let ui_elements = crate::accessibility::get_focused_app_elements();

        Ok(ScreenContext::new_with_details(
            &frontmost_app,
            frontmost_title.as_deref(),
            open_windows,
            clipboard,
        )
        .with_ui_elements(ui_elements))
    }
}

/// Run a JXA (JavaScript for Automation) script via osascript.
/// Uses the ObjC bridge for direct API access — no inter-app communication,
/// so no Automation consent popup (unlike `tell application "System Events"`).
pub fn run_jxa(script: &str) -> Option<String> {
    let output = Command::new("osascript")
        .arg("-l")
        .arg("JavaScript")
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

/// Get the process ID of the frontmost application.
pub fn get_frontmost_pid() -> Option<i32> {
    run_jxa(
        "ObjC.import('AppKit'); $.NSWorkspace.sharedWorkspace.frontmostApplication.processIdentifier",
    )?
    .trim()
    .parse()
    .ok()
}

/// Get frontmost app name via NSWorkspace (no Automation permission needed).
fn get_frontmost_app() -> Option<String> {
    run_jxa(
        "ObjC.import('AppKit'); $.NSWorkspace.sharedWorkspace.frontmostApplication.localizedName.js",
    )
}

/// Get frontmost window title via CGWindowListCopyWindowInfo.
/// Needs Screen Recording permission (already requested) but NOT Automation permission.
fn get_frontmost_title() -> Option<String> {
    run_jxa(
        r#"ObjC.import('CoreGraphics');
ObjC.import('Cocoa');
var pid = $.NSWorkspace.sharedWorkspace.frontmostApplication.processIdentifier;
var list = ObjC.deepUnwrap($.CGWindowListCopyWindowInfo($.kCGWindowListOptionOnScreenOnly, 0));
var title = '';
for (var i = 0; i < list.length; i++) {
    var w = list[i];
    if (w.kCGWindowOwnerPID === pid && w.kCGWindowName && w.kCGWindowLayer === 0) {
        title = w.kCGWindowName;
        break;
    }
}
title;"#,
    )
}

/// Get all visible windows via CGWindowListCopyWindowInfo.
/// Needs Screen Recording permission (already requested) but NOT Automation permission.
fn get_open_windows() -> Option<Vec<String>> {
    let text = run_jxa(
        r#"ObjC.import('CoreGraphics');
ObjC.import('Cocoa');
var opts = $.kCGWindowListOptionOnScreenOnly | $.kCGWindowListExcludeDesktopElements;
var list = ObjC.deepUnwrap($.CGWindowListCopyWindowInfo(opts, 0));
var results = [];
for (var i = 0; i < list.length; i++) {
    var w = list[i];
    if (w.kCGWindowLayer === 0 && w.kCGWindowOwnerName) {
        var name = w.kCGWindowName || '';
        if (name) results.push(w.kCGWindowOwnerName + ' - ' + name);
    }
}
results.join('\n');"#,
    )?;
    if text.is_empty() {
        return None;
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capture_context_returns_screen_context_with_elements_field() {
        let reader = MacOSScreenReader::new().unwrap();
        let ctx = reader.capture_context().unwrap();
        // ui_elements() should exist and be callable (may be empty without permissions)
        let _elements = ctx.ui_elements();
    }
}
