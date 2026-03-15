"""FastAPI server for the Aura memory agent."""

import asyncio
import hashlib
import hmac
import json
import logging
import time
import uuid
from typing import Any

import httpx
from fastapi import FastAPI, HTTPException, Request
from fastapi.responses import JSONResponse
from google.adk.runners import InMemoryRunner
from google.genai import types
from pydantic import BaseModel

import config
from agent import consolidate_agent, ingest_agent, query_agent
from config import MAX_BODY_BYTES, MAX_CONCURRENT_REQUESTS, validate_id

logger = logging.getLogger(__name__)

app = FastAPI(title="Aura Memory Agent")

# Validate required config at import time (tests set env vars before import)
if not config.LEGACY_AUTH_ENABLED and not config.GCP_PROJECT_ID:
    raise RuntimeError(
        "Either LEGACY_AUTH_ENABLED with AURA_AUTH_TOKEN or GCP_PROJECT_ID must be configured"
    )
if config.LEGACY_AUTH_ENABLED and not config.AUTH_TOKEN:
    raise RuntimeError(
        "AURA_AUTH_TOKEN environment variable must be set when LEGACY_AUTH_ENABLED is true. "
        "The memory agent refuses to start without authentication configured."
    )

# Rate-limiting semaphore
_semaphore = asyncio.Semaphore(MAX_CONCURRENT_REQUESTS)

# One runner per agent, created at module level
_ingest_runner = InMemoryRunner(agent=ingest_agent)
_consolidate_runner = InMemoryRunner(agent=consolidate_agent)
_query_runner = InMemoryRunner(agent=query_agent)

# ---------------------------------------------------------------------------
# Device token cache
# ---------------------------------------------------------------------------

_device_cache: dict[str, tuple[str, float]] = {}  # device_id -> (token_hash, timestamp)
CACHE_TTL = 60.0


# ---------------------------------------------------------------------------
# Pydantic models
# ---------------------------------------------------------------------------


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


# ---------------------------------------------------------------------------
# Middleware: body size limit
# ---------------------------------------------------------------------------


@app.middleware("http")
async def check_body_size(request: Request, call_next):
    content_length = request.headers.get("content-length")
    if content_length and int(content_length) > MAX_BODY_BYTES:
        return JSONResponse(
            status_code=413,
            content={"status": "error", "error": "Request body too large", "code": 413},
        )
    return await call_next(request)


# ---------------------------------------------------------------------------
# Auth helpers
# ---------------------------------------------------------------------------


def _extract_bearer_token(request: Request) -> str:
    """Extract Bearer token from Authorization header. Raises 401 if missing."""
    auth = request.headers.get("authorization", "")
    if not auth.startswith("Bearer "):
        raise HTTPException(
            status_code=401,
            detail={
                "status": "error",
                "error": "Missing or invalid Authorization header",
                "code": 401,
            },
        )
    return auth[7:]


async def _validate_device_token(device_id: str, token: str) -> bool:
    """Validate device token against Firestore devices collection."""
    now = time.time()

    # Check cache
    if device_id in _device_cache:
        cached_hash, cached_at = _device_cache[device_id]
        if now - cached_at < CACHE_TTL:
            provided_hash = hashlib.sha256(token.encode()).hexdigest()
            return hmac.compare_digest(provided_hash, cached_hash)

    # Cache miss — read from Firestore using ADC
    try:
        import google.auth
        import google.auth.transport.requests

        credentials, _ = google.auth.default()
        credentials.refresh(google.auth.transport.requests.Request())

        url = (
            f"https://firestore.googleapis.com/v1/projects/{config.GCP_PROJECT_ID}"
            f"/databases/(default)/documents/devices/{device_id}"
        )
        async with httpx.AsyncClient() as client:
            resp = await client.get(
                url,
                headers={"Authorization": f"Bearer {credentials.token}"},
                timeout=5.0,
            )
        if resp.status_code != 200:
            return False
        fields = resp.json().get("fields", {})
        stored_hash = fields.get("token_hash", {}).get("stringValue", "")
        _device_cache[device_id] = (stored_hash, now)
        provided_hash = hashlib.sha256(token.encode()).hexdigest()
        return hmac.compare_digest(provided_hash, stored_hash)
    except Exception as e:
        logger.error(f"Device token validation failed: {e}")
        return False


async def _check_auth_with_device(token: str, device_id: str) -> None:
    """Validate token. Tries legacy first, then device token via Firestore. Raises 401 on failure."""
    # Legacy check
    if config.LEGACY_AUTH_ENABLED and config.AUTH_TOKEN:
        provided = hashlib.sha256(token.encode()).hexdigest()
        expected = hashlib.sha256(config.AUTH_TOKEN.encode()).hexdigest()
        if hmac.compare_digest(provided, expected):
            return  # Legacy auth passed

    # Device token check via Firestore
    if await _validate_device_token(device_id, token):
        return

    raise HTTPException(
        status_code=401,
        detail={"status": "error", "error": "Invalid token", "code": 401},
    )


# ---------------------------------------------------------------------------
# ADK runner helper
# ---------------------------------------------------------------------------


async def _run_agent(runner: InMemoryRunner, user_id: str, message: str) -> str:
    """Run an ADK agent and return the final text response."""
    parts: list[str] = []
    async for event in runner.run_async(
        user_id=user_id,
        session_id=f"req-{uuid.uuid4().hex}",
        new_message=types.UserContent(parts=[types.Part(text=message)]),
    ):
        if event.is_final_response() and event.content:
            for part in event.content.parts:
                if part.text:
                    parts.append(part.text)
    return "\n".join(parts)


# ---------------------------------------------------------------------------
# Endpoints
# ---------------------------------------------------------------------------


@app.get("/health")
async def health() -> dict[str, str]:
    return {"status": "ok"}


@app.post("/ingest")
async def ingest(
    body: IngestRequest,
    request: Request,
) -> dict[str, Any]:
    try:
        validate_id(body.device_id, "device_id")
        validate_id(body.session_id, "session_id")
    except ValueError as exc:
        raise HTTPException(
            status_code=400,
            detail={"status": "error", "error": str(exc), "code": 400},
        )

    token = _extract_bearer_token(request)
    await _check_auth_with_device(token, body.device_id)

    async with _semaphore:
        # Filter to user / tool-call messages only
        relevant_roles = {"user", "tool_call"}
        filtered = [m for m in body.messages if m.role.lower() in relevant_roles]

        if not filtered:
            return {
                "status": "ok",
                "memories_stored": 0,
                "summary": "",
                "facts": [],
            }

        # Build transcript
        lines: list[str] = []
        for m in filtered:
            prefix = f"[{m.timestamp}] " if m.timestamp else ""
            lines.append(f"{prefix}{m.role.upper()}: {m.content}")
        transcript = "\n".join(lines)

        prompt = (
            f"Device: {body.device_id}\n"
            f"Session: {body.session_id}\n\n"
            f"Transcript:\n{transcript}"
        )

        try:
            raw = await _run_agent(_ingest_runner, body.device_id, prompt)
        except Exception as exc:
            logger.exception("ingest_agent error")
            raise HTTPException(
                status_code=500,
                detail={"status": "error", "error": str(exc), "code": 500},
            )

        # Parse JSON response from agent
        summary = ""
        facts: list[Any] = []
        try:
            parsed = json.loads(raw)
            summary = parsed.get("summary", "")
            facts = parsed.get("facts", [])
        except (json.JSONDecodeError, AttributeError):
            summary = raw
            facts = []

        return {
            "status": "ok",
            "memories_stored": len(facts),
            "summary": summary,
            "facts": facts,
        }


@app.post("/query")
async def query(
    body: QueryRequest,
    request: Request,
) -> dict[str, Any]:
    try:
        validate_id(body.device_id, "device_id")
    except ValueError as exc:
        raise HTTPException(
            status_code=400,
            detail={"status": "error", "error": str(exc), "code": 400},
        )

    token = _extract_bearer_token(request)
    await _check_auth_with_device(token, body.device_id)

    async with _semaphore:
        prompt = f"Device: {body.device_id}\n\nContext:\n{body.context}"

        try:
            result = await _run_agent(_query_runner, body.device_id, prompt)
        except Exception as exc:
            logger.exception("query_agent error")
            raise HTTPException(
                status_code=500,
                detail={"status": "error", "error": str(exc), "code": 500},
            )

        return {"context": result}


@app.post("/consolidate")
async def consolidate(
    body: ConsolidateRequest,
    request: Request,
) -> dict[str, Any]:
    try:
        validate_id(body.device_id, "device_id")
    except ValueError as exc:
        raise HTTPException(
            status_code=400,
            detail={"status": "error", "error": str(exc), "code": 400},
        )

    token = _extract_bearer_token(request)
    await _check_auth_with_device(token, body.device_id)

    async with _semaphore:
        prompt = f"Consolidate memories for device: {body.device_id}"

        try:
            result = await _run_agent(_consolidate_runner, body.device_id, prompt)
        except Exception as exc:
            logger.exception("consolidate_agent error")
            raise HTTPException(
                status_code=500,
                detail={"status": "error", "error": str(exc), "code": 500},
            )

        # Try to parse structured counts from agent response
        memories_consolidated = 0
        insights_generated = 0
        try:
            parsed = json.loads(result)
            memories_consolidated = parsed.get("memories_processed", 0)
            insights_generated = 1 if parsed.get("insight") else 0
        except (json.JSONDecodeError, AttributeError):
            pass  # Agent returned free-text — counts stay 0

        return {
            "status": "ok",
            "result": result,
            "memories_consolidated": memories_consolidated,
            "insights_generated": insights_generated,
        }
