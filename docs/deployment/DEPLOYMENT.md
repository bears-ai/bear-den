# BEARS Stack — Coolify Deployment Guide

Deploy the BEARS stack as separate services in Coolify. Shared knowledge uses **Outline (Cabinet)** via **Den** when you add them—see [PLAN.md](../planning/PLAN.md).

## Table of contents

1. [Overview](#overview)
2. [Prerequisites](#prerequisites)
3. [Architecture](#architecture)
4. [Deployment order](#deployment-order)
5. [Step-by-step](#step-by-step-deployment)
6. [Post-deployment](#post-deployment)
7. [Verification](#verification)
8. [Troubleshooting](#troubleshooting)

## Overview

- **This repo** (`bears-depoy`) — configs and docs for Letta, Bifrost, Garage, Den, etc.
- **Letta** — **bear** runtime: one Letta agent per bear; native memory (blocks, conversations).
- **Cabinet** — shared knowledge in **Outline**, exposed to **bears** through **Den** ([PLAN.md](../planning/PLAN.md)).

## Prerequisites

- Coolify v4+
- ~4 GB RAM minimum (Letta + Bifrost + Den; add headroom for Garage and operator workloads)
- API keys: OpenAI and/or Anthropic (and others per `services/bifrost/config.json`)

## Architecture

```
Den chat UI ──► Den ──► code-pool (Letta Code SDK) ──► Letta ──► Bifrost ──► model providers
                    │                      ▲
                    │                      │
                    └── Garage (S3) ←──────┘ (presigned upload/download for chat media)
(Optional: Outline/Cabinet with Den per PLAN.md)
```

**Web chat:** **Den embedded Deep Chat** → **Den** → **`code-pool/`** (harness) → **Letta** — [PLAN.md](../planning/PLAN.md), [DEN_ARCHITECTURE.md](../architecture/DEN_ARCHITECTURE.md).

## Deployment order

1. **Bifrost** — model gateway  
2. **Garage** — S3-compatible object storage (chat media, generated images, artifacts)  
3. **Letta** — must reach Bifrost (Letta HTTP API / persistence)  
4. **code-pool** — Letta Code SDK harness ([`../../code-pool/COOLIFY_DEPLOY.md`](../../code-pool/COOLIFY_DEPLOY.md)); streaming chat Den → **code-pool** → Letta  
5. **Den** — control plane + first-party web chat (bridge to **`CODE_POOL_BASE_URL`**, membership, operator console)  
6. **Outline + Den Cabinet wiring** — when enabling Cabinet ([PLAN.md](../planning/PLAN.md)); Den needs Garage credentials  

## Step-by-step deployment

### Step 1: Bifrost

Part of the overall order in this guide; details: [`../../services/bifrost/COOLIFY_DEPLOY.md`](../../services/bifrost/COOLIFY_DEPLOY.md).

**Recommended:** deploy from **Git** with Coolify’s **Docker Compose** build pack using [`../../services/bifrost/docker-compose.yml`](../../services/bifrost/docker-compose.yml) so `config.json` is taken from the repository on each deploy (enable **Preserve Repository During Deployment** in Coolify).

Otherwise (plain **Docker Image**): service name e.g. `bears-bifrost`, port `8080` (`APP_HOST=0.0.0.0`, `APP_PORT=8080`), provider keys in Coolify env, mount `services/bifrost/config.json` → `/app/data/config.json` (read-only).

- Health: `GET http://bears-bifrost:8080/health`

### Step 2: Garage

See [`../../services/garage/COOLIFY_DEPLOY.md`](../../services/garage/COOLIFY_DEPLOY.md).

[Garage](https://garagehq.deuxfleurs.fr/) is the BEARS object store (S3-compatible, self-hosted, Rust). Deploy before Den.

- Image: `dxflrs/garage:v2.2.0`, config via `garage.toml`  
- S3 API port: `3900` (internal), admin: `3903`  
- After deploy: create bucket `bears-media` + service key for Den  
- Health: `garage stats -a` or `GET http://bears-garage:3903/health`

### Step 3: Letta

See [`../../services/letta/COOLIFY_DEPLOY.md`](../../services/letta/COOLIFY_DEPLOY.md).

- `LLM_API_URL=http://bears-bifrost:8080/v1`  
- `LETTA_SERVER_PASS`, `OPENAI_API_KEY` (embeddings; chat completions go through Bifrost)  
- Volume: `bears-letta-data` → `/root/.letta`  
- Health: `GET http://bears-letta:8283/v1/health`

### Step 4: code-pool (Letta Code SDK harness)

See [`../../code-pool/COOLIFY_DEPLOY.md`](../../code-pool/COOLIFY_DEPLOY.md).

- Deploy **`bears-code-pool`** from **`code-pool/`** (Node, **`@letta-ai/letta-code-sdk`**).  
- **`LETTA_BASE_URL=http://bears-letta:8283`** and **`LETTA_API_KEY`** matching Letta’s server credential (same as Den uses for provisioning).  
- Persist **`~/.letta`** on a volume (CLI auth and local state).  
- **`CODE_POOL_BASE_URL`** in Den must point at this service (e.g. `http://bears-code-pool:3030`). Optional shared secret: **`CODE_POOL_INTERNAL_TOKEN`** on both sides.

### Step 5: Den

Build and deploy Den from repo root **`den/`** (Rust/Axum) — see [PHASE1_BOOTSTRAP.md](../planning/PHASE1_BOOTSTRAP.md) for routes and env expectations. Den serves the **embedded Deep Chat** UI and proxies streaming chat **Den → code-pool → Letta** per [DEN_ARCHITECTURE.md](../architecture/DEN_ARCHITECTURE.md).

- **`LETTA_BASE_URL`** — Letta from Step 3 (persistence, history, provisioning). **`CODE_POOL_BASE_URL`** — harness from Step 4 (streaming agent loop).  
- Garage: bucket + credentials for presigned URLs when media upload is enabled  

### Step 6: Outline & Den (Cabinet)

Follow [PLAN.md](../planning/PLAN.md) when you deploy the control plane and Outline-backed Cabinet.

## Post-deployment

- **Den chat UI:** end users chat at Den’s **`/bear/{slug}`** (or equivalent) routes — **Den → code-pool → Letta**  
- Letta UI (internal): **bear** / agent and memory management at `:8283`  
- Add **Outline** for shared knowledge, **users↔bears** membership, and channel routing  

## Verification

| Check | Command / action |
|-------|------------------|
| Bifrost | `curl http://bears-bifrost:8080/health` |
| Garage | `curl http://bears-garage:3903/health` |
| Letta | `curl http://bears-letta:8283/v1/health` |
| Den | `curl` your Den health or `/` per deploy (see `den/` docs) |

End-to-end: create a **bear** in Letta (or via Den when deployed), open the bear’s chat page on Den, send a message.

## Troubleshooting

- **Letta ↔ Bifrost:** `LLM_API_URL` must match Bifrost’s internal URL and `/v1` suffix; provider keys must be valid on the Bifrost service.  
- **Den ↔ code-pool:** confirm `CODE_POOL_BASE_URL`, optional `CODE_POOL_INTERNAL_TOKEN`, and Docker network DNS names match your Coolify service names.  

Service-specific detail: `services/*/COOLIFY_DEPLOY.md`.

## Support

- [ARCHITECTURE_NOTES.md](../architecture/ARCHITECTURE_NOTES.md)  
- [PLAN.md](../planning/PLAN.md) — Den, Cabinet, Outline  
- [DEN_ARCHITECTURE.md](../architecture/DEN_ARCHITECTURE.md)  

---

*Last updated: 2026-04-19*
