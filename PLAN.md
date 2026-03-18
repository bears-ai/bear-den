Here’s a high‑level, ops‑oriented plan and architecture, with an MVP that works **without Cabinet first**, then adds Cabinet/Outline in stages.

**Terminology**

- **BEARS** — the whole system (stack): Letta, LiteLLM, Den, Outline, frontends, LettaBot, etc.
- **Den** — the **BEARS control plane and gateway**: single orchestration service for identity, routing, policy, Cabinet API, and model-call observability (see below).

I’ll break it into:

1. System architecture (high‑level components and responsibilities)
2. Capability “contracts” (pseudo, not detailed APIs)
3. Project plan: phases and milestones (MVP → Cabinet → polish)

---

## 1. System architecture: components and responsibilities

### Core components

1. **Den** (control plane + gateway)
   - Maps **external identities** (Slack, WhatsApp, web, etc.) to **internal users**.
   - **Agent registry**; **routes** chat to the correct Letta agent.
   - **Auth** and **tool/model policies** (RBAC, gating, rate limits).
   - **Cabinet API** for agents (search/read/write), implemented against **Outline**.
   - **Tags and forwards** all model traffic **through LiteLLM** so logs/costs are attributable (`user_id`, `agent_id`, `channel`, etc.).
   - Auth‑aware proxy: frontends ↔ Letta; agent tool calls ↔ Cabinet.

2. **Letta**
   - Agent runtime (conversation loop + tools).
   - Per‑agent configuration: system prompts, tools, memory adapters.
   - Stateless(ish) from Den’s point of view; Den passes user/agent IDs and context.

3. **LettaBot**
   - Channel adapters:
     - Slack, WhatsApp (others later).
   - For each message:
     - Authenticate/identify external user,
     - Forward to **Den** with identity + channel metadata,
     - Stream responses back.

4. **OpenWebUI (and any other web/CLI frontends)**
   - Authenticates users (ideally via Den or a shared SSO).
   - Forwards chat requests to **Den** instead of directly to Letta.

5. **LiteLLM**
   - Model routing/proxy between Letta and OpenAI (and any other models).
   - Observability hooks (request logs, costs, model usage).
   - Optional: caching / rate‑limiting at the model layer.

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

---

## 2. Capability “contracts” (pseudo, high‑level)

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
  - Output: agents the user is allowed to see/use (for OpenWebUI agent picker).

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

Den doesn’t need to know Letta’s internal details, just that it can be called with a user+agent context. Model calls from Letta run **through LiteLLM** with tags Den supplies (or enforces) for observability.

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

### 2.5 Den ↔ LiteLLM

**Den’s job:** every model call is **tagged** and **routed through LiteLLM** for observability (costs, usage, attribution).

- Den attaches (or requires) metadata: `user_id`, `agent_id`, `channel`, etc., on traffic that reaches LiteLLM—whether by configuring Letta’s outbound calls, proxying, or headers/metadata conventions.
- **Policy alignment:** Den’s model/tool allowlists and rate limits match what LiteLLM enforces or logs.
- LiteLLM remains the single gateway to OpenAI and other providers from BEARS’ perspective.

---

## 3. Project plan: phased MVP → Cabinet → maturity

### Phase 0 – Foundations / prerequisites

**Goal:** Have basic pieces running in isolation.

- Letta running locally or on your infra.
- LiteLLM configured as Letta’s model proxy.
- OpenWebUI already talking to Letta (your current state).
- LettaBot installed on Slack and (optionally) wired to Letta directly for experiments (no **Den** yet).

Deliverables:
- Working Letta + LiteLLM stack.
- Working Slack bot talking to *some* agent (can be crude).

---

### Phase 1 – **Den**: auth‑aware proxy & agent manager (no Cabinet yet)

**Goal:** Move from “frontends → Letta” to “frontends → **Den** → Letta”, and centralize identity/policy.

**Capabilities to implement:**

1. **Identity and user mapping**
   - Minimal user model: `user_id`, plus mappings:
     - `slack_user_id → user_id`
     - `whatsapp_number → user_id`
     - `webui_account_id → user_id` (if you integrate OpenWebUI auth).
   - Simple auth for web/CLI:
     - Could be shared secret, basic login, or OAuth; doesn’t need to be fancy initially.

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

4. **Frontends switched to Den**
   - LettaBot:
     - Instead of calling Letta directly, it calls Den `/chat/send`.
   - OpenWebUI:
     - Configure it to talk to Den:
       - Either treat Den as a “LLM backend with tools”,
       - Or implement a small adapter that maps its requests to `/chat/send`.

5. **LiteLLM observability** (via Den)
   - Den ensures model traffic through LiteLLM carries tags (`user_id`, `agent_id`, `channel`).
   - Validate per‑user/per‑agent token usage and channel‑level traffic in LiteLLM (or Den) logs.

**Phase 1 success:**

- Any user in Slack/WhatsApp/OpenWebUI can talk to an agent **via Den**.
- Den knows who they are (internal `user_id`).
- You can list agents per user and enforce basic access rules, even if it’s just “admins vs normal users.”
- No Cabinet/Outline yet: agents use **Letta’s native memory** (blocks, conversations, etc.); shared knowledge arrives in later phases.

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
- At least one production‑like workflow uses Cabinet in Slack or OpenWebUI.

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
   - Use LiteLLM + **Den** logs to:
     - Monitor per‑user/per‑agent token usage,
     - Alert on spikes or errors.
   - Build minimal dashboards (could be just Grafana over logs) for:
     - Channel usage,
     - Agent performance.

This is the “make it livable and reliable” phase.

---

## Summary

**Knowledge:** **Letta memory** is per-agent context. **Cabinet (Outline)** is the shared knowledgebase for humans and agents.

We’re aiming for:

- **Den** as the **BEARS control plane and gateway**:
  - Maps external identities → internal users; agent registry; chat routing to Letta.
  - Auth and tool/model policies; **Cabinet API** backed by Outline.
  - Tags and forwards model calls through **LiteLLM** for observability.
  - LettaBot and OpenWebUI target Den; Cabinet/Outline auth aligned with human auth.

- **Phased delivery**:
  - **MVP (Phase 1):** Den in front of Letta; multi‑user chat; no Cabinet yet.
  - **Phase 2:** Cabinet abstraction defined and wired as tools (even if stubbed).
  - **Phase 3:** Cabinet backed by Outline with properties + embeddings.
  - **Phase 4:** Refine memory policies, multi‑user ergonomics, RBAC, and workflows.

If you want to go one level deeper next, we can pick one phase (probably Phase 1) and outline concrete tasks and acceptance criteria (e.g., “what needs to work by the end of an MVP weekend”).
