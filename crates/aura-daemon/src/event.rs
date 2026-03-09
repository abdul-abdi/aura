use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AuraEvent {
    // Voice pipeline
    WakeWordDetected,
    ListeningStarted,
    ListeningStopped,
    VoiceCommand { text: String },

    // Actions
    ActionExecuted { description: String },
    ActionFailed { description: String, error: String },

    // Conversation
    AssistantSpeaking { text: String },
    BargeIn,

    // Gemini session
    GeminiConnected,
    GeminiReconnecting { attempt: u32 },

    // Overlay
    ShowOverlay { content: OverlayContent },
    HideOverlay,

    // System
    Shutdown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum OverlayContent {
    Listening,
    Processing,
    Response { text: String },
    Error { message: String },
}
