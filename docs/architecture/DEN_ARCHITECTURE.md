# Multi-User Architecture: Den (Axum) + Self-Hosted Letta

*Earlier notes drew on Letta Discord discussion:* https://discord.com/channels/1161736243340640419/1467667826730078386

BEARS uses **only self-hosted Letta** (e.g. `letta/letta:latest` on Coolify). **Den** is the control plane and gateway (**Rust / Axum**). **For Phase 1 and bear chat**, **Letta calls Bifrost directly** for model calls; Den may talk to Bifrost **for observability** on that path (metrics/health/logs). **Future** Den features (for example control-plane LLM helpers) are **not** required to route through Bifrost—see [PLAN.md](../planning/PLAN.md) §2.5.

### Three layers (names)

| Layer | Product | Role |
|-------|---------|------|
| **Persistence** | **Letta** (self-hosted server) | Agent state: memory blocks, conversations, tool registration, model calls **Letta → Bifrost**. This is Letta’s **memory and persistence** API. |
| **Harness** | **[Letta Code](https://docs.letta.com/letta-code)** (`letta` CLI, `letta server`) | **Agent execution**: skills, tool loops, local tools, [Channels](https://docs.letta.com/letta-code/channels/) (e.g. Slack), [scheduling](https://docs.letta.com/letta-code/scheduling) (`letta cron` with `letta server`). Sits **between** Den and Letta’s persistence. |
| **Control plane** | **Den** | **Operations**: identity, bears, membership, skill and MCP catalogs, materialized config, **Den meta tools**, first-party **web chat UI**. (You can also call this the **operations layer**—same thing.) |

**Mandatory harness:** every path that talks to a bear goes **through Letta Code**, which uses **Letta** as its persistence backend. Den does **not** call Letta’s message APIs directly for end-user chat; it bridges to the harness so **web (Den)** and **channels** share one stack. **Channel priority for us:** **Slack** and the **Den web UI**; **WhatsApp** is desired but **not** in Letta Code [Channels](https://docs.letta.com/letta-code/channels/) yet (beta today: Slack + Telegram)—track upstream or use an interim approach until it exists.

**Phase 1 implementation:** [PHASE1_BOOTSTRAP.md](../planning/PHASE1_BOOTSTRAP.md) — Rust service in repo-root **`den/`**; **Trestle** is a throwaway bootstrap label for milestone 0 only, not a directory in this repo.

## Overview

**v1 Den:** **Operator console** (browser, priority) provisions **users**, **bears** (Letta agents), **membership**, and surfaces **Letta Code harness** deploy config (env, channel bind instructions, skill paths, generated `letta-code.yaml` from Den); **end-user chat** is **Web → Den → Letta Code → Letta** via **Den's chat UI** on a path such as `/app` or `/chat` (**primary** path — same Axum routes: auth, bear list, **SSE streaming** `POST /v1/chat/send` with Den proxying or bridging to the harness). **Open WebUI → Den → Letta Code → Letta** is **optional** when you deploy Open WebUI. Den remains the control plane (**bear** registry, **users↔bears** membership, policy). **Slack** attaches via Letta Code **[Channels](https://docs.letta.com/letta-code/channels/)** (`letta server --channels slack`, bind agent); **WhatsApp** is not in Channels yet—see roadmap note above. **Many‑to‑many:** each user can use many bears; some bears are shared by many users. Den enforces membership on every **web** request before involving the harness; Slack DM policies and routes follow Letta Code + Den-generated config.

### Den implementation (Axum)

- **Stack:** Axum + reqwest (no official Letta Rust SDK).
- **Letta base URL:** e.g. `http://bears-letta:8283` on Coolify internal network. Use **`LETTA_SERVER_PASS`** (or your Letta version’s admin auth) for server-to-server calls—never expose to browsers.
- **OpenAPI:** Generate typed clients from **your** Letta server’s spec if published (path varies by version; check [Letta docs](https://docs.letta.com)); otherwise call REST paths you verify against the running image.
- **Streaming:** Expose **SSE** (or NDJSON) to the browser by forwarding or adapting the **Letta Code** harness streaming response; use `reqwest-eventsource`, `eventsource-stream`, or equivalent from Axum handlers (Den's chat UI is the reference client; optional **Open WebUI** adapters). Confirm stream shapes against your deployed **Letta Code** version when implementing the Den bridge.

Examples below use **Python/TypeScript** for readability; **Den** implements the same flows via reqwest.

---

## Letta concepts (self-hosted)

API shapes depend on your Letta version—confirm against your server.

### Bears, users, and conversations

- A **bear** is the **primary Letta agent** in Den’s registry (the assistant users talk to). Den also tracks **harness binding**: Slack channel bind, `LETTA_AGENT_ID` for `letta channels bind`, **skill** paths, and—where used—**predefined subagent** configuration (e.g. Letta **`reflection`** and related types) so deploys are reproducible; see [dynamic-skills-subagents-adr.md](../dynamic-skills-subagents-adr.md). One bear ↔ one primary Letta `agent_id` plus materialized harness config. **Users ↔ bears** is **many‑to‑many**: store `(user_id, bear_id)` membership in Den; optional roles (owner, member, read‑only).
- **Conversations** isolate threads (Slack thread, WhatsApp chat, Den chat or Open WebUI session). Prefer **per-conversation** message APIs where available so concurrent channels do not block each other.

### Memory blocks

- **human**, **persona**, optional **shared** read-only blocks (org policy)—same ideas as Cloud; create/attach via your server’s blocks/agents API or Letta UI.
- **Archival memory** (Letta): vector-searchable store the agent uses per Letta’s tools — complementary to blocks, not a Den-managed layer. **Phase 1 UX:** **memory dashboard** shows **`human`** plus a **holistic weight** per member bear (cross-bear “how much learned,” not capacity pressure); **bear detail** shows full Letta **state** (all blocks + archival where exposed). Product copy distinguishes **curated blocks** vs **retrievable** archival. See [PLAN.md](../planning/PLAN.md) § Phase 1 memory model and [PHASE1_DECISIONS.md](../planning/PHASE1_DECISIONS.md) decision 8.
- **Shared blocks and multiple writers:** Block tools use **read-modify-write** on a single string without CAS; concurrent **`memory_insert`** can drop updates, **`memory_replace`** fails loudly, and **compiled context** can lag across conversations (**LET-7893**). Team-oriented deployments should follow the patterns in [PLAN.md § Shared memory blocks and concurrency](../planning/PLAN.md#shared-memory-blocks-and-concurrency-letta) (single-writer or append-only logs, proxy-side size limits, optional conversation recompile). Future **memfs**/git-backed blocks (**LET-8217**) add merge/conflict semantics and **limit** bypass risk (**LET-8133**)—see that section for detail.

### Provisioning bears (Den-owned)

**Den** is responsible for **bear lifecycle**: create/update the Letta agent, record the bear in Den’s registry, attach **users↔bears** membership, **manage skills per bear for the harness** (see [Den-managed skills](#den-managed-skills)), **regenerate harness deploy config** and materialize skill trees, keep **optional Open WebUI** client views consistent, and (when Cabinet exists) set **Cabinet** permissions per user and bear.

**Templates / Identities** as described for Letta Cloud may not exist on self-hosted builds. Typical flow:

1. **Den** calls Letta’s API to **create or update** the Letta agent (model, system prompt, tools, memory blocks) for a new or changed **bear**.
2. Den stores **`bear_id` ↔ `associated_letta_id`** plus metadata (name, description, tool flags, default model, …).
3. Den maintains **`(user_id, bear_id)`** membership (many‑to‑many).
4. Den **publishes** bear lists: Den JSON APIs expose membership-filtered bears; **optional Open WebUI** adapter / agent picker sources from the same Den APIs; **generated harness config** (env templates, Slack bind instructions, skill paths) is updated so each bear maps to the correct Letta agent id.
5. When Cabinet ships: Den applies **deck/kind ACLs** per `(user_id, bear_id)` on Cabinet operations.

Admins may still use the Letta UI for experiments; **production truth** for which bears exist and who may use them should live in **Den**.

For a concise list of **Letta agent knobs that Den’s bear UI does not yet drive**, see [LETTA_BEAR_UI_EXPOSURE.md](LETTA_BEAR_UI_EXPOSURE.md).

---

## System architecture

```
  Den chat UI ─────────┐
  Open WebUI (opt.) ───┼──► Den ──────► Letta Code ───► Letta ───► Bifrost ───► providers
                       │      (v1 web: Den auth + membership, then harness)

  Slack (Channels) ────────────────────► Letta Code ───► Letta ───► Bifrost ───► providers
  WhatsApp — not in Letta Code Channels yet (desired; see text above)
```

**Web** chat is **never** Den → Letta for messages: **Web → Den → Letta Code → Letta**. **Slack** uses the **Letta Code** harness ([Channels](https://docs.letta.com/letta-code/channels/), beta). Den may call Bifrost separately for **metrics/health** on the **bear inference** path (not bear chat inference through Den). Older notes in [PLAN.md](../planning/PLAN.md) that described optional **harness → Den → Letta** are superseded here by **Den → Letta Code → Letta** for web; Letta remains the persistence API the harness calls.

### Cabinet (Outline)

Long-lived shared knowledge: **bears** via **Den** Cabinet tools; humans in **Outline**. See [PLAN.md](../planning/PLAN.md). **Tool shape:** Cabinet access for agents follows the **Den meta tools** pattern ([Den-controlled facade](#den-meta-tools-bears-control-plane-tools), **Letta Code–brokered**), not a separate MCP requirement by default.

---

## Den — behavioral requirements

1. **Authenticate** end users (OAuth, session, API key, etc.).
2. **Register bears** and **`(user_id, bear_id)`** membership (many‑to‑many); optional `letta_identity` metadata if you use identities.
3. **Provision bears:** create/update Letta agents via Letta’s API (state backend); keep registry and clients in sync (optional Open WebUI, **Letta Code** harness config).
4. **Route** chat: resolve **bear** + conversation, call the **Letta Code** harness (not Letta’s HTTP message APIs directly from Den for end users), **stream** the response back to the browser or client.
5. **Enforce** membership: the authenticated user may only invoke **bears** they belong to (on web paths Den controls before the harness).
6. **Cabinet (later):** enforce per‑user, per‑bear permissions on Cabinet tools.
7. **Channel users ↔ Den `user_id`:** optional but valuable for a unified directory—map `(channel, external_id)` to `user_id` for operator views, analytics, and config; Letta Code still owns real-time Slack I/O.

### Slack (Letta Code Channels) and WhatsApp (desired)

**Slack** connects to **Letta Code** via **[Channels](https://docs.letta.com/letta-code/channels/)** (beta): `letta server --channels slack`, `letta channels bind --channel slack --agent <id>`. Den supplies generated **config and skill paths** the harness reads. **WhatsApp** is not available in Channels yet; treat as roadmap / follow upstream.

Lazy or admin-mapped **`external_identities`** for `(channel, external_id) → user_id` remains useful for Den-side UX; see [PLAN.md](../planning/PLAN.md) value-add table.

### Public API (Den)

Minimum surface (names align with [PLAN.md](../planning/PLAN.md) where noted):

| Endpoint | Method | Description |
|----------|--------|-------------|
| /auth/login | POST | Authenticate, session token |
| /auth/signup | POST | Create user + attach to default bear(s) and/or provision new bears on Letta as policy dictates |
| /chat/send | POST | User message → **Letta Code** harness → Letta (streaming); same role as `/chat/message` |
| /chat/message | POST | Optional alias for clients expecting this name |
| /chat/conversations | GET/POST | List / create conversations |
| /agents | GET | **Bears** visible to user (member list) |
| /, /console, /assets/* | GET | **Operator console** (priority): provisioning UI; end-user chat on **`/bear/{slug}`** (primary browser path) |
| /admin/* | … | User/bear admin JSON (+ operator session); automation may use `ADMIN_API_KEY` server-side |

Cabinet tool endpoints are internal or agent-facing per PLAN.

---

## Letta Code (harness layer)

**Letta Code is required** for BEARS: it is the **[harness](https://docs.letta.com/letta-code)** that runs the agent loop—skills, tool execution, [Channels](https://docs.letta.com/letta-code/channels/) (Slack), [scheduling](https://docs.letta.com/letta-code/scheduling), streaming to Den for web. **Letta** is the **persistence and server API** the harness uses—agents, blocks, conversations, and model calls **through Letta → Bifrost**.

**Routines (Phase 1):** **Den** stores **first-class** scheduled work (**routines**) each **bound to one bear**; execution is delegated to the harness/Letta per [routines-automation-adr.md](../routines-automation-adr.md). **Output delivery** (artifacts vs conversation vs hybrid) is an **open** design topic; **no** automatic skill-learning from unattended runs by default ([PHASE1_DECISIONS.md](../planning/PHASE1_DECISIONS.md) decision **10**).

- **Letta Code → Letta:** Point `LETTA_BASE_URL` at the self-hosted Letta HTTP API (e.g. `http://bears-letta:8283`) and use the Letta admin API key—this is the normal harness ↔ persistence link, not a shortcut to bypass Den for policy reasons.
- **Den → Letta Code:** Den bridges **browser and operator-initiated** traffic so web chat matches Slack. Implementation detail (HTTP adapter, sidecar, shared network) belongs in `den/` and deployment docs; the **contract** is **Web → Den → Letta Code → Letta**.
- **Tools:** For BEARS-defined capabilities (below), **Letta Code is the execution broker** between agents and **Den**—not a place to embed ad hoc tool scripts. See [Den meta tools](#den-meta-tools-bears-control-plane-tools).

Example **environment** (illustrative; confirm against [Letta Code docs](https://docs.letta.com/letta-code)):

```bash
LETTA_BASE_URL=http://bears-letta:8283
LETTA_API_KEY=${LETTA_SERVER_PASS}
# Slack (Channels): run `letta server --channels slack` and:
#   letta channels bind --channel slack --agent <letta-agent-id>
```

Regenerate **harness deploy artifacts** (e.g. `letta-code.yaml` from the operator console) from Den’s DB when **bears** or **users↔bears** membership changes.

### Den-managed skills

**Den is the system of record for which [Letta Code / Agent Skills](https://docs.letta.com/letta-code/skills/) each bear may use.** Operators attach skills **per bear** (underlying Letta agent). **Letta Code** **loads and runs** skills from the filesystem layouts it supports; Den does not reimplement the skill runtime.

**Responsibilities**

- **Catalog:** Den stores skill metadata (name, source URL or package id, pinned revision, scope: org-wide vs user-uploaded) and optional trust flags.
- **Attachment:** `(bear_id, skill_id, enabled, order)` (or equivalent) defines the skill set for that bear’s bot.
- **Materialization:** On change, Den writes or syncs [Agent Skills](https://agentskills.io/)–compatible **directories** (`SKILL.md` + assets) onto paths **Letta Code** reads—e.g. per-agent under `~/.letta/agents/{letta_agent_id}/skills/`, shared org library mapped to **global** skill dirs—**exact layout depends on your Letta Code version**; keep this mapping in deploy docs next to volume mounts.
- **Sharing:** Reuse the same catalog entry across many bears; materialize **copies** per agent dir or use a **shared** directory plus Den policy for who may attach which catalog skill.

**Operator console:** paste GitHub URLs, pick from catalog, preview, enable/disable, reorder. **GitOps:** exported config or CI can drive the same materialization inputs as the UI.

**Security:** Treat skills as **trusted code adjacent to the agent**; restrict who can publish org skills; cap size; validate fetches (SSRF, malware, prompt injection) per org policy.

### Dynamic skills, reflection subagents, and bear configuration

**Beyond static catalog skills:** BEARS targets **dynamic** skills—operators attach **catalog** skills per bear (above), and **bears** may **create or refine** skills over time using **Letta Code** capabilities (e.g. upstream **skills-creation** patterns) and Letta **subagent** mechanisms such as **`reflection`** for auto-discovery. **Den** does not run the skill runtime; it **extends bear provisioning** so each bear’s configuration includes **predefined subagents** and remains **GitOps-friendly**.

**Single ADR:** [dynamic-skills-subagents-adr.md](../dynamic-skills-subagents-adr.md) — preliminary decisions, security notes, and an **inspirational** expert sketch (e.g. `skill-curator` subagent, `Task` policy, `SubagentStop` hook, git staging). BEARS prioritizes **user/operator control** over promoted skills; expert “conservative” bias is optional, not the default product goal.

### Den-managed MCP servers (Phase 1)

**Den is the system of record for which [Model Context Protocol](https://modelcontextprotocol.io/) (MCP) servers each bear may use.** Letta and **Letta Code** remain the **runtime** that opens MCP sessions and invokes tools; Den does not replace an MCP host. This mirrors **Den-managed skills**: same split between **catalog**, **per-bear attachment**, and **materialization** into config the runtime reads.

**Responsibilities**

- **Catalog (local registry):** Den stores MCP server metadata (display name, transport hints, org vs imported, optional link to the [official MCP Registry](https://modelcontextprotocol.io/registry), trust flags). Operators may **query or import** from the official registry for discovery; **cataloging a public server does not require Den to proxy** tool traffic.
- **Attachment:** `(bear_id, mcp_server_id, enabled, order)` (or equivalent) defines which MCP servers a bear’s agent may use, analogous to skills.
- **Materialization:** On change, Den updates generated **harness** config, sidecar env, or Letta agent fields (for example `tool_ids` or MCP transport blocks) as supported by your Letta and Letta Code versions—**exact wiring belongs in deploy docs** next to how the harness reaches each MCP URL or stdio command.
- **Provisioning:** **Coolify** (or your stack orchestrator) runs MCP server **containers or processes**; Den records **connection templates** (internal base URL, stdio command shape, required env **names**) and **policy**, not ad hoc process spawning inside Den.

**Shared patterns with skills:** Operator console flows (catalog table, attach to bear, reorder, disable), GitOps or exported config driving the same inputs as the UI, reuse of trust and review habits.

**Security:** Treat MCP servers like **network-exposed and executable-adjacent** capabilities: allowlists, secrets injected by the platform (Coolify → env), supply-chain review for imports, SSRF policy on any fetch-by-URL catalog path.

See [PLAN.md](../planning/PLAN.md) Phase 1 for the phased implementation checklist (MCP alongside skills).

---

## Den meta tools (BEARS control-plane tools)

**Den-controlled tool facade, brokered through Letta Code.** BEARS distinguishes **control-plane** capabilities (identity, policy, org data) from generic **skills** and third-party **MCP** integrations. Those capabilities use a single architectural pattern: **Den** is the operational center; **Letta Code** brokers execution; **Letta** holds agent state but is **not** where BEARS-specific tool *implementations* live as ad-hoc local scripts.

**Principles**

- **Den** owns **tool definitions** (names, JSON schemas), **permissions**, **availability** per bear or environment, **routing**, and **rollout** configuration. Policy and enablement are **Den’s system of record**, not mutable one-off state inside a Letta runtime UI.
- **Den** implements capabilities as **stable HTTP APIs** (or internal services behind a documented base path). Those endpoints are **reusable** by other components (operator console, automation, future clients)—not buried inside a single runtime process.
- **Letta Code** is the **execution broker**: it resolves **which** tools an agent may call from **Den** (or from config materialized from Den), **invokes Den-routed operations** during the tool loop, and returns **normalized** results to the agent. Agents see tools **surfaced through the harness**; the **source of truth** for what those tools are remains **Den**.
- **No ad hoc local tool code** (for example custom Python dropped into the Letta server’s tool sandbox) for BEARS control-plane features. **Deployment must be reproducible** from **version-controlled Den and Letta Code** config—plus the usual DB backups and object storage—not from manual edits in a Letta console.

**Intent.** These tools cover **BEARS control-plane** work: enforce **Den** policy, update **Letta** state in a governed way (e.g. conversation `summary`), or reach **Cabinet** through Den. Examples: **conversation titles**, **Cabinet** search/read/write with deck/kind ACLs, future **meta** actions (audit hooks, rate hints, feature gates).

**Relationship to skills and MCP**

- **[Den-managed skills](#den-managed-skills)** and **[Den-managed MCP servers](#den-managed-mcp-servers-phase-1)** stay the right patterns for **filesystem skills** and **optional third-party** tool hosts (Exa, Composio, org MCP). Letta/Letta Code remain the runtime that loads skills and opens MCP sessions where applicable.
- **Den meta tools** are **tightly coupled** to BEARS identity and policy. They are **not** “must be MCP”—use MCP when you **deliberately** want a portable or vendor-hosted server.
- **Cabinet** follows the **same** Den-facade pattern: **Den** is the policy and API boundary to Outline; **Letta Code** brokers calls into Den. A separate “Cabinet MCP” is unnecessary unless you integrate an external MCP Den does not own—**Den still fronts policy** on every call.

**Contract (conceptual).**

1. **Authorization.** Enforce **user and membership** in **Den** on every invocation (signed server-to-server context from the harness, HMAC, mTLS, or equivalent). Do **not** trust model-supplied user ids for security boundaries.
2. **Scope.** Bind each call to **`bear_id` / `letta_agent_id`** and, when relevant, **`conversation_id`**; reject cross-bear or out-of-membership use.
3. **Provisioning.** Den’s registry and materialized config record **which** meta tools each bear has; GitOps and review apply the same as other bear fields. Letta agent / harness wiring reflects that catalog—**without** embedding implementation source in the Letta process.
4. **Transport.** Exact wire format (HTTP from Letta Code to Den, streaming callbacks, etc.) is defined by **Letta Code + Den** integration work—not by scattering executors in the Letta server. The invariant is: **handlers live in Den**; **Letta Code** mediates the agent tool loop.

**When MCP is still appropriate.** Third-party catalogs, reuse across products, or **optional** per-bear attachments that are **not** BEARS-specific—attach via the MCP catalog and materialize into harness/Letta config as documented elsewhere in this file.

---

## Operator console (provisioning UI)

**Purpose:** Ship **before** (or in tight parallel with) end-user chat: browser flows for **operator login**, **users**, **bears** + **Letta provision**, **membership**, **skills and MCP servers per bear**, **Letta Code harness** deploy handoff / sync (`letta-code.yaml`), and optional **Letta connectivity** check. See [PHASE1_BOOTSTRAP.md](../planning/PHASE1_BOOTSTRAP.md) for routes, `is_admin`, and milestones **M4b** / **first user-testable moment**.

---

## Den native web UI (end-user chat) — **primary**

**Purpose:** The default **browser** chat experience for **end users** follows **Web → Den → Letta Code → Letta** so there is a **single** agent stack for web and Slack (**Letta → Bifrost** for models remains as today). Mount under **`/app` or `/chat`** so **`/` can remain the operator console**. Other clients (optional Open WebUI) adapt.

**Stack:** [Deep Chat](https://deepchat.dev) web component (`<deep-chat>`) vendored under `den/src/web/assets/deep-chat/`. MiniJinja template at `src/web/templates/bear_chat.html`; handler in `src/web/bear_chat.rs`.

**Integration:**

1. **Session or Bearer auth** — chat page uses `credentials: 'same-origin'` on `fetch` to `POST /v1/chat/send` (SSE). Den authenticates and checks membership, then invokes the **Letta Code** harness (not raw Letta message APIs).
2. **Bear picker** — dashboard at `/` lists membership-filtered bears with links to `/bear/{slug}`.
3. **Streaming** — forward the **harness** stream through Den to the browser; consume in the page with `ReadableStream` / `EventSource` and the Deep Chat handler parses `data:` SSE lines and renders `assistant_message` content.

**Ops:** Same Den deployment; you can run **Den + Letta Code + Letta** without Open WebUI. Same-origin chat avoids cross-origin cookie complexity.

---

## Open WebUI (optional)

Point Open WebUI (or a pipe function) at **Den**, not raw Letta, when multi-user auth and routing matter. Den forwards to **Letta Code → Letta** using the **same membership rules** as the Den chat UI. Optional: OpenAI-compatible shim on Den for `/v1/chat/completions`. Ship after the Den chat UI proves the Den → Letta Code chat contract (**M6b** in [PHASE1_BOOTSTRAP.md](../planning/PHASE1_BOOTSTRAP.md)).

---

## Deployment

| Component | Notes |
|-----------|--------|
| **Self-hosted Letta** | Coolify service; volume for `/root/.letta`; `LETTA_SERVER_PASS`; `LLM_API_URL` → Bifrost |
| **Den** | Axum service; `LETTA_BASE_URL=http://bears-letta:8283`; Letta admin credential; `DATABASE_URL`; `SESSION_SECRET`; Outline/Cabinet credentials when Phase 3+ |
| **PostgreSQL** | Den users, **bears**, **users↔bears** membership, sessions |
| **Letta Code** | **Required** harness (`letta server`); Slack via [Channels](https://docs.letta.com/letta-code/channels/); tokens and `~/.letta/channels/` state; connects to Letta for persistence |
| **Den chat UI** | Served by Den (Deep Chat web component); **primary** browser chat — **same origin** to Den; chat traffic **Den → Letta Code** |
| **Open WebUI** | **Optional**; talks to Den when deployed; Den → Letta Code → Letta |

```bash
# Den (example)
LETTA_BASE_URL=http://bears-letta:8283
LETTA_AUTH=<LETTA_SERVER_PASS or Bearer token per your Letta version>
DATABASE_URL=postgresql://...
SESSION_SECRET=...

# Letta Code harness (example — see Letta Code docs)
# Slack: configure via `letta channels configure slack`, `letta server --channels slack`
```

### Self-hosted Letta checklist

- Deploy Letta + Bifrost per [DEPLOYMENT.md](../deployment/DEPLOYMENT.md)
- Create a **baseline agent** (or template script) for per-user clones
- Harden **Letta admin** credential; reachable only from Den / internal network
- Wire **Cabinet** and other **Den meta tools** through the **Den facade + Letta Code broker** when deployed ([PLAN.md](../planning/PLAN.md)); avoid ad hoc tool scripts in the Letta runtime

### Security

- Letta admin access is **full**; keep it on the internal network and only on **Den** (server-side).
- Den validates every **web** request before calling the **harness** (and uses Letta’s admin APIs only where provisioning requires it server-side).
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
| **Self-hosted Letta** | **Persistence backend**: agent state, memory blocks, conversations, tools, calls to Bifrost |
| **Letta Code** | **Harness**: agent loop, [skills](https://docs.letta.com/letta-code/skills/), channels (Slack), scheduling; uses Letta for persistence |
| **Den (Axum)** | **Control plane**: auth; **bear** provisioning on Letta + **harness** config; **skills catalog and per-bear skill sets** (materialized for Letta Code); **users↔bears** membership; **web** routing **Den → Letta Code**; **Den meta tools** (definitions and APIs in Den, **brokered by Letta Code**); Cabinet API; operator console; optional channel↔user mapping for directory/analytics |
| **Den chat UI** | **Primary** browser UI → **Den** → **Letta Code** |
| **Open WebUI** | **Optional** web UI → **Den** → **Letta Code** when deployed |
| **PostgreSQL** | Den: users, mappings, sessions |
