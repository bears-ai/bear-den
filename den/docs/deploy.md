# Deployment notes

**This project** is designed as **one container image** built from the root [`Dockerfile`](../Dockerfile). The same binary serves every role; behavior is selected with **environment variables** (`RUN_WEB`, `RUN_API`, `RUN_WORKERS`, ports, `DATABASE_URL`, etc.). For process layout and health endpoints, see [infrastructure-and-ops.md](infrastructure-and-ops.md). For local development, see [quickstart.md](quickstart.md).

## Prerequisites

Trestle expects a **working PostgreSQL service** you operate separately. The app connects at startup using **`DATABASE_URL`** (required); without it the process fails immediately (see [`src/config.rs`](../src/config.rs)).

**Before the container or binary runs:**

1. Start Postgres (sidecar VM, compose service, managed DB, etc.).
2. Create a database and **apply migrations** from [`migrations/`](../migrations/) (for example `sqlx migrate run` against that URL, or your platform's migration job).

## Typical runtime environment

**Minimal variables** for a hello-world-style deploy are listed in **[`.env.example`](../.env.example)**. In short:

| Variable | Role |
|----------|------|
| `DATABASE_URL` | **Required.** Postgres connection string; DB must exist and be migrated. |
| `RUN_WEB` / `RUN_API` / `RUN_WORKERS` | Enable HTTP web, HTTP API, and in-process workers (each defaults to `false` if unset). Turn **at least one** on for a meaningful smoke test. |
| `PORT` / `API_PORT` | Listen ports when web/API are enabled (defaults `3000` / `3001`). |

**Optional** keys your stack may need include: `WEB_SERVER_URL`, `API_SERVER_URL`, `SESSION_COOKIE_DOMAIN` (production cookie `Domain`; leave unset for host-only cookies), `MAILGUN_API_KEY`, `MAILGUN_DOMAIN`, `TEMPLATES_DIR`, and legacy `SERVER_MODE` (prefer `RUN_*`). Mail can stay empty for a minimal check.

**Health checks:** web and API expose `GET /healthcheck` (liveness) and `GET /health/ready` (readiness; **503** if the database is unreachable). Details per service are in [infrastructure-and-ops.md](infrastructure-and-ops.md).

## SQLx offline builds

Set **`SQLX_OFFLINE=true`** for CI or air-gapped builds after committing the query cache under [`.sqlx/`](../.sqlx/) (regenerate with `cargo sqlx prepare` against a migrated database; see [sqlx-patterns.md](sqlx-patterns.md)).

## Docker image build

The root `Dockerfile` needs **`--build-arg DATABASE_URL=...`** so SQLx can reach Postgres **during** `cargo build`. That URL must resolve from the **build** environment (often *not* `localhost` from inside BuildKit). See **[`deploy/docker-build.env.example`](../deploy/docker-build.env.example)**. For broader ops patterns, see [infrastructure-and-ops.md](infrastructure-and-ops.md).

## Host-specific notes

Coolify, Kubernetes, or plain Docker all map to the same idea: one image, inject env, expose the ports you enabled. Replace this section with instructions for your stack when you ship a product.
