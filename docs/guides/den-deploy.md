# Deployment notes

**This project** is designed as **one container image** built from the root [`Dockerfile`](../Dockerfile). The same binary serves every role; Den behavior is selected with **environment variables** (`RUN_WEB`, `RUN_API`, `RUN_WORKERS`, ports, `DATABASE_URL`, etc.), while the explicit `cabinet-serve` command selects the narrow Cabinet surface. For process layout and health endpoints, see [infrastructure-and-ops.md](infrastructure-and-ops.md). For local development, see [den-quickstart.md](den-quickstart.md).

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
| `DEN_API_ORIGIN` | Public API origin when `RUN_API=true`; for Den armature adapters this may be `https://api.bears.[domain]`, another hostname, or a published host+port URL. The API listener mounts BearWire under `/bearwire`; compose derives `API_SERVER_URL` from this. |
| `SQLX_MIGRATE_IGNORE_MISSING` | Leave **unset** (default) so SQLx applies [`migrations/`](../migrations/) strictly. Set to `true` only for documented recovery when `_sqlx_migrations` references files no longer present in the repo. |

**Optional** keys your stack may need include: `WEB_SERVER_URL`, `API_SERVER_URL`, `SESSION_COOKIE_DOMAIN` (production cookie `Domain`; leave unset for host-only cookies), `MAILGUN_API_KEY`, `MAILGUN_DOMAIN`, `TEMPLATES_DIR`, and legacy `SERVER_MODE` (prefer `RUN_*`). Mail can stay empty for a minimal check.

**Optional pair web search:** set `DEN_SEARCH_PROVIDER=brave` and `BRAVE_SEARCH_API_KEY` to enable the `den.web.search` tool for the `pair` role. `DEN_SEARCH_MAX_RESULTS` defaults to `5` and is clamped to `1..10`. Without these settings, `den.web.search` returns a clear configuration error; `den.web.fetch` still works for direct URLs.

**Migration contract:** The `_sqlx_migrations` table in Postgres tracks which files from `migrations/` have been applied. Deploy the same binary (or same migration set) against each environment so history stays consistent; Coolify/CI should not mix ad-hoc SQL with Den’s embedded migrator without updating the repo.

**Health checks:** web and API expose `GET /healthcheck` (liveness) and `GET /health/ready` (readiness; **503** if the database is unreachable). Details per service are in [infrastructure-and-ops.md](infrastructure-and-ops.md).

## Cabinet-only composition (next release)

**Branch implementation, rollout pending:** `den cabinet-serve` (container command `cabinet-serve`) uses the existing Den image/binary but composes only Cabinet UI/assets/login, health and the initial read-only `/mcp` edge. It does not start Bear turns, BearWire, chat, Work/reflection/sandbox workers, native memory stores or Bifrost catalog refresh. Do not mount Bear SQLite or sandbox volumes in its dedicated container. See the [Cabinet guide](cabinet.md#distinct-cabinet-surface--implementation-not-deployed) and [Phase 5](../roadmap/CABINET_IMPLEMENTATION_PLAN.md#phase-5--distinct-cabinet-surface-and-external-mcp).

The approved root Compose definition now provides `bears-cabinet` behind the opt-in `cabinet` profile, with a suffixed internal hostname, the same `DEN_IMAGE` as Den/migrations, a read-only filesystem and only an ephemeral `/tmp`. It mounts no native-memory or sandbox volumes. The local source-build override includes Cabinet with `pull_policy: never`. A hostname proxying the existing full Den web process is not this dedicated composition.

Add `cabinet` to `COMPOSE_PROFILES` in `.env` (retain `bundled` only when using bundled Postgres), set the public Cabinet origin, and route that hostname to `bears-cabinet` on its configured listener port. Root Compose passes the same origin to Den, Cabinet and migrations; it uses `SERVICE_URL_CABINET` only as the platform-origin fallback. Den waits for Cabinet health when the profile is active; an unconfigured deployment keeps its existing local Cabinet fallback.

Configuration is supplied through `.env`/deployment environment, not baked into the image:

| Setting | Cabinet rollout requirement |
|---------|-----------------------------|
| `DATABASE_URL` | Same canonical Postgres database as Den; budget the additional pool's connections |
| `CABINET_URL` | The same absolute HTTP(S) URL on Den, Cabinet and migrations, e.g. `https://cabinet.example.com` or `https://cabinet.example.com/`; the trailing slash is optional and normalized; no credentials, subpath, query or fragment; HTTPS at production ingress; optional `SERVICE_URL_CABINET` fallback |
| `WEB_SERVER_URL` | Keep the Den public origin, including in Cabinet, for return links; Compose derives it from `DEN_WEB_ORIGIN` |
| `CABINET_PORT` | Compose supplies Cabinet's `PORT` (default `3000`); route the Cabinet hostname to this container, not Den's API |
| `CABINET_DB_MAX_CONNECTIONS` | Cabinet pool budget; defaults to the shared `DB_MAX_CONNECTIONS` value, then `5` |
| `JWT_SECRET` | Same existing Den secret for delegated OAuth MCP verification; no dev-secret fallback |
| Existing `S3_*` settings | Share the existing configured artifact storage when file workflows are enabled; do not introduce another bucket/registry |

Run the existing `den migrate` job from the matching release image first. It owns SQLx migrations and shared browser-session schema creation. Cabinet admission verifies migration completeness/checksums, absence of unknown/newer/dirty schema and the session table; it does not migrate. Its host-only `cabinet_session` cookie is distinct from Den's, ignores `SESSION_COOKIE_DOMAIN` and is Secure in production. Humans log in independently using their Den account; no shared-cookie SSO is implied. Browser writes require a matching Origin/Referer, while native MCP uses its independent bearer/transport guard.

With the origin configured consistently, Cabinet owns registered-upload recovery and Den stops starting that recovery task. Without the origin, Den retains local Cabinet and its recovery owner. Do not enable the origin on Den before the dedicated surface is healthy: old GET/HEAD links redirect, old mutation routes return `410 Gone`, and there is intentionally no hidden fallback editor. Preserve Job/Bear administration in Den and test their links into Cabinet.

**Artifact/deployment validation (2026-10-13):** the production Alpine/musl image `bears-den:cabinet-surface-validation` built successfully with `SQLX_OFFLINE=true` and embedded templates/assets. Seven resolved-Compose contract tests and six opt-in image tests pass, including complete acceptance with `CABINET_URL` both with and without a trailing slash. The same image's `migrate` command prepares an isolated database; a disposable Cabinet container then passes trusted loopback HTTPS login with Secure/HttpOnly host-only cookies, page creation/edit, discussion/history/comparison, embedded CSS, native-route isolation and browser/MCP credential separation. Its process runs as UID 10001, with a read-only root and no native-memory directory/mounts. Full workspace all-target strict Clippy passes. No running Den service was restarted, no deployment secrets were changed, and this is not public-host or live-S3 acceptance.

The repeatable checks live in `tests/deployment/`: `python3 -m unittest discover -s tests/deployment -v` runs the Compose checks. To opt into image acceptance, supply `CABINET_TEST_IMAGE`, `CABINET_TEST_NETWORK` and `CABINET_TEST_DATABASE_URL` for an existing isolated database whose name follows `den_cabinet_*_validation` (optional numeric suffix); the image migrates it, retains its test audit data and removes its own container. No production database or Bear-memory volume should be used.

**Remaining rollout checks:** select/configure the actual public hostname and TLS ingress, activate the profile consistently, verify native-browser navigation and Den/Bear/Docket handoffs, exercise configured storage/revocation against the live provider, and verify OAuth consent/client setup. `cabinet-serve` deliberately ignores Den `RUN_*` flags for composition. Deploy the narrow command, not a second `den serve` with workers disabled.

## SQLx offline builds

Set **`SQLX_OFFLINE=true`** for CI or air-gapped builds after committing the query cache under [`.sqlx/`](../.sqlx/) (regenerate with `./scripts/sqlx.sh prepare-all` against a database with the current migrations; CI verifies the committed cache with `./scripts/sqlx.sh migrate run` then `./scripts/sqlx.sh prepare --check --workspace -- --all-targets` (see [sqlx-patterns.md](sqlx-patterns.md))).

## Docker image build

The root `Dockerfile` needs **`--build-arg DATABASE_URL=...`** so SQLx can reach Postgres **during** `cargo build`. That URL must resolve from the **build** environment (often *not* `localhost` from inside BuildKit). See **[`deploy/docker-build.env.example`](../deploy/docker-build.env.example)**. For broader ops patterns, see [infrastructure-and-ops.md](infrastructure-and-ops.md).

## Host-specific notes

Coolify, Kubernetes, or plain Docker all map to the same idea: one image, inject env, expose the ports you enabled. Replace this section with instructions for your stack when you ship a product.
