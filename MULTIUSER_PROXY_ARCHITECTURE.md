# Multi-User Letta Proxy Architecture

Generated with Ezra on the Letta discord: https://discord.com/channels/1161736243340640419/1467667826730078386

## Overview

This document describes the architecture for a multi-user, multi-agent system built on top of Letta Cloud. The system serves N users with N-to-N relationships to Letta agents, where each agent is aware of the user it serves and can restrict its use of memory and tools accordingly.

The architecture follows **Pattern A: one agent per user**, provisioned via Letta Templates. A thin authentication proxy sits between end users and the Letta API, handling user identity, routing, and access control. End users interact through either OpenWebUI or LettaBot (Slack, Telegram, Discord, etc.).

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
|  (chat frontend)  |     | (Slack/TG/Discord)|
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

**Cabinet** is the BEARS name for long-lived knowledge that **both humans and agents** use. It is implemented on **Outline** (wiki-style UI, search, properties). Agents access it through **BEARS Core** tool endpoints (see [PLAN.md](PLAN.md)); people edit the same documents in Outline.

- **Does not replace** Letta’s native memory (memory blocks, conversations, built-in memory tools).
- **Obviates** the older **Git Sync + Qdrant + standalone knowledgebase** stack for shared archival knowledge—no need to duplicate that pipeline once Cabinet is live.

When designing agent tools, prefer Cabinet for durable reference content; keep Letta blocks for per-user / per-agent context the agent updates during chat.

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

5. **Handles agent provisioning.** On user signup:

   ```python
   # 1. Create Letta Identity
   identity = client.identities.create(
       identifierkey=f"user{user.id}",
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

## LettaBot Integration

Use a single LettaBot instance configured with multiple agents via the `agents:` array in `lettabot.yaml`:

```yaml
server:
  letta:
    apiKey: ${LETTA_API_KEY}
    baseUrl: https://api.letta.com

agents:
  - name: user-alice
    agentId: agent-xxx  # Alice's Letta agent
    channels:
      slack:
        allowedUsers: ["U_ALICE_SLACK_ID"]
      telegram:
        allowedUsers: [123456789]

  - name: user-bob
    agentId: agent-yyy  # Bob's Letta agent
    channels:
      slack:
        allowedUsers: ["U_BOB_SLACK_ID"]
```

Key LettaBot configuration notes:

- **Single process, multiple agents.** Do NOT run separate LettaBot instances per user. Use the `agents:` array.
- **DM policies.** Use allowlist DM policy to restrict each agent to its designated user.
- **Per-user rate limits.** Use `dailyUserLimit` in group configs if agents share channels.
- **Conversation isolation.** LettaBot already creates per-channel/per-thread conversations. Each Slack DM or Telegram chat gets its own conversation.
- **Slack user awareness limitation.** LettaBot does not natively map Slack user IDs to Letta Identities. The agent knows who it's talking to from its human memory block (set at provisioning), but in shared channels you would need a custom tool to resolve Slack user metadata. For simplicity, use DM-only or allowlisted channels.
- **Config changes require restart.** Adding new agents to `lettabot.yaml` requires restarting the LettaBot process. There is no hot-reload for agent config. Plan for this in your ops workflow.
- **`lettabot.yaml` is the source of truth.** Generate it from your database during deploys. When a new user is provisioned, regenerate the config and restart.

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
# Channel tokens as needed:
SLACK_BOT_TOKEN=
TELEGRAM_BOT_TOKEN=
DISCORD_BOT_TOKEN=
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
| LettaBot | Channel interface (Slack/TG/Discord), conversation isolation, message relay |
| OpenWebUI | Web chat frontend, talks to proxy |
| Your Database | User accounts, user-agent mappings, sessions, admin metadata |
