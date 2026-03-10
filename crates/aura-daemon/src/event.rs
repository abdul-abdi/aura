use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AuraEvent {
    // Voice
    WakeWordDetected,

    // Gemini session
    GeminiConnected,
    GeminiReconnecting {
        attempt: u32,
    },

    // Conversation
    BargeIn,

    // Tool execution
    ToolExecuted {
        name: String,
        success: bool,
        output: String,
    },

    // System
    Shutdown,
}
