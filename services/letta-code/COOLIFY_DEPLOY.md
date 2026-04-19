# Letta Code — Coolify deployment guide

**Stack order:** Deploy **after** [Letta](../letta/COOLIFY_DEPLOY.md) (persistence + Letta HTTP API); **before** [Den](../../den/COOLIFY_DEPLOY.md). See [DEPLOYMENT.md](../../docs/deployment/DEPLOYMENT.md).

## Why this service exists (BEARS contract)

BEARS **always** routes end-user web chat through the **Letta Code harness**, not only through the Letta server container:

```text
Den (Deep Chat) ──► Letta Code (`letta server`) ──► Letta HTTP API ──► Bifrost ──► models
```

- **[`services/letta`](../letta/COOLIFY_DEPLOY.md)** — `letta/letta` holds agent state and exposes the **Letta HTTP API** (`/v1/agents`, `/v1/health`, …). Den uses **`LETTA_API_BASE_URL`** for provisioning and operator flows.
- **`services/letta-code` (this doc)** — always-on **`letta server`** from [`@letta-ai/letta-code`](https://www.npmjs.com/package/@letta-ai/letta-code). This is **mandatory** for the chat pipeline: Den sets **`LETTA_CODE_BASE_URL`** to the harness HTTP base URL that implements conversation list, history, and streaming (`/v1/conversations/…`). Den does **not** fall back to **`LETTA_API_BASE_URL`** for chat.

Treating “`LETTA_CODE_BASE_URL` = same host as Letta” as a **shortcut** is **not** the BEARS production topology: you still operate a **separate** `bears-letta-code` deployment so skills, channels, scheduling, and harness upgrades are owned by the Letta Code process boundary.

## Upstream references

| Resource | Role |
|----------|------|
| [letta-ai/letta-code-server-deployment](https://github.com/letta-ai/letta-code-server-deployment) | Official container pattern: install CLI, run `letta server`, persist `/root` |
| [Letta Code — Remote environments](https://docs.letta.com/letta-code/remote) | Auth (OAuth device flow vs `LETTA_API_KEY`), outbound WebSocket when using Letta Cloud features |
| [Letta Code — CLI](https://docs.letta.com/letta-code/cli-reference/index.md) | `letta server`, optional `--channels slack`, `--debug` |

Self-hosted Letta: set **`LETTA_BASE_URL`** to your Letta API origin (see below). The upstream quickstart’s “no inbound ports” story targets **chat.letta.com** remote listeners; **BEARS** still runs `letta server` in-cluster and wires **Den → harness** on the private Docker network.

## Prerequisites

- **Letta** reachable at an internal URL (e.g. `http://bears-letta:8283`) with auth configured (`LETTA_SERVER_PASS` or your image’s equivalent).
- Coolify (or equivalent) with a **shared network** so **Den** can HTTP(S) to **`bears-letta-code`** at **`LETTA_CODE_BASE_URL`**.

## Deployment options

### Option A — Docker Compose from this repo (recommended)

This is the **least work** for Coolify + GitOps: same pattern as [`services/bifrost`](../bifrost/COOLIFY_DEPLOY.md) — one compose file declares the build, service name, volume, and restart policy; you mostly set secrets in the UI.

1. **Add New Resource** → **Docker Compose** (build pack) → connect this repository.
2. **Base Directory:** `services/letta-code` · **Compose file:** [`docker-compose.yaml`](docker-compose.yaml).
3. Enable **Preserve Repository During Deployment** (or your platform equivalent) so the compose file stays tied to the branch you deploy.
4. In **Environment Variables**, set **`LETTA_API_KEY`** (required) and adjust **`LETTA_BASE_URL`** if your Letta service name differs. The compose file lists `LETTA_BASE_URL`, `LETTA_API_KEY`, `ENV_NAME`, and `LETTA_DEBUG` under `environment` so Coolify can **prefill / show** those keys; optional vars have defaults in compose (see [`docker-compose.yaml`](docker-compose.yaml)). Prefer Coolify secrets for `LETTA_API_KEY` — do not commit real values.
5. Ensure the **`letta-code-data`** volume (or your override) mounts **`/root`** — already declared in compose; add persistent storage in the UI if your platform requires an explicit volume binding.

For **local** `docker compose`, put the same keys in a **`.env`** file beside the compose file (Compose uses it for `${…}` substitution); see [`.env.example`](.env.example).

### Environment (both options)

| Variable | Required | Description |
|----------|----------|-------------|
| `LETTA_BASE_URL` | Yes | Letta HTTP API base, no trailing slash — e.g. `http://bears-letta:8283` |
| `LETTA_API_KEY` | Yes* | Bearer token the Letta server accepts (typically the same secret as **`LETTA_SERVER_PASS`** on Letta). *Required for unattended/self-hosted; avoid interactive OAuth in CI. |
| `ENV_NAME` | No | Display name for this environment (default in image: `cloud`; set e.g. `bears-prod`). |
| `LETTA_DEBUG` | No | Set to `1` for verbose logs while debugging. |

### Option B — Dockerfile only (no Compose)

Use when you **do not** want compose — e.g. pre-building an image in CI, pushing to a registry, and deploying **Docker Image** in Coolify with env + volume configured by hand. Build context: this directory; image runs **`letta server`** per [`Dockerfile`](Dockerfile).

**Service name:** `bears-letta-code` (match Den’s `LETTA_CODE_BASE_URL` host). **Volume:** mount **`/root`**. **Networking:** internal only unless debugging; Den must reach **`http://bears-letta-code:<port>`** — see **Integration contract**.

Optional **`letta server` flags** (override `CMD` or wrap entrypoint if needed):

- `--env-name "<name>"` — align with `ENV_NAME`.
- `--channels slack` (or `telegram`, …) — [Channels](https://docs.letta.com/letta-code/channels/).
- `--debug` — verbose logs in headless mode (the repo `Dockerfile` uses this).

## Integration contract (Den `LETTA_CODE_BASE_URL`)

Den’s Rust client expects an HTTP API at **`LETTA_CODE_BASE_URL`** (no path suffix), including at least:

- `GET /v1/conversations/` (with `agent_id`, ordering query params)
- `GET /v1/conversations/{id}/messages`
- `POST /v1/conversations/{id}/messages` (SSE streaming)
- `PATCH /v1/conversations/{id}` (thread title / summary)

Authoritative list and shapes: [`den/src/core/letta/client.rs`](../../den/src/core/letta/client.rs).

**Pin** `@letta-ai/letta-code` (and the Letta server image) to versions you have verified against that contract. After deploy, smoke-test from a shell on the Den network:

```bash
# Replace host, port, agent id, and token with your values.
curl -sS -H "Authorization: Bearer $LETTA_SERVER_PASS" \
  "http://bears-letta-code:<port>/v1/conversations/?agent_id=<agent-id>&limit=5&order_by=last_message_at&order=desc"
```

The **listen address and port** for that HTTP surface are defined by the Letta Code release you run—**do not assume** it shares port `8283` with the Letta container. Configure Coolify published ports / internal DNS so **`LETTA_CODE_BASE_URL`** matches exactly.

## Den environment (consumer)

| Variable | Value |
|----------|--------|
| `LETTA_API_BASE_URL` | `http://bears-letta:8283` (example) — provisioning / agents |
| `LETTA_CODE_BASE_URL` | `http://bears-letta-code:<harness-http-port>` — **only** chat/conversation traffic |
| `LETTA_API_KEY` | Bearer for Letta API |
| `LETTA_CODE_API_KEY` | Optional; defaults to `LETTA_API_KEY` if unset |

See [`den/.env.example`](../../den/.env.example) and [`den/COOLIFY_DEPLOY.md`](../../den/COOLIFY_DEPLOY.md).

## Troubleshooting

| Symptom | What to check |
|---------|----------------|
| Den: “`LETTA_CODE_BASE_URL` not set” | Set harness URL on Den; chat never uses `LETTA_API_BASE_URL`. |
| 401/403 from harness | Align **`LETTA_API_KEY`** on the Letta Code container with Letta’s **`LETTA_SERVER_PASS`** (or your auth scheme). |
| Harness cannot reach Letta | **`LETTA_BASE_URL`** must use the **internal** Docker hostname of the Letta service; no `localhost` inside the container unless using host networking. |
| OAuth URL in logs on startup | Prefer **`LETTA_API_KEY`** for headless/self-hosted; persist **`/root`** if you use OAuth. |
| Chat works but Slack/Telegram does not | Add `--channels …`, configure bots per [Channels](https://docs.letta.com/letta-code/channels/) docs. |

## Security

- Do not expose **`LETTA_CODE_BASE_URL`** to the public internet without TLS and policy; Den should call it on a **private** network.
- Treat **`LETTA_API_KEY`** / Letta admin pass as **server-to-server** secrets (same trust domain as Den → Letta).

## Deployment completion checklist

- [ ] Letta healthy (`GET /v1/health` on `LETTA_API_BASE_URL`).
- [ ] `bears-letta-code` running `letta server`, **`LETTA_BASE_URL`** points at Letta.
- [ ] Volume **`/root`** attached for persistence.
- [ ] From Den’s network, conversation API smoke test succeeds (see above).
- [ ] Den **`LETTA_CODE_BASE_URL`** matches harness HTTP base; web chat sends a message end-to-end.

**Services in this slice:** `bears-bifrost`, `bears-letta`, **`bears-letta-code`**, then **Den**.
