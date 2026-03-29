use serde::{Deserialize, Serialize};

/// Events sent from the daemon to connected UI clients over IPC.
///
/// Each event is serialized as a single JSON line (JSONL) terminated by `\n`.
/// Clients subscribe to a broadcast channel and receive all events — they can
/// filter by variant on the Swift side.
///
/// Uses internally-tagged encoding (`#[serde(tag = "type")]`) so the JSON is
/// flat — no `"data"` wrapper. Variant names are snake_case to match Swift
/// `Decodable` expectations.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum DaemonEvent {
    /// Dot color update for status bar icon.
    DotColor { color: DotColorName, pulsing: bool },

    /// A transcript line from the user or the assistant.
    Transcript {
        role: Role,
        text: String,
        done: bool,
        #[serde(default = "default_source")]
        source: String,
    },

    /// Tool execution status.
    ToolStatus {
        name: String,
        status: ToolRunStatus,
        #[serde(skip_serializing_if = "Option::is_none")]
        output: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        summary: Option<String>,
    },

    /// Status message update.
    Status { message: String },

    /// Recent session summaries for the UI empty state.
    RecentSessions { sessions: Vec<RecentSessionInfo> },

    /// The daemon is shutting down.
    Shutdown,

    /// Base64-encoded 24kHz 16-bit PCM audio for playback.
    PlaybackAudio { audio: String },

    /// Gemini started an audio response — prepare playback.
    PlaybackStart,

    /// Gemini finished the audio response — flush playback.
    PlaybackStop,

    /// Gemini was interrupted (barge-in) — stop playback immediately.
    Interrupted,

    /// Autonomy mode changed.
    ModeChanged { mode: String },

    /// A proactive intervention from Gemini.
    ProactiveIntervention { text: String, intervention_id: String },
}

/// Dot color names sent over IPC — maps to the Swift `DotColorName` enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DotColorName {
    Gray,
    Green,
    Amber,
    Red,
}

/// Tool execution lifecycle status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolRunStatus {
    Running,
    Completed,
    Failed,
}

/// Conversation participant role.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    User,
    Assistant,
}

/// Commands sent from UI clients to the daemon over IPC.
///
/// Each command is a single JSON line terminated by `\n`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum UICommand {
    /// Send a text message to the Gemini session (text input mode).
    SendText { text: String },

    /// Toggle microphone on/off.
    ToggleMic,

    /// Request reconnection to Gemini.
    Reconnect,

    /// Request a graceful shutdown.
    Shutdown,

    /// Audio frame from the Swift mic capture (base64-encoded 16kHz PCM).
    AudioFrame { pcm: String },

    /// Set the autonomy mode.
    SetMode { mode: String },

    /// Dismiss a proactive intervention.
    DismissIntervention { intervention_id: String },
}

/// A recent session summary for display in the UI.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecentSessionInfo {
    pub session_id: String,
    pub summary: String,
    pub created_at: String,
}

fn default_source() -> String {
    "voice".into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transcript_serialization() {
        let event = DaemonEvent::Transcript {
            role: Role::Assistant,
            text: "Hello".into(),
            done: false,
            source: "voice".into(),
        };
        let json = serde_json::to_string(&event).unwrap();
        assert_eq!(
            json,
            r#"{"type":"transcript","role":"assistant","text":"Hello","done":false,"source":"voice"}"#
        );

        // Roundtrip
        let parsed: DaemonEvent = serde_json::from_str(&json).unwrap();
        match parsed {
            DaemonEvent::Transcript {
                role, text, done, ..
            } => {
                assert_eq!(role, Role::Assistant);
                assert_eq!(text, "Hello");
                assert!(!done);
            }
            _ => panic!("unexpected variant"),
        }
    }

    #[test]
    fn dot_color_serialization() {
        let event = DaemonEvent::DotColor {
            color: DotColorName::Green,
            pulsing: true,
        };
        let json = serde_json::to_string(&event).unwrap();
        assert_eq!(
            json,
            r#"{"type":"dot_color","color":"green","pulsing":true}"#
        );
    }

    #[test]
    fn tool_status_serialization() {
        let event = DaemonEvent::ToolStatus {
            name: "click".into(),
            status: ToolRunStatus::Running,
            output: None,
            summary: None,
        };
        let json = serde_json::to_string(&event).unwrap();
        assert_eq!(
            json,
            r#"{"type":"tool_status","name":"click","status":"running"}"#
        );
    }

    #[test]
    fn tool_status_with_output_serialization() {
        let event = DaemonEvent::ToolStatus {
            name: "click".into(),
            status: ToolRunStatus::Completed,
            output: Some("done".into()),
            summary: None,
        };
        let json = serde_json::to_string(&event).unwrap();
        assert_eq!(
            json,
            r#"{"type":"tool_status","name":"click","status":"completed","output":"done"}"#
        );
    }

    #[test]
    fn send_text_serialization() {
        let cmd = UICommand::SendText {
            text: "hello".into(),
        };
        let json = serde_json::to_string(&cmd).unwrap();
        assert_eq!(json, r#"{"type":"send_text","text":"hello"}"#);

        // Roundtrip
        let parsed: UICommand = serde_json::from_str(&json).unwrap();
        match parsed {
            UICommand::SendText { text } => assert_eq!(text, "hello"),
            _ => panic!("unexpected variant"),
        }
    }

    #[test]
    fn reconnect_serialization() {
        let cmd = UICommand::Reconnect;
        let json = serde_json::to_string(&cmd).unwrap();
        assert_eq!(json, r#"{"type":"reconnect"}"#);
    }

    #[test]
    fn shutdown_serialization() {
        let event = DaemonEvent::Shutdown;
        let json = serde_json::to_string(&event).unwrap();
        assert_eq!(json, r#"{"type":"shutdown"}"#);
    }

    #[test]
    fn status_serialization() {
        let event = DaemonEvent::Status {
            message: "Connected".into(),
        };
        let json = serde_json::to_string(&event).unwrap();
        assert_eq!(json, r#"{"type":"status","message":"Connected"}"#);
    }

    #[test]
    fn transcript_with_source_serialization() {
        let event = DaemonEvent::Transcript {
            role: Role::User,
            text: "hello".into(),
            done: true,
            source: "text".into(),
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains(r#""source":"text""#));
    }

    #[test]
    fn playback_audio_serialization() {
        let event = DaemonEvent::PlaybackAudio {
            audio: "AQID".into(),
        };
        let json = serde_json::to_string(&event).unwrap();
        assert_eq!(json, r#"{"type":"playback_audio","audio":"AQID"}"#);
    }

    #[test]
    fn playback_start_serialization() {
        let event = DaemonEvent::PlaybackStart;
        let json = serde_json::to_string(&event).unwrap();
        assert_eq!(json, r#"{"type":"playback_start"}"#);
    }

    #[test]
    fn playback_stop_serialization() {
        let event = DaemonEvent::PlaybackStop;
        let json = serde_json::to_string(&event).unwrap();
        assert_eq!(json, r#"{"type":"playback_stop"}"#);
    }

    #[test]
    fn interrupted_serialization() {
        let event = DaemonEvent::Interrupted;
        let json = serde_json::to_string(&event).unwrap();
        assert_eq!(json, r#"{"type":"interrupted"}"#);
    }

    #[test]
    fn mode_changed_serialization() {
        let event = DaemonEvent::ModeChanged {
            mode: "copilot".into(),
        };
        let json = serde_json::to_string(&event).unwrap();
        assert_eq!(json, r#"{"type":"mode_changed","mode":"copilot"}"#);
    }

    #[test]
    fn proactive_intervention_serialization() {
        let event = DaemonEvent::ProactiveIntervention {
            text: "You have a meeting in 5 minutes".into(),
            intervention_id: "int_123".into(),
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains(r#""type":"proactive_intervention""#));
        assert!(json.contains(r#""intervention_id":"int_123""#));
    }

    #[test]
    fn audio_frame_deserialization() {
        let json = r#"{"type":"audio_frame","pcm":"AQID"}"#;
        let cmd: UICommand = serde_json::from_str(json).unwrap();
        match cmd {
            UICommand::AudioFrame { pcm } => assert_eq!(pcm, "AQID"),
            _ => panic!("unexpected variant"),
        }
    }

    #[test]
    fn set_mode_deserialization() {
        let json = r#"{"type":"set_mode","mode":"ghost"}"#;
        let cmd: UICommand = serde_json::from_str(json).unwrap();
        match cmd {
            UICommand::SetMode { mode } => assert_eq!(mode, "ghost"),
            _ => panic!("unexpected variant"),
        }
    }

    #[test]
    fn dismiss_intervention_deserialization() {
        let json = r#"{"type":"dismiss_intervention","intervention_id":"int_456"}"#;
        let cmd: UICommand = serde_json::from_str(json).unwrap();
        match cmd {
            UICommand::DismissIntervention { intervention_id } => assert_eq!(intervention_id, "int_456"),
            _ => panic!("unexpected variant"),
        }
    }
}
