"""FastAPI server for the Aura memory agent."""

import asyncio
import hashlib
import hmac
import json
import logging
import uuid
from typing import Any

from fastapi import FastAPI, Header, HTTPException, Request
from fastapi.responses import JSONResponse
from google.adk.runners import InMemoryRunner
from google.genai import types
from pydantic import BaseModel

from agent import consolidate_agent, ingest_agent, query_agent
from config import AUTH_TOKEN, MAX_BODY_BYTES, MAX_CONCURRENT_REQUESTS, validate_id

logger = logging.getLogger(__name__)

app = FastAPI(title="Aura Memory Agent")

# Rate-limiting semaphore
_semaphore = asyncio.Semaphore(MAX_CONCURRENT_REQUESTS)

# One runner per agent, created at module level
_ingest_runner = InMemoryRunner(agent=ingest_agent)
_consolidate_runner = InMemoryRunner(agent=consolidate_agent)
_query_runner = InMemoryRunner(agent=query_agent)


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
# Auth helper
# ---------------------------------------------------------------------------


def _check_auth(authorization: str | None) -> None:
    """Constant-time Bearer token check. Raises 401 on failure."""
    if not authorization or not authorization.startswith("Bearer "):
        raise HTTPException(
            status_code=401,
            detail={
                "status": "error",
                "error": "Missing or invalid Authorization header",
                "code": 401,
            },
        )
    token = authorization.removeprefix("Bearer ")
    # Constant-time comparison via SHA-256 digests
    expected_digest = hashlib.sha256(AUTH_TOKEN.encode()).digest()
    actual_digest = hashlib.sha256(token.encode()).digest()
    if not hmac.compare_digest(expected_digest, actual_digest):
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
    authorization: str | None = Header(default=None),
) -> dict[str, Any]:
    _check_auth(authorization)

    async with _semaphore:
        try:
            validate_id(body.device_id, "device_id")
            validate_id(body.session_id, "session_id")
        except ValueError as exc:
            raise HTTPException(
                status_code=400,
                detail={"status": "error", "error": str(exc), "code": 400},
            )

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
    authorization: str | None = Header(default=None),
) -> dict[str, Any]:
    _check_auth(authorization)

    async with _semaphore:
        try:
            validate_id(body.device_id, "device_id")
        except ValueError as exc:
            raise HTTPException(
                status_code=400,
                detail={"status": "error", "error": str(exc), "code": 400},
            )

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
    authorization: str | None = Header(default=None),
) -> dict[str, Any]:
    _check_auth(authorization)

    async with _semaphore:
        try:
            validate_id(body.device_id, "device_id")
        except ValueError as exc:
            raise HTTPException(
                status_code=400,
                detail={"status": "error", "error": str(exc), "code": 400},
            )

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
