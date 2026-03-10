use std::collections::HashMap;

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
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
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
            );
            CREATE TABLE IF NOT EXISTS settings (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL
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

    pub fn set_setting(&self, key: &str, value: &str) -> Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO settings (key, value) VALUES (?1, ?2)",
            rusqlite::params![key, value],
        )?;
        Ok(())
    }

    pub fn get_setting(&self, key: &str) -> Result<Option<String>> {
        let mut stmt = self
            .conn
            .prepare("SELECT value FROM settings WHERE key = ?1")?;
        let result = stmt.query_row(rusqlite::params![key], |row| row.get(0));
        match result {
            Ok(v) => Ok(Some(v)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// Delete sessions (and their messages) older than `max_age_days` days.
    /// Returns the number of sessions deleted.
    pub fn prune_old_sessions(&self, max_age_days: u32) -> Result<usize> {
        // We delete messages first since the schema has no ON DELETE CASCADE.
        // Use datetime() to normalize RFC 3339 timestamps for comparison.
        let cutoff = format!("-{max_age_days} days");
        self.conn.execute(
            "DELETE FROM messages WHERE session_id IN (
                SELECT id FROM sessions
                WHERE datetime(started_at) < datetime('now', ?1)
            )",
            params![cutoff],
        )?;
        let deleted = self.conn.execute(
            "DELETE FROM sessions WHERE datetime(started_at) < datetime('now', ?1)",
            params![cutoff],
        )?;
        Ok(deleted)
    }

    /// Return a brief summary of the last `max_sessions` sessions, including
    /// timestamps and which tools were used (with counts).
    pub fn get_recent_summary(&self, max_sessions: usize) -> Result<String> {
        let sessions = self.list_sessions(max_sessions)?;
        if sessions.is_empty() {
            return Ok(String::new());
        }

        let mut lines = Vec::new();
        lines.push("Recent history:".to_string());

        for session in &sessions {
            // Collect tool calls for this session
            let mut stmt = self.conn.prepare(
                "SELECT content FROM messages WHERE session_id = ?1 AND role = 'tool_call' ORDER BY id ASC",
            )?;
            let tool_names: Vec<String> = stmt
                .query_map(params![session.id], |row| row.get::<_, String>(0))?
                .filter_map(|r| r.ok())
                .filter_map(|content| {
                    // Content format is "tool_name: {args}" — extract the name
                    content.split(':').next().map(|s| s.trim().to_string())
                })
                .collect();

            if tool_names.is_empty() {
                continue;
            }

            // Count occurrences of each tool
            let mut counts: HashMap<String, usize> = HashMap::new();
            for name in &tool_names {
                *counts.entry(name.clone()).or_insert(0) += 1;
            }

            // Format: "tool_name (Nx)" sorted by count descending
            let mut sorted: Vec<(String, usize)> = counts.into_iter().collect();
            sorted.sort_by(|a, b| b.1.cmp(&a.1));
            let tool_summary: Vec<String> = sorted
                .iter()
                .map(|(name, count)| {
                    if *count > 1 {
                        format!("{name} ({count}x)")
                    } else {
                        name.clone()
                    }
                })
                .collect();

            // Format timestamp — truncate to minutes
            let ts = &session.started_at;
            let display_ts = if ts.len() >= 16 { &ts[..16] } else { ts };

            lines.push(format!(
                "- Session {display_ts}: Used {}",
                tool_summary.join(", ")
            ));
        }

        if lines.len() <= 1 {
            // Only the header, no actual sessions with tools
            return Ok(String::new());
        }

        Ok(lines.join("\n"))
    }

    /// Run VACUUM to reclaim disk space after pruning.
    pub fn vacuum(&self) -> Result<()> {
        self.conn.execute_batch("VACUUM")?;
        Ok(())
    }

    /// Query a PRAGMA value. Useful for inspecting database configuration.
    #[cfg(test)]
    pub(crate) fn pragma_query_value<T: rusqlite::types::FromSql>(
        &self,
        pragma_name: &str,
    ) -> Result<T> {
        let value = self
            .conn
            .pragma_query_value(None, pragma_name, |row| row.get(0))?;
        Ok(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wal_mode_enabled() {
        let dir = tempfile::tempdir().unwrap();
        let mem = SessionMemory::open(&dir.path().join("test.db")).unwrap();
        let mode: String = mem.pragma_query_value("journal_mode").unwrap();
        assert_eq!(mode, "wal");
    }

    #[test]
    fn settings_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let mem = SessionMemory::open(&dir.path().join("test.db")).unwrap();
        mem.set_setting("resumption_handle", "abc123").unwrap();
        assert_eq!(
            mem.get_setting("resumption_handle").unwrap(),
            Some("abc123".into())
        );
    }

    #[test]
    fn settings_missing_key_returns_none() {
        let dir = tempfile::tempdir().unwrap();
        let mem = SessionMemory::open(&dir.path().join("test.db")).unwrap();
        assert_eq!(mem.get_setting("nonexistent").unwrap(), None);
    }

    #[test]
    fn settings_upsert_updates_value() {
        let dir = tempfile::tempdir().unwrap();
        let mem = SessionMemory::open(&dir.path().join("test.db")).unwrap();
        mem.set_setting("handle", "v1").unwrap();
        mem.set_setting("handle", "v2").unwrap();
        assert_eq!(mem.get_setting("handle").unwrap(), Some("v2".into()));
    }

    #[test]
    fn recent_summary_empty_when_no_sessions() {
        let dir = tempfile::tempdir().unwrap();
        let mem = SessionMemory::open(&dir.path().join("test.db")).unwrap();
        let summary = mem.get_recent_summary(3).unwrap();
        assert!(summary.is_empty());
    }

    #[test]
    fn recent_summary_includes_tool_counts() {
        let dir = tempfile::tempdir().unwrap();
        let mem = SessionMemory::open(&dir.path().join("test.db")).unwrap();

        let sid = mem.start_session().unwrap();
        mem.add_message(&sid, MessageRole::ToolCall, "click: {}", None)
            .unwrap();
        mem.add_message(&sid, MessageRole::ToolCall, "click: {}", None)
            .unwrap();
        mem.add_message(&sid, MessageRole::ToolCall, "type_text: {}", None)
            .unwrap();
        mem.end_session(&sid, None).unwrap();

        let summary = mem.get_recent_summary(3).unwrap();
        assert!(summary.contains("Recent history:"));
        assert!(summary.contains("click (2x)"));
        assert!(summary.contains("type_text"));
    }

    #[test]
    fn recent_summary_skips_sessions_without_tools() {
        let dir = tempfile::tempdir().unwrap();
        let mem = SessionMemory::open(&dir.path().join("test.db")).unwrap();

        let sid = mem.start_session().unwrap();
        mem.add_message(&sid, MessageRole::User, "hello", None)
            .unwrap();
        mem.end_session(&sid, None).unwrap();

        let summary = mem.get_recent_summary(3).unwrap();
        assert!(summary.is_empty());
    }
}
