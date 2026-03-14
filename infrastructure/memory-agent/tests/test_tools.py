"""Unit tests for Firestore-backed tool functions."""

import sys
import os

sys.path.insert(0, os.path.join(os.path.dirname(__file__), ".."))

import pytest
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
