# BEARS plan & architecture

High‑level, ops‑oriented plan and architecture: MVP **without Cabinet first**, then Cabinet/Outline in stages.

**Reading order:** Skim [§1](#1-system-architecture)–[§2](#2-capability-contracts-pseudo) for components and pseudo-contracts; use [§3](#3-phased-roadmap) for phased delivery.

## Table of contents

| Section | Contents |
|---------|----------|
| [§1](#1-system-architecture) | Components, Letta vs Cabinet, optional LettaBot→Den |
| [§2](#2-capability-contracts-pseudo) | Frontends→Den, Den→Letta, bears→Cabinet, Outline, LiteLLM observability |
| [§3](#3-phased-roadmap) | Phase 0–4 milestones |
| [Summary](#summary) | One-page recap |

**Terminology**

- **BEARS** — the **deployment stack** (acronym): Letta, LiteLLM, Den, Outline, frontends, LettaBot, etc. Not the same as a single **bear**.
- **Bear** — one **Letta-backed agent**: a distinct assistant with its own Letta agent id, prompts, memory, and tools. Users interact with **bears**; Den registers and provisions them.
- **Users ↔ bears (many‑to‑many)** — a **user** may access **many** bears (e.g. personal + household + project). A **bear** may be shared by **many** users (e.g. a household assistant). Den stores membership and enforces it on every chat and Cabinet call.
- **Den** — the **BEARS control plane and gateway**: identity, **bear lifecycle** (provision Letta agents, surface bears in Open WebUI and LettaBot config), routing, authz, Cabinet API, and LiteLLM observability reads. Den is the **system of record** for which users may use which bears and how they appear in each channel (see below).

---

## 1. System architecture

### Core components

1. **Den** (control plane + gateway), implemented in **Rust with Axum**
   - Maps **external identities** (Slack, WhatsApp, web, etc.) to **internal users**.
   - **Provisions and registers bears:** creates and updates **Letta agents** via API; keeps Den’s **bear registry** in sync (`bear_id` / `agent_id` ↔ `associated_letta_id`); drives **which bears exist** and **who may use them**.
   - **Surfaces bears in clients:** emits or updates config so **Open WebUI** (agent picker, adapters) and **LettaBot** (`lettabot.yaml` or equivalent) list the correct bears per user/channel. **Den may also serve a first‑party web chat** built with **[Loquix](https://github.com/loquix-dev/loquix)** (framework‑agnostic Lit web components for AI chat): same **auth, membership, and streaming** endpoints as other web clients—**alternative to Open WebUI**, not a replacement for Den’s control‑plane role. *Traffic path* may still be Open WebUI → Den → Letta while LettaBot → Letta stays direct in v1—see optional proxy below—but **provisioning and visibility** are Den’s job.
   - **User and Cabinet permissions:** membership tables (users↔bears); later, **Cabinet** ACLs per user and bear (decks, kinds, read/write)—enforced on Den’s Cabinet API.
   - **Routes** chat to the correct Letta agent for the chosen bear.
   - **Auth** and **tool/model policies** (RBAC, gating, rate limits).
   - **Cabinet API** for bears (search/read/write), implemented against **Outline**.
   - **LiteLLM (observability only):** Den does **not** proxy model traffic. **Letta calls LiteLLM directly.** Den may connect to LiteLLM for **metrics, spend logs, admin API**, etc., and join that with Den’s `user_id` / `agent_id` / channel data where your logging pipeline allows.
   - Auth‑aware proxy: frontends ↔ **Letta** only for chat (not through LiteLLM); **bear** tool calls ↔ Cabinet.

2. **Letta** (**self‑hosted only** in BEARS—e.g. Coolify `bears-letta:8283`, not Letta Cloud)
   - **Bear runtime:** conversation loop + tools for each Letta agent (each **bear**).
   - **Model calls:** **Letta → LiteLLM** directly (`LLM_API_URL`). No Den in that path.
   - Per‑**bear** configuration: system prompts, tools, memory adapters.
   - Stateless(ish) from Den’s point of view; Den calls the **self‑hosted Letta REST API** (reqwest from Axum).

3. **LettaBot**
   - Channel adapters: Slack, WhatsApp (others later).
   - **Initial Den releases:** LettaBot typically talks **directly to Letta** (same as today’s experiments). **Not** required to go through Den for v1.
   - **Optional later:** route LettaBot → **Den** → Letta so messaging channels share Den’s identity and policy with web—see [Den as LettaBot proxy (optional)](#den-as-lettabot--letta-proxy-optional-value-add-not-a-v1-feature) below.

4. **Web chat frontends**
   - **Open WebUI** (and any other web/CLI clients): authenticate users (ideally via Den or a shared SSO); forward chat to **Den** instead of directly to Letta.
   - **Den + Loquix** (optional first‑party UI): HTML/JS served by Den (static assets or embedded) using **[Loquix](https://github.com/loquix-dev/loquix)** components; the browser calls Den’s **same** `POST /chat/send` (or `/v1/chat/send`) streaming API and `GET /agents` / bear list—**no separate inference path**; LiteLLM remains Letta → LiteLLM only.

5. **LiteLLM**
   - **Letta’s** model gateway (Letta → LiteLLM → providers). Den does not proxy this traffic.
   - Observability (logs, costs, metrics) — **Den** may consume for dashboards/correlation only.

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

### Den as LettaBot → Letta proxy (optional value-add, **not** a v1 feature)

Routing **LettaBot** through **Den** (instead of LettaBot → Letta direct) is a **potential** enhancement, **not** part of the **initial Den release**.

**Why you might add it later**

| Benefit | Description |
|--------|-------------|
| **Unified identity** | Slack/WhatsApp external ids live in Den next to web users (`user_id`, `external_identities`). |
| **One policy surface** | Same rate limits, **bear** access, and (with Cabinet) permissions for web and chat apps. |
| **Provisioning** | Den remains the source of truth for bears; lazy onboarding (e.g. first DM) without hand-editing `lettabot.yaml` for every user change. |
| **Audit** | One place to log who used which **bear** on which channel. |

**v1 scope:** Den’s first shipped role is **Open WebUI (web) → Den → Letta**, plus **bear registry**, **bear provisioning** (Letta agent create/update), **surfacing bears** in Open WebUI and LettaBot config, **users↔bears** membership, auth, Cabinet API (as phases land), and LiteLLM observability reads. **LettaBot chat traffic remains direct-to-Letta** until you explicitly route it through Den; Den still **owns** which bears exist and how they appear in LettaBot/Open WebUI.

---

## 2. Capability contracts (pseudo)

Not exact APIs, but what each interface *does*.

### 2.1 Frontends → Den

**Purpose:** send authenticated user messages to the right **bear** (Letta agent), get responses back.

**Clients:** **Open WebUI** (pipe/adapter), a **Den-hosted Loquix** chat page, or other HTTP clients—all use the same authenticated endpoints and streaming semantics where applicable.

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
    - Constructs a Letta request with:
      - `user_id`, `agent_id`, channel context.
    - Forwards to Letta; streams responses back.
  - Output:
    - Streaming or buffered messages to the frontend.

- `GET /agents/list` (or `/bears/list`)
  - Input: `{ user_token }`
  - Output: **bears** the user is allowed to see/use (for Open WebUI agent picker).

Later, you can add:

- `GET /usage` (per user, per **bear**).
- `GET /logs` (for admin/debug).

---

### 2.2 Den → Letta (bear invocation)

**Purpose:** call Letta with clear identity and context; Letta returns messages (and tool calls).

You can think of a single RPC:

- `invoke_agent(user_id, agent_id, message, channel_ctx, session_ctx)`

Where:

- `user_id`: Den’s internal ID (stable across Slack/WhatsApp/web).
- `agent_id`: Den’s **bear** key; resolves to Letta’s agent id via Den’s registry (and must pass **user↔bear** membership checks).
- `channel_ctx`: `{ channel, channel_user_id, channel_conversation_id }`.
- `session_ctx`: optional (recent messages, conversation ID, etc.) that Den or Letta manages.

Letta returns:

- Model messages,
- Tool calls (e.g., `{"tool": "cabinet.search", ...}`),
- Final responses.

Den doesn’t need to know Letta’s internal details for routing chat. **Inference path:** user → Den → Letta → **LiteLLM** (direct). Den does not touch LiteLLM for model requests.

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

### 2.5 Den and LiteLLM (observability only)

- **Traffic:** **Letta → LiteLLM** for all completions/embeddings. **Den never proxies** LiteLLM.
- **Den’s use of LiteLLM:** optional **read-only** integration for observability—e.g. LiteLLM **metrics**, **spend tracking**, admin API, or exported logs—so operators (or Den) can monitor cost and usage. Correlating calls to Den’s `user_id` / `agent_id` may require **Letta/LiteLLM metadata** (e.g. custom headers or logging hooks) configured outside Den’s request path.
- **Policy:** Den can still enforce **which users may chat with which bears** before forwarding to Letta; model allowlists at the **LiteLLM** layer remain separate (configure both consistently).

---

## 3. Phased roadmap

### Phase 0 – Foundations / prerequisites

**Goal:** Have basic pieces running in isolation.

- Letta running locally or on your infra.
- LiteLLM configured as Letta’s model proxy.
- Open WebUI already talking to Letta (your current state).
- LettaBot installed on Slack and (optionally) wired to Letta directly for experiments (no **Den** yet).

Deliverables:
- Working Letta + LiteLLM stack.
- Working Slack bot talking to *some* **bear** (can be crude).

---

### Phase 1 – **Den**: auth‑aware proxy & **bear** manager (no Cabinet yet)

**Implementation plan:** [PHASE1_BOOTSTRAP.md](PHASE1_BOOTSTRAP.md) — Den (Rust) lives in repo-root **`den/`**; **Trestle** is only an ephemeral bootstrap codename for M0 (not a repo path).

**Goal:** Move **web chat** from “Open WebUI → Letta” to “Open WebUI → **Den** → Letta”, with identity and policy in Den. **LettaBot → Den → Letta is out of scope for this release** (see [optional LettaBot proxy](#den-as-lettabot--letta-proxy-optional-value-add-not-a-v1-feature)).

**Delivery priority:** Ship a **Den-hosted operator console** (browser) **early** so the **first user-testable moment** is “operator provisions users, auth, and bears (Letta + LettaBot yaml) without API gymnastics.” Open WebUI / Loquix chat follow as soon as that control plane is usable — see [PHASE1_BOOTSTRAP.md](PHASE1_BOOTSTRAP.md) milestones **M4b**, **M5**.

**Capabilities to implement:**

1. **Identity and user mapping** (v1: **web-first**)
   - Minimal user model: `user_id` + **`webui_account_id → user_id`** (or equivalent for Open WebUI).
   - **Slack/WhatsApp mappings** in Den are for when (if) you add the optional LettaBot→Den path—not required for v1.
   - Simple auth for web: shared secret, basic login, or OAuth.

2. **Bear registry and membership (many‑to‑many)**
   - A DB (or config that Den imports) holding at least:
     - **Bears:** `agents[bear_id]` (name may stay `agent_id` in APIs) `= { name, description, associated_letta_id, tools_enabled, default_model, … }`.
     - **Membership:** which `user_id`s may use which `bear_id` (and optional roles). A user has many bears; a bear can have many users.
   - **Den’s** job:
     - **Provision** bears in Letta (create/update agents) when admins or workflows add or change a bear.
     - **Publish** bear lists to Open WebUI and regenerate **LettaBot** config snippets / `lettabot.yaml` as needed so each channel exposes the right bears.
     - Validate `agent_id` / bear id on every request; deny if the user is not a member.

3. **Chat proxy API** (on Den)
   - `POST /chat/send`:
     - Accepts message, auth token, optional `agent_id`.
     - Resolves `user_id`.
     - Applies rate limits / policies.
     - Calls `invoke_agent` on Letta.
     - Streams response back.

4. **Web UIs → Den** (v1 release targets)
   - **Operator console (priority):** Den serves a browser UI for **user** accounts, **operator auth**, **bear** CRUD and **Letta provision**, **membership**, and **LettaBot** `lettabot.yaml` preview/download (see [PHASE1_BOOTSTRAP.md](PHASE1_BOOTSTRAP.md)).
   - **Open WebUI:** configure to talk to Den (`/chat/send` or adapter): auth, **bear** picker (only member bears), streaming.
   - **Den native chat (Loquix):** optional **end-user** chat page (e.g. `/app` or `/chat`) using Loquix — **after** the operator console and chat API are stable ([Loquix repo](https://github.com/loquix-dev/loquix)).
   - **LettaBot:** keep **direct to Letta** for v1 chat; Den still **updates LettaBot config** so Slack/WhatsApp show the correct bears per allowlists; optional Den proxy for chat later (see optional proxy section above).

5. **LiteLLM observability** (Den reads, does not proxy)
   - Letta → LiteLLM stays direct. **Den** connects to LiteLLM **only** for observability (metrics/spend/logs APIs or log shipping) as needed.
   - Where possible, align Letta/LiteLLM logging with Den’s identity data for attribution.

**Phase 1 success (v1):**

- **Operator console:** provision users, bears (Letta agents), membership, and LettaBot yaml from the browser.
- Web users chat **via Den** → Letta (**Open WebUI** and/or **Den’s Loquix page**); Den resolves `user_id`, enforces **bear** membership, streams replies.
- **Slack/WhatsApp** may still use LettaBot → Letta direct for **messages**; Den still drives **which bears** exist and appear in bot config. No requirement that chat hits Den until you adopt the optional proxy.
- Bear registry, **users↔bears** membership, and basic RBAC for **web** users.
- No Cabinet/Outline yet: **Letta native memory** only; shared knowledge in later phases.

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
   - **LiteLLM** metrics/spend + **Den** logs + optional correlation (Letta does not route LLM traffic through Den).
   - Dashboards: channel usage (Den), model cost (LiteLLM), per‑**bear** usage.

This is the “make it livable and reliable” phase.

---

## Summary

**Knowledge:** **Letta memory** is per‑**bear** (per Letta agent) context. **Cabinet (Outline)** is the shared knowledgebase for humans and bears.

**Bears:** Each **agent** in the product sense is a **bear**. **Users ↔ bears** is **many‑to‑many**.

We’re aiming for:

- **Den** as the **BEARS control plane and gateway**:
  - Maps external identities → internal users; **provisions bears** in Letta; **bear registry** and **users↔bears** membership; **surfaces bears** in Open WebUI and LettaBot config; chat routing to **Letta** (Letta → **LiteLLM** direct for models).
  - Auth and tool/model policies; per‑user and per‑bear **Cabinet** permissions when Cabinet ships; **Cabinet API** backed by Outline.
  - **LiteLLM:** Den uses it **only for observability** (not as a proxy for model traffic).
  - **v1:** Open WebUI → Den → Letta; **optional** Den-hosted **Loquix** UI → same Den APIs → Letta. **LettaBot** may stay **direct-to-Letta** for chat while Den still **owns bear provisioning and bot/UI exposure**. **LettaBot → Den** for messages is an optional later value-add (see § Den as LettaBot proxy). Cabinet/Outline auth aligned with human auth when Cabinet ships.

- **Phased delivery**:
  - **MVP (Phase 1):** Den for **web**; bear lifecycle + membership; LettaBot may stay direct-to-Letta for **traffic**; no Cabinet yet.
  - **Phase 2:** Cabinet abstraction defined and wired as tools (even if stubbed).
  - **Phase 3:** Cabinet backed by Outline with properties + embeddings.
  - **Phase 4:** Refine memory policies, multi‑user ergonomics, RBAC, and workflows.
