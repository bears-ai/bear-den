# Codepool — Coolify deployment

**BEARS harness** (Letta Code SDK): warm conversation session pool, streaming endpoints for **Den**, optional channel listener hooks, **`GET /internal/pool`** metrics.

**Stack order:** Deploy **after** [Letta](../letta/COOLIFY_DEPLOY.md) (persistence API). Deploy **before or with** [Den](../den/COOLIFY_DEPLOY.md). See [DEPLOYMENT.md](../../docs/deployment/DEPLOYMENT.md).

**Recommended:** use the **repository root** [`docker-compose.yaml`](../../docker-compose.yaml) so **`bears-codepool`** shares the **`bears-stack`** network with **`bears-letta`**, **`bears-den`**, and **Bifrost**. Postgres is usually **managed** outside compose (`DATABASE_URL` on Den); optional **`bears-postgres`** uses profile **`bundled`** (see [DEPLOYMENT.md](../../docs/deployment/DEPLOYMENT.md)).

## What this service is

- **Not** the Letta server container — Codepool calls **`LETTA_BASE_URL`** (same Letta API Den uses for provisioning and history).
- Build context is **`services/codepool/`** (sibling of **`services/den/`**).

## Integration (Den)

| Den env | Purpose |
|--------|---------|
| `CODEPOOL_BASE_URL` | `http://bears-codepool:3030` (internal, no trailing slash) |
| `CODEPOOL_INTERNAL_TOKEN` | Optional; must match `CODEPOOL_INTERNAL_TOKEN` here |

Den uses **`LETTA_BASE_URL`** for conversation list/history; **streaming sends** go to **Codepool** when `CODEPOOL_BASE_URL` is set.

## Pre-built image (recommended)

A GitHub Actions workflow ([`.github/workflows/codepool-image.yml`](../../.github/workflows/codepool-image.yml)) builds on every push to `main` that touches `services/codepool/` and pushes to GHCR:

- Tags: **`ghcr.io/<github-org>/codepool:latest`** and **`ghcr.io/<github-org>/codepool:<short-sha>`** (use `github.repository_owner` in the workflow; override **`CODEPOOL_IMAGE`** in compose for forks).
- **Path filter:** only `services/codepool/**` (and the workflow file) — same idea as [Den’s workflow](../../.github/workflows/den-image.yml).
- After a successful push, the workflow calls the **Coolify deploy webhook** (`COOLIFY_WEBHOOK` + `COOLIFY_TOKEN` GitHub secrets). That URL must come from the **same** Coolify application that runs this compose stack (Coolify: resource → **Webhook** → *Deploy webhook*). Den and Codepool workflows typically share one webhook if they deploy **one** Docker Compose project.
- The webhook step is **non-failing** in CI (`continue-on-error`) so a bad webhook does not block the image push; fix secrets and redeploy from Coolify or re-run the workflow after updating them.

**GitHub Actions `curl: (22) … 404`:** Coolify returned **Not Found** for that webhook URL — usually **wrong or outdated `COOLIFY_WEBHOOK`** (copied from another resource, old deployment, or typo). Open the **bears-stack** (or your compose) application in Coolify, copy the current **Deploy webhook** URL, and update the repo secret. Confirm **`COOLIFY_TOKEN`** is a Coolify API token with **Deploy** permission ([Coolify: GitHub Actions](https://coolify.io/docs/applications/ci-cd/github/actions)).

**Coolify `error from registry: unauthorized` when pulling `ghcr.io/.../codepool`:** The Docker daemon on the Coolify server cannot read the image. For **private** GHCR packages, SSH to the host and run `docker login ghcr.io` with a GitHub PAT that has **`read:packages`** (as **root**, since Coolify’s daemon uses root’s config — same as [Den’s GHCR steps](../den/COOLIFY_DEPLOY.md#coolify-setup)). Alternatively, in GitHub → **Packages** → **codepool** → **Package settings**, set visibility to **public** so pulls need no login.

**Build locally instead of GHCR:** from the repo root, build the image directly with `docker build -t bears-codepool:local services/codepool`, then set `CODEPOOL_IMAGE=bears-codepool:local` for local compose runs.

## Coolify (Docker Compose from Git)

1. **Add Resource** → **Docker Compose** → this repository.
2. **Base Directory:** `.` (repo root) and **[`docker-compose.yaml`](../../docker-compose.yaml)**.
3. Set **`LETTA_BASE_URL`**, **`LETTA_API_KEY`** (same as Letta admin password), **`LETTA_MEMFS_LOCAL=1`** (default; **Letta** in root compose uses **MemFS Manager** / **`bears-memfs-manager`** for git HTTP), optional **`CODEPOOL_INTERNAL_TOKEN`**.
4. If you did **not** use the root compose file, attach the **same Docker network** as Den and Letta so **`bears-codepool`** resolves.

## Health

- `GET /health` — liveness  
- `GET /internal/pool` — conversation + channel listener stats (protect with bearer token if `CODEPOOL_INTERNAL_TOKEN` is set)

## Volumes and memory model

**Git-backed memory (upstream):** The **canonical** memfs/git state is on the **Letta** volume **`bears-letta-data`** (under `/root/.letta/memfs/repository/…`); **Letta** and **MemFS Manager** share that volume, and **Letta**’s **`LETTA_MEMFS_SERVICE_URL`** points at **`bears-memfs-manager`**. See [Letta Coolify deploy](../letta/COOLIFY_DEPLOY.md) and [DEPLOYMENT.md](../../docs/deployment/DEPLOYMENT.md). Back up that volume (and Letta Postgres).

**This service:** Mount **`bears-codepool-letta-home` → `/home/node/.letta`** for the Letta Code **CLI** (client-side cache / mirror under your image `USER` **`node`**). It is **not** the primary durability surface.

The Docker image includes **`git`** for any CLI git operations Letta Code requires.
