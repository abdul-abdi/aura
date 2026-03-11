//! Silent preflight check for macOS Automation (AppleEvents) permission.
//!
//! Uses `AEDeterminePermissionToAutomateTarget` with `askUserIfNeeded = false`
//! so it NEVER triggers a macOS popup. Returns the current permission state
//! for a given target app.

use std::ffi::c_void;

/// Result of an Automation permission preflight check.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AutomationPermission {
    /// User has granted Automation access to the target app.
    Granted,
    /// User explicitly denied access. Script would fail with -1743.
    Denied,
    /// Not yet decided — running the script will show the consent popup.
    NeedsConsent,
    /// Target app is not running — cannot determine permission.
    AppNotRunning,
    /// Check failed for an unknown reason.
    Unknown(i32),
}

// Apple Event constants
const TYPE_APPLICATION_BUNDLE_ID: u32 = u32::from_be_bytes(*b"bund");
const TYPE_WILDCARD: u32 = u32::from_be_bytes(*b"****");
const NO_ERR: i32 = 0;
const ERR_AE_EVENT_NOT_PERMITTED: i32 = -1743;
const ERR_AE_EVENT_WOULD_REQUIRE_USER_CONSENT: i32 = -1744;
const PROC_NOT_FOUND: i32 = -600;

#[repr(C)]
struct AEDesc {
    descriptor_type: u32,
    data_handle: *mut c_void,
}

#[link(name = "CoreServices", kind = "framework")]
unsafe extern "C" {
    fn AECreateDesc(
        type_code: u32,
        data_ptr: *const c_void,
        data_size: isize,
        result: *mut AEDesc,
    ) -> i32;

    fn AEDisposeDesc(the_aedesc: *mut AEDesc) -> i32;

    fn AEDeterminePermissionToAutomateTarget(
        target: *const AEDesc,
        the_ae_event_class: u32,
        the_ae_event_id: u32,
        ask_user_if_needed: bool,
    ) -> i32;
}

/// Silently check if Aura has Automation permission to control a target app.
///
/// `bundle_id` is the target app's bundle identifier, e.g. `"com.apple.finder"`.
/// This never triggers a popup — it's a read-only preflight.
pub fn check_automation_permission(bundle_id: &str) -> AutomationPermission {
    // SAFETY: AECreateDesc, AEDeterminePermissionToAutomateTarget, and AEDisposeDesc
    // are stable macOS C APIs. We pass a valid UTF-8 byte pointer with correct length
    // for the bundle ID, and a zeroed AEDesc struct that AECreateDesc populates.
    // AEDisposeDesc is always called to free the descriptor.
    unsafe {
        let mut desc = AEDesc {
            descriptor_type: 0,
            data_handle: std::ptr::null_mut(),
        };

        let status = AECreateDesc(
            TYPE_APPLICATION_BUNDLE_ID,
            bundle_id.as_ptr() as *const c_void,
            bundle_id.len() as isize,
            &mut desc,
        );

        if status != NO_ERR {
            return AutomationPermission::Unknown(status);
        }

        let result = AEDeterminePermissionToAutomateTarget(
            &desc,
            TYPE_WILDCARD,
            TYPE_WILDCARD,
            false, // never ask the user — silent check only
        );

        AEDisposeDesc(&mut desc);

        match result {
            NO_ERR => AutomationPermission::Granted,
            ERR_AE_EVENT_NOT_PERMITTED => AutomationPermission::Denied,
            ERR_AE_EVENT_WOULD_REQUIRE_USER_CONSENT => AutomationPermission::NeedsConsent,
            PROC_NOT_FOUND => AutomationPermission::AppNotRunning,
            other => AutomationPermission::Unknown(other),
        }
    }
}

/// Well-known bundle IDs for apps commonly automated via AppleScript.
pub fn app_name_to_bundle_id(app_name: &str) -> Option<&'static str> {
    match app_name.to_lowercase().as_str() {
        "system events" => Some("com.apple.systemevents"),
        "finder" => Some("com.apple.finder"),
        "safari" => Some("com.apple.Safari"),
        "mail" => Some("com.apple.mail"),
        "messages" => Some("com.apple.MobileSMS"),
        "notes" => Some("com.apple.Notes"),
        "reminders" => Some("com.apple.reminders"),
        "calendar" => Some("com.apple.iCal"),
        "contacts" => Some("com.apple.AddressBook"),
        "music" => Some("com.apple.Music"),
        "tv" => Some("com.apple.TV"),
        "photos" => Some("com.apple.Photos"),
        "preview" => Some("com.apple.Preview"),
        "terminal" => Some("com.apple.Terminal"),
        "textedit" => Some("com.apple.TextEdit"),
        "keynote" => Some("com.apple.iWork.Keynote"),
        "pages" => Some("com.apple.iWork.Pages"),
        "numbers" => Some("com.apple.iWork.Numbers"),
        "maps" => Some("com.apple.Maps"),
        "shortcuts" => Some("com.apple.shortcuts"),
        "automator" => Some("com.apple.Automator"),
        "script editor" => Some("com.apple.ScriptEditor2"),
        "spotify" => Some("com.spotify.client"),
        "slack" => Some("com.tinyspeck.slackmacgap"),
        "zoom" | "zoom.us" => Some("us.zoom.xos"),
        "discord" => Some("com.hnc.Discord"),
        "chrome" | "google chrome" => Some("com.google.Chrome"),
        "firefox" => Some("org.mozilla.firefox"),
        "arc" => Some("company.thebrowser.Browser"),
        "iterm" | "iterm2" => Some("com.googlecode.iterm2"),
        "visual studio code" | "vscode" | "code" => Some("com.microsoft.VSCode"),
        "xcode" => Some("com.apple.dt.Xcode"),
        _ => None,
    }
}

/// Extract the target app name from an AppleScript `tell application "..."` pattern.
/// Returns the first match found.
pub fn extract_target_app(script: &str) -> Option<String> {
    // Match: tell application "AppName"
    let lower = script.to_lowercase();
    let marker = "tell application \"";
    if let Some(start) = lower.find(marker) {
        let after = start + marker.len();
        let rest = &script[after..];
        if let Some(end) = rest.find('"') {
            return Some(rest[..end].to_string());
        }
    }
    // Match: Application("AppName") for JXA
    let jxa_marker = "application(\"";
    if let Some(start) = lower.find(jxa_marker) {
        let after = start + jxa_marker.len();
        let rest = &script[after..];
        if let Some(end) = rest.find('"') {
            return Some(rest[..end].to_string());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_applescript_target() {
        let script = r#"tell application "Finder" to get name of every disk"#;
        assert_eq!(extract_target_app(script), Some("Finder".to_string()));
    }

    #[test]
    fn extract_jxa_target() {
        let script = r#"Application("Safari").activate()"#;
        assert_eq!(extract_target_app(script), Some("Safari".to_string()));
    }

    #[test]
    fn extract_no_target() {
        let script = "display dialog \"Hello\"";
        assert_eq!(extract_target_app(script), None);
    }

    #[test]
    fn extract_system_events() {
        let script = r#"tell application "System Events" to get name of first process whose frontmost is true"#;
        assert_eq!(
            extract_target_app(script),
            Some("System Events".to_string())
        );
    }

    #[test]
    fn known_bundle_ids() {
        assert_eq!(app_name_to_bundle_id("Finder"), Some("com.apple.finder"));
        assert_eq!(
            app_name_to_bundle_id("System Events"),
            Some("com.apple.systemevents")
        );
        assert_eq!(
            app_name_to_bundle_id("Google Chrome"),
            Some("com.google.Chrome")
        );
        assert_eq!(app_name_to_bundle_id("UnknownApp123"), None);
    }

    #[test]
    fn bundle_id_lookup_case_insensitive() {
        assert_eq!(app_name_to_bundle_id("FINDER"), Some("com.apple.finder"));
        assert_eq!(app_name_to_bundle_id("safari"), Some("com.apple.Safari"));
    }
}
