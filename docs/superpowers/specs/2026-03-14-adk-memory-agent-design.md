# ADK Memory Agent — Design Spec

**Date:** 2026-03-14
**Branch:** `feat/genai-sdk-consolidation`
**Status:** Approved

## Problem

Aura's hackathon submission requires Google GenAI SDK or ADK usage and Google Cloud hosting. The current architecture calls Gemini via raw WebSocket (Rust) with no SDK. The consolidation service on Cloud Run also uses raw HTTP. This fails the hackathon's hard requirement: "agents must be built using Google GenAI SDK or Agent Development Kit."

## Solution

Replace the Rust consolidation service with a Python ADK-powered memory agent on Cloud Run. Three ADK agents (Ingest, Consolidate, Query) manage a two-tier memory system where local SQLite handles fast working memory and Firestore stores durable cross-session memories.

This satisfies the hackathon requirement with genuine ADK usage (not a checkbox), follows a Google-published pattern (always-on-memory-agent), and improves Aura's memory capabilities.

## Architecture

```
┌──────────────────────────────────────────────────────────┐
│                  macOS (Aura Daemon)                      │
│                                                          │
│  aura-voice  aura-screen  aura-input   aura-memory       │
│  (audio)     (capture)    (kb/mouse)   (SQLite Tier 1)   │
│       └──────────┬────────────┘              │           │
│             ┌────▼─────┐                     │           │
│             │processor │◄────────────────────┘           │
│             └────┬─────┘                                 │
│                  │ session end:  POST /ingest             │
│                  │ session start: POST /query              │
│                  │ after ingest: POST /consolidate        │
└──────────────────┼───────────────────────────────────────┘
                   │
                   ▼
┌──────────────────────────────────────────────────────────┐
│           Cloud Run: aura-memory-agent                    │
│                                                          │
│  ┌──────────────┐ ┌─────────────────┐ ┌───────────────┐ │
│  │ IngestAgent   │ │ConsolidateAgent │ │ QueryAgent     │ │
│  │ (ADK)         │ │(ADK)            │ │ (ADK)          │ │
│  │ gemini-3.1-   │ │gemini-3-flash-  │ │ gemini-3.1-    │ │
│  │ flash-lite    │ │preview          │ │ flash-lite     │ │
│  └───────┬───────┘ └───────┬─────────┘ └──────┬────────┘ │
│          └─────────┬───────┘───────────────────┘         │
│                    ▼                                     │
│           ┌───────────────┐                              │
│           │   Firestore   │ (Tier 2: durable memory)    │
│           └───────────────┘                              │
└──────────────────────────────────────────────────────────┘
```

### Two-Tier Memory

- **Tier 1 (SQLite, local):** Session messages, screen context, tool history. Fast working memory. Unchanged from current implementation.
- **Tier 2 (Firestore, cloud):** Extracted facts, cross-session connections, insights. Managed by ADK memory agent.

### Session Lifecycle

```
[Fresh activation] ──▶ POST /query (screen ctx) ──▶ Memory Agent
                            │                           │
                            ◀── relevant memories ──────┘
                            │
                   Inject into system prompt
                            │
                   Start fresh Gemini session
                            │
                  ┌─────────▼──────────┐
                  │   Live session      │
                  │   (audio/tools)     │
                  └─────────┬──────────┘
                            │
[Brief disconnect] ────────▶ Session resumption (existing, unchanged)
                            │
[Session end/done] ────────▶ POST /ingest (transcript)
                            │
                            ▶ POST /consolidate (fire-and-forget)
```

Fresh activations query the memory agent for context. Brief disconnects use session resumption (unchanged).

### Reconnection Strategy

Session resumption is **kept** for brief network interruptions — it preserves the model's conversational state and is the right UX for a 2-second WiFi blip. The memory agent enhances the `is_first` (fresh activation) path only.

**Two reconnection modes:**
- **Brief interruption (auto-reconnect):** Session resumption via handle, same as today. No memory agent call.
- **Fresh activation (user clicks connect, or first launch):** Query memory agent for context, start new Gemini session.

The `is_first_connect` flag distinguishes these. On `is_first == true`, the daemon queries the memory agent. On `is_first == false`, it uses session resumption as today.

**Fallback:** If the memory agent is unreachable on fresh activation (timeout, Cloud Run cold start), the daemon uses local SQLite context (recent session summary + local facts) — same as the current behavior.

## ADK Agents

### IngestAgent

**Model:** `gemini-3.1-flash-lite-preview`

**Instruction:** "You are a memory extraction agent. Analyze conversation transcripts and extract structured memories. Extract facts about the user's preferences, habits, entities they work with, tasks they perform, and useful context. Be selective — only store information worth remembering. If the session was trivial (just a greeting or test), store nothing."

**Tools:**
- `store_memory(summary, entities, topics, category, importance, source_session, device_id)` — write to Firestore `users/{device_id}/memories/{id}`
- `read_recent_memories(device_id, limit)` — check for duplicates before storing

**Categories:** preference, habit, entity, task, context (same as current)

### ConsolidateAgent

**Model:** `gemini-3-flash-preview`

**Instruction:** "You are a memory consolidation agent. Review unconsolidated memories, find connections between them, and generate insights. Like the human brain during sleep — compress, connect, and synthesize. Look for patterns across sessions."

**Tools:**
- `read_unconsolidated_memories(device_id)` — memories where `consolidated == false`
- `read_consolidation_history(device_id)` — avoid redundant insights
- `store_consolidation(source_ids, summary, insight, connections, device_id)` — write to Firestore
- `mark_consolidated(memory_ids, device_id)` — set `consolidated = true`

### QueryAgent

**Model:** `gemini-3.1-flash-lite-preview`

**Instruction:** "You are a memory retrieval agent. Given the user's current context (screen, time, recent activity), find the most relevant memories and synthesize a brief context summary. Be concise — this will be injected into a real-time voice conversation. Return 2-4 sentences max."

**Tools:**
- `read_all_memories(device_id, limit)` — recent memories
- `read_consolidation_history(device_id)` — cross-session insights
- `search_memories(query, device_id)` — loads memories and filters in Python (Firestore has no native full-text search; acceptable at hackathon scale)
- `get_memory_stats(device_id)` — total counts

## API Endpoints

| Endpoint | Method | Purpose | Called by | Timeout |
|---|---|---|---|---|
| `/ingest` | POST | Send transcript → IngestAgent extracts memories | Daemon on session end | 10s |
| `/query` | POST | Retrieve relevant context for new session | Daemon on session start | 3s |
| `/consolidate` | POST | Trigger cross-memory pattern finding | Daemon after ingest | 30s |
| `/health` | GET | Liveness probe | Cloud Run | — |

All endpoints except `/health` require `Authorization: Bearer <token>` with constant-time comparison.

**Request/Response formats:**

```
POST /ingest
  Body: {device_id, session_id, messages: [{role, content, timestamp}]}
  Response: {status, memories_stored: int, summary: string, facts: [{category, content, entities, importance}]}
  Note: Returns summary and facts so daemon can persist them in local SQLite (Tier 1).

POST /query
  Body: {device_id, context: string}
  Response: {context: "relevant memory summary paragraph"}
  Note: POST (not GET) because screen context can be large.

POST /consolidate
  Body: {device_id}
  Response: {status, memories_consolidated: int, insights_generated: int}
```

**Error response format (all endpoints):**
```
{status: "error", error: "human-readable message", code: int}
```

**Input validation:** All endpoints validate `device_id` and `session_id` — alphanumeric + hyphens + underscores only, max 128 chars (matches `aura-firestore` validation rules).

**Rate limiting:** Concurrent request semaphore (max 10), max request body 1 MiB (same as current Rust service).

**Model configuration:** Model names are read from environment variables with defaults, not hardcoded:
- `INGEST_MODEL` (default: `gemini-3.1-flash-lite-preview`)
- `CONSOLIDATE_MODEL` (default: `gemini-3-flash-preview`)
- `QUERY_MODEL` (default: `gemini-3.1-flash-lite-preview`)

## Firestore Schema

```
users/{device_id}/
  memories/{memory_id}:
    summary: string
    entities: [string]
    topics: [string]
    category: string  (preference|habit|entity|task|context)
    importance: float (0.0–1.0)
    source_session: string
    consolidated: bool
    created_at: timestamp

  consolidations/{consolidation_id}:
    source_ids: [string]
    summary: string
    insight: string
    connections: [{from_id: string, to_id: string, relationship: string}]
    created_at: timestamp

  sessions/{session_id}:
    summary: string
    created_at: timestamp
```

Existing `facts/` and `sessions/` collections are kept — no migration needed. The memory agent writes to `memories/` and `consolidations/` alongside. Old data remains readable.

## Python Service Structure

```
infrastructure/memory-agent/
├── agent.py              # ADK agent definitions (Ingest, Consolidate, Query)
├── tools.py              # Firestore-backed tool functions
├── server.py             # FastAPI app, endpoints, auth
├── config.py             # Environment variable loading, validation, defaults
├── requirements.txt      # google-adk, google-cloud-firestore, fastapi, uvicorn
├── Dockerfile            # Python 3.12 slim
└── tests/
    ├── test_tools.py     # Unit tests for tool functions
    └── test_server.py    # Endpoint tests
```

**Dependencies:**
- `google-adk` — Agent Development Kit
- `google-cloud-firestore` — native async Firestore client (auto-auth via service account)
- `fastapi` + `uvicorn` — HTTP server
- `pytest` — testing

**Dockerfile:**
```dockerfile
FROM python:3.12-slim
WORKDIR /app
COPY requirements.txt .
RUN pip install --no-cache-dir -r requirements.txt
COPY . .
CMD ["uvicorn", "server:app", "--host", "0.0.0.0", "--port", "8080"]
```

## Rust Daemon Changes

### processor.rs — session start (`GeminiEvent::Connected`, `is_first == true`)

Enhance the existing `is_first` branch:

1. Capture screen context (unchanged)
2. Fire async `POST /query` with `{device_id, context: screen_summary}`, 3s timeout
3. In parallel, read local SQLite for `get_recent_summary()` (unchanged, fallback)
4. If cloud responds: merge cloud memories into greeting context
5. If cloud fails: use local context only (same as today)
6. Build greeting message with best available context
7. Send to Gemini with "Please greet the user now."

The `is_first == false` (reconnect) path stays unchanged — session resumption handles brief interruptions.

### processor.rs — session end (`GeminiEvent::Disconnected`)

After existing local SQLite operations:

1. POST `/ingest` with transcript, 10s timeout
2. If POST succeeds: persist returned `summary` and `facts` in local SQLite (same as today's local consolidation path)
3. If POST fails: fall back to existing local consolidation (unchanged)
4. Spawn detached task: POST `/consolidate` fire-and-forget (no await, no blocking)
5. Local `end_session()` call unchanged

### processor.rs — `load_firestore_facts()` replacement

The existing `load_firestore_facts()` call in `orchestrator.rs` (which reads from the old `facts/` collection) is replaced by the `/query` call above on fresh activation. The QueryAgent reads from `memories/` and `consolidations/` — the new collections. Remove the `load_firestore_facts()` call from the startup path to avoid duplicating context.

### session.rs — no changes

Session resumption is kept. No changes to session.rs.

### config.rs

- `cloud_run_url` reused as-is (points to memory agent instead of consolidation service)
- No new config fields needed

**Net Rust changes:** ~60-80 lines modified, ~20 lines removed.

## CI/CD Changes

### ci.yml

Add a Python lint/test job:

```yaml
python-lint:
  name: Python Lint & Test
  runs-on: ubuntu-latest
  steps:
    - uses: actions/checkout@v6
    - uses: actions/setup-python@v5
      with: {python-version: '3.12'}
    - run: pip install ruff pytest
    - run: ruff check infrastructure/memory-agent/
    - run: cd infrastructure/memory-agent && pytest
```

Update Docker build matrix: swap `consolidation` entry for `memory-agent`.

### deploy-cloud.yml

- Update change detection paths: `infrastructure/memory-agent/**` instead of `infrastructure/`
- Update deploy job: `--source infrastructure/memory-agent/`, service name `aura-memory-agent-${ENV_SUFFIX}`
- Same secrets: `GEMINI_API_KEY`, `AURA_AUTH_TOKEN`
- Memory: 512Mi (Python + ADK overhead)

## Deployment

Reuse existing `deploy-gcp.sh` with updated source path:

```bash
gcloud run deploy aura-memory-agent-${ENV_SUFFIX} \
  --source infrastructure/memory-agent/ \
  --set-secrets="GEMINI_API_KEY=gemini-api-key:latest,AURA_AUTH_TOKEN=aura-consolidation-auth-token:latest" \
  --set-env-vars="GCP_PROJECT_ID=$PROJECT_ID" \
  --memory 512Mi --cpu 1 \
  --min-instances 0 --max-instances 5
```

## Migration and Backward Compatibility

| Component | Fate |
|---|---|
| `infrastructure/consolidation/` (Rust) | Replaced by `infrastructure/memory-agent/`. Delete after confirmed working. |
| `aura-memory/consolidate.rs` | Kept — local fallback if cloud unreachable. |
| `aura-daemon/cloud.rs` | Kept — backup Firestore sync path. |
| `aura-firestore` crate | Kept — daemon reads facts locally. |
| Existing Firestore `facts/` collection | Kept — old data remains, new data in `memories/`. |

**Rollback plan:** Remove `cloud_run_url` from config → daemon falls back to local consolidation automatically. Zero data loss.

## Testing Strategy

**Python (`infrastructure/memory-agent/tests/`):**
- `test_tools.py`: Unit test Firestore tool functions with mock client. Verify document structure, ID generation, query filters.
- `test_server.py`: Endpoint tests — auth rejection, valid ingest, valid query, consolidate. Mock ADK runner.

**Rust:**
- Existing `aura-memory/consolidate.rs` tests stay unchanged (local fallback).
- No new Rust tests needed — daemon changes are minimal HTTP calls with timeout + fallback.

**Integration (manual, pre-deploy):**
1. Start memory agent locally (`uvicorn server:app`)
2. POST sample transcript to `/ingest` → verify memories in Firestore emulator
3. GET `/query?q=test` → verify relevant memories returned
4. POST `/consolidate` → verify insights generated

## Scope Summary

| Layer | Action | Lines |
|---|---|---|
| Python memory agent (agent.py, tools.py, server.py) | New | ~400 |
| Dockerfile + requirements.txt | New | ~15 |
| Tests (Python) | New | ~100 |
| Rust daemon (processor.rs, orchestrator.rs) | Modified | ~60-80 changed, ~20 removed |
| CI/CD workflows | Modified | ~30 changed |
| Deploy script | Modified | ~20 changed |
| **Total** | | **~550 new, ~110 modified, ~20 removed** |
