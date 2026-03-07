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

    // Overlay
    ShowOverlay { content: OverlayContent },
    HideOverlay,

    // System
    Shutdown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Intent {
    OpenApp { name: String },
    SearchFiles { query: String },
    TileWindows { layout: String },
    SummarizeScreen,
    LaunchUrl { url: String },
    Unknown { raw: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum OverlayContent {
    Listening,
    Processing,
    Response { text: String },
    Error { message: String },
}
