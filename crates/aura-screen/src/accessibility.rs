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

/// Depth limit for the fast density probe in `get_focused_app_elements`.
pub const PROBE_MAX_DEPTH: usize = 3;
/// Timeout in milliseconds for the fast density probe.
pub const PROBE_TIMEOUT_MS: u128 = 200;

const AX_ERROR_SUCCESS: i32 = 0;
const AX_VALUE_CG_POINT: u32 = 1;
const AX_VALUE_CG_SIZE: u32 = 2;

/// Interactive roles whose children should be collected (bounded, 1 level, max 10).
const RECURSE_INTO_ROLES: &[&str] = &["AXPopUpButton", "AXComboBox", "AXTabGroup", "AXMenuBar"];

const MAX_CHILDREN_PER_INTERACTIVE: usize = 10;

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
    // AXStaticText provides text label context (e.g. labels next to buttons/fields).
    // Only collected when the element has a non-empty label (enforced in walk_element_with_limits).
    // The existing MAX_ELEMENTS cap prevents flooding on label-heavy UIs.
    "AXStaticText",
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
    fn AXUIElementSetAttributeValue(
        element: CFTypeRef,
        attribute: CFTypeRef,
        value: CFTypeRef,
    ) -> i32;
    fn AXUIElementPerformAction(element: CFTypeRef, action: CFTypeRef) -> i32;
    fn AXUIElementSetMessagingTimeout(element: CFTypeRef, timeout: f32) -> i32;
    fn AXUIElementCopyElementAtPosition(
        application: CFTypeRef,
        x: f32,
        y: f32,
        element: *mut CFTypeRef,
    ) -> i32;
}

// ── RAII wrapper for CFTypeRef ───────────────────────────────────────────────

pub(crate) struct CfRef(CFTypeRef);

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

/// Result of an AX write operation (set value, perform action, set focus).
pub struct AXActionResult {
    pub success: bool,
    pub element: Option<UIElement>,
    pub error: Option<String>,
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

// ── Adaptive limits ───────────────────────────────────────────────────────────

/// Return `(max_elements, max_depth)` based on the number of interactive
/// elements found during the fast density probe.
///
/// | probe_count | max_elements | max_depth | description        |
/// |-------------|-------------|-----------|-------------------|
/// | >= 20       | 150         | 7         | Rich/dense tree    |
/// | >= 5        | 80          | 5         | Moderate density   |
/// | < 5         | 50          | 3         | Sparse/broken tree |
fn adaptive_limits(probe_count: usize) -> (usize, usize) {
    if probe_count >= 20 {
        (150, 7)
    } else if probe_count >= 5 {
        (80, 5)
    } else {
        (50, 3)
    }
}

// ── Tree walker ───────────────────────────────────────────────────────────────

fn walk_element_with_limits(
    element: CFTypeRef,
    depth: usize,
    elements: &mut Vec<UIElement>,
    start_time: &Instant,
    max_elements: usize,
    max_depth: usize,
    timeout_ms: u128,
) {
    if depth > max_depth {
        return;
    }
    if elements.len() >= max_elements {
        return;
    }
    if start_time.elapsed().as_millis() >= timeout_ms {
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

        // AXStaticText with no label provides no useful context — skip it to
        // avoid cluttering the element list with invisible/empty text nodes.
        if role == "AXStaticText" && label.is_none() {
            let children = get_ax_children(element);
            for child in children {
                walk_element_with_limits(
                    child,
                    depth + 1,
                    elements,
                    start_time,
                    max_elements,
                    max_depth,
                    timeout_ms,
                );
                unsafe { CFRelease(child) };
            }
            return;
        }

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
            role: role.clone(),
            label: label.clone(),
            value: value.clone(),
            bounds: bounds.clone(),
            enabled,
            focused,
            parent_label: None,
        });

        // Bounded 1-level recursion into container roles to expose children
        // (dropdown options, tab labels, combo box items, menu bar items).
        if RECURSE_INTO_ROLES.contains(&role.as_str()) {
            let parent_label_for_children = label.clone();
            let children = get_ax_children(element);
            let mut child_count = 0;
            for child in &children {
                if child_count >= MAX_CHILDREN_PER_INTERACTIVE || elements.len() >= max_elements {
                    break;
                }
                if let Some(child_role) = get_ax_string(*child, "AXRole") {
                    let child_label = get_ax_string(*child, "AXTitle")
                        .filter(|s| !s.is_empty())
                        .or_else(|| {
                            get_ax_string(*child, "AXDescription").filter(|s| !s.is_empty())
                        });
                    let child_value = get_ax_string(*child, "AXValue").filter(|s| !s.is_empty());
                    let child_enabled = get_ax_bool(*child, "AXEnabled");
                    let child_bounds = match (get_ax_position(*child), get_ax_size(*child)) {
                        (Some((x, y)), Some((w, h))) => Some(ElementBounds {
                            x,
                            y,
                            width: w,
                            height: h,
                        }),
                        _ => None,
                    };
                    elements.push(UIElement {
                        role: child_role,
                        label: child_label,
                        value: child_value,
                        bounds: child_bounds,
                        enabled: child_enabled,
                        focused: false,
                        parent_label: parent_label_for_children.clone(),
                    });
                    child_count += 1;
                }
            }
            for child in &children {
                unsafe { CFRelease(*child) };
            }
        }
    } else {
        // Not interactive — recurse into children.
        let children = get_ax_children(element);
        for child in children {
            walk_element_with_limits(
                child,
                depth + 1,
                elements,
                start_time,
                max_elements,
                max_depth,
                timeout_ms,
            );
            // Release the retain we added in get_ax_children.
            unsafe { CFRelease(child) };
        }
    }
}

// ── Public API ────────────────────────────────────────────────────────────────

/// Walk the accessibility tree of the frontmost application and return
/// all interactive elements found within limits.
///
/// Uses a two-phase strategy:
/// 1. Fast probe at depth `PROBE_MAX_DEPTH` with `PROBE_TIMEOUT_MS` to count
///    interactive elements and gauge tree density.
/// 2. If the tree is rich enough to warrant a deeper walk, perform a second
///    walk using `adaptive_limits` to tune `max_elements` and `max_depth`.
pub fn get_focused_app_elements() -> Vec<UIElement> {
    let pid = match crate::macos::get_frontmost_pid() {
        Some(p) => p,
        None => return Vec::new(),
    };

    let app_element = unsafe { AXUIElementCreateApplication(pid) };
    if app_element.is_null() {
        return Vec::new();
    }
    unsafe { AXUIElementSetMessagingTimeout(app_element, 1.0) };
    let app_ref = CfRef::new(app_element);

    // ── Phase 1: fast density probe ──────────────────────────────────────────
    let mut probe_elements: Vec<UIElement> = Vec::new();
    let probe_start = Instant::now();
    walk_element_with_limits(
        app_ref.as_raw(),
        0,
        &mut probe_elements,
        &probe_start,
        MAX_ELEMENTS,
        PROBE_MAX_DEPTH,
        PROBE_TIMEOUT_MS,
    );
    let probe_count = probe_elements.len();

    // Determine adaptive limits for the full walk.
    let (adaptive_max_elements, adaptive_max_depth) = adaptive_limits(probe_count);

    // If the probe already hit the element cap or the adaptive depth is no
    // deeper than the probe depth, the probe result is sufficient — return it.
    if probe_count >= MAX_ELEMENTS || adaptive_max_depth <= PROBE_MAX_DEPTH {
        return probe_elements;
    }

    // ── Phase 2: deeper adaptive walk ────────────────────────────────────────
    let mut elements: Vec<UIElement> = Vec::new();
    let start_time = Instant::now();
    walk_element_with_limits(
        app_ref.as_raw(),
        0,
        &mut elements,
        &start_time,
        adaptive_max_elements,
        adaptive_max_depth,
        TIMEOUT_MS,
    );
    elements
}

/// Return the currently focused interactive element, if any.
///
/// Uses a direct `AXFocusedUIElement` query on the app element (single IPC call)
/// instead of walking the entire AX tree.
pub fn get_focused_element() -> Option<UIElement> {
    let pid = crate::macos::get_frontmost_pid()?;
    let app = unsafe { AXUIElementCreateApplication(pid) };
    if app.is_null() {
        return None;
    }
    let app_ref = CfRef::new(app);

    // Query the app's focused element directly
    let attr_key = cf_string_from_str("AXFocusedUIElement");
    let mut value: CFTypeRef = std::ptr::null();
    let ret =
        unsafe { AXUIElementCopyAttributeValue(app_ref.as_raw(), attr_key.as_raw(), &mut value) };
    if ret != AX_ERROR_SUCCESS || value.is_null() {
        return None;
    }
    let focused_ref = CfRef::new(value);

    // Read properties from the focused element
    let role = get_ax_string(focused_ref.as_raw(), "AXRole")?;
    let label = get_ax_string(focused_ref.as_raw(), "AXTitle")
        .filter(|s| !s.is_empty())
        .or_else(|| get_ax_string(focused_ref.as_raw(), "AXDescription").filter(|s| !s.is_empty()));
    let value = get_ax_string(focused_ref.as_raw(), "AXValue").filter(|s| !s.is_empty());
    let enabled = get_ax_bool(focused_ref.as_raw(), "AXEnabled");
    let focused = get_ax_bool(focused_ref.as_raw(), "AXFocused");
    let bounds = match (
        get_ax_position(focused_ref.as_raw()),
        get_ax_size(focused_ref.as_raw()),
    ) {
        (Some((x, y)), Some((w, h))) => Some(ElementBounds {
            x,
            y,
            width: w,
            height: h,
        }),
        _ => None,
    };

    Some(UIElement {
        role,
        label,
        value,
        bounds,
        enabled,
        focused,
        parent_label: None,
    })
}

/// Hit-test the screen at logical coordinates `(x, y)` and return the UI element
/// at that position. If the element directly under the cursor is not interactive
/// (e.g. AXGroup, AXScrollArea), walks up the parent chain to find the nearest
/// interactive ancestor.
///
/// Coordinates are in macOS logical points (same as CGEvent coordinates).
/// Returns `None` if no element is found or Accessibility permission is denied.
pub fn element_at_position(x: f64, y: f64) -> Option<UIElement> {
    let pid = crate::macos::get_frontmost_pid()?;
    let app = unsafe { AXUIElementCreateApplication(pid) };
    if app.is_null() {
        return None;
    }
    let app_ref = CfRef::new(app);
    unsafe { AXUIElementSetMessagingTimeout(app_ref.as_raw(), 0.5) };

    let mut hit_ref: CFTypeRef = std::ptr::null();
    let ret = unsafe {
        AXUIElementCopyElementAtPosition(app_ref.as_raw(), x as f32, y as f32, &mut hit_ref)
    };
    if ret != AX_ERROR_SUCCESS || hit_ref.is_null() {
        return None;
    }
    let hit = CfRef::new(hit_ref);

    // Try the hit element first; if not interactive, walk up parents
    if let Some(el) = build_ui_element(hit.as_raw()) {
        if INTERACTIVE_ROLES.contains(&el.role.as_str()) {
            return Some(el);
        }
    }

    // Walk up the parent chain (max 8 levels) to find an interactive ancestor
    let mut current = hit.as_raw();
    unsafe { CFRetain(current) };
    for _ in 0..8 {
        let attr_key = cf_string_from_str("AXParent");
        let mut parent: CFTypeRef = std::ptr::null();
        let ret =
            unsafe { AXUIElementCopyAttributeValue(current, attr_key.as_raw(), &mut parent) };
        unsafe { CFRelease(current) };
        if ret != AX_ERROR_SUCCESS || parent.is_null() {
            return None;
        }
        current = parent;

        if let Some(el) = build_ui_element(current) {
            if INTERACTIVE_ROLES.contains(&el.role.as_str()) {
                unsafe { CFRelease(current) };
                return Some(el);
            }
        }
    }
    unsafe { CFRelease(current) };
    None
}

/// Build a UIElement from a raw AXUIElement ref without walking children.
fn build_ui_element(element: CFTypeRef) -> Option<UIElement> {
    let role = get_ax_string(element, "AXRole")?;
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
    Some(UIElement {
        role,
        label,
        value,
        bounds,
        enabled,
        focused,
        parent_label: None,
    })
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

// ── AX write helpers ──────────────────────────────────────────────────────────

/// Recursively find a raw AXUIElement ref matching a UIElement by role + label.
///
/// The caller must CFRelease the returned value when done.
/// Kept for use by `click_element_inner`'s error path and potential future callers.
#[allow(dead_code)]
fn find_raw_element(
    element: CFTypeRef,
    target: &UIElement,
    depth: usize,
    start_time: &Instant,
) -> Option<CFTypeRef> {
    if depth > MAX_DEPTH || start_time.elapsed().as_millis() >= TIMEOUT_MS {
        return None;
    }
    let role = get_ax_string(element, "AXRole")?;
    if INTERACTIVE_ROLES.contains(&role.as_str()) {
        let label = get_ax_string(element, "AXTitle")
            .filter(|s| !s.is_empty())
            .or_else(|| get_ax_string(element, "AXDescription").filter(|s| !s.is_empty()));
        if role == target.role && label == target.label {
            // Retain so caller can safely wrap in CfRef (which will CFRelease on drop)
            unsafe { CFRetain(element) };
            return Some(element);
        }
        return None; // Interactive element but doesn't match — don't recurse into it
    }
    let children = get_ax_children(element);
    for child in &children {
        if let Some(found) = find_raw_element(*child, target, depth + 1, start_time) {
            // found is already retained by the recursive call — release all children
            // (if found == child, the retain from the recursive match keeps it alive)
            for c in &children {
                unsafe { CFRelease(*c) };
            }
            return Some(found);
        }
    }
    for child in &children {
        unsafe { CFRelease(*child) };
    }
    None
}

/// Single-pass tree walk that finds the first interactive element matching
/// the given label/role filters, returning both the raw AXUIElement ref and
/// the UIElement metadata. Avoids building a full element list and re-walking.
fn find_element_single_pass(
    element: CFTypeRef,
    label_filter: Option<&str>,
    role_filter: Option<&str>,
    skip: &mut usize,
    depth: usize,
    start_time: &Instant,
) -> Option<(CFTypeRef, UIElement)> {
    if depth > MAX_DEPTH || start_time.elapsed().as_millis() >= TIMEOUT_MS {
        return None;
    }

    let role = get_ax_string(element, "AXRole")?;

    if INTERACTIVE_ROLES.contains(&role.as_str()) {
        // Check role filter
        if let Some(r) = role_filter
            && !matches_role(&role, r)
        {
            return None;
        }

        // Collect label: prefer AXTitle, fall back to AXDescription
        let label = get_ax_string(element, "AXTitle")
            .filter(|s| !s.is_empty())
            .or_else(|| get_ax_string(element, "AXDescription").filter(|s| !s.is_empty()));

        let value = get_ax_string(element, "AXValue").filter(|s| !s.is_empty());

        // Check label filter
        if let Some(lbl) = label_filter {
            let lbl_lower = lbl.to_lowercase();
            let in_label = label
                .as_deref()
                .map(|s| s.to_lowercase().contains(&lbl_lower))
                .unwrap_or(false);
            let in_value = value
                .as_deref()
                .map(|s| s.to_lowercase().contains(&lbl_lower))
                .unwrap_or(false);
            if !in_label && !in_value {
                return None;
            }
        }

        // Match found — check skip count for index support
        if *skip > 0 {
            *skip -= 1;
            return None;
        }

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

        // Retain so caller can safely wrap in CfRef
        unsafe { CFRetain(element) };
        return Some((
            element,
            UIElement {
                role,
                label,
                value,
                bounds,
                enabled,
                focused,
                parent_label: None,
            },
        ));
    }

    // Not interactive — recurse into children
    let children = get_ax_children(element);
    for child in &children {
        if let Some(found) = find_element_single_pass(
            *child,
            label_filter,
            role_filter,
            skip,
            depth + 1,
            start_time,
        ) {
            // Release all children (found element was separately retained)
            for c in &children {
                unsafe { CFRelease(*c) };
            }
            return Some(found);
        }
    }
    for child in &children {
        unsafe { CFRelease(*child) };
    }
    None
}

/// Find a raw AXUIElement and its UIElement metadata for the given label/role filters.
fn find_ax_raw(label: Option<&str>, role: Option<&str>) -> Option<(CfRef, UIElement)> {
    find_ax_raw_nth(label, role, 0)
}

/// Find the nth matching raw AXUIElement and its UIElement metadata.
pub(crate) fn find_ax_raw_nth(
    label: Option<&str>,
    role: Option<&str>,
    index: usize,
) -> Option<(CfRef, UIElement)> {
    let pid = crate::macos::get_frontmost_pid()?;
    let app = unsafe { AXUIElementCreateApplication(pid) };
    if app.is_null() {
        return None;
    }
    let app_ref = CfRef::new(app);
    let start = Instant::now();
    let mut skip = index;
    let (raw, el) = find_element_single_pass(app_ref.as_raw(), label, role, &mut skip, 0, &start)?;
    Some((CfRef::new(raw), el))
}

// ── Public AX write functions ─────────────────────────────────────────────────

/// Attempt to scroll an element into view using the AXScrollToVisible action.
/// Returns `true` if the action was accepted by the accessibility server.
pub(crate) fn ax_scroll_to_visible(element_ref: &CfRef) -> bool {
    let action_key = cf_string_from_str("AXScrollToVisible");
    let ret = unsafe { AXUIElementPerformAction(element_ref.as_raw(), action_key.as_raw()) };
    ret == AX_ERROR_SUCCESS
}

/// Find the Nth element matching label/role, scroll it into view, then return
/// the updated `UIElement` (with refreshed bounds). Returns `None` if the
/// element cannot be found or still has no bounds after scrolling.
pub fn scroll_to_visible_and_get_element(
    label: Option<&str>,
    role: Option<&str>,
    index: usize,
) -> Option<UIElement> {
    let (raw_ref, _el) = find_ax_raw_nth(label, role, index)?;
    ax_scroll_to_visible(&raw_ref);
    // Give the scroll animation time to settle before re-querying bounds.
    std::thread::sleep(std::time::Duration::from_millis(200));
    // Re-query the element to pick up updated position/size after scrolling.
    let (_raw2, el) = find_ax_raw_nth(label, role, index)?;
    Some(el)
}

/// Perform an AX action (e.g. "AXPress") on the element matching `label`/`role`.
pub fn ax_perform_action(label: Option<&str>, role: Option<&str>, action: &str) -> AXActionResult {
    ax_perform_action_nth(label, role, action, 0)
}

/// Perform an AX action on the Nth matching element (0-indexed).
/// Single-pass walk: finds the exact element by index, then performs the action on it.
pub fn ax_perform_action_nth(
    label: Option<&str>,
    role: Option<&str>,
    action: &str,
    index: usize,
) -> AXActionResult {
    let Some((raw_ref, element)) = find_ax_raw_nth(label, role, index) else {
        return AXActionResult {
            success: false,
            element: None,
            error: Some(format!(
                "no element found for label={label:?} role={role:?} index={index}"
            )),
        };
    };
    let action_key = cf_string_from_str(action);
    let ret = unsafe { AXUIElementPerformAction(raw_ref.as_raw(), action_key.as_raw()) };
    if ret == AX_ERROR_SUCCESS {
        AXActionResult {
            success: true,
            element: Some(element),
            error: None,
        }
    } else {
        AXActionResult {
            success: false,
            element: Some(element),
            error: Some(format!("AXUIElementPerformAction returned {ret}")),
        }
    }
}

/// Set the AXValue of the element matching `label`/`role` to `value`.
pub fn ax_set_value(label: Option<&str>, role: Option<&str>, value: &str) -> AXActionResult {
    let Some((raw_ref, element)) = find_ax_raw(label, role) else {
        return AXActionResult {
            success: false,
            element: None,
            error: Some(format!(
                "no element found for label={label:?} role={role:?}"
            )),
        };
    };
    let attr_key = cf_string_from_str("AXValue");
    let cf_value = cf_string_from_str(value);
    let ret = unsafe {
        AXUIElementSetAttributeValue(raw_ref.as_raw(), attr_key.as_raw(), cf_value.as_raw())
    };
    if ret == AX_ERROR_SUCCESS {
        AXActionResult {
            success: true,
            element: Some(element),
            error: None,
        }
    } else {
        AXActionResult {
            success: false,
            element: Some(element),
            error: Some(format!("AXUIElementSetAttributeValue returned {ret}")),
        }
    }
}

/// Set keyboard focus on the element matching `label`/`role`.
pub fn ax_set_focused(label: Option<&str>, role: Option<&str>) -> AXActionResult {
    let Some((raw_ref, element)) = find_ax_raw(label, role) else {
        return AXActionResult {
            success: false,
            element: None,
            error: Some(format!(
                "no element found for label={label:?} role={role:?}"
            )),
        };
    };
    let attr_key = cf_string_from_str("AXFocused");
    let ret = unsafe {
        AXUIElementSetAttributeValue(
            raw_ref.as_raw(),
            attr_key.as_raw(),
            kCFBooleanTrue as CFTypeRef,
        )
    };
    if ret == AX_ERROR_SUCCESS {
        AXActionResult {
            success: true,
            element: Some(element),
            error: None,
        }
    } else {
        AXActionResult {
            success: false,
            element: Some(element),
            error: Some(format!(
                "AXUIElementSetAttributeValue(AXFocused) returned {ret}"
            )),
        }
    }
}

/// Check if a subrole indicates a secure (password) text field.
/// macOS blocks synthetic keyboard input to these fields via SecureEventInput.
pub fn is_secure_text_subrole(subrole: &str) -> bool {
    subrole == "AXSecureTextField"
}

/// Check if the currently focused element is a secure text field.
/// Returns true if the focused element's subrole is AXSecureTextField.
pub fn is_focused_element_secure() -> bool {
    let pid = match crate::macos::get_frontmost_pid() {
        Some(p) => p,
        None => return false,
    };

    let app = unsafe { AXUIElementCreateApplication(pid) };
    if app.is_null() {
        return false;
    }
    let app_ref = CfRef::new(app);

    // Get the raw AXUIElement ref for the focused element
    let attr_key = cf_string_from_str("AXFocusedUIElement");
    let mut focused: CFTypeRef = std::ptr::null();
    let err =
        unsafe { AXUIElementCopyAttributeValue(app_ref.as_raw(), attr_key.as_raw(), &mut focused) };
    if err != AX_ERROR_SUCCESS || focused.is_null() {
        return false;
    }
    let focused_ref = CfRef::new(focused);

    // Get the subrole of the focused element
    match get_ax_string(focused_ref.as_raw(), "AXSubrole") {
        Some(subrole) => is_secure_text_subrole(&subrole),
        None => false,
    }
}

/// Collect AXMenuItem elements from the frontmost app's AX tree.
pub fn get_menu_items() -> Vec<UIElement> {
    let pid = match crate::macos::get_frontmost_pid() {
        Some(p) => p,
        None => return Vec::new(),
    };
    let app = unsafe { AXUIElementCreateApplication(pid) };
    if app.is_null() {
        return Vec::new();
    }
    let app_ref = CfRef::new(app);
    let mut elements = Vec::new();
    let start = std::time::Instant::now();
    collect_menu_items(app_ref.as_raw(), 0, &mut elements, &start);
    elements
}

fn collect_menu_items(
    element: CFTypeRef,
    depth: usize,
    elements: &mut Vec<UIElement>,
    start_time: &std::time::Instant,
) {
    if depth > 8 || elements.len() >= 30 || start_time.elapsed().as_millis() >= 500 {
        return;
    }
    let role = match get_ax_string(element, "AXRole") {
        Some(r) => r,
        None => return,
    };
    if role == "AXMenuItem" {
        let label = get_ax_string(element, "AXTitle")
            .filter(|s| !s.is_empty())
            .or_else(|| get_ax_string(element, "AXDescription").filter(|s| !s.is_empty()));
        let enabled = get_ax_bool(element, "AXEnabled");
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
            value: None,
            bounds,
            enabled,
            focused: false,
            parent_label: None,
        });
        // Recurse into children to capture submenu items
    }
    let children = get_ax_children(element);
    for child in &children {
        collect_menu_items(*child, depth + 1, elements, start_time);
    }
    for child in &children {
        unsafe { CFRelease(*child) };
    }
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
            parent_label: None,
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
        // AXStaticText is included to expose text labels for Gemini context.
        assert!(INTERACTIVE_ROLES.contains(&"AXStaticText"));
        // Container/layout roles that provide no actionable context must NOT be present.
        assert!(!INTERACTIVE_ROLES.contains(&"AXGroup"));
    }

    // ── AXActionResult tests ───────────────────────────────────────────────────

    #[test]
    fn ax_action_result_default_success() {
        let result = AXActionResult {
            success: true,
            element: None,
            error: None,
        };
        assert!(result.success);
        assert!(result.element.is_none());
        assert!(result.error.is_none());
    }

    #[test]
    fn ax_action_result_with_element() {
        let el = make_element("AXButton", "OK");
        let result = AXActionResult {
            success: true,
            element: Some(el.clone()),
            error: None,
        };
        assert!(result.success);
        assert_eq!(result.element.unwrap().role, "AXButton");
    }

    #[test]
    fn ax_action_result_with_error() {
        let result = AXActionResult {
            success: false,
            element: None,
            error: Some("something went wrong".to_string()),
        };
        assert!(!result.success);
        assert_eq!(result.error.as_deref(), Some("something went wrong"));
    }

    // ── get_focused_element tests ──────────────────────────────────────────────

    #[test]
    fn get_focused_element_returns_none_for_unfocused_elements() {
        // All elements have focused=false via make_element
        let elements: Vec<UIElement> = vec![
            make_element("AXButton", "Submit"),
            make_element("AXTextField", "Email"),
        ];
        let focused = elements.into_iter().find(|el| el.focused);
        assert!(focused.is_none());
    }

    #[test]
    fn get_focused_element_finds_focused() {
        let mut el = make_element("AXTextField", "Search");
        el.focused = true;
        let elements = vec![make_element("AXButton", "Submit"), el];
        let focused = elements.into_iter().find(|e| e.focused);
        assert!(focused.is_some());
        assert_eq!(focused.unwrap().role, "AXTextField");
    }

    // ── adaptive_limits tests ──────────────────────────────────────────────────

    #[test]
    fn adaptive_limits_scale_with_density() {
        let (max_el, max_dep) = adaptive_limits(25);
        assert_eq!(max_el, 150);
        assert_eq!(max_dep, 7);
        let (max_el, max_dep) = adaptive_limits(10);
        assert_eq!(max_el, 80);
        assert_eq!(max_dep, 5);
        let (max_el, max_dep) = adaptive_limits(3);
        assert_eq!(max_el, 50);
        assert_eq!(max_dep, 3);
    }

    // ── AX write no-match error path tests ────────────────────────────────────

    #[test]
    fn ax_perform_action_no_match_returns_error() {
        // No real AX tree (no frontmost app in test environment) — exercises no-match path.
        let result = ax_perform_action(Some("NonExistentButton99"), Some("AXButton"), "AXPress");
        assert!(!result.success);
        assert!(result.error.is_some());
    }

    #[test]
    fn ax_set_value_no_match_returns_error() {
        let result = ax_set_value(Some("NonExistentField99"), Some("AXTextField"), "hello");
        assert!(!result.success);
        assert!(result.error.is_some());
    }

    #[test]
    fn ax_set_focused_no_match_returns_error() {
        let result = ax_set_focused(Some("NonExistentField99"), Some("AXTextField"));
        assert!(!result.success);
        assert!(result.error.is_some());
    }

    #[test]
    fn is_secure_field_detects_password_subroles() {
        assert!(is_secure_text_subrole("AXSecureTextField"));
        assert!(!is_secure_text_subrole("AXTextField"));
        assert!(!is_secure_text_subrole("AXTextArea"));
        assert!(!is_secure_text_subrole(""));
    }
}
