# ADK Memory Agent Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the Rust consolidation service with a Python ADK-powered memory agent on Cloud Run, satisfying the hackathon's ADK requirement while improving Aura's cross-session memory.

**Architecture:** Three ADK agents (Ingest, Consolidate, Query) run on Cloud Run behind a FastAPI server. The Rust daemon calls them on session start/end. Firestore is the durable memory store. Local SQLite remains the fast working memory (Tier 1). Session resumption is kept for brief disconnects.

**Tech Stack:** Python 3.12, google-adk, google-cloud-firestore, FastAPI, uvicorn

**Spec:** `docs/superpowers/specs/2026-03-14-adk-memory-agent-design.md`

---

## File Map

### New Files (Python service)

| File | Responsibility |
|---|---|
| `infrastructure/memory-agent/config.py` | Env var loading, validation, defaults |
| `infrastructure/memory-agent/tools.py` | Firestore-backed tool functions for all 3 agents |
| `infrastructure/memory-agent/agent.py` | ADK agent definitions (Ingest, Consolidate, Query) |
| `infrastructure/memory-agent/server.py` | FastAPI app, endpoints, auth, rate limiting |
| `infrastructure/memory-agent/requirements.txt` | Python dependencies |
| `infrastructure/memory-agent/Dockerfile` | Container image |
| `infrastructure/memory-agent/tests/test_tools.py` | Unit tests for tool functions |
| `infrastructure/memory-agent/tests/test_server.py` | Endpoint tests |

### Modified Files (Rust daemon + CI/CD)

| File | Change |
|---|---|
| `crates/aura-daemon/Cargo.toml` | Add `reqwest` dependency for HTTP calls to memory agent |
| `crates/aura-daemon/src/processor.rs` | Add memory agent query on fresh connect, add cloud ingest on disconnect |
| `crates/aura-daemon/src/orchestrator.rs` | Remove `load_firestore_facts()` call (lines 101-127) |
| `.github/workflows/ci.yml` | Add Python lint/test job, update Docker matrix |
| `.github/workflows/deploy-cloud.yml` | Update consolidation deploy to memory-agent |
| `scripts/deploy-gcp.sh` | Update service name and source path |

---

## Chunk 1: Python Foundation (config + tools + tests)

### Task 1: Project scaffold and config

**Files:**
- Create: `infrastructure/memory-agent/config.py`
- Create: `infrastructure/memory-agent/requirements.txt`
- Create: `infrastructure/memory-agent/__init__.py`
- Create: `infrastructure/memory-agent/tests/__init__.py`

- [ ] **Step 1: Create directory structure**

```bash
mkdir -p infrastructure/memory-agent/tests
```

- [ ] **Step 2: Write requirements.txt**

Create `infrastructure/memory-agent/requirements.txt`:

```
google-adk>=1.0.0,<2.0.0
google-cloud-firestore>=2.19.0,<3.0.0
fastapi>=0.115.0,<1.0.0
uvicorn>=0.34.0,<1.0.0
pytest>=8.0.0
pytest-asyncio>=0.24.0
ruff>=0.8.0
```

- [ ] **Step 3: Write config.py**

Create `infrastructure/memory-agent/config.py`:

```python
"""Environment configuration with validation and defaults."""

import os
import re

# Models
INGEST_MODEL: str = os.getenv("INGEST_MODEL", "gemini-3.1-flash-lite-preview")
CONSOLIDATE_MODEL: str = os.getenv("CONSOLIDATE_MODEL", "gemini-3-flash-preview")
QUERY_MODEL: str = os.getenv("QUERY_MODEL", "gemini-3.1-flash-lite-preview")

# Auth
GEMINI_API_KEY: str = os.environ.get("GEMINI_API_KEY", "")
AUTH_TOKEN: str = os.environ.get("AURA_AUTH_TOKEN", "")

# GCP
GCP_PROJECT_ID: str = os.environ.get("GCP_PROJECT_ID", "")

# Limits
MAX_CONCURRENT_REQUESTS: int = 10
MAX_BODY_BYTES: int = 1_024 * 1_024  # 1 MiB
MAX_ID_LENGTH: int = 128

# Validation
_ID_PATTERN = re.compile(r"^[a-zA-Z0-9_-]+$")


def validate_id(value: str, field_name: str) -> str:
    """Validate a string is safe for Firestore document paths."""
    if not value or len(value) > MAX_ID_LENGTH:
        raise ValueError(f"{field_name} must be 1-{MAX_ID_LENGTH} characters, got {len(value)}")
    if not _ID_PATTERN.match(value):
        raise ValueError(f"{field_name} must be alphanumeric, hyphens, underscores only")
    return value
```

- [ ] **Step 4: Create empty __init__.py files**

Create `infrastructure/memory-agent/__init__.py` (empty) and `infrastructure/memory-agent/tests/__init__.py` (empty).

- [ ] **Step 5: Commit**

```bash
git add infrastructure/memory-agent/
git commit -m "feat: scaffold memory agent project with config"
```

---

### Task 2: Firestore tool functions

**Files:**
- Create: `infrastructure/memory-agent/tools.py`
- Create: `infrastructure/memory-agent/tests/test_tools.py`

- [ ] **Step 1: Write test_tools.py with tests for validation and document structure**

Create `infrastructure/memory-agent/tests/test_tools.py`:

```python
"""Unit tests for Firestore-backed tool functions."""

import pytest
from unittest.mock import AsyncMock, MagicMock, patch
from config import validate_id


def test_validate_id_valid():
    assert validate_id("abc-123_XYZ", "test") == "abc-123_XYZ"


def test_validate_id_empty():
    with pytest.raises(ValueError, match="must be 1-128"):
        validate_id("", "test")


def test_validate_id_too_long():
    with pytest.raises(ValueError, match="must be 1-128"):
        validate_id("a" * 129, "test")


def test_validate_id_bad_chars():
    with pytest.raises(ValueError, match="alphanumeric"):
        validate_id("../../etc/passwd", "test")


def test_validate_id_slash():
    with pytest.raises(ValueError, match="alphanumeric"):
        validate_id("path/traversal", "test")
```

- [ ] **Step 2: Run tests to verify they pass (config validation tests)**

```bash
cd infrastructure/memory-agent && python -m pytest tests/test_tools.py -v
```

Expected: All 5 validation tests pass.

- [ ] **Step 3: Write tools.py with all Firestore tool functions**

Create `infrastructure/memory-agent/tools.py`.

**IMPORTANT:** All tool functions must be `async def` — ADK supports async tools and these run inside an asyncio event loop (FastAPI + uvicorn). Using `run_until_complete` would raise `RuntimeError`.

```python
"""Firestore-backed tool functions for ADK memory agents."""

import hashlib
from datetime import datetime, timezone

from google.cloud import firestore

from config import GCP_PROJECT_ID, validate_id

_db: firestore.AsyncClient | None = None


def get_db() -> firestore.AsyncClient:
    """Get or create the async Firestore client."""
    global _db
    if _db is None:
        _db = firestore.AsyncClient(project=GCP_PROJECT_ID or None)
    return _db


def _memory_doc_id(category: str, summary: str) -> str:
    """Deterministic document ID from category + summary. Uses SHA-256 truncated to 16 hex chars.
    Identical (category, summary) pairs produce the same ID for idempotent writes."""
    raw = f"{category}:{summary}"
    return hashlib.sha256(raw.encode()).hexdigest()[:16]


def _user_ref(device_id: str):
    return get_db().collection("users").document(device_id)


# ── IngestAgent tools ──────────────────────────────────────────


async def store_memory(
    summary: str,
    entities: list[str],
    topics: list[str],
    category: str,
    importance: float,
    source_session: str,
    device_id: str,
) -> dict:
    """Store a processed memory in Firestore.

    Args:
        summary: A concise 1-2 sentence summary of the memory.
        entities: Key people, apps, files, or concepts mentioned.
        topics: 2-4 topic tags.
        category: One of: preference, habit, entity, task, context.
        importance: Float 0.0 to 1.0 indicating importance.
        source_session: The session ID this memory came from.
        device_id: The user's device identifier.

    Returns:
        dict with memory_id and confirmation.
    """
    validate_id(device_id, "device_id")
    validate_id(source_session, "source_session")

    doc_id = _memory_doc_id(category, summary)
    now = datetime.now(timezone.utc).isoformat()
    doc_data = {
        "summary": summary,
        "entities": entities,
        "topics": topics,
        "category": category,
        "importance": max(0.0, min(1.0, importance)),
        "source_session": source_session,
        "consolidated": False,
        "created_at": now,
    }

    ref = _user_ref(device_id).collection("memories").document(doc_id)
    await ref.set(doc_data)

    return {"memory_id": doc_id, "status": "stored", "summary": summary}


async def read_recent_memories(device_id: str, limit: int = 5) -> dict:
    """Read recent memories to check for duplicates before storing.

    Args:
        device_id: The user's device identifier.
        limit: Maximum number of memories to return.

    Returns:
        dict with list of recent memories.
    """
    validate_id(device_id, "device_id")
    ref = _user_ref(device_id).collection("memories")
    query = ref.order_by("created_at", direction=firestore.Query.DESCENDING).limit(limit)
    docs = []
    async for doc in query.stream():
        data = doc.to_dict()
        data["id"] = doc.id
        docs.append(data)
    return {"memories": docs, "count": len(docs)}


# ── ConsolidateAgent tools ─────────────────────────────────────


async def read_unconsolidated_memories(device_id: str) -> dict:
    """Read memories that haven't been consolidated yet.

    Args:
        device_id: The user's device identifier.

    Returns:
        dict with list of unconsolidated memories.
    """
    validate_id(device_id, "device_id")
    ref = _user_ref(device_id).collection("memories")
    query = ref.where("consolidated", "==", False).limit(20)
    docs = []
    async for doc in query.stream():
        data = doc.to_dict()
        data["id"] = doc.id
        docs.append(data)
    return {"memories": docs, "count": len(docs)}


async def store_consolidation(
    source_ids: list[str],
    summary: str,
    insight: str,
    connections: list[dict],
    device_id: str,
) -> dict:
    """Store a consolidation result linking related memories.

    Args:
        source_ids: List of memory IDs that were consolidated.
        summary: A synthesized summary across all source memories.
        insight: One key pattern or insight discovered.
        connections: List of dicts with from_id, to_id, relationship.
        device_id: The user's device identifier.

    Returns:
        dict with confirmation.
    """
    validate_id(device_id, "device_id")
    now = datetime.now(timezone.utc).isoformat()
    doc_data = {
        "source_ids": source_ids,
        "summary": summary,
        "insight": insight,
        "connections": connections,
        "created_at": now,
    }
    ref = _user_ref(device_id).collection("consolidations")
    await ref.add(doc_data)
    return {"status": "consolidated", "memories_processed": len(source_ids), "insight": insight}


async def mark_consolidated(memory_ids: list[str], device_id: str) -> dict:
    """Mark memories as consolidated after processing.

    Args:
        memory_ids: List of memory document IDs to mark.
        device_id: The user's device identifier.

    Returns:
        dict with count of marked memories.
    """
    validate_id(device_id, "device_id")
    batch = get_db().batch()
    for mid in memory_ids:
        ref = _user_ref(device_id).collection("memories").document(mid)
        batch.update(ref, {"consolidated": True})
    await batch.commit()
    return {"status": "marked", "count": len(memory_ids)}


async def read_consolidation_history(device_id: str) -> dict:
    """Read past consolidation insights.

    Args:
        device_id: The user's device identifier.

    Returns:
        dict with list of consolidation records.
    """
    validate_id(device_id, "device_id")
    ref = _user_ref(device_id).collection("consolidations")
    query = ref.order_by("created_at", direction=firestore.Query.DESCENDING).limit(10)
    docs = []
    async for doc in query.stream():
        data = doc.to_dict()
        data["id"] = doc.id
        docs.append(data)
    return {"consolidations": docs, "count": len(docs)}


# ── QueryAgent tools ───────────────────────────────────────────


async def read_all_memories(device_id: str, limit: int = 20) -> dict:
    """Read all stored memories, most recent first.

    Args:
        device_id: The user's device identifier.
        limit: Maximum number of memories to return.

    Returns:
        dict with list of memories.
    """
    validate_id(device_id, "device_id")
    ref = _user_ref(device_id).collection("memories")
    query = ref.order_by("created_at", direction=firestore.Query.DESCENDING).limit(limit)
    docs = []
    async for doc in query.stream():
        data = doc.to_dict()
        data["id"] = doc.id
        docs.append(data)
    return {"memories": docs, "count": len(docs)}


async def search_memories(query: str, device_id: str) -> dict:
    """Search memories by keyword matching against summaries and entities.

    Firestore has no native full-text search, so this loads recent memories
    and filters in Python. Acceptable at hackathon scale.

    Args:
        query: Search query string.
        device_id: The user's device identifier.

    Returns:
        dict with matching memories.
    """
    validate_id(device_id, "device_id")
    query_lower = query.lower()
    ref = _user_ref(device_id).collection("memories")
    q = ref.order_by("created_at", direction=firestore.Query.DESCENDING).limit(50)
    matches = []
    async for doc in q.stream():
        data = doc.to_dict()
        summary = (data.get("summary") or "").lower()
        entities = [e.lower() for e in (data.get("entities") or [])]
        topics = [t.lower() for t in (data.get("topics") or [])]
        if (
            query_lower in summary
            or any(query_lower in e for e in entities)
            or any(query_lower in t for t in topics)
        ):
            data["id"] = doc.id
            matches.append(data)
    return {"memories": matches, "count": len(matches)}


async def get_memory_stats(device_id: str) -> dict:
    """Get memory statistics for a device.

    Args:
        device_id: The user's device identifier.

    Returns:
        dict with counts of memories, consolidations, etc.
    """
    validate_id(device_id, "device_id")
    mem_ref = _user_ref(device_id).collection("memories")
    con_ref = _user_ref(device_id).collection("consolidations")

    total = 0
    unconsolidated = 0
    async for _ in mem_ref.stream():
        total += 1
    async for _ in mem_ref.where("consolidated", "==", False).stream():
        unconsolidated += 1

    consolidations = 0
    async for _ in con_ref.stream():
        consolidations += 1

    return {
        "total_memories": total,
        "unconsolidated": unconsolidated,
        "consolidations": consolidations,
    }
```

- [ ] **Step 4: Add tool function tests to test_tools.py**

Append to `infrastructure/memory-agent/tests/test_tools.py`:

```python
import sys
import os
sys.path.insert(0, os.path.join(os.path.dirname(__file__), ".."))

from tools import _memory_doc_id


def test_memory_doc_id_deterministic():
    id1 = _memory_doc_id("preference", "likes dark mode")
    id2 = _memory_doc_id("preference", "likes dark mode")
    assert id1 == id2


def test_memory_doc_id_different_inputs():
    id1 = _memory_doc_id("preference", "likes dark mode")
    id2 = _memory_doc_id("habit", "likes dark mode")
    assert id1 != id2


def test_memory_doc_id_length():
    doc_id = _memory_doc_id("task", "some content")
    assert len(doc_id) == 16
```

- [ ] **Step 5: Run all tests**

```bash
cd infrastructure/memory-agent && python -m pytest tests/test_tools.py -v
```

Expected: All 8 tests pass.

- [ ] **Step 6: Commit**

```bash
git add infrastructure/memory-agent/
git commit -m "feat: add Firestore tool functions for memory agents"
```

---

## Chunk 2: ADK Agents + FastAPI Server

### Task 3: ADK agent definitions

**Files:**
- Create: `infrastructure/memory-agent/agent.py`

- [ ] **Step 1: Write agent.py with all three ADK agents**

Create `infrastructure/memory-agent/agent.py`:

```python
"""ADK agent definitions for the Aura memory system."""

from google.adk.agents import Agent

from config import INGEST_MODEL, CONSOLIDATE_MODEL, QUERY_MODEL
from tools import (
    store_memory,
    read_recent_memories,
    read_unconsolidated_memories,
    store_consolidation,
    mark_consolidated,
    read_consolidation_history,
    read_all_memories,
    search_memories,
    get_memory_stats,
)

ingest_agent = Agent(
    name="ingest_agent",
    model=INGEST_MODEL,
    instruction=(
        "You are a memory extraction agent for Aura, a macOS desktop voice assistant. "
        "Analyze conversation transcripts and extract structured memories.\n\n"
        "Extract facts about the user's preferences, habits, entities they work with, "
        "tasks they perform, and useful context. Be selective — only store information "
        "worth remembering across sessions.\n\n"
        "If the session was trivial (just a greeting, test, or very short), store nothing.\n\n"
        "Before storing, use read_recent_memories to check for duplicates. "
        "Do not store a memory if a very similar one already exists.\n\n"
        "Categories: preference, habit, entity, task, context.\n"
        "Importance: 0.0 (trivial) to 1.0 (critical). Most facts are 0.4-0.7.\n\n"
        "After extracting memories, respond with a JSON summary of what you stored:\n"
        '{"summary": "brief session summary", "facts": [{"category": "...", "content": "...", '
        '"entities": [...], "importance": 0.5}]}'
    ),
    tools=[store_memory, read_recent_memories],
)

consolidate_agent = Agent(
    name="consolidate_agent",
    model=CONSOLIDATE_MODEL,
    instruction=(
        "You are a memory consolidation agent. Review unconsolidated memories, "
        "find connections between them, and generate insights.\n\n"
        "Like the human brain during sleep — compress, connect, and synthesize. "
        "Look for patterns across sessions.\n\n"
        "Steps:\n"
        "1. Use read_unconsolidated_memories to get unprocessed memories\n"
        "2. Use read_consolidation_history to see what insights already exist\n"
        "3. Find connections: shared entities, related topics, behavioral patterns\n"
        "4. Use store_consolidation to save your findings\n"
        "5. Use mark_consolidated to mark processed memories\n\n"
        "If there are fewer than 2 unconsolidated memories, respond that there's "
        "nothing to consolidate yet."
    ),
    tools=[
        read_unconsolidated_memories,
        read_consolidation_history,
        store_consolidation,
        mark_consolidated,
    ],
)

query_agent = Agent(
    name="query_agent",
    model=QUERY_MODEL,
    instruction=(
        "You are a memory retrieval agent for Aura, a macOS desktop voice assistant. "
        "Given the user's current context (screen content, time, recent activity), "
        "find the most relevant memories and synthesize a brief context summary.\n\n"
        "Be concise — this will be injected into a real-time voice conversation. "
        "Return 2-4 sentences max.\n\n"
        "Steps:\n"
        "1. Use search_memories with keywords from the context\n"
        "2. Use read_all_memories for recent history\n"
        "3. Use read_consolidation_history for cross-session insights\n"
        "4. Synthesize the most relevant information into a brief summary\n\n"
        "Focus on actionable context: what was the user doing, what do they care about, "
        "what patterns are relevant right now."
    ),
    tools=[read_all_memories, read_consolidation_history, search_memories, get_memory_stats],
)
```

- [ ] **Step 2: Verify imports work**

```bash
cd infrastructure/memory-agent && python -c "import agent; print('Agents:', agent.ingest_agent.name, agent.consolidate_agent.name, agent.query_agent.name)"
```

Expected: `Agents: ingest_agent consolidate_agent query_agent`

- [ ] **Step 3: Commit**

```bash
git add infrastructure/memory-agent/agent.py
git commit -m "feat: define ADK agents for ingest, consolidate, query"
```

---

### Task 4: FastAPI server with auth and endpoints

**Files:**
- Create: `infrastructure/memory-agent/server.py`
- Create: `infrastructure/memory-agent/tests/test_server.py`

- [ ] **Step 1: Write test_server.py**

Create `infrastructure/memory-agent/tests/test_server.py`:

```python
"""Endpoint tests for the memory agent server."""

import sys
import os
sys.path.insert(0, os.path.join(os.path.dirname(__file__), ".."))

import pytest
from unittest.mock import patch, AsyncMock, MagicMock
from fastapi.testclient import TestClient

# Set required env vars before importing server
os.environ.setdefault("AURA_AUTH_TOKEN", "test-token-123")
os.environ.setdefault("GEMINI_API_KEY", "fake-key")
os.environ.setdefault("GCP_PROJECT_ID", "test-project")

from server import app

client = TestClient(app)
AUTH = {"Authorization": "Bearer test-token-123"}
BAD_AUTH = {"Authorization": "Bearer wrong-token"}


def test_health():
    resp = client.get("/health")
    assert resp.status_code == 200


def test_ingest_no_auth():
    resp = client.post("/ingest", json={"device_id": "d1", "session_id": "s1", "messages": []})
    assert resp.status_code == 401


def test_query_no_auth():
    resp = client.post("/query", json={"device_id": "d1", "context": "test"})
    assert resp.status_code == 401


def test_consolidate_no_auth():
    resp = client.post("/consolidate", json={"device_id": "d1"})
    assert resp.status_code == 401


def test_ingest_bad_auth():
    resp = client.post(
        "/ingest",
        json={"device_id": "d1", "session_id": "s1", "messages": []},
        headers=BAD_AUTH,
    )
    assert resp.status_code == 401


def test_ingest_bad_device_id():
    resp = client.post(
        "/ingest",
        json={"device_id": "../../bad", "session_id": "s1", "messages": []},
        headers=AUTH,
    )
    assert resp.status_code == 400


def test_query_bad_device_id():
    resp = client.post(
        "/query",
        json={"device_id": "", "context": "test"},
        headers=AUTH,
    )
    assert resp.status_code == 400


# ── Happy-path tests with mocked ADK runner ──────────────────


@patch("server._run_agent")
def test_ingest_valid(mock_run):
    mock_run.return_value = '{"summary": "User opened Safari", "facts": [{"category": "task", "content": "opened Safari", "entities": ["Safari"], "importance": 0.5}]}'
    resp = client.post(
        "/ingest",
        json={
            "device_id": "test-device",
            "session_id": "test-session",
            "messages": [{"role": "user", "content": "open Safari"}],
        },
        headers=AUTH,
    )
    assert resp.status_code == 200
    data = resp.json()
    assert data["status"] == "ok"
    assert data["summary"] == "User opened Safari"
    assert len(data["facts"]) == 1


@patch("server._run_agent")
def test_query_valid(mock_run):
    mock_run.return_value = "User frequently works with Rust projects in the morning."
    resp = client.post(
        "/query",
        json={"device_id": "test-device", "context": "VS Code open with Rust file"},
        headers=AUTH,
    )
    assert resp.status_code == 200
    data = resp.json()
    assert "context" in data
    assert len(data["context"]) > 0


@patch("server._run_agent")
def test_consolidate_valid(mock_run):
    mock_run.return_value = "Consolidated 3 memories into 1 insight."
    resp = client.post(
        "/consolidate",
        json={"device_id": "test-device"},
        headers=AUTH,
    )
    assert resp.status_code == 200
    assert resp.json()["status"] == "ok"


@patch("server._run_agent")
def test_ingest_empty_messages(mock_run):
    resp = client.post(
        "/ingest",
        json={"device_id": "test-device", "session_id": "s1", "messages": []},
        headers=AUTH,
    )
    assert resp.status_code == 200
    assert resp.json()["memories_stored"] == 0
    mock_run.assert_not_called()
```

- [ ] **Step 2: Write server.py**

Create `infrastructure/memory-agent/server.py`:

```python
"""FastAPI server for the Aura memory agent."""

import asyncio
import hashlib
import json
import logging

from fastapi import FastAPI, Header, HTTPException, Request
from fastapi.responses import JSONResponse
from pydantic import BaseModel

from google.adk.runners import InMemoryRunner
from google.genai import types

from starlette.middleware.trustedhost import TrustedHostMiddleware
from starlette.requests import Request as StarletteRequest
from starlette.responses import JSONResponse as StarletteJSONResponse

from config import AUTH_TOKEN, MAX_BODY_BYTES, MAX_CONCURRENT_REQUESTS, validate_id
from agent import ingest_agent, consolidate_agent, query_agent

logging.basicConfig(level=logging.INFO, format="%(asctime)s %(levelname)s %(message)s")
log = logging.getLogger("memory-agent")

app = FastAPI(title="Aura Memory Agent", docs_url=None, redoc_url=None)


@app.middleware("http")
async def limit_body_size(request: Request, call_next):
    """Reject requests with bodies larger than MAX_BODY_BYTES (1 MiB)."""
    content_length = request.headers.get("content-length")
    if content_length and int(content_length) > MAX_BODY_BYTES:
        return JSONResponse(
            status_code=413,
            content={"status": "error", "error": "Request body too large", "code": 413},
        )
    return await call_next(request)

_semaphore = asyncio.Semaphore(MAX_CONCURRENT_REQUESTS)

# ADK runners (stateless — each request gets a fresh session)
_ingest_runner = InMemoryRunner(agent=ingest_agent, app_name="aura_memory")
_consolidate_runner = InMemoryRunner(agent=consolidate_agent, app_name="aura_memory")
_query_runner = InMemoryRunner(agent=query_agent, app_name="aura_memory")


# ── Request models ─────────────────────────────────────────────


class Message(BaseModel):
    role: str
    content: str
    timestamp: str | None = None


class IngestRequest(BaseModel):
    device_id: str
    session_id: str
    messages: list[Message]


class QueryRequest(BaseModel):
    device_id: str
    context: str


class ConsolidateRequest(BaseModel):
    device_id: str


# ── Auth ───────────────────────────────────────────────────────


def _check_auth(authorization: str | None):
    """Verify bearer token with constant-time comparison."""
    if not authorization or not authorization.startswith("Bearer "):
        raise HTTPException(status_code=401, detail="Unauthorized")
    token = authorization[7:]
    # Constant-time comparison via SHA-256
    provided = hashlib.sha256(token.encode()).digest()
    expected = hashlib.sha256(AUTH_TOKEN.encode()).digest()
    if provided != expected:
        raise HTTPException(status_code=401, detail="Unauthorized")


# ── Helpers ────────────────────────────────────────────────────


async def _run_agent(runner: InMemoryRunner, user_id: str, message: str) -> str:
    """Run an ADK agent and collect the final text response."""
    parts = []
    async for event in runner.run_async(
        user_id=user_id,
        session_id=f"req-{id(message)}",
        new_message=types.UserContent(parts=[types.Part(text=message)]),
    ):
        if event.is_final_response() and event.content:
            for part in event.content.parts:
                if part.text:
                    parts.append(part.text)
    return "\n".join(parts)


# ── Endpoints ──────────────────────────────────────────────────


@app.get("/health")
async def health():
    return {"status": "ok"}


@app.post("/ingest")
async def ingest(req: IngestRequest, authorization: str | None = Header(None)):
    _check_auth(authorization)

    try:
        validate_id(req.device_id, "device_id")
        validate_id(req.session_id, "session_id")
    except ValueError as e:
        raise HTTPException(status_code=400, detail=str(e))

    async with _semaphore:
        # Format transcript for the agent
        lines = []
        for msg in req.messages:
            role = msg.role.upper()
            if role in ("USER", "TOOL_CALL"):
                lines.append(f"[{role}] {msg.content}")
        if not lines:
            return {"status": "ok", "memories_stored": 0, "summary": "", "facts": []}

        transcript = "\n".join(lines)
        prompt = (
            f"Device ID: {req.device_id}\n"
            f"Session ID: {req.session_id}\n\n"
            f"--- TRANSCRIPT ---\n{transcript}"
        )

        log.info(f"Ingesting session {req.session_id} ({len(lines)} messages)")
        response_text = await _run_agent(_ingest_runner, req.device_id, prompt)

        # Parse the agent's JSON response
        try:
            result = json.loads(response_text)
        except json.JSONDecodeError:
            log.warning(f"Agent returned non-JSON: {response_text[:200]}")
            result = {"summary": response_text, "facts": []}

        return {
            "status": "ok",
            "memories_stored": len(result.get("facts", [])),
            "summary": result.get("summary", ""),
            "facts": result.get("facts", []),
        }


@app.post("/query")
async def query(req: QueryRequest, authorization: str | None = Header(None)):
    _check_auth(authorization)

    try:
        validate_id(req.device_id, "device_id")
    except ValueError as e:
        raise HTTPException(status_code=400, detail=str(e))

    async with _semaphore:
        prompt = (
            f"Device ID: {req.device_id}\n\n"
            f"Current context:\n{req.context}"
        )

        log.info(f"Querying memories for device {req.device_id}")
        response_text = await _run_agent(_query_runner, req.device_id, prompt)

        return {"context": response_text}


@app.post("/consolidate")
async def consolidate(req: ConsolidateRequest, authorization: str | None = Header(None)):
    _check_auth(authorization)

    try:
        validate_id(req.device_id, "device_id")
    except ValueError as e:
        raise HTTPException(status_code=400, detail=str(e))

    async with _semaphore:
        prompt = f"Device ID: {req.device_id}\n\nConsolidate unconsolidated memories now."

        log.info(f"Consolidating memories for device {req.device_id}")
        response_text = await _run_agent(_consolidate_runner, req.device_id, prompt)

        return {
            "status": "ok",
            "result": response_text,
            "memories_consolidated": 0,
            "insights_generated": 0,
        }
```

- [ ] **Step 3: Run server tests**

```bash
cd infrastructure/memory-agent && python -m pytest tests/test_server.py -v
```

Expected: All 11 tests pass (health, auth rejection, validation, happy-path with mocked runner).

- [ ] **Step 4: Commit**

```bash
git add infrastructure/memory-agent/server.py infrastructure/memory-agent/tests/test_server.py
git commit -m "feat: add FastAPI server with auth, rate limiting, endpoints"
```

---

### Task 5: Dockerfile and local verification

**Files:**
- Create: `infrastructure/memory-agent/Dockerfile`

- [ ] **Step 1: Write Dockerfile**

Create `infrastructure/memory-agent/Dockerfile`:

```dockerfile
FROM python:3.12-slim

WORKDIR /app

COPY requirements.txt .
RUN pip install --no-cache-dir -r requirements.txt

COPY config.py agent.py tools.py server.py ./

ENV PORT=8080
EXPOSE 8080

CMD ["uvicorn", "server:app", "--host", "0.0.0.0", "--port", "8080"]
```

- [ ] **Step 2: Verify Docker build succeeds**

```bash
cd infrastructure/memory-agent && docker build -t aura-memory-agent .
```

Expected: Build completes successfully.

- [ ] **Step 3: Commit**

```bash
git add infrastructure/memory-agent/Dockerfile
git commit -m "feat: add Dockerfile for memory agent"
```

---

## Chunk 3: Rust Daemon Integration

### Task 6: Add reqwest dependency and memory agent query on fresh activation

**Files:**
- Modify: `crates/aura-daemon/Cargo.toml`
- Modify: `crates/aura-daemon/src/processor.rs:160-206`

- [ ] **Step 1: Add reqwest to aura-daemon dependencies**

Add to `crates/aura-daemon/Cargo.toml` under `[dependencies]`:

```toml
reqwest = { version = "0.12", default-features = false, features = ["json", "rustls-tls"] }
```

- [ ] **Step 2: Add helper function for memory agent HTTP calls**

Add a helper at the top of `processor.rs` (after existing imports/constants) for calling the memory agent:

```rust
/// Query the memory agent for relevant context on fresh activation.
async fn query_memory_agent(
    cloud_run_url: &str,
    cloud_run_auth_token: &str,
    device_id: &str,
    screen_context: &str,
) -> Option<String> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(3))
        .build()
        .ok()?;

    let url = format!("{cloud_run_url}/query");
    let body = serde_json::json!({
        "device_id": device_id,
        "context": screen_context,
    });

    let resp = client
        .post(&url)
        .bearer_auth(cloud_run_auth_token)
        .json(&body)
        .send()
        .await
        .ok()?;

    if !resp.status().is_success() {
        tracing::warn!("Memory agent /query returned {}", resp.status());
        return None;
    }

    let json: serde_json::Value = resp.json().await.ok()?;
    json.get("context").and_then(|c| c.as_str()).map(String::from)
}
```

- [ ] **Step 3: Modify the `is_first` branch in the Connected handler**

In the `GeminiEvent::Connected` handler where `is_first == true`, after screen context capture and before building the greeting message, add the memory agent query:

```rust
// Query memory agent for cross-session context (3s timeout, fallback to local)
let cloud_memories = if let (Some(ref url), Some(ref token), Some(ref did)) =
    (&cloud_run_url, &cloud_run_auth_token, &cloud_run_device_id)
{
    match query_memory_agent(url, token, did, &greeting_context).await {
        Some(ctx) => {
            tracing::info!("Got cloud memory context ({} chars)", ctx.len());
            Some(ctx)
        }
        None => {
            tracing::info!("Memory agent unavailable, using local context only");
            None
        }
    }
} else {
    None
};
```

Then include `cloud_memories` in the context message:

```rust
let memory_section = match cloud_memories {
    Some(ref mem) => format!("\n\nRelevant memories from past sessions:\n{mem}"),
    None => String::new(),
};

let context_msg = format!(
    "[System: User just activated Aura. {time_context} Current screen context:\n{greeting_context}{history_section}{memory_section}]\n\nPlease greet the user now."
);
```

- [ ] **Step 4: Run `cargo check` to verify compilation**

```bash
cargo check -p aura-daemon 2>&1 | tail -5
```

Expected: No errors.

- [ ] **Step 5: Commit**

```bash
git add crates/aura-daemon/Cargo.toml crates/aura-daemon/src/processor.rs
git commit -m "feat: query memory agent on fresh activation for cross-session context"
```

---

### Task 7: Enhance processor.rs — session end with memory agent ingest

**Files:**
- Modify: `crates/aura-daemon/src/processor.rs:974-1063`

- [ ] **Step 1: Add helper for memory agent ingest call**

Add another helper function:

```rust
/// Send session transcript to the memory agent for ingestion.
async fn ingest_to_memory_agent(
    cloud_run_url: &str,
    cloud_run_auth_token: &str,
    device_id: &str,
    session_id: &str,
    messages: &[aura_memory::store::Message],
) -> Option<aura_memory::consolidate::ConsolidationResponse> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .ok()?;

    let msg_json: Vec<serde_json::Value> = messages
        .iter()
        .filter(|m| matches!(m.role, aura_memory::MessageRole::User | aura_memory::MessageRole::ToolCall))
        .map(|m| {
            serde_json::json!({
                "role": match m.role {
                    aura_memory::MessageRole::User => "user",
                    aura_memory::MessageRole::ToolCall => "tool_call",
                    _ => "other",
                },
                "content": m.content,
                "timestamp": m.timestamp,
            })
        })
        .collect();

    if msg_json.is_empty() {
        return None;
    }

    let url = format!("{cloud_run_url}/ingest");
    let body = serde_json::json!({
        "device_id": device_id,
        "session_id": session_id,
        "messages": msg_json,
    });

    let resp = client
        .post(&url)
        .bearer_auth(cloud_run_auth_token)
        .json(&body)
        .send()
        .await
        .ok()?;

    if !resp.status().is_success() {
        tracing::warn!("Memory agent /ingest returned {}", resp.status());
        return None;
    }

    let json: serde_json::Value = resp.json().await.ok()?;
    let summary = json.get("summary").and_then(|s| s.as_str()).unwrap_or("").to_string();
    let facts: Vec<aura_memory::consolidate::ExtractedFact> = json
        .get("facts")
        .and_then(|f| serde_json::from_value(f.clone()).ok())
        .unwrap_or_default();

    Some(aura_memory::consolidate::ConsolidationResponse { summary, facts })
}
```

- [ ] **Step 2: Modify the Disconnected handler to try memory agent first**

In the `GeminiEvent::Disconnected` handler, before the existing `consolidate_session` call, try the memory agent:

```rust
// Try memory agent first, fall back to existing consolidation
let consolidation_result = if let (Some(ref url), Some(ref token), Some(ref did)) =
    (&cloud_run_url, &cloud_run_auth_token, &cloud_run_device_id)
{
    match ingest_to_memory_agent(url, token, did, &es_sid, &messages).await {
        Some(resp) => {
            tracing::info!("Memory agent ingested session successfully");
            // Fire-and-forget consolidation
            let consolidate_url = format!("{url}/consolidate");
            let consolidate_token = token.clone();
            let consolidate_did = did.clone();
            tokio::spawn(async move {
                let client = reqwest::Client::builder()
                    .timeout(std::time::Duration::from_secs(30))
                    .build();
                if let Ok(client) = client {
                    let _ = client
                        .post(&consolidate_url)
                        .bearer_auth(&consolidate_token)
                        .json(&serde_json::json!({"device_id": consolidate_did}))
                        .send()
                        .await;
                }
            });
            Ok(resp)
        }
        None => {
            tracing::info!("Memory agent unavailable, falling back to local consolidation");
            aura_memory::consolidate::consolidate_session(
                &es_key, &messages,
                cloud_run_url.as_deref(),
                cloud_run_auth_token.as_deref(),
                cloud_run_device_id.as_deref(),
                Some(&es_sid),
            ).await
        }
    }
} else {
    // No cloud config — use local consolidation
    aura_memory::consolidate::consolidate_session(
        &es_key, &messages, None, None, None, Some(&es_sid),
    ).await
};
```

Then use `consolidation_result` in the existing match block that persists to SQLite.

- [ ] **Step 3: Run `cargo check` to verify compilation**

```bash
cargo check -p aura-daemon 2>&1 | tail -5
```

Expected: No errors.

- [ ] **Step 4: Run existing tests**

```bash
cargo test -p aura-daemon 2>&1 | tail -10
```

Expected: All existing tests pass.

- [ ] **Step 5: Commit**

```bash
git add crates/aura-daemon/src/processor.rs
git commit -m "feat: ingest sessions via memory agent with local fallback"
```

---

### Task 8: Remove load_firestore_facts from orchestrator startup

**Files:**
- Modify: `crates/aura-daemon/src/orchestrator.rs`

- [ ] **Step 1: Remove the `load_firestore_facts` block (lines 101-127)**

In `crates/aura-daemon/src/orchestrator.rs`, remove the entire block at lines 101-127:

```rust
// Remove this entire block:
    // On fresh session start, load facts from Firestore and inject into system prompt.
    // This is optional — daemon works fine without Firestore configured.
    if matches!(session_mode, SessionMode::Fresh)
        && let (Some(project_id), Some(device_id)) = (
            &gemini_config.firestore_project_id,
            &gemini_config.device_id,
        )
    {
        if let Some(firebase_api_key) = &gemini_config.firebase_api_key {
            match cloud::load_firestore_facts(project_id, device_id, firebase_api_key).await {
                // ... all of this through the closing braces
            }
        }
    }
```

Keep the `flush_pending_syncs` block at lines 129-136 — that handles retrying failed Firestore writes and is still useful.

- [ ] **Step 2: Run `cargo check`**

```bash
cargo check -p aura-daemon 2>&1 | tail -5
```

Expected: No errors.

- [ ] **Step 3: Commit**

```bash
git add crates/aura-daemon/src/orchestrator.rs
git commit -m "refactor: remove load_firestore_facts, replaced by memory agent query"
```

---

## Chunk 4: CI/CD and Deployment

### Task 9: Update CI workflow

**Files:**
- Modify: `.github/workflows/ci.yml`

- [ ] **Step 1: Add Python lint/test job after the existing `audit` job**

Add to `ci.yml`:

```yaml
  python:
    name: Python Lint & Test
    runs-on: ubuntu-latest
    timeout-minutes: 10
    steps:
      - uses: actions/checkout@de0fac2e4500dabe0009e67214ff5f5447ce83dd # v6
      - uses: actions/setup-python@42375524e23c412d93fb67b49958b491fce71c38 # v5
        with:
          python-version: '3.12'
      - name: Install dependencies
        run: |
          cd infrastructure/memory-agent
          pip install -r requirements.txt
          pip install ruff
      - name: Lint
        run: ruff check infrastructure/memory-agent/
      - name: Test
        run: cd infrastructure/memory-agent && python -m pytest tests/ -v
```

- [ ] **Step 2: Update Docker build matrix — swap consolidation for memory-agent**

In the `docker` job's matrix, change:

```yaml
      matrix:
        include:
          - name: proxy
            context: .
            dockerfile: crates/aura-proxy/Dockerfile
          - name: memory-agent
            context: infrastructure/memory-agent/
            dockerfile: infrastructure/memory-agent/Dockerfile
```

- [ ] **Step 3: Add `python` to the gate job's needs and check**

Add `python` to `needs:` list and add the check:

```yaml
  gate:
    needs: [format, lint, test, build, audit, coverage, docker, commits, python]
```

And in the check script:

```bash
echo "python:   ${{ needs.python.result }}"
```

```bash
[[ "${{ needs.python.result }}" != "success" ]] && ...
```

- [ ] **Step 4: Commit**

```bash
git add .github/workflows/ci.yml
git commit -m "ci: add Python lint/test job for memory agent"
```

---

### Task 10: Update deploy workflow and script

**Files:**
- Modify: `.github/workflows/deploy-cloud.yml`
- Modify: `scripts/deploy-gcp.sh`

- [ ] **Step 1: Update deploy-cloud.yml change detection**

In the `detect-changes` job, update the consolidation detection:

```bash
if echo "$CHANGED" | grep -q 'infrastructure/memory-agent/'; then
    echo "consolidation=true" >> "$GITHUB_OUTPUT"
else
    echo "consolidation=false" >> "$GITHUB_OUTPUT"
fi
```

Also update the top-level paths trigger:

```yaml
paths:
  - 'infrastructure/memory-agent/**'
  - 'crates/aura-proxy/**'
  - '.github/workflows/deploy-cloud.yml'
```

- [ ] **Step 2: Update the deploy-consolidation job**

Change the `gcloud run deploy` command:

```yaml
      - name: Deploy to Cloud Run
        run: |
          gcloud run deploy aura-memory-agent-${{ env.ENV_SUFFIX }} \
            --source infrastructure/memory-agent/ \
            --project "${{ secrets.GCP_PROJECT_ID }}" \
            --region "${{ vars.GCP_REGION }}" \
            --allow-unauthenticated \
            --set-secrets="GEMINI_API_KEY=gemini-api-key:latest,AURA_AUTH_TOKEN=aura-consolidation-auth-token:latest" \
            --set-env-vars="GCP_PROJECT_ID=${{ secrets.GCP_PROJECT_ID }}" \
            --memory 512Mi \
            --cpu 1 \
            --min-instances 0 \
            --max-instances 5 \
            --quiet
```

Update the health check to use `aura-memory-agent-${{ env.ENV_SUFFIX }}` instead of `aura-consolidation-${{ env.ENV_SUFFIX }}`.

- [ ] **Step 3: Update deploy-gcp.sh**

Change the service name and source path:

```bash
SERVICE_NAME="aura-memory-agent-${ENV_SUFFIX}"
```

Update the `gcloud run deploy` command to use `--source infrastructure/memory-agent/` and `--memory 512Mi`.

- [ ] **Step 4: Commit**

```bash
git add .github/workflows/deploy-cloud.yml scripts/deploy-gcp.sh
git commit -m "ci: update deploy pipeline for memory agent service"
```

---

### Task 11: Full build verification

- [ ] **Step 1: Run Rust tests**

```bash
cargo test --workspace 2>&1 | tail -20
```

Expected: All tests pass.

- [ ] **Step 2: Run Python tests**

```bash
cd infrastructure/memory-agent && python -m pytest tests/ -v
```

Expected: All tests pass.

- [ ] **Step 3: Run Python linter**

```bash
cd infrastructure/memory-agent && ruff check .
```

Expected: No errors.

- [ ] **Step 4: Verify Docker build**

```bash
cd infrastructure/memory-agent && docker build -t aura-memory-agent .
```

Expected: Build succeeds.

- [ ] **Step 5: Build and launch Aura locally**

```bash
bash scripts/dev.sh
```

Expected: Aura builds and launches. Memory agent calls will fail gracefully (no cloud URL configured) and fall back to local consolidation.

- [ ] **Step 6: Final commit if any formatting fixes needed**

```bash
cargo fmt --all
git add crates/ infrastructure/ .github/ scripts/ && git commit -m "chore: formatting fixes" || true
```
