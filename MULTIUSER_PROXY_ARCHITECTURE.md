# Multi-User Architecture: Den (Axum) + Self-Hosted Letta

*Earlier notes drew on Letta Discord discussion:* https://discord.com/channels/1161736243340640419/1467667826730078386

BEARS uses **only self-hosted Letta** (e.g. `letta/letta:latest` on Coolify). **Den** is the control plane and gateway (**Rust / Axum**). **Letta calls LiteLLM directly** for models; Den may talk to LiteLLM **only for observability** (metrics/spend/logs)—see [PLAN.md](PLAN.md).

## Overview

Multi-user pattern: **one Letta agent per user** (or your chosen mapping), with **Den** between clients and Letta. Den authenticates users, maps channels (OpenWebUI, Slack, WhatsApp) to internal `user_id`, routes to the correct `agent_id`, and forwards to **self-hosted Letta**. End users use **OpenWebUI** (web) or **LettaBot** (**Slack**, **WhatsApp**; Telegram/Discord optional).

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
+------------------+     +------------------+
|    OpenWebUI      |     |     LettaBot      |
|  (chat frontend)  |     |(Slack, WhatsApp) |
+--------+---------+     +--------+---------+
         |                         |
         v                         v
+------------------------------------------------+
|  Den (Rust / Axum) — BEARS control plane       |
|  Auth, user↔agent map, policy, Cabinet API     |
|  Proxies chat → Letta only                     |
+------------------------+-----------------------+
                         |
                         v
+------------------+
| Self-hosted Letta|
| :8283            |
+--------+---------+
         |  LLM traffic (direct)
         v
+------------------+
| LiteLLM          |──► model providers
+------------------+

(Den → Letta for chat only. Den may call LiteLLM separately for metrics/spend — not inference.)
```

### Cabinet (Outline)

Long-lived shared knowledge: agents via **Den** Cabinet tools; humans in **Outline**. See [PLAN.md](PLAN.md).

---

## Den — behavioral requirements

1. **Authenticate** end users (OAuth, session, API key, etc.).
2. **Store** `user_id` → `agent_id` (and optional `letta_identity` metadata if you use identities).
3. **Route** chat: resolve agent + conversation, call Letta message API, **stream** response back.
4. **Enforce** ownership: only the authenticated user’s agent may be used.
5. **Provision** agents on signup (HTTP to self-hosted Letta) and map **Slack / WhatsApp** external ids → `user_id` (see [PLAN.md](PLAN.md) § provisioning).

### Slack & WhatsApp

Same as [PLAN.md](PLAN.md): lazy or admin-mapped provisioning; LettaBot → **Den** → Letta in production; `external_identities` table for `(channel, external_id) → user_id`.

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

**Production:** LettaBot → **Den** (`/chat/send` or dedicated internal route) with channel metadata → Den → self-hosted Letta.

**Experiments:** LettaBot → Letta directly (bypass Den).

```yaml
server:
  letta:
    apiKey: ${LETTA_SERVER_PASS}   # admin password or token your Letta expects
    baseUrl: http://bears-letta:8283  # internal URL when talking to Letta
    # When calling Den instead: baseUrl: http://bears-den:8080 (example)

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
| **Den (Axum)** | Auth, routing, policy, Cabinet API, Slack/WhatsApp identity, Letta proxy |
| **LettaBot** | Slack/WhatsApp adapter → Den |
| **OpenWebUI** | Web UI → Den |
| **PostgreSQL** | Den: users, mappings, sessions |
