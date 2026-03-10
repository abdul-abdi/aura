use cocoa::base::{NO, YES, id, nil};
use cocoa::foundation::{NSPoint, NSRect, NSSize, NSString};
use objc::{class, msg_send, sel, sel_impl};

pub struct AuraPopover {
    popover: id,
    scroll_view: id,
    stack_view: id,
    status_label: id,
}

unsafe impl Send for AuraPopover {}

#[allow(deprecated)]
impl AuraPopover {
    /// MUST be called on the main thread.
    pub unsafe fn new() -> Self {
        unsafe {
            let popover: id = msg_send![class!(NSPopover), alloc];
            let popover: id = msg_send![popover, init];

            let _: () = msg_send![popover, setContentSize: NSSize::new(340.0, 500.0)];
            let _: () = msg_send![popover, setBehavior: 1i64]; // Transient
            let _: () = msg_send![popover, setAnimates: YES];

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

            // Set background color on the scroll view's layer — warm dark charcoal
            let _: () = msg_send![scroll_view, setWantsLayer: YES];
            let layer: id = msg_send![scroll_view, layer];
            let bg: id = msg_send![class!(NSColor),
                colorWithRed: 0.11f64 green: 0.11f64 blue: 0.13f64 alpha: 1.0f64];
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
            let _: () = msg_send![status_label, setBezeled: NO];
            let _: () = msg_send![status_label, setDrawsBackground: NO];
            let _: () = msg_send![status_label, setEditable: NO];
            let _: () = msg_send![status_label, setSelectable: NO];
            let _: () = msg_send![status_label, setAlignment: 0i64]; // NSTextAlignmentLeft
            // Use medium weight for the status — slightly bolder
            let status_font: id = msg_send![class!(NSFont),
                systemFontOfSize: 11.0f64 weight: 0.23f64]; // NSFontWeightMedium
            let _: () = msg_send![status_label, setFont: status_font];
            let status_color: id = msg_send![class!(NSColor),
                colorWithRed: 0.55f64 green: 0.55f64 blue: 0.58f64 alpha: 1.0f64];
            let _: () = msg_send![status_label, setTextColor: status_color];
            let _: () = msg_send![status_label, setTranslatesAutoresizingMaskIntoConstraints: NO];

            // Subtle separator line between header and messages
            let separator: id = msg_send![class!(NSBox), alloc];
            let separator: id = msg_send![separator, init];
            let _: () = msg_send![separator, setBoxType: 3i64]; // NSBoxSeparator
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
            let c2: id = msg_send![sl_leading, constraintEqualToAnchor: c_leading constant: 12.0f64];
            let _: () = msg_send![c2, setActive: YES];

            let sl_trailing: id = msg_send![status_label, trailingAnchor];
            let c_trailing: id = msg_send![container, trailingAnchor];
            let c3: id = msg_send![sl_trailing, constraintEqualToAnchor: c_trailing constant: -12.0f64];
            let _: () = msg_send![c3, setActive: YES];

            let sl_height: id = msg_send![status_label, heightAnchor];
            let c4: id = msg_send![sl_height, constraintEqualToConstant: 20.0f64];
            let _: () = msg_send![c4, setActive: YES];

            // Separator below status label
            let sep_top: id = msg_send![separator, topAnchor];
            let sl_bottom: id = msg_send![status_label, bottomAnchor];
            let sep_c1: id = msg_send![sep_top, constraintEqualToAnchor: sl_bottom constant: 6.0f64];
            let _: () = msg_send![sep_c1, setActive: YES];

            let sep_leading: id = msg_send![separator, leadingAnchor];
            let sep_c2: id = msg_send![sep_leading, constraintEqualToAnchor: c_leading constant: 12.0f64];
            let _: () = msg_send![sep_c2, setActive: YES];

            let sep_trailing: id = msg_send![separator, trailingAnchor];
            let sep_c3: id = msg_send![sep_trailing, constraintEqualToAnchor: c_trailing constant: -12.0f64];
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
            }
        }
    }

    pub unsafe fn toggle(&self, relative_to: id) {
        unsafe {
            let shown: bool = msg_send![self.popover, isShown];
            if shown {
                let _: () = msg_send![self.popover, close];
            } else {
                let bounds: NSRect = msg_send![relative_to, bounds];
                let _: () = msg_send![self.popover, showRelativeToRect: bounds
                    ofView: relative_to
                    preferredEdge: 1u64]; // NSMinYEdge
            }
        }
    }

    pub unsafe fn add_message(&self, text: &str, is_user: bool) {
        unsafe {
            // Create bubble container — this gives us inner padding
            let bubble: id = msg_send![class!(NSView), alloc];
            let bubble: id = msg_send![bubble, init];
            let _: () = msg_send![bubble, setTranslatesAutoresizingMaskIntoConstraints: NO];
            let _: () = msg_send![bubble, setWantsLayer: YES];
            let bubble_layer: id = msg_send![bubble, layer];
            let _: () = msg_send![bubble_layer, setCornerRadius: 14.0f64];

            // Bubble background color — refined palette
            let bg_color: id = if is_user {
                // Vibrant teal-blue accent
                msg_send![class!(NSColor),
                    colorWithRed: 0.18f64 green: 0.52f64 blue: 0.92f64 alpha: 1.0f64]
            } else {
                // Subtle elevated surface
                msg_send![class!(NSColor),
                    colorWithRed: 0.18f64 green: 0.18f64 blue: 0.20f64 alpha: 1.0f64]
            };
            let cg_bg: id = msg_send![bg_color, CGColor];
            let _: () = msg_send![bubble_layer, setBackgroundColor: cg_bg];

            // Create the text field inside the bubble
            let label: id = msg_send![class!(NSTextField), alloc];
            let label: id = msg_send![label, init];

            let ns_text = NSString::alloc(nil).init_str(text);
            let _: () = msg_send![label, setStringValue: ns_text];
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

            // Text color — slightly off-white for better readability
            let text_color: id = msg_send![class!(NSColor),
                colorWithRed: 0.95f64 green: 0.95f64 blue: 0.97f64 alpha: 1.0f64];
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
            let lr: id = msg_send![l_trailing, constraintEqualToAnchor: b_trailing constant: -10.0f64];
            let _: () = msg_send![lr, setActive: YES];

            // Create a wrapper NSView that fills the stack width for alignment
            let wrapper: id = msg_send![class!(NSView), alloc];
            let wrapper: id = msg_send![wrapper, init];
            let _: () =
                msg_send![wrapper, setTranslatesAutoresizingMaskIntoConstraints: NO];
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
                let _: () =
                    msg_send![self.scroll_view, reflectScrolledClipView: clip_view];
            }
        }
    }

    pub unsafe fn set_status(&self, text: &str) {
        unsafe {
            // Prepend a colored dot based on status
            let decorated = if text.contains("Connected") || text.contains("Listening") {
                format!("\u{25CF} {text}")  // Filled circle — green context
            } else if text.contains("Reconnecting") || text.contains("Running") {
                format!("\u{25CF} {text}")  // Filled circle — amber context
            } else if text.contains("Error") || text.contains("Mic") {
                format!("\u{25CF} {text}")  // Filled circle — red context
            } else {
                format!("\u{25CB} {text}")  // Empty circle — inactive
            };
            let ns_text = NSString::alloc(nil).init_str(&decorated);
            let _: () = msg_send![self.status_label, setStringValue: ns_text];

            // Update status label color based on state
            let color: id = if text.contains("Connected") || text.contains("Listening") {
                msg_send![class!(NSColor),
                    colorWithRed: 0.30f64 green: 0.75f64 blue: 0.45f64 alpha: 1.0f64]
            } else if text.contains("Reconnecting") || text.contains("Running") {
                msg_send![class!(NSColor),
                    colorWithRed: 0.90f64 green: 0.72f64 blue: 0.25f64 alpha: 1.0f64]
            } else if text.contains("Error") || text.contains("Mic") {
                msg_send![class!(NSColor),
                    colorWithRed: 0.90f64 green: 0.30f64 blue: 0.30f64 alpha: 1.0f64]
            } else {
                msg_send![class!(NSColor),
                    colorWithRed: 0.50f64 green: 0.50f64 blue: 0.52f64 alpha: 1.0f64]
            };
            let _: () = msg_send![self.status_label, setTextColor: color];
        }
    }

    pub unsafe fn clear_messages(&self) {
        unsafe {
            let subviews: id = msg_send![self.stack_view, arrangedSubviews];
            let count: usize = msg_send![subviews, count];
            for i in (0..count).rev() {
                let view: id = msg_send![subviews, objectAtIndex: i];
                let _: () = msg_send![self.stack_view, removeArrangedSubview: view];
                let _: () = msg_send![view, removeFromSuperview];
            }
        }
    }
}

#[allow(deprecated)]
impl Drop for AuraPopover {
    fn drop(&mut self) {
        unsafe {
            let _: () = msg_send![self.popover, release];
        }
    }
}
