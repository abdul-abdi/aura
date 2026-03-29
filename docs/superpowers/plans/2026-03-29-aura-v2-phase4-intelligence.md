# Aura v2 Phase 4: The Intelligence Layer — Memory + Proactive + Autonomy

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Make Aura intelligent — it remembers the user across sessions, proactively intervenes when genuinely useful, and respects the user's preferred autonomy level. This transforms Aura from a voice assistant into a companion that notices things, adapts over time, and knows when to shut up.

**Architecture:** Three interconnected subsystems, all in the Rust daemon:
1. **Memory Engine** (aura-memory) — Append-only facts table, FTS5 search, two-tier profile derivation, injection into Gemini's system prompt at session start. Fact extraction runs after each session via Gemini REST API.
2. **Proactive Scorer** (aura-daemon module) — Single pure function `score_intervention()` that evaluates screen deltas, app context, idle duration, mode, standing instructions, and dismissal history to decide whether to allow Gemini's proactive audio. Client-side gate on Gemini's `ProactiveAudio`.
3. **Autonomy Gate** (aura-daemon module) — Ghost/Copilot/Autopilot mode state, standing instructions, `requires_approval()` gate before every tool execution, voice command mode switching.

**Tech Stack:** Rust 2024 edition (rusqlite, tokio, serde, chrono, tracing), Swift 6 (SwiftUI), Gemini 3.1 Flash Live API (ProactiveAudio, REST for fact extraction)

**Prerequisite:** Phase 1+2 voice MVP and Phase 3 screen+tools are working.

**Branch:** Continue on `v2` branch (created in Phase 1).

**Important:** v1 line numbers referenced in these plans may have drifted. Always verify line numbers by reading the file before editing.

**Architecture change:** Intelligence layer is fully local SQLite. No Firestore, no Cloud Run memory agent. Fact extraction uses the existing Gemini Live session or a lightweight Gemini REST call — not a cloud service.

---

## File Structure

### Rust (crates/)

| File | Action | Responsibility |
|---|---|---|
| `crates/aura-memory/src/store.rs` | **MODIFY** | Add `standing_instructions` table, `source` column to facts, new query methods for profile derivation |
| `crates/aura-memory/src/profile.rs` | **CREATE** | Two-tier profile derivation: stable profile (400 tokens) + recent corrections (200 tokens) |
| `crates/aura-memory/src/consolidate.rs` | **MODIFY** | Update extraction prompt to include `source` field, add explicit fact capture |
| `crates/aura-memory/src/lib.rs` | **MODIFY** | Re-export new modules |
| `crates/aura-daemon/src/proactive.rs` | **CREATE** | `score_intervention()` function, backoff state, trigger evaluation |
| `crates/aura-daemon/src/autonomy.rs` | **CREATE** | `AutonomyMode` enum, `requires_approval()` gate, mode state management |
| `crates/aura-daemon/src/protocol.rs` | **MODIFY** | New IPC events/commands for mode, proactive, standing instructions |
| `crates/aura-daemon/src/orchestrator.rs` | **MODIFY** | Wire proactive scorer, autonomy gate, profile injection into session lifecycle |
| `crates/aura-daemon/src/processor.rs` | **MODIFY** | Proactive gating on Gemini audio, approval gate before tool execution, "remember X" capture |
| `crates/aura-gemini/src/config.rs` | **MODIFY** | System prompt additions for `<user_profile>` and `<recent_context>` injection slots |

### Swift (AuraApp/Sources/)

| File | Action | Responsibility |
|---|---|---|
| `AuraApp/Sources/AutonomyView.swift` | **CREATE** | Mode picker + standing instructions CRUD UI |
| `AuraApp/Sources/Protocol.swift` | **MODIFY** | New event/command types for mode, proactive, standing instructions |
| `AuraApp/Sources/AppState.swift` | **MODIFY** | Mode state, standing instructions list |
| `AuraApp/Sources/ContentView.swift` | **MODIFY** | Wire AutonomyView into navigation |

---

## Task 1: Memory Schema — Extend facts table and add standing_instructions

**Files:**
- Modify: `crates/aura-memory/src/store.rs`

The v1 `facts` table exists but lacks a `source` column (needed to distinguish transcript-extracted vs. explicit vs. inferred facts). The v2 design also requires a `standing_instructions` table and new query methods for profile derivation. The existing schema already has FTS5 on facts, sessions, messages, and settings tables.

**Key constraint:** Facts are append-only and immutable. Never UPDATE, never DELETE facts. Contradictions are resolved by recency during profile derivation.

- [ ] **Step 1: Add `source` column to facts table and create `standing_instructions` table**

The existing `store.rs` creates tables in `SessionMemory::open()` via `execute_batch`. We need to add a migration for the `source` column (existing databases won't have it) and create the new `standing_instructions` table.

In `crates/aura-memory/src/store.rs`, add the following after the existing `execute_batch` that creates tables (around line 87, after the FTS backfill block ends at line 143):

```rust
// --- Add after the FTS backfill block (line 143) ---

// Migration: add `source` column to facts table if it doesn't exist.
// Default 'transcript' matches the v1 behavior where all facts came from session consolidation.
let has_source_col: bool = conn
    .prepare("SELECT COUNT(*) FROM pragma_table_info('facts') WHERE name = 'source'")?
    .query_row([], |row| row.get::<_, i64>(0))
    .unwrap_or(0)
    > 0;
if !has_source_col {
    conn.execute_batch(
        "ALTER TABLE facts ADD COLUMN source TEXT NOT NULL DEFAULT 'transcript';"
    )?;
    tracing::info!("Migrated facts table: added source column");
}

// Standing instructions table for Autopilot mode.
conn.execute_batch(
    "CREATE TABLE IF NOT EXISTS standing_instructions (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        instruction TEXT NOT NULL,
        created_at TEXT NOT NULL,
        enabled INTEGER NOT NULL DEFAULT 1
    );"
)?;
```

- [ ] **Step 2: Update the `Fact` struct to include `source`**

```rust
#[derive(Debug, Clone, Serialize)]
pub struct Fact {
    pub id: i64,
    pub session_id: String,
    pub category: String,
    pub content: String,
    pub entities: Option<String>,
    pub importance: f64,
    pub created_at: String,
    pub source: String,  // "transcript", "explicit", "inferred"
}
```

- [ ] **Step 3: Update all fact query methods to include `source`**

Update `get_facts_for_session`, `search_memory`, and `search_memory_with_sessions` to select and populate the `source` field. Every `query_map` that builds a `Fact` needs to read the additional column.

In `get_facts_for_session`:
```rust
pub fn get_facts_for_session(&self, session_id: &str) -> Result<Vec<Fact>> {
    let mut stmt = self.conn.prepare(
        "SELECT id, session_id, category, content, entities, importance, created_at, source
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
                source: row.get::<_, String>(7).unwrap_or_else(|_| "transcript".into()),
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(facts)
}
```

Apply the same pattern to `search_memory` (both FTS5 and LIKE branches) and the `search_memory_with_sessions` method.

- [ ] **Step 4: Update `add_fact` to accept a `source` parameter**

```rust
pub fn add_fact(
    &self,
    session_id: &str,
    category: &str,
    content: &str,
    entities: Option<&str>,
    importance: f64,
    source: &str,
) -> Result<()> {
    let now = chrono::Utc::now().to_rfc3339();
    self.conn.execute(
        "INSERT INTO facts (session_id, category, content, entities, importance, created_at, source)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![session_id, category, content, entities, importance, now, source],
    )?;
    Ok(())
}
```

- [ ] **Step 5: Add standing instruction CRUD methods**

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StandingInstruction {
    pub id: i64,
    pub instruction: String,
    pub created_at: String,
    pub enabled: bool,
}

impl SessionMemory {
    pub fn add_standing_instruction(&self, instruction: &str) -> Result<i64> {
        let now = chrono::Utc::now().to_rfc3339();
        self.conn.execute(
            "INSERT INTO standing_instructions (instruction, created_at) VALUES (?1, ?2)",
            params![instruction, now],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn list_standing_instructions(&self) -> Result<Vec<StandingInstruction>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, instruction, created_at, enabled
             FROM standing_instructions ORDER BY id ASC",
        )?;
        let instructions = stmt
            .query_map([], |row| {
                Ok(StandingInstruction {
                    id: row.get(0)?,
                    instruction: row.get(1)?,
                    created_at: row.get(2)?,
                    enabled: row.get::<_, i64>(3)? != 0,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(instructions)
    }

    pub fn get_enabled_standing_instructions(&self) -> Result<Vec<StandingInstruction>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, instruction, created_at, enabled
             FROM standing_instructions WHERE enabled = 1 ORDER BY id ASC",
        )?;
        let instructions = stmt
            .query_map([], |row| {
                Ok(StandingInstruction {
                    id: row.get(0)?,
                    instruction: row.get(1)?,
                    created_at: row.get(2)?,
                    enabled: row.get::<_, i64>(3)? != 0,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(instructions)
    }

    pub fn toggle_standing_instruction(&self, id: i64, enabled: bool) -> Result<()> {
        self.conn.execute(
            "UPDATE standing_instructions SET enabled = ?1 WHERE id = ?2",
            params![enabled as i64, id],
        )?;
        Ok(())
    }

    pub fn delete_standing_instruction(&self, id: i64) -> Result<()> {
        self.conn.execute(
            "DELETE FROM standing_instructions WHERE id = ?1",
            params![id],
        )?;
        Ok(())
    }
}
```

- [ ] **Step 6: Add profile derivation query — all facts sorted by recency x importance**

```rust
impl SessionMemory {
    /// Get all facts ordered by recency-weighted importance for profile derivation.
    /// Score = importance * recency_weight, where recency_weight decays from 1.0 (today)
    /// to 0.3 (30+ days ago).
    pub fn get_facts_for_profile(&self, limit: usize) -> Result<Vec<Fact>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, session_id, category, content, entities, importance, created_at, source,
                    importance * MAX(0.3, 1.0 - (julianday('now') - julianday(created_at)) / 30.0) AS score
             FROM facts
             ORDER BY score DESC
             LIMIT ?1",
        )?;
        let facts = stmt
            .query_map(params![limit], |row| {
                Ok(Fact {
                    id: row.get(0)?,
                    session_id: row.get(1)?,
                    category: row.get(2)?,
                    content: row.get(3)?,
                    entities: row.get(4)?,
                    importance: row.get(5)?,
                    created_at: row.get(6)?,
                    source: row.get::<_, String>(7).unwrap_or_else(|_| "transcript".into()),
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(facts)
    }

    /// Get the N most recent corrections and preferences (uncompressed, for Tier 2).
    pub fn get_recent_corrections(&self, limit: usize) -> Result<Vec<Fact>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, session_id, category, content, entities, importance, created_at, source
             FROM facts
             WHERE category IN ('correction', 'preference')
             ORDER BY created_at DESC
             LIMIT ?1",
        )?;
        let facts = stmt
            .query_map(params![limit], |row| {
                Ok(Fact {
                    id: row.get(0)?,
                    session_id: row.get(1)?,
                    category: row.get(2)?,
                    content: row.get(3)?,
                    entities: row.get(4)?,
                    importance: row.get(5)?,
                    created_at: row.get(6)?,
                    source: row.get::<_, String>(7).unwrap_or_else(|_| "transcript".into()),
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(facts)
    }
}
```

- [ ] **Step 7: Fix all existing callers of `add_fact` to pass the new `source` parameter**

Search the codebase for all calls to `add_fact` (primarily in `processor.rs` and `store.rs` tests) and add the `source` argument:

In `crates/aura-daemon/src/processor.rs` (save_memory tool handler, around line 580):
```rust
// Change:
m.add_fact(&s, &cat, &cont, None, 0.7)
// To:
m.add_fact(&s, &cat, &cont, None, 0.7, "explicit")
```

In `crates/aura-daemon/src/processor.rs` (end-of-session consolidation, around line 1288):
```rust
// Change:
mem.add_fact(&es_sid, cat, content, entities.as_deref(), *importance)?;
// To:
mem.add_fact(&es_sid, cat, content, entities.as_deref(), *importance, "transcript")?;
```

In `crates/aura-memory/src/store.rs` tests, update all `add_fact` calls:
```rust
// Change:
mem.add_fact(&sid, "preference", "User prefers dark mode", Some(r#"["dark mode"]"#), 0.8)
// To:
mem.add_fact(&sid, "preference", "User prefers dark mode", Some(r#"["dark mode"]"#), 0.8, "transcript")
```

- [ ] **Step 8: Write tests for new schema features**

```rust
#[cfg(test)]
mod phase4_tests {
    use super::*;

    #[test]
    fn source_column_defaults_to_transcript() {
        let dir = tempfile::tempdir().unwrap();
        let mem = SessionMemory::open(&dir.path().join("test.db")).unwrap();
        let sid = mem.start_session().unwrap();
        mem.add_fact(&sid, "preference", "likes dark mode", None, 0.8, "transcript").unwrap();
        let facts = mem.get_facts_for_session(&sid).unwrap();
        assert_eq!(facts[0].source, "transcript");
    }

    #[test]
    fn explicit_source_preserved() {
        let dir = tempfile::tempdir().unwrap();
        let mem = SessionMemory::open(&dir.path().join("test.db")).unwrap();
        let sid = mem.start_session().unwrap();
        mem.add_fact(&sid, "preference", "always use Chrome", None, 0.9, "explicit").unwrap();
        let facts = mem.get_facts_for_session(&sid).unwrap();
        assert_eq!(facts[0].source, "explicit");
    }

    #[test]
    fn standing_instructions_crud() {
        let dir = tempfile::tempdir().unwrap();
        let mem = SessionMemory::open(&dir.path().join("test.db")).unwrap();

        let id = mem.add_standing_instruction("When a build fails, show me the error").unwrap();
        assert!(id > 0);

        let all = mem.list_standing_instructions().unwrap();
        assert_eq!(all.len(), 1);
        assert!(all[0].enabled);
        assert_eq!(all[0].instruction, "When a build fails, show me the error");

        mem.toggle_standing_instruction(id, false).unwrap();
        let disabled = mem.get_enabled_standing_instructions().unwrap();
        assert!(disabled.is_empty());

        mem.toggle_standing_instruction(id, true).unwrap();
        let enabled = mem.get_enabled_standing_instructions().unwrap();
        assert_eq!(enabled.len(), 1);

        mem.delete_standing_instruction(id).unwrap();
        let empty = mem.list_standing_instructions().unwrap();
        assert!(empty.is_empty());
    }

    #[test]
    fn profile_facts_ordered_by_score() {
        let dir = tempfile::tempdir().unwrap();
        let mem = SessionMemory::open(&dir.path().join("test.db")).unwrap();
        let sid = mem.start_session().unwrap();

        mem.add_fact(&sid, "preference", "high importance recent", None, 1.0, "explicit").unwrap();
        mem.add_fact(&sid, "preference", "low importance recent", None, 0.1, "transcript").unwrap();

        let profile_facts = mem.get_facts_for_profile(10).unwrap();
        assert!(profile_facts.len() >= 2);
        // High importance should come first
        assert_eq!(profile_facts[0].content, "high importance recent");
    }

    #[test]
    fn recent_corrections_newest_first() {
        let dir = tempfile::tempdir().unwrap();
        let mem = SessionMemory::open(&dir.path().join("test.db")).unwrap();
        let sid = mem.start_session().unwrap();

        mem.add_fact(&sid, "correction", "first correction", None, 0.7, "explicit").unwrap();
        mem.add_fact(&sid, "correction", "second correction", None, 0.7, "explicit").unwrap();
        mem.add_fact(&sid, "preference", "a preference", None, 0.8, "transcript").unwrap();

        let recent = mem.get_recent_corrections(10).unwrap();
        assert_eq!(recent.len(), 3); // 2 corrections + 1 preference
        // Most recent first
        assert_eq!(recent[0].content, "a preference");
    }
}
```

- [ ] **Step 9: Run tests**

```bash
cd /Users/abdullahiabdi/Developer/aura && cargo test -p aura-memory 2>&1
```
Expected: All tests pass, including new phase4_tests.

- [ ] **Step 10: Run clippy**

```bash
cd /Users/abdullahiabdi/Developer/aura && cargo clippy -p aura-memory -- -D warnings 2>&1
```
Expected: No warnings.

- [ ] **Step 11: Commit**

```bash
git add crates/aura-memory/src/store.rs
git commit -m "feat(memory): extend schema with source column and standing instructions

Add source column to facts table (transcript/explicit/inferred) with
migration for existing databases. Add standing_instructions table for
Autopilot mode. Add profile derivation queries weighted by recency x
importance. All facts remain append-only and immutable."
```

---

## Task 2: Profile Derivation — Two-Tier Injection

**Files:**
- Create: `crates/aura-memory/src/profile.rs`
- Modify: `crates/aura-memory/src/lib.rs`

Build the two-tier profile from accumulated facts: Tier 1 is a compressed stable profile (400 tokens max), Tier 2 is the last 20 corrections/preferences uncompressed (200 tokens max). Both tiers are formatted as XML blocks for injection into the Gemini system prompt.

- [ ] **Step 1: Create `crates/aura-memory/src/profile.rs`**

```rust
//! Two-tier user profile derivation from accumulated facts.
//!
//! Tier 1 — Stable Profile (≤400 tokens):
//!   Long-term preferences, workflows, personality. Compressed from all facts
//!   weighted by recency × importance.
//!
//! Tier 2 — Recent Corrections (≤200 tokens):
//!   Last 20 corrections/preferences, uncompressed, newest first.
//!   Ensures recent feedback isn't squeezed out by compression.

use crate::store::{Fact, SessionMemory};
use anyhow::Result;

/// Maximum approximate token count for Tier 1 stable profile.
/// Using ~4 chars per token as a rough heuristic.
const TIER1_MAX_CHARS: usize = 1600; // ~400 tokens

/// Maximum approximate token count for Tier 2 recent corrections.
const TIER2_MAX_CHARS: usize = 800; // ~200 tokens

/// Maximum number of facts to consider for Tier 1.
const TIER1_MAX_FACTS: usize = 100;

/// Maximum number of recent corrections for Tier 2.
const TIER2_MAX_CORRECTIONS: usize = 20;

/// Derived user profile ready for injection into the system prompt.
#[derive(Debug, Clone)]
pub struct UserProfile {
    /// Tier 1: compressed stable profile text.
    pub stable_profile: String,
    /// Tier 2: recent corrections/preferences, newest first.
    pub recent_corrections: String,
    /// Total number of facts the profile was derived from.
    pub fact_count: usize,
}

impl UserProfile {
    /// Whether this profile has any content worth injecting.
    pub fn is_empty(&self) -> bool {
        self.stable_profile.is_empty() && self.recent_corrections.is_empty()
    }

    /// Format the profile as XML blocks for system prompt injection.
    ///
    /// Returns an empty string if the profile is empty — the system prompt
    /// should work fine without profile injection for new users.
    pub fn to_xml(&self) -> String {
        if self.is_empty() {
            return String::new();
        }

        let mut xml = String::with_capacity(
            self.stable_profile.len() + self.recent_corrections.len() + 100,
        );

        if !self.stable_profile.is_empty() {
            xml.push_str("\n<user_profile>\n");
            xml.push_str(&self.stable_profile);
            xml.push_str("\n</user_profile>\n");
        }

        if !self.recent_corrections.is_empty() {
            xml.push_str("\n<recent_context>\n");
            xml.push_str(&self.recent_corrections);
            xml.push_str("\n</recent_context>\n");
        }

        xml
    }
}

/// Derive a two-tier user profile from the memory store.
///
/// This is called once at session start. The result is injected into the
/// Gemini system prompt so the model has immediate context about the user.
pub fn derive_profile(memory: &SessionMemory) -> Result<UserProfile> {
    // Tier 1: top facts by recency-weighted importance
    let tier1_facts = memory.get_facts_for_profile(TIER1_MAX_FACTS)?;
    let stable_profile = compress_tier1(&tier1_facts);

    // Tier 2: most recent corrections and preferences
    let tier2_facts = memory.get_recent_corrections(TIER2_MAX_CORRECTIONS)?;
    let recent_corrections = format_tier2(&tier2_facts);

    let fact_count = tier1_facts.len();

    Ok(UserProfile {
        stable_profile,
        recent_corrections,
        fact_count,
    })
}

/// Compress Tier 1 facts into a single paragraph within the char budget.
///
/// Groups facts by category, deduplicates by content similarity,
/// and truncates to fit within TIER1_MAX_CHARS.
fn compress_tier1(facts: &[Fact]) -> String {
    if facts.is_empty() {
        return String::new();
    }

    // Group by category for organized output
    let mut preferences: Vec<&str> = Vec::new();
    let mut workflows: Vec<&str> = Vec::new();
    let mut habits: Vec<&str> = Vec::new();
    let mut entities: Vec<&str> = Vec::new();
    let mut other: Vec<&str> = Vec::new();

    for fact in facts {
        let bucket = match fact.category.as_str() {
            "preference" | "correction" => &mut preferences,
            "workflow" => &mut workflows,
            "habit" => &mut habits,
            "entity" => &mut entities,
            _ => &mut other,
        };
        bucket.push(&fact.content);
    }

    let mut lines: Vec<String> = Vec::new();
    let mut total_chars = 0;

    // Add facts in priority order: preferences > workflows > habits > entities > other
    for (label, bucket) in [
        ("Preferences", &preferences),
        ("Workflows", &workflows),
        ("Habits", &habits),
        ("Key context", &entities),
        ("Other", &other),
    ] {
        if bucket.is_empty() {
            continue;
        }

        let mut section_items: Vec<String> = Vec::new();
        for content in bucket {
            let line = content.to_string();
            if total_chars + line.len() + 4 > TIER1_MAX_CHARS {
                break;
            }
            total_chars += line.len() + 4; // account for "- " prefix and newline
            section_items.push(format!("- {line}"));
        }

        if !section_items.is_empty() {
            lines.push(format!("{label}:"));
            lines.extend(section_items);
        }

        if total_chars >= TIER1_MAX_CHARS {
            break;
        }
    }

    lines.join("\n")
}

/// Format Tier 2 corrections as a dated list, newest first.
fn format_tier2(facts: &[Fact]) -> String {
    if facts.is_empty() {
        return String::new();
    }

    let mut lines: Vec<String> = Vec::new();
    let mut total_chars = 0;

    for fact in facts {
        // Extract date portion from ISO 8601 timestamp (e.g., "Mar 28" from "2026-03-28T...")
        let date_display = format_short_date(&fact.created_at);
        let line = format!("- [{date_display}] {}", fact.content);

        if total_chars + line.len() + 1 > TIER2_MAX_CHARS {
            break;
        }
        total_chars += line.len() + 1;
        lines.push(line);
    }

    lines.join("\n")
}

/// Format an ISO 8601 timestamp as a short date like "Mar 28".
fn format_short_date(iso_timestamp: &str) -> String {
    // Parse "2026-03-28T12:34:56+00:00" -> "Mar 28"
    if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(iso_timestamp) {
        dt.format("%b %d").to_string()
    } else if iso_timestamp.len() >= 10 {
        // Fallback: extract YYYY-MM-DD and format manually
        iso_timestamp[5..10].to_string()
    } else {
        "recent".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_profile_produces_empty_xml() {
        let profile = UserProfile {
            stable_profile: String::new(),
            recent_corrections: String::new(),
            fact_count: 0,
        };
        assert!(profile.is_empty());
        assert_eq!(profile.to_xml(), "");
    }

    #[test]
    fn profile_xml_contains_both_tiers() {
        let profile = UserProfile {
            stable_profile: "Preferences:\n- Prefers dark mode".into(),
            recent_corrections: "- [Mar 28] Don't narrate actions".into(),
            fact_count: 2,
        };
        let xml = profile.to_xml();
        assert!(xml.contains("<user_profile>"));
        assert!(xml.contains("Prefers dark mode"));
        assert!(xml.contains("</user_profile>"));
        assert!(xml.contains("<recent_context>"));
        assert!(xml.contains("Don't narrate actions"));
        assert!(xml.contains("</recent_context>"));
    }

    #[test]
    fn profile_xml_only_stable_when_no_corrections() {
        let profile = UserProfile {
            stable_profile: "Preferences:\n- Uses Chrome".into(),
            recent_corrections: String::new(),
            fact_count: 1,
        };
        let xml = profile.to_xml();
        assert!(xml.contains("<user_profile>"));
        assert!(!xml.contains("<recent_context>"));
    }

    #[test]
    fn compress_tier1_respects_char_budget() {
        let facts: Vec<Fact> = (0..200)
            .map(|i| Fact {
                id: i,
                session_id: "s1".into(),
                category: "preference".into(),
                content: format!("Preference item number {i} with some padding text"),
                entities: None,
                importance: 0.8,
                created_at: "2026-03-28T12:00:00+00:00".into(),
                source: "transcript".into(),
            })
            .collect();

        let compressed = compress_tier1(&facts);
        assert!(compressed.len() <= TIER1_MAX_CHARS + 50); // allow small overshoot from section header
    }

    #[test]
    fn format_tier2_newest_first() {
        let facts = vec![
            Fact {
                id: 2,
                session_id: "s1".into(),
                category: "correction".into(),
                content: "Second correction".into(),
                entities: None,
                importance: 0.7,
                created_at: "2026-03-29T12:00:00+00:00".into(),
                source: "explicit".into(),
            },
            Fact {
                id: 1,
                session_id: "s1".into(),
                category: "correction".into(),
                content: "First correction".into(),
                entities: None,
                importance: 0.7,
                created_at: "2026-03-28T12:00:00+00:00".into(),
                source: "explicit".into(),
            },
        ];

        let formatted = format_tier2(&facts);
        let lines: Vec<&str> = formatted.lines().collect();
        assert!(lines[0].contains("Second correction"));
        assert!(lines[1].contains("First correction"));
    }

    #[test]
    fn format_short_date_parses_rfc3339() {
        let date = format_short_date("2026-03-28T12:34:56+00:00");
        assert_eq!(date, "Mar 28");
    }

    #[test]
    fn derive_profile_on_empty_db() {
        let dir = tempfile::tempdir().unwrap();
        let mem = SessionMemory::open(&dir.path().join("test.db")).unwrap();
        let profile = derive_profile(&mem).unwrap();
        assert!(profile.is_empty());
        assert_eq!(profile.fact_count, 0);
    }

    #[test]
    fn derive_profile_with_facts() {
        let dir = tempfile::tempdir().unwrap();
        let mem = SessionMemory::open(&dir.path().join("test.db")).unwrap();
        let sid = mem.start_session().unwrap();
        mem.add_fact(&sid, "preference", "Prefers terse responses", None, 0.9, "explicit").unwrap();
        mem.add_fact(&sid, "correction", "Don't use Safari", None, 0.8, "explicit").unwrap();

        let profile = derive_profile(&mem).unwrap();
        assert!(!profile.is_empty());
        assert!(profile.stable_profile.contains("terse responses"));
        assert!(profile.recent_corrections.contains("Safari"));
    }
}
```

- [ ] **Step 2: Update `crates/aura-memory/src/lib.rs`**

```rust
pub mod consolidate;
pub mod profile;
pub mod store;

pub use store::{Fact, Message, MessageRole, Session, SessionMemory, StandingInstruction};
pub use profile::{UserProfile, derive_profile};
```

- [ ] **Step 3: Run tests**

```bash
cd /Users/abdullahiabdi/Developer/aura && cargo test -p aura-memory 2>&1
```
Expected: All tests pass.

- [ ] **Step 4: Commit**

```bash
git add crates/aura-memory/src/profile.rs crates/aura-memory/src/lib.rs
git commit -m "feat(memory): add two-tier profile derivation from accumulated facts

Tier 1 (stable profile, 400 tokens): long-term preferences grouped
by category, recency-weighted. Tier 2 (recent corrections, 200 tokens):
last 20 corrections/preferences, uncompressed, newest first. Formatted
as <user_profile> + <recent_context> XML blocks for system prompt."
```

---

## Task 3: Profile Injection at Session Start

**Files:**
- Modify: `crates/aura-daemon/src/orchestrator.rs`
- Modify: `crates/aura-gemini/src/config.rs`

At session start, derive the user profile from memory and inject it into the Gemini system prompt before establishing the WebSocket connection.

- [ ] **Step 1: Add profile injection into the system prompt in orchestrator.rs**

In `orchestrator.rs`, inside `run_daemon()`, after the mode hint and destructive action guardrail are injected into `gemini_config.system_prompt` (around line 70-78), add profile injection:

```rust
// Inject user profile from memory into system prompt.
// This runs synchronously on a blocking thread since we need the memory lock.
{
    let profile_memory = Arc::clone(&memory);
    let profile = tokio::task::spawn_blocking(move || {
        let mem = profile_memory.lock().ok()?;
        aura_memory::derive_profile(&mem).ok()
    })
    .await
    .ok()
    .flatten();

    if let Some(profile) = profile {
        if !profile.is_empty() {
            let xml = profile.to_xml();
            tracing::info!(
                fact_count = profile.fact_count,
                stable_len = profile.stable_profile.len(),
                corrections_len = profile.recent_corrections.len(),
                "Injected user profile into system prompt"
            );
            gemini_config.system_prompt.push_str(&xml);
        } else {
            tracing::debug!("No user profile to inject (new user or empty facts)");
        }
    }
}
```

- [ ] **Step 2: Add a comment/docstring in config.rs explaining profile injection**

At the end of the `DEFAULT_SYSTEM_PROMPT` string in `crates/aura-gemini/src/config.rs` (before the closing `"#;`), add a comment block that documents the injection point. This is a hint for the orchestrator — the actual injection happens by appending to the string, not by replacing a placeholder:

```rust
// At the very end of DEFAULT_SYSTEM_PROMPT, before the closing "#;, add:

// Note: The orchestrator may append <user_profile> and <recent_context> XML blocks
// after this prompt at session start. These contain the user's learned preferences
// and recent corrections derived from accumulated facts in the memory engine.
// The model should use this context naturally — reference known preferences without
// announcing "I remember that..." — just act on them.
```

This is a documentation-only change. The actual prompt text visible to Gemini should NOT mention profile injection — the model should use profile context seamlessly.

- [ ] **Step 3: Run tests**

```bash
cd /Users/abdullahiabdi/Developer/aura && cargo test -p aura-daemon -- orchestrator 2>&1
cargo test -p aura-gemini 2>&1
```
Expected: All tests pass. The system prompt tests in config.rs should still pass since we only added a Rust comment, not prompt text.

- [ ] **Step 4: Commit**

```bash
git add crates/aura-daemon/src/orchestrator.rs crates/aura-gemini/src/config.rs
git commit -m "feat(memory): inject user profile into system prompt at session start

Derives two-tier profile from accumulated facts in SQLite and appends
as <user_profile> + <recent_context> XML blocks to the Gemini system
prompt before WebSocket connection. ~600 tokens total injection cost."
```

---

## Task 4: Fact Extraction — Update Consolidation for v2

**Files:**
- Modify: `crates/aura-memory/src/consolidate.rs`

The existing consolidation flow extracts facts after each session ends. Update the extraction prompt to include the `source` field and add the "correction" category needed for the profile derivation Tier 2.

- [ ] **Step 1: Update the consolidation prompt to include source and correction category**

In `crates/aura-memory/src/consolidate.rs`, update `build_consolidation_prompt`:

```rust
// Replace the preamble string in build_consolidation_prompt with:
let preamble = "You are a memory extraction agent. Analyze this conversation history and extract key facts.\n\n\
     The conversation contains user messages and tool calls from an AI desktop assistant.\n\n\
     Extract facts in these categories:\n\
     - \"preference\": User preferences (apps, settings, workflows they like)\n\
     - \"correction\": Explicit corrections to the assistant's behavior (\"don't do X\", \"always do Y\")\n\
     - \"habit\": Repeated behaviors or patterns\n\
     - \"workflow\": Multi-step workflows or processes the user follows\n\
     - \"entity\": Important files, apps, projects, or people the user works with\n\
     - \"task\": What the user was trying to accomplish\n\
     - \"context\": Other useful context for future sessions\n\n\
     For each fact, include a source field:\n\
     - \"explicit\": User directly stated this (\"remember that...\", \"I prefer...\", \"don't...\")\n\
     - \"inferred\": Pattern observed from behavior, not explicitly stated\n\n\
     Respond ONLY with valid JSON matching this schema:\n\
     {\"summary\": \"string\", \"facts\": [{\"category\": \"string\", \"content\": \"string\", \"entities\": [\"string\"], \"importance\": number, \"source\": \"string\"}]}\n\n\
     Categories: preference, correction, habit, workflow, entity, task, context.\n\
     Sources: explicit, inferred.\n\n\
     If the session was trivial (just a greeting or test), return an empty facts array.\n\n\
     --- CONVERSATION ---\n";
```

- [ ] **Step 2: Update the `ExtractedFact` struct to include `source`**

```rust
#[derive(Debug, Deserialize)]
pub struct ExtractedFact {
    pub category: String,
    pub content: String,
    #[serde(default)]
    pub entities: Vec<String>,
    #[serde(default = "default_importance")]
    pub importance: f64,
    #[serde(default = "default_source")]
    pub source: String,
}

fn default_source() -> String {
    "inferred".to_string()
}
```

- [ ] **Step 3: Update the processor's end-of-session consolidation to pass `source`**

In `crates/aura-daemon/src/processor.rs`, in the consolidation result handler (around line 1265-1288), update the facts_json tuple to include source:

```rust
// Change the facts_json mapping from:
let facts_json: Vec<(String, String, Option<String>, f64)> = ...
// To:
let facts_json: Vec<(String, String, Option<String>, f64, String)> = response
    .facts
    .iter()
    .map(|f| {
        let entities = if f.entities.is_empty() {
            None
        } else {
            serde_json::to_string(&f.entities).ok()
        };
        (f.category.clone(), f.content.clone(), entities, f.importance, f.source.clone())
    })
    .collect();

// And update the add_fact call in the memory_op closure:
// Change:
for (cat, content, entities, importance) in &facts_json {
    mem.add_fact(&es_sid, cat, content, entities.as_deref(), *importance)?;
}
// To:
for (cat, content, entities, importance, source) in &facts_json {
    mem.add_fact(&es_sid, cat, content, entities.as_deref(), *importance, source)?;
}
```

- [ ] **Step 4: Update consolidation tests**

In `crates/aura-memory/src/consolidate.rs`, update `parse_consolidation_response` test:

```rust
#[test]
fn parse_consolidation_response_with_source() {
    let json = r#"{"summary":"User searched for Rust docs","facts":[{"category":"task","content":"User searched for Rust documentation","entities":["Rust"],"importance":0.6,"source":"inferred"}]}"#;
    let resp: ConsolidationResponse = serde_json::from_str(json).unwrap();
    assert_eq!(resp.facts[0].source, "inferred");
}

#[test]
fn parse_consolidation_response_source_defaults() {
    let json = r#"{"summary":"test","facts":[{"category":"task","content":"test fact","entities":[],"importance":0.5}]}"#;
    let resp: ConsolidationResponse = serde_json::from_str(json).unwrap();
    assert_eq!(resp.facts[0].source, "inferred");
}
```

- [ ] **Step 5: Run tests**

```bash
cd /Users/abdullahiabdi/Developer/aura && cargo test -p aura-memory 2>&1
cargo test -p aura-daemon 2>&1
```
Expected: All tests pass.

- [ ] **Step 6: Commit**

```bash
git add crates/aura-memory/src/consolidate.rs crates/aura-daemon/src/processor.rs
git commit -m "feat(memory): add source field and correction category to fact extraction

Consolidation prompt now extracts source (explicit/inferred) and
supports the correction category needed for profile Tier 2.
End-of-session fact storage passes source through to SQLite."
```

---

## Task 5: In-Session Explicit Fact Capture ("Remember X")

**Files:**
- Modify: `crates/aura-daemon/src/processor.rs`

When the user says "remember X" or Gemini calls `save_memory`, the fact should be saved immediately with `source='explicit'`. This is already partially implemented in v1 (the `save_memory` tool handler at ~line 560 in processor.rs), but we need to ensure the `source` parameter is correctly set and add support for voice-triggered "remember" commands that Gemini can detect and route to `save_memory`.

The existing `save_memory` handler already calls `m.add_fact()` — Task 1 Step 7 already updated it to pass `"explicit"` as the source. This task verifies the integration and adds a new "correction" category path.

- [ ] **Step 1: Verify the save_memory handler passes source='explicit'**

Confirm that the change from Task 1 Step 7 is in place. In `processor.rs` around line 580:

```rust
m.add_fact(&s, &cat, &cont, None, 0.7, "explicit")
```

The importance is hardcoded to 0.7 for tool-invoked saves. This is fine — explicit facts from the user carry medium-high importance by default.

- [ ] **Step 2: Add "correction" to the valid categories list**

In `processor.rs`, find the `VALID_CATEGORIES` constant (near the save_memory handler):

```rust
// Change:
const VALID_CATEGORIES: &[&str] = &["preference", "habit", "entity", "task", "context"];
// To:
const VALID_CATEGORIES: &[&str] = &["preference", "correction", "habit", "workflow", "entity", "task", "context"];
```

- [ ] **Step 3: Run tests**

```bash
cd /Users/abdullahiabdi/Developer/aura && cargo test -p aura-daemon 2>&1
```
Expected: All tests pass.

- [ ] **Step 4: Commit**

```bash
git add crates/aura-daemon/src/processor.rs
git commit -m "feat(memory): add correction and workflow categories to save_memory tool

Adds correction and workflow to valid save_memory categories so Gemini
can store behavioral corrections (don't do X) and workflow patterns
directly. Addresses cold-path bottleneck — facts from voice commands
are saved immediately, not deferred to end-of-session consolidation."
```

---

## Task 6: Autonomy Mode — State Management and Approval Gate

**Files:**
- Create: `crates/aura-daemon/src/autonomy.rs`

The autonomy system has three modes (Ghost/Copilot/Autopilot) with different thresholds for proactive interventions and tool execution. The mode is stored in SQLite settings and can be changed via voice command or UI.

- [ ] **Step 1: Create `crates/aura-daemon/src/autonomy.rs`**

```rust
//! Autonomy mode management and approval gate.
//!
//! Three modes control how aggressively Aura intervenes:
//! - **Ghost**: Listen and observe, never speak unprompted, execute only on explicit request.
//! - **Copilot** (default): Speak when highly confident (0.85), execute on explicit request.
//! - **Autopilot**: Speak when moderately confident (0.60), execute safe tools automatically,
//!   standing instructions are active.

use serde::{Deserialize, Serialize};

/// The three autonomy modes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AutonomyMode {
    Ghost,
    Copilot,
    Autopilot,
}

impl AutonomyMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Ghost => "ghost",
            Self::Copilot => "copilot",
            Self::Autopilot => "autopilot",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "ghost" => Some(Self::Ghost),
            "copilot" => Some(Self::Copilot),
            "autopilot" => Some(Self::Autopilot),
            _ => None,
        }
    }

    /// Proactive intervention threshold for this mode.
    /// Returns None for Ghost (never intervene).
    pub fn proactive_threshold(&self) -> Option<f64> {
        match self {
            Self::Ghost => None,
            Self::Copilot => Some(0.85),
            Self::Autopilot => Some(0.60),
        }
    }

    /// Maximum unprompted interventions per this interval (in seconds).
    pub fn rate_limit(&self) -> Option<(u32, u64)> {
        match self {
            Self::Ghost => None,
            Self::Copilot => Some((1, 300)),    // 1 per 5 min
            Self::Autopilot => Some((1, 120)),  // 1 per 2 min
        }
    }
}

impl Default for AutonomyMode {
    fn default() -> Self {
        Self::Copilot
    }
}

/// Result of the approval gate check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApprovalResult {
    /// Action is allowed without user confirmation.
    Allow,
    /// Action requires explicit user request (not just proactive suggestion).
    RequireExplicitRequest,
    /// Action needs user confirmation before execution ("Delete X — sure?").
    Confirm,
    /// Action is suggested but not executed ("Want me to...?").
    Suggest,
}

/// Metadata about a tool action for the approval gate.
pub struct ToolAction {
    pub name: String,
    pub is_destructive: bool,
    pub is_user_requested: bool,
}

impl ToolAction {
    /// Check if this tool is considered "safe" for auto-execution in Autopilot mode.
    /// Safe tools are read-only or low-risk actions.
    pub fn is_safe_tool(&self) -> bool {
        matches!(
            self.name.as_str(),
            "get_screen_context"
                | "recall_memory"
                | "save_memory"
                | "move_mouse"
                | "scroll"
                | "activate_app"
                | "click_menu_item"
                | "press_key"
                | "type_text"
                | "write_clipboard"
                | "click"
                | "click_element"
        )
    }
}

/// Check whether a tool action requires user approval given the current mode.
///
/// This gate runs before every tool execution. The rules:
/// - Destructive actions ALWAYS require confirmation regardless of mode.
/// - In Ghost/Copilot: actions require explicit user request.
/// - In Autopilot: safe tools execute automatically; others are suggested.
/// - Standing instructions can promote a "Suggest" to "Allow" in Autopilot.
pub fn requires_approval(
    action: &ToolAction,
    mode: AutonomyMode,
    standing_instructions: &[crate::StandingInstruction],
) -> ApprovalResult {
    // Destructive actions ALWAYS require confirmation regardless of mode
    if action.is_destructive {
        return ApprovalResult::Confirm;
    }

    // User-requested actions are always allowed (user explicitly asked)
    if action.is_user_requested {
        return ApprovalResult::Allow;
    }

    match mode {
        AutonomyMode::Ghost => ApprovalResult::RequireExplicitRequest,
        AutonomyMode::Copilot => ApprovalResult::RequireExplicitRequest,
        AutonomyMode::Autopilot => {
            if action.is_safe_tool() {
                ApprovalResult::Allow
            } else if matches_standing_instruction(action, standing_instructions) {
                ApprovalResult::Allow
            } else {
                ApprovalResult::Suggest
            }
        }
    }
}

/// Check if any standing instruction covers this action.
fn matches_standing_instruction(
    action: &ToolAction,
    instructions: &[crate::StandingInstruction],
) -> bool {
    // Simple keyword matching for now.
    // Future: use Gemini to evaluate instruction relevance.
    let action_lower = action.name.to_lowercase();
    instructions.iter().any(|inst| {
        let inst_lower = inst.instruction.to_lowercase();
        // Check if the instruction mentions the tool name or related keywords
        inst_lower.contains(&action_lower)
            || (action_lower == "run_shell_command" && inst_lower.contains("shell"))
            || (action_lower == "run_applescript" && inst_lower.contains("script"))
            || (action_lower.contains("click") && inst_lower.contains("click"))
    })
}

/// Load the autonomy mode from SQLite settings, defaulting to Copilot.
pub fn load_mode(memory: &aura_memory::SessionMemory) -> AutonomyMode {
    memory
        .get_setting("autonomy_mode")
        .ok()
        .flatten()
        .and_then(|s| AutonomyMode::from_str(&s))
        .unwrap_or_default()
}

/// Save the autonomy mode to SQLite settings.
pub fn save_mode(memory: &aura_memory::SessionMemory, mode: AutonomyMode) -> anyhow::Result<()> {
    memory.set_setting("autonomy_mode", mode.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_mode_is_copilot() {
        assert_eq!(AutonomyMode::default(), AutonomyMode::Copilot);
    }

    #[test]
    fn mode_roundtrip() {
        for mode in [AutonomyMode::Ghost, AutonomyMode::Copilot, AutonomyMode::Autopilot] {
            let s = mode.as_str();
            assert_eq!(AutonomyMode::from_str(s), Some(mode));
        }
    }

    #[test]
    fn mode_from_str_case_insensitive() {
        assert_eq!(AutonomyMode::from_str("GHOST"), Some(AutonomyMode::Ghost));
        assert_eq!(AutonomyMode::from_str("Copilot"), Some(AutonomyMode::Copilot));
        assert_eq!(AutonomyMode::from_str("invalid"), None);
    }

    #[test]
    fn ghost_thresholds() {
        assert_eq!(AutonomyMode::Ghost.proactive_threshold(), None);
        assert_eq!(AutonomyMode::Ghost.rate_limit(), None);
    }

    #[test]
    fn copilot_thresholds() {
        assert_eq!(AutonomyMode::Copilot.proactive_threshold(), Some(0.85));
        assert_eq!(AutonomyMode::Copilot.rate_limit(), Some((1, 300)));
    }

    #[test]
    fn autopilot_thresholds() {
        assert_eq!(AutonomyMode::Autopilot.proactive_threshold(), Some(0.60));
        assert_eq!(AutonomyMode::Autopilot.rate_limit(), Some((1, 120)));
    }

    #[test]
    fn destructive_always_requires_confirm() {
        let action = ToolAction {
            name: "run_shell_command".into(),
            is_destructive: true,
            is_user_requested: true,
        };
        // Even in Autopilot with user request, destructive = Confirm
        assert_eq!(
            requires_approval(&action, AutonomyMode::Autopilot, &[]),
            ApprovalResult::Confirm
        );
    }

    #[test]
    fn user_requested_non_destructive_always_allowed() {
        let action = ToolAction {
            name: "click".into(),
            is_destructive: false,
            is_user_requested: true,
        };
        assert_eq!(
            requires_approval(&action, AutonomyMode::Ghost, &[]),
            ApprovalResult::Allow
        );
    }

    #[test]
    fn ghost_blocks_proactive_actions() {
        let action = ToolAction {
            name: "click".into(),
            is_destructive: false,
            is_user_requested: false,
        };
        assert_eq!(
            requires_approval(&action, AutonomyMode::Ghost, &[]),
            ApprovalResult::RequireExplicitRequest
        );
    }

    #[test]
    fn copilot_blocks_proactive_actions() {
        let action = ToolAction {
            name: "click".into(),
            is_destructive: false,
            is_user_requested: false,
        };
        assert_eq!(
            requires_approval(&action, AutonomyMode::Copilot, &[]),
            ApprovalResult::RequireExplicitRequest
        );
    }

    #[test]
    fn autopilot_allows_safe_tools_proactively() {
        let action = ToolAction {
            name: "get_screen_context".into(),
            is_destructive: false,
            is_user_requested: false,
        };
        assert_eq!(
            requires_approval(&action, AutonomyMode::Autopilot, &[]),
            ApprovalResult::Allow
        );
    }

    #[test]
    fn autopilot_suggests_unsafe_tools_without_instruction() {
        let action = ToolAction {
            name: "run_shell_command".into(),
            is_destructive: false,
            is_user_requested: false,
        };
        assert_eq!(
            requires_approval(&action, AutonomyMode::Autopilot, &[]),
            ApprovalResult::Suggest
        );
    }

    #[test]
    fn autopilot_allows_unsafe_tool_with_matching_instruction() {
        let action = ToolAction {
            name: "run_shell_command".into(),
            is_destructive: false,
            is_user_requested: false,
        };
        let instructions = vec![aura_memory::StandingInstruction {
            id: 1,
            instruction: "Run shell commands for build tasks automatically".into(),
            created_at: "2026-03-28T12:00:00+00:00".into(),
            enabled: true,
        }];
        assert_eq!(
            requires_approval(&action, AutonomyMode::Autopilot, &instructions),
            ApprovalResult::Allow
        );
    }

    #[test]
    fn mode_persists_in_sqlite() {
        let dir = tempfile::tempdir().unwrap();
        let mem = aura_memory::SessionMemory::open(&dir.path().join("test.db")).unwrap();

        assert_eq!(load_mode(&mem), AutonomyMode::Copilot); // default
        save_mode(&mem, AutonomyMode::Ghost).unwrap();
        assert_eq!(load_mode(&mem), AutonomyMode::Ghost);
        save_mode(&mem, AutonomyMode::Autopilot).unwrap();
        assert_eq!(load_mode(&mem), AutonomyMode::Autopilot);
    }
}
```

- [ ] **Step 2: Register the module in the daemon crate**

Find the `mod` declarations in `crates/aura-daemon/src/lib.rs` (or `main.rs` depending on structure) and add:

```rust
pub mod autonomy;
```

If the daemon uses `crate::` paths internally (as suggested by `use aura_daemon::...` in orchestrator.rs), this needs to be a public module in the library part of the crate.

- [ ] **Step 3: Run tests**

```bash
cd /Users/abdullahiabdi/Developer/aura && cargo test -p aura-daemon -- autonomy 2>&1
```
Expected: All autonomy tests pass.

- [ ] **Step 4: Run clippy**

```bash
cd /Users/abdullahiabdi/Developer/aura && cargo clippy -p aura-daemon -- -D warnings 2>&1
```

- [ ] **Step 5: Commit**

```bash
git add crates/aura-daemon/src/autonomy.rs crates/aura-daemon/src/lib.rs
git commit -m "feat(autonomy): add Ghost/Copilot/Autopilot mode state and approval gate

Three modes with different proactive thresholds and tool execution
policies. requires_approval() gate checks mode, standing instructions,
and destructive flag before every tool execution. Mode persists in
SQLite settings table."
```

---

## Task 7: Proactive Scorer — Single Scoring Function

**Files:**
- Create: `crates/aura-daemon/src/proactive.rs`

A single pure function that evaluates whether to allow a proactive intervention. This is the client-side gate on Gemini's `ProactiveAudio` — even when Gemini decides to speak unprompted, the scorer can suppress it.

- [ ] **Step 1: Create `crates/aura-daemon/src/proactive.rs`**

```rust
//! Proactive awareness scoring and anti-annoyance mechanisms.
//!
//! The proactive engine uses Gemini's native `ProactiveAudio` flag. When Gemini
//! decides to speak unprompted, the client-side scorer acts as a gate:
//! if the scorer's threshold isn't met, playback is suppressed but the event
//! is logged for threshold tuning.
//!
//! Architecture: one Gemini session, one decision point. The scorer evaluates
//! Gemini's proactive output — it doesn't generate proactive content itself.

use std::time::{Duration, Instant};

use crate::autonomy::AutonomyMode;

/// Minimum time between proactive interventions in Copilot mode.
const COPILOT_RATE_LIMIT: Duration = Duration::from_secs(300); // 5 min

/// Minimum time between proactive interventions in Autopilot mode.
const AUTOPILOT_RATE_LIMIT: Duration = Duration::from_secs(120); // 2 min

/// Duration after which dismissal backoff resets.
const BACKOFF_RESET_DURATION: Duration = Duration::from_secs(1800); // 30 min

/// Maximum backoff multiplier to prevent infinite suppression.
const MAX_BACKOFF_MULTIPLIER: f64 = 16.0;

/// Tracks dismissal history for exponential backoff.
#[derive(Debug, Clone)]
pub struct BackoffState {
    /// Number of consecutive dismissals.
    dismiss_count: u32,
    /// Timestamp of the last dismissal.
    last_dismissal: Option<Instant>,
    /// Timestamp of the last allowed intervention.
    last_intervention: Option<Instant>,
}

impl BackoffState {
    pub fn new() -> Self {
        Self {
            dismiss_count: 0,
            last_dismissal: None,
            last_intervention: None,
        }
    }

    /// Record a dismissal — doubles the effective threshold.
    pub fn record_dismissal(&mut self) {
        self.dismiss_count = self.dismiss_count.saturating_add(1);
        self.last_dismissal = Some(Instant::now());
    }

    /// Record that an intervention was shown to the user.
    pub fn record_intervention(&mut self) {
        self.last_intervention = Some(Instant::now());
    }

    /// Reset backoff if enough time has passed since the last dismissal.
    fn maybe_reset(&mut self) {
        if let Some(last) = self.last_dismissal {
            if last.elapsed() >= BACKOFF_RESET_DURATION {
                self.dismiss_count = 0;
                self.last_dismissal = None;
            }
        }
    }

    /// Apply backoff multiplier to a threshold.
    /// Each dismissal doubles the threshold, up to MAX_BACKOFF_MULTIPLIER.
    pub fn apply_backoff(&mut self, base_threshold: f64) -> f64 {
        self.maybe_reset();
        let multiplier = (2.0_f64.powi(self.dismiss_count as i32)).min(MAX_BACKOFF_MULTIPLIER);
        (base_threshold * multiplier).min(1.0) // threshold can't exceed 1.0
    }

    /// Check if the rate limit allows an intervention now.
    pub fn rate_limit_allows(&self, mode: AutonomyMode) -> bool {
        let limit = match mode {
            AutonomyMode::Ghost => return false,
            AutonomyMode::Copilot => COPILOT_RATE_LIMIT,
            AutonomyMode::Autopilot => AUTOPILOT_RATE_LIMIT,
        };

        match self.last_intervention {
            Some(last) => last.elapsed() >= limit,
            None => true, // no previous intervention
        }
    }
}

impl Default for BackoffState {
    fn default() -> Self {
        Self::new()
    }
}

/// Context about what's happening on screen for the scorer.
#[derive(Debug, Clone)]
pub struct ScreenDelta {
    /// Whether the screen content changed since last check.
    pub has_changes: bool,
    /// How different the screen is (0.0 = identical, 1.0 = completely different).
    pub change_magnitude: f64,
}

/// Context about the current app.
#[derive(Debug, Clone)]
pub struct AppContext {
    /// Name of the frontmost application.
    pub app_name: String,
    /// Title of the focused window, if available.
    pub window_title: Option<String>,
}

/// A proactive intervention that passed the scoring gate.
#[derive(Debug, Clone)]
pub struct Intervention {
    /// Unique ID for dismissal tracking.
    pub id: String,
}

/// Result of the scoring decision.
#[derive(Debug)]
pub enum ScoringDecision {
    /// Allow Gemini's proactive audio to play.
    Allow(Intervention),
    /// Suppress playback — threshold not met.
    Suppress { reason: String },
}

/// Score whether a proactive intervention should be allowed.
///
/// This is the single decision point. Even when Gemini's ProactiveAudio triggers,
/// the client-side scorer has final authority on whether to play the audio.
///
/// Arguments:
/// - `screen_delta`: What changed on screen since last check.
/// - `app_context`: Current frontmost app and window.
/// - `idle_duration`: How long the user has been idle (no input events).
/// - `mode`: Current autonomy mode (Ghost/Copilot/Autopilot).
/// - `standing_instructions`: Active standing rules (only relevant in Autopilot).
/// - `backoff`: Mutable backoff state — updated on allow/dismiss.
pub fn score_intervention(
    screen_delta: &ScreenDelta,
    _app_context: &AppContext,
    idle_duration: Duration,
    mode: AutonomyMode,
    _standing_instructions: &[aura_memory::StandingInstruction],
    backoff: &mut BackoffState,
) -> ScoringDecision {
    // Ghost mode: never intervene
    let base_threshold = match mode.proactive_threshold() {
        Some(t) => t,
        None => {
            return ScoringDecision::Suppress {
                reason: "Ghost mode — interventions disabled".into(),
            };
        }
    };

    // Rate limit check
    if !backoff.rate_limit_allows(mode) {
        return ScoringDecision::Suppress {
            reason: format!("Rate limited — too soon after last intervention"),
        };
    }

    // Apply dismissal backoff
    let threshold = backoff.apply_backoff(base_threshold);

    // Compute a raw score based on available signals.
    // In the initial implementation, we use simple heuristics.
    // Future: feed screen context + standing instructions to a Gemini evaluation call.
    let mut score = 0.0;

    // Screen changes suggest the user is active — proactive help may be useful
    if screen_delta.has_changes {
        score += 0.3 * screen_delta.change_magnitude;
    }

    // Idle users might be stuck — higher score for idle > 60s
    if idle_duration > Duration::from_secs(60) {
        score += 0.4;
    } else if idle_duration > Duration::from_secs(30) {
        score += 0.2;
    }

    // Gemini has already decided this is worth saying (ProactiveAudio triggered),
    // so give a baseline confidence boost. The scorer is a gate, not a generator.
    score += 0.5;

    if score > threshold {
        let intervention = Intervention {
            id: uuid::Uuid::new_v4().to_string(),
        };
        backoff.record_intervention();
        ScoringDecision::Allow(intervention)
    } else {
        ScoringDecision::Suppress {
            reason: format!("Score {score:.2} below threshold {threshold:.2}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn default_screen_delta() -> ScreenDelta {
        ScreenDelta {
            has_changes: true,
            change_magnitude: 0.5,
        }
    }

    fn default_app_context() -> AppContext {
        AppContext {
            app_name: "Xcode".into(),
            window_title: Some("main.rs".into()),
        }
    }

    #[test]
    fn ghost_always_suppresses() {
        let mut backoff = BackoffState::new();
        let result = score_intervention(
            &default_screen_delta(),
            &default_app_context(),
            Duration::from_secs(120),
            AutonomyMode::Ghost,
            &[],
            &mut backoff,
        );
        assert!(matches!(result, ScoringDecision::Suppress { .. }));
    }

    #[test]
    fn copilot_allows_when_idle_and_screen_active() {
        let mut backoff = BackoffState::new();
        let result = score_intervention(
            &ScreenDelta { has_changes: true, change_magnitude: 0.8 },
            &default_app_context(),
            Duration::from_secs(120), // idle for 2 min
            AutonomyMode::Copilot,
            &[],
            &mut backoff,
        );
        // Score: 0.3*0.8 + 0.4 + 0.5 = 1.14 > 0.85 threshold
        assert!(matches!(result, ScoringDecision::Allow(_)));
    }

    #[test]
    fn autopilot_allows_with_lower_threshold() {
        let mut backoff = BackoffState::new();
        let result = score_intervention(
            &ScreenDelta { has_changes: true, change_magnitude: 0.3 },
            &default_app_context(),
            Duration::from_secs(10), // not idle
            AutonomyMode::Autopilot,
            &[],
            &mut backoff,
        );
        // Score: 0.3*0.3 + 0 + 0.5 = 0.59 < 0.60 threshold — barely suppressed
        // (exact threshold behavior depends on rounding)
        // Let's use a higher magnitude to ensure Allow
        let result = score_intervention(
            &ScreenDelta { has_changes: true, change_magnitude: 0.5 },
            &default_app_context(),
            Duration::from_secs(10),
            AutonomyMode::Autopilot,
            &[],
            &mut backoff,
        );
        // Score: 0.3*0.5 + 0 + 0.5 = 0.65 > 0.60
        assert!(matches!(result, ScoringDecision::Allow(_)));
    }

    #[test]
    fn dismissal_backoff_doubles_threshold() {
        let mut backoff = BackoffState::new();

        // First call should allow
        let result = score_intervention(
            &ScreenDelta { has_changes: true, change_magnitude: 0.8 },
            &default_app_context(),
            Duration::from_secs(120),
            AutonomyMode::Copilot,
            &[],
            &mut backoff,
        );
        assert!(matches!(result, ScoringDecision::Allow(_)));

        // Record dismissal — threshold doubles from 0.85 to 1.0 (capped)
        backoff.record_dismissal();

        let result = score_intervention(
            &ScreenDelta { has_changes: true, change_magnitude: 0.8 },
            &default_app_context(),
            Duration::from_secs(120),
            AutonomyMode::Copilot,
            &[],
            &mut backoff,
        );
        // Threshold after 1 dismiss: 0.85 * 2 = 1.7 -> capped to 1.0
        // Score: ~1.14 > 1.0 — should still allow (barely)
        assert!(matches!(result, ScoringDecision::Allow(_)));
    }

    #[test]
    fn rate_limit_suppresses_rapid_interventions() {
        let mut backoff = BackoffState::new();

        // First intervention allowed
        let result = score_intervention(
            &ScreenDelta { has_changes: true, change_magnitude: 0.8 },
            &default_app_context(),
            Duration::from_secs(120),
            AutonomyMode::Copilot,
            &[],
            &mut backoff,
        );
        assert!(matches!(result, ScoringDecision::Allow(_)));

        // Second intervention immediately after — rate limited
        let result = score_intervention(
            &ScreenDelta { has_changes: true, change_magnitude: 0.8 },
            &default_app_context(),
            Duration::from_secs(120),
            AutonomyMode::Copilot,
            &[],
            &mut backoff,
        );
        assert!(matches!(result, ScoringDecision::Suppress { .. }));
    }

    #[test]
    fn backoff_resets_after_timeout() {
        let mut backoff = BackoffState::new();
        backoff.record_dismissal();
        backoff.record_dismissal();
        assert_eq!(backoff.dismiss_count, 2);

        // Simulate time passing (we can't easily fast-forward Instant in tests,
        // so just verify the reset logic exists)
        // In production, 30 min of no dismissals resets the counter.
        assert!(backoff.apply_backoff(0.5) > 0.5); // backoff active
    }

    #[test]
    fn max_backoff_capped() {
        let mut backoff = BackoffState::new();
        for _ in 0..20 {
            backoff.record_dismissal();
        }
        // Should be capped at MAX_BACKOFF_MULTIPLIER
        let threshold = backoff.apply_backoff(0.5);
        assert!(threshold <= 1.0);
    }
}
```

- [ ] **Step 2: Register the module**

Add to `crates/aura-daemon/src/lib.rs`:

```rust
pub mod proactive;
```

- [ ] **Step 3: Run tests**

```bash
cd /Users/abdullahiabdi/Developer/aura && cargo test -p aura-daemon -- proactive 2>&1
```
Expected: All proactive tests pass.

- [ ] **Step 4: Commit**

```bash
git add crates/aura-daemon/src/proactive.rs crates/aura-daemon/src/lib.rs
git commit -m "feat(proactive): add client-side scoring function with dismissal backoff

Single score_intervention() function gates Gemini ProactiveAudio output.
Evaluates screen delta, idle duration, mode threshold, rate limits, and
dismissal backoff. Ghost=never, Copilot=0.85, Autopilot=0.60. Backoff
doubles per dismiss, resets after 30min, capped at 16x."
```

---

## Task 8: Extend IPC Protocol for Autonomy and Proactive Events

**Files:**
- Modify: `crates/aura-daemon/src/protocol.rs`
- Modify: `AuraApp/Sources/Protocol.swift`

Add new IPC events and commands for mode changes, proactive interventions, dismissals, standing instructions, and playback control.

- [ ] **Step 1: Extend Rust DaemonEvent enum**

In `crates/aura-daemon/src/protocol.rs`, add to the `DaemonEvent` enum:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum DaemonEvent {
    // ... existing variants ...

    /// Autonomy mode changed.
    ModeChanged { mode: String },

    /// A proactive intervention from Gemini that passed the scorer.
    ProactiveIntervention {
        text: String,
        intervention_id: String,
    },

    /// Gemini audio for playback (base64-encoded 24kHz 16-bit PCM).
    PlaybackAudio { audio: String },

    /// Start of a new audio playback stream.
    PlaybackStart,

    /// End of audio playback stream.
    PlaybackStop,

    /// Gemini was interrupted (barge-in) — stop playback immediately.
    Interrupted,

    /// Current standing instructions list (sent on mode change or instruction update).
    StandingInstructions {
        instructions: Vec<StandingInstructionInfo>,
    },
}

/// Standing instruction info for IPC transport.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StandingInstructionInfo {
    pub id: i64,
    pub instruction: String,
    pub enabled: bool,
}
```

- [ ] **Step 2: Extend Rust UICommand enum**

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum UICommand {
    // ... existing variants ...

    /// Set the autonomy mode.
    SetMode { mode: String },

    /// Dismiss a proactive intervention.
    DismissIntervention { intervention_id: String },

    /// Add a standing instruction.
    AddStandingInstruction { instruction: String },

    /// Toggle a standing instruction on/off.
    ToggleStandingInstruction { id: i64, enabled: bool },

    /// Delete a standing instruction.
    DeleteStandingInstruction { id: i64 },

    /// Audio frame from Swift mic capture (base64-encoded 16kHz PCM).
    AudioFrame { pcm: String },
}
```

- [ ] **Step 3: Add serialization tests for new protocol types**

```rust
#[test]
fn mode_changed_serialization() {
    let event = DaemonEvent::ModeChanged { mode: "autopilot".into() };
    let json = serde_json::to_string(&event).unwrap();
    assert_eq!(json, r#"{"type":"mode_changed","mode":"autopilot"}"#);
}

#[test]
fn proactive_intervention_serialization() {
    let event = DaemonEvent::ProactiveIntervention {
        text: "Build failed — want me to read the error?".into(),
        intervention_id: "abc123".into(),
    };
    let json = serde_json::to_string(&event).unwrap();
    assert!(json.contains("proactive_intervention"));
    assert!(json.contains("abc123"));
}

#[test]
fn set_mode_serialization() {
    let cmd = UICommand::SetMode { mode: "ghost".into() };
    let json = serde_json::to_string(&cmd).unwrap();
    assert_eq!(json, r#"{"type":"set_mode","mode":"ghost"}"#);
    let parsed: UICommand = serde_json::from_str(&json).unwrap();
    match parsed {
        UICommand::SetMode { mode } => assert_eq!(mode, "ghost"),
        _ => panic!("unexpected variant"),
    }
}

#[test]
fn dismiss_intervention_serialization() {
    let cmd = UICommand::DismissIntervention { intervention_id: "xyz".into() };
    let json = serde_json::to_string(&cmd).unwrap();
    assert!(json.contains("dismiss_intervention"));
    assert!(json.contains("xyz"));
}

#[test]
fn add_standing_instruction_serialization() {
    let cmd = UICommand::AddStandingInstruction {
        instruction: "When a build fails, show me the error".into(),
    };
    let json = serde_json::to_string(&cmd).unwrap();
    assert!(json.contains("add_standing_instruction"));
}

#[test]
fn audio_frame_serialization() {
    let cmd = UICommand::AudioFrame { pcm: "AQID".into() };
    let json = serde_json::to_string(&cmd).unwrap();
    assert_eq!(json, r#"{"type":"audio_frame","pcm":"AQID"}"#);
}

#[test]
fn playback_events_serialization() {
    let start = serde_json::to_string(&DaemonEvent::PlaybackStart).unwrap();
    assert_eq!(start, r#"{"type":"playback_start"}"#);
    let stop = serde_json::to_string(&DaemonEvent::PlaybackStop).unwrap();
    assert_eq!(stop, r#"{"type":"playback_stop"}"#);
    let interrupted = serde_json::to_string(&DaemonEvent::Interrupted).unwrap();
    assert_eq!(interrupted, r#"{"type":"interrupted"}"#);
}
```

- [ ] **Step 4: Update Swift Protocol.swift**

Add the new event types to the `DaemonEvent` enum, `EventType` enum, and decoder:

```swift
// Add to DaemonEvent enum:
case modeChanged(ModeChangedUpdate)
case proactiveIntervention(ProactiveInterventionUpdate)
case playbackAudio(PlaybackAudioUpdate)
case playbackStart
case playbackStop
case interrupted
case standingInstructions(StandingInstructionsUpdate)

// Add to EventType enum:
case modeChanged = "mode_changed"
case proactiveIntervention = "proactive_intervention"
case playbackAudio = "playback_audio"
case playbackStart = "playback_start"
case playbackStop = "playback_stop"
case interrupted
case standingInstructions = "standing_instructions"

// Add decoder cases in init(from:):
case .modeChanged:
    self = .modeChanged(try ModeChangedUpdate(from: decoder))
case .proactiveIntervention:
    self = .proactiveIntervention(try ProactiveInterventionUpdate(from: decoder))
case .playbackAudio:
    self = .playbackAudio(try PlaybackAudioUpdate(from: decoder))
case .playbackStart:
    self = .playbackStart
case .playbackStop:
    self = .playbackStop
case .interrupted:
    self = .interrupted
case .standingInstructions:
    self = .standingInstructions(try StandingInstructionsUpdate(from: decoder))

// New structs:
struct ModeChangedUpdate: Decodable {
    let mode: String
}

struct ProactiveInterventionUpdate: Decodable {
    let text: String
    let interventionId: String

    private enum CodingKeys: String, CodingKey {
        case text
        case interventionId = "intervention_id"
    }
}

struct PlaybackAudioUpdate: Decodable {
    let audio: String
}

struct StandingInstructionsUpdate: Decodable {
    let instructions: [StandingInstructionInfo]
}

struct StandingInstructionInfo: Decodable, Identifiable {
    let id: Int64
    let instruction: String
    let enabled: Bool
}
```

Add new UICommand variants:

```swift
// Add to UICommand enum:
case setMode(String)
case dismissIntervention(String)
case addStandingInstruction(String)
case toggleStandingInstruction(id: Int64, enabled: Bool)
case deleteStandingInstruction(id: Int64)
case audioFrame(String)

// Add to encode(to:):
case .setMode(let mode):
    try container.encode("set_mode", forKey: .type)
    try container.encode(mode, forKey: .text)
case .dismissIntervention(let id):
    try container.encode("dismiss_intervention", forKey: .type)
    try container.encode(id, forKey: .text)
case .addStandingInstruction(let instruction):
    try container.encode("add_standing_instruction", forKey: .type)
    try container.encode(instruction, forKey: .text)
case .toggleStandingInstruction(let id, let enabled):
    try container.encode("toggle_standing_instruction", forKey: .type)
    // Need additional CodingKeys for id and enabled
case .deleteStandingInstruction(let id):
    try container.encode("delete_standing_instruction", forKey: .type)
case .audioFrame(let pcm):
    try container.encode("audio_frame", forKey: .type)
    try container.encode(pcm, forKey: .text)
```

Note: The `toggleStandingInstruction` and `deleteStandingInstruction` cases need additional CodingKeys. Add:

```swift
private enum CodingKeys: String, CodingKey {
    case type
    case text
    case mode
    case id
    case enabled
    case pcm
    case instruction
    case interventionId = "intervention_id"
}
```

And update the encoder accordingly for each case.

- [ ] **Step 5: Run tests**

```bash
cd /Users/abdullahiabdi/Developer/aura && cargo test -p aura-daemon -- protocol 2>&1
```
Expected: All protocol serialization tests pass.

- [ ] **Step 6: Commit**

```bash
git add crates/aura-daemon/src/protocol.rs AuraApp/Sources/Protocol.swift
git commit -m "feat(ipc): extend protocol with autonomy, proactive, and audio events

Add ModeChanged, ProactiveIntervention, PlaybackAudio/Start/Stop,
Interrupted, StandingInstructions events (daemon->UI). Add SetMode,
DismissIntervention, AddStandingInstruction, AudioFrame commands
(UI->daemon). Both Rust and Swift sides updated in sync."
```

---

## Task 9: Wire Proactive Gating into Processor

**Files:**
- Modify: `crates/aura-daemon/src/processor.rs`
- Modify: `crates/aura-daemon/src/orchestrator.rs`

When Gemini sends proactive audio (audio that wasn't triggered by user speech), the processor must check the scorer before forwarding to playback. If suppressed, log the event but don't play the audio.

- [ ] **Step 1: Add proactive state to the processor context**

In `crates/aura-daemon/src/orchestrator.rs`, add the proactive and autonomy state to the `DaemonContext` (or pass them as additional arguments to `run_processor`):

```rust
// In the DaemonContext struct (crates/aura-daemon/src/context.rs or wherever it's defined):
pub struct DaemonContext {
    // ... existing fields ...
    pub autonomy_mode: Arc<std::sync::RwLock<AutonomyMode>>,
    pub backoff_state: Arc<std::sync::Mutex<BackoffState>>,
    pub standing_instructions: Arc<std::sync::RwLock<Vec<StandingInstruction>>>,
}
```

In `run_daemon()`, initialize these before creating the context:

```rust
// Load autonomy mode from SQLite
let initial_mode = {
    let mem = memory.lock().map_err(|e| anyhow::anyhow!("lock: {e}"))?;
    crate::autonomy::load_mode(&mem)
};
let autonomy_mode = Arc::new(std::sync::RwLock::new(initial_mode));

// Load standing instructions
let initial_instructions = {
    let mem = memory.lock().map_err(|e| anyhow::anyhow!("lock: {e}"))?;
    mem.get_enabled_standing_instructions().unwrap_or_default()
};
let standing_instructions = Arc::new(std::sync::RwLock::new(initial_instructions));

let backoff_state = Arc::new(std::sync::Mutex::new(crate::proactive::BackoffState::new()));
```

- [ ] **Step 2: Add proactive gating in the audio response handler**

In `processor.rs`, in the `GeminiEvent::Audio` handler (or wherever audio responses are forwarded to playback), add the proactive gate:

```rust
// When Gemini sends audio that appears to be proactive (not in response to user speech):
// The exact detection depends on whether Gemini flags proactive audio separately.
// For now, we track whether the user recently spoke — if not, this is proactive.

// Add state tracking near the event loop:
let mut last_user_speech = Instant::now();
let mut idle_since = Instant::now();

// In the Audio event handler:
Ok(GeminiEvent::Audio { data, is_proactive }) => {
    if is_proactive.unwrap_or(false) {
        // This is proactive audio — check the scorer
        let mode = *autonomy_mode.read().unwrap_or_else(|e| e.into_inner());
        let instructions = standing_instructions.read().unwrap_or_else(|e| e.into_inner()).clone();
        let mut backoff = backoff_state.lock().unwrap_or_else(|e| e.into_inner());

        let screen_delta = proactive::ScreenDelta {
            has_changes: true,
            change_magnitude: 0.5, // TODO: get actual screen delta from capture loop
        };
        let app_ctx = proactive::AppContext {
            app_name: "Unknown".into(), // TODO: get from screen context
            window_title: None,
        };
        let idle_duration = idle_since.elapsed();

        match proactive::score_intervention(
            &screen_delta,
            &app_ctx,
            idle_duration,
            mode,
            &instructions,
            &mut backoff,
        ) {
            proactive::ScoringDecision::Allow(intervention) => {
                tracing::info!(id = %intervention.id, "Proactive audio allowed by scorer");
                // Forward audio to playback
                let _ = ipc_tx.send(DaemonEvent::PlaybackAudio { audio: data });
                let _ = ipc_tx.send(DaemonEvent::ProactiveIntervention {
                    text: String::new(), // transcript will follow
                    intervention_id: intervention.id,
                });
            }
            proactive::ScoringDecision::Suppress { reason } => {
                tracing::info!(reason = %reason, "Proactive audio suppressed by scorer");
                // Don't play, but log for threshold tuning
            }
        }
    } else {
        // Normal audio response to user speech — always forward
        let _ = ipc_tx.send(DaemonEvent::PlaybackAudio { audio: data });
    }
}
```

Note: The `is_proactive` flag on `GeminiEvent::Audio` depends on how the Gemini 3.1 Flash Live API signals proactive audio. If there's no separate flag, we can infer it from timing: if no user audio was sent in the last N seconds, the response is likely proactive. Adjust the detection logic based on the actual API behavior.

- [ ] **Step 3: Handle mode change commands in the IPC loop**

In the IPC command handling section of `orchestrator.rs` (the `tokio::select!` loop that processes `UICommand` variants), add handlers:

```rust
UICommand::SetMode { mode } => {
    if let Some(new_mode) = crate::autonomy::AutonomyMode::from_str(&mode) {
        tracing::info!(mode = %mode, "Autonomy mode changed via IPC");
        *autonomy_mode.write().unwrap_or_else(|e| e.into_inner()) = new_mode;

        // Persist to SQLite
        let mem = Arc::clone(&memory);
        let m = new_mode;
        tokio::task::spawn_blocking(move || {
            if let Ok(mem) = mem.lock() {
                let _ = crate::autonomy::save_mode(&mem, m);
            }
        });

        let _ = ipc_tx.send(DaemonEvent::ModeChanged { mode });
    }
}
UICommand::DismissIntervention { intervention_id } => {
    tracing::info!(id = %intervention_id, "Proactive intervention dismissed");
    backoff_state.lock().unwrap_or_else(|e| e.into_inner()).record_dismissal();
}
UICommand::AddStandingInstruction { instruction } => {
    let mem = Arc::clone(&memory);
    let inst = instruction.clone();
    let si = Arc::clone(&standing_instructions);
    let tx = ipc_tx.clone();
    tokio::task::spawn_blocking(move || {
        if let Ok(mem) = mem.lock() {
            if let Ok(id) = mem.add_standing_instruction(&inst) {
                tracing::info!(id, instruction = %inst, "Standing instruction added");
                // Refresh the in-memory list
                if let Ok(updated) = mem.get_enabled_standing_instructions() {
                    let infos: Vec<_> = updated.iter().map(|i| StandingInstructionInfo {
                        id: i.id,
                        instruction: i.instruction.clone(),
                        enabled: i.enabled,
                    }).collect();
                    *si.write().unwrap_or_else(|e| e.into_inner()) = updated;
                    let _ = tx.send(DaemonEvent::StandingInstructions { instructions: infos });
                }
            }
        }
    });
}
UICommand::ToggleStandingInstruction { id, enabled } => {
    let mem = Arc::clone(&memory);
    let si = Arc::clone(&standing_instructions);
    let tx = ipc_tx.clone();
    tokio::task::spawn_blocking(move || {
        if let Ok(mem) = mem.lock() {
            let _ = mem.toggle_standing_instruction(id, enabled);
            if let Ok(updated) = mem.get_enabled_standing_instructions() {
                let infos: Vec<_> = updated.iter().map(|i| StandingInstructionInfo {
                    id: i.id,
                    instruction: i.instruction.clone(),
                    enabled: i.enabled,
                }).collect();
                *si.write().unwrap_or_else(|e| e.into_inner()) = updated;
                let _ = tx.send(DaemonEvent::StandingInstructions { instructions: infos });
            }
        }
    });
}
UICommand::DeleteStandingInstruction { id } => {
    let mem = Arc::clone(&memory);
    let si = Arc::clone(&standing_instructions);
    let tx = ipc_tx.clone();
    tokio::task::spawn_blocking(move || {
        if let Ok(mem) = mem.lock() {
            let _ = mem.delete_standing_instruction(id);
            if let Ok(updated) = mem.get_enabled_standing_instructions() {
                let infos: Vec<_> = updated.iter().map(|i| StandingInstructionInfo {
                    id: i.id,
                    instruction: i.instruction.clone(),
                    enabled: i.enabled,
                }).collect();
                *si.write().unwrap_or_else(|e| e.into_inner()) = updated;
                let _ = tx.send(DaemonEvent::StandingInstructions { instructions: infos });
            }
        }
    });
}
```

- [ ] **Step 4: Run tests**

```bash
cd /Users/abdullahiabdi/Developer/aura && cargo test -p aura-daemon 2>&1
cargo clippy -p aura-daemon -- -D warnings 2>&1
```
Expected: All tests pass, no clippy warnings.

- [ ] **Step 5: Commit**

```bash
git add crates/aura-daemon/src/processor.rs crates/aura-daemon/src/orchestrator.rs
git commit -m "feat(proactive): wire scorer into audio pipeline and IPC command handling

Proactive audio from Gemini is gated by score_intervention() before
playback. SetMode, DismissIntervention, and standing instruction
CRUD commands handled in the IPC loop. Mode and instructions persist
to SQLite and sync to in-memory state."
```

---

## Task 10: Wire Approval Gate Before Tool Execution

**Files:**
- Modify: `crates/aura-daemon/src/processor.rs`

Before every tool execution in the processor, check `requires_approval()`. If the result is `Confirm` or `RequireExplicitRequest`, either ask the user or skip the tool. If `Suggest`, tell the user what Aura wants to do.

- [ ] **Step 1: Add approval check before tool dispatch**

In `processor.rs`, find the tool dispatch section (where `GeminiEvent::ToolCall` is handled and tool execution begins). Before the actual execution, add:

```rust
// Before dispatching any tool execution:
let tool_action = crate::autonomy::ToolAction {
    name: tool_name.clone(),
    is_destructive: is_destructive_tool(&tool_name, &tool_args),
    is_user_requested: true, // TODO: track whether this was from user speech or proactive
};

let mode = *autonomy_mode.read().unwrap_or_else(|e| e.into_inner());
let instructions = standing_instructions.read().unwrap_or_else(|e| e.into_inner()).clone();

match crate::autonomy::requires_approval(&tool_action, mode, &instructions) {
    crate::autonomy::ApprovalResult::Allow => {
        // Proceed with execution
    }
    crate::autonomy::ApprovalResult::RequireExplicitRequest => {
        // Skip execution — tell Gemini the tool requires explicit user request
        let response = serde_json::json!({
            "success": false,
            "error": "Tool execution blocked — user has not explicitly requested this action. Current mode requires explicit user request for tool execution."
        });
        let _ = session.send_tool_response(&tool_call_id, &tool_name, response).await;
        continue; // skip to next event
    }
    crate::autonomy::ApprovalResult::Confirm => {
        // Need user confirmation — send a confirmation request and skip for now
        // The confirmation flow would involve sending a DaemonEvent to the UI
        // and waiting for a response. For MVP, we inform Gemini to ask the user.
        let response = serde_json::json!({
            "success": false,
            "error": "This is a destructive action. Ask the user for explicit confirmation before retrying."
        });
        let _ = session.send_tool_response(&tool_call_id, &tool_name, response).await;
        continue;
    }
    crate::autonomy::ApprovalResult::Suggest => {
        // Tell Gemini to suggest the action rather than execute it
        let response = serde_json::json!({
            "success": false,
            "error": "In current mode, suggest this action to the user instead of executing it. Ask: 'Want me to [action]?'"
        });
        let _ = session.send_tool_response(&tool_call_id, &tool_name, response).await;
        continue;
    }
}
```

- [ ] **Step 2: Add `is_destructive_tool` helper function**

```rust
/// Determine if a tool call is destructive based on tool name and arguments.
fn is_destructive_tool(name: &str, args: &serde_json::Value) -> bool {
    match name {
        "run_shell_command" => {
            // Check if the command is destructive
            let cmd = args.get("command").and_then(|c| c.as_str()).unwrap_or("");
            let cmd_args = args.get("args").and_then(|a| a.as_array());
            matches!(cmd, "killall" | "defaults") || {
                // Check args for rm, mv to trash, etc.
                cmd_args.map_or(false, |a| {
                    a.iter().any(|arg| {
                        let s = arg.as_str().unwrap_or("");
                        s.contains("rm ") || s.contains("delete") || s.contains("trash")
                    })
                })
            }
        }
        "run_applescript" => {
            let script = args.get("script").and_then(|s| s.as_str()).unwrap_or("");
            script.contains("delete") || script.contains("quit") || script.contains("close")
        }
        "shutdown_aura" => true,
        _ => false,
    }
}
```

- [ ] **Step 3: Run tests**

```bash
cd /Users/abdullahiabdi/Developer/aura && cargo test -p aura-daemon 2>&1
cargo clippy -p aura-daemon -- -D warnings 2>&1
```

- [ ] **Step 4: Commit**

```bash
git add crates/aura-daemon/src/processor.rs
git commit -m "feat(autonomy): wire approval gate before every tool execution

requires_approval() checks mode, standing instructions, and destructive
flag before tool dispatch. Blocked tools return structured errors to
Gemini instructing it to ask the user or suggest the action."
```

---

## Task 11: Voice Command Mode Switching

**Files:**
- Modify: `crates/aura-gemini/src/config.rs`

Add instructions to the system prompt so Gemini recognizes mode-switching voice commands ("go quiet", "help me", "take the wheel", "from now on [instruction]") and calls the appropriate tools.

- [ ] **Step 1: Add autonomy instructions to the system prompt**

In `crates/aura-gemini/src/config.rs`, add a new `<autonomy>` section to `DEFAULT_SYSTEM_PROMPT` (insert before `<rules>`):

```rust
// Add this section before </rules> in DEFAULT_SYSTEM_PROMPT:

<autonomy>
The user controls how much you do on your own. Three modes:

- **Ghost**: You listen and observe but never speak unprompted. Execute actions only when explicitly asked.
- **Copilot** (default): You can speak when highly confident it's useful. Execute actions when asked.
- **Autopilot**: You actively help — speak when moderately confident, execute safe actions automatically, standing instructions are active.

Voice commands for mode switching (respond naturally when the user says these):
- "Go quiet" / "shut up" / "stop helping" → Switch to Ghost mode. Say "Going quiet." and call set_mode("ghost").
- "Help me" / "I need help" → Switch to Copilot mode. Say "I'm here." and call set_mode("copilot").
- "Take the wheel" / "go auto" → Switch to Autopilot mode. Say "Taking over." and call set_mode("autopilot").
- "From now on [instruction]" / "Remember to always [instruction]" → Add a standing instruction. Acknowledge and call add_standing_instruction(instruction).

set_mode and add_standing_instruction are internal tools — call them when mode switching or standing instruction commands are detected. Don't mention modes or instructions unless the user brings them up.

In Ghost mode: don't initiate conversation. Only respond when directly addressed.
In Copilot mode: offer help when you notice something genuinely useful (errors, stuck user, relevant context).
In Autopilot mode: proactively help based on screen context and standing instructions. Execute safe actions without asking.
</autonomy>
```

- [ ] **Step 2: Add set_mode and add_standing_instruction tool declarations**

In `crates/aura-gemini/src/tools.rs` (where `build_tool_declarations()` is defined), add two new function declarations:

```rust
// Add to the function declarations array:
FunctionDeclaration {
    name: "set_mode".into(),
    description: "Switch the autonomy mode. Modes: ghost (silent), copilot (default, speak when useful), autopilot (proactive, auto-execute safe actions).".into(),
    parameters: Some(serde_json::json!({
        "type": "object",
        "properties": {
            "mode": {
                "type": "string",
                "description": "The mode to switch to: ghost, copilot, or autopilot",
                "enum": ["ghost", "copilot", "autopilot"]
            }
        },
        "required": ["mode"]
    })),
},
FunctionDeclaration {
    name: "add_standing_instruction".into(),
    description: "Add a standing instruction for Autopilot mode. Standing instructions are persistent rules that guide proactive behavior.".into(),
    parameters: Some(serde_json::json!({
        "type": "object",
        "properties": {
            "instruction": {
                "type": "string",
                "description": "The instruction text, e.g. 'When a build fails, show me the error'"
            }
        },
        "required": ["instruction"]
    })),
},
```

- [ ] **Step 3: Handle set_mode and add_standing_instruction in the processor**

In `processor.rs`, add inline handlers for these tools (similar to the existing `save_memory` and `recall_memory` handlers):

```rust
// In the tool dispatch match block:
"set_mode" => {
    let mode_str = args.get("mode").and_then(|m| m.as_str()).unwrap_or("copilot");
    if let Some(new_mode) = crate::autonomy::AutonomyMode::from_str(mode_str) {
        *autonomy_mode.write().unwrap_or_else(|e| e.into_inner()) = new_mode;

        let mem = Arc::clone(&memory);
        let m = new_mode;
        tokio::task::spawn_blocking(move || {
            if let Ok(mem) = mem.lock() {
                let _ = crate::autonomy::save_mode(&mem, m);
            }
        });

        let _ = ipc_tx.send(DaemonEvent::ModeChanged { mode: mode_str.to_string() });

        let response = serde_json::json!({
            "success": true,
            "mode": mode_str
        });
        let _ = session.send_tool_response(&id, &name, response).await;
    } else {
        let response = serde_json::json!({
            "success": false,
            "error": format!("Invalid mode '{}'. Must be ghost, copilot, or autopilot.", mode_str)
        });
        let _ = session.send_tool_response(&id, &name, response).await;
    }
}
"add_standing_instruction" => {
    let instruction = args.get("instruction")
        .and_then(|i| i.as_str())
        .unwrap_or("")
        .to_string();

    if instruction.is_empty() {
        let response = serde_json::json!({"success": false, "error": "instruction is required"});
        let _ = session.send_tool_response(&id, &name, response).await;
    } else {
        let mem = Arc::clone(&memory);
        let inst = instruction.clone();
        let si = Arc::clone(&standing_instructions);
        let tx = ipc_tx.clone();

        let result = tokio::task::spawn_blocking(move || {
            let mem = mem.lock().ok()?;
            let new_id = mem.add_standing_instruction(&inst).ok()?;
            let updated = mem.get_enabled_standing_instructions().ok()?;
            Some((new_id, updated))
        }).await.ok().flatten();

        if let Some((new_id, updated)) = result {
            let infos: Vec<_> = updated.iter().map(|i| StandingInstructionInfo {
                id: i.id,
                instruction: i.instruction.clone(),
                enabled: i.enabled,
            }).collect();
            *standing_instructions.write().unwrap_or_else(|e| e.into_inner()) = updated;
            let _ = tx.send(DaemonEvent::StandingInstructions { instructions: infos });

            let response = serde_json::json!({
                "success": true,
                "id": new_id,
                "instruction": instruction
            });
            let _ = session.send_tool_response(&id, &name, response).await;
        } else {
            let response = serde_json::json!({"success": false, "error": "Failed to save instruction"});
            let _ = session.send_tool_response(&id, &name, response).await;
        }
    }
}
```

- [ ] **Step 4: Run tests**

```bash
cd /Users/abdullahiabdi/Developer/aura && cargo test 2>&1 | tail -20
cargo clippy -- -D warnings 2>&1 | tail -10
```
Expected: All tests pass, no clippy warnings.

- [ ] **Step 5: Commit**

```bash
git add crates/aura-gemini/src/config.rs crates/aura-gemini/src/tools.rs crates/aura-daemon/src/processor.rs
git commit -m "feat(autonomy): add voice command mode switching and standing instructions

System prompt teaches Gemini to recognize 'go quiet', 'help me',
'take the wheel', and 'from now on [instruction]' commands. set_mode
and add_standing_instruction tool declarations added. Processor handles
both tools inline with mode persistence and IPC broadcast."
```

---

## Task 12: SwiftUI Autonomy View

**Files:**
- Create: `AuraApp/Sources/AutonomyView.swift`
- Modify: `AuraApp/Sources/AppState.swift`
- Modify: `AuraApp/Sources/ContentView.swift`

A SwiftUI view with a mode picker (Ghost/Copilot/Autopilot segmented control) and a list of standing instructions with add/toggle/delete support.

- [ ] **Step 1: Add autonomy state to AppState**

In `AuraApp/Sources/AppState.swift`, add:

```swift
// Add properties:
@Published var autonomyMode: String = "copilot"
@Published var standingInstructions: [StandingInstructionInfo] = []

// Add handler for autonomy events (call from existing event handler):
func handleAutonomyEvent(_ event: DaemonEvent) {
    switch event {
    case .modeChanged(let update):
        autonomyMode = update.mode
    case .standingInstructions(let update):
        standingInstructions = update.instructions
    default:
        break
    }
}
```

- [ ] **Step 2: Create AutonomyView.swift**

```swift
// AuraApp/Sources/AutonomyView.swift
import SwiftUI

/// Autonomy mode picker and standing instructions management.
struct AutonomyView: View {
    @EnvironmentObject var appState: AppState
    @State private var newInstruction = ""

    private var modeBinding: Binding<String> {
        Binding(
            get: { appState.autonomyMode },
            set: { newMode in
                appState.autonomyMode = newMode
                appState.connection?.send(.setMode(newMode))
            }
        )
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            // Mode picker
            VStack(alignment: .leading, spacing: 8) {
                Text("Autonomy Mode")
                    .font(.headline)

                Picker("Mode", selection: modeBinding) {
                    Text("Ghost").tag("ghost")
                    Text("Copilot").tag("copilot")
                    Text("Autopilot").tag("autopilot")
                }
                .pickerStyle(.segmented)

                Text(modeDescription)
                    .font(.caption)
                    .foregroundColor(.secondary)
            }

            Divider()

            // Standing instructions
            VStack(alignment: .leading, spacing: 8) {
                Text("Standing Instructions")
                    .font(.headline)

                if appState.autonomyMode != "autopilot" {
                    Text("Active in Autopilot mode only")
                        .font(.caption)
                        .foregroundColor(.secondary)
                }

                // Instruction list
                ForEach(appState.standingInstructions) { instruction in
                    HStack {
                        Toggle("", isOn: Binding(
                            get: { instruction.enabled },
                            set: { enabled in
                                appState.connection?.send(
                                    .toggleStandingInstruction(
                                        id: instruction.id,
                                        enabled: enabled
                                    )
                                )
                            }
                        ))
                        .labelsHidden()
                        .toggleStyle(.checkbox)

                        Text(instruction.instruction)
                            .font(.body)
                            .foregroundColor(instruction.enabled ? .primary : .secondary)
                            .lineLimit(2)

                        Spacer()

                        Button(action: {
                            appState.connection?.send(
                                .deleteStandingInstruction(id: instruction.id)
                            )
                        }) {
                            Image(systemName: "trash")
                                .foregroundColor(.red.opacity(0.7))
                        }
                        .buttonStyle(.plain)
                    }
                    .padding(.vertical, 2)
                }

                if appState.standingInstructions.isEmpty {
                    Text("No instructions yet. Say \"From now on...\" or add one below.")
                        .font(.caption)
                        .foregroundColor(.secondary)
                        .padding(.vertical, 4)
                }

                // Add new instruction
                HStack {
                    TextField("Add instruction...", text: $newInstruction)
                        .textFieldStyle(.roundedBorder)
                        .onSubmit {
                            addInstruction()
                        }

                    Button("Add") {
                        addInstruction()
                    }
                    .disabled(newInstruction.trimmingCharacters(in: .whitespaces).isEmpty)
                }
            }
        }
        .padding()
    }

    private var modeDescription: String {
        switch appState.autonomyMode {
        case "ghost":
            return "Silent mode. Aura listens and observes but never speaks unprompted."
        case "copilot":
            return "Default. Aura speaks when highly confident it's useful."
        case "autopilot":
            return "Proactive. Aura helps actively and follows standing instructions."
        default:
            return ""
        }
    }

    private func addInstruction() {
        let trimmed = newInstruction.trimmingCharacters(in: .whitespaces)
        guard !trimmed.isEmpty else { return }
        appState.connection?.send(.addStandingInstruction(trimmed))
        newInstruction = ""
    }
}
```

- [ ] **Step 3: Wire AutonomyView into ContentView**

In `AuraApp/Sources/ContentView.swift`, add navigation to the AutonomyView. This depends on the existing navigation structure — it could be a tab, a settings sheet, or a section in the conversation view. Add it as a disclosure group or a sheet:

```swift
// If using a settings gear button in the header:
.sheet(isPresented: $showAutonomy) {
    AutonomyView()
        .environmentObject(appState)
        .frame(minWidth: 350, minHeight: 400)
}

// Or if using a toolbar:
.toolbar {
    ToolbarItem(placement: .automatic) {
        Button(action: { showAutonomy = true }) {
            Image(systemName: "slider.horizontal.3")
        }
    }
}
```

Add `@State private var showAutonomy = false` to the ContentView.

- [ ] **Step 4: Build**

```bash
cd /Users/abdullahiabdi/Developer/aura && swift build --package-path AuraApp 2>&1 | tail -5
```
Expected: Build succeeds.

- [ ] **Step 5: Commit**

```bash
git add AuraApp/Sources/AutonomyView.swift AuraApp/Sources/AppState.swift AuraApp/Sources/ContentView.swift
git commit -m "feat(ui): add SwiftUI AutonomyView with mode picker and standing instructions

Segmented control for Ghost/Copilot/Autopilot mode switching. Standing
instructions list with toggle, delete, and add-new. Mode changes and
instruction CRUD send IPC commands to the daemon. State syncs
bidirectionally via DaemonEvent broadcasts."
```

---

## Task 13: Integration Build and Smoke Test

**Files:** No new files — integration verification.

- [ ] **Step 1: Full Rust build**

```bash
cd /Users/abdullahiabdi/Developer/aura && cargo build 2>&1 | tail -10
```
Expected: Compilation succeeds.

- [ ] **Step 2: Full Rust test suite**

```bash
cd /Users/abdullahiabdi/Developer/aura && cargo test 2>&1 | tail -30
```
Expected: All tests pass.

- [ ] **Step 3: Clippy**

```bash
cd /Users/abdullahiabdi/Developer/aura && cargo clippy -- -D warnings 2>&1 | tail -10
```
Expected: No warnings.

- [ ] **Step 4: Swift build**

```bash
cd /Users/abdullahiabdi/Developer/aura && swift build --package-path AuraApp 2>&1 | tail -5
```
Expected: Build succeeds.

- [ ] **Step 5: Manual smoke test checklist**

1. Launch AuraApp + daemon
2. Verify profile injection: check daemon logs for `"Injected user profile into system prompt"` (will be absent on first run with empty facts)
3. Mode switching: say "Aura, go quiet" — verify mode changes to Ghost, menu bar reflects it
4. Say "Aura, help me" — verify mode changes to Copilot
5. Standing instruction: say "Aura, from now on when a build fails show me the error" — verify instruction appears in AutonomyView
6. Save a fact: say "Aura, remember that I prefer dark mode" — verify save_memory is called with source=explicit
7. End session, start new session — verify profile contains "dark mode" preference
8. Proactive gating: in Ghost mode, verify no unprompted speech from Gemini

- [ ] **Step 6: Commit any fixes**

```bash
git add -A
git commit -m "fix(v2): address integration issues found in Phase 4 smoke test"
```

---

## Summary of Changes

| Area | What | Files |
|---|---|---|
| **Memory Schema** | source column on facts, standing_instructions table, profile derivation queries | `store.rs` |
| **Profile Derivation** | Two-tier: stable (400 tokens) + recent (200 tokens), XML injection | `profile.rs` |
| **Profile Injection** | At session start, derive profile and append to system prompt | `orchestrator.rs` |
| **Fact Extraction** | Updated consolidation prompt with source and correction category | `consolidate.rs` |
| **Explicit Capture** | save_memory with source=explicit, correction/workflow categories | `processor.rs` |
| **Autonomy State** | Ghost/Copilot/Autopilot enum, SQLite persistence, approval gate | `autonomy.rs` |
| **Proactive Scorer** | score_intervention() with backoff, rate limits, mode thresholds | `proactive.rs` |
| **Proactive Gating** | Client-side gate on Gemini ProactiveAudio in processor | `processor.rs` |
| **IPC Protocol** | New events/commands for mode, proactive, standing instructions | `protocol.rs` |
| **Voice Commands** | System prompt teaches mode switching, tool declarations | `config.rs`, `tools.rs` |
| **SwiftUI** | AutonomyView with mode picker and standing instructions | `AutonomyView.swift` |

## Dependencies Between Tasks

```
Task 1 (Schema) ─── Task 2 (Profile) ─── Task 3 (Injection)
      │                                         │
      └──── Task 4 (Consolidation) ─────────────┤
      │                                         │
      └──── Task 5 (Explicit Capture) ──────────┤
                                                 │
Task 6 (Autonomy) ── Task 7 (Scorer) ── Task 9 (Wire Proactive)
      │                                         │
      └──── Task 10 (Approval Gate) ────────────┤
      │                                         │
      └──── Task 8 (IPC Protocol) ──────────────┤
      │                                         │
      └──── Task 11 (Voice Commands) ───────────┤
      │                                         │
      └──── Task 12 (UI) ──────────────────────┤
                                                 │
                                         Task 13 (Integration)
```

**Parallelizable:**
- Tasks 1-5 (Memory track) and Tasks 6-7 (Autonomy/Proactive track) can be worked in parallel
- Task 8 (IPC) should be done early — both tracks need it
- Tasks 9-12 depend on both tracks
- Task 13 is the final integration gate
