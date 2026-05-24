# Deployment notes

**This project** is designed as **one container image** built from the root [`Dockerfile`](../Dockerfile). The same binary serves every role; behavior is selected with **environment variables** (`RUN_WEB`, `RUN_API`, `RUN_WORKERS`, ports, `DATABASE_URL`, etc.). For process layout and health endpoints, see [infrastructure-and-ops.md](infrastructure-and-ops.md). For local development, see [quickstart.md](quickstart.md).

## Prerequisites

Trestle expects a **working PostgreSQL service** you operate separately. The app connects at startup using **`DATABASE_URL`** (required); without it the process fails immediately (see [`src/config.rs`](../src/config.rs)).

**Before the container or binary runs:**

1. Start Postgres (sidecar VM, compose service, managed DB, etc.).
2. Create a database (empty is fine). On startup, `den` runs **embedded SQLx migrations** from [`migrations/`](../migrations/) against `DATABASE_URL` before serving (same as local `cargo run` once the process connects).

## Typical runtime environment

**Minimal variables** for a hello-world-style deploy are listed in **[`.env.example`](../.env.example)**. In short:

| Variable | Role |
|----------|------|
| `DATABASE_URL` | **Required.** Postgres connection string; the database must exist; schema is applied automatically at startup. |
| `RUN_WEB` / `RUN_API` / `RUN_WORKERS` | Enable HTTP web, HTTP API, and in-process workers (each defaults to `false` if unset). Turn **at least one** on for a meaningful smoke test. |
| `PORT` / `API_PORT` | Listen ports when web/API are enabled (defaults `3000` / `3001`). |
| `JWT_SECRET` | **Required** when `RUN_API=true` (OAuth access tokens are HS256-signed), or when the binary is built with `--features production` (release / Docker image). Use a long random secret. Web-only local runs without the production feature may omit it (a dev-only default applies only if the API listener is off). |
| `ACP_GATEWAY_ENABLED` | Enable the API-only ACP gateway on `/acp/*`; requires `RUN_API=true` and `LETTA_BASE_URL`. ACP routes to the Bear's API-direct `pair` role, not Codepool. In the root BEARS Compose stack this defaults to `true`. |
| `API_SERVER_URL` | Public API origin when `RUN_API=true`; for BEARS ACP adapters this may be `https://api.bears.[domain]`, another hostname, or a published host+port URL. |
| `SQLX_MIGRATE_IGNORE_MISSING` | Leave **unset** (default) so SQLx applies [`migrations/`](../migrations/) strictly. Set to `true` only for documented recovery when `_sqlx_migrations` references files no longer present in the repo. |

**Optional** keys your stack may need include: `WEB_SERVER_URL`, `API_SERVER_URL`, `SESSION_COOKIE_DOMAIN` (production cookie `Domain`; leave unset for host-only cookies), `MAILGUN_API_KEY`, `MAILGUN_DOMAIN`, `TEMPLATES_DIR`, and legacy `SERVER_MODE` (prefer `RUN_*`). Mail can stay empty for a minimal check.

**Optional pair web search:** set `DEN_SEARCH_PROVIDER=brave` and `BRAVE_SEARCH_API_KEY` to enable the `den.web.search` tool for the `pair` role. `DEN_SEARCH_MAX_RESULTS` defaults to `5` and is clamped to `1..10`. Without these settings, `den.web.search` returns a clear configuration error; `den.web.fetch` still works for direct URLs.

**Migration contract:** The `_sqlx_migrations` table in Postgres tracks which files from `migrations/` have been applied. Deploy the same binary (or same migration set) against each environment so history stays consistent; Coolify/CI should not mix ad-hoc SQL with Den’s embedded migrator without updating the repo.

**Health checks:** web and API expose `GET /healthcheck` (liveness) and `GET /health/ready` (readiness; **503** if the database is unreachable). Details per service are in [infrastructure-and-ops.md](infrastructure-and-ops.md).

## SQLx offline builds

Set **`SQLX_OFFLINE=true`** for CI or air-gapped builds after committing the query cache under [`.sqlx/`](../.sqlx/) (regenerate with `cargo sqlx prepare` against a database that has applied the current migrations at least once — for example after one local `cargo run`; see [sqlx-patterns.md](sqlx-patterns.md)).

## Docker image build

The root `Dockerfile` needs **`--build-arg DATABASE_URL=...`** so SQLx can reach Postgres **during** `cargo build`. That URL must resolve from the **build** environment (often *not* `localhost` from inside BuildKit). See **[`deploy/docker-build.env.example`](../deploy/docker-build.env.example)**. For broader ops patterns, see [infrastructure-and-ops.md](infrastructure-and-ops.md).

## Host-specific notes

Coolify, Kubernetes, or plain Docker all map to the same idea: one image, inject env, expose the ports you enabled. Replace this section with instructions for your stack when you ship a product.
