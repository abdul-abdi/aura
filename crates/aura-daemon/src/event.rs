use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AuraEvent {
    // Voice
    WakeWordDetected,
    ListeningStarted,

    // Gemini session
    GeminiConnected,
    GeminiReconnecting {
        attempt: u32,
    },

    // Conversation
    AssistantSpeaking {
        text: String,
    },
    UserTranscription {
        text: String,
    },
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
