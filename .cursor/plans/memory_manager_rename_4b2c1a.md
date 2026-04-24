---
name: Memory Manager rename
overview: Rename the BEARS git smart-HTTP sidecar from the informal "memfs sidecar" to the product name **Memory Manager** (path / service identifier **mem-manager**, kebab-case), including Docker service name, directory layout, and documentation—while keeping Letta/env vocabulary (`LETTA_MEMFS_SERVICE_URL`, on-disk `memfs/repository` paths) where it refers to upstream or filesystem layout.
todos:
  - id: move-service
    content: "Move `services/memfs-sidecar` → `services/api`; rename entry script (e.g. `git_memfs_server.py` → `server.py` or `memory_manager.py`); update Dockerfile COPY/CMD; adjust log lines and SERVER_NAME to reflect Memory Manager / mem-manager"
    status: pending
  - id: compose-preflight
    content: "Root `docker-compose.yaml`: service `bear-memfs` → `bears-memfs-manager`, build context `services/api`, all `depends_on` and default URLs `http://bears-memfs-manager:8285`; update header comments; `services/preflight/preflight.py` default URL"
    status: pending
  - id: docs-env
    content: "Update all deployment docs, Coolify deploy pages, `*.env.example`, `DEPLOYMENT.md`, `codepool` README, `services/letta` notes—replace host `bear-memfs` with `bears-memfs-manager` and describe service as Memory Manager; note `LETTA_MEMFS_SERVICE_URL` is unchanged (Letta contract)"
    status: pending
  - id: code-comments
    content: "Update Rust/TS comments that say `bear-memfs` or memfs sidecar to Memory Manager / `bears-memfs-manager` where they refer to this service; keep Letta/CLI terms `memfs` in codepool pool flags, Letta paths `~/.letta/memfs/...`, and migration comment text if they denote upstream only"
    status: pending
  - id: plans-cursor
    content: "Update `.cursor/plans/bear_memfs_head_ui_*.plan.md` and `modern_letta_memfs_*.plan.md` samples to new service name where they reference the sidecar; point prerequisite order (rename first, then head UI work)"
    status: pending
isProject: false
---

# Memory Manager (`mem-manager`) — rename from memfs sidecar

## Goals

- **Human name:** Memory Manager
- **Path / service slug (kebab-case):** `mem-manager` → service lives under [`services/api/`](services/api/) (replaces `services/memfs-sidecar/`)
- **Docker Compose service name:** `bears-memfs-manager` (matches `bears-pool`, `bear-bifrost`; resolvable as `http://bears-memfs-manager:8285` on the stack network)

**Naming convention:** Use **hyphens** in directory names, compose services, URLs, and log tags (`mem-manager`). In **Rust**, use normal **snake_case** for variables and modules (e.g. `mem_manager`, `fetch_memory_manager_head`)—do not force kebab-case into identifiers where the language forbids it.

## What we deliberately do *not* rename (avoid breaking Letta or on-disk meaning)

- **`LETTA_MEMFS_SERVICE_URL`** — Still the single env var **Letta** and **BEARS** use for the **base URL of the git smart-HTTP** endpoint. The value’s **hostname** changes to `bears-memfs-manager`; the **variable name** stays (upstream docs and existing deployments expect it).
- **Paths like `/root/.letta/memfs/repository`** and env **`MEMFS_BASE`** (optional: add a one-line comment in the Dockerfile that this is Letta’s *LocalStorageBackend* path name, not the service’s display name).
- **Letta Code / Codepool flags** — e.g. `LETTA_MEMFS_LOCAL`, `CODEPOOL_DISABLE_MEMFS`, CLI `--memfs` — these are **upstream** product terms; do not rename identifiers in TypeScript unless we only touch **comment strings** that refer to *our* container hostname (`bear-memfs` → `bears-memfs-manager`).

## Concrete inventory (files to touch)

### Must change

| Area | Action |
|------|--------|
| [docker-compose.yaml](docker-compose.yaml) | Service key `bear-memfs` → `bears-memfs-manager`; `context: ./services/api`; all `http://bear-memfs:8285` → `http://bears-memfs-manager:8285`; `depends_on` for Letta |
| [services/memfs-sidecar/](services/memfs-sidecar/) | **Move** tree to `services/api/`; [Dockerfile](services/memfs-sidecar/Dockerfile) paths; [git_memfs_server.py](services/memfs-sidecar/git_memfs_server.py) — rename file if desired, update module docstring, `SERVER_NAME`, log prefix (`[git-memfs]` → e.g. `[mem-manager]`) |
| [services/preflight/preflight.py](services/preflight/preflight.py) | Default URL host `bear-memfs` → `bears-memfs-manager` |
| [services/worker/src/core/bears/rollout.rs](services/worker/src/core/bears/rollout.rs) | URL in comment |
| [codepool](codepool) | Comments and `.env.example` that cite `bear-memfs:8285` |
| [services/letta/.env.example](services/letta/.env.example) | Example URL host |
| [docs/deployment/DEPLOYMENT.md](docs/deployment/DEPLOYMENT.md) | Table row for service; all `bear-memfs` mentions |
| [services/frontend/COOLIFY_DEPLOY.md](services/frontend/COOLIFY_DEPLOY.md), [services/letta/COOLIFY_DEPLOY.md](services/letta/COOLIFY_DEPLOY.md) | Service name + URLs |
| [services/frontend/README.md](services/frontend/README.md) | Same |

### Light touch (clarity only)

| Area | Action |
|------|--------|
| [services/worker/src/core/letta/client.rs](services/worker/src/core/letta/client.rs) | Comment: `LETTA_MEMFS_SERVICE_URL` + "Memory Manager (`bears-memfs-manager`)" |
| [services/frontend/src/provisioning/noop.ts](services/frontend/src/provisioning/noop.ts), [index.ts](services/frontend/src/provisioning/index.ts) | Example URL host in comments |
| [services/worker/migrations/20260421120000_bear_runtime_plan.up.sql](services/worker/migrations/20260421120000_bear_runtime_plan.up.sql) | Optional: comment only — "memfs" in comment refers to plan JSON field / Letta, not service rename |

### Cursor / planning docs

- [`.cursor/plans/bear_memfs_head_ui_ef2450cc.plan.md`](.cursor/plans/bear_memfs_head_ui_ef2450cc.plan.md) — After rename: use **`bears-memfs-manager`** in examples and "Memory Manager" in prose; state **prerequisite: complete this rename** before implementing the head endpoint.
- [`.cursor/plans/modern_letta_memfs_bears_fabf06fb.plan.md`](.cursor/plans/modern_letta_memfs_bears_fabf06fb.plan.md) — Optional: add a footnote where "sidecar" is mentioned that the service is now called Memory Manager.

## Operator / migration note (for deploy docs)

- On upgrade, **Coolify/Compose** will recreate the service with the new name; **same image and volume** attachment as before (`bear-letta-data`). Any **custom** `LETTA_MEMFS_SERVICE_URL` that still points at `http://bear-memfs:8285` must be updated to **`http://bears-memfs-manager:8285`**.
- Grep the repo for **`bear-memfs`** after edits to catch stragglers.

## Order relative to the bear head UI plan

1. **Execute this rename** (or land it in a dedicated PR) so new Den management API paths live on a service already named Memory Manager in compose and docs.
2. **Then** implement the memfs head / private-memory view plan against `services/api` and `bears-memfs-manager`.
