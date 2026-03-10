use cocoa::base::{NO, id};
use cocoa::foundation::{NSPoint, NSRect, NSSize};
use objc::{class, msg_send, sel, sel_impl};

use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum DotColor {
    Gray = 0,
    Green = 1,
    Amber = 2,
    Red = 3,
    GreenDim = 4,
}

pub struct AuraStatusItem {
    status_item: id,
    color: Arc<AtomicU8>,
}

unsafe impl Send for AuraStatusItem {}

#[allow(deprecated)]
impl AuraStatusItem {
    /// MUST be called on the main thread.
    pub unsafe fn new() -> Self {
        unsafe {
            let status_bar: id = msg_send![class!(NSStatusBar), systemStatusBar];
            let length: f64 = -1.0; // NSVariableStatusItemLength
            let status_item: id = msg_send![status_bar, statusItemWithLength: length];
            let _: () = msg_send![status_item, retain];

            let color = Arc::new(AtomicU8::new(DotColor::Gray as u8));
            let item = Self { status_item, color };
            item.update_icon(DotColor::Gray);
            item
        }
    }

    pub unsafe fn set_color(&self, color: DotColor) {
        self.color.store(color as u8, Ordering::Relaxed);
        unsafe {
            self.update_icon(color);
        }
    }

    pub fn current_color(&self) -> DotColor {
        match self.color.load(Ordering::Relaxed) {
            1 => DotColor::Green,
            2 => DotColor::Amber,
            3 => DotColor::Red,
            4 => DotColor::GreenDim,
            _ => DotColor::Gray,
        }
    }

    unsafe fn update_icon(&self, color: DotColor) {
        unsafe {
            let size = NSSize::new(18.0, 18.0);
            let image: id = msg_send![class!(NSImage), alloc];
            let image: id = msg_send![image, initWithSize: size];
            let _: () = msg_send![image, lockFocus];

            let (r, g, b): (f64, f64, f64) = match color {
                DotColor::Gray => (0.55, 0.55, 0.55),
                DotColor::Green => (0.30, 0.88, 0.52),
                DotColor::Amber => (1.0, 0.78, 0.28),
                DotColor::Red => (0.92, 0.28, 0.28),
                DotColor::GreenDim => (0.20, 0.55, 0.32),
            };

            // Draw a subtle glow behind the dot for active states
            let dot_size = 10.0f64;
            let offset = (18.0 - dot_size) / 2.0;

            if !matches!(color, DotColor::Gray | DotColor::GreenDim) {
                let glow_color: id =
                    msg_send![class!(NSColor), colorWithRed: r green: g blue: b alpha: 0.25f64];
                let _: () = msg_send![glow_color, setFill];
                let glow_inset = 2.0f64;
                let glow_rect = NSRect::new(
                    NSPoint::new(offset - glow_inset, offset - glow_inset),
                    NSSize::new(dot_size + glow_inset * 2.0, dot_size + glow_inset * 2.0),
                );
                let glow_path: id = msg_send![class!(NSBezierPath), bezierPathWithOvalInRect: glow_rect];
                let _: () = msg_send![glow_path, fill];
            }

            let ns_color: id =
                msg_send![class!(NSColor), colorWithRed: r green: g blue: b alpha: 1.0f64];
            let _: () = msg_send![ns_color, setFill];

            let rect = NSRect::new(
                NSPoint::new(offset, offset),
                NSSize::new(dot_size, dot_size),
            );
            let path: id = msg_send![class!(NSBezierPath), bezierPathWithOvalInRect: rect];
            let _: () = msg_send![path, fill];

            let _: () = msg_send![image, unlockFocus];
            let _: () = msg_send![image, setTemplate: NO];

            let button: id = msg_send![self.status_item, button];
            let _: () = msg_send![button, setImage: image];
        }
    }

    pub fn raw(&self) -> id {
        self.status_item
    }
}

#[allow(deprecated)]
impl Drop for AuraStatusItem {
    fn drop(&mut self) {
        unsafe {
            let _: () = msg_send![self.status_item, release];
        }
    }
}
