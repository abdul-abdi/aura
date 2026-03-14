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
    query = ref.order_by("created_at", direction=firestore.Query.DESCENDING).limit(
        limit
    )
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
    return {
        "status": "consolidated",
        "memories_processed": len(source_ids),
        "insight": insight,
    }


async def mark_consolidated(memory_ids: list[str], device_id: str) -> dict:
    """Mark memories as consolidated after processing.

    Args:
        memory_ids: List of memory document IDs to mark.
        device_id: The user's device identifier.

    Returns:
        dict with count of successfully marked memories and any failures.
    """
    validate_id(device_id, "device_id")
    marked = 0
    failed = 0
    for mid in memory_ids:
        ref = _user_ref(device_id).collection("memories").document(mid)
        try:
            await ref.update({"consolidated": True})
            marked += 1
        except Exception:
            # Document may not exist (hallucinated ID from agent)
            failed += 1
    return {"status": "marked", "count": marked, "failed": failed}


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
    query = ref.order_by("created_at", direction=firestore.Query.DESCENDING).limit(
        limit
    )
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
