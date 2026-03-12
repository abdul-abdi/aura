# Hackathon GCP + Session Reconnection + Activity Stream UI — Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Make Aura hackathon-ready with real GCP integration (Firestore + Cloud Run), smart session reconnection, and an Activity Stream UI that shows the agent's brain in real-time.

**Architecture:** SQLite stays as local cache. Firestore becomes durable memory (facts + session summaries) written via a Cloud Run consolidation service. Session reconnection uses resumption handles with a context-overload safety net. The SwiftUI panel replaces chat bubbles with a typed activity stream. Wake word is removed.

**Tech Stack:** Rust (workspace crates), SwiftUI, Firestore REST API, Cloud Run, gcloud CLI, reqwest, serde

**Design doc:** `docs/plans/2026-03-12-hackathon-gcp-ui-design.md`

---

## Task 1: Remove Wake Word

**Files:**
- Delete: `crates/aura-voice/src/wakeword.rs`
- Delete: `crates/aura-voice/tests/wakeword_test.rs`
- Modify: `crates/aura-voice/src/lib.rs:5`
- Modify: `crates/aura-voice/Cargo.toml:11`
- Modify: `crates/aura-daemon/src/event.rs:6` (remove WakeWordDetected variant)

**Step 1: Remove wakeword module export**

In `crates/aura-voice/src/lib.rs`, remove line 5:
```rust
// Remove this line:
pub mod wakeword;
```

**Step 2: Remove rustpotter dependency**

In `crates/aura-voice/Cargo.toml`, remove line 11:
```toml
# Remove this line:
rustpotter = "3"
```

**Step 3: Delete wakeword files**

```bash
rm crates/aura-voice/src/wakeword.rs
rm crates/aura-voice/tests/wakeword_test.rs
```

**Step 4: Remove WakeWordDetected from AuraEvent**

In `crates/aura-daemon/src/event.rs`, remove the `WakeWordDetected` variant (line 6).

**Step 5: Fix any remaining references**

```bash
cargo build --workspace 2>&1 | head -50
```

Fix any compilation errors from removed references to `wakeword` or `WakeWordDetected`.

**Step 6: Run tests**

```bash
cargo test --workspace
```
Expected: All tests pass.

**Step 7: Commit**

```bash
git add -A && git commit -m "refactor: remove wake word — mic always streaming, no trigger phrase needed"
```

---

## Task 2: Add `source` field to IPC Transcript + `summary` to ToolStatus

**Files:**
- Modify: `crates/aura-daemon/src/protocol.rs:14-38` (DaemonEvent enum)
- Modify: `crates/aura-daemon/src/protocol.rs:86-196` (tests)
- Modify: `AuraApp/Sources/Protocol.swift:57-66` (TranscriptUpdate + ToolStatusUpdate)

**Step 1: Update Rust DaemonEvent::Transcript**

In `crates/aura-daemon/src/protocol.rs`, add `source` field to Transcript and `summary` to ToolStatus:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum DaemonEvent {
    DotColor { color: DotColorName, pulsing: bool },

    Transcript {
        role: Role,
        text: String,
        done: bool,
        /// "voice" or "text" — how this transcript originated.
        #[serde(default = "default_source")]
        source: String,
    },

    ToolStatus {
        name: String,
        status: ToolRunStatus,
        #[serde(skip_serializing_if = "Option::is_none")]
        output: Option<String>,
        /// Human-readable one-line summary for the activity stream.
        #[serde(skip_serializing_if = "Option::is_none")]
        summary: Option<String>,
    },

    Status { message: String },
    Shutdown,
}

fn default_source() -> String {
    "voice".into()
}
```

**Step 2: Update all DaemonEvent::Transcript send sites in main.rs**

Search for `DaemonEvent::Transcript` in `crates/aura-daemon/src/main.rs` and add `source: "voice".into()` or `source: "text".into()` depending on origin. User transcripts from Gemini events are `"voice"`, from IPC text input are `"text"`.

**Step 3: Update all DaemonEvent::ToolStatus send sites in main.rs**

Add `summary: Some(...)` with human-readable text. Use the existing `displayName`-style logic to generate summaries like:
- `activate_app("Safari")` → `summary: Some("Safari foregrounded".into())`
- `click_element("Save")` → `summary: Some("Clicked \"Save\"".into())`
- `type_text("hello")` → `summary: Some("Typed \"hello\"".into())`

For now, default to `summary: None` — we'll add smart summaries in the tool dispatch.

**Step 4: Update protocol tests**

Update existing tests to include the new fields. Add test for source field:

```rust
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
```

**Step 5: Update Swift Protocol.swift**

In `AuraApp/Sources/Protocol.swift`, add `source` to `TranscriptUpdate` and `summary` to `ToolStatusUpdate`:

```swift
struct TranscriptUpdate: Decodable {
    let role: TranscriptRole
    let text: String
    let done: Bool
    let source: TranscriptSource

    enum TranscriptSource: String, Decodable {
        case voice
        case text
    }
}

struct ToolStatusUpdate: Decodable {
    let name: String
    let status: ToolRunStatus
    let output: String?
    let summary: String?
}
```

**Step 6: Build and test**

```bash
cargo test --workspace
```

**Step 7: Commit**

```bash
git add -A && git commit -m "feat: add source field to transcripts and summary to tool status for activity stream"
```

---

## Task 3: Session Reconnection Logic (Rust)

**Files:**
- Modify: `crates/aura-gemini/src/session.rs:459-468` (undo handle clearing)
- Modify: `crates/aura-gemini/src/session.rs:76-79` (connect signature)
- Modify: `crates/aura-daemon/src/main.rs:217-291` (reconnect loop)
- Modify: `crates/aura-daemon/src/main.rs:304-362` (run_daemon)
- Modify: `crates/aura-gemini/src/config.rs:5-85` (system prompt variants)

**Step 1: Add SessionMode enum to daemon**

In `crates/aura-daemon/src/main.rs`, add near the top (after imports):

```rust
/// Whether this session resumes a previous Gemini context or starts fresh.
#[derive(Debug, Clone)]
enum SessionMode {
    /// Resume previous session via handle — no greeting.
    Resume { handle: String },
    /// Fresh session — Aura greets and introduces itself.
    Fresh,
}
```

**Step 2: Update reconnect loop to determine SessionMode**

In `crates/aura-daemon/src/main.rs`, replace the reconnect loop (lines 217-291) to:
1. Track `reconnect_counter: u32` before the loop
2. On each iteration, determine SessionMode:
   - If reconnect signal from UI → attempt `Resume` with stored handle
   - If auto-reconnect or no handle → `Fresh`
   - If `reconnect_counter >= 3` → force `Fresh`
3. Pass `session_mode` to `run_daemon()`

**Step 3: Update run_daemon to accept SessionMode**

Add `session_mode: SessionMode` parameter to `run_daemon()`. Use it to:
- `Resume { handle }` → pass `Some(handle)` to `GeminiLiveSession::connect()`
- `Fresh` → pass `None`

Also append session-mode-specific hint to system prompt:
- Resume: `"\n\nThis is a resumed session. Continue naturally — no greeting, no reintroduction."`
- Fresh: `"\n\nThis is a new session. Greet the user briefly and naturally introduce yourself. You have memory of past interactions — reference relevant facts naturally if appropriate."`

**Step 4: Undo handle clearing in session.rs**

In `crates/aura-gemini/src/session.rs:459-468`, replace the block that clears the handle. Instead, use the handle if available:

```rust
// Replace:
// {
//     let mut handle = state.resumption_handle.lock().await;
//     if handle.is_some() {
//         tracing::info!("Clearing resumption handle for fresh session...");
//         *handle = None;
//     }
// }
// let setup = build_setup_message(&state.config, None);

// With:
let current_handle = {
    let handle = state.resumption_handle.lock().await;
    handle.clone()
};
let setup = build_setup_message(&state.config, current_handle);
```

**Step 5: Add context overload safety net**

In `crates/aura-daemon/src/main.rs`, after `run_daemon()` returns:
- If session lasted < 30 seconds AND mode was Resume → increment `poison_counter`
- If `poison_counter >= 2` → clear handle from SQLite, force Fresh next time
- If session lasted >= 30 seconds → reset `poison_counter`, increment `reconnect_counter`

**Step 6: Run tests**

```bash
cargo test --workspace
```

**Step 7: Commit**

```bash
git add -A && git commit -m "feat: smart session reconnection — resume with handle, fresh with greeting, context overload safety net"
```

---

## Task 4: Activity Stream UI (SwiftUI)

**Files:**
- Rewrite: `AuraApp/Sources/MessageBubble.swift` → `ActivityRow.swift`
- Modify: `AuraApp/Sources/ConversationView.swift` (use new ActivityRow + reconnect banner)
- Modify: `AuraApp/Sources/ContentView.swift` (reconnect banner integration)
- Modify: `AuraApp/Sources/AppState.swift` (new message model, source handling)
- Modify: `AuraApp/Sources/Protocol.swift` (ChatMessage refactor)

**Step 1: Refactor ChatMessage to ActivityEvent**

In `AuraApp/Sources/Protocol.swift`, replace `ChatMessage` and `MessageRole`:

```swift
/// A single event in the activity stream.
struct ActivityEvent: Identifiable {
    let id: UUID
    let kind: EventKind
    var text: String
    let timestamp: Date

    init(kind: EventKind, text: String) {
        self.id = UUID()
        self.kind = kind
        self.text = text
        self.timestamp = Date()
    }
}

enum EventKind: Equatable {
    case userSpeech          // 🎤 voice transcript
    case userText            // 💬 typed message
    case assistantSpeech     // 🔊 what Aura said
    case toolCall(ToolRunStatus)  // ⚡ tool execution
    case turnSeparator       // ─ ─ visual break
}
```

**Step 2: Create ActivityRow view**

Replace `MessageBubble.swift` content with `ActivityRow.swift` (keep filename or rename):

```swift
struct ActivityRow: View {
    let event: ActivityEvent
    @State private var isExpanded = false

    var body: some View {
        switch event.kind {
        case .userSpeech:
            iconRow(symbol: "mic.fill", color: .primary, quoted: true)

        case .userText:
            iconRow(symbol: "text.bubble.fill", color: .primary, quoted: true)

        case .assistantSpeech:
            iconRow(symbol: "speaker.wave.2.fill", color: .primary, quoted: true)

        case .toolCall(let status):
            toolRow(status: status)

        case .turnSeparator:
            separatorRow
        }
    }

    private func iconRow(symbol: String, color: Color, quoted: Bool) -> some View {
        HStack(alignment: .top, spacing: 8) {
            Image(systemName: symbol)
                .font(.system(size: 11))
                .foregroundStyle(.secondary)
                .frame(width: 16)
            Text(quoted ? "\"\(event.text)\"" : event.text)
                .font(.system(size: 13))
                .foregroundStyle(color)
                .textSelection(.enabled)
        }
        .padding(.vertical, 2)
    }

    private func toolRow(status: ToolRunStatus) -> some View {
        VStack(alignment: .leading, spacing: 2) {
            HStack(spacing: 6) {
                Image(systemName: status.symbolName)
                    .font(.system(size: 11))
                    .foregroundStyle(status.color)
                    .frame(width: 16)
                // Tool name is first line of text
                Text(event.text.components(separatedBy: "\n").first ?? event.text)
                    .font(.system(size: 12, design: .monospaced))
                    .foregroundStyle(status == .running ? Color.primary : .secondary)
            }

            // Result (second line) — indented, dimmed
            if status != .running,
               let result = event.text.components(separatedBy: "\n").dropFirst().first,
               !result.isEmpty {
                HStack(spacing: 6) {
                    Color.clear.frame(width: 16) // indent to align with text
                    Text(result)
                        .font(.system(size: 11))
                        .foregroundStyle(.tertiary)
                        .lineLimit(isExpanded ? nil : 1)
                        .textSelection(.enabled)
                }
                .onTapGesture { isExpanded.toggle() }
            }
        }
        .padding(.vertical, 1)
    }

    private var separatorRow: some View {
        HStack {
            Rectangle()
                .fill(Color.secondary.opacity(0.15))
                .frame(height: 0.5)
        }
        .padding(.vertical, 6)
    }
}
```

**Step 3: Update ConversationView with reconnect banner**

In `AuraApp/Sources/ConversationView.swift`, add reconnect banner and use ActivityRow:

```swift
struct ConversationView: View {
    let events: [ActivityEvent]
    let connectionState: AppState.ConnectionState
    let isThinking: Bool
    let onReconnect: () -> Void

    var body: some View {
        ScrollViewReader { proxy in
            ScrollView(.vertical, showsIndicators: true) {
                VStack(spacing: 0) {
                    // Reconnect banner
                    if connectionState == .disconnected {
                        reconnectBanner
                    }

                    if events.isEmpty && !isThinking {
                        emptyState
                    } else {
                        LazyVStack(spacing: 4) {
                            ForEach(events) { event in
                                ActivityRow(event: event)
                                    .id(event.id)
                                    .transition(.asymmetric(
                                        insertion: .move(edge: .bottom).combined(with: .opacity),
                                        removal: .opacity
                                    ))
                            }

                            if isThinking {
                                TypingIndicator()
                                    .id("typing-indicator")
                                    .transition(.opacity.combined(with: .move(edge: .bottom)))
                            }
                        }
                        .padding(.horizontal, 12)
                        .padding(.vertical, 8)
                    }
                }
            }
            .onChange(of: events.count) { _, _ in
                if let last = events.last {
                    withAnimation(.spring(response: 0.35, dampingFraction: 0.8)) {
                        proxy.scrollTo(last.id, anchor: .bottom)
                    }
                }
            }
            .onChange(of: isThinking) { _, thinking in
                if thinking {
                    withAnimation(.spring(response: 0.35, dampingFraction: 0.8)) {
                        proxy.scrollTo("typing-indicator", anchor: .bottom)
                    }
                }
            }
        }
    }

    private var reconnectBanner: some View {
        Button(action: onReconnect) {
            HStack(spacing: 8) {
                Image(systemName: "arrow.trianglehead.2.counterclockwise")
                    .font(.system(size: 12, weight: .medium))
                Text("Connection lost. Reconnect")
                    .font(.system(size: 12, weight: .medium))
            }
            .foregroundStyle(.white)
            .frame(maxWidth: .infinity)
            .padding(.vertical, 10)
            .background(
                Color(red: 1.0, green: 0.78, blue: 0.28).opacity(0.9),
                in: RoundedRectangle(cornerRadius: 8, style: .continuous)
            )
        }
        .buttonStyle(.plain)
        .padding(.horizontal, 12)
        .padding(.top, 8)
    }

    // ... keep emptyState and TypingIndicator unchanged
}
```

**Step 4: Update AppState to use ActivityEvent**

In `AuraApp/Sources/AppState.swift`:
- Replace `var messages: [ChatMessage]` with `var events: [ActivityEvent]`
- Update `handleTranscript` to create events based on `source`:
  - `source == .voice && role == .user` → `.userSpeech`
  - `source == .text && role == .user` → `.userText`
  - `role == .assistant` → `.assistantSpeech`
- Update `handleToolStatus` to create `.toolCall` events with summary as result line
- Update `sendText` to append `.userText` event
- Add turn separator logic: when assistant turn completes (`done: true`) and next event is a user event, insert `.turnSeparator`
- Update `handleDisconnect` to set `connectionState = .disconnected` (not `.connecting`)

**Step 5: Update ContentView**

In `AuraApp/Sources/ContentView.swift`, update `mainContent` to pass `onReconnect` and use `events`:

```swift
ConversationView(
    events: appState.events,
    connectionState: appState.connectionState,
    isThinking: appState.isThinking,
    onReconnect: { appState.requestReconnect() }
)
```

**Step 6: Build the SwiftUI app**

```bash
./scripts/dev.sh
```

**Step 7: Commit**

```bash
git add -A && git commit -m "feat: activity stream UI — typed event feed with reconnect banner, replaces chat bubbles"
```

---

## Task 5: Firestore Crate (`aura-firestore`)

**Files:**
- Create: `crates/aura-firestore/Cargo.toml`
- Create: `crates/aura-firestore/src/lib.rs`
- Create: `crates/aura-firestore/src/client.rs`
- Create: `crates/aura-firestore/src/auth.rs`
- Create: `crates/aura-firestore/tests/client_test.rs`
- Modify: `Cargo.toml` (workspace members)

**Step 1: Create crate structure**

```bash
mkdir -p crates/aura-firestore/src crates/aura-firestore/tests
```

**Step 2: Create Cargo.toml**

```toml
[package]
name = "aura-firestore"
version.workspace = true
edition.workspace = true

[dependencies]
tokio.workspace = true
tracing.workspace = true
anyhow.workspace = true
serde.workspace = true
serde_json.workspace = true
reqwest = { version = "0.12", features = ["json"] }
```

**Step 3: Create auth.rs — Firebase anonymous auth**

```rust
//! Firebase anonymous authentication for Firestore REST access.

use anyhow::{Context, Result};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct AnonAuthResponse {
    #[serde(rename = "idToken")]
    id_token: String,
}

/// Get a Firebase anonymous auth token using the API key.
pub async fn get_anonymous_token(api_key: &str) -> Result<String> {
    let url = format!(
        "https://identitytoolkit.googleapis.com/v1/accounts:signUp?key={api_key}"
    );
    let client = reqwest::Client::new();
    let resp: AnonAuthResponse = client
        .post(&url)
        .json(&serde_json::json!({"returnSecureToken": true}))
        .send()
        .await
        .context("Firebase anonymous auth request failed")?
        .json()
        .await
        .context("Failed to parse Firebase auth response")?;
    Ok(resp.id_token)
}
```

**Step 4: Create client.rs — Firestore REST client**

```rust
//! Thin Firestore REST client for reading/writing facts and session summaries.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

const FIRESTORE_BASE: &str = "https://firestore.googleapis.com/v1";

pub struct FirestoreClient {
    project_id: String,
    device_id: String,
    client: reqwest::Client,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct FirestoreFact {
    pub category: String,
    pub content: String,
    pub entities: Vec<String>,
    pub importance: f64,
    pub session_id: String,
}

impl FirestoreClient {
    pub fn new(project_id: String, device_id: String) -> Self {
        Self {
            project_id,
            device_id,
            client: reqwest::Client::new(),
        }
    }

    fn collection_url(&self, collection: &str) -> String {
        format!(
            "{FIRESTORE_BASE}/projects/{}/databases/(default)/documents/users/{}/{}",
            self.project_id, self.device_id, collection
        )
    }

    /// Write a fact to Firestore.
    pub async fn write_fact(&self, fact: &FirestoreFact, auth_token: &str) -> Result<()> {
        let url = self.collection_url("facts");
        let doc = fact_to_firestore_doc(fact);
        self.client
            .post(&url)
            .bearer_auth(auth_token)
            .json(&doc)
            .send()
            .await
            .context("Firestore write_fact request failed")?;
        Ok(())
    }

    /// Write a session summary to Firestore.
    pub async fn write_session(
        &self,
        session_id: &str,
        summary: &str,
        auth_token: &str,
    ) -> Result<()> {
        let url = format!("{}/sessions/{session_id}", self.collection_url("sessions").rsplit_once('/').unwrap().0);
        // Use patch to create-or-update
        let doc = serde_json::json!({
            "fields": {
                "summary": {"stringValue": summary},
                "updated_at": {"stringValue": chrono::Utc::now().to_rfc3339()}
            }
        });
        self.client
            .patch(&url)
            .bearer_auth(auth_token)
            .json(&doc)
            .send()
            .await
            .context("Firestore write_session failed")?;
        Ok(())
    }

    /// Read all facts from Firestore for this device.
    pub async fn read_facts(&self, auth_token: &str) -> Result<Vec<FirestoreFact>> {
        let url = self.collection_url("facts");
        let resp: serde_json::Value = self
            .client
            .get(&url)
            .bearer_auth(auth_token)
            .send()
            .await
            .context("Firestore read_facts failed")?
            .json()
            .await
            .context("Failed to parse Firestore response")?;

        let docs = resp
            .get("documents")
            .and_then(|d| d.as_array())
            .cloned()
            .unwrap_or_default();

        let facts: Vec<FirestoreFact> = docs
            .iter()
            .filter_map(|doc| firestore_doc_to_fact(doc))
            .collect();

        Ok(facts)
    }
}

fn fact_to_firestore_doc(fact: &FirestoreFact) -> serde_json::Value {
    serde_json::json!({
        "fields": {
            "category": {"stringValue": &fact.category},
            "content": {"stringValue": &fact.content},
            "entities": {"arrayValue": {"values": fact.entities.iter().map(|e| serde_json::json!({"stringValue": e})).collect::<Vec<_>>()}},
            "importance": {"doubleValue": fact.importance},
            "session_id": {"stringValue": &fact.session_id},
        }
    })
}

fn firestore_doc_to_fact(doc: &serde_json::Value) -> Option<FirestoreFact> {
    let fields = doc.get("fields")?;
    Some(FirestoreFact {
        category: fields.get("category")?.get("stringValue")?.as_str()?.into(),
        content: fields.get("content")?.get("stringValue")?.as_str()?.into(),
        entities: fields
            .get("entities")
            .and_then(|e| e.get("arrayValue"))
            .and_then(|a| a.get("values"))
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.get("stringValue").and_then(|s| s.as_str()).map(String::from))
                    .collect()
            })
            .unwrap_or_default(),
        importance: fields
            .get("importance")
            .and_then(|i| i.get("doubleValue"))
            .and_then(|d| d.as_f64())
            .unwrap_or(0.5),
        session_id: fields.get("session_id")?.get("stringValue")?.as_str()?.into(),
    })
}
```

**Step 5: Create lib.rs**

```rust
pub mod auth;
pub mod client;
```

**Step 6: Add to workspace**

In root `Cargo.toml`, add `"crates/aura-firestore"` to the members list.

**Step 7: Write basic tests**

```rust
// crates/aura-firestore/tests/client_test.rs
use aura_firestore::client::{FirestoreClient, FirestoreFact};

#[test]
fn fact_roundtrip_serialization() {
    let fact = FirestoreFact {
        category: "preference".into(),
        content: "User prefers dark mode".into(),
        entities: vec!["dark mode".into()],
        importance: 0.8,
        session_id: "test-session".into(),
    };
    let json = serde_json::to_string(&fact).unwrap();
    let parsed: FirestoreFact = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.category, "preference");
    assert_eq!(parsed.entities.len(), 1);
}
```

**Step 8: Build and test**

```bash
cargo test --workspace
```

**Step 9: Commit**

```bash
git add -A && git commit -m "feat: add aura-firestore crate — Firestore REST client for facts and session summaries"
```

---

## Task 6: Cloud Run Consolidation Service

**Files:**
- Create: `infrastructure/consolidation/Cargo.toml`
- Create: `infrastructure/consolidation/src/main.rs`
- Create: `infrastructure/Dockerfile`
- Modify: `Cargo.toml` (workspace members — add infrastructure/consolidation)

**Step 1: Create consolidation service**

This is a standalone Rust HTTP server that:
1. Receives POST `/consolidate` with session messages
2. Calls Gemini REST API to extract facts (reuses logic from `consolidate.rs`)
3. Writes facts + session summary to Firestore
4. Returns the extracted facts to the caller

```bash
mkdir -p infrastructure/consolidation/src
```

**Step 2: Create Cargo.toml**

```toml
[package]
name = "aura-consolidation"
version = "0.1.0"
edition = "2024"

[dependencies]
tokio = { version = "1", features = ["full"] }
axum = "0.8"
serde = { version = "1", features = ["derive"] }
serde_json = "1"
reqwest = { version = "0.12", features = ["json"] }
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter"] }
anyhow = "1"
tower-http = { version = "0.6", features = ["cors"] }
subtle = "2"
sha2 = "0.10"
```

Note: This is a standalone crate, NOT added to the workspace (it deploys independently to Cloud Run).

**Step 3: Create main.rs**

The consolidation service with `/consolidate` and `/health` endpoints. Reads env vars: `GEMINI_API_KEY`, `AURA_AUTH_TOKEN`, `GCP_PROJECT_ID`, `PORT`.

Reuses the same prompt-building logic from `crates/aura-memory/src/consolidate.rs` but calls Firestore after extraction.

**Step 4: Create Dockerfile**

```dockerfile
FROM rust:1.85-bookworm AS builder
WORKDIR /app
COPY infrastructure/consolidation/ .
RUN cargo build --release

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y ca-certificates && rm -rf /var/lib/apt/lists/*
COPY --from=builder /app/target/release/aura-consolidation /usr/local/bin/
ENV PORT=8080
EXPOSE 8080
CMD ["aura-consolidation"]
```

**Step 5: Build locally to verify**

```bash
cd infrastructure/consolidation && cargo build && cd ../..
```

**Step 6: Commit**

```bash
git add -A && git commit -m "feat: Cloud Run consolidation service — extracts facts and writes to Firestore"
```

---

## Task 7: Integrate Firestore into Daemon

**Files:**
- Modify: `crates/aura-daemon/Cargo.toml` (add aura-firestore dependency)
- Modify: `crates/aura-daemon/src/main.rs` (Firestore sync on session end, load facts on fresh start)
- Modify: `crates/aura-memory/src/consolidate.rs` (call Cloud Run instead of local Gemini)
- Modify: `crates/aura-gemini/src/config.rs` (inject facts into system prompt)

**Step 1: Add aura-firestore dependency to daemon**

In `crates/aura-daemon/Cargo.toml`:
```toml
aura-firestore = { path = "../aura-firestore" }
```

**Step 2: Update consolidate.rs to call Cloud Run**

Replace the direct Gemini REST call in `consolidate_session()` with a call to the Cloud Run consolidation endpoint. Keep the local Gemini fallback if Cloud Run URL is not configured.

```rust
pub async fn consolidate_session(
    api_key: &str,
    messages: &[Message],
    cloud_run_url: Option<&str>,
    auth_token: Option<&str>,
) -> Result<ConsolidationResponse> {
    if let Some(url) = cloud_run_url {
        consolidate_via_cloud_run(url, auth_token, messages).await
    } else {
        consolidate_locally(api_key, messages).await
    }
}
```

**Step 3: Load facts from Firestore on fresh session**

In `run_daemon()`, when `SessionMode::Fresh`:
1. Create `FirestoreClient` with project_id and device_id from config
2. Call `read_facts()` to get stored facts
3. Format facts into a context string
4. Append to system prompt: `"\n\nMemory from past sessions:\n{facts_context}"`

**Step 4: Write facts to Firestore on session end**

After consolidation completes (in the session end handler), write facts to Firestore via the client.

**Step 5: Add Firestore config to GeminiConfig or a new AppConfig**

Add fields: `firestore_project_id`, `device_id`, `cloud_run_url`, `cloud_run_auth_token` — loaded from config.toml or env vars.

**Step 6: Build and test**

```bash
cargo test --workspace
```

**Step 7: Commit**

```bash
git add -A && git commit -m "feat: integrate Firestore into daemon — load facts on fresh start, sync on session end"
```

---

## Task 8: GCP Auto-Deploy Script

**Files:**
- Create: `scripts/deploy-gcp.sh`

**Step 1: Write the deploy script**

```bash
#!/usr/bin/env bash
set -euo pipefail

# Usage: ./scripts/deploy-gcp.sh --project <GCP_PROJECT_ID>

# Parse args
PROJECT_ID=""
REGION="us-central1"
SERVICE_NAME="aura-consolidation"

while [[ $# -gt 0 ]]; do
    case "$1" in
        --project) PROJECT_ID="$2"; shift 2;;
        --region) REGION="$2"; shift 2;;
        *) echo "Unknown arg: $1"; exit 1;;
    esac
done

if [[ -z "$PROJECT_ID" ]]; then
    echo "Usage: ./scripts/deploy-gcp.sh --project <GCP_PROJECT_ID>"
    exit 1
fi

echo "==> Checking gcloud CLI..."
command -v gcloud >/dev/null 2>&1 || { echo "gcloud CLI not found. Install: https://cloud.google.com/sdk/docs/install"; exit 1; }

echo "==> Setting project to $PROJECT_ID..."
gcloud config set project "$PROJECT_ID"

echo "==> Enabling required APIs..."
gcloud services enable \
    firestore.googleapis.com \
    run.googleapis.com \
    artifactregistry.googleapis.com \
    cloudbuild.googleapis.com

echo "==> Creating Firestore database (if not exists)..."
gcloud firestore databases create --location="$REGION" 2>/dev/null || echo "Firestore database already exists"

echo "==> Building and deploying Cloud Run service..."
gcloud run deploy "$SERVICE_NAME" \
    --source infrastructure/ \
    --region "$REGION" \
    --allow-unauthenticated \
    --set-env-vars "GEMINI_API_KEY=${GEMINI_API_KEY:-},GCP_PROJECT_ID=$PROJECT_ID" \
    --memory 256Mi \
    --cpu 1 \
    --min-instances 0 \
    --max-instances 3

CLOUD_RUN_URL=$(gcloud run services describe "$SERVICE_NAME" --region "$REGION" --format 'value(status.url)')

echo ""
echo "==> Deployment complete!"
echo ""
echo "Add to ~/.config/aura/config.toml:"
echo ""
echo "  [cloud]"
echo "  firestore_project_id = \"$PROJECT_ID\""
echo "  cloud_run_url = \"$CLOUD_RUN_URL\""
echo ""
```

**Step 2: Make executable**

```bash
chmod +x scripts/deploy-gcp.sh
```

**Step 3: Commit**

```bash
git add scripts/deploy-gcp.sh && git commit -m "feat: GCP auto-deploy script — Firestore + Cloud Run in one command"
```

---

## Task Order & Dependencies

```
Task 1: Remove Wake Word          (independent)
Task 2: IPC Protocol Changes      (independent)
Task 3: Session Reconnection      (independent)
Task 4: Activity Stream UI        (depends on Task 2)
Task 5: Firestore Crate           (independent)
Task 6: Cloud Run Service         (depends on Task 5)
Task 7: Integrate Firestore       (depends on Tasks 3, 5, 6)
Task 8: Deploy Script             (depends on Task 6)
```

**Parallel tracks:**
- Track A: Tasks 1, 2, 4 (UI + cleanup)
- Track B: Tasks 3 (reconnection)
- Track C: Tasks 5, 6, 7, 8 (GCP)

Tasks 1, 2, 3, 5 can all run in parallel.
