"""Unit tests for Firestore-backed tool functions."""

import sys
import os

sys.path.insert(0, os.path.join(os.path.dirname(__file__), ".."))

import pytest
from unittest.mock import AsyncMock, MagicMock, patch

from config import validate_id
from tools import _memory_doc_id


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


# ── Firestore tool function tests with mocked client ─────────


def _mock_firestore():
    """Create a mock Firestore client with chainable collection/document refs."""
    mock_db = MagicMock()
    mock_doc_ref = MagicMock()
    mock_doc_ref.set = AsyncMock()
    mock_doc_ref.update = AsyncMock()

    mock_collection_ref = MagicMock()
    mock_collection_ref.document.return_value = mock_doc_ref
    mock_collection_ref.add = AsyncMock()

    # Chain: db.collection("users").document(id).collection("memories").document(id)
    mock_user_doc = MagicMock()
    mock_user_doc.collection.return_value = mock_collection_ref
    mock_db.collection.return_value.document.return_value = mock_user_doc

    return mock_db, mock_doc_ref, mock_collection_ref


@pytest.mark.asyncio
async def test_store_memory_writes_to_firestore():
    mock_db, mock_doc_ref, _ = _mock_firestore()

    with patch("tools._db", mock_db):
        with patch("tools.get_db", return_value=mock_db):
            from tools import store_memory

            result = await store_memory(
                summary="User likes dark mode",
                entities=["dark mode"],
                topics=["preference"],
                category="preference",
                importance=0.7,
                source_session="session-123",
                device_id="device-abc",
            )

    assert result["status"] == "stored"
    assert result["summary"] == "User likes dark mode"
    assert len(result["memory_id"]) == 16
    mock_doc_ref.set.assert_awaited_once()
    # Verify the document data
    call_args = mock_doc_ref.set.call_args[0][0]
    assert call_args["summary"] == "User likes dark mode"
    assert call_args["category"] == "preference"
    assert call_args["importance"] == 0.7
    assert call_args["consolidated"] is False


@pytest.mark.asyncio
async def test_store_memory_clamps_importance():
    mock_db, mock_doc_ref, _ = _mock_firestore()

    with patch("tools._db", mock_db):
        with patch("tools.get_db", return_value=mock_db):
            from tools import store_memory

            await store_memory(
                summary="test",
                entities=[],
                topics=[],
                category="task",
                importance=5.0,  # Over 1.0
                source_session="s1",
                device_id="d1",
            )

    call_args = mock_doc_ref.set.call_args[0][0]
    assert call_args["importance"] == 1.0


@pytest.mark.asyncio
async def test_store_memory_rejects_bad_device_id():
    from tools import store_memory

    with pytest.raises(ValueError, match="alphanumeric"):
        await store_memory(
            summary="test",
            entities=[],
            topics=[],
            category="task",
            importance=0.5,
            source_session="s1",
            device_id="../bad",
        )


@pytest.mark.asyncio
async def test_mark_consolidated_handles_missing_doc():
    mock_db, _, _ = _mock_firestore()

    # Make update raise NotFound for a hallucinated ID
    mock_ref = MagicMock()
    mock_ref.update = AsyncMock(side_effect=Exception("NOT_FOUND"))
    mock_db.collection.return_value.document.return_value.collection.return_value.document.return_value = mock_ref

    with patch("tools._db", mock_db):
        with patch("tools.get_db", return_value=mock_db):
            from tools import mark_consolidated

            result = await mark_consolidated(
                memory_ids=["nonexistent-id"],
                device_id="device-abc",
            )

    assert result["count"] == 0
    assert result["failed"] == 1
