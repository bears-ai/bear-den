# BEARS plan & architecture

High‑level, ops‑oriented plan and architecture: MVP **without Cabinet first**, then Cabinet/Outline in stages.

**Reading order:** Skim [§1](#1-system-architecture)–[§2](#2-capability-contracts-pseudo) for components and pseudo-contracts; use [§3](#3-phased-roadmap) for phased delivery.

## Table of contents

| Section | Contents |
|---------|----------|
| [§1](#1-system-architecture) | Components, Letta vs Cabinet, optional LettaBot→Den |
| [§2](#2-capability-contracts-pseudo) | Frontends→Den, Den→Letta, Cabinet, Outline, LiteLLM observability |
| [§3](#3-phased-roadmap) | Phase 0–4 milestones |
| [Summary](#summary) | One-page recap |

**Terminology**

- **BEARS** — the whole system (stack): Letta, LiteLLM, Den, Outline, frontends, LettaBot, etc.
- **Den** — the **BEARS control plane and gateway**: single orchestration service for identity, routing, policy, Cabinet API, and model-call observability (see below).

---

## 1. System architecture

### Core components

1. **Den** (control plane + gateway), implemented in **Rust with Axum**
   - Maps **external identities** (Slack, WhatsApp, web, etc.) to **internal users**.
   - **Agent registry**; **routes** chat to the correct Letta agent.
   - **Auth** and **tool/model policies** (RBAC, gating, rate limits).
   - **Cabinet API** for agents (search/read/write), implemented against **Outline**.
   - **LiteLLM (observability only):** Den does **not** proxy model traffic. **Letta calls LiteLLM directly.** Den may connect to LiteLLM for **metrics, spend logs, admin API**, etc., and join that with Den’s `user_id` / `agent_id` / channel data where your logging pipeline allows.
   - Auth‑aware proxy: frontends ↔ **Letta** only for chat (not through LiteLLM); agent tool calls ↔ Cabinet.

2. **Letta** (**self‑hosted only** in BEARS—e.g. Coolify `bears-letta:8283`, not Letta Cloud)
   - Agent runtime (conversation loop + tools).
   - **Model calls:** **Letta → LiteLLM** directly (`LLM_API_URL`). No Den in that path.
   - Per‑agent configuration: system prompts, tools, memory adapters.
   - Stateless(ish) from Den’s point of view; Den calls the **self‑hosted Letta REST API** (reqwest from Axum).

3. **LettaBot**
   - Channel adapters: Slack, WhatsApp (others later).
   - **Initial Den releases:** LettaBot typically talks **directly to Letta** (same as today’s experiments). **Not** required to go through Den for v1.
   - **Optional later:** route LettaBot → **Den** → Letta so messaging channels share Den’s identity and policy with web—see [Den as LettaBot proxy (optional)](#den-as-lettabot--letta-proxy-optional-value-add-not-a-v1-feature) below.

4. **Open WebUI** (and any other web/CLI frontends)
   - Authenticates users (ideally via Den or a shared SSO).
   - Forwards chat requests to **Den** instead of directly to Letta.

5. **LiteLLM**
   - **Letta’s** model gateway (Letta → LiteLLM → providers). Den does not proxy this traffic.
   - Observability (logs, costs, metrics) — **Den** may consume for dashboards/correlation only.

6. **Cabinet (later)**
   - Logical knowledge layer agents use for long‑term reference & history.
   - **API exposed by Den**; storage and human UI on **Outline**.
   - Den enforces identity and policy on every Cabinet operation.

7. **Outline**
   - Human knowledge base UI.
   - Stores docs, properties, versions.
   - Optionally uses its own embeddings for search.

### Knowledge model: Letta memory vs Cabinet

- **Letta’s own memory** (memory blocks, conversations, built-in tools) stays as-is. Cabinet does **not** replace how Letta manages per-agent context, blocks, or the conversation loop.
- **Cabinet** (implemented on **Outline**) is the **shared knowledgebase**: documents that **both humans and agents** can read and edit.

### Den as LettaBot → Letta proxy (optional value-add, **not** a v1 feature)

Routing **LettaBot** through **Den** (instead of LettaBot → Letta direct) is a **potential** enhancement, **not** part of the **initial Den release**.

**Why you might add it later**

| Benefit | Description |
|--------|-------------|
| **Unified identity** | Slack/WhatsApp external ids live in Den next to web users (`user_id`, `external_identities`). |
| **One policy surface** | Same rate limits, agent access, and (with Cabinet) permissions for web and chat apps. |
| **Provisioning** | Lazy onboarding (e.g. first DM) without regenerating `lettabot.yaml` and restarting LettaBot per user. |
| **Audit** | One place to log who used which agent on which channel. |

**v1 scope:** Den’s first shipped role is **Open WebUI (web) → Den → Letta**, plus agent registry, auth, Cabinet API (as phases land), and LiteLLM observability reads. **LettaBot remains direct-to-Letta** until you explicitly choose to put Den in that path.

---

## 2. Capability contracts (pseudo)

Not exact APIs, but what each interface *does*.

### 2.1 Frontends → Den

**Purpose:** send authenticated user messages to the right Letta agent, get responses back.

**Capabilities:**

- `POST /chat/send`
  - Input: `{ user_token, channel, agent_id, message, metadata }`
    - `user_token`: Den or upstream auth token → Den resolves to internal `user_id`.
    - `channel`: `"slack" | "whatsapp" | "webui" | ..."`
    - `agent_id`: which agent to talk to (or default per-channel).
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

- `GET /agents/list`
  - Input: `{ user_token }`
  - Output: agents the user is allowed to see/use (for Open WebUI agent picker).

Later, you can add:

- `GET /usage` (per user, per agent).
- `GET /logs` (for admin/debug).

---

### 2.2 Den → Letta (agent invocation)

**Purpose:** call Letta with clear identity and context; Letta returns messages (and tool calls).

You can think of a single RPC:

- `invoke_agent(user_id, agent_id, message, channel_ctx, session_ctx)`

Where:

- `user_id`: Den’s internal ID (stable across Slack/WhatsApp/web).
- `agent_id`: configuration key in Letta (from Den’s registry).
- `channel_ctx`: `{ channel, channel_user_id, channel_conversation_id }`.
- `session_ctx`: optional (recent messages, conversation ID, etc.) that Den or Letta manages.

Letta returns:

- Model messages,
- Tool calls (e.g., `{"tool": "cabinet.search", ...}`),
- Final responses.

Den doesn’t need to know Letta’s internal details for routing chat. **Inference path:** user → Den → Letta → **LiteLLM** (direct). Den does not touch LiteLLM for model requests.

---

### 2.3 Agents → Den (via Letta tools)

Two notable services:

#### a) Cabinet (later phase)

Pseudo‑contract:

- `cabinet.search(query, filters) -> [doc_summary]`
  - `filters` might include `kind`, `project`, `tags`, etc.
  - Implemented as **Den** endpoints that call Outline search.

- `cabinet.get(doc_id) -> full_doc`
- `cabinet.create(kind, title, body, properties) -> doc_id`
- `cabinet.update(doc_id, body?, properties?) -> doc_id`

Agents never talk directly to Outline; they call these tools, which **Den** implements on top of Outline APIs and policies.

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

- Which decks a given `user_id` and `agent_id` can touch.
- Property schema (kinds, projects, tags, etc.).

---

### 2.5 Den and LiteLLM (observability only)

- **Traffic:** **Letta → LiteLLM** for all completions/embeddings. **Den never proxies** LiteLLM.
- **Den’s use of LiteLLM:** optional **read-only** integration for observability—e.g. LiteLLM **metrics**, **spend tracking**, admin API, or exported logs—so operators (or Den) can monitor cost and usage. Correlating calls to Den’s `user_id` / `agent_id` may require **Letta/LiteLLM metadata** (e.g. custom headers or logging hooks) configured outside Den’s request path.
- **Policy:** Den can still enforce **which users/agents may chat** before forwarding to Letta; model allowlists at the **LiteLLM** layer remain separate (configure both consistently).

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
- Working Slack bot talking to *some* agent (can be crude).

---

### Phase 1 – **Den**: auth‑aware proxy & agent manager (no Cabinet yet)

**Goal:** Move **web chat** from “Open WebUI → Letta” to “Open WebUI → **Den** → Letta”, with identity and policy in Den. **LettaBot → Den → Letta is out of scope for this release** (see [optional LettaBot proxy](#den-as-lettabot--letta-proxy-optional-value-add-not-a-v1-feature)).

**Capabilities to implement:**

1. **Identity and user mapping** (v1: **web-first**)
   - Minimal user model: `user_id` + **`webui_account_id → user_id`** (or equivalent for Open Web UI).
   - **Slack/WhatsApp mappings** in Den are for when (if) you add the optional LettaBot→Den path—not required for v1.
   - Simple auth for web: shared secret, basic login, or OAuth.

2. **Agent registry**
   - A config file or small DB:
     - `agents[agent_id] = { name, description, associated_letta_id, allowed_users_or_roles, tools_enabled, default_model }`.
   - **Den’s** job:
     - Validate `agent_id` requests from frontends.
     - Only allow access based on user permissions.

3. **Chat proxy API** (on Den)
   - `POST /chat/send`:
     - Accepts message, auth token, optional `agent_id`.
     - Resolves `user_id`.
     - Applies rate limits / policies.
     - Calls `invoke_agent` on Letta.
     - Streams response back.

4. **Open WebUI → Den** (v1 release target)
   - Configure Open WebUI to talk to Den (`/chat/send` or adapter): auth, agent picker, streaming.
   - **LettaBot:** keep **direct to Letta** for v1; optional Den front later (see optional proxy section above).

5. **LiteLLM observability** (Den reads, does not proxy)
   - Letta → LiteLLM stays direct. **Den** connects to LiteLLM **only** for observability (metrics/spend/logs APIs or log shipping) as needed.
   - Where possible, align Letta/LiteLLM logging with Den’s identity data for attribution.

**Phase 1 success (v1):**

- Web users (Open WebUI) chat **via Den** → Letta; Den resolves `user_id`, enforces agent access, streams replies.
- **Slack/WhatsApp** may still use LettaBot → Letta direct; no requirement that they hit Den yet.
- Agent registry + basic RBAC for **web** users.
- No Cabinet/Outline yet: **Letta native memory** only; shared knowledge in later phases.

---

### Phase 2 – Introduce Cabinet as an abstract service (Outline still in background)

**Goal:** Define the **Cabinet abstraction** and wire it as Letta tools, while starting with a minimal Outline integration.

**Steps:**

1. **Define Cabinet concepts & schema (on paper/config)**
   - Collections ("Decks"): `knowledge`, `history`, `projects`.
   - Properties:
     - `kind`, `project`, `tags`, `people`, `source`, `status`, etc.
   - Decide which agents can read/write which kinds.

2. **Cabinet API on Den (skeleton)**
   - Implement Cabinet endpoints (for agents):
     - `cabinet.search`, `cabinet.get`, `cabinet.create`, `cabinet.update`.
   - Initially, these can return stub data or use a temporary in‑memory store while you finalize behavior.

3. **Letta tools for Cabinet**
   - Define tools the agents can call:
     - `cabinet_search_tool`
     - `cabinet_read_tool`
     - `cabinet_write_tool`
   - Wire them to **Den’s** Cabinet API.
   - Update one or two agents to:
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

4. **Update agents to use Cabinet for real**
   - Pick one high‑value agent use case:
     - E.g., “Household Brain” in Slack that:
       - Stores important decisions/summaries in `knowledge`,
       - Logs key events in `history`.
   - Verify:
     - Humans can browse/edit these docs in Outline.
     - Agents can find and reference them later.

**Phase 3 success:**

- Cabinet is real: Outline is the store, agents read/write it **via Den**.
- Human and agent access are governed by the same identity/policy layer **in Den**.
- At least one production‑like workflow uses Cabinet in Slack or Open Web UI.

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
       - Some users/agents can write to `knowledge`,
       - Others only to `history` or read‑only.
     - Restrict dangerous or heavy tools/models to specific roles.

4. **Observability & ops polish**
   - **LiteLLM** metrics/spend + **Den** logs + optional correlation (Letta does not route LLM traffic through Den).
   - Dashboards: channel usage (Den), model cost (LiteLLM), agent performance.

This is the “make it livable and reliable” phase.

---

## Summary

**Knowledge:** **Letta memory** is per-agent context. **Cabinet (Outline)** is the shared knowledgebase for humans and agents.

We’re aiming for:

- **Den** as the **BEARS control plane and gateway**:
  - Maps external identities → internal users; agent registry; chat routing to **Letta** (Letta → **LiteLLM** direct for models).
  - Auth and tool/model policies; **Cabinet API** backed by Outline.
  - **LiteLLM:** Den uses it **only for observability** (not as a proxy for model traffic).
  - **v1:** Open WebUI → Den → Letta. **LettaBot → Den** is an optional later value-add (see § Den as LettaBot proxy). Cabinet/Outline auth aligned with human auth when Cabinet ships.

- **Phased delivery**:
  - **MVP (Phase 1):** Den for **web**; LettaBot may stay direct-to-Letta; no Cabinet yet.
  - **Phase 2:** Cabinet abstraction defined and wired as tools (even if stubbed).
  - **Phase 3:** Cabinet backed by Outline with properties + embeddings.
  - **Phase 4:** Refine memory policies, multi‑user ergonomics, RBAC, and workflows.

If you want to go one level deeper next, we can pick one phase (probably Phase 1) and outline concrete tasks and acceptance criteria (e.g., “what needs to work by the end of an MVP weekend”).
