# Multi-User Architecture: Den (Axum) + Self-Hosted Letta

*Earlier notes drew on Letta Discord discussion:* https://discord.com/channels/1161736243340640419/1467667826730078386

BEARS uses **only self-hosted Letta** (e.g. `letta/letta:latest` on Coolify). **Den** is the control plane and gateway (**Rust / Axum**). **Letta calls LiteLLM directly** for models; Den may talk to LiteLLM **only for observability** (metrics/spend/logs)—see [PLAN.md](PLAN.md).

**Phase 1 implementation:** [PHASE1_BOOTSTRAP.md](PHASE1_BOOTSTRAP.md) — Rust service in **`services/den/`**; **Twain** is a throwaway bootstrap label for milestone 0 only, not a directory in this repo.

## Overview

**v1 Den:** **Open WebUI → Den → Letta** (web auth, **bear** registry, **users↔bears** membership, policy). **LettaBot** (**Slack/WhatsApp**) typically stays **LettaBot → Letta direct** for **chat** until you adopt the optional proxy path—see [PLAN.md](PLAN.md) § *Den as LettaBot → Letta proxy*—while Den still **provisions Letta agents** and **updates LettaBot / Open WebUI** so the right **bears** appear per user. **Many‑to‑many:** each user can use many bears; some bears are shared by many users. Den enforces membership on every request.

### Den implementation (Axum)

- **Stack:** Axum + reqwest (no official Letta Rust SDK).
- **Letta base URL:** e.g. `http://bears-letta:8283` on Coolify internal network. Use **`LETTA_SERVER_PASS`** (or your Letta version’s admin auth) for server-to-server calls—never expose to browsers.
- **OpenAPI:** Generate typed clients from **your** Letta server’s spec if published (path varies by version; check [Letta docs](https://docs.letta.com)); otherwise call REST paths you verify against the running image.
- **Streaming:** Letta message streams are typically **SSE**; use `reqwest-eventsource`, `eventsource-stream`, or equivalent from Axum handlers when proxying to browsers or LettaBot.

Examples below use **Python/TypeScript** for readability; **Den** implements the same flows via reqwest.

---

## Letta concepts (self-hosted)

API shapes depend on your Letta version—confirm against your server.

### Bears, users, and conversations

- A **bear** is one **Letta agent**. **Users ↔ bears** is **many‑to‑many**: store `(user_id, bear_id)` membership in Den; optional roles (owner, member, read‑only).
- **Conversations** isolate threads (Slack thread, WhatsApp chat, Open WebUI session). Prefer **per-conversation** message APIs where available so concurrent channels do not block each other.

### Memory blocks

- **human**, **persona**, optional **shared** read-only blocks (org policy)—same ideas as Cloud; create/attach via your server’s blocks/agents API or Letta UI.

### Provisioning bears (Den-owned)

**Den** is responsible for **bear lifecycle**: create/update the Letta agent, record the bear in Den’s registry, attach **users↔bears** membership, **regenerate LettaBot config** and **Open WebUI** exposure, and (when Cabinet exists) set **Cabinet** permissions per user and bear.

**Templates / Identities** as described for Letta Cloud may not exist on self-hosted builds. Typical flow:

1. **Den** calls Letta’s API to **create or update** the Letta agent (model, system prompt, tools, memory blocks) for a new or changed **bear**.
2. Den stores **`bear_id` ↔ `associated_letta_id`** plus metadata (name, description, tool flags, default model, …).
3. Den maintains **`(user_id, bear_id)`** membership (many‑to‑many).
4. Den **publishes** bear lists: Open WebUI adapter / agent picker sources from Den; **LettaBot** `lettabot.yaml` (or generated fragment) is updated so channel allowlists reference the correct Letta agent ids for each bear.
5. When Cabinet ships: Den applies **deck/kind ACLs** per `(user_id, bear_id)` on Cabinet operations.

Admins may still use the Letta UI for experiments; **production truth** for which bears exist and who may use them should live in **Den**.

---

## System architecture

```
  Open WebUI ─────► Den ──────► Letta ───► LiteLLM ───► providers
  (v1 path)

  LettaBot ───────────────────► Letta     (v1: direct; optional later: via Den)
  (Slack/WhatsApp)
```

Den → Letta for **web** chat only. Den may call LiteLLM separately for **metrics/spend** (not inference). Optional **LettaBot → Den → Letta**: [PLAN.md](PLAN.md).

### Cabinet (Outline)

Long-lived shared knowledge: **bears** via **Den** Cabinet tools; humans in **Outline**. See [PLAN.md](PLAN.md).

---

## Den — behavioral requirements

1. **Authenticate** end users (OAuth, session, API key, etc.).
2. **Register bears** and **`(user_id, bear_id)`** membership (many‑to‑many); optional `letta_identity` metadata if you use identities.
3. **Provision bears:** create/update Letta agents via API; keep registry and clients in sync (Open WebUI, LettaBot yaml).
4. **Route** chat: resolve **bear** + conversation, call Letta message API, **stream** response back.
5. **Enforce** membership: the authenticated user may only invoke **bears** they belong to.
6. **Cabinet (later):** enforce per‑user, per‑bear permissions on Cabinet tools.
7. **Slack/WhatsApp → `user_id` mapping** applies when/if LettaBot fronts Den for chat—not required for v1 web-only Den; Den may still map channel users to `user_id` for **config** and future proxying.

### Slack & WhatsApp (optional Den proxy)

If you later route LettaBot through Den: lazy or admin-mapped provisioning, `external_identities` for `(channel, external_id) → user_id`. See [PLAN.md](PLAN.md) value-add table.

### Public API (Den)

Minimum surface (names align with [PLAN.md](PLAN.md) where noted):

| Endpoint | Method | Description |
|----------|--------|-------------|
| /auth/login | POST | Authenticate, session token |
| /auth/signup | POST | Create user + attach to default bear(s) and/or provision new bears on Letta as policy dictates |
| /chat/send | POST | User message → agent (streaming) — same role as `/chat/message` |
| /chat/message | POST | Optional alias for clients expecting this name |
| /chat/conversations | GET/POST | List / create conversations |
| /agents | GET | **Bears** visible to user (member list) |
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

Regenerate `lettabot.yaml` from Den’s DB when **bears** or **users↔bears** membership changes.

---

## Open WebUI

Point Open WebUI (or a pipe function) at **Den**, not raw Letta, when multi-user auth and routing matter. Den forwards to self-hosted Letta. Optional: OpenAI-compatible shim on Den for `/v1/chat/completions`.

---

## Deployment

| Component | Notes |
|-----------|--------|
| **Self-hosted Letta** | Coolify service; volume for `/root/.letta`; `LETTA_SERVER_PASS`; `LLM_API_URL` → LiteLLM |
| **Den** | Axum service; `LETTA_BASE_URL=http://bears-letta:8283`; Letta admin credential; `DATABASE_URL`; `SESSION_SECRET`; Outline/Cabinet credentials when Phase 3+ |
| **PostgreSQL** | Den users, **bears**, **users↔bears** membership, sessions |
| **LettaBot** | Slack + WhatsApp tokens; config volume |
| **Open WebUI** | Talks to Den in production multi-user mode |

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

## Bear design (personal vs shared)

Bears may be **personal** (one primary user, still many‑to‑many if you add delegates) or **shared** (household/project bear with many members). Prompts and memory blocks should match the sharing model.

Example **personal** bear prompt shape:

```
You are a personal assistant for {{user_name}}.
You serve this user’s interests; respect boundaries for other people they mention.
...
```

Example **shared** bear prompt shape:

```
You are a household assistant for {{group_name}}.
Multiple members may chat with you; attribute preferences per user when known.
...
```

Seed **human** / **persona** (and optional **shared**) blocks when Den provisions each bear on Letta.

---

## Summary

| Layer | Responsibility |
|-------|------------------|
| **Self-hosted Letta** | Agent state, memory blocks, conversations, tools, calls to LiteLLM |
| **Den (Axum)** | Auth; **bear** provisioning (Letta + Open WebUI + LettaBot config); **users↔bears** membership; routing; Cabinet API; Letta proxy; optional Slack/WhatsApp identity when LettaBot fronts Den |
| **LettaBot** | Slack/WhatsApp → Letta direct (v1); optional → Den later |
| **Open WebUI** | Web UI → Den (v1) |
| **PostgreSQL** | Den: users, mappings, sessions |
