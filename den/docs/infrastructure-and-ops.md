# Infrastructure and operations

## Processes

**This project** builds as **one Rust binary** (crate name defaults to **`newapp`**). At runtime you enable:

- **Web** — `RUN_WEB=true` (port from `PORT`, default `3000`)
- **API** — `RUN_API=true` (port from `API_PORT`, default `3001`)
- **Workers** — `RUN_WORKERS=true` (background tasks in the same process)

Legacy `SERVER_MODE=web|api|both` may still be parsed for migration; prefer the `RUN_*` flags (see `src/config.rs`).

You can run any combination (e.g. web + workers only). If nothing is enabled, the process will warn and do little useful work.

## Configuration

- **`DATABASE_URL`** — PostgreSQL (required for normal operation).
- **Service toggles** — `RUN_WEB`, `RUN_API`, `RUN_WORKERS`.
- **Templates / assets** — paths and production embedding follow `Config` and feature flags (`production`).

Other variables (mail, OAuth, optional integrations) are defined on `Config` as needed for your deployment.

## Database

- **PostgreSQL** with migrations under `migrations/`.
- **SQLx** with compile-time checked queries; CI/production builds typically use offline data (`.sqlx/`). See [sqlx-patterns.md](sqlx-patterns.md).

Session storage uses the same database (tower-sessions SQLx store); session migrations run at startup.

## Deployment

- **Docker** — root `Dockerfile` produces one image; set env in the orchestrator (same as local). See [deploy.md](deploy.md).

## Logging

Structured logging via **tracing** with default filters wired in [`src/lib.rs`](../src/lib.rs) (`run()`, crate prefix `newapp`). Override with **`RUST_LOG`** when debugging.

## Health checks

| Service | Liveness | Readiness (PostgreSQL `SELECT 1`) |
|---------|-----------|-----------------------------------|
| Web (`RUN_WEB`) | `GET /healthcheck` → `OK` | `GET /health/ready` → `OK` or **503** |
| API (`RUN_API`) | `GET /healthcheck` → `API OK` | `GET /health/ready` → `OK` or **503** |

## Workers

When `RUN_WORKERS=true`, long-running and periodic tasks run in-process. See [`src/lib.rs`](../src/lib.rs) for the worker slot; this slim starter keeps workers idle until shutdown.

## Graceful shutdown

On **Unix**, the process handles **SIGTERM** and **Ctrl+C**. On other platforms, **Ctrl+C** only.
