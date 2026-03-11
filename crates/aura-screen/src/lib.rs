//! Aura screen context: accessibility API integration

#[cfg(target_os = "macos")]
pub mod accessibility;
pub mod capture;
pub mod context;
#[cfg(target_os = "macos")]
pub mod macos;
