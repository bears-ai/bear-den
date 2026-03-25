# AGENTS.md

How to orient in **this project**: a Rust web **starter** (Axum, SQLx, MiniJinja, PostgreSQL, optional split web/API/workers). For **running locally**, see [`docs/quickstart.md`](docs/quickstart.md). For **deploy** (env, migrations, Docker build), see [`docs/deploy.md`](docs/deploy.md). **Service toggles** (`RUN_WEB`, `RUN_API`, `RUN_WORKERS`) and health checks are covered in [`docs/infrastructure-and-ops.md`](docs/infrastructure-and-ops.md).

## Start here

1. [`docs/README.md`](docs/README.md) — documentation index.
2. [`docs/concepts-overview.md`](docs/concepts-overview.md) — repository layout and where things live in code.
3. [`docs/quickstart.md`](docs/quickstart.md) — local development (env, migrations, `cargo run`).
4. [`docs/axum-in-this-repo.md`](docs/axum-in-this-repo.md) — how Axum routers, state, and layers map to `src/web` and `src/api`.
5. [`docs/development-principles.md`](docs/development-principles.md) — development principles (dependencies, frontend minimalism); populate for your product.
6. Implementation patterns under [`docs/`](docs/): SQLx, MiniJinja contexts, Axum handlers, infrastructure, frontend, deploy.

## Working on features

- **HTTP (web UI)** — `src/web/`, templates under `src/web/templates/`.
- **HTTP (standalone API + OAuth provider)** — `src/api/`.
- **Shared domain / DB** — `src/core/` (this tree still carries a large legacy domain from extraction; follow existing modules and `migrations/`).
- **Config** — `src/config.rs`, plus env and ops notes in [`docs/deploy.md`](docs/deploy.md), [`docs/infrastructure-and-ops.md`](docs/infrastructure-and-ops.md), and [`.env.example`](.env.example).
- **Entrypoint / workers** — [`src/lib.rs`](src/lib.rs) (`run()`), thin [`src/main.rs`](src/main.rs).

## After substantial changes

- If project focus shifts, suggest updates to [`docs/concepts-overview.md`](docs/concepts-overview.md) and any affected run/deploy docs under [`docs/`](docs/); update the root [`README.md`](README.md) only if you still use it as the primary human-facing overview.
- If you add a repeatable workflow, document it in `tasks.md` at the repo root (create if missing).

## Patterns (read when touching that layer)

| Topic | Doc |
|--------|-----|
| Development principles | [`docs/development-principles.md`](docs/development-principles.md) |
| SQLx macros & `cargo sqlx prepare` | [`docs/sqlx-patterns.md`](docs/sqlx-patterns.md) |
| `minijinja::context!` | [`docs/minijinja-context-patterns.md`](docs/minijinja-context-patterns.md) |
| Axum in this repo (routers, state, layers) | [`docs/axum-in-this-repo.md`](docs/axum-in-this-repo.md) |
| Axum routes & extractors (`{id}` not `:id`) | [`docs/axum-handler-patterns.md`](docs/axum-handler-patterns.md) |
| Services, deploy, ops | [`docs/infrastructure-and-ops.md`](docs/infrastructure-and-ops.md) |
| Local quickstart (`cargo run`, dev quirks) | [`docs/quickstart.md`](docs/quickstart.md) |
| Deploy notes | [`docs/deploy.md`](docs/deploy.md) |
| Frontend / templates | [`docs/frontend-development.md`](docs/frontend-development.md) |
| MiniJinja template limits (vs full Jinja2) | [`docs/minijinja-template-limitations.md`](docs/minijinja-template-limitations.md) |

## Planning docs

The [`plans/`](plans/) directory is reserved for **your** product roadmap when you turn this starter into a concrete app. The starter ships without any product plans.
