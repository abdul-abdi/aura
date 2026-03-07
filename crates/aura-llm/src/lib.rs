//! Aura LLM: local language model interface for intent parsing

pub mod intent;
#[cfg(feature = "llamacpp")]
pub mod llamacpp;
pub mod provider;
