use aura_llm::intent::Intent;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AuraEvent {
    // Voice pipeline
    WakeWordDetected,
    ListeningStarted,
    ListeningStopped,
    VoiceCommand { text: String },

    // Intent
    IntentParsed { intent: Intent },

    // Actions
    ActionExecuted { description: String },
    ActionFailed { description: String, error: String },

    // Conversation
    AssistantSpeaking { text: String },
    BargeIn,

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
