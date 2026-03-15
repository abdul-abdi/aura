"""Tests for the FastAPI server endpoints."""

import sys
import os

sys.path.insert(0, os.path.join(os.path.dirname(__file__), ".."))

os.environ.setdefault("AURA_AUTH_TOKEN", "test-token-123")
os.environ.setdefault("GEMINI_API_KEY", "fake-key")
os.environ.setdefault("GCP_PROJECT_ID", "test-project")

from unittest.mock import patch

from fastapi.testclient import TestClient

from server import app

client = TestClient(app)

AUTH_HEADERS = {"Authorization": "Bearer test-token-123"}


# ---------------------------------------------------------------------------
# Health
# ---------------------------------------------------------------------------


def test_health():
    response = client.get("/health")
    assert response.status_code == 200
    assert response.json() == {"status": "ok"}


# ---------------------------------------------------------------------------
# Auth required – no token
# ---------------------------------------------------------------------------


def test_ingest_no_auth():
    response = client.post(
        "/ingest",
        json={"device_id": "dev1", "session_id": "sess1", "messages": []},
    )
    assert response.status_code == 401


def test_query_no_auth():
    response = client.post(
        "/query",
        json={"device_id": "dev1", "context": "some context"},
    )
    assert response.status_code == 401


def test_consolidate_no_auth():
    response = client.post(
        "/consolidate",
        json={"device_id": "dev1"},
    )
    assert response.status_code == 401


# ---------------------------------------------------------------------------
# Auth required – wrong token
# ---------------------------------------------------------------------------


def test_ingest_bad_auth():
    response = client.post(
        "/ingest",
        headers={"Authorization": "Bearer wrong-token"},
        json={"device_id": "dev1", "session_id": "sess1", "messages": []},
    )
    assert response.status_code == 401


# ---------------------------------------------------------------------------
# Input validation
# ---------------------------------------------------------------------------


def test_ingest_bad_device_id():
    response = client.post(
        "/ingest",
        headers=AUTH_HEADERS,
        json={
            "device_id": "../../bad",
            "session_id": "sess1",
            "messages": [],
        },
    )
    assert response.status_code == 400


def test_query_bad_device_id():
    response = client.post(
        "/query",
        headers=AUTH_HEADERS,
        json={"device_id": "", "context": "some context"},
    )
    assert response.status_code == 400


# ---------------------------------------------------------------------------
# Happy-path – ingest valid
# ---------------------------------------------------------------------------


def test_ingest_valid():
    mock_response = (
        '{"summary": "User opened Safari", "facts": [{"category": "habit", '
        '"content": "opens Safari daily", "entities": ["Safari"], "importance": 0.5}]}'
    )
    with patch("server._run_agent") as mock_run:
        mock_run.return_value = mock_response
        response = client.post(
            "/ingest",
            headers=AUTH_HEADERS,
            json={
                "device_id": "dev1",
                "session_id": "sess1",
                "messages": [
                    {"role": "user", "content": "Open Safari for me"},
                    {"role": "assistant", "content": "Opening Safari"},
                ],
            },
        )
    assert response.status_code == 200
    data = response.json()
    assert data["status"] == "ok"
    assert data["summary"] == "User opened Safari"
    assert isinstance(data["facts"], list)
    assert data["memories_stored"] == 1


# ---------------------------------------------------------------------------
# Happy-path – query valid
# ---------------------------------------------------------------------------


def test_query_valid():
    mock_response = "User prefers dark mode and uses Safari."
    with patch("server._run_agent") as mock_run:
        mock_run.return_value = mock_response
        response = client.post(
            "/query",
            headers=AUTH_HEADERS,
            json={"device_id": "dev1", "context": "user is on Safari"},
        )
    assert response.status_code == 200
    data = response.json()
    assert data["context"] == mock_response


# ---------------------------------------------------------------------------
# Happy-path – consolidate valid
# ---------------------------------------------------------------------------


def test_consolidate_valid():
    mock_response = "Consolidated 3 memories into 1 insight."
    with patch("server._run_agent") as mock_run:
        mock_run.return_value = mock_response
        response = client.post(
            "/consolidate",
            headers=AUTH_HEADERS,
            json={"device_id": "dev1"},
        )
    assert response.status_code == 200
    data = response.json()
    assert data["status"] == "ok"


# ---------------------------------------------------------------------------
# Edge case – empty messages skips agent
# ---------------------------------------------------------------------------


def test_ingest_empty_messages():
    with patch("server._run_agent") as mock_run:
        response = client.post(
            "/ingest",
            headers=AUTH_HEADERS,
            json={
                "device_id": "dev1",
                "session_id": "sess1",
                "messages": [],
            },
        )
        mock_run.assert_not_called()
    assert response.status_code == 200
    data = response.json()
    assert data["memories_stored"] == 0


# ---------------------------------------------------------------------------
# Device token auth
# ---------------------------------------------------------------------------


def test_device_token_auth_with_mock():
    """Device token validated via mocked Firestore."""
    with patch("server._validate_device_token", return_value=True):
        with patch("server._run_agent") as mock_run:
            mock_run.return_value = "User prefers dark mode."
            resp = client.post(
                "/query",
                headers={"Authorization": "Bearer device-token-123"},
                json={"device_id": "dev-test", "context": "what's on screen"},
            )
            assert resp.status_code == 200


def test_legacy_disabled_rejects_shared_token():
    """When legacy disabled and device validation fails, reject."""
    with patch("server.config.LEGACY_AUTH_ENABLED", False):
        with patch("server._validate_device_token", return_value=False):
            resp = client.post(
                "/query",
                headers={"Authorization": "Bearer wrong-token"},
                json={"device_id": "dev-test", "context": "test"},
            )
            assert resp.status_code == 401
