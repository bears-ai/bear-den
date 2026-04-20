#!/usr/bin/env python3
"""One-shot env validation for the BEARS compose stack (URI syntax + required secrets)."""

from __future__ import annotations

import os
import sys
from urllib.parse import urlparse


def err(msg: str) -> None:
    print(f"preflight: ERROR: {msg}", file=sys.stderr)


def info(msg: str) -> None:
    print(f"preflight: {msg}", file=sys.stderr)


def fail(msg: str) -> None:
    err(msg)
    sys.exit(1)


def require_non_empty(name: str) -> str:
    raw = os.environ.get(name)
    if raw is None or not str(raw).strip():
        fail(f"{name} must be set and non-empty")
    return str(raw).strip()


def parse_sql_uri(name: str, value: str) -> None:
    u = urlparse(value)
    if u.scheme not in ("postgres", "postgresql"):
        fail(f"{name} must use postgres:// or postgresql:// (got scheme {u.scheme!r})")
    if not u.hostname:
        fail(f"{name} must include a host name")


def validate_optional_let_pg_uri() -> None:
    raw = os.environ.get("LETTA_PG_URI", "") or ""
    value = raw.strip()
    if not value:
        info("LETTA_PG_URI unset (optional; Letta may use embedded defaults)")
        return
    parse_sql_uri("LETTA_PG_URI", value)
    if urlparse(value).scheme == "postgres":
        fail(
            "LETTA_PG_URI uses postgres:// — use postgresql:// so Alembic registers "
            "the SQLAlchemy driver (see services/letta/COOLIFY_DEPLOY.md)"
        )
    info("LETTA_PG_URI parses as PostgreSQL URI")


def validate_http_url(name: str, value: str) -> None:
    u = urlparse(value.strip())
    if u.scheme not in ("http", "https"):
        fail(f"{name} must be an http(s) URL (got scheme {u.scheme!r})")
    if not u.netloc:
        fail(f"{name} must include a host (netloc)")


def main() -> None:
    info("checking required secrets and URI-shaped environment variables")

    require_non_empty("JWT_SECRET")
    require_non_empty("LETTA_SERVER_PASS")
    info("JWT_SECRET and LETTA_SERVER_PASS are set")

    database_url = require_non_empty("DATABASE_URL")
    parse_sql_uri("DATABASE_URL", database_url)
    info("DATABASE_URL parses as PostgreSQL URI")

    validate_optional_let_pg_uri()

    llm = os.environ.get("LLM_API_URL", "").strip() or "http://bear-bifrost:8080/v1"
    validate_http_url("LLM_API_URL", llm)
    info(f"LLM_API_URL OK ({llm})")

    letta_base = os.environ.get("LETTA_BASE_URL", "").strip() or "http://bear-letta:8283"
    validate_http_url("LETTA_BASE_URL", letta_base)
    info(f"LETTA_BASE_URL OK ({letta_base})")

    codepool_base = os.environ.get("CODEPOOL_BASE_URL", "").strip() or "http://bear-codepool:3030"
    validate_http_url("CODEPOOL_BASE_URL", codepool_base)
    info(f"CODEPOOL_BASE_URL OK ({codepool_base})")

    web = os.environ.get("WEB_SERVER_URL", "").strip() or "http://localhost:3000"
    validate_http_url("WEB_SERVER_URL", web)
    info(f"WEB_SERVER_URL OK ({web})")

    if not (os.environ.get("OPENAI_API_KEY") or "").strip():
        err("OPENAI_API_KEY is empty — embeddings and direct OpenAI calls may fail")

    info("all preflight checks passed")


if __name__ == "__main__":
    main()
