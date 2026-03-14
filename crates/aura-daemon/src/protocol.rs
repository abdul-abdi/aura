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
}
