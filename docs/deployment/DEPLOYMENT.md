# BEARS Stack — Coolify Deployment Guide

Deploy BEARS on Coolify from the repository root `docker-compose.yaml`. This is the supported path for operators: one Compose resource, one shared network, and service names that resolve internally as `bears-*`.

## What You Deploy

Use the root [`docker-compose.yaml`](../../docker-compose.yaml). It starts the application services:

| Service | Purpose |
| ------- | ------- |
| `bears-bifrost` | Model gateway on port `8080` |
| `bears-redis` | Redis for Letta git/memfs locking |
| `bears-memfs-manager` | MemFS Manager git HTTP service on port `8285` |
| `bears-letta` | Letta API on port `8283` |
| `bears-codepool` | Letta Code SDK harness on port `3030` |
| `bears-den` | Den web UI/control plane on port `3000` and Den API/ACP gateway on port `3001` |

Databases are separate Coolify resources:

| Database | Used By | Environment Variable |
| -------- | ------- | -------------------- |
| Postgres | Den | `DATABASE_URL` |
| PGVector/Postgres | Letta | `LETTA_PG_URI` |

## Requirements

- Coolify v4+
- One Postgres database for Den
- One PGVector/Postgres database for Letta
- An OpenAI API key
- Public access for Den web, Den API, and Letta. The API can be a subdomain such as `api.bears.[domain]`, a separate hostname, or a published port on the web host.

## 1. Create The Databases

Create these first in Coolify:

1. Postgres for Den.
2. PGVector/Postgres for Letta.

Copy each database's **Postgres URL (internal)** value. You will paste those into the Compose resource environment variables.

For local/devcontainer runs, the `bundled` compose profile provides two separate databases: `bears-postgres` for Den and `bears-letta-postgres` with PGVector enabled for Letta. Production should still prefer managed databases.

## 2. Create The Compose Resource

In Coolify:

1. Go to **Add Resource**.
2. Choose **Docker Compose**.
3. Select this repository.
4. Set **Build Pack** to `Docker Compose`.
5. Set **Base Directory** to `.`.
6. Set **Compose File** to `docker-compose.yaml`.

## 3. Configure Domains

In the Compose resource general configuration:

1. Set a domain for `bears-letta` with port suffix `:8283`.
2. Set the primary web domain for `bears-den` with port suffix `:3000`.
3. Set public access for the Den API with port suffix `:3001`. A subdomain like `api.bears.[domain]` is the recommended convention, but not a requirement; you can also use another hostname or a published port on the web host.
4. Under **Build**, enable **Preserve Repository During Deployment**.
5. Save.

Den web is the browser-facing UI. Den API is the bearer-token machine-client surface and hosts the ACP gateway used by local ACP adapters such as `bears-acp-adapter`. Letta can be public or restricted depending on your operator workflow, but it still needs a configured domain if you want to access the Letta UI/API from outside the Docker network.

## 4. Connect The Network

In the Compose resource advanced settings:

1. Open **Docker Compose**.
2. Enable **Connect To Predefined Network**.
3. Save.

This keeps the `bears-*` service names stable for internal URLs such as `http://bears-letta:8283` and `http://bears-codepool:3030`.

## 5. Set Environment Variables

Set these on the Compose resource:

| Variable | Value |
| -------- | ----- |
| `JWT_SECRET` | Random secret string |
| `LETTA_SERVER_PASS` | Random secret string; also used as the Letta API key by Den and Codepool |
| `OPENAI_API_KEY` | Your OpenAI API key |
| `DATABASE_URL` | Den Postgres **Postgres URL (internal)** from Coolify |
| `LETTA_PG_URI` | Letta PGVector/Postgres **Postgres URL (internal)** from Coolify |
| `WEB_SERVER_URL` | Public Den web origin, e.g. `https://bears.[domain]` |
| `API_SERVER_URL` | Public Den API origin, e.g. `https://api.bears.[domain]` or `https://bears.[domain]:3001` |

Optional:

| Variable | Value |
| -------- | ----- |
| `CODEPOOL_INTERNAL_TOKEN` | Random shared token if you want Den to authenticate to Codepool |
| `ACP_GATEWAY_ENABLED` | Defaults to `true`; set to `false` only if you do not want the Den API ACP gateway mounted |
| `DEN_IMAGE` | Override the prebuilt Den image |
| `CODEPOOL_IMAGE` | Override the prebuilt Codepool image |
| `DEN_PULL_POLICY` / `CODEPOOL_PULL_POLICY` | Leave as `always` in production; dev/smoke sets `never` with locally built images |

You usually do not need to set internal service URLs. The compose file already defaults to:

| Variable | Default |
| -------- | ------- |
| `LETTA_BASE_URL` | `http://bears-letta:8283` |
| `CODEPOOL_BASE_URL` | `http://bears-codepool:3030` |
| `LLM_API_URL` | `http://bears-bifrost:8080/v1` |
| `LETTA_MEMFS_SERVICE_URL` | `http://bears-memfs-manager:8285` |

## 6. Deploy

Click **Deploy**.

If deploy preflight fails, check the missing environment variable in the logs first. The compose file intentionally defaults required secrets and database URLs to `SETME` so bad deploys fail early.

## Verification

From Coolify's terminal for a service on the same network:

| Check | Command |
| ----- | ------- |
| Bifrost | `curl http://bears-bifrost:8080/health` |
| Letta | `curl http://bears-letta:8283/v1/health` |
| Codepool | `curl http://bears-codepool:3030/health` |
| Den web | Open the public Den URL |
| Den API | `curl ${API_SERVER_URL}/health` |
| ACP gateway auth check | `curl -i -X POST ${API_SERVER_URL}/acp/bears/test-bear/sessions/smoke-session/prompt -H 'Content-Type: application/json' -d '{"message":"hello"}'` should return `401` without a bearer token |

End-to-end check: create or open a bear in Den, go to its chat page, and send a message.

## Troubleshooting

- If Den cannot start, confirm `DATABASE_URL`, `JWT_SECRET`, `WEB_SERVER_URL`, `API_SERVER_URL`, and `CODEPOOL_BASE_URL`.
- If Letta cannot start, confirm `LETTA_PG_URI`, `LETTA_SERVER_PASS`, `OPENAI_API_KEY`, and `LLM_API_URL`.
- If chat does not stream, confirm `bears-den` can reach `http://bears-codepool:3030` and `bears-codepool` can reach `http://bears-letta:8283`.
- If Bifrost is unhealthy, confirm `OPENAI_API_KEY` and `services/bifrost/config.json`.

## Optional Backups

The root compose file includes `bears-letta-data-backup` behind the `volume-backup` profile for backing up the Letta data volume to S3-compatible storage. Enable it only after the base stack is healthy.

Set `COMPOSE_PROFILES=volume-backup` and provide the `SCALEWAY_*` backup variables if you use that profile.

---

Last updated: 2026-04-26
