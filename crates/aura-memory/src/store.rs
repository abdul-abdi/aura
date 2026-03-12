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

#[derive(Debug, Clone, Serialize)]
pub struct Fact {
    pub id: i64,
    pub session_id: String,
    pub category: String,
    pub content: String,
    pub entities: Option<String>,
    pub importance: f64,
    pub created_at: String,
}

pub struct SessionMemory {
    pub(crate) conn: Connection,
}

impl SessionMemory {
    pub fn open(path: &std::path::Path) -> Result<Self> {
        let conn = Connection::open(path)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;

        // Check whether the FTS virtual table already exists before running DDL,
        // so we can decide whether a one-time backfill rebuild is needed.
        let fts_existed: bool = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='facts_fts'",
                [],
                |row| row.get::<_, i64>(0),
            )
            .unwrap_or(0)
            > 0;

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
            );
            CREATE TABLE IF NOT EXISTS facts (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                session_id TEXT NOT NULL REFERENCES sessions(id),
                category TEXT NOT NULL,
                content TEXT NOT NULL,
                entities TEXT,
                importance REAL NOT NULL DEFAULT 0.5,
                created_at TEXT NOT NULL
            );
            CREATE VIRTUAL TABLE IF NOT EXISTS facts_fts USING fts5(
                content, category, entities,
                content='facts',
                content_rowid='id',
                tokenize='porter unicode61'
            );
            CREATE TRIGGER IF NOT EXISTS facts_ai AFTER INSERT ON facts BEGIN
                INSERT INTO facts_fts(rowid, content, category, entities)
                VALUES (new.id, new.content, new.category, COALESCE(new.entities, ''));
            END;
            CREATE TRIGGER IF NOT EXISTS facts_ad AFTER DELETE ON facts BEGIN
                INSERT INTO facts_fts(facts_fts, rowid, content, category, entities)
                VALUES ('delete', old.id, old.content, old.category, COALESCE(old.entities, ''));
            END;
            CREATE TRIGGER IF NOT EXISTS facts_au AFTER UPDATE ON facts BEGIN
                INSERT INTO facts_fts(facts_fts, rowid, content, category, entities)
                VALUES ('delete', old.id, old.content, old.category, COALESCE(old.entities, ''));
                INSERT INTO facts_fts(rowid, content, category, entities)
                VALUES (new.id, new.content, new.category, COALESCE(new.entities, ''));
            END;",
        )?;

        // Backfill the FTS index from existing facts when the virtual table was
        // just created for the first time (i.e. upgrading an existing database).
        if !fts_existed {
            conn.execute_batch(
                "INSERT INTO facts_fts(facts_fts) VALUES('rebuild');",
            )?;
        }
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
            "UPDATE sessions SET ended_at = COALESCE(ended_at, ?1), summary = COALESCE(?2, summary) WHERE id = ?3",
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

    pub fn delete_setting(&self, key: &str) -> Result<()> {
        self.conn.execute(
            "DELETE FROM settings WHERE key = ?1",
            rusqlite::params![key],
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

    /// Delete sessions (and their messages and facts) older than `max_age_days` days.
    /// Returns the number of sessions deleted.
    pub fn prune_old_sessions(&self, max_age_days: u32) -> Result<usize> {
        let tx = self.conn.unchecked_transaction()?;
        let cutoff = format!("-{max_age_days} days");
        tx.execute(
            "DELETE FROM facts WHERE session_id IN (
                SELECT id FROM sessions
                WHERE datetime(started_at) < datetime('now', ?1)
            )",
            params![cutoff],
        )?;
        tx.execute(
            "DELETE FROM messages WHERE session_id IN (
                SELECT id FROM sessions
                WHERE datetime(started_at) < datetime('now', ?1)
            )",
            params![cutoff],
        )?;
        let deleted = tx.execute(
            "DELETE FROM sessions WHERE datetime(started_at) < datetime('now', ?1)",
            params![cutoff],
        )?;
        tx.commit()?;
        Ok(deleted)
    }

    /// Return a brief summary of the last `max_sessions` sessions, including
    /// timestamps and which tools were used (with counts).
    pub fn get_recent_summary(&self, max_sessions: usize) -> Result<String> {
        let sessions = self.list_sessions(max_sessions)?;
        if sessions.is_empty() {
            return Ok(String::new());
        }

        // Batch query: get all tool calls for these sessions in one query
        let placeholders: Vec<String> = (1..=sessions.len()).map(|i| format!("?{i}")).collect();
        let sql = format!(
            "SELECT session_id, content FROM messages WHERE role = 'tool_call' AND session_id IN ({}) ORDER BY id ASC",
            placeholders.join(", ")
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let params: Vec<&dyn rusqlite::types::ToSql> = sessions
            .iter()
            .map(|s| &s.id as &dyn rusqlite::types::ToSql)
            .collect();
        let rows = stmt.query_map(params.as_slice(), |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;

        // Group tool calls by session
        let mut session_tools: HashMap<String, Vec<String>> = HashMap::new();
        for row in rows {
            let (session_id, content) = row?;
            if let Some(name) = content.split(':').next().map(|s| s.trim().to_string()) {
                session_tools.entry(session_id).or_default().push(name);
            }
        }

        let mut lines = Vec::new();
        lines.push("Recent history:".to_string());

        for session in &sessions {
            let Some(tool_names) = session_tools.get(&session.id) else {
                continue;
            };
            if tool_names.is_empty() {
                continue;
            }

            let mut counts: HashMap<String, usize> = HashMap::new();
            for name in tool_names {
                *counts.entry(name.clone()).or_insert(0) += 1;
            }

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

            let ts = &session.started_at;
            let display_ts = if ts.len() >= 16 { &ts[..16] } else { ts };

            lines.push(format!(
                "- Session {display_ts}: Used {}",
                tool_summary.join(", ")
            ));
        }

        if lines.len() <= 1 {
            return Ok(String::new());
        }

        Ok(lines.join("\n"))
    }

    /// Run VACUUM to reclaim disk space after pruning.
    pub fn vacuum(&self) -> Result<()> {
        self.conn.execute_batch("VACUUM")?;
        Ok(())
    }

    pub fn add_fact(
        &self,
        session_id: &str,
        category: &str,
        content: &str,
        entities: Option<&str>,
        importance: f64,
    ) -> Result<()> {
        let now = chrono::Utc::now().to_rfc3339();
        self.conn.execute(
            "INSERT INTO facts (session_id, category, content, entities, importance, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![session_id, category, content, entities, importance, now],
        )?;
        Ok(())
    }

    pub fn get_facts_for_session(&self, session_id: &str) -> Result<Vec<Fact>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, session_id, category, content, entities, importance, created_at
             FROM facts WHERE session_id = ?1 ORDER BY id ASC",
        )?;
        let facts = stmt
            .query_map(params![session_id], |row| {
                Ok(Fact {
                    id: row.get(0)?,
                    session_id: row.get(1)?,
                    category: row.get(2)?,
                    content: row.get(3)?,
                    entities: row.get(4)?,
                    importance: row.get(5)?,
                    created_at: row.get(6)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(facts)
    }

    pub fn search_memory(&self, query: &str) -> Result<Vec<Fact>> {
        // Use FTS5 for queries of 2+ characters; fall back to LIKE for single chars.
        if query.len() >= 2 {
            let fts_query = query
                .split_whitespace()
                .map(|w| format!("\"{}\"", w.replace('"', "")))
                .collect::<Vec<_>>()
                .join(" OR ");

            let mut stmt = self.conn.prepare(
                "SELECT f.id, f.session_id, f.category, f.content, f.entities,
                        f.importance, f.created_at
                 FROM facts f
                 JOIN facts_fts ON f.id = facts_fts.rowid
                 WHERE facts_fts MATCH ?1
                 ORDER BY rank
                 LIMIT 10",
            )?;
            let facts = stmt
                .query_map(params![fts_query], |row| {
                    Ok(Fact {
                        id: row.get(0)?,
                        session_id: row.get(1)?,
                        category: row.get(2)?,
                        content: row.get(3)?,
                        entities: row.get(4)?,
                        importance: row.get(5)?,
                        created_at: row.get(6)?,
                    })
                })?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(facts)
        } else {
            let pattern = format!("%{query}%");
            let mut stmt = self.conn.prepare(
                "SELECT id, session_id, category, content, entities, importance, created_at
                 FROM facts
                 WHERE content LIKE ?1 OR entities LIKE ?1
                 ORDER BY importance DESC, created_at DESC
                 LIMIT 10",
            )?;
            let facts = stmt
                .query_map(params![pattern], |row| {
                    Ok(Fact {
                        id: row.get(0)?,
                        session_id: row.get(1)?,
                        category: row.get(2)?,
                        content: row.get(3)?,
                        entities: row.get(4)?,
                        importance: row.get(5)?,
                        created_at: row.get(6)?,
                    })
                })?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(facts)
        }
    }

    pub fn search_memory_with_sessions(&self, query: &str) -> Result<serde_json::Value> {
        let facts = self.search_memory(query)?;
        let pattern = format!("%{query}%");
        let mut stmt = self.conn.prepare(
            "SELECT id, started_at, ended_at, summary FROM sessions
             WHERE summary LIKE ?1
             ORDER BY started_at DESC LIMIT 5",
        )?;
        let sessions = stmt
            .query_map(params![pattern], |row| {
                Ok(Session {
                    id: row.get(0)?,
                    started_at: row.get(1)?,
                    ended_at: row.get(2)?,
                    summary: row.get(3)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(serde_json::json!({
            "facts": facts,
            "sessions": sessions,
        }))
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

    #[test]
    fn facts_table_exists() {
        let dir = tempfile::tempdir().unwrap();
        let mem = SessionMemory::open(&dir.path().join("test.db")).unwrap();
        let sid = mem.start_session().unwrap();
        mem.add_fact(
            &sid,
            "preference",
            "User prefers dark mode",
            Some(r#"["dark mode"]"#),
            0.8,
        )
        .unwrap();
        let facts = mem.get_facts_for_session(&sid).unwrap();
        assert_eq!(facts.len(), 1);
        assert_eq!(facts[0].category, "preference");
        assert_eq!(facts[0].content, "User prefers dark mode");
        assert!((facts[0].importance - 0.8).abs() < 0.01);
    }

    #[test]
    fn search_facts_by_query() {
        let dir = tempfile::tempdir().unwrap();
        let mem = SessionMemory::open(&dir.path().join("test.db")).unwrap();
        let sid = mem.start_session().unwrap();
        mem.add_fact(
            &sid,
            "preference",
            "User prefers dark mode in VS Code",
            Some(r#"["VS Code","dark mode"]"#),
            0.8,
        )
        .unwrap();
        mem.add_fact(
            &sid,
            "entity",
            "User edited report.pdf in Pages",
            Some(r#"["report.pdf","Pages"]"#),
            0.6,
        )
        .unwrap();
        mem.end_session(&sid, Some("Edited documents")).unwrap();

        let results = mem.search_memory("dark mode").unwrap();
        assert_eq!(results.len(), 1);
        assert!(results[0].content.contains("dark mode"));
    }

    #[test]
    fn end_session_preserves_existing_summary() {
        let dir = tempfile::tempdir().unwrap();
        let mem = SessionMemory::open(&dir.path().join("test.db")).unwrap();
        let sid = mem.start_session().unwrap();

        mem.end_session(&sid, Some("Important summary")).unwrap();
        mem.end_session(&sid, None).unwrap();

        let sessions = mem.list_sessions(1).unwrap();
        assert_eq!(sessions[0].summary.as_deref(), Some("Important summary"));
    }

    #[test]
    fn prune_deletes_facts_for_old_sessions() {
        let dir = tempfile::tempdir().unwrap();
        let mem = SessionMemory::open(&dir.path().join("test.db")).unwrap();

        let sid = mem.start_session().unwrap();
        mem.add_fact(&sid, "preference", "likes dark mode", None, 0.8)
            .unwrap();
        mem.end_session(&sid, Some("test session")).unwrap();

        // Backdate session to 100 days ago
        mem.conn
            .execute(
                "UPDATE sessions SET started_at = datetime('now', '-100 days') WHERE id = ?1",
                rusqlite::params![sid],
            )
            .unwrap();

        let deleted = mem.prune_old_sessions(30).unwrap();
        assert_eq!(deleted, 1);

        let facts = mem.get_facts_for_session(&sid).unwrap();
        assert!(facts.is_empty());
    }
}
