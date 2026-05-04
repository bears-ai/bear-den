# BEARS roadmap

High-level implementation roadmap for BEARS. This file is the planning hub: it should describe delivery order and link to canonical architecture docs rather than duplicate every contract.

**Architecture sources of truth:**

- [Architecture notes](../architecture/ARCHITECTURE_NOTES.md) — single-page stack view.
- [Den architecture](../architecture/DEN_ARCHITECTURE.md) — control plane, harness, Den meta tools, skills, MCP.
- [`bear_channel` and ACP](../architecture/BEAR_CHANNEL_AND_ACP.md) — canonical Den -> Codepool runtime boundary and planned ACP mapping.
- [Architecture Decision Records](../architecture/adr/README.md) — durable cross-cutting decisions.

**Active implementation plans:**

- [Phase 1 bootstrap](PHASE1_BOOTSTRAP.md)
- [Phase 1 locked decisions](PHASE1_DECISIONS.md)
- [`bear_channel` Phase 7+](BEAR_CHANNEL_PHASE7_PLANS.md)
- [ACP discovery prompt](ACP_DISCOVERY_PROMPT.md)
- [ACP direct local tool runtime](ACP_DIRECT_LOCAL_TOOL_RUNTIME_PLAN.md)
- [Multi-agent bear implementation (doc + UI Phase 8.5)](MULTI_AGENT_IMPLEMENTATION_PLAN.md)


**Reading order:** Use [§3](#3-phased-roadmap) for delivery sequencing. Use [§1](#1-system-architecture)–[§2](#2-capability-contracts-pseudo) only as historical planning context where the architecture docs do not yet cover a topic.

## Table of contents

| Section | Contents |
|---------|----------|
| [§1](#1-system-architecture) | Components, Letta vs Cabinet, [artifacts & Garage](#artifacts-and-object-storage-garage), Phase 1 memory model, [shared blocks & concurrency](#shared-memory-blocks-and-concurrency-letta), [dynamic skills & subagents](#dynamic-skills-reflection-subagents-and-bear-configuration), [routines & scheduling](#routines-and-scheduled-work-phase-1-idea-5), Den→Letta Code→Letta, Den-managed skills and MCP (Phase 1) |
| [§2](#2-capability-contracts-pseudo) | Frontends→Den, Den→Letta, bears→Cabinet, Outline, Bifrost observability |
| [§3](#3-phased-roadmap) | Phase 0–4 milestones |
| [Summary](#summary) | One-page recap |

**Terminology**

- **BEARS** — the **deployment stack** (acronym): Letta, Bifrost, Den, Outline, frontends, **Letta Code** harness, etc. Not the same as a single **bear**.
- **Bear** — the **logical assistant** users and operators name: one coherent identity, membership, and policy. **Runtime:** implemented on Letta as **one or more Letta agents** with explicit **roles** (see [multi-agent-architecture.md](../architecture/adr/multi-agent-architecture.md) and [MULTI_AGENT_IMPLEMENTATION_PLAN.md](MULTI_AGENT_IMPLEMENTATION_PLAN.md)); the old single-agent `bears.letta_agent_id` routing field has been dropped from the active schema. **Subagents** (e.g. Letta **`reflection`** type) remain **configured per bear** where upstream supports them—see [dynamic-skills-subagents.md](../architecture/adr/dynamic-skills-subagents.md). Den registers and provisions the bear **and** materializes predefined subagent configuration for reproducible deploys.
- **Harness binding (per bear)** — Den-generated mapping from a **bear** to **Letta agent ids per role** (e.g. **talk** for web/Slack via Letta Code, **pair** for ACP), plus **Letta Code** skill paths, **subagent** definitions the harness/Letta expect, Slack [Channels](https://docs.letta.com/letta-code/channels/) bind, and related deploy config. Documentation and operator UI must not imply a single anonymous `letta_agent_id` without role—see **Phase 8.5** in [MULTI_AGENT_IMPLEMENTATION_PLAN.md](MULTI_AGENT_IMPLEMENTATION_PLAN.md).
- **Users ↔ bears (many‑to‑many)** — a **user** may access **many** bears (e.g. personal + household + project). A **bear** may be shared by **many** users (e.g. a household assistant). Den stores membership and enforces it on every chat and Cabinet call.
- **Routine** — a **scheduled or triggered** unit of work managed in **Den** (Phase 1), **assigned to exactly one bear**; inherits that bear’s tools, policy, and membership context. **Where outputs are stored or shown** is **not yet fixed** — see [routines-automation.md](../architecture/adr/routines-automation.md).
- **Den** — the **BEARS control plane and gateway** (also the **operations layer** in plain language): identity, **bear lifecycle** (provision Letta agents, surface bears in the **Den chat UI** and **Letta Code** harness config), **[routines](#routines-and-scheduled-work-phase-1-idea-5)** (DB-backed schedules + UI, Phase 1), **[skills catalog and per-bear attachments](https://docs.letta.com/letta-code/skills/)** (materialized for **Letta Code**; see [DEN_ARCHITECTURE.md](../architecture/DEN_ARCHITECTURE.md)), **local MCP server catalog and per-bear MCP attachments** (Phase 1, alongside skills; [DEN_ARCHITECTURE.md](../architecture/DEN_ARCHITECTURE.md)), routing, authz, Cabinet API, and **optional Bifrost observability reads** for the **Letta → Bifrost bear inference path** (details in [§2.5](#25-den-and-bifrost-observability-on-the-bear-path)). Den is the **system of record** for which users may use which bears, which skills each bear loads, **which MCP servers each bear may use**, and how they appear on **web** and **Slack** (**WhatsApp** desired upstream; not in Letta Code Channels yet).
- **bear_id** / **letta_agent_id** / **role_agent_id** — **`bear_id`** is Den’s **public** identifier for a bear in JSON APIs (**v1:** `bear_id` only; see [PHASE1_DECISIONS.md](PHASE1_DECISIONS.md)). **`letta_agent_id`** is Letta’s internal id for **one** runtime agent; a bear may have **several** (per role). Runtime handoffs use **`role_agent_id`** for the selected role (talk / pair / curate / work / watch). The legacy **`bears.letta_agent_id`** column has been dropped; active routing reads `bear_agents`.

---

## 1. System architecture

### Core components

1. **Den** (control plane + gateway), implemented in **Rust with Axum**
   - Maps **external identities** (Slack, WhatsApp, web, etc.) to **internal users**.
   - **Provisions and registers bears:** creates and updates **Letta agents** via API; keeps Den’s **bear registry** in sync (`bear_id` + role ↔ `letta_agent_id`); drives **which bears exist** and **who may use them**.
   - **Surfaces bears in clients:** emits or updates config so **Den's chat UI** (first-party browser chat) and **Letta Code** harness deploy artifacts (e.g. `letta-code.yaml`) list the correct bears per user/channel. The Den chat UI is the **only** first-party web client; *traffic path for web chat is* **Den chat UI → Den → Letta Code → Letta** (the harness is mandatory for agent interaction; see [DEN_ARCHITECTURE.md](../architecture/DEN_ARCHITECTURE.md)).
   - **Manages skills for each bear:** catalog (URLs, pins, org library), attach/detach per bear, then **materialize** [Agent Skills](https://agentskills.io/)–compatible trees onto volumes/paths Letta Code reads; Letta Code remains the runtime that discovers and loads skills. **Dynamic** skills (bear-created / improved over time) and **reflection** subagents follow [dynamic-skills-subagents.md](../architecture/adr/dynamic-skills-subagents.md) and [PHASE1_DECISIONS.md](PHASE1_DECISIONS.md) decision **9**.
   - **Manages MCP servers for each bear (Phase 1, with skills):** **local catalog** in Den (org-defined and curated third-party entries), optional **discovery** from the [official MCP Registry](https://modelcontextprotocol.io/registry) without requiring Den to proxy public servers; **per-bear attachments** using the **same catalog vs attachment pattern** as skills; **provisioning** of MCP server processes left to **Coolify** (Den stores connection metadata and policy, not generic process orchestration).
   - **Manages routines (Phase 1):** **first-class** **schedules** and **routine** definitions in Den (Postgres + operator UI); each routine is **bound to one bear**; **tooling and membership** match that bear. Execution uses **Letta Code** / Letta as the harness; **delivery of outputs** (artifacts vs dedicated conversation vs hybrid) remains **open** — see [routines-automation.md](../architecture/adr/routines-automation.md). **No automatic skill-learning** from unattended routine runs without explicit design ([PHASE1_DECISIONS.md](PHASE1_DECISIONS.md) decision **10**, [dynamic-skills-subagents.md](../architecture/adr/dynamic-skills-subagents.md)).
   - **User and Cabinet permissions:** membership tables (users↔bears); later, **Cabinet** ACLs per user and bear (decks, kinds, read/write)—enforced on Den’s Cabinet API.
   - **Routes** web chat through **Letta Code** to the chosen bear's `talk` role agent; **channels** connect to Letta Code directly, still backed by the role-scoped Letta agent ids Den provisions.
   - **Auth** and **tool/model policies** (RBAC, gating, rate limits).
   - **Cabinet API** for bears (search/read/write), implemented against **Outline**.
   - **Bifrost (Phase 1 and bear chat paths):** On the **end-user bear inference path**, Den does **not** proxy model traffic: **Letta → Bifrost** (via `LLM_API_URL`). Den may still connect to Bifrost for **metrics, health checks, Prometheus scrapes, or log exports** (per your Bifrost version and config), and join that with Den’s `user_id` / `bear_id` / channel data where your logging pipeline allows. **Future flexibility:** Den may call models or gateways **directly** for **control-plane or operational** LLM work without changing the **Letta → Bifrost** path for bear chat.
   - Auth‑aware proxy: frontends ↔ **Letta Code** ↔ **Letta** for chat (not through Bifrost on Den); **bear** tool calls ↔ Cabinet.

2. **Letta** (**self‑hosted only** in BEARS—e.g. Coolify `bears-letta:8283`, not Letta Cloud)
   - **Bear runtime:** conversation loop + tools for each Letta agent (each **bear**).
   - **Model calls:** **Letta → Bifrost** directly (`LLM_API_URL`). No Den in that path.
   - Per‑**bear** configuration: system prompts, tools, memory adapters.
   - Stateless(ish) from Den’s point of view; Den calls the **self‑hosted Letta REST API** (reqwest from Axum).

3. **Letta Code** (**required** in BEARS)
   - **Harness:** [Channels](https://docs.letta.com/letta-code/channels/) (**Slack**; **WhatsApp** not available in Letta Code yet—desired), filesystem **[skills](https://docs.letta.com/letta-code/skills/)**, tool loops, streaming to Den for web.
   - **Letta Code → Letta:** Letta Code uses the self-hosted Letta HTTP API for **persistence** (agents, blocks, conversations, models via **Letta → Bifrost**).
   - **Den → Letta Code:** Den bridges **browser** traffic so web matches channel behavior; Den **does not** call Letta’s end-user message APIs directly for chat.

4. **Web chat (Den)**
   - **Den chat UI** (Deep Chat web component served by Den): the browser calls Den’s **`POST /v1/chat/send`** streaming API and **`GET /v1/bears`** (membership-filtered list; see [PHASE1_BOOTSTRAP.md](PHASE1_BOOTSTRAP.md) for route variants)—Den forwards to **Letta Code → Letta**; **bear** model calls remain **Letta → Bifrost**.

5. **Bifrost**
   - **Letta’s** model gateway for **bear** calls (Letta → Bifrost → providers). Den does not proxy **that** traffic in Phase 1.
   - Observability (logs, metrics, usage) — **Den** may consume Bifrost for dashboards/correlation on the bear path; future Den-side LLM usage may or may not go through Bifrost.

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

### Artifacts and object storage (Garage)

**Files** (agent outputs, tool results, **skills**-mediated artifacts, **user uploads**, **routine** outputs) **do not** live in **Letta**. They are stored in **Garage** (S3-compatible) under a dedicated **artifacts bucket**, keyed and **metadata-tagged** with at least **`conversation_id`**, **bear**, **user**, and **provenance** (including `human_upload` vs `agent`). **Garbage collection** removes stale ephemeral objects per **Den** policy.

- **Architecture record:** [artifacts-garage.md](../architecture/adr/artifacts-garage.md).
- **Cabinet vs artifacts:** **Cabinet attachments** (Outline) use a **separate bucket** and lifecycle—**not** subject to artifact GC. Optional future **UI** to promote an artifact into Cabinet.
- **Deploy:** [services/garage/COOLIFY_DEPLOY.md](../../services/garage/COOLIFY_DEPLOY.md) — two buckets (artifacts + cabinet); Den credentials scoped accordingly.

### Phase 1 memory model (user promise, persistence, and UX)

Aligned with [multi-user-memory.md](../architecture/adr/multi-user-memory.md) (Scenario A for web-first 1:1) and the **Idea 3** product discussion:

- **User-facing promise:** The bear keeps a **small, curated** set of facts in **always-in-context memory blocks** (e.g. `persona`, isolated `human` per conversation/user). **Longer or older material** is **findable when needed** via Letta’s **archival memory** and related tools — not implied to sit in the prompt on every turn. (Letta typically exposes archival as **vector-backed retrieval** the agent invokes **on demand** via tools; confirm behavior against your deployed Letta/Letta Code version.)
- **Persistence:** Phase 1 adds **no second memory store in Den**. All memory semantics remain **Letta-native** (blocks, conversations, archival as Letta implements them). Den surfaces **state** for UX only.
- **UX (two surfaces):**
  - **Memory dashboard (end-user):** Show read access to **`human`** (and related content per [multi-user-memory.md](../architecture/adr/multi-user-memory.md)) for bears the user can access. Do **not** add aggregate scoring, capacity, or pressure metrics in Phase 1; purpose is assurance that Letta-native memory exists, not memory management (Letta owns automation).
  - **Bear detail (operator):** Full **Letta-native state summary** for one bear — **all** memory blocks and **archival** indicators/stats **where the API exposes them**; prefer **tokens** (or whatever the API returns). Read-only **assurance** that Letta has things under control; **not** an affordance to edit or consolidate memory in Den. See **Phase 1 memory visibility (Idea 2)** in [PHASE1_DECISIONS.md](PHASE1_DECISIONS.md).
- **Scope:** Phase 1 stays **1:1 per (user, bear)** for web; **no** new shared “household memory” layer in Den (group-mode / `person:{name}` extras remain as in the ADR).

### Dynamic skills, reflection subagents, and bear configuration

**Goal:** **Catalog skills** from Den plus **bear-authored** skills that can **improve over time**, using **Letta Code** (including upstream **skills-creation** patterns where enabled) and Letta **subagents** such as **`reflection`** for auto-discovery—without Den reimplementing the harness.

- **Architecture record:** [dynamic-skills-subagents.md](../architecture/adr/dynamic-skills-subagents.md) (status **Proposed**). It records an **inspirational Letta expert sketch** (e.g. `skill-curator` subagent, memory policy, `SubagentStop` hook, git staging)—**not** a mandatory blueprint. **Product emphasis:** **users and operators stay in control** of promoted skills; conservatism in the sketch is one possible bias, not the BEARS default goal.
- **Den’s role:** Keep **catalog attach + materialization** as today; **extend bear configuration** so operators define **predefined subagents** (types, parameters) provisioned with the primary agent. **Runtime** remains **Letta Code → Letta**.

### Routines and scheduled work (Phase 1, Idea 5)

**Goal:** **First-class routines** in Den—not only host cron or Letta Code CLI scheduling in docs. Operators (and policy allowing, end users) define **recurring or triggered work** assigned to a **bear**.

- **Decisions:** Each routine has an **assigned bear**; **tools, models, MCP, and membership** are **inherited** from that bear (same as interactive chat). **Skill creation / reflection / curator** must **not** automatically learn from these **background** runs—users stay in control of promoted skills.
- **File outputs:** Routine outputs that are **files** are stored as **artifacts** in **Garage** (S3), **not** in Letta—see [artifacts-garage.md](../architecture/adr/artifacts-garage.md). **How** users browse or open linked results in the web UI (thread list, artifact view, notifications) remains **product UX** on top of the same storage model.
- **Harness:** [Letta Code scheduling](https://docs.letta.com/letta-code/scheduling) and Den-driven triggers remain the execution path; exact invoke contract is implementation detail.

### Shared memory blocks and concurrency (Letta)

BEARS is **team-oriented**: several users (and sometimes **several concurrent agent runs**—e.g. Slack, web, scheduled jobs) can interact with bears whose **memory blocks** are shared or overlap. Letta’s block tools implement **read-modify-write** on a **single string** (`block.value`) without CAS or row-level locking exposed to tools—so **architects must assume contention** on shared blocks. The following summarizes known behaviors and mitigations (upstream issue ids refer to Letta’s tracker).

**Write-path races**

- Memory tools are **not atomic at the DB layer**: two concurrent updates can interleave like a classic lost update (both read `v1`, each writes its own `v2`).
- **`memory_insert`** / legacy **`core_memory_append`**: additive semantics help, but **two concurrent inserts can still drop one** if they race in the same window.
- **`memory_replace`** / legacy **`core_memory_replace`**: find-and-replace on a substring—if the target moved between read and write, the op **fails with an error** (loud failure is desirable under contention). **`memory_insert` is more robust** than **`memory_replace`** when multiple writers exist.
- **Practical ranking:** prefer **`memory_insert`** for cross-writer shared state; treat **`memory_replace`** as **brittle** under contention.

**Read-path staleness**

- Shared block writes become visible to other agents **on the next system-prompt compilation**, not mid-turn.
- **Compiled context is per-conversation.** Issue **LET-7893**: some conversations can retain **stale** compiled context even after an agent-level recompile; **conversation-level** `POST /v1/conversations/{id}/recompile` is the targeted fix (agent-level `POST /v1/agents/{id}/recompile` exists on Cloud). Do **not** assume a write in one conversation is immediately visible in another agent’s in-flight turn—there is **no cross-agent memory barrier** you can rely on from the model’s perspective.

**Memfs / git-backed blocks (if/when issue LET-8217 ships)**

- Shared **memfs** inherits **git-style** merge semantics: concurrent writes can surface as **merge conflicts**, not last-write-wins. Reports also mention **sync bugs** (e.g. git→Postgres webhook failing under concurrent sessions) and **block limit validation bypass** on the memfs write path (**LET-8133**): git sync can persist **unbounded** growth—**do not rely** on server-side `limit` alone for safety.

**Design patterns (implementation agents and Den-adjacent automation)**

1. **Single-writer per shared block** — route all mutations for a given shared block through **one** curator/supervisory agent or job (aligns with upstream “conscience” style patterns, e.g. LET-8179). Others **read** freely.
2. **Prefer `memory_insert` over `memory_replace`** for shared writes; favor an **append-log** shape (“record event”) over rewriting canonical state in place.
3. **Structure shared blocks as append-only logs**; optionally **compact** in a separate, infrequent pass (single writer).
4. **Do not assume write-then-read across agents** — after a writer updates a block, readers may need an explicit **`POST /v1/conversations/{id}/recompile`** (when available) or accept **next-turn** visibility.
5. **Keep mutation-heavy working state in per-agent or per-conversation blocks**; **promote** to shared blocks only at **checkpoints**.
6. **Validate block size in your proxy or automation** — enforce caps in Den or harness-side tooling; do not assume Letta’s limit is enforced on every path (**LET-8133**).
7. **Conflict detection at the boundary you control** — if Den or a meta tool orchestrates writes, stamp versions/timestamps and reject or merge **non-monotonic** updates in that layer.
8. **Monitor for silent desync** — e.g. empty `in_context_message_ids` as a canary for wiped compiled context; consider **heartbeat recompiles** for long-lived conversations.

**Production readiness checks**

- Load-test **concurrent `memory_insert`** from two or more agents against one block—confirm **no drops** at expected concurrency.
- Measure **recompile latency end-to-end** (writer updates → reader’s next turn reflects it).
- Exercise behavior when a block **exceeds configured `limit`** during a shared write (especially if memfs or git paths are enabled).

**Unknowns**

- Exact **race window** size on Cloud’s Postgres path is **not** published—treat contention as **probabilistic**, not negligible.

See also [multi-user-memory.md](../architecture/adr/multi-user-memory.md) (per-user isolation vs shared blocks) and [DEN_ARCHITECTURE.md](../architecture/DEN_ARCHITECTURE.md) (memory blocks overview).

### Canonical paths vs optional channel proxy

**Canonical (BEARS target):** **Den chat UI → Den → Letta Code → Letta** for web; **Slack → Letta Code → Letta** ([Channels](https://docs.letta.com/letta-code/channels/), beta). **WhatsApp** is not in Letta Code Channels yet—track upstream. **Letta** is the persistence backend; **Letta Code** is the harness (skills, tools, channels). **Den** owns registry, membership, **skills and MCP catalogs**, **per-bear materialization** for both (Phase 1), and generated harness config.

**Optional later:** route **channel** messages **Letta Code → Den → Letta Code** (Den in the middle of the messaging hop) **only** if you need a single Den audit point for every Slack/WhatsApp payload—**not** the default and **not** required for web+Letta Code alignment. See [DEN_ARCHITECTURE.md](../architecture/DEN_ARCHITECTURE.md) for the full diagram.

**v1 scope (aligned with Phase 1):** **Den chat UI → Den → Letta Code → Letta**, **bear registry**, **bear provisioning** (Letta agent create/update), **Letta Code** config, **Den-managed skills** and **MCP catalog** (each: catalog + per-bear attach + materialize into runtime-visible config—exact milestones may trail core chat; develop **side-by-side**; see [PHASE1_BOOTSTRAP.md](PHASE1_BOOTSTRAP.md)), **surfacing bears** in Letta Code, **users↔bears** membership, auth, Cabinet API (as phases land), and Bifrost observability reads.

---

## 2. Capability contracts (pseudo)

Not exact APIs, but what each interface *does*.

### 2.1 Frontends → Den

**Purpose:** send authenticated user messages to the right **bear** (Letta agent), get responses back.

**Clients:** **Den chat UI** (primary web UI), automation / API clients, or other HTTP clients—all use the same authenticated endpoints and streaming semantics where applicable.

**Capabilities:**

- `POST /chat/send`
  - Input: `{ user_token, channel, bear_id, message, conversation_id?, metadata }`
    - `user_token`: Den or upstream auth token → Den resolves to internal `user_id`.
    - `channel`: `"slack" | "whatsapp" | "web" | ..."`
    - `bear_id`: which **bear** to talk to (Den’s id → selected role `letta_agent_id`; web chat uses `talk`), or default per-channel—**only if** this user is a member of that bear.
    - `metadata`:
      - `channel_user_id`, `channel_conversation_id`, etc.
  - Behavior:
    - Authenticates `user_token` → `user_id`.
    - Checks policies:
      - Is `bear_id` allowed for this user?
      - Apply rate limits, etc.
    - Constructs a request for **Letta Code** (with `user_id`, `bear_id`, conversation/thread hints, and channel context) so the harness handles the Letta conversation.
    - Lets Letta Code resolve or create the canonical Letta conversation/thread mapping for that bear and channel context.
    - Forwards to **Letta Code → Letta**; streams responses back through Den.
  - Output:
    - Streaming or buffered messages to the frontend.

- `GET /agents/list` (or `/bears/list`)
  - Input: `{ user_token }`
  - Output: **bears** the user is allowed to see/use (Den bear list).

Later, you can add:

- `GET /usage` (per user, per **bear**).
- `GET /logs` (for admin/debug).

---

### 2.2 Den → Letta Code → Letta (chat) and Den → Letta (provisioning)

**Chat (end users):** Den calls **Letta Code** with clear identity and context; Letta Code runs the agent loop and uses **Letta** for persistence and model calls.

You can think of a single logical RPC from Den’s perspective:

- `invoke_bear_bot(user_id, bear_id, message, channel_ctx, session_ctx)`

Where:

- `user_id`: Den’s internal ID (stable across Slack/WhatsApp/web).
- `bear_id`: Den’s **bear** key; resolves to the selected role's **`letta_agent_id`** and the harness binding Den configured (web chat uses `talk`; all web requests must pass **user↔bear** membership checks).
- `channel_ctx`: `{ channel, channel_user_id, channel_conversation_id }`.
- `session_ctx`: optional (recent messages, caller `conversation_id`, thread hints, etc.); **resolved canonically by Letta Code/Letta** in Phase 1.

**Inference path:** user → **Den** → **Letta Code** → **Letta** → **Bifrost**. Den does not touch Bifrost for model requests.

**Provisioning (operators, jobs):** Den still calls **Letta’s REST API** directly to create/update agents, memory blocks, tools, etc., and to read health—**not** the same code path as streaming chat to browsers.

---

### 2.3 Bears → Den (via Letta tools)

Two notable services:

#### a) Cabinet (later phase)

Pseudo‑contract:

- `cabinet_search(query, filters) -> [doc_summary]`
  - `filters` might include `kind`, `project`, `tags`, etc.
  - Implemented as **Den** endpoints that call Outline search.

- `cabinet_get(doc_id) -> full_doc`
- `cabinet_create(kind, title, body, properties) -> doc_id`
- `cabinet_update(doc_id, body?, properties?) -> doc_id`

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

- Which decks a given `user_id` and **bear** (`bear_id`) can touch.
- Property schema (kinds, projects, tags, etc.).

---

### 2.5 Den and Bifrost (observability on the bear path)

- **Traffic:** **Letta → Bifrost** for calls configured against `LLM_API_URL` (typically chat completions). **Embeddings** may use the same URL or direct provider credentials per Letta settings. **Den does not proxy Bifrost for bear chat** in Phase 1.
- **Den’s use of Bifrost:** optional **read-only** integration for observability—e.g. Bifrost **metrics**, **`/health`**, Prometheus, or exported logs—so operators (or Den) can monitor **bear** usage. Correlating calls to Den’s `user_id` / `bear_id` may require **Letta/Bifrost metadata** (e.g. custom headers or logging hooks) configured outside Den’s request path. **Future:** Den-side operational or control-plane LLM calls are **not** required to use Bifrost; keep docs and dashboards clear about **which path** is being described.
- **Naming (`BIFROST_*`):** Den uses **Bifrost-specific** configuration (e.g. `BIFROST_BASE_URL`), not a vendor-neutral `MODEL_GATEWAY_*`, so the **operator console** may assume **Bifrost’s** health routes, Prometheus layout, and documented management APIs without an extra abstraction layer.
- **Policy:** Den enforces **which users may chat with which bears** before forwarding to **Letta Code**; model allowlists at the **Bifrost** layer remain separate (configure both consistently).

---

## 3. Phased roadmap

### Phase 0 – Foundations / prerequisites

**Goal:** Have basic pieces running in isolation.

- Letta running locally or on your infra.
- Bifrost configured as Letta’s model proxy.
- **Letta Code** with **Slack** ([Channels](https://docs.letta.com/letta-code/channels/)) and (optionally) wired to Letta directly for experiments (no **Den** yet).

Deliverables:
- Working Letta + Bifrost stack.
- Working Slack bot talking to *some* **bear** (can be crude).

---

### Phase 1 – **Den**: auth‑aware proxy & **bear** manager (no Cabinet yet)

**Implementation plan:** [PHASE1_BOOTSTRAP.md](PHASE1_BOOTSTRAP.md) — Den (Rust) lives in repo-root **`services/den/`**; **Trestle** is only an ephemeral bootstrap codename for M0 (not a repo path).

**Goal:** **Web chat** follows **browser → Den → Letta Code → Letta**, with identity and policy in Den and a single agent stack for web and channels. The **browser client** is **Den's embedded Deep Chat** UI. **Letta Code** is **in scope** as the required agent runtime; **optional channel-only Den proxy** (Letta Code → Den for audit) stays out of scope unless you explicitly adopt it (see [Canonical paths vs optional channel proxy](#canonical-paths-vs-optional-channel-proxy)).

**Delivery priority:** Ship a **Den-hosted operator console** (browser) **early** so the **first user-testable moment** is “operator provisions users, auth, and bears (Letta + **Letta Code** harness deploy handoff) without API gymnastics.” End-user chat follows as soon as the chat API (**M5**) is stable — see [PHASE1_BOOTSTRAP.md](PHASE1_BOOTSTRAP.md) milestones **M4b**, **M5**, **M6**.

**Capabilities to implement:**

1. **Identity and user mapping** (v1: **web-first**)
   - Minimal user model: `user_id` (and optional external identity columns for Slack/WhatsApp when used).
   - **Slack/WhatsApp mappings** in Den are optional in early Phase 1; they help operator directory and future channel-only Den proxy—not required for v1 web + Letta Code.
   - Simple auth for web: shared secret, basic login, or OAuth.

2. **Bear registry and membership (many‑to‑many)**
   - A DB (or config that Den imports) holding at least:
     - **Bears:** logical bear rows plus role-agent rows such as `{ bear_id, role, letta_agent_id, tools_enabled, default_model, … }` (public JSON uses **`bear_id`** per [PHASE1_DECISIONS.md](PHASE1_DECISIONS.md)).
     - **Membership:** which `user_id`s may use which `bear_id` (and optional roles). A user has many bears; a bear can have many users.
   - **Den’s** job:
     - **Provision** bears in Letta (create/update agents) when admins or workflows add or change a bear; support **duplicate bear** when operators want to start a new bear from an existing configuration.
     - **Publish** bear lists via Den APIs and regenerate **harness** config snippets / `letta-code.yaml` as needed so **web** and **Slack** expose the right bears.
     - Validate `bear_id` on every request; deny if the user is not a member.

3. **Chat proxy API** (on Den)
   - `POST /chat/send`:
     - Accepts message, auth token, optional `bear_id`, and optional `conversation_id`/thread hints.
     - Resolves `user_id`.
     - Applies rate limits / policies.
     - Invokes **Letta Code** for the resolved bear (not raw Letta chat APIs), forwarding conversation context without Den-owned thread mapping.
     - Streams response back.

4. **Web UIs → Den** (v1 release targets)
   - **Operator console (priority):** Den serves a browser UI for **user** accounts, **operator auth**, **bear** CRUD including **duplicate bear**, **Letta provision**, **membership**, **skills and MCP servers per bear**, and **Letta Code** harness deploy preview/download (`/admin/letta-code`, `letta-code.yaml`; see [PHASE1_BOOTSTRAP.md](PHASE1_BOOTSTRAP.md)).
   - **Den native chat:** **end-user** chat page at `/bear/{slug}` — **after** the operator console and chat API are stable; same-origin with Den by default.
   - **Letta Code:** required for **all** chat traffic; Den **updates harness config** and **materializes skills and MCP attachments** so each bear’s binding matches Den’s registry; **Slack** uses Letta Code [Channels](https://docs.letta.com/letta-code/channels/); optional **channel-only** Den proxy later (see [Canonical paths vs optional channel proxy](#canonical-paths-vs-optional-channel-proxy)).

5. **Bifrost observability** (Den reads; no bear-traffic proxy)
   - Letta → Bifrost stays direct for **bear** model calls. **Den** connects to Bifrost **only** for observability (metrics/health/logs APIs or log shipping) as needed on that path.
   - Where possible, align Letta/Bifrost logging with Den’s identity data for attribution.

6. **MCP catalog and bear attachments (Phase 1, alongside skills)**
   - Same **operator and GitOps patterns** as skills: local registry in Den, **per-bear** allowlists, materialization into Letta / Letta Code config as your versions support.
   - **Principles**
     - **No separate self-hosted “MCP management platform”:** Den holds a **local registry** (Postgres or exported config). The [official MCP Registry](https://modelcontextprotocol.io/registry) is an **optional upstream for discovery** (metadata, `server.json`–style identifiers): operators may **import or link** entries; **public third-party servers** can remain **non-proxied** (Den catalogs and authorizes; Letta/Letta Code or the deployment connects per your network layout).
     - **Provisioning:** **Coolify** (or equivalent) deploys **first-party** MCP servers (for example GitHub repository access) and any **third-party** MCP images you choose to run yourself. Den records **how bears may use** each server (URL, stdio template, required secret *names*, internal DNS name)—not a generic multi-tenant MCP process spawner inside Den.
     - **Shared patterns with skills:** **Catalog** rows (metadata, trust flags, source URL or official registry id) + **`(bear_id, server_id, enabled, order)`** attachments + **materialization** into generated yaml, env templates, or Letta agent fields (for example `tool_ids`) as supported by deployed Letta and Letta Code. Operator console: browse/import, attach per bear, reorder, disable. **GitOps:** same export or CI story as skills where applicable.
     - **Security:** Treat MCP as **adjacent executable and network capability** to the agent: allowlists, secret injection via the platform (Coolify secrets to env), SSRF and supply-chain checks for catalog imports, clear audit of which bear may use which server.
   - **Implementation steps** (order flexible vs chat and skills milestones in [PHASE1_BOOTSTRAP.md](PHASE1_BOOTSTRAP.md))
     1. **Schema and APIs:** Local `mcp_servers` (or equivalent) + `bear_mcp_servers` join table; admin CRUD and list-for-bear; optional read-through to the official registry API for **search and import only**.
     2. **Operator UI:** Reuse skills UX patterns (catalog table, attach to bear, per-bear list).
     3. **Deploy integration:** Document Coolify service templates for the first MCP (for example GitHub); Den fields for base URL, command template, and health hints.
     4. **Letta and Letta Code wiring:** Map attached servers to Letta agent tool configuration or Letta Code MCP client configuration as supported by the deployed Letta and Letta Code versions (may trail skills materialization slightly; keep the same release train).
     5. **First catalog entry:** **GitHub-hosted repository access** MCP (org-built or vetted upstream), deployed on Coolify, attached to selected bears.

**Phase 1 success (v1):**

- **Operator console:** provision users, create/duplicate bears (Letta agents), membership, **skills and MCP servers per bear** (each: catalog + attach; materialization may ship shortly after core chat), and harness deploy config from the browser.
- Web users chat **Den’s chat UI → Den → Letta Code → Letta**; Den resolves `user_id`, enforces **bear** membership, streams replies.
- **Slack** uses **Letta Code → Letta** for messages; **WhatsApp** is not in Letta Code Channels yet. Den still drives **which bears**, **which skills**, **which MCP servers**, and harness config.
- Conversation behavior is channel/thread-aware with one canonical owner: Letta Code + Letta. A bear stays consistent across channels while each channel/thread remains a distinct conversation context.
- Bear registry, **users↔bears** membership, and basic RBAC for **web** users.
- No Cabinet/Outline yet: **Letta native memory** only; shared knowledge in later phases.
- **User onboarding:** new account → Personal Bear auto-provisioned → user lands in chat with onboarding prompt.
- **Memory dashboard and bear memory UX:** **Dashboard:** **`human`** (and per-conversation isolation as the Letta API exposes for 1:1 web); **`person:{name}`** blocks appear **when they already exist** (mostly **group-mode**, post–Phase 1). No aggregate scoring, capacity, or pressure metric. **Bear detail** (operator): full **Letta-native** summary — **all** blocks + archival hints; assurance-only, not a management UI. Copy follows the **Phase 1 memory model** ([§ Knowledge model](#knowledge-model-letta-memory-vs-cabinet)): curated blocks vs **findable** longer history — **not** a Den-side memory layer and **not** “everything always in context.”
- **Org policy:** operator sets a shared `org_policy` Letta block (default from `services/den/defaults/org_policy.md`) applied to all bears.
- **Routines:** **first-class** schedules + management UI in Den; each routine **assigned to a bear**; inherited policy/membership; **file outputs** in **Garage** per [artifacts-garage.md](../architecture/adr/artifacts-garage.md) and [routines-automation.md](../architecture/adr/routines-automation.md); **no** automatic skill learning from unattended routine runs ([PHASE1_DECISIONS.md](PHASE1_DECISIONS.md) decision **10**).

---

### Phase 2 – Introduce Cabinet as an abstract service (Outline still in background)

**Goal:** Define the **Cabinet abstraction** and wire it as Letta tools, while starting with a minimal Outline integration.

**Steps:**

1. **Define Cabinet concepts & schema (on paper/config)**
   - Collections ("Decks"): `knowledge`, `history`, `projects`.
   - Properties:
     - `kind`, `project`, `tags`, `people`, `source`, `status`, etc.
   - Decide which **bears** can read/write which kinds.

2. **Cabinet API on Den (skeleton)**
   - Implement Cabinet endpoints (for **bears**):
     - `cabinet_search`, `cabinet_get`, `cabinet_create`, `cabinet_update`.
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

4. **Provisional users and messaging identity**
   - When the **Letta Code** harness reports a message from a user not in Den’s registry (unknown Slack user, WhatsApp number, etc.), Den creates a **provisional user** record: no login credentials, flagged `is_provisional = true`, with a `display_name` derived from the external id.
   - Den provisions `person:{name}` blocks for provisional users on any bear they interact with, so the bear can accumulate knowledge about them across interactions.
   - The admin console shows provisional users alongside full accounts; operators can promote a provisional user to a full account (linking their external id to login credentials).

At the end of Phase 2, Cabinet is a defined, testable contract, even if Outline isn’t fully wired.

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
     - `cabinet_search` uses Outline search + property filters.
     - `cabinet_create` uses Outline document creation with correct deck and props.
     - `cabinet_update` uses Outline document updates.

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
- At least one production‑like workflow uses Cabinet in Slack or Den web chat.

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
   - Align **shared Letta block** updates with [§ Shared memory blocks and concurrency](#shared-memory-blocks-and-concurrency-letta): prefer a **single writer** (or Den-orchestrated merges) for promoted shared state; use **Cabinet** for durable shared knowledge where Letta block semantics are the wrong fit.

3. **RBAC and tool governance**
   - In **Den**:
     - More nuanced rules:
       - Some users/**bears** can write to `knowledge`,
       - Others only to `history` or read‑only.
     - Restrict dangerous or heavy tools/models to specific roles.

4. **Observability & ops polish**
   - **Bifrost** metrics/usage + **Den** logs + optional correlation (Letta does not route **bear** LLM traffic through Den for chat).
   - Dashboards: channel usage (Den), model usage/cost (Bifrost + provider billing), per‑**bear** usage.

This is the “make it livable and reliable” phase.

---

## Summary

**Knowledge:** **Letta memory** is per‑**bear** (per Letta agent) context — **blocks** (curated, bounded) plus **archival** and tools as Letta provides. **Files** (uploads, agent outputs, routines) live in **Garage**, not Letta ([§ Artifacts and object storage](#artifacts-and-object-storage-garage)). **Shared blocks** under multi-writer concurrency need explicit patterns ([§ Shared memory blocks and concurrency](#shared-memory-blocks-and-concurrency-letta)). **Phase 1:** no Den memory store; **dashboard** shows Letta-native `human` memory without aggregate scoring; **bear detail** shows full state ([§ Phase 1 memory model](#phase-1-memory-model-user-promise-persistence-and-ux)). **Cabinet (Outline)** is the shared knowledgebase for humans and bears (post–Phase 1); **Cabinet attachments** use a **separate S3 bucket** from **artifacts**.

**Bears:** Each **primary agent** in the product sense is a **bear**; **subagents** (e.g. reflection) are **part of bear configuration** where used ([dynamic-skills-subagents.md](../architecture/adr/dynamic-skills-subagents.md)). **Users ↔ bears** is **many‑to‑many**.

We’re aiming for:

- **Den** as the **BEARS control plane and gateway**:
  - Maps external identities → internal users; **provisions bears** in Letta; **bear registry** and **users↔bears** membership; **routines** (Phase 1, bear-assigned schedules); **Garage artifacts** (agent outputs, uploads, GC — not Letta); **skills and MCP catalogs** and **per-bear bot attachments** (materialized for Letta Code); **surfaces bears** in **Den chat UI** and Letta Code config; web chat routing **Den → Letta Code → Letta** (Letta → **Bifrost** direct for models).
  - Auth and tool/model policies; per‑user and per‑bear **Cabinet** permissions when Cabinet ships; **Cabinet API** backed by Outline.
  - **Bifrost:** For **Phase 1 and bear chat**, Den uses it **for observability only** (not as a proxy for **bear** model traffic); future Den **control-plane** LLM usage may use other call paths.
  - **v1:** **Den chat UI** → **Den** → **Letta Code** → **Letta** as the **web chat** path. **Letta Code** is **required** for agent interaction; **channels** → Letta Code → Letta. **Optional channel-only Den proxy** for audit is a later value-add (see [Canonical paths vs optional channel proxy](#canonical-paths-vs-optional-channel-proxy)). Cabinet/Outline auth aligned with human auth when Cabinet ships.

- **Phased delivery**:
  - **MVP (Phase 1):** Den for **web** via **Letta Code**; bear lifecycle + membership + **first-class routines** (schedules + UI; files → **Garage** per [artifacts-garage.md](../architecture/adr/artifacts-garage.md)) + **Den-managed skills** and **local MCP catalog** (optional official registry discovery), **per-bear MCP attachments** (same patterns as skills), **Coolify** for MCP server processes + **Garage** (**artifacts** + separate **Cabinet** bucket); first server example **GitHub repository access**; no Cabinet **app** yet.
  - **Phase 2:** Cabinet abstraction defined and wired as tools (even if stubbed); Outline + **`bears-cabinet`** (or equivalent) for document attachments, distinct from **`bears-artifacts`** ([artifacts-garage.md](../architecture/adr/artifacts-garage.md)).
  - **Phase 3:** Cabinet backed by Outline with properties + embeddings.
  - **Phase 4:** Refine memory policies, multi‑user ergonomics, RBAC, and workflows.
