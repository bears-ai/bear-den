# Multi-User Architecture: Den (Axum) + Self-Hosted Letta

*Earlier notes drew on Letta Discord discussion:* https://discord.com/channels/1161736243340640419/1467667826730078386

BEARS uses **only self-hosted Letta** (e.g. `letta/letta:latest` on Coolify). **Den** is the control plane and gateway (**Rust / Axum**). **Letta calls LiteLLM directly** for models; Den may talk to LiteLLM **only for observability** (metrics/spend/logs)—see [PLAN.md](PLAN.md).

## Overview

**v1 Den:** **OpenWebUI → Den → Letta** (web auth, agent registry, policy). **LettaBot** (**Slack/WhatsApp**) typically stays **LettaBot → Letta direct** until you adopt the optional proxy path—see [PLAN.md](PLAN.md) § *Den as LettaBot → Letta proxy*. Multi-user pattern for web: one agent per user (or your mapping), `user_id` → `agent_id` in Den’s DB.

### Den implementation (Axum)

- **Stack:** Axum + reqwest (no official Letta Rust SDK).
- **Letta base URL:** e.g. `http://bears-letta:8283` on Coolify internal network. Use **`LETTA_SERVER_PASS`** (or your Letta version’s admin auth) for server-to-server calls—never expose to browsers.
- **OpenAPI:** Generate typed clients from **your** Letta server’s spec if published (path varies by version; check [Letta docs](https://docs.letta.com)); otherwise call REST paths you verify against the running image.
- **Streaming:** Letta message streams are typically **SSE**; use `reqwest-eventsource`, `eventsource-stream`, or equivalent from Axum handlers when proxying to browsers or LettaBot.

Examples below use **Python/TypeScript** for readability; **Den** implements the same flows via reqwest.

---

## Letta concepts (self-hosted)

API shapes depend on your Letta version—confirm against your server.

### Agents & conversations

- Each **user** maps to a **Letta agent** (or shared agent + conversation isolation, per product choice).
- **Conversations** isolate threads (Slack thread, WhatsApp chat, OpenWebUI session). Prefer **per-conversation** message APIs where available so concurrent channels do not block each other.

### Memory blocks

- **human**, **persona**, optional **shared** read-only blocks (org policy)—same ideas as Cloud; create/attach via your server’s blocks/agents API or Letta UI.

### Provisioning (self-hosted)

**Templates / Identities** as described for Letta Cloud may not exist on self-hosted builds. Typical approach:

1. **Create agent** via Letta API or admin UI (model, system prompt, tools, memory blocks).
2. Store `user_id` → `agent_id` in **Den’s database**.
3. Optionally script agent creation from Den on signup (HTTP `POST` to Letta’s agent endpoint with shared headers).

---

## System architecture

```
  OpenWebUI ──────► Den ──────► Letta ───► LiteLLM ───► providers
  (v1 path)

  LettaBot ───────────────────► Letta     (v1: direct; optional later: via Den)
  (Slack/WhatsApp)
```

Den → Letta for **web** chat only. Den may call LiteLLM separately for **metrics/spend** (not inference). Optional **LettaBot → Den → Letta**: [PLAN.md](PLAN.md).

### Cabinet (Outline)

Long-lived shared knowledge: agents via **Den** Cabinet tools; humans in **Outline**. See [PLAN.md](PLAN.md).

---

## Den — behavioral requirements

1. **Authenticate** end users (OAuth, session, API key, etc.).
2. **Store** `user_id` → `agent_id` (and optional `letta_identity` metadata if you use identities).
3. **Route** chat: resolve agent + conversation, call Letta message API, **stream** response back.
4. **Enforce** ownership: only the authenticated user’s agent may be used.
5. **Provision** agents on signup (HTTP to self-hosted Letta). **Slack/WhatsApp → `user_id` mapping** applies when/if LettaBot fronts Den—not required for v1 web-only Den.

### Slack & WhatsApp (optional Den proxy)

If you later route LettaBot through Den: lazy or admin-mapped provisioning, `external_identities` for `(channel, external_id) → user_id`. See [PLAN.md](PLAN.md) value-add table.

### Public API (Den)

Minimum surface (names align with [PLAN.md](PLAN.md) where noted):

| Endpoint | Method | Description |
|----------|--------|-------------|
| /auth/login | POST | Authenticate, session token |
| /auth/signup | POST | Create user + provision agent on Letta |
| /chat/send | POST | User message → agent (streaming) — same role as `/chat/message` |
| /chat/message | POST | Optional alias for clients expecting this name |
| /chat/conversations | GET/POST | List / create conversations |
| /agents | GET | Agents visible to user |
| /admin/* | … | User/agent admin (optional) |

Cabinet tool endpoints are internal or agent-facing per PLAN.

---

## LettaBot (Slack & WhatsApp)

**v1 / default:** LettaBot → **Letta** directly (`baseUrl: http://bears-letta:8283`). **Not** a Den release requirement.

**Optional later:** LettaBot → **Den** → Letta (unified identity/policy)—see [PLAN.md](PLAN.md).

```yaml
server:
  letta:
    apiKey: ${LETTA_SERVER_PASS}
    baseUrl: http://bears-letta:8283   # direct to Letta (v1)
    # Future optional: baseUrl pointing at Den if fronting LettaBot

agents:
  - name: user-alice
    agentId: <letta-agent-id>
    channels:
      slack:
        allowedUsers: ["U_ALICE_SLACK_ID"]
      whatsapp:
        allowedUsers: ["+15551234567"]
```

Regenerate `lettabot.yaml` from Den’s DB when users are added.

---

## OpenWebUI

Point OpenWebUI (or a pipe function) at **Den**, not raw Letta, when multi-user auth and routing matter. Den forwards to self-hosted Letta. Optional: OpenAI-compatible shim on Den for `/v1/chat/completions`.

---

## Deployment

| Component | Notes |
|-----------|--------|
| **Self-hosted Letta** | Coolify service; volume for `/root/.letta`; `LETTA_SERVER_PASS`; `LLM_API_URL` → LiteLLM |
| **Den** | Axum service; `LETTA_BASE_URL=http://bears-letta:8283`; Letta admin credential; `DATABASE_URL`; `SESSION_SECRET`; Outline/Cabinet credentials when Phase 3+ |
| **PostgreSQL** | Den user/agent/session data |
| **LettaBot** | Slack + WhatsApp tokens; config volume |
| **OpenWebUI** | Talks to Den in production multi-user mode |

```bash
# Den (example)
LETTA_BASE_URL=http://bears-letta:8283
LETTA_AUTH=<LETTA_SERVER_PASS or Bearer token per your Letta version>
DATABASE_URL=postgresql://...
SESSION_SECRET=...

# LettaBot
LETTABOT_CONFIG=/path/to/lettabot.yaml
SLACK_BOT_TOKEN=...
WHATSAPP_ACCESS_TOKEN=...
```

### Self-hosted Letta checklist

- Deploy Letta + LiteLLM per [DEPLOYMENT.md](DEPLOYMENT.md)
- Create a **baseline agent** (or template script) for per-user clones
- Harden **Letta admin** credential; reachable only from Den / internal network
- Attach **Cabinet** tools when Den exposes them ([PLAN.md](PLAN.md))

### Security

- Letta admin access is **full**; keep it on the internal network and only on **Den** (server-side).
- Den validates every request before calling Letta.
- Rate-limit Den at the edge.

---

## Agent design (one user per agent)

Example system prompt shape:

```
You are a personal assistant for {{user_name}}.
You serve exactly one user. Do not share information across users.
...
```

Seed **human** / **persona** blocks when creating each user’s agent via API or UI.

---

## Summary

| Layer | Responsibility |
|-------|------------------|
| **Self-hosted Letta** | Agent state, memory blocks, conversations, tools, calls to LiteLLM |
| **Den (Axum)** | Auth, routing, policy, Cabinet API, Letta proxy; optional Slack/WhatsApp identity when LettaBot fronts Den |
| **LettaBot** | Slack/WhatsApp → Letta direct (v1); optional → Den later |
| **Open WebUI** | Web UI → Den (v1) |
| **PostgreSQL** | Den: users, mappings, sessions |
