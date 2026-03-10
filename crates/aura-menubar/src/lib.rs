//! Aura menu bar: native macOS status item and popover UI
//!
//! Uses the `cocoa` crate (deprecated in favor of `objc2`) for Cocoa FFI.
#![allow(deprecated)]

pub mod app;
pub mod popover;
pub mod status_item;
