use serde::{Deserialize, Serialize};

/// Events sent from the daemon to connected UI clients over IPC.
///
/// Each event is serialized as a single JSON line (JSONL) terminated by `\n`.
/// Clients subscribe to a broadcast channel and receive all events — they can
/// filter by variant on the Swift side.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum DaemonEvent {
    /// Connection status changed.
    ConnectionState {
        state: ConnectionState,
        message: String,
    },

    /// A transcript line from the user or the assistant.
    Transcript { role: Role, text: String },

    /// A tool call started executing.
    ToolStarted { name: String, id: String },

    /// A tool call finished.
    ToolFinished {
        name: String,
        id: String,
        success: bool,
    },

    /// The daemon is shutting down.
    Shutdown,
}

/// Connection states exposed to the UI.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConnectionState {
    Connecting,
    Connected,
    Reconnecting,
    Disconnected,
    Error,
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
#[serde(tag = "type", content = "data")]
pub enum UICommand {
    /// Send a text message to the Gemini session (text input mode).
    SendText { text: String },

    /// Request a graceful shutdown.
    Shutdown,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn daemon_event_roundtrip() {
        let event = DaemonEvent::Transcript {
            role: Role::Assistant,
            text: "Hello, world!".into(),
        };
        let json = serde_json::to_string(&event).unwrap();
        let parsed: DaemonEvent = serde_json::from_str(&json).unwrap();
        match parsed {
            DaemonEvent::Transcript { role, text } => {
                assert_eq!(role, Role::Assistant);
                assert_eq!(text, "Hello, world!");
            }
            _ => panic!("unexpected variant"),
        }
    }

    #[test]
    fn ui_command_roundtrip() {
        let cmd = UICommand::SendText {
            text: "test".into(),
        };
        let json = serde_json::to_string(&cmd).unwrap();
        let parsed: UICommand = serde_json::from_str(&json).unwrap();
        match parsed {
            UICommand::SendText { text } => assert_eq!(text, "test"),
            _ => panic!("unexpected variant"),
        }
    }

    #[test]
    fn connection_state_roundtrip() {
        let event = DaemonEvent::ConnectionState {
            state: ConnectionState::Connected,
            message: "Ready".into(),
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("\"connected\""));
        let parsed: DaemonEvent = serde_json::from_str(&json).unwrap();
        match parsed {
            DaemonEvent::ConnectionState { state, message } => {
                assert_eq!(state, ConnectionState::Connected);
                assert_eq!(message, "Ready");
            }
            _ => panic!("unexpected variant"),
        }
    }
}
