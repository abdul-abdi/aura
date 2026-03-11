# Input Control & AppleScript Integration — Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Fix coordinate accuracy, reduce tool-call chaining, and give the AI clear guidance on when to use AppleScript vs CGEvent by adding an accessibility tree walker, three new high-level tools, and rewriting the system prompt.

**Architecture:** Extend `aura-screen` with macOS Accessibility API (AXUIElement) FFI to walk the frontmost app's UI tree. Add `click_element`, `activate_app`, and `click_menu_item` tools to `aura-daemon`. Rewrite the system prompt in `aura-gemini` with a clear decision tree.

**Tech Stack:** Rust, macOS ApplicationServices framework (AXUIElement FFI), CoreFoundation, existing `aura-input` CGEvent layer, existing `aura-bridge` AppleScript executor.

**Design doc:** `docs/plans/2026-03-11-input-control-design.md`

---

## Task 1: UIElement Types and ScreenContext Extension

**Files:**
- Modify: `crates/aura-screen/src/context.rs`
- Test: inline `#[cfg(test)]` module in same file

### Step 1: Write failing tests

Add to the bottom of `crates/aura-screen/src/context.rs`, inside a new test module:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ui_element_display_with_all_fields() {
        let el = UIElement {
            role: "AXButton".into(),
            label: Some("Save".into()),
            value: None,
            bounds: Some(ElementBounds { x: 100.0, y: 200.0, width: 80.0, height: 30.0 }),
            enabled: true,
            focused: false,
        };
        let s = el.summary();
        assert!(s.contains("button"), "should contain role without AX prefix");
        assert!(s.contains("Save"), "should contain label");
        assert!(s.contains("100"), "should contain x bound");
        assert!(s.contains("enabled"), "should indicate enabled");
    }

    #[test]
    fn ui_element_display_without_label() {
        let el = UIElement {
            role: "AXButton".into(),
            label: None,
            value: None,
            bounds: Some(ElementBounds { x: 10.0, y: 20.0, width: 30.0, height: 30.0 }),
            enabled: true,
            focused: false,
        };
        let s = el.summary();
        assert!(!s.contains("\"\""), "should not show empty quotes for missing label");
    }

    #[test]
    fn ui_element_display_focused() {
        let el = UIElement {
            role: "AXTextField".into(),
            label: Some("Search".into()),
            value: Some("hello".into()),
            bounds: Some(ElementBounds { x: 0.0, y: 0.0, width: 100.0, height: 20.0 }),
            enabled: true,
            focused: true,
        };
        let s = el.summary();
        assert!(s.contains("focused"), "should indicate focused");
    }

    #[test]
    fn screen_context_summary_includes_ui_elements() {
        let ctx = ScreenContext::new_with_details(
            "Safari",
            Some("GitHub"),
            vec!["Safari - GitHub".into()],
            None,
        ).with_ui_elements(vec![
            UIElement {
                role: "AXButton".into(),
                label: Some("Back".into()),
                value: None,
                bounds: Some(ElementBounds { x: 48.0, y: 52.0, width: 30.0, height: 30.0 }),
                enabled: true,
                focused: false,
            },
        ]);
        let summary = ctx.summary();
        assert!(summary.contains("UI Elements"), "summary should include UI Elements section");
        assert!(summary.contains("Back"), "summary should include element label");
    }

    #[test]
    fn screen_context_summary_empty_elements() {
        let ctx = ScreenContext::new_with_details("Finder", None, vec![], None);
        let summary = ctx.summary();
        assert!(!summary.contains("UI Elements"), "should not show UI Elements when empty");
    }

    #[test]
    fn element_bounds_center() {
        let b = ElementBounds { x: 100.0, y: 200.0, width: 80.0, height: 30.0 };
        let (cx, cy) = b.center();
        assert!((cx - 140.0).abs() < 0.01);
        assert!((cy - 215.0).abs() < 0.01);
    }
}
```

### Step 2: Run tests to verify they fail

Run: `cargo test -p aura-screen -- context::tests --no-run 2>&1 | head -20`
Expected: Compilation errors — `UIElement`, `ElementBounds`, `with_ui_elements`, `summary` method on UIElement don't exist yet.

### Step 3: Implement UIElement, ElementBounds, and extend ScreenContext

Add these types and methods to `crates/aura-screen/src/context.rs`:

```rust
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ElementBounds {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl ElementBounds {
    /// Center point of the element in logical screen coordinates.
    pub fn center(&self) -> (f64, f64) {
        (self.x + self.width / 2.0, self.y + self.height / 2.0)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UIElement {
    pub role: String,
    pub label: Option<String>,
    pub value: Option<String>,
    pub bounds: Option<ElementBounds>,
    pub enabled: bool,
    pub focused: bool,
}

impl UIElement {
    /// Human-readable one-line summary for Gemini context injection.
    pub fn summary(&self) -> String {
        // Strip "AX" prefix for readability: "AXButton" -> "button"
        let role = self.role.strip_prefix("AX").unwrap_or(&self.role).to_lowercase();
        let mut parts = vec![role];
        if let Some(ref label) = self.label {
            parts.push(format!("\"{}\"", label));
        }
        if let Some(ref bounds) = self.bounds {
            parts.push(format!(
                "bounds={{x:{}, y:{}, w:{}, h:{}}}",
                bounds.x as i32, bounds.y as i32, bounds.width as i32, bounds.height as i32
            ));
        }
        if self.enabled {
            parts.push("enabled".into());
        }
        if self.focused {
            parts.push("focused".into());
        }
        parts.join(" ")
    }
}
```

Extend `ScreenContext` — add a `ui_elements` field:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScreenContext {
    frontmost_app: String,
    frontmost_title: Option<String>,
    open_windows: Vec<String>,
    clipboard: Option<String>,
    ui_elements: Vec<UIElement>,
}
```

Update `empty()`:
```rust
pub fn empty() -> Self {
    Self {
        frontmost_app: String::new(),
        frontmost_title: None,
        open_windows: Vec::new(),
        clipboard: None,
        ui_elements: Vec::new(),
    }
}
```

Update `new_with_details()`:
```rust
pub fn new_with_details(
    frontmost_app: &str,
    frontmost_title: Option<&str>,
    open_windows: Vec<String>,
    clipboard: Option<String>,
) -> Self {
    Self {
        frontmost_app: frontmost_app.to_string(),
        frontmost_title: frontmost_title.map(String::from),
        open_windows,
        clipboard,
        ui_elements: Vec::new(),
    }
}
```

Add builder method:
```rust
pub fn with_ui_elements(mut self, elements: Vec<UIElement>) -> Self {
    self.ui_elements = elements;
    self
}

pub fn ui_elements(&self) -> &[UIElement] {
    &self.ui_elements
}
```

Update `summary()` to include UI elements:
```rust
pub fn summary(&self) -> String {
    let mut parts = Vec::new();
    parts.push(format!("Frontmost app: {}", self.frontmost_app));
    if let Some(ref title) = self.frontmost_title {
        parts.push(format!("Window title: {title}"));
    }
    if !self.open_windows.is_empty() {
        parts.push(format!("Open windows: {}", self.open_windows.join(", ")));
    }
    if let Some(ref clip) = self.clipboard {
        let truncated: String = clip.chars().take(200).collect();
        if truncated.len() < clip.len() {
            parts.push(format!("Clipboard: {truncated}..."));
        } else {
            parts.push(format!("Clipboard: {truncated}"));
        }
    }
    if !self.ui_elements.is_empty() {
        parts.push("UI Elements (interactive):".into());
        for (i, el) in self.ui_elements.iter().enumerate() {
            parts.push(format!("  [{}] {}", i, el.summary()));
        }
    }
    parts.join("\n")
}
```

### Step 4: Run tests to verify they pass

Run: `cargo test -p aura-screen -- context::tests -v`
Expected: All 6 tests pass.

### Step 5: Commit

```bash
git add crates/aura-screen/src/context.rs
git commit -m "feat: add UIElement and ElementBounds types to screen context"
```

---

## Task 2: Accessibility Tree Walker Module

**Files:**
- Create: `crates/aura-screen/src/accessibility.rs`
- Modify: `crates/aura-screen/src/lib.rs` (add `pub mod accessibility;`)
- Modify: `crates/aura-screen/src/macos.rs` (make `run_jxa` pub, add `get_frontmost_pid`)

### Step 1: Write failing tests

Create `crates/aura-screen/src/accessibility.rs` with tests first:

```rust
//! macOS Accessibility API integration for walking the UI element tree.
//!
//! Uses AXUIElement FFI to enumerate interactive elements (buttons, text fields,
//! checkboxes, etc.) in the frontmost application. Returns elements with their
//! role, label, bounds, and state for use by Gemini's tool-calling layer.

use std::ffi::c_void;
use std::time::Instant;

use core_foundation::base::{CFRelease, CFTypeRef, TCFType};
use core_foundation::string::CFString;

use crate::context::{ElementBounds, UIElement};

/// Maximum number of UI elements to return.
const MAX_ELEMENTS: usize = 50;

/// Maximum recursion depth when walking the AX tree.
const MAX_DEPTH: usize = 5;

/// Timeout for the entire tree walk in milliseconds.
const TIMEOUT_MS: u128 = 500;

/// AX API success code.
const AX_ERROR_SUCCESS: i32 = 0;

/// AXValue type constants for extracting CGPoint/CGSize.
const AX_VALUE_CG_POINT: u32 = 1;
const AX_VALUE_CG_SIZE: u32 = 2;

/// Roles considered interactive (worth reporting to Gemini).
const INTERACTIVE_ROLES: &[&str] = &[
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

#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct CGPoint {
    x: f64,
    y: f64,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct CGSize {
    width: f64,
    height: f64,
}

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

/// RAII wrapper for CFTypeRef values returned by AX "Copy"/"Create" functions.
/// Calls CFRelease on drop to prevent leaks.
struct CfRef(CFTypeRef);

impl Drop for CfRef {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: self.0 was returned by an AX Copy/Create function and is
            // a valid retained CFTypeRef. CFRelease is safe to call once per retain.
            unsafe { CFRelease(self.0) };
        }
    }
}

impl CfRef {
    fn as_ptr(&self) -> CFTypeRef {
        self.0
    }
}

/// Get a string attribute from an AX element (e.g. AXRole, AXTitle, AXDescription).
fn get_ax_string(element: CFTypeRef, attr: &str) -> Option<String> {
    let attr_cf = CFString::new(attr);
    let mut value: CFTypeRef = std::ptr::null();
    // SAFETY: element is a valid AXUIElementRef, attr_cf is a valid CFStringRef,
    // value is a valid out-pointer. The function writes a retained CFTypeRef.
    let status =
        unsafe { AXUIElementCopyAttributeValue(element, attr_cf.as_CFTypeRef(), &mut value) };
    if status != AX_ERROR_SUCCESS || value.is_null() {
        return None;
    }
    let _guard = CfRef(value);
    // SAFETY: if the attribute is a string, value is a CFStringRef. We check by
    // attempting the conversion — if it's not a string, CFString::wrap_under_get_rule
    // would be wrong, but we first verify via CFGetTypeID.
    unsafe {
        let type_id = core_foundation::base::CFGetTypeID(value);
        if type_id != core_foundation::string::CFString::type_id() {
            return None;
        }
        // wrap_under_get_rule doesn't consume the reference (we still release via _guard)
        let cf_str = CFString::wrap_under_get_rule(value as *const _);
        Some(cf_str.to_string())
    }
}

/// Get a boolean attribute from an AX element (e.g. AXEnabled, AXFocused).
fn get_ax_bool(element: CFTypeRef, attr: &str) -> bool {
    let attr_cf = CFString::new(attr);
    let mut value: CFTypeRef = std::ptr::null();
    // SAFETY: Same as get_ax_string — valid element, attribute, and out-pointer.
    let status =
        unsafe { AXUIElementCopyAttributeValue(element, attr_cf.as_CFTypeRef(), &mut value) };
    if status != AX_ERROR_SUCCESS || value.is_null() {
        return false;
    }
    let _guard = CfRef(value);
    // SAFETY: We check that the value is a CFBoolean before interpreting it.
    unsafe {
        let type_id = core_foundation::base::CFGetTypeID(value);
        if type_id != core_foundation::boolean::CFBoolean::type_id() {
            return false;
        }
        let cf_bool = core_foundation::boolean::CFBoolean::wrap_under_get_rule(value as *const _);
        cf_bool == core_foundation::boolean::CFBoolean::true_value()
    }
}

/// Get the AXPosition attribute (CGPoint) from an AX element.
fn get_ax_position(element: CFTypeRef) -> Option<(f64, f64)> {
    let attr_cf = CFString::new("AXPosition");
    let mut value: CFTypeRef = std::ptr::null();
    // SAFETY: Valid element, attribute, and out-pointer.
    let status =
        unsafe { AXUIElementCopyAttributeValue(element, attr_cf.as_CFTypeRef(), &mut value) };
    if status != AX_ERROR_SUCCESS || value.is_null() {
        return None;
    }
    let _guard = CfRef(value);
    let mut point = CGPoint { x: 0.0, y: 0.0 };
    // SAFETY: value is an AXValueRef containing a CGPoint. AXValueGetValue writes
    // the point data into our stack-allocated struct.
    let ok =
        unsafe { AXValueGetValue(value, AX_VALUE_CG_POINT, &mut point as *mut _ as *mut c_void) };
    if ok {
        Some((point.x, point.y))
    } else {
        None
    }
}

/// Get the AXSize attribute (CGSize) from an AX element.
fn get_ax_size(element: CFTypeRef) -> Option<(f64, f64)> {
    let attr_cf = CFString::new("AXSize");
    let mut value: CFTypeRef = std::ptr::null();
    // SAFETY: Valid element, attribute, and out-pointer.
    let status =
        unsafe { AXUIElementCopyAttributeValue(element, attr_cf.as_CFTypeRef(), &mut value) };
    if status != AX_ERROR_SUCCESS || value.is_null() {
        return None;
    }
    let _guard = CfRef(value);
    let mut size = CGSize {
        width: 0.0,
        height: 0.0,
    };
    // SAFETY: value is an AXValueRef containing a CGSize.
    let ok =
        unsafe { AXValueGetValue(value, AX_VALUE_CG_SIZE, &mut size as *mut _ as *mut c_void) };
    if ok {
        Some((size.width, size.height))
    } else {
        None
    }
}

/// Get the AXChildren array from an AX element. Returns retained CFTypeRefs
/// that the caller must release.
fn get_ax_children(element: CFTypeRef) -> Vec<CFTypeRef> {
    let attr_cf = CFString::new("AXChildren");
    let mut value: CFTypeRef = std::ptr::null();
    // SAFETY: Valid element, attribute, and out-pointer.
    let status =
        unsafe { AXUIElementCopyAttributeValue(element, attr_cf.as_CFTypeRef(), &mut value) };
    if status != AX_ERROR_SUCCESS || value.is_null() {
        return Vec::new();
    }
    // value is a CFArrayRef — we need to read its contents before releasing
    // SAFETY: We verified status == SUCCESS and value is non-null.
    // CFArrayGetCount and CFArrayGetValueAtIndex are safe for valid CFArrayRefs.
    unsafe {
        let type_id = core_foundation::base::CFGetTypeID(value);
        if type_id != core_foundation::array::CFArray::type_id() {
            CFRelease(value);
            return Vec::new();
        }

        let count = core_foundation::array::CFArrayGetCount(value as *const _);
        let mut children = Vec::with_capacity(count as usize);
        for i in 0..count {
            let child = core_foundation::array::CFArrayGetValueAtIndex(value as *const _, i);
            if !child.is_null() {
                // Retain the child because CFArrayGetValueAtIndex returns unretained
                core_foundation::base::CFRetain(child);
                children.push(child);
            }
        }
        CFRelease(value); // Release the array itself
        children
    }
}

/// Recursively walk an AX element tree, collecting interactive elements.
fn walk_element(
    element: CFTypeRef,
    depth: usize,
    elements: &mut Vec<UIElement>,
    start: Instant,
) {
    // Bail conditions
    if depth > MAX_DEPTH || elements.len() >= MAX_ELEMENTS || start.elapsed().as_millis() > TIMEOUT_MS
    {
        return;
    }

    // Get role
    let role = match get_ax_string(element, "AXRole") {
        Some(r) => r,
        None => return,
    };

    // If this element has an interactive role, collect it
    if INTERACTIVE_ROLES.contains(&role.as_str()) {
        let label = get_ax_string(element, "AXTitle")
            .or_else(|| get_ax_string(element, "AXDescription"));
        let value = get_ax_string(element, "AXValue");
        let enabled = get_ax_bool(element, "AXEnabled");
        let focused = get_ax_bool(element, "AXFocused");

        let bounds = match (get_ax_position(element), get_ax_size(element)) {
            (Some((x, y)), Some((w, h))) if w > 0.0 && h > 0.0 => {
                Some(ElementBounds {
                    x,
                    y,
                    width: w,
                    height: h,
                })
            }
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
        // Don't recurse into interactive elements' children (they're leaf-like for our purposes)
        return;
    }

    // Recurse into children for container roles
    let children = get_ax_children(element);
    for child in &children {
        if elements.len() >= MAX_ELEMENTS || start.elapsed().as_millis() > TIMEOUT_MS {
            break;
        }
        walk_element(*child, depth + 1, elements, start);
    }
    // Release all children
    for child in children {
        // SAFETY: child was retained in get_ax_children
        unsafe { CFRelease(child) };
    }
}

/// Walk the accessibility tree of the frontmost application and return interactive UI elements.
///
/// Returns an empty Vec if:
/// - Accessibility permission is not granted
/// - The frontmost app PID cannot be determined
/// - The tree walk times out (500ms max)
///
/// Elements are in document order (top-to-bottom, left-to-right), capped at 50.
pub fn get_focused_app_elements() -> Vec<UIElement> {
    let pid = match crate::macos::get_frontmost_pid() {
        Some(pid) => pid,
        None => return Vec::new(),
    };

    // SAFETY: AXUIElementCreateApplication is a stable macOS API that takes a PID
    // and returns a retained AXUIElementRef (CFTypeRef).
    let app_element = unsafe { AXUIElementCreateApplication(pid) };
    if app_element.is_null() {
        return Vec::new();
    }
    let _guard = CfRef(app_element);

    let mut elements = Vec::new();
    let start = Instant::now();
    walk_element(app_element, 0, &mut elements, start);
    elements
}

/// Find UI elements matching optional label and role filters.
///
/// - `label`: case-insensitive substring match against AXTitle/AXDescription/AXValue
/// - `role`: matches AX role name. Accepts short forms: "button" matches "AXButton"
pub fn find_elements(
    elements: &[UIElement],
    label: Option<&str>,
    role: Option<&str>,
) -> Vec<UIElement> {
    elements
        .iter()
        .filter(|el| {
            let label_match = label.map_or(true, |l| {
                let l_lower = l.to_lowercase();
                el.label
                    .as_ref()
                    .map_or(false, |el_label| el_label.to_lowercase().contains(&l_lower))
                    || el.value
                        .as_ref()
                        .map_or(false, |el_val| el_val.to_lowercase().contains(&l_lower))
            });
            let role_match = role.map_or(true, |r| matches_role(&el.role, r));
            label_match && role_match
        })
        .cloned()
        .collect()
}

/// Check if an AX role matches a user-provided filter.
/// Accepts both full AX names ("AXButton") and short forms ("button").
fn matches_role(element_role: &str, filter: &str) -> bool {
    let role_lower = element_role.to_lowercase();
    let filter_lower = filter.to_lowercase();

    // Exact match (e.g. "AXButton" == "AXButton")
    if role_lower == filter_lower {
        return true;
    }
    // Short form match (e.g. "button" matches "AXButton")
    let role_stripped = role_lower.strip_prefix("ax").unwrap_or(&role_lower);
    role_stripped == filter_lower
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::ElementBounds;

    fn make_element(role: &str, label: Option<&str>) -> UIElement {
        UIElement {
            role: role.into(),
            label: label.map(String::from),
            value: None,
            bounds: Some(ElementBounds {
                x: 0.0,
                y: 0.0,
                width: 50.0,
                height: 30.0,
            }),
            enabled: true,
            focused: false,
        }
    }

    #[test]
    fn matches_role_exact() {
        assert!(matches_role("AXButton", "AXButton"));
        assert!(matches_role("AXTextField", "AXTextField"));
    }

    #[test]
    fn matches_role_short_form() {
        assert!(matches_role("AXButton", "button"));
        assert!(matches_role("AXTextField", "textfield"));
        assert!(matches_role("AXCheckBox", "checkbox"));
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
        let elements = vec![
            make_element("AXButton", Some("Save")),
            make_element("AXButton", Some("Cancel")),
            make_element("AXTextField", Some("Name")),
        ];
        let found = find_elements(&elements, Some("save"), None);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].label.as_deref(), Some("Save"));
    }

    #[test]
    fn find_elements_by_role() {
        let elements = vec![
            make_element("AXButton", Some("Save")),
            make_element("AXButton", Some("Cancel")),
            make_element("AXTextField", Some("Name")),
        ];
        let found = find_elements(&elements, None, Some("button"));
        assert_eq!(found.len(), 2);
    }

    #[test]
    fn find_elements_by_label_and_role() {
        let elements = vec![
            make_element("AXButton", Some("Save")),
            make_element("AXButton", Some("Cancel")),
            make_element("AXTextField", Some("Save")),
        ];
        let found = find_elements(&elements, Some("Save"), Some("button"));
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].role, "AXButton");
    }

    #[test]
    fn find_elements_no_match() {
        let elements = vec![make_element("AXButton", Some("OK"))];
        let found = find_elements(&elements, Some("nonexistent"), None);
        assert!(found.is_empty());
    }

    #[test]
    fn find_elements_label_in_value() {
        let mut el = make_element("AXTextField", Some("Search"));
        el.value = Some("hello world".into());
        let elements = vec![el];
        let found = find_elements(&elements, Some("hello"), None);
        assert_eq!(found.len(), 1);
    }

    #[test]
    fn interactive_roles_contains_common_types() {
        assert!(INTERACTIVE_ROLES.contains(&"AXButton"));
        assert!(INTERACTIVE_ROLES.contains(&"AXTextField"));
        assert!(INTERACTIVE_ROLES.contains(&"AXCheckBox"));
        assert!(INTERACTIVE_ROLES.contains(&"AXLink"));
        assert!(!INTERACTIVE_ROLES.contains(&"AXGroup"));
        assert!(!INTERACTIVE_ROLES.contains(&"AXStaticText"));
    }
}
```

### Step 2: Export the module and add get_frontmost_pid

Modify `crates/aura-screen/src/lib.rs` — add:
```rust
#[cfg(target_os = "macos")]
pub mod accessibility;
```

Modify `crates/aura-screen/src/macos.rs` — make `run_jxa` public (change `fn run_jxa` to `pub fn run_jxa`) and add:
```rust
/// Get the process ID of the frontmost application.
pub fn get_frontmost_pid() -> Option<i32> {
    run_jxa(
        "ObjC.import('AppKit'); $.NSWorkspace.sharedWorkspace.frontmostApplication.processIdentifier",
    )?
    .trim()
    .parse()
    .ok()
}
```

### Step 3: Run tests to verify unit tests pass (pure logic tests should pass, FFI tests are integration-only)

Run: `cargo test -p aura-screen -- accessibility::tests -v`
Expected: All 8 `find_elements` and `matches_role` tests pass. The FFI functions (`get_focused_app_elements`) are not tested in unit tests — they require a running app and permissions.

### Step 4: Commit

```bash
git add crates/aura-screen/src/accessibility.rs crates/aura-screen/src/lib.rs crates/aura-screen/src/macos.rs
git commit -m "feat: add accessibility tree walker with element filtering"
```

---

## Task 3: Integrate AX Tree into get_screen_context

**Files:**
- Modify: `crates/aura-screen/src/macos.rs:15-27`

### Step 1: Write failing test

Add to `crates/aura-screen/src/macos.rs` (or create a test file):

Since `capture_context` does live system calls, the test validates the return type includes ui_elements field. Add at bottom of `macos.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capture_context_returns_screen_context_with_elements_field() {
        // This just validates the type signature compiles — the actual AX walk
        // may return empty elements in CI/test environments without permissions.
        let reader = MacOSScreenReader::new().unwrap();
        let ctx = reader.capture_context().unwrap();
        // ui_elements() should exist and be callable
        let _elements = ctx.ui_elements();
    }
}
```

### Step 2: Implement — update capture_context to include AX tree

Modify `capture_context()` in `crates/aura-screen/src/macos.rs`:

```rust
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
```

### Step 3: Run test

Run: `cargo test -p aura-screen -- macos::tests -v`
Expected: PASS (compiles and runs; elements may be empty in test environment).

### Step 4: Run full crate tests

Run: `cargo test -p aura-screen -v`
Expected: All tests pass.

### Step 5: Commit

```bash
git add crates/aura-screen/src/macos.rs
git commit -m "feat: integrate accessibility tree into get_screen_context"
```

---

## Task 4: System Prompt Rewrite

**Files:**
- Modify: `crates/aura-gemini/src/config.rs:5-44`

### Step 1: Write failing test

Add to the test module in `crates/aura-gemini/src/config.rs`:

```rust
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
        !config.system_prompt.contains("Prefer direct UI interaction"),
        "old contradictory guidance should be removed"
    );
}
```

### Step 2: Run test to verify it fails

Run: `cargo test -p aura-gemini -- tests::system_prompt_has_decision_tree -v`
Expected: FAIL — current prompt doesn't contain "Choosing the Right Tool" or new tool names.

### Step 3: Replace the DEFAULT_SYSTEM_PROMPT constant

Replace the entire `DEFAULT_SYSTEM_PROMPT` in `crates/aura-gemini/src/config.rs`:

```rust
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
```

### Step 4: Run test to verify it passes

Run: `cargo test -p aura-gemini -- tests::system_prompt_has_decision_tree -v`
Expected: PASS

### Step 5: Also verify existing tests still pass

Run: `cargo test -p aura-gemini -v`
Expected: All tests pass. The `test_system_prompt_has_aura_personality` test checks for "Aura", "run_applescript", "get_screen_context", "move_mouse", "click", "type_text", "press_key" — all still present.

### Step 6: Commit

```bash
git add crates/aura-gemini/src/config.rs
git commit -m "feat: rewrite system prompt with tool decision tree"
```

---

## Task 5: New Tool Declarations

**Files:**
- Modify: `crates/aura-gemini/src/tools.rs`

### Step 1: Write failing tests

Add to the test module in `crates/aura-gemini/src/tools.rs`:

```rust
#[test]
fn tool_declarations_has_thirteen_functions() {
    let tools = build_tool_declarations();
    let decls = tools[0].function_declarations.as_ref().unwrap();
    assert_eq!(decls.len(), 13, "Should have 13 function declarations (10 + 3 new)");
}

#[test]
fn new_tool_names_present() {
    let tools = build_tool_declarations();
    let decls = tools[0].function_declarations.as_ref().unwrap();
    let names: Vec<&str> = decls.iter().map(|fd| fd.name.as_str()).collect();
    assert!(names.contains(&"click_element"), "should have click_element");
    assert!(names.contains(&"activate_app"), "should have activate_app");
    assert!(names.contains(&"click_menu_item"), "should have click_menu_item");
}

#[test]
fn click_element_has_label_and_role_params() {
    let tools = build_tool_declarations();
    let decls = tools[0].function_declarations.as_ref().unwrap();
    let ce = decls.iter().find(|d| d.name == "click_element").unwrap();
    assert!(ce.parameters["properties"]["label"].is_object());
    assert!(ce.parameters["properties"]["role"].is_object());
    assert!(ce.parameters["properties"]["index"].is_object());
    // No required params — both label and role are optional
    let required = ce.parameters.get("required");
    assert!(
        required.is_none() || required.unwrap().as_array().unwrap().is_empty(),
        "click_element should have no required params"
    );
}

#[test]
fn activate_app_has_required_name() {
    let tools = build_tool_declarations();
    let decls = tools[0].function_declarations.as_ref().unwrap();
    let aa = decls.iter().find(|d| d.name == "activate_app").unwrap();
    let required = aa.parameters["required"].as_array().unwrap();
    assert!(required.iter().any(|v| v == "name"));
}

#[test]
fn click_menu_item_has_required_menu_path() {
    let tools = build_tool_declarations();
    let decls = tools[0].function_declarations.as_ref().unwrap();
    let cmi = decls.iter().find(|d| d.name == "click_menu_item").unwrap();
    let required = cmi.parameters["required"].as_array().unwrap();
    assert!(required.iter().any(|v| v == "menu_path"));
}
```

### Step 2: Run tests to verify they fail

Run: `cargo test -p aura-gemini -- tests::tool_declarations_has_thirteen -v`
Expected: FAIL — only 10 declarations exist.

### Step 3: Add three new tool declarations

In `crates/aura-gemini/src/tools.rs`, add these three `FunctionDeclaration` entries to the `function_declarations` vec (after `recall_memory`, before the closing `]`):

```rust
FunctionDeclaration {
    name: "click_element".into(),
    description:
        "Click a UI element by its accessibility label and/or role. More reliable than \
        clicking by coordinates — finds the element in the app's accessibility tree and \
        clicks its exact center. Use for buttons, text fields, checkboxes, links, tabs, \
        and other labeled UI elements. \
        Invoke this tool only after you have identified the element you want to click \
        from screen context or the user's instruction."
            .into(),
    parameters: json!({
        "type": "object",
        "properties": {
            "label": {
                "type": "string",
                "description": "Text label to match (case-insensitive substring). Matches against the element's title, description, or value."
            },
            "role": {
                "type": "string",
                "description": "Element type to match: button, textfield, checkbox, link, tab, menuitem, popupbutton, slider, combobox"
            },
            "index": {
                "type": "integer",
                "description": "If multiple elements match, click the Nth one (0-indexed). Default: 0"
            }
        }
    }),
    behavior: Some("NON_BLOCKING".into()),
},
FunctionDeclaration {
    name: "activate_app".into(),
    description:
        "Launch an application or bring it to the front. More reliable than clicking \
        the Dock or using Spotlight. \
        Invoke this tool only after the user asks to open, switch to, or launch an app."
            .into(),
    parameters: json!({
        "type": "object",
        "properties": {
            "name": {
                "type": "string",
                "description": "Application name, e.g. 'Safari', 'Terminal', 'Slack', 'Visual Studio Code'"
            }
        },
        "required": ["name"]
    }),
    behavior: Some("NON_BLOCKING".into()),
},
FunctionDeclaration {
    name: "click_menu_item".into(),
    description:
        "Click a menu bar item by path. More reliable than clicking menus by coordinates \
        (menus dismiss on mis-click). Supports nested submenus up to 3 levels. \
        Invoke this tool only after you know the exact menu path needed."
            .into(),
    parameters: json!({
        "type": "object",
        "properties": {
            "menu_path": {
                "type": "array",
                "items": { "type": "string" },
                "description": "Menu item path from menu bar, e.g. [\"File\", \"Save As...\"] or [\"View\", \"Developer\", \"JavaScript Console\"]"
            },
            "app": {
                "type": "string",
                "description": "Target app name. Defaults to the frontmost app if omitted."
            }
        },
        "required": ["menu_path"]
    }),
    behavior: Some("NON_BLOCKING".into()),
},
```

Also update the existing `run_applescript` description to remove the contradictory "chain multiple calls" guidance:

Replace:
```
"Prefer simple scripts — chain multiple calls over one complex script. \
Invoke this tool only after you have confirmed the user's intent and \
understand what action to take."
```

With:
```
"For complex workflows, a single well-written script is better than multiple calls. \
Invoke this tool only after you have confirmed the user's intent and \
understand what action to take."
```

### Step 4: Update existing tests that check counts/names

Update `tool_declarations_returns_two_tool_objects` test:
```rust
assert_eq!(decls.len(), 13, "Should have 13 function declarations");
```

Update `tool_names_are_correct` test to include new names:
```rust
let names: Vec<&str> = decls.iter().map(|fd| fd.name.as_str()).collect();
assert_eq!(
    names,
    vec![
        "run_applescript",
        "get_screen_context",
        "shutdown_aura",
        "move_mouse",
        "click",
        "type_text",
        "press_key",
        "scroll",
        "drag",
        "recall_memory",
        "click_element",
        "activate_app",
        "click_menu_item",
    ]
);
```

Update `tool_declarations_serialize_to_valid_json`:
```rust
assert_eq!(decls.len(), 13);
```

### Step 5: Run all tests

Run: `cargo test -p aura-gemini -v`
Expected: All tests pass, including new ones.

### Step 6: Commit

```bash
git add crates/aura-gemini/src/tools.rs
git commit -m "feat: add click_element, activate_app, click_menu_item tool declarations"
```

---

## Task 6: Tool Handlers in Daemon — activate_app and click_menu_item

**Files:**
- Modify: `crates/aura-daemon/src/main.rs`

### Step 1: Implement activate_app handler

In `execute_tool()` at `crates/aura-daemon/src/main.rs`, add before the `other =>` fallback match arm (line ~1701):

```rust
"activate_app" => {
    let name = args
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    if name.is_empty() {
        return serde_json::json!({
            "success": false,
            "error": "name parameter is required"
        });
    }
    // Sanitize app name to prevent AppleScript injection
    let safe_name = name.replace('\\', "").replace('"', "");
    let script = format!(r#"tell application "{safe_name}" to activate"#);

    // Pre-check automation permission if we know the bundle ID
    if let Some(bundle_id) = aura_bridge::automation::app_name_to_bundle_id(&safe_name) {
        let perm = aura_bridge::automation::check_automation_permission(bundle_id);
        if perm == aura_bridge::automation::AutomationPermission::Denied {
            return serde_json::json!({
                "success": false,
                "error": format!(
                    "Automation permission for {safe_name} is denied. \
                     Grant in System Settings > Privacy & Security > Automation."
                ),
                "error_kind": "automation_denied",
            });
        }
    }

    let result = executor.run(&script, ScriptLanguage::AppleScript, 10).await;
    serde_json::json!({
        "success": result.success,
        "app": safe_name,
        "stderr": result.stderr,
    })
}
```

### Step 2: Implement click_menu_item handler

Add after `activate_app` handler:

```rust
"click_menu_item" => {
    let menu_path: Vec<String> = args
        .get("menu_path")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();

    if menu_path.len() < 2 {
        return serde_json::json!({
            "success": false,
            "error": "menu_path requires at least 2 items: [\"MenuBarItem\", \"MenuItem\", ...\"SubmenuItem\"]"
        });
    }

    // Determine target app
    let target_app = if let Some(app) = args.get("app").and_then(|v| v.as_str()) {
        app.to_string()
    } else {
        match screen_reader.capture_context() {
            Ok(ctx) => ctx.frontmost_app().to_string(),
            Err(_) => {
                return serde_json::json!({
                    "success": false,
                    "error": "Could not determine frontmost app. Specify 'app' parameter."
                })
            }
        }
    };

    // Pre-check automation permission for System Events
    if let Some(bundle_id) = aura_bridge::automation::app_name_to_bundle_id("System Events") {
        let perm = aura_bridge::automation::check_automation_permission(bundle_id);
        if perm == aura_bridge::automation::AutomationPermission::Denied {
            return serde_json::json!({
                "success": false,
                "error": "Automation permission for System Events is denied. \
                         Grant in System Settings > Privacy & Security > Automation.",
                "error_kind": "automation_denied",
            });
        }
    }

    let script = build_menu_click_script(&target_app, &menu_path);
    let result = executor.run(&script, ScriptLanguage::AppleScript, 10).await;
    if result.success {
        serde_json::json!({
            "success": true,
            "clicked": menu_path.join(" > "),
        })
    } else {
        serde_json::json!({
            "success": false,
            "error": format!("Menu item not found or click failed: {}", result.stderr),
            "stderr": result.stderr,
        })
    }
}
```

### Step 3: Add the build_menu_click_script helper function

Add as a standalone function near `execute_tool` in `main.rs`:

```rust
/// Build an AppleScript to click a menu item via System Events.
/// Supports 2-level (menu bar > item) and 3+ level (menu bar > submenu > item) paths.
fn build_menu_click_script(app: &str, path: &[String]) -> String {
    let process = app.replace('\\', "").replace('"', "");
    let escaped: Vec<String> = path
        .iter()
        .map(|s| s.replace('\\', "").replace('"', ""))
        .collect();

    match escaped.len() {
        2 => format!(
            "tell application \"System Events\" to tell process \"{process}\"\n\
             \tclick menu item \"{}\" of menu 1 of menu bar item \"{}\" of menu bar 1\n\
             end tell",
            escaped[1], escaped[0]
        ),
        3 => format!(
            "tell application \"System Events\" to tell process \"{process}\"\n\
             \tclick menu item \"{}\" of menu 1 of menu item \"{}\" of menu 1 of menu bar item \"{}\" of menu bar 1\n\
             end tell",
            escaped[2], escaped[1], escaped[0]
        ),
        _ => {
            // Build nested chain for 4+ levels
            let leaf = escaped.last().unwrap();
            let mut chain = format!("menu item \"{}\"", leaf);
            for item in escaped[1..escaped.len() - 1].iter().rev() {
                chain = format!("{chain} of menu 1 of menu item \"{item}\"");
            }
            chain = format!("{chain} of menu 1 of menu bar item \"{}\"", escaped[0]);
            format!(
                "tell application \"System Events\" to tell process \"{process}\"\n\
                 \tclick {chain} of menu bar 1\n\
                 end tell"
            )
        }
    }
}
```

### Step 4: Run compilation check

Run: `cargo check -p aura-daemon`
Expected: Compiles without errors.

### Step 5: Commit

```bash
git add crates/aura-daemon/src/main.rs
git commit -m "feat: add activate_app and click_menu_item tool handlers"
```

---

## Task 7: Tool Handler — click_element

**Files:**
- Modify: `crates/aura-daemon/src/main.rs`
- Dependency: `crates/aura-screen/src/accessibility.rs` (from Task 2)

### Step 1: Add click_element handler

In `execute_tool()`, add after the accessibility permission guard block (after line ~1603) and before `"move_mouse"`:

```rust
"click_element" => {
    if !aura_input::accessibility::check_accessibility(false) {
        return serde_json::json!({
            "success": false,
            "error": "Accessibility permission is not granted. \
                      Required for click_element to read UI elements and click. \
                      Enable in System Settings > Privacy & Security > Accessibility.",
            "error_kind": "accessibility_denied",
        });
    }

    let label = args.get("label").and_then(|v| v.as_str()).map(String::from);
    let role = args.get("role").and_then(|v| v.as_str()).map(String::from);
    let index = args.get("index").and_then(|v| v.as_u64()).unwrap_or(0) as usize;

    // Run AX tree walk + click on blocking thread (FFI calls are synchronous)
    match tokio::task::spawn_blocking(move || {
        click_element_inner(label.as_deref(), role.as_deref(), index)
    })
    .await
    {
        Ok(result) => result,
        Err(e) => serde_json::json!({
            "success": false,
            "error": format!("Task panicked: {e}"),
        }),
    }
}
```

### Step 2: Add the click_element_inner function

Add as a standalone function:

```rust
/// Find a UI element by label/role in the frontmost app's accessibility tree and click it.
fn click_element_inner(
    label: Option<&str>,
    role: Option<&str>,
    index: usize,
) -> serde_json::Value {
    if label.is_none() && role.is_none() {
        return serde_json::json!({
            "success": false,
            "error": "At least one of 'label' or 'role' must be provided",
        });
    }

    let all_elements = aura_screen::accessibility::get_focused_app_elements();
    if all_elements.is_empty() {
        return serde_json::json!({
            "success": false,
            "error": "No interactive UI elements found. The app may not expose accessibility data, \
                      or Accessibility permission may not be fully granted.",
        });
    }

    let matches = aura_screen::accessibility::find_elements(&all_elements, label, role);

    if matches.is_empty() {
        // Include alternatives to help Gemini self-correct
        let alternatives: Vec<String> = all_elements
            .iter()
            .filter_map(|el| {
                let label_str = el.label.as_deref().unwrap_or("(unlabeled)");
                let role_short = el.role.strip_prefix("AX").unwrap_or(&el.role).to_lowercase();
                Some(format!("{role_short} \"{label_str}\""))
            })
            .take(15)
            .collect();

        return serde_json::json!({
            "success": false,
            "error": format!(
                "No element matching label={:?} role={:?}. Available elements: {}",
                label, role, alternatives.join(", ")
            ),
        });
    }

    let target = match matches.get(index) {
        Some(el) => el,
        None => {
            return serde_json::json!({
                "success": false,
                "error": format!(
                    "Index {} out of range. Found {} matching elements.",
                    index,
                    matches.len()
                ),
            })
        }
    };

    let bounds = match &target.bounds {
        Some(b) => b,
        None => {
            return serde_json::json!({
                "success": false,
                "error": "Element found but has no bounds (may be offscreen or hidden)",
                "element": {
                    "role": target.role,
                    "label": target.label,
                },
            })
        }
    };

    // AX bounds are already in logical screen coordinates — no FrameDims conversion needed
    let (center_x, center_y) = bounds.center();
    match aura_input::mouse::click(center_x, center_y, "left", 1) {
        Ok(()) => serde_json::json!({
            "success": true,
            "element": {
                "role": target.role,
                "label": target.label,
            },
            "clicked_at": {
                "x": center_x,
                "y": center_y,
            },
        }),
        Err(e) => serde_json::json!({
            "success": false,
            "error": format!("Click failed: {e}"),
        }),
    }
}
```

### Step 3: Ensure click_element is NOT caught by the input accessibility guard

The existing guard at line 1593 catches `"move_mouse" | "click" | "type_text" | "press_key" | "scroll" | "drag"`. `click_element` is NOT in that list — it has its own accessibility check inside its handler. This is correct because `click_element` needs accessibility for BOTH reading the AX tree AND clicking.

### Step 4: Run compilation check

Run: `cargo check -p aura-daemon`
Expected: Compiles without errors.

### Step 5: Commit

```bash
git add crates/aura-daemon/src/main.rs
git commit -m "feat: add click_element tool handler with AX tree lookup"
```

---

## Task 8: Final Verification and Cleanup

### Step 1: Run all workspace tests

Run: `cargo test --workspace`
Expected: All tests pass across all crates.

### Step 2: Run clippy

Run: `cargo clippy --workspace -- -D warnings`
Expected: No warnings.

### Step 3: Run cargo fmt

Run: `cargo fmt --all`

### Step 4: Verify the full tool count

Run: `cargo test -p aura-gemini -- tool_declarations -v`
Expected: All tool declaration tests pass, 13 tools total.

### Step 5: Commit any formatting changes

```bash
git add -A
git commit -m "chore: apply cargo fmt"
```

---

## Summary of All Changes

| Task | Files | Description |
|------|-------|-------------|
| 1 | `aura-screen/src/context.rs` | UIElement, ElementBounds types + ScreenContext extension |
| 2 | `aura-screen/src/accessibility.rs` (new), `lib.rs`, `macos.rs` | AX tree walker with element filtering |
| 3 | `aura-screen/src/macos.rs` | Integrate AX tree into get_screen_context |
| 4 | `aura-gemini/src/config.rs` | System prompt rewrite with decision tree |
| 5 | `aura-gemini/src/tools.rs` | 3 new tool declarations + updated descriptions |
| 6 | `aura-daemon/src/main.rs` | activate_app + click_menu_item handlers |
| 7 | `aura-daemon/src/main.rs` | click_element handler with AX lookup |
| 8 | Workspace | Final verification, clippy, fmt |
