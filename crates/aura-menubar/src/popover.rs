use cocoa::base::{NO, YES, id, nil};
use cocoa::foundation::{NSPoint, NSRect, NSSize, NSString};
use objc::{class, msg_send, sel, sel_impl};

const MAX_MESSAGES: u32 = 100;

/// Semantic status category used for coloring the status label.
/// Replaces string `.contains()` matching with a proper enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusColor {
    Green,
    Amber,
    Red,
    Gray,
}

impl StatusColor {
    /// Classify a status text string into a semantic color.
    fn from_status_text(text: &str) -> Self {
        if text.contains("Connected") || text.contains("Listening") {
            StatusColor::Green
        } else if text.contains("Reconnecting") || text.contains("Running") {
            StatusColor::Amber
        } else if text.contains("Error") || text.contains("Mic") {
            StatusColor::Red
        } else {
            StatusColor::Gray
        }
    }

    /// The status dot prefix character.
    fn dot_char(self) -> &'static str {
        match self {
            StatusColor::Green | StatusColor::Amber | StatusColor::Red => "\u{25CF}", // Filled
            StatusColor::Gray => "\u{25CB}",                                          // Empty
        }
    }

    /// Returns an NSColor `id` for this status color.
    ///
    /// # Safety
    /// Must be called on the main thread.
    unsafe fn ns_color(self) -> id {
        unsafe {
            match self {
                StatusColor::Green => {
                    msg_send![class!(NSColor), colorWithRed: 0.30f64
                        green: 0.75f64 blue: 0.45f64 alpha: 1.0f64]
                }
                StatusColor::Amber => {
                    msg_send![class!(NSColor), colorWithRed: 0.90f64
                        green: 0.72f64 blue: 0.25f64 alpha: 1.0f64]
                }
                StatusColor::Red => {
                    msg_send![class!(NSColor), colorWithRed: 0.90f64
                        green: 0.30f64 blue: 0.30f64 alpha: 1.0f64]
                }
                StatusColor::Gray => msg_send![class!(NSColor), secondaryLabelColor],
            }
        }
    }
}

pub struct AuraPopover {
    popover: id,
    scroll_view: id,
    stack_view: id,
    status_label: id,
    message_count: u32,
}

// SAFETY: AuraPopover holds Objective-C `id` pointers that are only ever
// accessed on the main thread. The struct is sent once from the initialization
// site into GLOBAL_STATE, after which all access is main-thread-only via
// NSTimer callbacks and ObjC event handlers mediated by the GLOBAL_STATE lock.
unsafe impl Send for AuraPopover {}

impl AuraPopover {
    /// Create a new popover with a scroll view and status header.
    ///
    /// # Safety
    /// Must be called on the main thread (AppKit requirement).
    pub unsafe fn new() -> Self {
        unsafe {
            let popover: id = msg_send![class!(NSPopover), alloc];
            let popover: id = msg_send![popover, init];

            let _: () = msg_send![popover, setContentSize: NSSize::new(340.0, 500.0)];
            let _: () = msg_send![popover, setBehavior: 1i64]; // Transient
            let _: () = msg_send![popover, setAnimates: YES];

            // VoiceOver: mark the popover for accessibility
            let a11y_label = NSString::alloc(nil).init_str("Aura conversation");
            let _: () = msg_send![popover, setAccessibilityLabel: a11y_label];
            let _: () = msg_send![a11y_label, release];

            let vc: id = msg_send![class!(NSViewController), alloc];
            let vc: id = msg_send![vc, init];

            let frame = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(340.0, 500.0));

            // Create NSScrollView filling the frame
            let scroll_view: id = msg_send![class!(NSScrollView), alloc];
            let scroll_view: id = msg_send![scroll_view, initWithFrame: frame];
            let _: () = msg_send![scroll_view, setHasVerticalScroller: YES];
            let _: () = msg_send![scroll_view, setDrawsBackground: NO];

            // Create NSStackView with vertical orientation
            let stack_view: id = msg_send![class!(NSStackView), alloc];
            let stack_view: id = msg_send![stack_view, init];
            let _: () = msg_send![stack_view, setOrientation: 1i64]; // Vertical
            let _: () = msg_send![stack_view, setSpacing: 8.0f64];
            let _: () = msg_send![stack_view, setAlignment: 9i64]; // NSLayoutAttributeLeading

            // Edge insets: top, left, bottom, right
            #[repr(C)]
            struct NSEdgeInsets {
                top: f64,
                left: f64,
                bottom: f64,
                right: f64,
            }
            let insets = NSEdgeInsets {
                top: 12.0,
                left: 12.0,
                bottom: 12.0,
                right: 12.0,
            };
            let _: () = msg_send![stack_view, setEdgeInsets: insets];

            // Set stack view as the scroll view's document view
            let _: () = msg_send![scroll_view, setDocumentView: stack_view];

            // Disable autoresizing mask constraints on the stack view
            let _: () = msg_send![stack_view, setTranslatesAutoresizingMaskIntoConstraints: NO];

            // Pin stack view width to scroll view's content clip view width
            let clip_view: id = msg_send![scroll_view, contentView];
            let stack_width: id = msg_send![stack_view, widthAnchor];
            let clip_width: id = msg_send![clip_view, widthAnchor];
            let constraint: id = msg_send![stack_width, constraintEqualToAnchor: clip_width];
            let _: () = msg_send![constraint, setActive: YES];

            // Pin stack view top to document view top
            let stack_top: id = msg_send![stack_view, topAnchor];
            let clip_top: id = msg_send![clip_view, topAnchor];
            let top_constraint: id = msg_send![stack_top, constraintEqualToAnchor: clip_top];
            let _: () = msg_send![top_constraint, setActive: YES];

            // Use system window background color for the scroll view layer
            let _: () = msg_send![scroll_view, setWantsLayer: YES];
            let layer: id = msg_send![scroll_view, layer];
            let bg: id = msg_send![class!(NSColor), windowBackgroundColor];
            let cg_color: id = msg_send![bg, CGColor];
            let _: () = msg_send![layer, setBackgroundColor: cg_color];

            // Create a container view to hold status bar + scroll view
            let container: id = msg_send![class!(NSView), alloc];
            let container: id = msg_send![container, initWithFrame: frame];
            let _: () = msg_send![container, setWantsLayer: YES];

            // Create status label at the top — acts as a branded header
            let status_label: id = msg_send![class!(NSTextField), alloc];
            let status_label: id = msg_send![status_label, init];
            let status_str = NSString::alloc(nil).init_str("\u{25CF} Connecting...");
            let _: () = msg_send![status_label, setStringValue: status_str];
            let _: () = msg_send![status_str, release];
            let _: () = msg_send![status_label, setBezeled: NO];
            let _: () = msg_send![status_label, setDrawsBackground: NO];
            let _: () = msg_send![status_label, setEditable: NO];
            let _: () = msg_send![status_label, setSelectable: NO];
            let _: () = msg_send![status_label, setAlignment: 0i64]; // NSTextAlignmentLeft
            // Use medium weight for the status — slightly bolder
            let status_font: id = msg_send![class!(NSFont),
                systemFontOfSize: 11.0f64 weight: 0.23f64]; // NSFontWeightMedium
            let _: () = msg_send![status_label, setFont: status_font];
            // Use system semantic color for status text
            let status_color: id = msg_send![class!(NSColor), secondaryLabelColor];
            let _: () = msg_send![status_label, setTextColor: status_color];
            let _: () = msg_send![status_label, setTranslatesAutoresizingMaskIntoConstraints: NO];

            // VoiceOver: accessibility label on the status field
            let status_a11y = NSString::alloc(nil).init_str("Aura connection status");
            let _: () = msg_send![status_label, setAccessibilityLabel: status_a11y];
            let _: () = msg_send![status_a11y, release];

            // Subtle separator line between header and messages
            let separator: id = msg_send![class!(NSBox), alloc];
            let separator: id = msg_send![separator, init];
            let _: () = msg_send![separator, setBoxType: 3i64]; // NSBoxSeparator
            let _: () = msg_send![separator, setTitlePosition: 0i64]; // NSNoTitle
            let _: () = msg_send![separator, setTranslatesAutoresizingMaskIntoConstraints: NO];

            // Add all subviews to container
            let _: () = msg_send![container, addSubview: status_label];
            let _: () = msg_send![container, addSubview: separator];
            let _: () = msg_send![scroll_view, setTranslatesAutoresizingMaskIntoConstraints: NO];
            let _: () = msg_send![container, addSubview: scroll_view];

            // Layout: status label at top, separator, then scroll view fills rest
            let sl_top: id = msg_send![status_label, topAnchor];
            let c_top: id = msg_send![container, topAnchor];
            let c1: id = msg_send![sl_top, constraintEqualToAnchor: c_top constant: 8.0f64];
            let _: () = msg_send![c1, setActive: YES];

            let sl_leading: id = msg_send![status_label, leadingAnchor];
            let c_leading: id = msg_send![container, leadingAnchor];
            let c2: id =
                msg_send![sl_leading, constraintEqualToAnchor: c_leading constant: 12.0f64];
            let _: () = msg_send![c2, setActive: YES];

            let sl_trailing: id = msg_send![status_label, trailingAnchor];
            let c_trailing: id = msg_send![container, trailingAnchor];
            let c3: id =
                msg_send![sl_trailing, constraintEqualToAnchor: c_trailing constant: -12.0f64];
            let _: () = msg_send![c3, setActive: YES];

            // Auto-size status label height using intrinsicContentSize instead of fixed 20px.
            // Content hugging at required priority ensures the label wraps its text tightly.
            let _: () = msg_send![status_label,
                setContentHuggingPriority: 999.0f32 forOrientation: 1i64]; // Vertical

            // Separator below status label
            let sep_top: id = msg_send![separator, topAnchor];
            let sl_bottom: id = msg_send![status_label, bottomAnchor];
            let sep_c1: id =
                msg_send![sep_top, constraintEqualToAnchor: sl_bottom constant: 6.0f64];
            let _: () = msg_send![sep_c1, setActive: YES];

            let sep_leading: id = msg_send![separator, leadingAnchor];
            let sep_c2: id =
                msg_send![sep_leading, constraintEqualToAnchor: c_leading constant: 12.0f64];
            let _: () = msg_send![sep_c2, setActive: YES];

            let sep_trailing: id = msg_send![separator, trailingAnchor];
            let sep_c3: id =
                msg_send![sep_trailing, constraintEqualToAnchor: c_trailing constant: -12.0f64];
            let _: () = msg_send![sep_c3, setActive: YES];

            let sep_height: id = msg_send![separator, heightAnchor];
            let sep_c4: id = msg_send![sep_height, constraintEqualToConstant: 1.0f64];
            let _: () = msg_send![sep_c4, setActive: YES];

            // Scroll view below separator
            let sv_top: id = msg_send![scroll_view, topAnchor];
            let sep_bottom: id = msg_send![separator, bottomAnchor];
            let c5: id = msg_send![sv_top, constraintEqualToAnchor: sep_bottom constant: 4.0f64];
            let _: () = msg_send![c5, setActive: YES];

            let sv_leading: id = msg_send![scroll_view, leadingAnchor];
            let c6: id = msg_send![sv_leading, constraintEqualToAnchor: c_leading];
            let _: () = msg_send![c6, setActive: YES];

            let sv_trailing: id = msg_send![scroll_view, trailingAnchor];
            let c7: id = msg_send![sv_trailing, constraintEqualToAnchor: c_trailing];
            let _: () = msg_send![c7, setActive: YES];

            let sv_bottom: id = msg_send![scroll_view, bottomAnchor];
            let c_bottom: id = msg_send![container, bottomAnchor];
            let c8: id = msg_send![sv_bottom, constraintEqualToAnchor: c_bottom];
            let _: () = msg_send![c8, setActive: YES];

            let _: () = msg_send![vc, setView: container];
            let _: () = msg_send![popover, setContentViewController: vc];

            Self {
                popover,
                scroll_view,
                stack_view,
                status_label,
                message_count: 0,
            }
        }
    }

    /// Toggle the popover's visibility relative to the given view.
    ///
    /// # Safety
    /// Must be called on the main thread (AppKit requirement).
    /// `relative_to` must be a valid NSView pointer.
    pub fn raw(&self) -> id {
        self.popover
    }

    /// Append a chat message bubble to the popover's scroll view.
    ///
    /// # Safety
    /// Must be called on the main thread (AppKit requirement).
    pub unsafe fn add_message(&mut self, text: &str, is_user: bool) {
        unsafe {
            // Remove oldest message if at capacity
            if self.message_count >= MAX_MESSAGES {
                let subviews: id = msg_send![self.stack_view, arrangedSubviews];
                let count: usize = msg_send![subviews, count];
                if count > 0 {
                    let first: id = msg_send![subviews, objectAtIndex: 0usize];
                    let _: () = msg_send![self.stack_view, removeArrangedSubview: first];
                    let _: () = msg_send![first, removeFromSuperview];
                    self.message_count -= 1;
                }
            }

            // Create bubble container — this gives us inner padding
            let bubble: id = msg_send![class!(NSView), alloc];
            let bubble: id = msg_send![bubble, init];
            let _: () = msg_send![bubble, setTranslatesAutoresizingMaskIntoConstraints: NO];
            let _: () = msg_send![bubble, setWantsLayer: YES];
            let bubble_layer: id = msg_send![bubble, layer];
            let _: () = msg_send![bubble_layer, setCornerRadius: 14.0f64];

            // Bubble background color — use system semantic colors for dark/light mode
            let bg_color: id = if is_user {
                // System accent-based tint for user messages
                msg_send![class!(NSColor), controlAccentColor]
            } else {
                // Elevated surface using system color
                msg_send![class!(NSColor), controlBackgroundColor]
            };
            let cg_bg: id = msg_send![bg_color, CGColor];
            let _: () = msg_send![bubble_layer, setBackgroundColor: cg_bg];

            // Create the text field inside the bubble
            let label: id = msg_send![class!(NSTextField), alloc];
            let label: id = msg_send![label, init];

            let ns_text = NSString::alloc(nil).init_str(text);
            let _: () = msg_send![label, setStringValue: ns_text];
            let _: () = msg_send![ns_text, release];
            let _: () = msg_send![label, setBezeled: NO];
            let _: () = msg_send![label, setDrawsBackground: NO];
            let _: () = msg_send![label, setEditable: NO];
            let _: () = msg_send![label, setSelectable: YES];

            // Word wrapping with unlimited lines
            let _: () = msg_send![label, setLineBreakMode: 0i64]; // NSLineBreakByWordWrapping
            let _: () = msg_send![label, setMaximumNumberOfLines: 0i64]; // Unlimited
            let _: () = msg_send![label, setPreferredMaxLayoutWidth: 210.0f64];

            let font: id = msg_send![class!(NSFont), systemFontOfSize: 13.0f64];
            let _: () = msg_send![label, setFont: font];

            // Text color — use system semantic colors for proper dark/light mode
            let text_color: id = if is_user {
                // White text on accent-colored bubble
                msg_send![class!(NSColor), alternateSelectedControlTextColor]
            } else {
                // Standard label color for assistant bubbles
                msg_send![class!(NSColor), labelColor]
            };
            let _: () = msg_send![label, setTextColor: text_color];

            // Add label inside bubble with inner padding
            let _: () = msg_send![label, setTranslatesAutoresizingMaskIntoConstraints: NO];
            let _: () = msg_send![bubble, addSubview: label];

            // Inner padding: 10px horizontal, 8px vertical
            let l_top: id = msg_send![label, topAnchor];
            let b_top: id = msg_send![bubble, topAnchor];
            let lt: id = msg_send![l_top, constraintEqualToAnchor: b_top constant: 8.0f64];
            let _: () = msg_send![lt, setActive: YES];

            let l_bottom: id = msg_send![label, bottomAnchor];
            let b_bottom: id = msg_send![bubble, bottomAnchor];
            let lb: id = msg_send![l_bottom, constraintEqualToAnchor: b_bottom constant: -8.0f64];
            let _: () = msg_send![lb, setActive: YES];

            let l_leading: id = msg_send![label, leadingAnchor];
            let b_leading: id = msg_send![bubble, leadingAnchor];
            let ll: id = msg_send![l_leading, constraintEqualToAnchor: b_leading constant: 10.0f64];
            let _: () = msg_send![ll, setActive: YES];

            let l_trailing: id = msg_send![label, trailingAnchor];
            let b_trailing: id = msg_send![bubble, trailingAnchor];
            let lr: id =
                msg_send![l_trailing, constraintEqualToAnchor: b_trailing constant: -10.0f64];
            let _: () = msg_send![lr, setActive: YES];

            // Create a wrapper NSView that fills the stack width for alignment
            let wrapper: id = msg_send![class!(NSView), alloc];
            let wrapper: id = msg_send![wrapper, init];
            let _: () = msg_send![wrapper, setTranslatesAutoresizingMaskIntoConstraints: NO];
            let _: () = msg_send![wrapper, addSubview: bubble];

            // Constrain bubble vertically within wrapper
            let bubble_top: id = msg_send![bubble, topAnchor];
            let wrapper_top: id = msg_send![wrapper, topAnchor];
            let top_c: id = msg_send![bubble_top,
                constraintEqualToAnchor: wrapper_top constant: 3.0f64];
            let _: () = msg_send![top_c, setActive: YES];

            let bubble_bottom: id = msg_send![bubble, bottomAnchor];
            let wrapper_bottom: id = msg_send![wrapper, bottomAnchor];
            let bottom_c: id = msg_send![bubble_bottom,
                constraintEqualToAnchor: wrapper_bottom constant: -3.0f64];
            let _: () = msg_send![bottom_c, setActive: YES];

            // Horizontal alignment: user messages trailing, assistant leading
            if is_user {
                let bubble_trailing: id = msg_send![bubble, trailingAnchor];
                let wrapper_trailing: id = msg_send![wrapper, trailingAnchor];
                let trailing_c: id =
                    msg_send![bubble_trailing, constraintEqualToAnchor: wrapper_trailing];
                let _: () = msg_send![trailing_c, setActive: YES];

                // Don't stretch full width — minimum 70px gap on left
                let bubble_leading: id = msg_send![bubble, leadingAnchor];
                let wrapper_leading: id = msg_send![wrapper, leadingAnchor];
                let leading_c: id = msg_send![bubble_leading,
                    constraintGreaterThanOrEqualToAnchor: wrapper_leading constant: 70.0f64];
                let _: () = msg_send![leading_c, setActive: YES];
            } else {
                let bubble_leading: id = msg_send![bubble, leadingAnchor];
                let wrapper_leading: id = msg_send![wrapper, leadingAnchor];
                let leading_c: id =
                    msg_send![bubble_leading, constraintEqualToAnchor: wrapper_leading];
                let _: () = msg_send![leading_c, setActive: YES];

                // Don't stretch full width — minimum 70px gap on right
                let bubble_trailing: id = msg_send![bubble, trailingAnchor];
                let wrapper_trailing: id = msg_send![wrapper, trailingAnchor];
                let trailing_c: id = msg_send![bubble_trailing,
                    constraintLessThanOrEqualToAnchor: wrapper_trailing constant: -70.0f64];
                let _: () = msg_send![trailing_c, setActive: YES];
            }

            // Add wrapper to the stack view
            let _: () = msg_send![self.stack_view, addArrangedSubview: wrapper];
            self.message_count += 1;

            // Force layout so frame sizes are computed before scrolling
            let _: () = msg_send![self.stack_view, layoutSubtreeIfNeeded];

            // Auto-scroll to bottom
            let clip_view: id = msg_send![self.scroll_view, contentView];
            let doc_view: id = msg_send![self.scroll_view, documentView];
            let doc_frame: NSRect = msg_send![doc_view, frame];
            let clip_bounds: NSRect = msg_send![clip_view, bounds];
            let new_y = doc_frame.size.height - clip_bounds.size.height;
            if new_y > 0.0 {
                let point = NSPoint::new(0.0, new_y);
                let _: () = msg_send![clip_view, scrollToPoint: point];
                let _: () = msg_send![self.scroll_view, reflectScrolledClipView: clip_view];
            }
        }
    }

    /// Update the status header text and color based on connection state.
    ///
    /// # Safety
    /// Must be called on the main thread (AppKit requirement).
    pub unsafe fn set_status(&self, text: &str) {
        unsafe {
            let status = StatusColor::from_status_text(text);

            // Prepend a colored dot based on status
            let decorated = format!("{} {text}", status.dot_char());
            let ns_text = NSString::alloc(nil).init_str(&decorated);
            let _: () = msg_send![self.status_label, setStringValue: ns_text];
            let _: () = msg_send![ns_text, release];

            // Update status label color based on enum
            let color: id = status.ns_color();
            let _: () = msg_send![self.status_label, setTextColor: color];
        }
    }

    /// Remove all chat message bubbles from the popover.
    ///
    /// # Safety
    /// Must be called on the main thread (AppKit requirement).
    pub unsafe fn clear_messages(&mut self) {
        unsafe {
            let subviews: id = msg_send![self.stack_view, arrangedSubviews];
            let count: usize = msg_send![subviews, count];
            for i in (0..count).rev() {
                let view: id = msg_send![subviews, objectAtIndex: i];
                let _: () = msg_send![self.stack_view, removeArrangedSubview: view];
                let _: () = msg_send![view, removeFromSuperview];
            }
            self.message_count = 0;
        }
    }
}

impl Drop for AuraPopover {
    fn drop(&mut self) {
        unsafe {
            let _: () = msg_send![self.popover, release];
        }
    }
}
