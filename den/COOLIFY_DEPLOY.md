# Den — Coolify deployment guide

**Stack context:** Den is the BEARS **control plane** (Rust / Axum): provisioning, **users↔bears** membership, routing, and related HTTP surfaces. It sits alongside **Letta**, **Bifrost**, and optional **Outline** (Cabinet) per [DEPLOYMENT.md](../docs/deployment/DEPLOYMENT.md) and [PLAN.md](../docs/planning/PLAN.md). For architecture, see [DEN_ARCHITECTURE.md](../docs/architecture/DEN_ARCHITECTURE.md).

## Overview

- **One image, one binary** — built from [`Dockerfile`](Dockerfile) in this directory. Runtime behavior is controlled with **environment variables** (`RUN_WEB`, `RUN_API`, `RUN_WORKERS`, ports, `DATABASE_URL`, …). Deeper reference: [`docs/deploy.md`](docs/deploy.md) and [`docs/infrastructure-and-ops.md`](docs/infrastructure-and-ops.md).
- **PostgreSQL is mandatory** — Den exits on startup if it cannot use `DATABASE_URL`. The **database must exist** (empty is fine); on each start Den runs **embedded SQLx migrations** from [`migrations/`](migrations/) against that URL before serving traffic, so routine deploys do not need a separate migration job. By default migrations are **strict** (see `SQLX_MIGRATE_IGNORE_MISSING` in [`.env.example`](.env.example) / [`docs/deploy.md`](docs/deploy.md)—leave it unset in production).
- **SQLx at image build time** — the `Dockerfile` runs `cargo build` with compile-time SQLx checks. Coolify’s build environment must supply a **`DATABASE_URL` build argument** that resolves **from the build machine** (often the same Postgres you use at runtime, reachable on the Docker build network). See **Build-time database** below.

## Prerequisites

- Coolify v4+
- A **PostgreSQL** instance (Coolify managed database, external managed Postgres, or another service on a shared Docker network).
- **Git** access to this monorepo if you use the **Dockerfile** build pack (recommended for GitOps).
- **Letta** (and **Bifrost**) when you enable bear provisioning and chat proxying — set `LETTA_BASE_URL` (and `LETTA_API_KEY` when Letta enforces auth). Cross-service hostnames follow your Coolify stack naming (for example the internal hostname shown on the Letta resource).

---

## Option A: Build from Git — Dockerfile build pack

### 1. Database (before first deploy)

1. Provision **PostgreSQL** (Coolify **Add Resource** → **Database** → PostgreSQL, or attach an existing instance).
2. Create an **empty database** (or pick an existing one) and a role with permission to create tables and run DDL — Den applies schema automatically on startup.

### 2. Create the Den resource

1. Open your Coolify **project** → **Add New Resource**.
2. Connect **this** repository (public or private, per your hosting setup).
3. Choose the **Dockerfile** build pack (not “Docker Image” alone — you want Coolify to **build** from the repo).

### 3. Point Coolify at `den/`

In the Dockerfile deployment settings, set:

| Field | Value |
| ----- | ----- |
| **Branch** | Your production branch (for example `main`). |
| **Base Directory** | `den` |
| **Dockerfile** | `Dockerfile` (path is relative to the base directory). |

Coolify should clone the repo and run `docker build` with context rooted at `den/`.

### 4. Build arguments (required for the current `Dockerfile`)

Open **Build Arguments** / **Docker Build Args** (wording varies by Coolify version) and add:

| Name | Purpose |
| ---- | ------- |
| `DATABASE_URL` | Used at **image build** for SQLx when **`SQLX_OFFLINE` is unset or `false`**. Must reach PostgreSQL from the build environment (disposable compile-only DB is fine). The Dockerfile defaults to a dummy URL when you use offline mode instead. |
| `SQLX_OFFLINE` | Set to **`true`** to compile against committed [`.sqlx/`](.sqlx/) query metadata (no live Postgres during `cargo build`). The Dockerfile bind-mounts `.sqlx` read-only for this path. Regenerate metadata with `cargo sqlx prepare` when queries change. |

If you omit `SQLX_OFFLINE=true`, the build needs a reachable Postgres so SQLx can verify queries against a database that has applied the current migrations (same as before). Offline builds are the usual **CI / air-gapped** approach (see [`docs/deploy.md`](docs/deploy.md)).

At **container start**, Den connects using the **runtime** `DATABASE_URL` and applies any pending migrations there automatically.

Optional: pin **`RUST_VERSION`** in the `Dockerfile` or override it via build args if your Coolify setup supports passing additional `ARG` values.

### 5. Runtime environment variables

In the resource → **Environment Variables** / **Production Variables**, set at least:

| Variable | Notes |
| -------- | ----- |
| `DATABASE_URL` | **Required.** The database Den serves at runtime; migrations run against this URL on startup (connection string as accepted by SQLx / `tokio-postgres`). |
| `JWT_SECRET` | **Required for release images** (Dockerfile builds with `--features production`). Use a long random value. Also required whenever `RUN_API=true` in dev builds so OAuth access tokens can be signed (HS256). |
| `RUN_WEB` | `true` to serve the web UI (recommended first smoke). |
| `RUN_API` | `true` if you need the standalone API listener. |
| `RUN_WORKERS` | `true` when you want in-process workers enabled. |
| `PORT` | Web listen port inside the container (default **3000**). |
| `API_PORT` | API listen port when `RUN_API=true` (default **3001**). |

Strongly recommended for production:

| Variable | Notes |
| -------- | ----- |
| `WEB_SERVER_URL` | Public origin of the web app (**no** trailing slash), for example `https://den.example.com`. |
| `API_SERVER_URL` | Public origin of the API when `RUN_API=true`. |
| `SESSION_COOKIE_DOMAIN` | Cookie `Domain` when sessions must span subdomains; omit for host-only cookies. |

Integrations (set when you wire the rest of the stack):

| Variable | Notes |
| -------- | ----- |
| `LETTA_BASE_URL` | Internal base URL for Letta (no trailing slash), for example `http://bears-letta:8283`. |
| `LETTA_API_KEY` | Bearer token when Letta is configured with `LETTA_SERVER_PASS` / API auth. |

Mail, OAuth, and other keys are documented in [`.env.example`](.env.example) and [`docs/deploy.md`](docs/deploy.md).

**Migrations:** Den applies embedded SQL from [`migrations/`](migrations/) on startup. By default, SQLx does **not** ignore migration files missing from the binary; do not set `SQLX_MIGRATE_IGNORE_MISSING` in production unless you are following a documented recovery procedure for a legacy `_sqlx_migrations` table.

**Sessions:** Login sessions use `tower-sessions` with the Postgres store; the session cookie carries an opaque id and data lives in Postgres. Optional signed/encrypted cookies (`with_signed` / `with_private`) are not configured in this repo—no extra session signing env var is required today.

### 6. Ports

- **Web only (`RUN_WEB=true`, `RUN_API=false`):** expose internal port **3000** and map it to HTTPS / your Coolify domain as usual.
- **Web + API:** add a **second published port** in Coolify for **3001** (or change `API_PORT` and expose the matching port). The runtime image listens on whichever ports you configure via `PORT` / `API_PORT`.

The `Dockerfile` only declares `EXPOSE 3000`; publishing the API port is done in Coolify’s **Ports** / **Networking** UI when you enable the API.

### 7. Health checks (Coolify)

Prefer **HTTP** health checks so you do not need shell inside the image:

| Mode | Path | Expected |
| ---- | ---- | -------- |
| Liveness (web) | `GET /healthcheck` on the **web** port | Plain-text response containing **OK** |
| Readiness (web) | `GET /health/ready` on the **web** port | **200** when Postgres is reachable; **503** when not |
| API enabled | Same paths on the **API** port | **API OK** text on `/healthcheck` when the API server is enabled |

Use **readiness** on `/health/ready` if you want Coolify to wait for database connectivity before sending traffic.

Suggested intervals match your other BEARS services (for example 30s interval, generous start period on cold Rust startup).

### 8. Restart policy

Set restart policy to **unless stopped** (or your platform equivalent) so Den recovers after host reboots.

### 9. Deploy

Use **Deploy** / **Redeploy** on the resource. Watch **Build logs** for compile failures and **Application logs** for runtime config errors (missing `DATABASE_URL`, unreachable Letta, etc.).

### 10. Networking with Letta and Bifrost

- If Den and Letta are **different** Coolify resources, attach them to a **shared Docker network** (Coolify’s “connect to predefined network” / equivalent) so internal DNS names resolve.
- Set `LETTA_BASE_URL` to Letta’s **internal** URL (scheme + host + port, no path suffix).
- Operator-facing Bifrost integration (when present in your build) is configured via env keys documented in [`.env.example`](.env.example); align hostnames with your Bifrost service name inside Coolify.

---

## Option B (recommended): Pre-built image from CI — “Docker Image” resource

A GitHub Actions workflow ([`.github/workflows/den-image.yml`](../.github/workflows/den-image.yml)) builds the Docker image on every push to `main` that touches `den/` and pushes it to GHCR. This avoids compiling Rust on the Coolify host (which can OOM on small servers).

The workflow:

- Builds with **`SQLX_OFFLINE=true`** against the committed [`.sqlx/`](.sqlx/) metadata (no database needed at build time).
- Tags images as **`ghcr.io/theartificial/den:latest`** and **`ghcr.io/theartificial/den:<short-sha>`**.
- Uses GitHub Actions layer cache (`type=gha`) so unchanged layers are reused across builds.

### Coolify setup

1. **Add New Resource** → **Docker Image**.
2. Set **Image** to `ghcr.io/theartificial/den:latest` (or pin a SHA tag for reproducibility).
3. If the GHCR package is **private**, add a registry credential in Coolify (**Keys & Tokens** → **Docker Registry**) using a GitHub PAT with `read:packages` scope.
4. Configure **environment variables**, **ports**, **health checks**, and **restart policy** exactly as in **Option A §5–§8**.
5. The CI workflow triggers a Coolify redeploy automatically via webhook after a successful image push. Set two **GitHub repository secrets** (**Settings → Secrets → Actions**):

   | Secret | Where to find it |
   | ------ | ---------------- |
   | `COOLIFY_WEBHOOK` | Coolify dashboard → your Den resource → **Webhooks** → copy the deploy URL. |
   | `COOLIFY_TOKEN` | Coolify dashboard → **Keys & Tokens** (or **API Tokens**) → create a token with **deploy** permission. |

### Keeping `.sqlx/` up to date

When you add or change SQLx queries, run `cargo sqlx prepare` locally against a database with current migrations applied, then commit the updated `.sqlx/` directory. The CI build will fail if the metadata is stale.

New versions still apply migrations automatically on first container start against the configured `DATABASE_URL`.

---

## Verify (without the shell)

After deploy:

1. Open the **Logs** tab on the Den resource and confirm the process started without configuration errors.
2. If you assigned a public domain in Coolify, open **`https://<your-host>/healthcheck`** in a browser — you should see a short **OK**-style response for the web server.
3. Optionally open **`/health/ready`** — expect success only when the database is reachable.
4. For the full operator experience, load the **web root** `/` and complete any first-run or sign-in flows your deployment enables.

---

## Troubleshooting

| Symptom | What to check in Coolify |
| ------- | ------------------------ |
| **Build fails** during `cargo build` / SQLx | **`DATABASE_URL` build arg** reachable from the build server for compile-time checks; repo includes committed [`.sqlx/`](.sqlx/) if you use offline builds. |
| **Build killed / exit 255 with no compiler error** | Likely OOM during the Rust link step. Switch to **Option B** (CI-built image) or add swap / RAM to the build host. |
| **Container exits immediately** | **Logs** — missing or invalid `DATABASE_URL`, or a **migration error** (DDL permissions, broken migration, incompatible existing schema). |
| **Running but `/health/ready` is 503** | Database credentials or network from the Den container to Postgres; if the process exits instead, check logs for migration failures. |
| **Letta provisioning fails** | `LETTA_BASE_URL` scheme/host/port; shared network with Letta; `LETTA_API_KEY` matches Letta’s server password / auth configuration. |
| **Sessions or redirects wrong** | `WEB_SERVER_URL` / `API_SERVER_URL` and (if used) `SESSION_COOKIE_DOMAIN` must match the URLs users actually use. |

---

## Reference

- Example env keys: [`.env.example`](.env.example)
- Deploy and SQLx notes: [`docs/deploy.md`](docs/deploy.md)
- Ports, health endpoints, toggles: [`docs/infrastructure-and-ops.md`](docs/infrastructure-and-ops.md)
- Stack placement: [`docs/deployment/DEPLOYMENT.md`](../docs/deployment/DEPLOYMENT.md)
- Den + Letta architecture: [`docs/architecture/DEN_ARCHITECTURE.md`](../docs/architecture/DEN_ARCHITECTURE.md)
