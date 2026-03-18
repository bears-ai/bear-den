# Multi-User Letta Proxy Architecture

Generated with Ezra on the Letta discord: https://discord.com/channels/1161736243340640419/1467667826730078386

## Overview

This document describes the architecture for a multi-user, multi-agent system built on top of Letta Cloud. The system serves N users with N-to-N relationships to Letta agents, where each agent is aware of the user it serves and can restrict its use of memory and tools accordingly.

The architecture follows **Pattern A: one agent per user**, provisioned via Letta Templates. A thin authentication proxy sits between end users and the Letta API, handling user identity, routing, and access control. End users interact through **OpenWebUI** (web) or **LettaBot** on messaging channels—primarily **Slack** and **WhatsApp** in the BEARS plan (Telegram/Discord are also supported by LettaBot where configured).

### Implementation Language

The proxy is implemented in **Rust using Axum**. There is no official Letta Rust SDK. Instead, call the Letta REST API directly using reqwest. The Letta API publishes an OpenAPI spec (https://api.letta.com/openapi.json) which can be used with openapi-generator or progenitor to auto-generate typed Rust client structs.

Streaming responses from Letta (conversations and agent messages) use SSE (server-sent events). Use reqwest-eventsource or eventsource-stream crates for consuming these streams and forwarding to frontends.

All SDK examples in this document are shown in Python/TypeScript for readability. The REST endpoints and request/response shapes are identical -- translate to reqwest calls with the same paths, headers, and JSON bodies.

---

## Letta Concepts Used

### Identities

A first-class Letta API object representing an end user. Each Identity has:

- **identifier_key:** your application's unique user ID (UUID, email, etc.)
- **identity_type:** "user" for individual users, "org" for organizations
- **name:** human-readable display name

Identities link users to agents and enable multi-user isolation.

- **API:** `POST /v1/identities/`
- **SDK:** `client.identities.create(identifier_key=..., identity_type="user", name=...)`

### Templates (Cloud-only)

Blueprints for stamping out agents. Define the agent's system prompt, memory block structure, tools, and model once in the ADE, then create agents programmatically from that template.

- **API:** `POST /v1/templates/{template_version}/agents`
- **SDK:** `client.templates.agents.create(template_version=..., identity_ids=[...], memory_variables={...})`

Templates support `{{variable}}` placeholders in memory blocks that get filled at agent creation time. Use this to inject user-specific data (name, role, preferences) into the agent's initial memory.

### Conversations

Independent message threads within a single agent. Each conversation has its own context window and message history. All conversations on the same agent share memory blocks and searchable message history.

Key behaviors:

- `agents.messages.create()` is NOT thread-safe for concurrent requests to the same agent
- Separate conversations ARE safe for concurrent access
- `conversations.messages.create()` always returns a stream
- `agents.messages.create()` defaults to streaming; pass `streaming=False` for a LettaResponse

Use conversations to isolate sessions (e.g., different chat channels, different UI sessions for the same user).

**SDK:**

```python
# Create
conversation = client.conversations.create(agent_id="agent-xxx")

# Send message (always streams)
stream = client.conversations.messages.create(
    conversation.id,
    messages=[{"role": "user", "content": "Hello"}],
    stream_tokens=True,
)
```

### Memory Blocks

Persistent text segments pinned to the agent's context window. The agent reads and writes these using built-in memory tools.

Relevant block patterns:

- **human block** (per-agent): stores info about the user this agent serves. Agent updates this as it learns.
- **persona block** (per-agent): defines the agent's identity and behavioral constraints.
- **Shared blocks** (read-only): org-wide policies, knowledge bases, tool usage guidelines. Attach to all agents. Set `read_only=True` to prevent agent modification.

Shared blocks are created once and attached to multiple agents:

```python
block = client.blocks.create(
    label="org_policy",
    value="Organization policies and guidelines...",
    limit=4000,
)
# Attach to an agent (repeat for each agent)
client.agents.blocks.attach(agent_id=agent_id, block_id=block.id)
```


---

## System Architecture

```
+------------------+     +------------------+
|    OpenWebUI      |     |     LettaBot      |
|  (chat frontend)  |     |(Slack, WhatsApp) |
+--------+---------+     +--------+---------+
         |                         |
         v                         v
+------------------------------------------------+
|            Authentication Proxy                 |
|                                                 |
|  - User auth (OAuth / session / API key)        |
|  - Maps user -> Letta agent_id                  |
|  - Maps session -> conversation_id              |
|  - Enforces access control                      |
|  - Proxies to Letta Cloud API                   |
+------------------------+-----------------------+
                         |
                         v
+------------------------------------------------+
|              Letta Cloud API                    |
|         https://api.letta.com                   |
|                                                 |
|  Agents  |  Identities  |  Conversations       |
|  Blocks  |  Templates   |  Tools               |
+------------------------------------------------+
```

### Cabinet (Outline) — shared knowledgebase

**Cabinet** is the BEARS name for long-lived knowledge that **both humans and agents** use. It is implemented on **Outline** (wiki-style UI, search, properties). Agents access it through **Den** (see [PLAN.md](PLAN.md)); people edit the same documents in Outline.

- **Does not replace** Letta’s native memory (memory blocks, conversations, built-in memory tools).
When designing agent tools, use **Cabinet** for durable reference content; keep Letta blocks for per-user / per-agent context the agent updates during chat.

### Authentication Proxy Requirements

The proxy is a stateless HTTP service that:

1. **Authenticates end users.** Choose your auth method (OAuth, magic link, API keys, Supabase Auth, etc.). The proxy validates credentials before forwarding any request to Letta.

2. **Maintains a user-to-agent mapping.** Store in your database:

   ```sql
   users table:
     id              UUID (PK)
     email           TEXT
     name            TEXT
     letta_identity_id  TEXT  -- from client.identities.create()
     created_at      TIMESTAMP

   user_agents table:
     user_id         UUID (FK -> users.id)
     agent_id        TEXT  -- Letta agent ID
     agent_name      TEXT  -- human-readable
     created_at      TIMESTAMP
   ```

3. **Routes requests to the correct agent.** When a user sends a chat message:
   - Authenticate the user
   - Look up their agent_id from the mapping table
   - Look up or create a conversation_id for their current session
   - Forward to `POST /v1/conversations/{conversation_id}/messages`
   - Stream the response back to the caller

4. **Prevents cross-user access.** The proxy MUST verify that the requested agent_id belongs to the authenticated user before forwarding any request. The Letta API key is org-level and grants access to all agents.

5. **Handles agent provisioning** (and **channel identity mapping**). The proxy—**Den** in [PLAN.md](PLAN.md)—must tie every chat surface to the same internal `user_id` → Letta identity → agent model. **Slack** and **WhatsApp** do not use email/password signup the same way as web; treat each channel explicitly.

   #### Web (OpenWebUI)

   Classic flow: user signs up or logs in; you create or load `users` row, then provision Letta:

   ```python
   # 1. Create Letta Identity
   identity = client.identities.create(
       identifier_key=f"user{user.id}",
       identity_type="user",
       name=user.name,
   )

   # 2. Create agent from template
   result = client.templates.agents.create(
       template_version="your-project/your-template:latest",
       identity_ids=[identity.id],
       memory_variables={
           "user_name": user.name,
           "user_email": user.email,
       },
   )
   agent_id = result.agent_ids[0]

   # 3. Attach shared org blocks
   for block_id in shared_block_ids:
       client.agents.blocks.attach(agent_id=agent_id, block_id=block_id)

   # 4. Store mapping in your database
   db.insert("user_agents", user_id=user.id, agent_id=agent_id)
   ```

   #### Slack

   - **External identity:** Slack `user_id` (e.g. `U0123…`) per workspace; include `team_id` if you serve multiple workspaces.
   - **Provisioning options:** (a) **Lazy:** on first message, LettaBot forwards Slack user + team to Den; Den looks up `slack_user_id` → `user_id`, creates user + Letta identity + agent if missing, then serves `/chat/message`. (b) **Admin-invite:** map Slack users to accounts in advance (CSV, admin UI, or Slack directory sync).
   - **LettaBot:** Typically one LettaBot agent entry per human user with `allowedUsers: [U_…]` so DMs route to the right agent; config can be **generated from Den’s DB** on deploy (same as `lettabot.yaml` pattern below).
   - **Shared channels:** Risk of multiple Slack users talking to one bot endpoint—enforce allowlists, thread/DM-only policies, or Den-side checks that `channel_user_id` matches the mapped user before calling Letta.
   - **Conversation mapping:** LettaBot already isolates Slack DM vs thread vs channel; Den should map `(slack_user_id, conversation_key)` → Letta `conversation_id` consistently.

   #### WhatsApp

   - **External identity:** WhatsApp **phone number** (E.164) or platform user id from the WhatsApp Business / Cloud API; stable per customer.
   - **Provisioning:** Same pattern as Slack—**lazy on first inbound message** (LettaBot → Den with `whatsapp_number` / wa id) or pre-provisioned allowlist for known numbers.
   - **Privacy:** Phone numbers are PII; store hashed or tokenized in logs; restrict which agents/tools can echo them back.
   - **LettaBot:** Configure WhatsApp channel in `lettabot.yaml` with per-user agent rows and allowlists analogous to Slack (see below).
   - **Session model:** One WhatsApp chat thread ↔ one Letta conversation; multi-device same number should map to the same internal `user_id`.

   #### Shared tables (conceptual)

   Extend your schema so Den can resolve any channel to `user_id`:

   ```sql
   -- Example: external_identities
   user_id UUID REFERENCES users(id),
   channel TEXT NOT NULL,  -- 'slack' | 'whatsapp' | 'webui'
   external_id TEXT NOT NULL,  -- Slack U… or E.164 phone
   metadata JSONB,            -- e.g. { "team_id": "T…" } for Slack
   UNIQUE (channel, external_id)
   ```

   All **Slack** and **WhatsApp** traffic to Letta should go **through Den** (`POST /chat/message` or equivalent) so the same policy, agent registry, and LiteLLM tagging apply as for OpenWebUI.

### API Surface

The proxy should expose these endpoints (at minimum):

| Endpoint | Method | Description |
|----------|--------|-------------|
| /auth/login | POST | Authenticate user, return session token |
| /auth/signup | POST | Create user, provision Letta identity + agent |
| /chat/message | POST | Send message to user's agent (streaming) |
| /chat/conversations | GET | List user's conversations |
| /chat/conversations | POST | Create new conversation |
| /agents | GET | List user's agents |
| /admin/users | GET | List all users (admin only) |
| /admin/users/:id/agents | GET | List agents for a user (admin only) |
| /admin/agents/:id | GET | Agent details (admin only) |
| /admin/agents/:id/blocks | GET | View agent memory blocks (admin only) |
| /admin/agents/:id/blocks/:label | PATCH | Update a memory block (admin only) |

### Admin UI Requirements

The admin UI provides management without requiring direct Letta ADE access:

- **User management:** List users, view their linked agents, manually provision or deprovision agents
- **Agent overview:** View agent status, model, memory block contents, conversation count
- **Memory inspection:** Read and edit agent memory blocks (especially the `human` block for corrections)
- **Shared block management:** Create, edit, and view which agents share a given block
- **Conversation viewer:** Browse message history for any agent/conversation (for debugging and oversight)
- **Template management:** Link to ADE for template editing; display current template version in use

---

## LettaBot Integration (Slack & WhatsApp)

**Target flow:** LettaBot receives messages on **Slack** and/or **WhatsApp**, identifies the sender, and calls **Den** (not Letta directly) with `channel`, `channel_user_id` (Slack `U…` or WhatsApp id/phone), and message body. Den resolves identity, picks the Letta agent, and returns the streamed reply to LettaBot for delivery on the same channel.

Until Den is deployed, LettaBot can talk to Letta directly for experiments; production should match the diagram above (LettaBot → Den → Letta Cloud).

Use a **single** LettaBot process with multiple logical agents via the `agents:` array in `lettabot.yaml` (regenerate from Den’s DB when users are added):

```yaml
server:
  letta:
    apiKey: ${LETTA_API_KEY}
    baseUrl: https://api.letta.com  # or Den URL if LettaBot is adapted to call Den

agents:
  - name: user-alice
    agentId: agent-xxx
    channels:
      slack:
        allowedUsers: ["U_ALICE_SLACK_ID"]
      whatsapp:
        allowedUsers: ["+15551234567"]   # E.164; exact shape depends on LettaBot version

  - name: user-bob
    agentId: agent-yyy
    channels:
      slack:
        allowedUsers: ["U_BOB_SLACK_ID"]
      whatsapp:
        allowedUsers: ["+447700900123"]
```

*(Telegram/Discord blocks work the same way if you enable those channels.)*

Key LettaBot notes for **Slack** and **WhatsApp**:

- **Single process, multiple agents.** One LettaBot, many `agents:` entries—do not run one process per user.
- **DM / allowlist.** Use per-user allowlists so Alice’s Slack id and Alice’s WhatsApp number both route to Alice’s agent only.
- **Per-user rate limits.** For shared entry points, use `dailyUserLimit` where LettaBot supports it.
- **Conversation isolation.** LettaBot creates per-channel / per-thread / per-chat conversations; Slack DMs and WhatsApp 1:1 threads stay isolated.
- **Slack:** In shared channels, multiple users may @ the bot—combine allowlists with Den-side checks so only the mapped user’s agent runs (or use DM-only for strict 1:1).
- **WhatsApp:** Business API constraints (24h session windows, template messages) affect how you reply; Den/LettaBot should respect Meta’s rules for outbound messages.
- **Config restart.** Changes to `lettabot.yaml` require restart; **generate YAML from Den** when provisioning Slack/WhatsApp users.

---

## OpenWebUI Integration

OpenWebUI should be configured to talk to your proxy, not directly to Letta:

- Point OpenWebUI's API base URL at your proxy
- The proxy handles auth + routing transparently
- If OpenWebUI supports OpenAI-compatible chat completions, you can expose a `/v1/chat/completions` endpoint on your proxy that translates to Letta conversation messages
- Alternatively, LettaBot exposes an OpenAI-compatible API on port 8080 (`GET /v1/models`, `POST /v1/chat/completions`) -- but this is per-LettaBot-instance and does not provide per-user routing

---

## Deployment Requirements

### Infrastructure

| Component | Requirement | Notes |
|-----------|-------------|-------|
| Letta Cloud | API key (org-level) | All agent state lives here. BYOK supported for model calls. |
| Proxy service | Rust (Axum) stateless HTTP server | Needs persistent DB connection (sqlx or diesel). |
| Database | PostgreSQL (recommended) | Stores user accounts, user-agent mappings, session tokens. Supabase works well here. |
| LettaBot | Single process, persistent volume | lettabot-agent.json must persist across restarts. |
| OpenWebUI | Standard deployment | Configured to point at proxy. |

### Environment Variables

**Proxy service:**

```
LETTA_API_KEY=          # Org-level Letta Cloud API key
LETTA_BASE_URL=https://api.letta.com
LETTA_TEMPLATE_VERSION= # e.g. your-project/your-template:latest
DATABASE_URL=           # PostgreSQL connection string
SESSION_SECRET=         # For signing session tokens
```

**LettaBot:**

```
LETTA_API_KEY=          # Same org-level key
LETTA_BASE_URL=https://api.letta.com
LETTABOT_CONFIG=/path/to/lettabot.yaml
# Channel tokens (Slack + WhatsApp are first-class in BEARS):
SLACK_BOT_TOKEN=
WHATSAPP_ACCESS_TOKEN=   # WhatsApp Cloud API
WHATSAPP_PHONE_NUMBER_ID=
WHATSAPP_APP_SECRET=     # webhook verification
# Optional: TELEGRAM_BOT_TOKEN=, DISCORD_BOT_TOKEN=
```


### Letta Cloud Setup Checklist

- Create an API key at app.letta.com/api-keys
- Design your agent template in the ADE:
  - System prompt with behavioral constraints and user-awareness instructions
  - human block with `{{user_name}}`, `{{user_email}}` variables
  - persona block defining the agent's role
  - Attach all tools the agent needs
  - Save and version the template
- Create shared read-only blocks for org-wide policies
- Verify BYOK configuration if using your own model API keys

### Scaling Considerations

- **Agent count:** Letta Cloud handles agent storage. No local scaling concern.
- **Proxy concurrency:** Each user message creates one Letta API call. Size your proxy for expected concurrent users. Letta conversations are safe for parallel access across different conversations.
- **LettaBot agent limit:** Not documented as hard-capped, but each agent in the `agents:` array adds config overhead. For 50+ users, generating and restarting with a large config is viable but test the startup time.
- **Memory block limits:** Default block limit is configurable (not fixed at 5k). 5-15 core blocks per agent is practical. If you need more structured per-user data, use archival memory (searchable, out-of-context storage).
- **Model costs:** Agents use the model configured in the template. BYOK avoids Letta request charges. Without BYOK, each message is a request against your plan quota.

### Security Notes

- The Letta API key grants full access to all agents in your org. Never expose it to end users. The proxy is your security boundary.
- Store the API key in environment variables or a secrets manager, never in client-side code.
- The proxy must validate every request: authenticated user owns the requested agent.
- Consider rate limiting at the proxy layer to prevent abuse.

---

## Template Design Guidance

Your agent template should instruct the agent about its role in a multi-user system. Example system prompt structure:

```
You are a personal assistant for {{user_name}}.

You have memory blocks that contain information about the user you serve
and the organization's policies. Your `human` block contains what you know
about your user. Your `org_policy` block contains read-only organizational
guidelines.

IMPORTANT CONSTRAINTS:
- You serve exactly one user. Do not share information across users.
- Follow the policies in your org_policy block.
- Use your tools only for the benefit of your assigned user.
- If you learn new information about your user, update your human memory block.
```

Memory block template:

- **human block** (template variable: `{{user_name}}`, `{{user_email}}`):
  ```
  "The user's name is {{user_name}}. Their email is {{user_email}}.
   I don't know much about them yet."
  ```
- **persona block:**
  ```
  "I am a personal assistant. I help my user with [your domain].
   I maintain strict boundaries and only serve my assigned user."
  ```

---

## Summary of Responsibilities

| Layer | Responsibility |
|-------|----------------|
| Letta Cloud | Agent state, memory, conversations, model inference, tool execution |
| Auth Proxy | User auth, user-to-agent routing, access control, admin UI, agent provisioning |
| LettaBot | **Slack & WhatsApp** (and optional TG/Discord): channel adapter; forwards to Den; conversation isolation |
| OpenWebUI | Web chat frontend, talks to proxy |
| Your Database | User accounts, user-agent mappings, sessions, admin metadata |
