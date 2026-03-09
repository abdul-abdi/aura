//! Aura OS bridge: platform-specific system actions

pub mod actions;

#[cfg(target_os = "macos")]
pub mod macos;
