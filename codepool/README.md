# Codepool

**BEARS harness runtime** (Letta Code SDK): warm **conversation** session pool, streaming endpoints for **Den**, optional **channel listener** hooks (see `src/channel-listeners.ts`), **`GET /internal/pool`** stats, and **`GET /metrics`** (Prometheus text, in-process counters).

- **Not** the Letta server — persistence stays at **`LETTA_BASE_URL`** (same Letta API Den uses for provisioning and history).
- **Not** under `services/` — first-class app at the repo root (alongside **`den/`**).

**Coolify / production:** Prefer the monorepo root [`docker-compose.yaml`](../docker-compose.yaml) (`bear-codepool` + `bear-letta` + `bear-den` on one network; optional **`bear-postgres`** via profile **`bundled`**). See [COOLIFY_DEPLOY.md](./COOLIFY_DEPLOY.md).

## Run locally

```bash
cd codepool
npm install
cp .env.example .env   # set LETTA_BASE_URL, LETTA_API_KEY
npm run build && npm start
```

## HTTP

| Method | Path | Notes |
|--------|------|--------|
| GET | `/health` | Liveness |
| GET | `/metrics` | Prometheus text (internal scrape; protect with network policy) |
| GET | `/internal/pool` | Conversation + channel listener stats (Bearer if `CODEPOOL_INTERNAL_TOKEN`) |
| POST | `/v1/conversations/:id/messages` | Letta-compatible streaming (body: `messages`, `agent_id`, …) |
| POST | `/v1/chat/completions` | OpenWebUI-style (`metadata.bear_agent_id`, optional `metadata.conversation_id`) |

Architecture: [docs/architecture/DEN_ARCHITECTURE.md](../docs/architecture/DEN_ARCHITECTURE.md). Deploy: [COOLIFY_DEPLOY.md](./COOLIFY_DEPLOY.md).

## Letta Code vendored patch

`@letta-ai/letta-code` is **patch-package**’d so `--no-system-info-reminder` also suppresses **agent-info** harness reminders after process restarts. When bumping `letta-code` or `letta-code-sdk`, read [docs/letta-code-patch-and-upstream.md](./docs/letta-code-patch-and-upstream.md) (upgrade checklist and how to contribute the same change upstream).
