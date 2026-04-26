# Bifrost — Coolify deployment guide

**Stack order:** This is **step 1** in [DEPLOYMENT.md](../../docs/deployment/DEPLOYMENT.md). Deploy **before** Letta.

## Overview

[Bifrost](https://github.com/maximhq/bifrost) is the BEARS **model gateway**: OpenAI-compatible `/v1` API, multi-provider routing. **Letta** calls Bifrost using `LLM_API_URL` (see `[../letta/COOLIFY_DEPLOY.md](../letta/COOLIFY_DEPLOY.md)`).

This repository uses **file-based (GitOps) configuration**: `services/bifrost/config.json` is mounted read-only into the container. There is **no `config_store`** block in that file, so Bifrost’s **built-in admin UI stays off** and the process does not rely on SQLite for gateway config—see [Bifrost “Two Configuration Modes”](https://docs.getbifrost.ai/quickstart/gateway/setting-up).

## Prerequisites

- Coolify v4+
- Provider API keys for every `env.*` reference in `services/bifrost/config.json` (the default file requires `**OPENAI_API_KEY`**)
- A **GitHub** (or other Git) remote for this repo—only required for the **Git + Docker Compose** path below

---

## Option A (recommended): `config.json` from Git — Docker Compose build pack

Coolify can **clone your repository on each deploy** and bind-mount files from that checkout into the container. Then `config.json` is always whatever is on the branch you deploy—no pasting JSON into the Coolify UI.

Official pattern: [Docker Compose build pack](https://coolify.io/docs/builds/packs/docker-compose) with a compose file in this repo (`[docker-compose.yaml](docker-compose.yaml)`).

### 1. Create the resource

1. Open your Coolify **project** → **Add New Resource**.
2. Choose **Public Repository** or **Private Repository** (GitHub App / deploy key) and select **this** monorepo.
3. When asked for the **build pack**, pick **Docker Compose** (not Nixpacks, not “Docker Image” alone).

### 2. Point Coolify at the compose file

In the Docker Compose build-pack settings:


| Field                       | Value                                |
| --------------------------- | ------------------------------------ |
| **Branch**                  | e.g. `main` (or your release branch) |
| **Base Directory**          | `services/bifrost`                   |
| **Docker Compose Location** | `docker-compose.yaml`                |


Coolify combines **Base Directory** + **Docker Compose Location** to find the file.

### 3. Preserve the Git checkout (required for the bind mount)

Enable **Preserve Repository During Deployment** (wording may vary slightly by Coolify version).

Without this, bind mounts like `./config.json:/app/data/config.json` often fail because the compose working tree is not kept on the server. This is the usual fix when mounting **files from the repo** into containers on Coolify; see community notes in the [Coolify Docker Compose + Git guide](https://dev.to/mandrasch/simple-coolify-example-with-docker-compose-github-deployments-53m).

### 4. Pin the image in Git (recommended)

Edit `[docker-compose.yaml](docker-compose.yaml)` and replace `maximhq/bifrost:latest` with a **pinned tag** (e.g. `maximhq/bifrost:v1.4.9`). Check [Docker Hub tags](https://hub.docker.com/r/maximhq/bifrost/tags). Redeploy after edits.

### 5. Environment variables (still in Coolify — secrets only)

`config.json` references keys via `env.*` (for example `env.OPENAI_API_KEY`). **Secrets stay in Coolify**, not in Git:

1. Open the resource → **Environment Variables** (or **Production Variables**).
2. Add at least `**OPENAI_API_KEY`** (and any other provider keys your `config.json` references).
3. Optional: override defaults with `**APP_PORT**`, `**LOG_LEVEL**`, `**LOG_STYLE**` if you do not want the values from `docker-compose.yaml`.

### 6. Ports, health check, restart

- **Ports:** `docker-compose.yaml` maps **`8080:8080`**. Adjust in the compose file if Coolify should publish a different host port, or remove the `ports:` section if you only need internal access and will reach the service by stack DNS (advanced—see Coolify networking docs).
- **Health check:** `GET /health` — Bifrost may **503** if internal store pings fail. The repo’s **[`config.json`](config.json)** sets **`disable_db_pings_in_health`** so file-based GitOps stacks stay **200** without SQLite/log/vector probes. Compose uses **`CMD-SHELL`** + **`wget`** (same as the upstream image) with a **long `start_period`** for slow ARM hosts.
- **Restart:** `restart: unless-stopped` is already set on the service.

### 7. Deploy

**Deploy** / **Redeploy**. On success, other services on the **same Coolify network** should resolve `**http://bears-bifrost:8080`** (the **service name** in `[docker-compose.yaml](docker-compose.yaml)`).

### 8. Connecting Letta across stacks (if needed)

If Letta is a **separate** Coolify resource, it must share a **Docker network** with Bifrost (Coolify “**Connect to Predefined Network**” / shared network—see [Coolify compose networking](https://coolify.io/docs/knowledge-base/docker/compose)). If both services live in the **same** compose stack, they already share a network.

---

## Option B: Public image only — manual `config.json` in Coolify

Use this when you **cannot** use the Git-backed compose flow (for example, no Git integration on that host). You pull `**maximhq/bifrost`** as a plain **Docker Image** resource and supply `config.json` via Coolify **Storages** (bind mount, pasted file, or host path)—as in the previous revision of this guide.

### 1. Create the service

1. **Add New Resource** → **Docker Image**.
2. **Save** to open the resource editor.

### 2. General / image


| Coolify field | Suggested value                                                                                                   |
| ------------- | ----------------------------------------------------------------------------------------------------------------- |
| **Name**      | `bears-bifrost`                                                                                                  |
| **Image**     | `maximhq/bifrost` — **pin a tag** in production; see [Docker Hub](https://hub.docker.com/r/maximhq/bifrost/tags). |


### 3. Ports

Expose **8080** internally (and publicly only if required). Match `**APP_PORT`** in environment.

### 4. Environment variables

Same secret set as **Option A §5** (`OPENAI_API_KEY`, …). See `[.env.example](.env.example)`.

### 5. Mount `config.json`

Under **Storages** / **Volumes**, mount **`config.json`** at **`/app/data/config.json`** (read-only): bind host path, Coolify file storage, or pasted content—**not** loaded automatically from Git in this mode.

### 6. Health check

The upstream Bifrost image includes BusyBox **`wget`** (not `curl`). Use the same probe as in [`docker-compose.yaml`](docker-compose.yaml):

```bash
wget --no-verbose --tries=1 -O /dev/null http://127.0.0.1:8080/health || exit 1
```

Use **`-O /dev/null`**, not GNU **`--spider`** — BusyBox `wget` in the image often does not support `--spider`, so the health check would fail every time.

- **Interval:** `30s` · **Timeout:** `10s` · **Retries:** `5` · **Start period:** `60s`

### 7. Restart policy

**`unless-stopped`**, then **Deploy**.

---

## Option C: Bake `config.json` into a tiny image (Git build, no bind mount)

If your Coolify plan prefers **one Dockerfile per service** instead of compose:

1. Add a `Dockerfile` next to `config.json` with:
  ```dockerfile
   FROM maximhq/bifrost:v1.4.9
   COPY config.json /app/data/config.json
  ```
2. In Coolify, create a **Dockerfile** deployment from Git: **Base Directory** `services/bifrost`, Dockerfile path `Dockerfile`.
3. Set provider secrets only in Coolify **Environment Variables**.

Every config change requires an **image rebuild**—good for immutability, less convenient for rapid `config.json` iteration than Option A.

---

## Verify (after deploy)

From a container on the **same Docker network** as `bears-bifrost` (or Coolify **Terminal** on that service):

```bash
curl -sS http://bears-bifrost:8080/health
curl -sS http://bears-bifrost:8080/v1/models
```

Optional smoke test:

```bash
curl -sS http://bears-bifrost:8080/v1/chat/completions \
  -H "Content-Type: application/json" \
  -d '{"model":"openai/gpt-4o-mini","messages":[{"role":"user","content":"ping"}]}'
```

## Letta (next service)

Set `**LLM_API_URL=http://bears-bifrost:8080/v1**` on Letta (adjust host if you renamed the compose service). Keep `**OPENAI_API_KEY**` on Letta for **embeddings**. Details: `[../letta/COOLIFY_DEPLOY.md](../letta/COOLIFY_DEPLOY.md)`.

## Observability

- **Liveness:** `GET /health`
- **Prometheus / OTLP:** [Bifrost observability](https://docs.getbifrost.ai/features/observability/default)
- **Den:** optional `**BIFROST_BASE_URL`** — [PLAN.md](../../docs/planning/PLAN.md)

## Troubleshooting


| Symptom                                                   | What to check                                                                                                                                |
| --------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------- |
| Compose deploy: “cannot mount … config.json” / empty file | **Preserve Repository During Deployment** enabled; **Base Directory** is `services/bifrost` so `./config.json` exists in the compose context (or use repo root + root `[docker-compose.yaml](../../docker-compose.yaml)` so `./services/bifrost/config.json` exists) |
| Logs: `read /app/data/config.json: is a directory` | Host path for the bind mount was missing, so Docker created a **directory** named `config.json`. Remove that bad path on the server, ensure the real file exists in the checkout, and match **Base Directory** to the compose path (see above). Repo compose sets **`create_host_path: false`** so this fails fast instead of creating a directory. |
| Container exits on start                                  | **Logs** — invalid JSON, missing `env.`* variables in Coolify                                                                                |
| **`bears-bifrost` unhealthy** (health check never passes) | **`GET /health`** returns **503** when config/log/vector store pings fail — set **`disable_db_pings_in_health`** in `client` (see repo `config.json`); ensure **`OPENAI_API_KEY`** is set if providers use `env.*`; allow a long **`start_period`** on ARM |
| Letta cannot resolve `bears-bifrost`                     | Same **Docker network** (Option A §8), or use Coolify-generated hostname for the stack                                                       |


## Reference

- Compose stack for Coolify: `[docker-compose.yaml](docker-compose.yaml)`
- Example env keys: `[.env.example](.env.example)`
- Bifrost provider config: [Provider configuration](https://docs.getbifrost.ai/quickstart/gateway/provider-configuration)
