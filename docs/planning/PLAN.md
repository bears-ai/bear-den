# BEARS plan & architecture

High‑level, ops‑oriented plan and architecture: MVP **without Cabinet first**, then Cabinet/Outline in stages.

**Reading order:** Skim [§1](#1-system-architecture)–[§2](#2-capability-contracts-pseudo) for components and pseudo-contracts; use [§3](#3-phased-roadmap) for phased delivery.

## Table of contents

| Section | Contents |
|---------|----------|
| [§1](#1-system-architecture) | Components, Letta vs Cabinet, Den→LettaBot→Letta, Den-managed skills and MCP (Phase 2) |
| [§2](#2-capability-contracts-pseudo) | Frontends→Den, Den→Letta, bears→Cabinet, Outline, Bifrost observability |
| [§3](#3-phased-roadmap) | Phase 0–4 milestones |
| [Summary](#summary) | One-page recap |

**Terminology**

- **BEARS** — the **deployment stack** (acronym): Letta, Bifrost, Den, Outline, frontends, LettaBot, etc. Not the same as a single **bear**.
- **Bear** — one **Letta-backed agent**: a distinct assistant with its own Letta agent id, prompts, memory, and tools. Users interact with **bears**; Den registers and provisions them.
- **Bot (LettaBot row)** — the **LettaBot** `agents[]` entry Den generates for a bear (channels, `agentId` pointing at Letta). “Skill management for the bear” is **skill management for this bot row** and the filesystem paths LettaBot uses for that agent.
- **Users ↔ bears (many‑to‑many)** — a **user** may access **many** bears (e.g. personal + household + project). A **bear** may be shared by **many** users (e.g. a household assistant). Den stores membership and enforces it on every chat and Cabinet call.
- **Den** — the **BEARS control plane and gateway**: identity, **bear lifecycle** (provision Letta agents, surface bears in the **Den chat UI**, **optional Open WebUI**, and LettaBot config), **[skills catalog and per-bear attachments](https://docs.letta.com/letta-code/skills/)** (materialized for LettaBot; see [DEN_ARCHITECTURE.md](../architecture/DEN_ARCHITECTURE.md)), **local MCP server catalog and per-bear MCP attachments** ([Phase 2](#phase-2--introduce-cabinet-as-an-abstract-service-outline-still-in-background); [DEN_ARCHITECTURE.md](../architecture/DEN_ARCHITECTURE.md)), routing, authz, Cabinet API, and Bifrost observability reads. Den is the **system of record** for which users may use which bears, which skills each bear’s bot may load, **which MCP servers each bear may use**, and how they appear in each channel (see below).

---

## 1. System architecture

### Core components

1. **Den** (control plane + gateway), implemented in **Rust with Axum**
   - Maps **external identities** (Slack, WhatsApp, web, etc.) to **internal users**.
   - **Provisions and registers bears:** creates and updates **Letta agents** via API; keeps Den’s **bear registry** in sync (`bear_id` / `agent_id` ↔ `associated_letta_id`); drives **which bears exist** and **who may use them**.
   - **Surfaces bears in clients:** emits or updates config so **Den's chat UI** (first-party browser chat), **optional Open WebUI** (when deployed), and **LettaBot** (`lettabot.yaml` or equivalent) list the correct bears per user/channel. The Den chat UI uses the same **auth, membership, and streaming** endpoints as every other web client; Open WebUI is an **optional** path for teams that want it—not a replacement for Den’s control‑plane role. *Traffic path for web chat is* **Den chat UI → Den → LettaBot → Letta** (LettaBot is mandatory for agent interaction; see [DEN_ARCHITECTURE.md](../architecture/DEN_ARCHITECTURE.md)).
   - **Manages skills for each bear’s bot:** catalog (URLs, pins, org library), attach/detach per bear, then **materialize** [Agent Skills](https://agentskills.io/)–compatible trees onto volumes/paths LettaBot reads; LettaBot remains the runtime that discovers and loads skills.
   - **Manages MCP servers for each bear (Phase 2):** **local catalog** in Den (org-defined and curated third-party entries), optional **discovery** from the [official MCP Registry](https://modelcontextprotocol.io/registry) without requiring Den to proxy public servers; **per-bear attachments** using the **same catalog vs attachment pattern** as skills; **provisioning** of MCP server processes left to **Coolify** (Den stores connection metadata and policy, not generic process orchestration).
   - **User and Cabinet permissions:** membership tables (users↔bears); later, **Cabinet** ACLs per user and bear (decks, kinds, read/write)—enforced on Den’s Cabinet API.
   - **Routes** web chat through **LettaBot** to the correct Letta agent for the chosen bear; **channels** connect to LettaBot directly, still backed by the same Letta agent ids Den provisions.
   - **Auth** and **tool/model policies** (RBAC, gating, rate limits).
   - **Cabinet API** for bears (search/read/write), implemented against **Outline**.
   - **Bifrost (observability only):** Den does **not** proxy model traffic. **Letta calls Bifrost directly.** Den may connect to Bifrost for **metrics, health checks, Prometheus scrapes, or log exports** (per your Bifrost version and config), and join that with Den’s `user_id` / `agent_id` / channel data where your logging pipeline allows.
   - Auth‑aware proxy: frontends ↔ **LettaBot** ↔ **Letta** for chat (not through Bifrost on Den); **bear** tool calls ↔ Cabinet.

2. **Letta** (**self‑hosted only** in BEARS—e.g. Coolify `bears-letta:8283`, not Letta Cloud)
   - **Bear runtime:** conversation loop + tools for each Letta agent (each **bear**).
   - **Model calls:** **Letta → Bifrost** directly (`LLM_API_URL`). No Den in that path.
   - Per‑**bear** configuration: system prompts, tools, memory adapters.
   - Stateless(ish) from Den’s point of view; Den calls the **self‑hosted Letta REST API** (reqwest from Axum).

3. **LettaBot** (**required** in BEARS)
   - **Core agent platform:** channel adapters (Slack, WhatsApp, others), Letta Code–style **skills** discovery/load, tool loops, streaming to Den for web.
   - **LettaBot → Letta:** LettaBot uses the self-hosted Letta HTTP API for **persistence** (agents, blocks, conversations, models via **Letta → Bifrost**).
   - **Den → LettaBot:** Den bridges **browser** traffic so web matches channel behavior; Den **does not** call Letta’s end-user message APIs directly for chat.

4. **Web chat frontends**
   - **Den chat UI** (**primary first-party UI**): Deep Chat web component served by Den; the browser calls Den’s **`POST /v1/chat/send`** streaming API and `GET /agents` or bear list—Den forwards to **LettaBot → Letta**; Bifrost remains **Letta → Bifrost** only.
   - **Open WebUI** (optional): authenticate users (ideally via Den or a shared SSO); forward chat to **Den** (which uses **LettaBot → Letta**) when you choose to deploy it.

5. **Bifrost**
   - **Letta’s** model gateway (Letta → Bifrost → providers). Den does not proxy this traffic.
   - Observability (logs, metrics, usage) — **Den** may consume for dashboards/correlation only.

6. **Cabinet (later)**
   - Logical knowledge layer **bears** use for long‑term reference & history.
   - **API exposed by Den**; storage and human UI on **Outline**.
   - Den enforces identity and policy on every Cabinet operation.

7. **Outline**
   - Human knowledge base UI.
   - Stores docs, properties, versions.
   - Optionally uses its own embeddings for search.

### Knowledge model: Letta memory vs Cabinet

- **Letta’s own memory** (memory blocks, conversations, built-in tools) stays as-is. Cabinet does **not** replace how Letta manages per‑**bear** context, blocks, or the conversation loop.
- **Cabinet** (implemented on **Outline**) is the **shared knowledgebase**: documents that **both humans and bears** can read and edit.

### Canonical paths vs optional channel proxy

**Canonical (BEARS target):** **Den chat UI → Den → LettaBot → Letta** for web; **channels → LettaBot → Letta** for Slack/WhatsApp/etc. **Letta** is the persistence backend; **LettaBot** is the agent runtime (including [skills](https://docs.letta.com/letta-code/skills/)). **Den** owns registry, membership, **skills catalog and per-bear materialization** (Phase 1), **MCP catalog and per-bear MCP attachments** (Phase 2), and generated LettaBot config.

**Optional later:** route **channel** messages **LettaBot → Den → LettaBot** (Den in the middle of the messaging hop) **only** if you need a single Den audit point for every Slack/WhatsApp payload—**not** the default and **not** required for web+LettaBot alignment. See [DEN_ARCHITECTURE.md](../architecture/DEN_ARCHITECTURE.md) for the full diagram.

**v1 scope (aligned with Phase 1):** **Den chat UI → Den → LettaBot → Letta**, **bear registry**, **bear provisioning** (Letta agent create/update), **LettaBot** config and **Den-managed skills** (catalog + per-bear attach + materialize to LettaBot-visible paths—exact milestone can trail core chat; see [PHASE1_BOOTSTRAP.md](PHASE1_BOOTSTRAP.md)), **surfacing bears** in LettaBot and optionally in Open WebUI when enabled, **users↔bears** membership, auth, Cabinet API (as phases land), and Bifrost observability reads. **Open WebUI → Den → LettaBot → Letta** is **optional**. **MCP catalog and per-bear MCP attachments** are **Phase 2** ([§3 Phase 2](#phase-2--introduce-cabinet-as-an-abstract-service-outline-still-in-background)), alongside Cabinet abstraction.

---

## 2. Capability contracts (pseudo)

Not exact APIs, but what each interface *does*.

### 2.1 Frontends → Den

**Purpose:** send authenticated user messages to the right **bear** (Letta agent), get responses back.

**Clients:** **Den chat UI** (primary web UI), **optional Open WebUI** (pipe/adapter), or other HTTP clients—all use the same authenticated endpoints and streaming semantics where applicable.

**Capabilities:**

- `POST /chat/send`
  - Input: `{ user_token, channel, agent_id, message, metadata }`
    - `user_token`: Den or upstream auth token → Den resolves to internal `user_id`.
    - `channel`: `"slack" | "whatsapp" | "webui" | ..."`
    - `agent_id`: which **bear** to talk to (Den’s id → Letta `associated_letta_id`), or default per-channel—**only if** this user is a member of that bear.
    - `metadata`:
      - `channel_user_id`, `channel_conversation_id`, etc.
  - Behavior:
    - Authenticates `user_token` → `user_id`.
    - Checks policies:
      - Is `agent_id` allowed for this user?
      - Apply rate limits, etc.
    - Constructs a request for **LettaBot** (with `user_id`, `agent_id`, channel context) so the bear’s bot row handles the Letta conversation.
    - Forwards to **LettaBot → Letta**; streams responses back through Den.
  - Output:
    - Streaming or buffered messages to the frontend.

- `GET /agents/list` (or `/bears/list`)
  - Input: `{ user_token }`
  - Output: **bears** the user is allowed to see/use (Den bear list; optional Open WebUI agent picker).

Later, you can add:

- `GET /usage` (per user, per **bear**).
- `GET /logs` (for admin/debug).

---

### 2.2 Den → LettaBot → Letta (chat) and Den → Letta (provisioning)

**Chat (end users):** Den calls **LettaBot** with clear identity and context; LettaBot runs the agent loop and uses **Letta** for persistence and model calls.

You can think of a single logical RPC from Den’s perspective:

- `invoke_bear_bot(user_id, agent_id, message, channel_ctx, session_ctx)`

Where:

- `user_id`: Den’s internal ID (stable across Slack/WhatsApp/web).
- `agent_id`: Den’s **bear** key; resolves to the Letta agent id and LettaBot row Den configured (and must pass **user↔bear** membership checks on web).
- `channel_ctx`: `{ channel, channel_user_id, channel_conversation_id }`.
- `session_ctx`: optional (recent messages, conversation ID, etc.) managed by LettaBot/Letta.

**Inference path:** user → **Den** → **LettaBot** → **Letta** → **Bifrost**. Den does not touch Bifrost for model requests.

**Provisioning (operators, jobs):** Den still calls **Letta’s REST API** directly to create/update agents, memory blocks, tools, etc., and to read health—**not** the same code path as streaming chat to browsers.

---

### 2.3 Bears → Den (via Letta tools)

Two notable services:

#### a) Cabinet (later phase)

Pseudo‑contract:

- `cabinet.search(query, filters) -> [doc_summary]`
  - `filters` might include `kind`, `project`, `tags`, etc.
  - Implemented as **Den** endpoints that call Outline search.

- `cabinet.get(doc_id) -> full_doc`
- `cabinet.create(kind, title, body, properties) -> doc_id`
- `cabinet.update(doc_id, body?, properties?) -> doc_id`

Bears never talk directly to Outline; they call these tools, which **Den** implements on top of Outline APIs and policies.

#### b) User profile / preferences (optional but nice)

- `user.get_profile(user_id) -> { name, pronouns, preferences, ... }`
- `user.update_preferences(...)`

Even if initially this is just a thin layer over some DB or config, the contract gives you room to grow.

---

### 2.4 Den → Outline (Cabinet backend)

**Purpose:** use Outline’s docs, properties, and embeddings as the Cabinet storage.

Capabilities (internal to Den):

- `outline.search(query, property_filters) -> docs`
  - Uses Outline’s embeddings + property filters.

- `outline.get_doc(doc_id) -> { title, content, properties }`
- `outline.create_doc(deck_id, title, content, properties) -> doc_id`
- `outline.update_doc(doc_id, content?, properties?)`

Den enforces:

- Which decks a given `user_id` and **bear** (`agent_id`) can touch.
- Property schema (kinds, projects, tags, etc.).

---

### 2.5 Den and Bifrost (observability only)

- **Traffic:** **Letta → Bifrost** for calls configured against `LLM_API_URL` (typically chat completions). **Embeddings** may use the same URL or direct provider credentials per Letta settings. **Den never proxies** Bifrost.
- **Den’s use of Bifrost:** optional **read-only** integration for observability—e.g. Bifrost **metrics**, **`/health`**, Prometheus, or exported logs—so operators (or Den) can monitor usage. Correlating calls to Den’s `user_id` / `agent_id` may require **Letta/Bifrost metadata** (e.g. custom headers or logging hooks) configured outside Den’s request path.
- **Naming (`BIFROST_*`):** Den uses **Bifrost-specific** configuration (e.g. `BIFROST_BASE_URL`), not a vendor-neutral `MODEL_GATEWAY_*`, so the **operator console** may assume **Bifrost’s** health routes, Prometheus layout, and documented management APIs without an extra abstraction layer.
- **Policy:** Den enforces **which users may chat with which bears** before forwarding to **LettaBot**; model allowlists at the **Bifrost** layer remain separate (configure both consistently).

---

## 3. Phased roadmap

### Phase 0 – Foundations / prerequisites

**Goal:** Have basic pieces running in isolation.

- Letta running locally or on your infra.
- Bifrost configured as Letta’s model proxy.
- Open WebUI already talking to Letta (your current state).
- LettaBot installed on Slack and (optionally) wired to Letta directly for experiments (no **Den** yet).

Deliverables:
- Working Letta + Bifrost stack.
- Working Slack bot talking to *some* **bear** (can be crude).

---

### Phase 1 – **Den**: auth‑aware proxy & **bear** manager (no Cabinet yet)

**Implementation plan:** [PHASE1_BOOTSTRAP.md](PHASE1_BOOTSTRAP.md) — Den (Rust) lives in repo-root **`den/`**; **Trestle** is only an ephemeral bootstrap codename for M0 (not a repo path).

**Goal:** **Web chat** follows **browser → Den → LettaBot → Letta**, with identity and policy in Den and a single agent stack for web and channels. The **default browser client** is **Den's chat UI**; **Open WebUI** is an **optional** addition. **LettaBot** is **in scope** as the required agent runtime; **optional channel-only Den proxy** (LettaBot → Den for audit) stays out of scope unless you explicitly adopt it (see [Canonical paths vs optional channel proxy](#canonical-paths-vs-optional-channel-proxy)).

**Delivery priority:** Ship a **Den-hosted operator console** (browser) **early** so the **first user-testable moment** is “operator provisions users, auth, and bears (Letta + LettaBot yaml) without API gymnastics.” End-user chat follows as soon as the chat API (**M5**) is stable — **before** optional Open WebUI — see [PHASE1_BOOTSTRAP.md](PHASE1_BOOTSTRAP.md) milestones **M4b**, **M5**, **M6**.

**Capabilities to implement:**

1. **Identity and user mapping** (v1: **web-first**)
   - Minimal user model: `user_id` + **`webui_account_id → user_id`** when **optional Open WebUI** maps external ids.
   - **Slack/WhatsApp mappings** in Den are optional in early Phase 1; they help operator directory and future channel-only Den proxy—not required for v1 web + LettaBot.
   - Simple auth for web: shared secret, basic login, or OAuth.

2. **Bear registry and membership (many‑to‑many)**
   - A DB (or config that Den imports) holding at least:
     - **Bears:** `agents[bear_id]` (name may stay `agent_id` in APIs) `= { name, description, associated_letta_id, tools_enabled, default_model, … }`.
     - **Membership:** which `user_id`s may use which `bear_id` (and optional roles). A user has many bears; a bear can have many users.
   - **Den’s** job:
     - **Provision** bears in Letta (create/update agents) when admins or workflows add or change a bear.
     - **Publish** bear lists via Den APIs, **optional Open WebUI**, and regenerate **LettaBot** config snippets / `lettabot.yaml` as needed so each channel exposes the right bears.
     - Validate `agent_id` / bear id on every request; deny if the user is not a member.

3. **Chat proxy API** (on Den)
   - `POST /chat/send`:
     - Accepts message, auth token, optional `agent_id`.
     - Resolves `user_id`.
     - Applies rate limits / policies.
     - Invokes **LettaBot** for the resolved bear (not raw Letta chat APIs).
     - Streams response back.

4. **Web UIs → Den** (v1 release targets)
   - **Operator console (priority):** Den serves a browser UI for **user** accounts, **operator auth**, **bear** CRUD and **Letta provision**, **membership**, and **LettaBot** `lettabot.yaml` preview/download (see [PHASE1_BOOTSTRAP.md](PHASE1_BOOTSTRAP.md)).
   - **Den native chat:** **primary end-user** chat page at `/bear/{slug}` — **after** the operator console and chat API are stable; same-origin with Den by default.
   - **Open WebUI (optional):** configure to talk to Den (`/v1/chat/send` or adapter): auth, **bear** picker (only member bears), streaming — ship when a deployment needs it (**M6b** in bootstrap plan).
   - **LettaBot:** required for **all** chat traffic; Den **updates LettaBot config** and **materializes skills** so each bear’s bot row matches Den’s registry; channels use LettaBot natively; optional **channel-only** Den proxy later (see [Canonical paths vs optional channel proxy](#canonical-paths-vs-optional-channel-proxy)).

5. **Bifrost observability** (Den reads, does not proxy)
   - Letta → Bifrost stays direct. **Den** connects to Bifrost **only** for observability (metrics/health/logs APIs or log shipping) as needed.
   - Where possible, align Letta/Bifrost logging with Den’s identity data for attribution.

**Phase 1 success (v1):**

- **Operator console:** provision users, bears (Letta agents), membership, **skills per bear** (catalog + attach; materialization may ship shortly after core chat), and LettaBot yaml from the browser.
- Web users chat **Den’s chat UI → Den → LettaBot → Letta** (**Open WebUI** optional on the same path); Den resolves `user_id`, enforces **bear** membership, streams replies.
- **Slack/WhatsApp** use **LettaBot → Letta** for messages; Den still drives **which bears**, **which skills**, and how they appear in bot config.
- Bear registry, **users↔bears** membership, and basic RBAC for **web** users.
- No Cabinet/Outline yet: **Letta native memory** only; shared knowledge in later phases.
- **User onboarding:** new account → Personal Bear auto-provisioned → user lands in chat with onboarding prompt.
- **Memory dashboard:** users can view their `human` and `person:{name}` Letta blocks across all their bears.
- **Org policy:** operator sets a shared `org_policy` Letta block (default from `den/defaults/org_policy.md`) applied to all bears.

---

### Phase 2 – Introduce Cabinet as an abstract service (Outline still in background)

**Goal:** Define the **Cabinet abstraction** and wire it as Letta tools, while starting with a minimal Outline integration. In parallel, introduce Den’s **local MCP server catalog** and **per-bear MCP attachments**, reusing the **same architectural patterns** as **Den-managed skills** (catalog vs attachment, operator console, GitOps-friendly exports). **MCP server provisioning** (containers, env, networking) stays in **Coolify**; Den is the **policy and metadata** layer and integrates discovered servers with Letta/LettaBot as those products expose MCP (for example `tool_ids`, transport config).

#### Cabinet (existing Phase 2 track)

**Steps:**

1. **Define Cabinet concepts & schema (on paper/config)**
   - Collections ("Decks"): `knowledge`, `history`, `projects`.
   - Properties:
     - `kind`, `project`, `tags`, `people`, `source`, `status`, etc.
   - Decide which **bears** can read/write which kinds.

2. **Cabinet API on Den (skeleton)**
   - Implement Cabinet endpoints (for **bears**):
     - `cabinet.search`, `cabinet.get`, `cabinet.create`, `cabinet.update`.
   - Initially, these can return stub data or use a temporary in‑memory store while you finalize behavior.

3. **Letta tools for Cabinet**
   - Define tools the **bears** can call:
     - `cabinet_search_tool`
     - `cabinet_read_tool`
     - `cabinet_write_tool`
   - Wire them to **Den’s** Cabinet API.
   - Update one or two **bears** to:
     - Use Cabinet for “remembering” things,
     - Summarizing conversations into “knowledge” notes.

4. **Provisional users and LettaBot identity**
   - When LettaBot reports a message from a user not in Den’s registry (unknown Slack user, WhatsApp number, etc.), Den creates a **provisional user** record: no login credentials, flagged `is_provisional = true`, with a `display_name` derived from the external id.
   - Den provisions `person:{name}` blocks for provisional users on any bear they interact with, so the bear can accumulate knowledge about them across interactions.
   - The admin console shows provisional users alongside full accounts; operators can promote a provisional user to a full account (linking their external id to login credentials).

#### MCP catalog and bear attachments (Phase 2)

**Principles**

- **No separate self-hosted “MCP management platform”:** Den holds a **local registry** (Postgres or exported config). The [official MCP Registry](https://modelcontextprotocol.io/registry) is an **optional upstream for discovery** (metadata, `server.json`–style identifiers): operators may **import or link** entries; **public third-party servers** can remain **non-proxied** (Den catalogs and authorizes; Letta/LettaBot or the deployment connects per your network layout).
- **Provisioning:** **Coolify** (or equivalent) deploys **first-party** MCP servers (for example GitHub repository access) and any **third-party** MCP images you choose to run yourself. Den records **how bears may use** each server (URL, stdio template, required secret *names*, internal DNS name)—not a generic multi-tenant MCP process spawner inside Den.
- **Shared patterns with skills:** **Catalog** rows (metadata, trust flags, source URL or official registry id) + **`(bear_id, server_id, enabled, order)`** attachments + **materialization** into whatever Letta/LettaBot needs (generated yaml, env templates, Letta tool registration) once the stack’s MCP wiring is defined. Operator console: browse/import, attach per bear, reorder, disable. **GitOps:** same export or CI story as skills where applicable.
- **Security:** Treat MCP as **adjacent executable and network capability** to the agent: allowlists, secret injection via the platform (Coolify secrets to env), SSRF and supply-chain checks for catalog imports, clear audit of which bear may use which server.

**Implementation steps (order flexible relative to Cabinet milestones)**

1. **Schema and APIs:** Local `mcp_servers` (or equivalent) + `bear_mcp_servers` join table; admin CRUD and list-for-bear; optional read-through to the official registry API for **search and import only**.
2. **Operator UI:** Reuse skills UX patterns (catalog table, attach to bear, per-bear list).
3. **Deploy integration:** Document Coolify service templates for the first MCP (for example GitHub); Den fields for base URL, command template, and health hints.
4. **Letta and LettaBot wiring:** Map attached servers to Letta agent tool configuration or LettaBot MCP client configuration as supported by the deployed Letta and LettaBot versions (may follow Cabinet tool work in the same phase).
5. **First catalog entry:** **GitHub-hosted repository access** MCP (org-built or vetted upstream), deployed on Coolify, attached to selected bears.

At the end of Phase 2, Cabinet is a defined, testable contract, even if Outline isn’t fully wired, and **MCP** is a **first-class, Den-governed** extension path for bear tools with a documented **Coolify plus local catalog** split of responsibilities.

---

### Phase 3 – Back Cabinet with Outline

**Goal:** Swap out the stub/in‑memory Cabinet backend with a real Outline instance.

**Steps:**

1. **Set up Outline**
   - Deploy Outline (self‑hosted).
   - Configure authentication to match/align with Den / BEARS (SSO, shared provider, or Den as OAuth consumer if you go that route).
   - Create decks:
     - `Cabinet – Knowledge`
     - `Cabinet – History`
     - `Cabinet – Projects`

2. **Map Cabinet schema to Outline properties**
   - Decide naming for properties:
     - e.g., `kind`, `project`, `tags`, `people`, `source`, `status`.
   - Confirm Outline APIs for:
     - Reading/writing properties,
     - Searching with embeddings + filters.

3. **Implement Outline‑backed Cabinet adapter in Den**
   - Implement:
     - `outline.search` → Outline API.
     - `outline.get_doc`, `.create_doc`, `.update_doc`.
   - Implement Cabinet methods on top of that:
     - `cabinet.search` → `outline.search` + property filters.
     - `cabinet.create` → `outline.create_doc` with correct deck and props.
     - `cabinet.update` → `outline.update_doc`.

4. **Update bears to use Cabinet for real**
   - Pick one high‑value **bear** use case:
     - E.g., “Household Brain” in Slack that:
       - Stores important decisions/summaries in `knowledge`,
       - Logs key events in `history`.
   - Verify:
     - Humans can browse/edit these docs in Outline.
     - Bears can find and reference them later.

**Phase 3 success:**

- Cabinet is real: Outline is the store, **bears** read/write it **via Den**.
- Human and **bear** access are governed by the same identity/policy layer **in Den**.
- At least one production‑like workflow uses Cabinet in Slack or Open WebUI.

---

### Phase 4 – Memory policies, multi‑user ergonomics, and workflows

**Goal:** Make the system pleasant and robust to live with for Hans, Shannon, and others.

**Focus areas:**

1. **Per‑user vs shared Cabinet policies**
   - Decide:
     - Which conversations get summarized into Cabinet automatically,
     - Which notes go into personal vs shared decks,
     - How to tag docs with `people` and `project` for retrievability.

2. **“Librarian” behavior**
   - Add:
     - Scheduled summaries:
       - E.g., daily/weekly “what changed” docs in `history` or `knowledge`.
     - Tools for “promote this to knowledge” from a chat.

3. **RBAC and tool governance**
   - In **Den**:
     - More nuanced rules:
       - Some users/**bears** can write to `knowledge`,
       - Others only to `history` or read‑only.
     - Restrict dangerous or heavy tools/models to specific roles.

4. **Observability & ops polish**
   - **Bifrost** metrics/usage + **Den** logs + optional correlation (Letta does not route LLM traffic through Den).
   - Dashboards: channel usage (Den), model usage/cost (Bifrost + provider billing), per‑**bear** usage.

This is the “make it livable and reliable” phase.

---

## Summary

**Knowledge:** **Letta memory** is per‑**bear** (per Letta agent) context. **Cabinet (Outline)** is the shared knowledgebase for humans and bears.

**Bears:** Each **agent** in the product sense is a **bear**. **Users ↔ bears** is **many‑to‑many**.

We’re aiming for:

- **Den** as the **BEARS control plane and gateway**:
  - Maps external identities → internal users; **provisions bears** in Letta; **bear registry** and **users↔bears** membership; **skills catalog and per-bear bot attachments** (materialized for LettaBot); **MCP catalog and per-bear MCP attachments** (Phase 2); **surfaces bears** in **Den chat UI**, **optional Open WebUI**, and LettaBot config; web chat routing **Den → LettaBot → Letta** (Letta → **Bifrost** direct for models).
  - Auth and tool/model policies; per‑user and per‑bear **Cabinet** permissions when Cabinet ships; **Cabinet API** backed by Outline.
  - **Bifrost:** Den uses it **only for observability** (not as a proxy for model traffic).
  - **v1:** **Den chat UI** → **Den** → **LettaBot** → **Letta** as the **default web chat** path; **Open WebUI** → Den → LettaBot → Letta **optional** when deployed. **LettaBot** is **required** for agent interaction; **channels** → LettaBot → Letta. **Optional channel-only Den proxy** for audit is a later value-add (see [Canonical paths vs optional channel proxy](#canonical-paths-vs-optional-channel-proxy)). Cabinet/Outline auth aligned with human auth when Cabinet ships.

- **Phased delivery**:
  - **MVP (Phase 1):** Den for **web** via **LettaBot**; bear lifecycle + membership + **Den-managed skills**; no Cabinet yet.
  - **Phase 2:** Cabinet abstraction defined and wired as tools (even if stubbed); **local MCP catalog** (optional official registry discovery), **per-bear MCP attachments** (shared patterns with skills), **Coolify** for MCP server deployment; first server example **GitHub repository access**.
  - **Phase 3:** Cabinet backed by Outline with properties + embeddings.
  - **Phase 4:** Refine memory policies, multi‑user ergonomics, RBAC, and workflows.
