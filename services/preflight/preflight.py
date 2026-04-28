#!/usr/bin/env python3
"""One-shot env validation for the BEARS compose stack (URI syntax + required secrets).

Runtime aggregation of similar checks (plus live DB/HTTP probes) is exposed on Den as
``GET /status`` and ``GET /status.json`` when the web server is enabled.
"""

from __future__ import annotations

import os
import socket
import sys
import time
from urllib.parse import urlparse


def err(msg: str) -> None:
    print(f"preflight: ERROR: {msg}", file=sys.stderr)


def warn(msg: str) -> None:
    print(f"preflight: WARNING: {msg}", file=sys.stderr)


def info(msg: str) -> None:
    print(f"preflight: {msg}", file=sys.stderr)


def fail(msg: str) -> None:
    err(msg)
    sys.exit(1)


def require_non_empty(name: str) -> str:
    raw = os.environ.get(name)
    value = "" if raw is None else str(raw).strip()
    if not value or value == "SETME":
        fail(f"{name} must be set (current value is {value or 'empty'})")
    return value


def parse_sql_uri(name: str, value: str) -> None:
    u = urlparse(value)
    if u.scheme not in ("postgres", "postgresql"):
        fail(f"{name} must use postgres:// or postgresql:// (got scheme {u.scheme!r})")
    if not u.hostname:
        fail(f"{name} must include a host name")


def redacted_sql_uri(value: str) -> str:
    u = urlparse(value)
    if not u.netloc:
        return "<unparseable>"
    auth = ""
    if u.username:
        auth = u.username
        if u.password:
            auth += ":***"
        auth += "@"
    host = u.hostname or ""
    if ":" in host and not host.startswith("["):
        host = f"[{host}]"
    port = f":{u.port}" if u.port else ""
    path = u.path or ""
    query = f"?{u.query}" if u.query else ""
    return f"{u.scheme}://{auth}{host}{port}{path}{query}"


def validate_sql_tcp_reachable(name: str, value: str, hint: str) -> None:
    u = urlparse(value)
    host = u.hostname
    port = u.port or 5432
    if not host:
        fail(f"{name} must include a host name")

    timeout_secs = float(os.environ.get("PREFLIGHT_DB_CONNECT_TIMEOUT_SECS", "3"))
    retries = int(os.environ.get("PREFLIGHT_DB_CONNECT_RETRIES", "5"))
    last_error = None

    info(
        f"{name} target {redacted_sql_uri(value)} "
        f"(host={host}, port={port}, connect_timeout={timeout_secs}s, retries={retries})"
    )

    try:
        addrs = socket.getaddrinfo(host, port, type=socket.SOCK_STREAM)
        rendered = sorted(
            {
                f"{family.name if hasattr(family, 'name') else family}:{addr[0]}:{addr[1]}"
                for family, _, _, _, addr in addrs
            }
        )
        info(f"{name} DNS resolved {host} -> {', '.join(rendered)}")
    except OSError as exc:
        warn(f"{name} DNS lookup failed for {host}: {exc}")

    for attempt in range(1, retries + 1):
        try:
            info(f"{name} TCP connect attempt {attempt}/{retries} to {host}:{port}")
            with socket.create_connection((host, port), timeout=timeout_secs):
                info(f"{name} TCP reachable ({host}:{port})")
                return
        except OSError as exc:
            last_error = exc
            warn(f"{name} TCP connect attempt {attempt}/{retries} failed: {exc}")
            if attempt < retries:
                time.sleep(1)

    fail(
        f"{name} host is not reachable at {host}:{port} after {retries} attempts: {last_error}. "
        f"{hint}"
    )


def validate_letta_pg_uri(reachable: bool = True) -> None:
    value = require_non_empty("LETTA_PG_URI")
    parse_sql_uri("LETTA_PG_URI", value)
    if urlparse(value).scheme == "postgres":
        fail(
            "LETTA_PG_URI uses postgres:// — use postgresql:// so Alembic registers "
            "the SQLAlchemy driver (see services/letta/COOLIFY_DEPLOY.md)"
        )
    info("LETTA_PG_URI parses as PostgreSQL URI")
    if reachable:
        validate_sql_tcp_reachable(
            "LETTA_PG_URI",
            value,
            "Deploy/attach Letta's Postgres/pgvector database and set LETTA_PG_URI to its reachable internal URL.",
        )


def validate_http_url(name: str, value: str) -> None:
    u = urlparse(value.strip())
    if u.scheme not in ("http", "https"):
        fail(f"{name} must be an http(s) URL (got scheme {u.scheme!r})")
    if not u.netloc:
        fail(f"{name} must include a host (netloc)")


def validate_database_url(reachable: bool = True) -> None:
    database_url = require_non_empty("DATABASE_URL")
    parse_sql_uri("DATABASE_URL", database_url)
    info("DATABASE_URL parses as PostgreSQL URI")
    if reachable:
        validate_sql_tcp_reachable(
            "DATABASE_URL",
            database_url,
            "If you want the compose-bundled Postgres, enable COMPOSE_PROFILES=bundled; otherwise set DATABASE_URL to your managed Postgres.",
        )


def validate_config_shape() -> None:
    info("checking required secrets and URI-shaped environment variables")

    require_non_empty("JWT_SECRET")
    require_non_empty("LETTA_SERVER_PASS")
    info("JWT_SECRET and LETTA_SERVER_PASS are set")

    validate_database_url(reachable=False)
    validate_letta_pg_uri(reachable=False)

    llm = os.environ.get("LLM_API_URL", "").strip() or "http://bears-bifrost:8080/v1"
    validate_http_url("LLM_API_URL", llm)
    info(f"LLM_API_URL OK ({llm})")

    letta_base = (
        os.environ.get("LETTA_BASE_URL", "").strip() or "http://bears-letta:8283"
    )
    validate_http_url("LETTA_BASE_URL", letta_base)
    info(f"LETTA_BASE_URL OK ({letta_base})")

    memfs = (
        os.environ.get("LETTA_MEMFS_SERVICE_URL", "").strip()
        or "http://bears-memfs-manager:8285"
    )
    validate_http_url("LETTA_MEMFS_SERVICE_URL", memfs)
    info(f"LETTA_MEMFS_SERVICE_URL OK ({memfs})")

    memfs_org = os.environ.get(
        "MEMFS_DEFAULT_ORG", "org-00000000-0000-4000-8000-000000000000"
    ).strip()
    if memfs_org == "org-default":
        fail(
            "MEMFS_DEFAULT_ORG must not use the old placeholder 'org-default'; set it to Letta's org id or leave it unset for the default self-hosted org."
        )
    if not memfs_org.startswith("org-"):
        fail("MEMFS_DEFAULT_ORG must look like a Letta org id (prefix 'org-')")
    info(f"MEMFS_DEFAULT_ORG OK ({memfs_org})")

    codepool_base = (
        os.environ.get("CODEPOOL_BASE_URL", "").strip() or "http://bears-codepool:3030"
    )
    validate_http_url("CODEPOOL_BASE_URL", codepool_base)
    info(f"CODEPOOL_BASE_URL OK ({codepool_base})")

    web = require_non_empty("WEB_SERVER_URL")
    validate_http_url("WEB_SERVER_URL", web)
    info(f"WEB_SERVER_URL OK ({web})")

    require_non_empty("OPENAI_API_KEY")
    info("OPENAI_API_KEY is set")

    info("configuration shape checks passed")


def main() -> None:
    mode = sys.argv[1] if len(sys.argv) > 1 else "all"

    if mode == "config":
        validate_config_shape()
    elif mode == "den-db":
        validate_database_url(reachable=True)
    elif mode == "letta-pg":
        validate_letta_pg_uri(reachable=True)
    elif mode == "all":
        validate_config_shape()
        validate_database_url(reachable=True)
        validate_letta_pg_uri(reachable=True)
        info("all preflight checks passed")
    else:
        fail(
            f"unknown preflight mode {mode!r}; expected config, den-db, letta-pg, or all"
        )


if __name__ == "__main__":
    main()
