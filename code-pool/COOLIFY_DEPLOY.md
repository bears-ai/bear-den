# code-pool — Coolify deployment

**BEARS harness** (Letta Code SDK): warm conversation session pool, streaming endpoints for **Den**, optional channel listener hooks, **`GET /internal/pool`** metrics.

**Stack order:** Deploy **after** [Letta](../services/letta/COOLIFY_DEPLOY.md) (persistence API). Deploy **before or with** [Den](../den/COOLIFY_DEPLOY.md). See [DEPLOYMENT.md](../docs/deployment/DEPLOYMENT.md).

## What this service is

- **Not** the Letta server container — `code-pool` calls **`LETTA_BASE_URL`** (same Letta API Den uses for provisioning and history).
- **Not** under `services/` — build context is repo root **`code-pool/`** (sibling of **`den/`**).

## Integration (Den)

| Den env | Purpose |
|--------|---------|
| `CODE_POOL_BASE_URL` | `http://bears-code-pool:3030` (internal, no trailing slash) |
| `CODE_POOL_INTERNAL_TOKEN` | Optional; must match `CODE_POOL_INTERNAL_TOKEN` here |

Den uses **`LETTA_BASE_URL`** for conversation list/history; **streaming sends** go to **code-pool** when `CODE_POOL_BASE_URL` is set.

## Coolify (Docker Compose from Git)

1. **Add Resource** → **Docker Compose** → this repository.
2. **Base Directory:** `code-pool` · **Compose file:** [`docker-compose.yaml`](docker-compose.yaml).
3. Set **`LETTA_BASE_URL`**, **`LETTA_API_KEY`**, optional **`CODE_POOL_INTERNAL_TOKEN`**.
4. Attach the **same Docker network** as Den and Letta.

## Health

- `GET /health` — liveness  
- `GET /internal/pool` — conversation + channel listener stats (protect with bearer token if `CODE_POOL_INTERNAL_TOKEN` is set)

## Volume

Mount a persistent volume on **`/home/node/.letta`** (or `/root/.letta` if running as root) for Letta Code CLI auth and agent-local state — align with your image `USER` (this Dockerfile runs as **`node`**; data under `/home/node/.letta`).
