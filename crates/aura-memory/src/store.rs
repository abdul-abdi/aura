use anyhow::Result;
use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageRole {
    User,
    Assistant,
    ToolCall,
    ToolResult,
}

impl MessageRole {
    fn as_str(&self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Assistant => "assistant",
            Self::ToolCall => "tool_call",
            Self::ToolResult => "tool_result",
        }
    }

    fn from_str(s: &str) -> Self {
        match s {
            "assistant" => Self::Assistant,
            "tool_call" => Self::ToolCall,
            "tool_result" => Self::ToolResult,
            _ => Self::User,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Session {
    pub id: String,
    pub started_at: String,
    pub ended_at: Option<String>,
    pub summary: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Message {
    pub id: i64,
    pub session_id: String,
    pub role: MessageRole,
    pub content: String,
    pub timestamp: String,
    pub metadata: Option<String>,
}

pub struct SessionMemory {
    conn: Connection,
}

impl SessionMemory {
    pub fn open(path: &std::path::Path) -> Result<Self> {
        let conn = Connection::open(path)?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS sessions (
                id TEXT PRIMARY KEY,
                started_at TEXT NOT NULL,
                ended_at TEXT,
                summary TEXT
            );
            CREATE TABLE IF NOT EXISTS messages (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                session_id TEXT NOT NULL REFERENCES sessions(id),
                role TEXT NOT NULL,
                content TEXT NOT NULL,
                timestamp TEXT NOT NULL,
                metadata TEXT
            );",
        )?;
        Ok(Self { conn })
    }

    pub fn start_session(&self) -> Result<String> {
        let id = uuid::Uuid::new_v4().to_string();
        let now = chrono::Utc::now().to_rfc3339();
        self.conn.execute(
            "INSERT INTO sessions (id, started_at) VALUES (?1, ?2)",
            params![id, now],
        )?;
        Ok(id)
    }

    pub fn end_session(&self, session_id: &str, summary: Option<&str>) -> Result<()> {
        let now = chrono::Utc::now().to_rfc3339();
        self.conn.execute(
            "UPDATE sessions SET ended_at = ?1, summary = ?2 WHERE id = ?3",
            params![now, summary, session_id],
        )?;
        Ok(())
    }

    pub fn add_message(
        &self,
        session_id: &str,
        role: MessageRole,
        content: &str,
        metadata: Option<&str>,
    ) -> Result<()> {
        let now = chrono::Utc::now().to_rfc3339();
        self.conn.execute(
            "INSERT INTO messages (session_id, role, content, timestamp, metadata)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![session_id, role.as_str(), content, now, metadata],
        )?;
        Ok(())
    }

    pub fn get_messages(&self, session_id: &str) -> Result<Vec<Message>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, session_id, role, content, timestamp, metadata
             FROM messages WHERE session_id = ?1 ORDER BY id ASC",
        )?;
        let messages = stmt
            .query_map(params![session_id], |row| {
                Ok(Message {
                    id: row.get(0)?,
                    session_id: row.get(1)?,
                    role: MessageRole::from_str(&row.get::<_, String>(2)?),
                    content: row.get(3)?,
                    timestamp: row.get(4)?,
                    metadata: row.get(5)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(messages)
    }

    pub fn list_sessions(&self, limit: usize) -> Result<Vec<Session>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, started_at, ended_at, summary
             FROM sessions ORDER BY started_at DESC LIMIT ?1",
        )?;
        let sessions = stmt
            .query_map(params![limit], |row| {
                Ok(Session {
                    id: row.get(0)?,
                    started_at: row.get(1)?,
                    ended_at: row.get(2)?,
                    summary: row.get(3)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(sessions)
    }
}
