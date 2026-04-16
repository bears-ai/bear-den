# Multi-User Architecture: Den (Axum) + Self-Hosted Letta

*Earlier notes drew on Letta Discord discussion:* https://discord.com/channels/1161736243340640419/1467667826730078386

BEARS uses **only self-hosted Letta** (e.g. `letta/letta:latest` on Coolify). **Den** is the control plane and gateway (**Rust / Axum**). **For Phase 1 and bear chat**, **Letta calls Bifrost directly** for model calls; Den may talk to Bifrost **for observability** on that path (metrics/health/logs). **Future** Den features (for example control-plane LLM helpers) are **not** required to route through Bifrost—see [PLAN.md](../planning/PLAN.md) §2.5.

**LettaBot** is the **mandatory agent runtime** for all conversational and tool-driven interaction: every path that talks to a bear goes **through LettaBot**, which in turn uses **Letta** as its **state and persistence backend** (agents, memory blocks, conversations, tools). Den does **not** call Letta’s message APIs directly for end-user chat; it forwards to LettaBot so browser, messaging channels, and future clients share one stack (including [Letta Code skills](https://docs.letta.com/letta-code/skills/) behavior where applicable).

**Phase 1 implementation:** [PHASE1_BOOTSTRAP.md](../planning/PHASE1_BOOTSTRAP.md) — Rust service in repo-root **`den/`**; **Trestle** is a throwaway bootstrap label for milestone 0 only, not a directory in this repo.

## Overview

**v1 Den:** **Operator console** (browser, priority) provisions **users**, **bears** (Letta agents), **membership**, and surfaces **LettaBot** yaml; **end-user chat** is **Web → Den → LettaBot → Letta** via **Den's chat UI** on a path such as `/app` or `/chat` (**primary** path — same Axum routes: auth, bear list, **SSE streaming** `POST /v1/chat/send` with Den proxying or bridging to LettaBot). **Open WebUI → Den → LettaBot → Letta** is **optional** when you deploy Open WebUI. Den remains the control plane (**bear** registry, **users↔bears** membership, policy). **Messaging channels** (Slack, WhatsApp, and other LettaBot adapters) attach **directly to LettaBot**, which still uses **Letta** underneath—the same agent ids Den provisions. **Many‑to‑many:** each user can use many bears; some bears are shared by many users. Den enforces membership on every **web** request before involving LettaBot; channel access control remains LettaBot’s pairing / allowlists plus Den-generated config.

### Den implementation (Axum)

- **Stack:** Axum + reqwest (no official Letta Rust SDK).
- **Letta base URL:** e.g. `http://bears-letta:8283` on Coolify internal network. Use **`LETTA_SERVER_PASS`** (or your Letta version’s admin auth) for server-to-server calls—never expose to browsers.
- **OpenAPI:** Generate typed clients from **your** Letta server’s spec if published (path varies by version; check [Letta docs](https://docs.letta.com)); otherwise call REST paths you verify against the running image.
- **Streaming:** Expose **SSE** (or NDJSON) to the browser by forwarding or adapting LettaBot’s streaming response; use `reqwest-eventsource`, `eventsource-stream`, or equivalent from Axum handlers (Den's chat UI is the reference client; optional **Open WebUI** adapters). Confirm stream shapes against your LettaBot version when implementing the Den bridge.

Examples below use **Python/TypeScript** for readability; **Den** implements the same flows via reqwest.

---

## Letta concepts (self-hosted)

API shapes depend on your Letta version—confirm against your server.

### Bears, users, and conversations

- A **bear** is one **Letta agent** in Den’s registry. In deployment terms it is also the **LettaBot agent row** that fronts that Letta agent: one bear ↔ one `agents[]` entry in generated **LettaBot** config (`agentId` = Letta’s agent id). **Users ↔ bears** is **many‑to‑many**: store `(user_id, bear_id)` membership in Den; optional roles (owner, member, read‑only).
- **Conversations** isolate threads (Slack thread, WhatsApp chat, Den chat or Open WebUI session). Prefer **per-conversation** message APIs where available so concurrent channels do not block each other.

### Memory blocks

- **human**, **persona**, optional **shared** read-only blocks (org policy)—same ideas as Cloud; create/attach via your server’s blocks/agents API or Letta UI.

### Provisioning bears (Den-owned)

**Den** is responsible for **bear lifecycle**: create/update the Letta agent, record the bear in Den’s registry, attach **users↔bears** membership, **manage skills for each bear’s LettaBot row** (see [Den-managed skills](#den-managed-skills)), **regenerate LettaBot config** and materialize skill trees, keep **optional Open WebUI** client views consistent, and (when Cabinet exists) set **Cabinet** permissions per user and bear.

**Templates / Identities** as described for Letta Cloud may not exist on self-hosted builds. Typical flow:

1. **Den** calls Letta’s API to **create or update** the Letta agent (model, system prompt, tools, memory blocks) for a new or changed **bear**.
2. Den stores **`bear_id` ↔ `associated_letta_id`** plus metadata (name, description, tool flags, default model, …).
3. Den maintains **`(user_id, bear_id)`** membership (many‑to‑many).
4. Den **publishes** bear lists: Den JSON APIs expose membership-filtered bears; **optional Open WebUI** adapter / agent picker sources from the same Den APIs; **LettaBot** `lettabot.yaml` (or generated fragment) is updated so channel allowlists reference the correct Letta agent ids for each bear.
5. When Cabinet ships: Den applies **deck/kind ACLs** per `(user_id, bear_id)` on Cabinet operations.

Admins may still use the Letta UI for experiments; **production truth** for which bears exist and who may use them should live in **Den**.

For a concise list of **Letta agent knobs that Den’s bear UI does not yet drive**, see [LETTA_BEAR_UI_EXPOSURE.md](LETTA_BEAR_UI_EXPOSURE.md).

---

## System architecture

```
  Den chat UI ─────────┐
  Open WebUI (opt.) ───┼──► Den ──────► LettaBot ───► Letta ───► Bifrost ───► providers
                       │      (v1 web: Den auth + membership, then LettaBot)

  Slack / WhatsApp / … ─────────────────► LettaBot ───► Letta ───► Bifrost ───► providers
```

**Web** chat is **never** Den → Letta for messages: **Web → Den → LettaBot → Letta**. **Channels** talk to **LettaBot**, which persists through **Letta**. Den may call Bifrost separately for **metrics/health** on the **bear inference** path (not bear chat inference through Den). Older notes in [PLAN.md](../planning/PLAN.md) that described optional **LettaBot → Den → Letta** are superseded here by **Den → LettaBot → Letta** for web; Letta remains the persistence API LettaBot calls.

### Cabinet (Outline)

Long-lived shared knowledge: **bears** via **Den** Cabinet tools; humans in **Outline**. See [PLAN.md](../planning/PLAN.md). **Tool shape:** Cabinet access for agents is **Den-native meta tools** (same pattern as [Den meta tools](#den-meta-tools-bears-control-plane-tools)), not a separate MCP requirement.

---

## Den — behavioral requirements

1. **Authenticate** end users (OAuth, session, API key, etc.).
2. **Register bears** and **`(user_id, bear_id)`** membership (many‑to‑many); optional `letta_identity` metadata if you use identities.
3. **Provision bears:** create/update Letta agents via Letta’s API (state backend); keep registry and clients in sync (optional Open WebUI, **LettaBot** config).
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
| /, /console, /assets/* | GET | **Operator console** (priority): provisioning UI; end-user chat on **`/bear/{slug}`** (primary browser path) |
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

Regenerate `lettabot.yaml` (or `LETTABOT_CONFIG_YAML`) from Den’s DB when **bears** or **users↔bears** membership changes.

### Den-managed skills

**Den is the system of record for which [Letta Code / Agent Skills](https://docs.letta.com/letta-code/skills/) each bear’s bot may use.** Operators attach skills **per bear** (i.e. per LettaBot `agents[]` entry / underlying Letta agent). LettaBot continues to **load and run** skills from the filesystem layouts it supports; Den does not reimplement the skill runtime.

**Responsibilities**

- **Catalog:** Den stores skill metadata (name, source URL or package id, pinned revision, scope: org-wide vs user-uploaded) and optional trust flags.
- **Attachment:** `(bear_id, skill_id, enabled, order)` (or equivalent) defines the skill set for that bear’s bot.
- **Materialization:** On change, Den writes or syncs [Agent Skills](https://agentskills.io/)–compatible **directories** (`SKILL.md` + assets) onto paths LettaBot reads—e.g. per-agent under `~/.letta/agents/{letta_agent_id}/skills/`, shared org library mapped to **global** skill dirs, or paths referenced from generated yaml—**exact layout depends on your LettaBot/Letta Code version**; keep this mapping in deploy docs next to volume mounts.
- **Sharing:** Reuse the same catalog entry across many bears; materialize **copies** per agent dir or use a **shared** directory plus Den policy for who may attach which catalog skill.

**Operator console:** paste GitHub URLs, pick from catalog, preview, enable/disable, reorder. **GitOps:** exported config or CI can drive the same materialization inputs as the UI.

**Security:** Treat skills as **trusted code adjacent to the agent**; restrict who can publish org skills; cap size; validate fetches (SSRF, malware, prompt injection) per org policy.

### Den-managed MCP servers (Phase 1)

**Den is the system of record for which [Model Context Protocol](https://modelcontextprotocol.io/) (MCP) servers each bear may use.** Letta and LettaBot remain the **runtime** that opens MCP sessions and invokes tools; Den does not replace an MCP host. This mirrors **Den-managed skills**: same split between **catalog**, **per-bear attachment**, and **materialization** into config the runtime reads.

**Responsibilities**

- **Catalog (local registry):** Den stores MCP server metadata (display name, transport hints, org vs imported, optional link to the [official MCP Registry](https://modelcontextprotocol.io/registry), trust flags). Operators may **query or import** from the official registry for discovery; **cataloging a public server does not require Den to proxy** tool traffic.
- **Attachment:** `(bear_id, mcp_server_id, enabled, order)` (or equivalent) defines which MCP servers a bear’s agent may use, analogous to skills.
- **Materialization:** On change, Den updates generated **LettaBot** yaml, sidecar config, or Letta agent fields (for example `tool_ids` or MCP transport blocks) as supported by your Letta and LettaBot versions—**exact wiring belongs in deploy docs** next to how LettaBot reaches each MCP URL or stdio command.
- **Provisioning:** **Coolify** (or your stack orchestrator) runs MCP server **containers or processes**; Den records **connection templates** (internal base URL, stdio command shape, required env **names**) and **policy**, not ad hoc process spawning inside Den.

**Shared patterns with skills:** Operator console flows (catalog table, attach to bear, reorder, disable), GitOps or exported config driving the same inputs as the UI, reuse of trust and review habits.

**Security:** Treat MCP servers like **network-exposed and executable-adjacent** capabilities: allowlists, secrets injected by the platform (Coolify → env), supply-chain review for imports, SSRF policy on any fetch-by-URL catalog path.

See [PLAN.md](../planning/PLAN.md) Phase 1 for the phased implementation checklist (MCP alongside skills).

---

## Den meta tools (BEARS control-plane tools)

**Intent.** Some agent capabilities are not generic “skills” or third-party MCP integrations—they are **BEARS control-plane** operations: enforce **Den** policy, touch **Letta** state in a governed way, or call **Cabinet** through Den. Examples include **renaming a conversation** (Letta `summary`), **Cabinet** search/read/write with deck/kind ACLs, and future **meta** actions (audit hooks, rate hints, feature gates).

**Default: implement in Den; register on the Letta agent—no MCP required for these.**

- **Placement.** Implement as **Den-owned** modules and HTTP handlers (or a small internal “tool gateway” behind one base path). During **bear provisioning**, register the corresponding **Letta custom tools** (names + schemas) on the bear’s agent alongside `letta_core` tools. **Den** executes the real work: membership checks, `PATCH /v1/conversations/…` for titles, Cabinet API calls to Outline, etc.
- **Why not MCP by default.** [Den-managed MCP servers](#den-managed-mcp-servers-phase-1) (under [LettaBot](#lettabot-core-platform)) remain the right pattern for **optional, reusable, third-party** tool hosts (Exa, Composio, org-specific MCP). **Meta** tools are **tightly coupled** to BEARS identity and policy; keeping them in Den avoids an extra hop, duplicate auth stories, and split ownership. Use MCP when you **deliberately** want portability or a vendor’s server—not because “tools must be MCP.”
- **Cabinet.** **Cabinet** tools follow the **same** pattern: **Den** is the policy and API boundary to Outline; bears invoke **Den-implemented** tools. A separate “Cabinet MCP” is unnecessary unless you integrate an external MCP that Den does not own—**Den still fronts policy** on every call.

**Contract (conceptual).**

1. **Authorization.** Resolve **user and membership** in Den (from tool-execution context Letta provides, signed server-to-server payloads, or internal-only callbacks). Do not trust model-supplied user ids for security boundaries.
2. **Scope.** Bind each invocation to **`bear_id` / `letta_agent_id`** and, when relevant, **`conversation_id`**; reject cross-bear or out-of-membership use.
3. **Provisioning.** Bear create/update attaches the meta tool definitions the Letta agent should expose; Den’s registry records which capabilities each bear has (for UI and GitOps parity).
4. **Transport.** Use whatever **Letta** supports for custom tools (HTTP callback to Den, server-registered executors, etc.); the **architectural** decision is *Den implements the handler*, independent of MCP.

**When MCP is still appropriate.** Third-party catalogs, **Cursor-style** reuse across products, or **optional** per-bear attachments that are **not** BEARS-specific—attach via the MCP catalog and materialize into LettaBot/Letta config as today.

---

## Operator console (provisioning UI)

**Purpose:** Ship **before** (or in tight parallel with) end-user chat: browser flows for **operator login**, **users**, **bears** + **Letta provision**, **membership**, **skills and MCP servers per bear**, **LettaBot yaml** handoff / sync, and optional **Letta connectivity** check. See [PHASE1_BOOTSTRAP.md](../planning/PHASE1_BOOTSTRAP.md) for routes, `is_admin`, and milestones **M4b** / **first user-testable moment**.

---

## Den native web UI (end-user chat) — **primary**

**Purpose:** The default **browser** chat experience for **end users** follows **Web → Den → LettaBot → Letta** so there is a **single** agent stack for web and channels (**Letta → Bifrost** for models remains as today). Mount under **`/app` or `/chat`** so **`/` can remain the operator console**. Other clients (optional Open WebUI) adapt.

**Stack:** [Deep Chat](https://deepchat.dev) web component (`<deep-chat>`) vendored under `den/src/web/assets/deep-chat/`. MiniJinja template at `src/web/templates/bear_chat.html`; handler in `src/web/bear_chat.rs`.

**Integration:**

1. **Session or Bearer auth** — chat page uses `credentials: 'same-origin'` on `fetch` to `POST /v1/chat/send` (SSE). Den authenticates and checks membership, then invokes **LettaBot** (not raw Letta message APIs).
2. **Bear picker** — dashboard at `/` lists membership-filtered bears with links to `/bear/{slug}`.
3. **Streaming** — forward **LettaBot’s** stream through Den to the browser; consume in the page with `ReadableStream` / `EventSource` and the Deep Chat handler parses `data:` SSE lines and renders `assistant_message` content.

**Ops:** Same Den deployment; you can run **Den + LettaBot + Letta** without Open WebUI. Same-origin chat avoids cross-origin cookie complexity.

---

## Open WebUI (optional)

Point Open WebUI (or a pipe function) at **Den**, not raw Letta, when multi-user auth and routing matter. Den forwards to **LettaBot → Letta** using the **same membership rules** as the Den chat UI. Optional: OpenAI-compatible shim on Den for `/v1/chat/completions`. Ship after the Den chat UI proves the Den → LettaBot chat contract (**M6b** in [PHASE1_BOOTSTRAP.md](../planning/PHASE1_BOOTSTRAP.md)).

---

## Deployment

| Component | Notes |
|-----------|--------|
| **Self-hosted Letta** | Coolify service; volume for `/root/.letta`; `LETTA_SERVER_PASS`; `LLM_API_URL` → Bifrost |
| **Den** | Axum service; `LETTA_BASE_URL=http://bears-letta:8283`; Letta admin credential; `DATABASE_URL`; `SESSION_SECRET`; Outline/Cabinet credentials when Phase 3+ |
| **PostgreSQL** | Den users, **bears**, **users↔bears** membership, sessions |
| **LettaBot** | **Required**; Slack/WhatsApp/… tokens; config volume; connects to Letta for persistence |
| **Den chat UI** | Served by Den (Deep Chat web component); **primary** browser chat — **same origin** to Den; chat traffic **Den → LettaBot** |
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
| **Den (Axum)** | Auth; **bear** provisioning on Letta + **LettaBot** config; **skills catalog and per-bear skill sets** (materialized for LettaBot); **users↔bears** membership; **web** routing **Den → LettaBot**; **Den meta tools** (Letta-registered control-plane tools—conversation titles, Cabinet, …); Cabinet API; operator console; optional channel↔user mapping for directory/analytics |
| **Den chat UI** | **Primary** browser UI → **Den** → **LettaBot** |
| **Open WebUI** | **Optional** web UI → **Den** → **LettaBot** when deployed |
| **PostgreSQL** | Den: users, mappings, sessions |
