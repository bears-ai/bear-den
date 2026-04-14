# Multi-User Architecture: Den (Axum) + Self-Hosted Letta

*Earlier notes drew on Letta Discord discussion:* https://discord.com/channels/1161736243340640419/1467667826730078386

BEARS uses **only self-hosted Letta** (e.g. `letta/letta:latest` on Coolify). **Den** is the control plane and gateway (**Rust / Axum**). **Letta calls Bifrost directly** for models; Den may talk to Bifrost **only for observability** (metrics/health/logs)—see [PLAN.md](../planning/PLAN.md).

**LettaBot** is the **mandatory agent runtime** for all conversational and tool-driven interaction: every path that talks to a bear goes **through LettaBot**, which in turn uses **Letta** as its **state and persistence backend** (agents, memory blocks, conversations, tools). Den does **not** call Letta’s message APIs directly for end-user chat; it forwards to LettaBot so browser, messaging channels, and future clients share one stack (including [Letta Code skills](https://docs.letta.com/letta-code/skills/) behavior where applicable).

**Phase 1 implementation:** [PHASE1_BOOTSTRAP.md](../planning/PHASE1_BOOTSTRAP.md) — Rust service in repo-root **`den/`**; **Trestle** is a throwaway bootstrap label for milestone 0 only, not a directory in this repo.

## Overview

**v1 Den:** **Operator console** (browser, priority) provisions **users**, **bears** (Letta agents), **membership**, and surfaces **LettaBot** yaml; **end-user chat** is **Web → Den → LettaBot → Letta** via **Den-hosted Loquix** on a path such as `/app` or `/chat` (**primary** path — same Axum routes: auth, bear list, **SSE streaming** `POST /v1/chat/send` with Den proxying or bridging to LettaBot). **Open WebUI → Den → LettaBot → Letta** is **optional** when you deploy Open WebUI. Den remains the control plane (**bear** registry, **users↔bears** membership, policy). **Messaging channels** (Slack, WhatsApp, and other LettaBot adapters) attach **directly to LettaBot**, which still uses **Letta** underneath—the same agent ids Den provisions. **Many‑to‑many:** each user can use many bears; some bears are shared by many users. Den enforces membership on every **web** request before involving LettaBot; channel access control remains LettaBot’s pairing / allowlists plus Den-generated config.

### Den implementation (Axum)

- **Stack:** Axum + reqwest (no official Letta Rust SDK).
- **Letta base URL:** e.g. `http://bears-letta:8283` on Coolify internal network. Use **`LETTA_SERVER_PASS`** (or your Letta version’s admin auth) for server-to-server calls—never expose to browsers.
- **OpenAPI:** Generate typed clients from **your** Letta server’s spec if published (path varies by version; check [Letta docs](https://docs.letta.com)); otherwise call REST paths you verify against the running image.
- **Streaming:** Expose **SSE** (or NDJSON) to the browser by forwarding or adapting LettaBot’s streaming response; use `reqwest-eventsource`, `eventsource-stream`, or equivalent from Axum handlers (**Loquix** is the reference client; optional **Open WebUI** adapters). Confirm stream shapes against your LettaBot version when implementing the Den bridge.

Examples below use **Python/TypeScript** for readability; **Den** implements the same flows via reqwest.

---

## Letta concepts (self-hosted)

API shapes depend on your Letta version—confirm against your server.

### Bears, users, and conversations

- A **bear** is one **Letta agent**. **Users ↔ bears** is **many‑to‑many**: store `(user_id, bear_id)` membership in Den; optional roles (owner, member, read‑only).
- **Conversations** isolate threads (Slack thread, WhatsApp chat, Loquix or Open WebUI session). Prefer **per-conversation** message APIs where available so concurrent channels do not block each other.

### Memory blocks

- **human**, **persona**, optional **shared** read-only blocks (org policy)—same ideas as Cloud; create/attach via your server’s blocks/agents API or Letta UI.

### Provisioning bears (Den-owned)

**Den** is responsible for **bear lifecycle**: create/update the Letta agent, record the bear in Den’s registry, attach **users↔bears** membership, **regenerate LettaBot config** and keep **Loquix** / **optional Open WebUI** client views consistent, and (when Cabinet exists) set **Cabinet** permissions per user and bear.

**Templates / Identities** as described for Letta Cloud may not exist on self-hosted builds. Typical flow:

1. **Den** calls Letta’s API to **create or update** the Letta agent (model, system prompt, tools, memory blocks) for a new or changed **bear**.
2. Den stores **`bear_id` ↔ `associated_letta_id`** plus metadata (name, description, tool flags, default model, …).
3. Den maintains **`(user_id, bear_id)`** membership (many‑to‑many).
4. Den **publishes** bear lists: **Loquix** and Den JSON APIs expose membership-filtered bears; **optional Open WebUI** adapter / agent picker sources from the same Den APIs; **LettaBot** `lettabot.yaml` (or generated fragment) is updated so channel allowlists reference the correct Letta agent ids for each bear.
5. When Cabinet ships: Den applies **deck/kind ACLs** per `(user_id, bear_id)` on Cabinet operations.

Admins may still use the Letta UI for experiments; **production truth** for which bears exist and who may use them should live in **Den**.

For a concise list of **Letta agent knobs that Den’s bear UI does not yet drive**, see [LETTA_BEAR_UI_EXPOSURE.md](LETTA_BEAR_UI_EXPOSURE.md).

---

## System architecture

```
  Loquix (on Den) ─────┐
  Open WebUI (opt.) ───┼──► Den ──────► LettaBot ───► Letta ───► Bifrost ───► providers
                       │      (v1 web: Den auth + membership, then LettaBot)

  Slack / WhatsApp / … ─────────────────► LettaBot ───► Letta ───► Bifrost ───► providers
```

**Web** chat is **never** Den → Letta for messages: **Web → Den → LettaBot → Letta**. **Channels** talk to **LettaBot**, which persists through **Letta**. Den may call Bifrost separately for **metrics/health** (not inference). Older notes in [PLAN.md](../planning/PLAN.md) that described optional **LettaBot → Den → Letta** are superseded here by **Den → LettaBot → Letta** for web; Letta remains the persistence API LettaBot calls.

### Cabinet (Outline)

Long-lived shared knowledge: **bears** via **Den** Cabinet tools; humans in **Outline**. See [PLAN.md](../planning/PLAN.md).

---

## Den — behavioral requirements

1. **Authenticate** end users (OAuth, session, API key, etc.).
2. **Register bears** and **`(user_id, bear_id)`** membership (many‑to‑many); optional `letta_identity` metadata if you use identities.
3. **Provision bears:** create/update Letta agents via Letta’s API (state backend); keep registry and clients in sync (**Loquix**, optional Open WebUI, **LettaBot** config).
4. **Route** chat: resolve **bear** + conversation, call **LettaBot** (not Letta’s HTTP message APIs directly from Den for end users), **stream** the response back to the browser or client.
5. **Enforce** membership: the authenticated user may only invoke **bears** they belong to (on web paths Den controls before LettaBot).
6. **Cabinet (later):** enforce per‑user, per‑bear permissions on Cabinet tools.
7. **Channel users ↔ Den `user_id`:** optional but valuable for a unified directory—map `(channel, external_id)` to `user_id` for operator views, analytics, and config; LettaBot still owns real-time channel I/O.

### Slack & WhatsApp (LettaBot-native)

Channels connect to **LettaBot**; Den supplies generated **LettaBot yaml** (agents, allowlists, skills paths when Den manages them). Lazy or admin-mapped **`external_identities`** for `(channel, external_id) → user_id` remains useful for Den-side UX; see [PLAN.md](../planning/PLAN.md) value-add table.

### Public API (Den)

Minimum surface (names align with [PLAN.md](../planning/PLAN.md) where noted):

| Endpoint | Method | Description |
|----------|--------|-------------|
| /auth/login | POST | Authenticate, session token |
| /auth/signup | POST | Create user + attach to default bear(s) and/or provision new bears on Letta as policy dictates |
| /chat/send | POST | User message → **LettaBot** → Letta (streaming); same role as `/chat/message` |
| /chat/message | POST | Optional alias for clients expecting this name |
| /chat/conversations | GET/POST | List / create conversations |
| /agents | GET | **Bears** visible to user (member list) |
| /, /console, /assets/* | GET | **Operator console** (priority): provisioning UI; **Loquix** end-user chat on **`/app` or `/chat`** (primary browser path) |
| /admin/* | … | User/bear admin JSON (+ operator session); automation may use `ADMIN_API_KEY` server-side |

Cabinet tool endpoints are internal or agent-facing per PLAN.

---

## LettaBot (core platform)

**LettaBot is required** for BEARS: it is the **runtime** that performs agent interaction (channels, Letta Code–style skills, tool loops, streaming to whatever fronts it). **Letta** is the **persistence and server API** LettaBot uses—agents, blocks, conversations, and model calls **through Letta → Bifrost**.

- **LettaBot → Letta:** LettaBot’s `server` block points at the self-hosted Letta HTTP API (e.g. `baseUrl: http://bears-letta:8283`, admin API key)—this is the **normal** LettaBot ↔ Letta link, not a “shortcut” to bypass Den for policy reasons.
- **Den → LettaBot:** Den bridges **browser and operator-initiated** traffic so web chat matches channel behavior. Implementation detail (HTTP adapter, sidecar, shared network) belongs in `den/` and deployment docs; the **contract** is **Web → Den → LettaBot → Letta**.

```yaml
server:
  letta:
    apiKey: ${LETTA_SERVER_PASS}
    baseUrl: http://bears-letta:8283   # Letta = persistence backend for LettaBot

agents:
  - name: user-alice
    agentId: <letta-agent-id>
    channels:
      slack:
        allowedUsers: ["U_ALICE_SLACK_ID"]
      whatsapp:
        allowedUsers: ["+15551234567"]
```

Regenerate `lettabot.yaml` (or `LETTABOT_CONFIG_YAML`) from Den’s DB when **bears** or **users↔bears** membership changes. Skill directories and [Agent Skills](https://agentskills.io/)–compatible trees can be managed from **Den’s operator UI** by materializing files/volumes LettaBot reads, aligned with [Letta Code skills](https://docs.letta.com/letta-code/skills/).

---

## Operator console (provisioning UI)

**Purpose:** Ship **before** (or in tight parallel with) end-user chat: browser flows for **operator login**, **users**, **bears** + **Letta provision**, **membership**, **LettaBot yaml** handoff, and optional **Letta connectivity** check. See [PHASE1_BOOTSTRAP.md](../planning/PHASE1_BOOTSTRAP.md) for routes, `is_admin`, and milestones **M4b** / **first user-testable moment**.

---

## Den native web UI (Loquix, end-user chat) — **primary**

**Purpose:** The default **browser** chat experience for **end users** follows **Web → Den → LettaBot → Letta** so there is a **single** agent stack for web and channels (**Letta → Bifrost** for models remains as today). Mount under **`/app` or `/chat`** so **`/` can remain the operator console**. **Streaming and request shapes should be optimized for Loquix first**; other clients (optional Open WebUI) adapt.

**Stack:** [Loquix](https://github.com/loquix-dev/loquix) — Lit 3 **web components** (`loquix-chat-container`, `loquix-message-list`, `loquix-chat-composer`, streaming and attachment patterns as needed). Import `@loquix/core`, tokens CSS, and `define/*` entrypoints per Loquix docs; ship static `index.html` + bundled JS from **`den/static/`** (or build step) and serve with `tower-http::services::ServeDir` (or embed with `rust-embed`).

**Integration:**

1. **Session or Bearer auth** — Loquix page uses `credentials: 'include'` or `Authorization` on `fetch` to `POST /v1/chat/send` (SSE or NDJSON—**match one contract** and document it in `den/README.md`; this is the **reference** contract). Den authenticates and checks membership, then invokes **LettaBot** (not raw Letta message APIs).
2. **Bear picker** — populate `loquix-model-selector` (or a simple custom list) from `GET /v1/bears` / `GET /agents` (membership-filtered).
3. **Streaming** — forward **LettaBot’s** stream through Den to the browser; consume in the page with `ReadableStream` / `EventSource` and append to `loquix-message-content` (see Loquix **Streaming chat** recipe).

**Ops:** Same Den deployment; you can run **Den + Loquix + LettaBot + Letta** without Open WebUI. **Same-origin** Loquix avoids cross-origin cookie complexity.

---

## Open WebUI (optional)

Point Open WebUI (or a pipe function) at **Den**, not raw Letta, when multi-user auth and routing matter. Den forwards to **LettaBot → Letta** using the **same membership rules** as Loquix. Optional: OpenAI-compatible shim on Den for `/v1/chat/completions`. Ship after Loquix proves the Den → LettaBot chat contract (**M6b** in [PHASE1_BOOTSTRAP.md](../planning/PHASE1_BOOTSTRAP.md)).

---

## Deployment

| Component | Notes |
|-----------|--------|
| **Self-hosted Letta** | Coolify service; volume for `/root/.letta`; `LETTA_SERVER_PASS`; `LLM_API_URL` → Bifrost |
| **Den** | Axum service; `LETTA_BASE_URL=http://bears-letta:8283`; Letta admin credential; `DATABASE_URL`; `SESSION_SECRET`; Outline/Cabinet credentials when Phase 3+ |
| **PostgreSQL** | Den users, **bears**, **users↔bears** membership, sessions |
| **LettaBot** | **Required**; Slack/WhatsApp/… tokens; config volume; connects to Letta for persistence |
| **Loquix (static)** | Served by Den; **primary** browser chat — **same origin** to Den; chat traffic **Den → LettaBot** |
| **Open WebUI** | **Optional**; talks to Den when deployed; Den → LettaBot → Letta |

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

- Deploy Letta + Bifrost per [DEPLOYMENT.md](../deployment/DEPLOYMENT.md)
- Create a **baseline agent** (or template script) for per-user clones
- Harden **Letta admin** credential; reachable only from Den / internal network
- Attach **Cabinet** tools when Den exposes them ([PLAN.md](../planning/PLAN.md))

### Security

- Letta admin access is **full**; keep it on the internal network and only on **Den** (server-side).
- Den validates every **web** request before calling **LettaBot** (and uses Letta’s admin APIs only where provisioning requires it server-side).
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
| **Self-hosted Letta** | **Persistence backend** for LettaBot: agent state, memory blocks, conversations, tools, calls to Bifrost |
| **LettaBot** | **Core agent platform**: all end-user and channel interaction; uses Letta for state; [skills](https://docs.letta.com/letta-code/skills/) and Letta Code behaviors apply here |
| **Den (Axum)** | Auth; **bear** provisioning on Letta + **LettaBot** config; **users↔bears** membership; **web** routing **Den → LettaBot**; Cabinet API; operator console; optional channel↔user mapping for directory/analytics |
| **Loquix (on Den)** | **Primary** browser UI → **Den** → **LettaBot** |
| **Open WebUI** | **Optional** web UI → **Den** → **LettaBot** when deployed |
| **PostgreSQL** | Den: users, mappings, sessions |
