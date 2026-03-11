//! Accessibility tree walker using macOS AXUIElement FFI.
//!
//! Enumerates interactive UI elements in the frontmost application.
//! Requires Accessibility permission (granted in System Settings → Privacy & Security → Accessibility).

use std::ffi::c_void;
use std::time::Instant;

use core_foundation::array::{CFArrayGetCount, CFArrayGetValueAtIndex};
use core_foundation::base::{CFGetTypeID, CFRelease, CFRetain, CFTypeRef};
use core_foundation::boolean::{CFBooleanGetTypeID, kCFBooleanTrue};
use core_foundation::string::{CFStringGetTypeID, CFStringRef};

use crate::context::{ElementBounds, UIElement};

// ── Constants ────────────────────────────────────────────────────────────────

pub const MAX_ELEMENTS: usize = 50;
pub const MAX_DEPTH: usize = 5;
pub const TIMEOUT_MS: u128 = 500;

const AX_ERROR_SUCCESS: i32 = 0;
const AX_VALUE_CG_POINT: u32 = 1;
const AX_VALUE_CG_SIZE: u32 = 2;

pub static INTERACTIVE_ROLES: &[&str] = &[
    "AXButton",
    "AXTextField",
    "AXTextArea",
    "AXCheckBox",
    "AXRadioButton",
    "AXPopUpButton",
    "AXMenuButton",
    "AXSlider",
    "AXLink",
    "AXTab",
    "AXMenuItem",
    "AXMenuBarItem",
    "AXComboBox",
    "AXIncrementor",
    "AXColorWell",
    "AXDisclosureTriangle",
];

// ── C structs for AXValueGetValue ────────────────────────────────────────────

#[repr(C)]
struct CGPoint {
    x: f64,
    y: f64,
}

#[repr(C)]
struct CGSize {
    width: f64,
    height: f64,
}

// ── FFI declarations ─────────────────────────────────────────────────────────

#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    fn AXUIElementCreateApplication(pid: i32) -> CFTypeRef;
    fn AXUIElementCopyAttributeValue(
        element: CFTypeRef,
        attribute: CFTypeRef,
        value: *mut CFTypeRef,
    ) -> i32;
    fn AXValueGetValue(value: CFTypeRef, the_type: u32, value_ptr: *mut c_void) -> bool;
}

// ── RAII wrapper for CFTypeRef ───────────────────────────────────────────────

struct CfRef(CFTypeRef);

impl CfRef {
    /// Takes ownership of a retained CFTypeRef (from a Create/Copy function).
    fn new(r: CFTypeRef) -> Self {
        Self(r)
    }

    fn as_raw(&self) -> CFTypeRef {
        self.0
    }
}

impl Drop for CfRef {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { CFRelease(self.0) };
        }
    }
}

// ── Helper: build a CFString attribute key ───────────────────────────────────

/// Returns a CfRef that owns a retained CFString built from `s`.
fn cf_string_from_str(s: &str) -> CfRef {
    use core_foundation::base::TCFType;
    use core_foundation::string::CFString;
    let cf = CFString::new(s);
    // cf owns +1 retain. We need to retain once more so that when CfRef drops,
    // the net count after both drops is zero.
    let raw = cf.as_CFTypeRef();
    unsafe { CFRetain(raw) };
    CfRef::new(raw)
    // `cf` drops here, releasing its own +1 retain; CfRef holds the extra +1.
}

// ── Attribute helpers ─────────────────────────────────────────────────────────

/// Fetch a string attribute (AXRole, AXTitle, AXDescription, AXValue).
/// Returns None if the attribute is absent or not a CFString.
fn get_ax_string(element: CFTypeRef, attr: &str) -> Option<String> {
    use core_foundation::base::TCFType;
    use core_foundation::string::CFString;

    let attr_key = cf_string_from_str(attr);
    let mut value: CFTypeRef = std::ptr::null();
    let ret = unsafe { AXUIElementCopyAttributeValue(element, attr_key.as_raw(), &mut value) };
    if ret != AX_ERROR_SUCCESS || value.is_null() {
        return None;
    }
    // CopyAttributeValue gives us +1 retain; CfRef will release on drop.
    let value_ref = CfRef::new(value);

    // Type-check: must be a CFString.
    if unsafe { CFGetTypeID(value_ref.as_raw()) } != unsafe { CFStringGetTypeID() } {
        return None;
    }

    // wrap_under_get_rule adds +1 retain; its Drop releases that extra retain.
    // CfRef still holds the original +1 and will release on drop.
    let cf_str = unsafe { CFString::wrap_under_get_rule(value_ref.as_raw() as CFStringRef) };
    Some(cf_str.to_string())
}

/// Fetch a boolean attribute (AXEnabled, AXFocused). Returns false if absent.
fn get_ax_bool(element: CFTypeRef, attr: &str) -> bool {
    let attr_key = cf_string_from_str(attr);
    let mut value: CFTypeRef = std::ptr::null();
    let ret = unsafe { AXUIElementCopyAttributeValue(element, attr_key.as_raw(), &mut value) };
    if ret != AX_ERROR_SUCCESS || value.is_null() {
        return false;
    }
    let value_ref = CfRef::new(value);

    // Type-check: must be a CFBoolean.
    if unsafe { CFGetTypeID(value_ref.as_raw()) } != unsafe { CFBooleanGetTypeID() } {
        return false;
    }

    // CFBooleans are singletons; compare pointer to kCFBooleanTrue.
    value_ref.as_raw() == unsafe { kCFBooleanTrue as CFTypeRef }
}

/// Fetch AXPosition as (x, y). Returns None if attribute absent.
fn get_ax_position(element: CFTypeRef) -> Option<(f64, f64)> {
    let attr_key = cf_string_from_str("AXPosition");
    let mut value: CFTypeRef = std::ptr::null();
    let ret = unsafe { AXUIElementCopyAttributeValue(element, attr_key.as_raw(), &mut value) };
    if ret != AX_ERROR_SUCCESS || value.is_null() {
        return None;
    }
    let value_ref = CfRef::new(value);

    let mut pt = CGPoint { x: 0.0, y: 0.0 };
    let ok = unsafe {
        AXValueGetValue(
            value_ref.as_raw(),
            AX_VALUE_CG_POINT,
            &mut pt as *mut CGPoint as *mut c_void,
        )
    };
    if ok { Some((pt.x, pt.y)) } else { None }
}

/// Fetch AXSize as (width, height). Returns None if attribute absent.
fn get_ax_size(element: CFTypeRef) -> Option<(f64, f64)> {
    let attr_key = cf_string_from_str("AXSize");
    let mut value: CFTypeRef = std::ptr::null();
    let ret = unsafe { AXUIElementCopyAttributeValue(element, attr_key.as_raw(), &mut value) };
    if ret != AX_ERROR_SUCCESS || value.is_null() {
        return None;
    }
    let value_ref = CfRef::new(value);

    let mut sz = CGSize {
        width: 0.0,
        height: 0.0,
    };
    let ok = unsafe {
        AXValueGetValue(
            value_ref.as_raw(),
            AX_VALUE_CG_SIZE,
            &mut sz as *mut CGSize as *mut c_void,
        )
    };
    if ok {
        Some((sz.width, sz.height))
    } else {
        None
    }
}

/// Fetch AXChildren as a Vec of individually retained CFTypeRef values.
/// The caller is responsible for calling CFRelease on each returned item after use.
fn get_ax_children(element: CFTypeRef) -> Vec<CFTypeRef> {
    let attr_key = cf_string_from_str("AXChildren");
    let mut value: CFTypeRef = std::ptr::null();
    let ret = unsafe { AXUIElementCopyAttributeValue(element, attr_key.as_raw(), &mut value) };
    if ret != AX_ERROR_SUCCESS || value.is_null() {
        return Vec::new();
    }
    // value is a retained CFArray — we own it and must release it.
    let array_ptr = value as core_foundation::array::CFArrayRef;
    let count = unsafe { CFArrayGetCount(array_ptr) };
    let mut children = Vec::with_capacity(count as usize);
    for i in 0..count {
        let child = unsafe { CFArrayGetValueAtIndex(array_ptr, i) as CFTypeRef };
        if !child.is_null() {
            // CFArrayGetValueAtIndex does NOT retain; we must retain each child we keep.
            unsafe { CFRetain(child) };
            children.push(child);
        }
    }
    // Release the array itself (we have individually retained all children).
    unsafe { CFRelease(value) };
    children
}

// ── Tree walker ───────────────────────────────────────────────────────────────

fn walk_element(
    element: CFTypeRef,
    depth: usize,
    elements: &mut Vec<UIElement>,
    start_time: &Instant,
) {
    if depth > MAX_DEPTH {
        return;
    }
    if elements.len() >= MAX_ELEMENTS {
        return;
    }
    if start_time.elapsed().as_millis() >= TIMEOUT_MS {
        return;
    }

    let role = match get_ax_string(element, "AXRole") {
        Some(r) => r,
        None => return,
    };

    if INTERACTIVE_ROLES.contains(&role.as_str()) {
        // Collect label: prefer AXTitle, fall back to AXDescription.
        let label = get_ax_string(element, "AXTitle")
            .filter(|s| !s.is_empty())
            .or_else(|| get_ax_string(element, "AXDescription").filter(|s| !s.is_empty()));

        let value = get_ax_string(element, "AXValue").filter(|s| !s.is_empty());
        let enabled = get_ax_bool(element, "AXEnabled");
        let focused = get_ax_bool(element, "AXFocused");

        let bounds = match (get_ax_position(element), get_ax_size(element)) {
            (Some((x, y)), Some((w, h))) => Some(ElementBounds {
                x,
                y,
                width: w,
                height: h,
            }),
            _ => None,
        };

        elements.push(UIElement {
            role,
            label,
            value,
            bounds,
            enabled,
            focused,
        });
    } else {
        // Not interactive — recurse into children.
        let children = get_ax_children(element);
        for child in children {
            walk_element(child, depth + 1, elements, start_time);
            // Release the retain we added in get_ax_children.
            unsafe { CFRelease(child) };
        }
    }
}

// ── Public API ────────────────────────────────────────────────────────────────

/// Walk the accessibility tree of the frontmost application and return
/// all interactive elements found within limits.
pub fn get_focused_app_elements() -> Vec<UIElement> {
    let pid = match crate::macos::get_frontmost_pid() {
        Some(p) => p,
        None => return Vec::new(),
    };

    let app_element = unsafe { AXUIElementCreateApplication(pid) };
    if app_element.is_null() {
        return Vec::new();
    }
    let app_ref = CfRef::new(app_element);

    let mut elements = Vec::new();
    let start_time = Instant::now();
    walk_element(app_ref.as_raw(), 0, &mut elements, &start_time);
    elements
}

/// Filter a slice of UIElements by optional label substring and optional role.
///
/// - `label`: case-insensitive substring matched against `UIElement.label` and `UIElement.value`
/// - `role`: accepts both full form ("AXButton") and short form ("button"), case-insensitive
pub fn find_elements(
    elements: &[UIElement],
    label: Option<&str>,
    role: Option<&str>,
) -> Vec<UIElement> {
    elements
        .iter()
        .filter(|el| {
            if let Some(r) = role
                && !matches_role(&el.role, r)
            {
                return false;
            }
            if let Some(lbl) = label {
                let lbl_lower = lbl.to_lowercase();
                let in_label = el
                    .label
                    .as_deref()
                    .map(|s| s.to_lowercase().contains(&lbl_lower))
                    .unwrap_or(false);
                let in_value = el
                    .value
                    .as_deref()
                    .map(|s| s.to_lowercase().contains(&lbl_lower))
                    .unwrap_or(false);
                if !in_label && !in_value {
                    return false;
                }
            }
            true
        })
        .cloned()
        .collect()
}

/// Returns true if `element_role` matches `filter`.
///
/// Accepts both full form ("AXButton") and short form ("button").
/// Comparison is case-insensitive. The filter may also carry the "AX" prefix.
pub fn matches_role(element_role: &str, filter: &str) -> bool {
    let role_lower = element_role.to_lowercase();
    let filter_lower = filter.to_lowercase();

    // Exact match (handles "AXButton" == "AXButton" and "axbutton" == "AXButton")
    if role_lower == filter_lower {
        return true;
    }

    // Short form: strip "ax" from both sides and compare
    let role_short = role_lower.strip_prefix("ax").unwrap_or(&role_lower);
    let filter_short = filter_lower.strip_prefix("ax").unwrap_or(&filter_lower);

    role_short == filter_short
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn make_element(role: &str, label: &str) -> UIElement {
        UIElement {
            role: role.to_string(),
            label: if label.is_empty() {
                None
            } else {
                Some(label.to_string())
            },
            value: None,
            bounds: Some(ElementBounds {
                x: 0.0,
                y: 0.0,
                width: 100.0,
                height: 30.0,
            }),
            enabled: true,
            focused: false,
        }
    }

    #[test]
    fn matches_role_exact() {
        assert!(matches_role("AXButton", "AXButton"));
    }

    #[test]
    fn matches_role_short_form() {
        assert!(matches_role("AXButton", "button"));
        assert!(matches_role("AXTextField", "textfield"));
    }

    #[test]
    fn matches_role_case_insensitive() {
        assert!(matches_role("AXButton", "Button"));
        assert!(matches_role("AXButton", "BUTTON"));
        assert!(matches_role("AXButton", "axbutton"));
    }

    #[test]
    fn matches_role_no_false_positives() {
        assert!(!matches_role("AXButton", "textfield"));
        assert!(!matches_role("AXPopUpButton", "button"));
    }

    #[test]
    fn find_elements_by_label() {
        let els = vec![
            make_element("AXButton", "Submit"),
            make_element("AXButton", "Cancel"),
            make_element("AXTextField", "Email"),
        ];
        let results = find_elements(&els, Some("submit"), None);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].label.as_deref(), Some("Submit"));
    }

    #[test]
    fn find_elements_by_role() {
        let els = vec![
            make_element("AXButton", "Submit"),
            make_element("AXButton", "Cancel"),
            make_element("AXTextField", "Email"),
        ];
        let results = find_elements(&els, None, Some("AXTextField"));
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].role, "AXTextField");
    }

    #[test]
    fn find_elements_by_label_and_role() {
        let els = vec![
            make_element("AXButton", "Submit"),
            make_element("AXTextField", "Submit"),
        ];
        let results = find_elements(&els, Some("submit"), Some("button"));
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].role, "AXButton");
    }

    #[test]
    fn find_elements_no_match() {
        let els = vec![make_element("AXButton", "Submit")];
        let results = find_elements(&els, Some("nonexistent"), None);
        assert!(results.is_empty());
    }

    #[test]
    fn find_elements_label_in_value() {
        let mut el = make_element("AXTextField", "");
        el.value = Some("hello@example.com".to_string());
        let els = vec![el];
        let results = find_elements(&els, Some("hello"), None);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].value.as_deref(), Some("hello@example.com"));
    }

    #[test]
    fn interactive_roles_contains_common_types() {
        assert!(INTERACTIVE_ROLES.contains(&"AXButton"));
        assert!(INTERACTIVE_ROLES.contains(&"AXTextField"));
        assert!(INTERACTIVE_ROLES.contains(&"AXCheckBox"));
        assert!(INTERACTIVE_ROLES.contains(&"AXLink"));
        // Non-interactive roles must NOT be present
        assert!(!INTERACTIVE_ROLES.contains(&"AXGroup"));
        assert!(!INTERACTIVE_ROLES.contains(&"AXStaticText"));
    }
}
