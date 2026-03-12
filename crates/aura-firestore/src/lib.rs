//! Firestore REST client for Aura's cloud memory persistence.

pub mod auth;
pub mod client;

pub use auth::AuthCache;
pub use client::validate_device_id;
