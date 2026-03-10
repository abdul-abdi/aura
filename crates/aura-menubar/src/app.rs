use cocoa::appkit::{NSApp, NSApplicationActivationPolicyAccessory};
use cocoa::base::{id, nil};
use cocoa::foundation::{NSAutoreleasePool, NSString};
use objc::declare::ClassDecl;
use objc::runtime::{Object, Sel};
use objc::{class, msg_send, sel, sel_impl};

use parking_lot::ReentrantMutex;
use std::cell::RefCell;
use tokio::sync::mpsc;

use crate::popover::AuraPopover;
use crate::status_item::{AuraStatusItem, DotColor};

#[derive(Debug, Clone)]
pub enum MenuBarMessage {
    SetColor(DotColor),
    AddMessage { text: String, is_user: bool },
    SetStatus { text: String },
    SetPulsing(bool),
    Reconnect,
    Shutdown,
}

/// Global mutable state accessed from ObjC callbacks.
/// ReentrantMutex allows the same thread to re-acquire the lock (needed when
/// ObjC callbacks triggered by popUpMenuPositioningItem re-enter while lock is held).
/// RefCell provides interior mutability with runtime borrow checking.
static GLOBAL_STATE: ReentrantMutex<RefCell<Option<AppState>>> =
    ReentrantMutex::new(RefCell::new(None));

struct AppState {
    status_item: AuraStatusItem,
    popover: AuraPopover,
    rx: mpsc::Receiver<MenuBarMessage>,
    pulsing: bool,
    pulse_bright: bool,
    pulse_counter: u32,
    reconnect_tx: Option<mpsc::Sender<()>>,
    shutdown_tx: Option<mpsc::Sender<()>>,
}

pub struct MenuBarApp {
    rx: mpsc::Receiver<MenuBarMessage>,
    reconnect_tx: mpsc::Sender<()>,
    shutdown_tx: mpsc::Sender<()>,
}

impl MenuBarApp {
    pub fn new() -> (
        Self,
        mpsc::Sender<MenuBarMessage>,
        mpsc::Receiver<()>,
        mpsc::Receiver<()>,
    ) {
        let (tx, rx) = mpsc::channel(64);
        let (reconnect_tx, reconnect_rx) = mpsc::channel(4);
        let (shutdown_tx, shutdown_rx) = mpsc::channel(4);
        (
            Self {
                rx,
                reconnect_tx,
                shutdown_tx,
            },
            tx,
            reconnect_rx,
            shutdown_rx,
        )
    }

    /// Run the menu bar app. Blocks forever on the main thread.
    pub fn run(self) {
        unsafe {
            let _pool = NSAutoreleasePool::new(nil);
            let app = NSApp();
            let _: () = msg_send![app, setActivationPolicy:
                NSApplicationActivationPolicyAccessory];

            let status_item = AuraStatusItem::new();
            let popover = AuraPopover::new();

            // Register click handler via custom ObjC class
            let handler_class = register_click_handler_class();
            let handler: id = msg_send![handler_class, new];
            let button: id = msg_send![status_item.raw(), button];
            let _: () = msg_send![button, setTarget: handler];
            let _: () = msg_send![button, setAction: sel!(handleClick:)];

            // Enable left+right click: NSEventMaskLeftMouseDown (2) | NSEventMaskRightMouseDown (8) = 10
            let _: () = msg_send![button, sendActionOn: 10i64];

            // Store state globally for ObjC callbacks
            {
                let guard = GLOBAL_STATE.lock();
                *guard.borrow_mut() = Some(AppState {
                    status_item,
                    popover,
                    rx: self.rx,
                    pulsing: false,
                    pulse_bright: true,
                    pulse_counter: 0,
                    reconnect_tx: Some(self.reconnect_tx),
                    shutdown_tx: Some(self.shutdown_tx),
                });
            }

            // Set up NSTimer to poll message channel every 50ms
            let timer_target = handler;
            let interval: f64 = 0.05;
            let _: id = msg_send![class!(NSTimer),
                scheduledTimerWithTimeInterval: interval
                target: timer_target
                selector: sel!(pollMessages:)
                userInfo: nil
                repeats: true
            ];

            tracing::info!("Menu bar app running");
            let _: () = msg_send![app, run];
        }
    }
}

/// Register a custom ObjC class with click and timer handlers.
fn register_click_handler_class() -> &'static objc::runtime::Class {
    if let Some(cls) = objc::runtime::Class::get("AuraClickHandler") {
        return cls;
    }
    let superclass = class!(NSObject);
    let mut decl = ClassDecl::new("AuraClickHandler", superclass)
        .expect("Failed to create AuraClickHandler class");

    unsafe {
        // Click handler -- left click toggles popover, right click shows menu
        decl.add_method(
            sel!(handleClick:),
            handle_click as extern "C" fn(&Object, Sel, id),
        );

        // Context menu actions
        decl.add_method(
            sel!(menuReconnect:),
            menu_reconnect as extern "C" fn(&Object, Sel, id),
        );
        decl.add_method(
            sel!(menuQuit:),
            menu_quit as extern "C" fn(&Object, Sel, id),
        );

        // Timer handler -- polls tokio channel for messages
        decl.add_method(
            sel!(pollMessages:),
            poll_messages as extern "C" fn(&Object, Sel, id),
        );
    }

    decl.register()
}

extern "C" fn handle_click(_this: &Object, _cmd: Sel, sender: id) {
    let guard = GLOBAL_STATE.lock();
    let borrow = guard.borrow();
    unsafe {
        if let Some(ref state) = *borrow {
            // Check if this is a right-click
            let current_event: id = msg_send![NSApp(), currentEvent];
            let event_type: u64 = msg_send![current_event, type];
            // NSEventTypeRightMouseDown = 3
            if event_type == 3 {
                let raw_item = state.status_item.raw();
                // Drop borrow and lock before calling ObjC methods that run a
                // nested event loop — otherwise poll_messages would deadlock
                // trying to re-borrow an already-borrowed RefCell.
                drop(borrow);
                drop(guard);
                show_context_menu(_this, sender, raw_item);
            } else {
                let button: id = msg_send![state.status_item.raw(), button];
                state.popover.toggle(button);
            }
        }
    }
}

unsafe fn show_context_menu(handler: &Object, _sender: id, raw_item: id) {
    unsafe {
        let menu: id = msg_send![class!(NSMenu), alloc];
        let menu: id = msg_send![menu, init];

        // Title item (disabled)
        let title_str = NSString::alloc(nil).init_str("Aura");
        let empty_sel = sel!(init); // dummy selector
        let title_item: id = msg_send![class!(NSMenuItem), alloc];
        let title_item: id = msg_send![title_item,
            initWithTitle: title_str
            action: empty_sel
            keyEquivalent: NSString::alloc(nil).init_str("")
        ];
        let _: () = msg_send![title_item, setEnabled: false];
        let _: () = msg_send![menu, addItem: title_item];

        // Separator
        let sep: id = msg_send![class!(NSMenuItem), separatorItem];
        let _: () = msg_send![menu, addItem: sep];

        // Reconnect
        let reconnect_str = NSString::alloc(nil).init_str("Reconnect");
        let reconnect_item: id = msg_send![class!(NSMenuItem), alloc];
        let reconnect_item: id = msg_send![reconnect_item,
            initWithTitle: reconnect_str
            action: sel!(menuReconnect:)
            keyEquivalent: NSString::alloc(nil).init_str("")
        ];
        let _: () = msg_send![reconnect_item, setTarget: handler as *const Object as id];
        let _: () = msg_send![menu, addItem: reconnect_item];

        // Quit
        let quit_str = NSString::alloc(nil).init_str("Quit Aura");
        let quit_item: id = msg_send![class!(NSMenuItem), alloc];
        let quit_item: id = msg_send![quit_item,
            initWithTitle: quit_str
            action: sel!(menuQuit:)
            keyEquivalent: NSString::alloc(nil).init_str("q")
        ];
        let _: () = msg_send![quit_item, setTarget: handler as *const Object as id];
        let _: () = msg_send![menu, addItem: quit_item];

        // Show menu at status item button
        let button: id = msg_send![raw_item, button];
        let _: () = msg_send![menu,
            popUpMenuPositioningItem: nil
            atLocation: cocoa::foundation::NSPoint::new(0.0, 0.0)
            inView: button
        ];
    }
}

extern "C" fn menu_reconnect(_this: &Object, _cmd: Sel, _sender: id) {
    let guard = GLOBAL_STATE.lock();
    let borrow = guard.borrow();
    if let Some(ref state) = *borrow
        && let Some(ref tx) = state.reconnect_tx
    {
        let _ = tx.try_send(());
    }
}

extern "C" fn menu_quit(_this: &Object, _cmd: Sel, _sender: id) {
    let guard = GLOBAL_STATE.lock();
    let borrow = guard.borrow();
    if let Some(ref state) = *borrow
        && let Some(ref tx) = state.shutdown_tx
    {
        let _ = tx.try_send(());
    }
}

extern "C" fn poll_messages(_this: &Object, _cmd: Sel, _timer: id) {
    unsafe {
        let pool: id = msg_send![class!(NSAutoreleasePool), new];

        let guard = GLOBAL_STATE.lock();
        let mut borrow = guard.borrow_mut();
        if let Some(ref mut state) = *borrow {
            // Drain all pending messages (non-blocking)
            while let Ok(msg) = state.rx.try_recv() {
                match msg {
                    MenuBarMessage::SetColor(color) => {
                        state.status_item.set_color(color);
                    }
                    MenuBarMessage::AddMessage { text, is_user } => {
                        state.popover.add_message(&text, is_user);
                    }
                    MenuBarMessage::SetStatus { text } => {
                        state.popover.set_status(&text);
                    }
                    MenuBarMessage::SetPulsing(enabled) => {
                        state.pulsing = enabled;
                        if !enabled {
                            // Reset to solid green when pulsing stops
                            state.status_item.set_color(DotColor::Green);
                        }
                    }
                    MenuBarMessage::Reconnect => {
                        if let Some(ref tx) = state.reconnect_tx {
                            let _ = tx.try_send(());
                        }
                    }
                    MenuBarMessage::Shutdown => {
                        let app = NSApp();
                        let _: () = msg_send![app, terminate: nil];
                    }
                }
            }

            // Handle pulsing animation (timer fires every 50ms, toggle every ~500ms = 10 ticks)
            if state.pulsing {
                state.pulse_counter = state.pulse_counter.wrapping_add(1);
                if state.pulse_counter.is_multiple_of(10) {
                    state.pulse_bright = !state.pulse_bright;
                    let color = if state.pulse_bright {
                        DotColor::Green
                    } else {
                        DotColor::GreenDim
                    };
                    state.status_item.set_color(color);
                }
            }
        }

        let _: () = msg_send![pool, drain];
    }
}
