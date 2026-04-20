# Codepool — Coolify deployment

**BEARS harness** (Letta Code SDK): warm conversation session pool, streaming endpoints for **Den**, optional channel listener hooks, **`GET /internal/pool`** metrics.

**Stack order:** Deploy **after** [Letta](../services/letta/COOLIFY_DEPLOY.md) (persistence API). Deploy **before or with** [Den](../den/COOLIFY_DEPLOY.md). See [DEPLOYMENT.md](../docs/deployment/DEPLOYMENT.md).

**Recommended:** use the **repository root** [`docker-compose.yaml`](../docker-compose.yaml) so **`bear-codepool`** shares the **`bear-stack`** network with **`bear-letta`**, **`bear-den`**, and **Bifrost**. Postgres is usually **managed** outside compose (`DATABASE_URL` on Den); optional **`bear-postgres`** uses profile **`bundled`** (see [DEPLOYMENT.md](../docs/deployment/DEPLOYMENT.md)).

## What this service is

- **Not** the Letta server container — Codepool calls **`LETTA_BASE_URL`** (same Letta API Den uses for provisioning and history).
- **Not** under `services/` — build context is repo root **`codepool/`** (sibling of **`den/`**).

## Integration (Den)

| Den env | Purpose |
|--------|---------|
| `CODEPOOL_BASE_URL` | `http://bear-codepool:3030` (internal, no trailing slash) |
| `CODEPOOL_INTERNAL_TOKEN` | Optional; must match `CODEPOOL_INTERNAL_TOKEN` here |

Den uses **`LETTA_BASE_URL`** for conversation list/history; **streaming sends** go to **Codepool** when `CODEPOOL_BASE_URL` is set.

## Pre-built image (recommended)

A GitHub Actions workflow ([`.github/workflows/codepool-image.yml`](../.github/workflows/codepool-image.yml)) builds on every push to `main` that touches `codepool/` and pushes to GHCR:

- Tags: **`ghcr.io/<github-org>/codepool:latest`** and **`ghcr.io/<github-org>/codepool:<short-sha>`** (use `github.repository_owner` in the workflow; override **`CODEPOOL_IMAGE`** in compose for forks).
- **Path filter:** only `codepool/**` (and the workflow file) — same idea as [Den’s workflow](../.github/workflows/den-image.yml).
- After a successful push, the workflow calls the same **Coolify webhook** as Den (`COOLIFY_WEBHOOK` + `COOLIFY_TOKEN` repository secrets) so the stack can redeploy and pull the new image.

If the GHCR package is **private**, authenticate on the Coolify host (`docker login ghcr.io` with a PAT that has `read:packages`), same as [Den’s guide](../den/COOLIFY_DEPLOY.md#coolify-setup).

**Build locally instead of GHCR:** from the repo root,  
`docker compose -f docker-compose.yaml -f docker-compose.codepool-src.yaml build bear-codepool`

## Coolify (Docker Compose from Git)

1. **Add Resource** → **Docker Compose** → this repository.
2. **Base Directory:** `.` (repo root) and **[`docker-compose.yaml`](../docker-compose.yaml)** — recommended — **or** base directory **`codepool`** with [`docker-compose.yaml`](docker-compose.yaml) (both use the **`CODEPOOL_IMAGE`** pull by default).
3. Set **`LETTA_BASE_URL`**, **`LETTA_API_KEY`**, optional **`CODEPOOL_INTERNAL_TOKEN`**.
4. If you did **not** use the root compose file, attach the **same Docker network** as Den and Letta so **`bear-codepool`** resolves.

## Health

- `GET /health` — liveness  
- `GET /internal/pool` — conversation + channel listener stats (protect with bearer token if `CODEPOOL_INTERNAL_TOKEN` is set)

## Volume

Mount a persistent volume on **`/home/node/.letta`** (or `/root/.letta` if running as root) for Letta Code CLI auth and agent-local state — align with your image `USER` (this Dockerfile runs as **`node`**; data under `/home/node/.letta`).
