//! Aura screen context: accessibility API integration

pub mod capture;
pub mod context;
#[cfg(target_os = "macos")]
pub mod macos;
