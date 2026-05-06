# Codepool

**BEARS harness runtime** (Letta Code SDK): warm **conversation** session pool, streaming endpoints for **Den**, optional **channel listener** hooks (see `src/channel-listeners.ts`), **`GET /internal/pool`** stats, and **`GET /metrics`** (Prometheus text, in-process counters).

- **Not** the Letta server — canonical git memfs is on the Letta volume, with Letta’s **`LETTA_MEMFS_SERVICE_URL`** pointing at **MemFS Manager** / **`bears-memfs-manager`**; this process uses **`LETTA_MEMFS_LOCAL=1`** and **`~/.letta`** in the container for the Letta Code CLI mirror.
- First-class app service under **`services/codepool/`** (alongside **`services/den/`**).

**Coolify / production:** Prefer the monorepo root [`docker-compose.yaml`](../../docker-compose.yaml) (`bears-codepool` + `bears-letta` + `bears-den` on one network; optional **`bears-postgres`** via profile **`bundled`**). See [COOLIFY_DEPLOY.md](./COOLIFY_DEPLOY.md).

## Run locally

```bash
cd services/codepool
npm install
cp .env.example .env   # set LETTA_BASE_URL, LETTA_API_KEY
npm run build && npm start
```

## Logging

Codepool logs in a Den-aligned human-readable format by default:

```/dev/null/codepool-log.txt#L1-1
2026-04-30T12:34:56.789Z  INFO bears-codepool bear_channel_message_end: bear channel message finished request_id=... outcome=ok duration_ms=1234
```

Set `LOG_STYLE=json` if you need the structured JSON stream for log aggregation. Set `LOG_LEVEL=debug|info|warn|error` to control verbosity. In the root compose stack these are exposed as `CODEPOOL_LOG_STYLE` and `CODEPOOL_LOG_LEVEL`.

## HTTP

| Method | Path | Notes |
|--------|------|--------|
| GET | `/health` | Liveness |
| GET | `/metrics` | Prometheus text (internal scrape; protect with network policy) |
| GET | `/internal/pool` | Conversation + channel listener stats (Bearer if `CODEPOOL_INTERNAL_TOKEN`) |
| POST | `/internal/bear_channel/sessions/:id/messages` | Canonical Den -> Codepool `bear_channel` streaming runtime endpoint (Bearer if `CODEPOOL_INTERNAL_TOKEN`) |
| POST | `/internal/bear_channel/sessions/:id/cancel` | Reserved cancellation endpoint; currently returns `501` until warm-pool cancellation is implemented |
| POST | `/v1/conversations/:id/messages` | Legacy Letta-compatible streaming (body: `messages`, `role_agent_id`, `bear_id`, …); no bear-level `letta_agent_id` fallback |
| POST | `/v1/chat/completions` | OpenWebUI-style (`metadata.role_agent_id`, optional `metadata.conversation_id`) |

Architecture: [docs/architecture/DEN_ARCHITECTURE.md](../../docs/architecture/DEN_ARCHITECTURE.md) and [`bear_channel` + ACP](../../docs/architecture/BEAR_CHANNEL_AND_ACP.md). Deploy: [COOLIFY_DEPLOY.md](./COOLIFY_DEPLOY.md).

## Letta Code vendored patch

`@letta-ai/letta-code` is **patch-package**’d so `--no-system-info-reminder` also suppresses **agent-info** harness reminders after process restarts. When bumping `letta-code` or `letta-code-sdk`, read [docs/letta-code-patch-and-upstream.md](./docs/letta-code-patch-and-upstream.md) (upgrade checklist and how to contribute the same change upstream).
