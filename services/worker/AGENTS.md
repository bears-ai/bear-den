# AGENTS.md

How to orient in **this project**: a Rust web **starter** (Axum, SQLx, MiniJinja, PostgreSQL, optional split web/API/workers). For **running locally**, see [`docs/quickstart.md`](docs/quickstart.md). For **deploy** (env, migrations, Docker build), see [`docs/deploy.md`](docs/deploy.md). **Service toggles** (`RUN_WEB`, `RUN_API`, `RUN_WORKERS`) and health checks are covered in [`docs/infrastructure-and-ops.md`](docs/infrastructure-and-ops.md).

In the **BEARS** monorepo, **Den** is the **provisioning controller** and **orchestrator** for the stack: it **configures** downstream services (especially the **Letta API server**), **runs** first-party surfaces such as the **web chat UI**, and exposes **control-plane tools** (for example **Den meta tools**). **Streaming harness execution** (Letta Code SDK) is implemented in **`services/frontend/`** (**Codepool**) at the repo root; Den calls it over the private network. With **`RUN_WEB=true`**, **`CODEPOOL_BASE_URL`** must be non-empty at startup (**production** images default to **`http://bears-pool:3030`** when unset; override for local dev — see [`docs/quickstart.md`](docs/quickstart.md)). **Its own** PostgreSQL state exists so Den can **enforce policy**, **validate** operations, and **rebuild** consistent outward configuration from **repo + backups**—not to hold Letta’s runtime agent memory, which stays on the **Letta** side. See [`../../docs/architecture/DEN_ARCHITECTURE.md`](../../docs/architecture/DEN_ARCHITECTURE.md) and the root [`../../AGENTS.md`](../../AGENTS.md) (“Den’s role”).

## Verifying Rust changes (agents + dev containers)

**`cargo` is available** in typical dev containers and CI images that include the Rust toolchain. After editing this crate, **run checks from the `services/worker/` directory** (package root), for example:

- `cargo build` or `cargo check` — compile the library + binary.
- `cargo test` — unit tests; integration tests that need Postgres require `DATABASE_URL` and applied migrations (see [`docs/quickstart.md`](docs/quickstart.md)).
- `cargo clippy --all-targets` — Clippy is not suppressed at the crate root; the heaviest legacy bundle remains scoped on [`src/api/oauth/mod.rs`](src/api/oauth/mod.rs). Fix warnings in code you touch and shrink those module-level allows over time.

Do not assume the environment is “simulated only”: prefer **running `cargo` yourself** to catch compile errors before handing work back.

**Docker build:** Do not treat a change as **complete** until a **`docker build`** of [`Dockerfile`](Dockerfile) from the `services/worker/` directory succeeds. Release images use **`--features production`**, Alpine/musl, and SQLx at build time in ways a local glibc `cargo check` does not fully replicate. When Docker is unavailable locally, say so explicitly; otherwise run the image build (build-time env: [`docs/deploy.md`](docs/deploy.md), [`COOLIFY_DEPLOY.md`](COOLIFY_DEPLOY.md)).

## Start here

1. [`docs/README.md`](docs/README.md) — documentation index.
2. [`docs/concepts-overview.md`](docs/concepts-overview.md) — repository layout and where things live in code.
3. [`docs/quickstart.md`](docs/quickstart.md) — local development (env, migrations, `cargo run`).
4. [`docs/axum-in-this-repo.md`](docs/axum-in-this-repo.md) — how Axum routers, state, and layers map to `src/web` and `src/api`.
5. [`docs/development-principles.md`](docs/development-principles.md) — development principles (dependencies, frontend minimalism); populate for your product.
6. Implementation patterns under [`docs/`](docs/): SQLx, MiniJinja contexts, Axum handlers, infrastructure, frontend, deploy.

## Database migrations (SQLx)

- **Never edit** an existing file under `migrations/` that has already been applied anywhere: SQLx checksums the file content in `_sqlx_migrations`. **Add a new** `*_up.sql` for fixes or new columns (see [`migrations/README.md`](migrations/README.md)).
- If checksum drift already happened, follow **Repairing checksum mismatch** in that README (`sqlx migrate info`, then align `checksum` with the canonical file).

## Working on features

- **HTTP (web UI)** — `src/web/`, templates under `src/web/templates/`. **CSS:** follow [`docs/frontend-development.md`](docs/frontend-development.md): no authored `<style>` blocks or inline layout/theme in templates; standalone pages still use `/assets/css/style.css` and scoped rules in `src/web/assets/css/specifics.css`.
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

## Planning docs (BEARS)

Monorepo **[`docs/planning/`](../../docs/planning/)**: [Phase 1 bootstrap](../../docs/planning/PHASE1_BOOTSTRAP.md), [Phase 1 decisions](../../docs/planning/PHASE1_DECISIONS.md), [PLAN](../../docs/planning/PLAN.md). The [`plans/`](plans/) folder here is only a **pointer** to those paths; do not duplicate planning markdown under `services/worker/plans/`.
