# Bifrost — Coolify deployment guide

**Stack order:** This is **step 1** in [DEPLOYMENT.md](../../docs/deployment/DEPLOYMENT.md). Deploy **before** Letta.

## Overview

[Bifrost](https://github.com/maximhq/bifrost) is the BEARS **model gateway**: OpenAI-compatible `/v1` API, multi-provider routing, low overhead. **Letta** calls Bifrost directly (`LLM_API_URL`).

This repository uses **file-based (GitOps) configuration**: `services/bifrost/config.json` is mounted read-only. There is **no `config_store`** block, so the admin UI stays off and Bifrost does not persist mutable config to SQLite—see [Bifrost “Two Configuration Modes”](https://docs.getbifrost.ai/quickstart/gateway/setting-up).

## Prerequisites

- Coolify v4+ (or any Docker host)
- API keys for providers you enable in `config.json` (at minimum `OPENAI_API_KEY` for the default file in this repo)

## Deployment (official image)

1. **Add resource** → **Docker Image** (pull, not build).

2. **Image**: `maximhq/bifrost`  
   Pin a tag in production (for example `maximhq/bifrost:v1.4.9` — check [Docker Hub](https://hub.docker.com/r/maximhq/bifrost/tags) for current tags).

3. **Service name** (internal DNS): e.g. `bears-bifrost`

4. **Ports**

   - **Internal**: `8080` (Bifrost default; override with `APP_PORT` if you change it)
   - Map publicly only if you intend to expose the gateway outside the private network.

5. **Environment variables** (see [`services/bifrost/.env.example`](.env.example))

   ```bash
   APP_HOST=0.0.0.0
   APP_PORT=8080
   OPENAI_API_KEY=sk-...
   # ANTHROPIC_API_KEY=sk-ant-...   # if you add Anthropic to config.json
   LOG_LEVEL=info
   LOG_STYLE=json
   ```

6. **Mount `config.json` (required for GitOps mode)**

   Mount the repo file into Bifrost’s **app directory** (same layout as `docker run -v …/data:/app/data` in upstream docs):

   - **Source (repo):** `services/bifrost/config.json`
   - **Target (container):** `/app/data/config.json`
   - **Read only:** Yes

   To add Anthropic (or other providers), edit `config.json` in git and redeploy. See [Provider configuration](https://docs.getbifrost.ai/quickstart/gateway/provider-configuration).

7. **Health check**

   ```text
   Command: wget --no-verbose --tries=1 --spider http://127.0.0.1:8080/health || exit 1
   ```

   Adjust host/port if you change `APP_PORT`.

8. **Restart policy**: `unless-stopped`

## Verify

```bash
curl -sS http://bears-bifrost:8080/health
curl -sS http://bears-bifrost:8080/v1/models
```

Smoke test (no `Authorization` header required for the default GitOps layout; provider keys are server-side):

```bash
curl -sS http://bears-bifrost:8080/v1/chat/completions \
  -H "Content-Type: application/json" \
  -d '{"model":"openai/gpt-4o-mini","messages":[{"role":"user","content":"ping"}]}'
```

## Letta wiring

Set on the **Letta** service:

```bash
LLM_API_URL=http://bears-bifrost:8080/v1
```

Keep **`OPENAI_API_KEY`** on Letta for **embeddings** (and any Letta features that call OpenAI directly). Chat completions go to Bifrost; Bifrost uses the keys declared in `config.json`.

If you later enable Bifrost **governance / virtual keys** and require a client `Authorization` header, align Letta’s forwarded credential with Bifrost’s docs— the stock BEARS layout assumes **no** gateway master key.

## Observability

- **HTTP:** `GET /health`
- **Prometheus:** scrape metrics from Bifrost if enabled in your version (see [Bifrost observability](https://docs.getbifrost.ai/features/observability/default)).
- **Den:** optional read-only checks against `BIFROST_BASE_URL` (see [PLAN.md](../../docs/planning/PLAN.md)); not implemented in Den by default.

## Troubleshooting

- **Empty model list:** confirm `config.json` is mounted at `/app/data/config.json` and env vars referenced via `env.*` exist on the container.
- **401 from provider:** invalid or missing provider API key in the environment.
- **Letta cannot reach gateway:** same Docker network, correct hostname (`bears-bifrost`), port `8080` unless overridden.

## Next step

Deploy **Letta**: [`../letta/COOLIFY_DEPLOY.md`](../letta/COOLIFY_DEPLOY.md).
