# Hackathon Readiness: GCP Integration, Session Reconnection & Activity Stream UI

**Date:** 2026-03-12
**Deadline:** 2026-03-16 (Gemini Live Agent Challenge)
**Category:** UI Navigator (primary), Live Agent (secondary strength)

## Goals

1. Add meaningful Google Cloud integration (Firestore + Cloud Run) for hackathon compliance
2. Smart session reconnection with context-aware resume vs fresh start
3. Replace chat UI with Activity Stream — a window into the agent's brain
4. Remove wake word (unnecessary friction)
5. Auto-deploy script for GCP infrastructure (+0.2 bonus points)

---

## 1. Firestore Integration

### Architecture

```
Direct WebSocket ──→ Gemini Live API (zero added latency)

Session ends ──→ Daemon POSTs transcript to Cloud Run
              ──→ Cloud Run calls Gemini REST to extract facts
              ──→ Cloud Run writes facts + session summary to Firestore
              ──→ Daemon caches response in local SQLite

Fresh launch ──→ Daemon pulls facts from Firestore
             ──→ Injects into system prompt as context
```

### New Crate: `aura-firestore`

Thin REST client for Firestore — no heavy SDK, just HTTPS + JSON to
`firestore.googleapis.com/v1`.

**Firestore schema:**
```
projects/{project_id}/databases/(default)/documents/
  users/{device_id}/
    facts/{fact_id}
      - category: string (preference|habit|entity|task|context)
      - content: string
      - entities: array<string>
      - importance: number
      - created_at: timestamp
      - session_id: string
    sessions/{session_id}
      - summary: string
      - started_at: timestamp
      - ended_at: timestamp
```

**Device ID:** UUID generated on first launch, stored in SQLite `settings` table.

**Auth:** Firebase anonymous auth for client reads. Cloud Run uses default
service account credentials for writes.

### Cloud Run Consolidation Service

Replaces the local `consolidate.rs` Gemini REST call. Endpoint:

```
POST /consolidate
Content-Type: application/json
Authorization: Bearer <auth_token>

{
  "device_id": "uuid",
  "session_id": "uuid",
  "messages": [
    {"role": "user", "content": "...", "timestamp": "..."},
    {"role": "assistant", "content": "...", "timestamp": "..."}
  ]
}

Response:
{
  "facts": [...],
  "summary": "..."
}
```

- Calls Gemini REST API to extract facts from transcript
- Writes facts + session summary to Firestore
- Returns extracted facts for local SQLite cache
- Auth: SHA-256 constant-time token comparison (reuse existing proxy auth pattern)

### What stays unchanged
- SQLite remains the local cache and real-time message store
- Direct WebSocket to Gemini (no proxy in data path)
- Existing `aura-memory` crate API unchanged — just adds Firestore sync behind the scenes

---

## 2. Session Reconnection

### Two modes

```rust
enum SessionMode {
    Resume { handle: String },
    Fresh,
}
```

### Path 1: Resume (Reconnect button)

1. User clicks "Reconnect" in panel or right-click menu
2. Daemon loads `resumption_handle` from SQLite settings
3. **Undo the current handle-clearing logic** in `session.rs:463-467`
4. Pass handle to `GeminiLiveSession::connect()`
5. System prompt hint: `"This is a resumed session. Continue naturally — no greeting, no reintroduction."`
6. If Gemini accepts → session continues seamlessly
7. If Gemini rejects → fall through to Fresh

### Path 2: Fresh (app launch or handle rejected)

1. No handle passed → new Gemini session
2. Pull facts from Firestore → inject into system prompt
3. System prompt hint: `"This is a new session. Greet the user briefly and naturally introduce yourself. You have memory of past interactions — reference relevant facts naturally if appropriate."`
4. Aura speaks greeting

### Context overload safety net

```
Reconnect pressed
    → handle exists?
        → yes → attempt resume
            → stable for 30s? → keep going (counter++)
            → dies within 30s? → handle poisoned → Fresh mode
            → counter >= 3? → Fresh mode (context too deep)
        → no → Fresh mode
```

- Track `reconnect_counter` per session
- If 3+ reconnects on same session → auto-switch to Fresh
- If session dies <30s after resume → handle is poisoned, clear it, go Fresh
- SlidingWindow compression still active (500K target tokens)

### How daemon distinguishes modes

- Fresh app launch → check SQLite for handle
  - Handle exists AND last session ended <5 min ago → `Resume`
  - Otherwise → `Fresh`
- Reconnect button → always attempt `Resume`
- Handle rejection → retry as `Fresh`, clear stored handle

---

## 3. Wake Word Removal

- Delete `crates/aura-voice/src/wakeword.rs`
- Remove `rustpotter` dependency from `aura-voice/Cargo.toml`
- Remove wake word initialization from daemon startup
- Remove `wakeword_test.rs`

**Activation model after removal:**
- App launches → mic starts → Aura greets → conversation active
- User speaks → Aura hears immediately (no trigger phrase)
- Barge-in works natively
- Panel closed → still listening (menu bar dot shows status)
- Cmd+Shift+A → toggle panel visibility

---

## 4. Activity Stream UI

### Replace chat bubbles with typed event stream

The panel becomes a window into Aura's brain — you see it think, act, verify, speak.

### Event types and visual treatment

| Event | Icon | Style |
|-------|------|-------|
| User speech | 🎤 | Primary text, quoted |
| User typed message | 💬 | Primary text, quoted |
| Aura speaking | 🔊 | Primary text, quoted |
| Tool call (running) | ⚡ | Amber, monospace tool name |
| Tool call (done) | ⚡ | Dimmed, one-line result indented below |
| Tool call (failed) | ⚡ | Red, error indented below |
| Thinking | ◐ | Animated spinner, disappears on completion |
| Turn separator | ─ ─ | Thin dashed line between exchanges |
| Reconnect banner | ↻ | Full-width tappable bar, amber bg, pinned at top |

### Layout

```
┌──────────────────────────────────────┐
│ ● Aura          Connected       ⌘⇧A │
├──────────────────────────────────────┤
│                                      │
│  ┌──────────────────────────────┐    │
│  │  ↻  Connection lost. Reconnect│   │  ← only when disconnected
│  └──────────────────────────────┘    │
│                                      │
│  🎤 "Open Safari and check email"   │
│                                      │
│  ⚡ activate_app                     │
│     ✓ Safari foregrounded            │
│                                      │
│  ⚡ click_element                    │
│     ✓ Focused "Address bar"          │
│                                      │
│  ⚡ type_text                        │
│     ✓ Typed "mail.google.com"        │
│                                      │
│  🔊 "Done. Gmail's loading up."     │
│                                      │
│  ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─    │
│                                      │
│  🎤 "What's my latest email?"       │
│  ◐ Thinking...                       │
│                                      │
├──────────────────────────────────────┤
│  Type a message...               ⬆  │
└──────────────────────────────────────┘
```

### Tool result rendering

- No raw JSON — daemon summarizes before sending to UI
- `{"facts":[],"sessions":[]}` → `✓ No memories found`
- `click_element("Save")` → `✓ Clicked "Save"`
- Tool call rows are tappable to expand full output

### Reconnect banner

- Pinned at top of scroll view when disconnected
- Tappable — triggers reconnect (Resume mode)
- Shows "Reconnecting..." with spinner while in progress
- Disappears on successful connect

### IPC protocol changes

Add `source` field to transcript events:
```json
{"type": "transcript", "role": "user", "text": "...", "source": "voice"}
{"type": "transcript", "role": "user", "text": "...", "source": "text"}
{"type": "transcript", "role": "assistant", "text": "...", "source": "voice"}
```

Add `summary` field to tool_status:
```json
{"type": "tool_status", "name": "click_element", "status": "completed", "summary": "Clicked \"Save\"", "output": "...full output..."}
```

---

## 5. GCP Auto-Deploy Script

### `scripts/deploy-gcp.sh`

Single command deploys all GCP infrastructure:

```bash
./scripts/deploy-gcp.sh --project my-gcp-project
```

**What it does:**
1. Validates `gcloud` CLI installed and authenticated
2. Enables required APIs (Firestore, Cloud Run, Artifact Registry)
3. Creates Firestore database in Native mode (if not exists)
4. Builds consolidation service Docker image
5. Pushes to Artifact Registry
6. Deploys to Cloud Run with env vars (Gemini API key, auth token)
7. Sets IAM permissions (Cloud Run → Firestore write access)
8. Outputs config values for `~/.config/aura/config.toml`

**Prerequisite:** `gcloud auth login` + GCP project ID

### `infrastructure/Dockerfile`

Consolidation service container:
- Rust binary (release build, multi-stage)
- Exposes `/consolidate` and `/health` endpoints
- Reads `GEMINI_API_KEY`, `AURA_AUTH_TOKEN`, `GCP_PROJECT_ID` from env

### `infrastructure/cloudbuild.yaml` (optional)

For CI/CD integration if needed.

---

## Files to Create

- `crates/aura-firestore/` — new crate (Firestore REST client)
- `infrastructure/Dockerfile` — consolidation service
- `infrastructure/cloudbuild.yaml` — optional CI
- `scripts/deploy-gcp.sh` — auto-deploy

## Files to Modify

- `crates/aura-memory/src/store.rs` — add Firestore sync calls
- `crates/aura-memory/src/consolidate.rs` — call Cloud Run instead of local Gemini
- `crates/aura-gemini/src/session.rs` — undo handle clearing, add SessionMode
- `crates/aura-gemini/src/config.rs` — system prompt changes (resume vs fresh hints)
- `crates/aura-daemon/src/main.rs` — reconnect logic, session mode routing, remove wake word init
- `crates/aura-voice/Cargo.toml` — remove rustpotter dependency
- `AuraApp/Sources/MessageBubble.swift` — replace with ActivityRow
- `AuraApp/Sources/ConversationView.swift` — activity stream rendering
- `AuraApp/Sources/ContentView.swift` — reconnect banner
- `AuraApp/Sources/AppState.swift` — new event types, connection state
- `AuraApp/Sources/Protocol.swift` — source field, summary field
- `Cargo.toml` (workspace) — add aura-firestore member

## Files to Delete

- `crates/aura-voice/src/wakeword.rs`
- `crates/aura-voice/tests/wakeword_test.rs`
