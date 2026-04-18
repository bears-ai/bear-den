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

- **This repo** (`bears-depoy`) — configs and docs for Letta, Bifrost, Open WebUI, etc.
- **Letta** — **bear** runtime: one Letta agent per bear; native memory (blocks, conversations).
- **Cabinet** — shared knowledge in **Outline**, exposed to **bears** through **Den** ([PLAN.md](../planning/PLAN.md)).

## Prerequisites

- Coolify v4+
- ~4 GB RAM minimum (Letta + Bifrost + Open WebUI)
- API keys: OpenAI and/or Anthropic (and others per `services/bifrost/config.json`)

## Architecture

```
Open WebUI → Letta → Bifrost → model providers
                                Garage (S3) ← Den (presigned upload/download for chat media)
(Target with Den: Den chat UI / Open WebUI → Den → **Letta Code** → Letta; Den provisions bears + membership + **skills per bear** — [PLAN.md](../planning/PLAN.md), [DEN_ARCHITECTURE.md](../architecture/DEN_ARCHITECTURE.md))
(Optional: Outline/Cabinet with Den per PLAN.md)
```

## Deployment order

1. **Bifrost** — model gateway  
2. **Garage** — S3-compatible object storage (chat media, generated images)  
3. **Letta** — must reach Bifrost  
4. **Open WebUI** — chat UI + open-webui-tools → Letta  
5. **Outline + Den** — when enabling Cabinet ([PLAN.md](../planning/PLAN.md)); Den needs Garage credentials

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

### Step 4: Open WebUI

1. Image: `ghcr.io/open-webui/open-webui:main`, port `3000`  
2. Secrets: `WEBUI_SECRET_KEY`, `WEBUI_JWT_SECRET_KEY` (generate with `openssl rand -base64 32`)  
3. Letta: `LETTA_API_URL=http://bears-letta:8283/v1`, `LETTA_SERVER_PASS=<same as Letta>`  
4. Optional: Coolify **PostgreSQL** + `DATABASE_URL` for production multi-user Open WebUI  
5. Volume: `bears-openwebui-data` → `/app/backend/data`  
6. Health: `GET /api/health`

### Step 5: Open WebUI ↔ Letta (open-webui-tools)

1. Open WebUI → **Settings** → **Workspace** → **Functions**  
2. Install Letta integration from [open-webui-tools](https://github.com/Haervwe/open-webui-tools) (or `../../services/letta/openwebui_pipe_example.py`)  
3. **Settings** → **Models**: register Letta-backed models  

Multi-user **Den (Axum)** + self-hosted Letta: [DEN_ARCHITECTURE.md](../architecture/DEN_ARCHITECTURE.md). Direct Open WebUI sessions: [`../../services/letta/OPENWEBUI_SESSIONS.md`](../../services/letta/OPENWEBUI_SESSIONS.md).

### Step 6: Outline & Den (Cabinet)

Follow [PLAN.md](../planning/PLAN.md) when you deploy the control plane and Outline-backed Cabinet.

## Post-deployment

- Open WebUI: chat with **bears** (Letta agents) via configured models  
- Letta UI (internal): **bear** / agent and memory management at `:8283`  
- Add **Den** + **Outline** for shared knowledge, **users↔bears** membership, and channel routing  

## Verification

| Check | Command / action |
|-------|------------------|
| Bifrost | `curl http://bears-bifrost:8080/health` |
| Garage | `curl http://bears-garage:3903/health` |
| Letta | `curl http://bears-letta:8283/v1/health` |
| Open WebUI | `curl http://bears-openwebui:3000/api/health` |

End-to-end: create a **bear** in Letta (or via Den when deployed), select it in Open WebUI, send a message.

## Troubleshooting

- **Letta ↔ Bifrost:** `LLM_API_URL` must match Bifrost’s internal URL and `/v1` suffix; provider keys must be valid on the Bifrost service.  
- **Open WebUI ↔ Letta:** function `LETTA_API_URL` and `LETTA_SERVER_PASS`  
- **Open WebUI DB:** if using Postgres, verify `DATABASE_URL` and network to DB  

Service-specific detail: `services/*/COOLIFY_DEPLOY.md`.

## Support

- [ARCHITECTURE_NOTES.md](../architecture/ARCHITECTURE_NOTES.md)  
- [PLAN.md](../planning/PLAN.md) — Den, Cabinet, Outline  
- [DEN_ARCHITECTURE.md](../architecture/DEN_ARCHITECTURE.md)  

---

**Deployment complete.** Multi-user production with **many bears per user**, **shared bears**, and Den: see [DEN_ARCHITECTURE.md](../architecture/DEN_ARCHITECTURE.md) and [PLAN.md](../planning/PLAN.md).
