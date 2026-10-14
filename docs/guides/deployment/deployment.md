# Bear Den Stack — Coolify Deployment Guide

Deploy Bear Den on Coolify from the repository root [`docker-compose.yaml`](../../../docker-compose.yaml). This is the supported path for operators: Coolify owns each resource's project/network identity and may also attach it to the predefined shared network for platform routing and managed resources.

The default stack runs the **in-process Den agent loop** (`AGENT_RUNTIME=native`): inference via Bifrost, Bear memory in per-Bear SQLite on a mounted volume. See [den-runtime.md](../../architecture/den-runtime.md) for the architecture and [den-deploy.md](../den-deploy.md) for single-image env details.

## What You Deploy

Use the root [`docker-compose.yaml`](../../../docker-compose.yaml). It starts:

| Service | Purpose |
| ------- | ------- |
| `bears-preflight-config` | One-shot deploy preflight: required env shape and secrets |
| `bears-preflight-den-db` | One-shot Den Postgres connectivity check (when `bundled` profile provides Postgres) |
| `bears-bifrost` | Model gateway on port `8080` |
| `bears-den` | Den web UI on port `3000` and Den API/BearWire on port `3001` |

Optional compose profiles:

| Profile | Service | Purpose |
| ------- | ------- | ------- |
| `bundled` | `bears-postgres` | Local/dev Den Postgres when you do not use a managed database |
| `cabinet` | `bears-cabinet` | Distinct Cabinet hostname/UI and read-only MCP, using the same Den image/database without native-memory volumes |
| `volume-backup` | `bears-den-sqlite-data-backup` | S3-compatible archive of per-Bear SQLite data |

**Database:** set **`DATABASE_URL`** for `bears-den` (managed Postgres is preferred in Coolify). For local compose only, `COMPOSE_PROFILES=bundled` starts `bears-postgres` with dev defaults.

**Bear memory:** `bears-den` mounts `bears-den-sqlite-data` at `BEAR_SQLITE_DATA_DIR` (default `/var/lib/den/bear-sqlite`). Image upgrades keep Bear memory when this volume is attached.

## Requirements

- Coolify v4+
- One Postgres database for Den (`DATABASE_URL`)
- An OpenAI API key (for Bifrost)
- Public access for Den web and Den API. The API can be a subdomain such as `api.bears.[domain]`, a separate hostname, or a published port on the web host.

## 1. Create The Database

Create Den Postgres in Coolify first. Copy the database's **Postgres URL (internal)** — you will paste it into `DATABASE_URL` on the Compose resource.

For local/devcontainer runs, the `bundled` compose profile provides `bears-postgres` for Den. Production should prefer managed databases.

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

1. Set the primary web domain for `bears-den` with port suffix `:3000`.
2. Set public access for the Den API with port suffix `:3001`. A subdomain like `api.bears.[domain]` is the recommended convention, but not a requirement; you can also use another hostname or a published port on the web host.
3. Under **Build**, enable **Preserve Repository During Deployment**.
4. Save.

Den web is the Bear administration/chat UI. Den API is the bearer-token machine-client surface and hosts BearWire for local armatures such as `bear-armature`.

### Dedicated Cabinet hostname

For the new Cabinet surface, add `cabinet` to `.env`'s `COMPOSE_PROFILES` and set `CABINET_URL` to its public HTTPS URL, with or without a trailing slash. Configure that hostname on `bears-cabinet` with port suffix `:3000` (or `CABINET_PORT` when overridden). The platform's `SERVICE_URL_CABINET` may supply the origin fallback, but explicit origin configuration is preferable. Do not change `DEN_WEB_ORIGIN` to the Cabinet hostname: it remains the return destination for Bear administration.

The service runs `cabinet-serve`, shares `DEN_IMAGE`, `DATABASE_URL`, identity and configured S3 storage, and mounts no Bear SQLite/sandbox volumes. Migrations run first; with the profile enabled, Den waits for Cabinet readiness before activating its cross-host handoffs. Den's legacy read links redirect and old Cabinet mutations return `410 Gone`; humans sign in separately on Cabinet with their existing Den account. Review [Cabinet-only deployment and validation](../den-deploy.md#cabinet-only-composition-next-release) before rollout. Without a configured Cabinet origin, the existing local Cabinet remains available. A remotely hosted Cabinet may use the origin without enabling a local Cabinet container; coordinate that deployment's readiness separately.

## 4. Connect The Network

In the Compose resource advanced settings:

1. Open **Docker Compose**.
2. Enable **Connect To Predefined Network**.
3. Save.

This attaches the stack to Coolify's shared network while Coolify retains ownership of the resource network used by its proxy. Production keeps stable internal URLs such as `http://bears-bifrost:8080` and `http://bears-den:3001`. When multiple deployments share the predefined network, configure `BEARS_INSTANCE_SUFFIX` as described below so secondary deployments do not advertise or call production identities.

## 5. Set Environment Variables

Set these on the Compose resource:

| Variable | Value |
| -------- | ----- |
| `AGENT_RUNTIME` | `native` (default in compose) |
| `BEARS_INSTANCE_SUFFIX` | Empty/unset for production; `-test` for the test deployment |
| `JWT_SECRET` | Random secret string |
| `OPENAI_API_KEY` | Your OpenAI API key |
| `DATABASE_URL` | Den Postgres **Postgres URL (internal)** from Coolify |
| `DEN_WEB_ORIGIN` | Public Den web origin, e.g. `https://bears.[domain]`; compose derives `WEB_SERVER_URL` from this |
| `DEN_API_ORIGIN` | Public Den API origin, e.g. `https://api.bears.[domain]` or `https://bears.[domain]:3001`; compose derives `API_SERVER_URL` from this |
| `CABINET_URL` | Required when enabling profile `cabinet`: the public Cabinet HTTPS URL; optional trailing slash; shared by Den, Cabinet and migrations |
| `COMPOSE_PROFILES` | Include `cabinet` for the dedicated surface; preserve any needed `bundled`/backup profiles |
| `BEAR_SQLITE_DATA_DIR` | Leave default `/var/lib/den/bear-sqlite` unless you customize the volume mount |

For GitHub-built Den images, also set:

| Variable | Value |
| -------- | ----- |
| `DEN_IMAGE` | Required Den runtime image. Set `ghcr.io/<owner>/den:latest` in production and `ghcr.io/<owner>/den:testing` in testing. |
| `SANDBOX_PROVIDER_IMAGE` | Required when sandbox work is enabled. Set the matching `ghcr.io/<owner>/den-sandbox-provider:<lane>` image. |

Optional:

| Variable | Value |
| -------- | ----- |
| `CARGO_BUILD_JOBS` | Used only by the local `docker-compose.dev.yaml` source-build override; it does not affect Coolify's Den image pull. |
| `BIFROST_APP_PORT` | Bifrost listen port. It may remain `8080` when instance DNS names are suffixed; use distinct values if your platform also requires unique published/listener ports |
| `BIFROST_ORIGIN` | Canonical internal Bifrost origin; compose derives `BIFROST_BASE_URL`, `BIFROST_MANAGEMENT_URL`, and `LLM_API_URL` from this. Defaults to `http://bears-bifrost${BEARS_INSTANCE_SUFFIX}:${BIFROST_APP_PORT}` |
| `RUN_WEB` / `RUN_API` / `RUN_WORKERS` | Service toggles inside the Den container (compose defaults all three on) |

### Shared-network production and test deployments

Production retains the historic unsuffixed identities by leaving `BEARS_INSTANCE_SUFFIX` empty. Set this only on the secondary deployment:

```dotenv
BEARS_INSTANCE_SUFFIX=-test
```

The test deployment then uses `bears-den-test`, `bears-bifrost-test`, `bears-qdrant-test`, `bears-sandbox-provider-test`, and `bears-sandbox-engine-test`. Named volumes are explicitly pinned to `bears-stack-test_*`, preserving test data without overriding Coolify's project/network lifecycle. Docker-in-Docker includes the suffixed engine hostname in its TLS certificate.

The value must be empty or a lowercase DNS suffix beginning with `-`. Do not set production to `-prod`: keeping production empty preserves its existing DNS names and volume names.

You usually do not need to set internal service URLs. The compose file already defaults to:

| Variable | Default |
| -------- | ------- |
| `BIFROST_ORIGIN` | `http://bears-bifrost${BEARS_INSTANCE_SUFFIX}:${BIFROST_APP_PORT}` |
| `LLM_API_URL` | `${BIFROST_ORIGIN}/v1` |
| `BIFROST_BASE_URL` | `${BIFROST_ORIGIN}` |
| `BIFROST_MANAGEMENT_URL` | `${BIFROST_ORIGIN}/api` |
| `BEAR_SQLITE_DATA_DIR` | `/var/lib/den/bear-sqlite` |
| `SANDBOX_SERVER_URL` | `auto`, resolved by Den to `http://bears-sandbox-provider${BEARS_INSTANCE_SUFFIX}:${SANDBOX_PORT}` |
| `SANDBOX_CALLBACK_API_URL` | `auto`, resolved by Den to `http://bears-den${BEARS_INSTANCE_SUFFIX}:${DEN_API_PORT}` |
| `DOCKER_HOST` (provider/builder) | `tcp://bears-sandbox-engine${BEARS_INSTANCE_SUFFIX}:2376` |

## 6. Deploy

For the initial deploy, click **Deploy** in Coolify.

For ongoing deployments, use the GitHub image and webhook flow:

1. In Coolify, disable automatic deploys on Git push for this Compose resource if they are enabled.
2. Configure GHCR read credentials on the Coolify host when the packages are private.
3. Create separate Coolify resources for production and testing:
   - production: `DEN_IMAGE=ghcr.io/<owner>/den:latest` and `SANDBOX_PROVIDER_IMAGE=ghcr.io/<owner>/den-sandbox-provider:latest`;
   - testing: set `BEARS_INSTANCE_SUFFIX=-test`, `DEN_IMAGE=ghcr.io/<owner>/den:testing`, and `SANDBOX_PROVIDER_IMAGE=ghcr.io/<owner>/den-sandbox-provider:testing`.
4. Configure `COOLIFY_WEBHOOK`, `COOLIFY_TOKEN`, and `BEARS_DEPLOY_URL` as GitHub Environment secrets for `production` and `testing`.
5. Push to `test` for the automatic testing lane or `main` for the automatic production lane.

`.github/workflows/den-image.yml` publishes both Den images before `.github/workflows/coolify-deploy.yml` triggers the corresponding Coolify webhook. Compose uses `pull_policy: always` for Den-derived services, so each webhook deployment resolves the newest digest behind `testing` or `latest` instead of relying on a deploy-host build cache.

If deploy preflight fails, check the missing environment variable in the logs first. The compose file intentionally defaults required secrets and database URLs to `SETME` so bad deploys fail early. Preflight services (`bears-preflight-config`, `bears-preflight-den-db`) must complete successfully before `bears-bifrost` and `bears-den` start.

## Verification

From Coolify's terminal for a service on the same network:

| Check | Command |
| ----- | ------- |
| Bifrost | `curl http://bears-bifrost:8080/health` |
| Den web | Open the public Den URL |
| Den API | `curl ${API_SERVER_URL}/healthcheck` |
| Den readiness | `curl ${API_SERVER_URL}/health/ready` |
| Cabinet (profile `cabinet`) | Open `${CABINET_URL}` (the root redirects to `/cabinet`); internal readiness is `http://bears-cabinet:3000/health/ready` (adjust port/suffix as configured) |
| Stack status | Open `${WEB_SERVER_URL}/status` or `curl ${WEB_SERVER_URL}/status.json` |
| BearWire auth check | `curl -i ${API_SERVER_URL}/bearwire/v1/sessions/smoke-session/events?bear_slug=test-bear` should return `401` without a bearer token |

End-to-end check: create or open a bear in Den, go to its chat page, and send a message.

## Troubleshooting

- If Den cannot start, confirm `DATABASE_URL`, `JWT_SECRET`, `WEB_SERVER_URL`, `API_SERVER_URL`, and `LLM_API_URL`.
- If Cabinet is enabled but unavailable, confirm the `cabinet` profile, consistent `CABINET_URL`, matching image/migrations and Cabinet readiness. Keep its `WEB_SERVER_URL` pointed at Den; a missing/invalid origin fails Cabinet startup, and no native-memory volume is required.
- If chat does not stream, confirm `bears-den` can reach `${BIFROST_ORIGIN}/v1` and `OPENAI_API_KEY` is set for Bifrost.
- If Bifrost is unhealthy, confirm `OPENAI_API_KEY` and `services/bifrost/config.json`.
- If Bear memory does not persist across redeploys, confirm the `bears-den-sqlite-data` volume is attached and `BEAR_SQLITE_DATA_DIR` matches the mount path.

## Optional Backups

The root compose file includes an optional volume-backup sidecar behind the `volume-backup` profile:

| Service | Volume | When needed |
| ------- | ------ | ----------- |
| `bears-den-sqlite-data-backup` | `bears-den-sqlite-data` | Native runtime — per-Bear SQLite memory |

Enable only after the base stack is healthy:

```bash
COMPOSE_PROFILES=volume-backup docker compose --profile volume-backup up -d
```

Provide the `SCALEWAY_*` backup variables (and optional `DEN_SQLITE_VOLUME_BACKUP_CRON`) in `.env`. Den does not run SQLite backups in-process; the sidecar archives the mounted volume for operator restore.

## Appendix: Legacy Letta stack

The pre-native Letta/Codepool/MemFS/Redis compose profile is **not** part of the default root `docker-compose.yaml`. Operators still on that path should consult archived notes under [`docs/archive/letta/`](../../archive/letta/) and set `AGENT_RUNTIME=letta` only in a custom compose overlay — not as the primary deployment documented here.

---

Last updated: 2026-06-11
