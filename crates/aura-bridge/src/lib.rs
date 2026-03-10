//! Aura OS bridge: platform-specific system actions

pub mod actions;
pub mod script;

#[cfg(target_os = "macos")]
pub mod macos;
