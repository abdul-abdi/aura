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
        raise ValueError(
            f"{field_name} must be 1-{MAX_ID_LENGTH} characters, got {len(value)}"
        )
    if not _ID_PATTERN.match(value):
        raise ValueError(
            f"{field_name} must be alphanumeric, hyphens, underscores only"
        )
    return value
