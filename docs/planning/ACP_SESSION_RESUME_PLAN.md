# ACP session load/resume plan

This document captures the implementation plan for making BEARS ACP sessions load and resume canonical BEARS/Letta conversations instead of treating ACP sessions as basic prompt-only transient sessions.

See also:

- [`../architecture/BEAR_CHANNEL_AND_ACP.md`](../architecture/BEAR_CHANNEL_AND_ACP.md)
- [`BEAR_CHANNEL_PLANS.md`](BEAR_CHANNEL_PLANS.md)
- [`ACP_CLIENT_TOOL_RELAY_PLAN.md`](ACP_CLIENT_TOOL_RELAY_PLAN.md)

## Summary

Den and Codepool already have most of the lower-level conversation mechanics needed for session resume:

- Codepool `bear_channel` accepts a `conversation_id`.
- Codepool maps `default` to the bear's main thread.
- Codepool maps pending `new-...` ids to Letta Code `createSession`.
- Codepool maps saved `conv-...` ids to Letta Code `resumeSession`.
- Codepool emits `conversation_resolved` when Letta Code resolves a pending/new session to a canonical conversation id.
- Den web chat already lists, loads history for, and sends messages to `default`, `new-...`, and `conv-...` conversations.

The ACP path is not complete yet:

- The adapter advertises `loadSession: false`.
- The adapter does not implement ACP `session/load`.
- The adapter currently does not persist `conversation_resolved` into its local ACP session context.
- Den's ACP prompt route currently accepts `conversation_id` in the request body but logs and ignores non-default client-supplied ids.
- Den creates/stores ACP session bindings and stores `resolved_conversation_id`, but the ACP prompt route does not yet use the resolved id on later prompts.

The immediate goal is to make ACP-created sessions stable across Codepool warm-pool eviction/restart, then expose explicit load/resume to ACP clients.

## Current implementation facts

### Codepool runtime layer

Codepool already routes by `conversation_id` at the `bear_channel` boundary:

- `default` resumes the agent default thread.
- `new-...` creates a new Letta Code session.
- `conv-...` resumes a saved Letta conversation.

Relevant files:

- `services/codepool/src/bear-channel.ts`
- `services/codepool/src/pool.ts`
- `services/codepool/src/server.ts`

### Den web chat path

Den web chat already exposes conversation list/history/send semantics over session-cookie auth:

- `GET /v1/chat/conversations`
- `GET /v1/chat/history`
- `POST /v1/chat/send`
- `PATCH /v1/chat/conversations/{conversation_id}`

It validates `conversation_id` values as `default`, saved `conv-...`, or pending `new-...`, and forwards them to Codepool.

Relevant file:

- `services/den/src/web/v1/mod.rs`

### Den ACP gateway path

The Den ACP gateway currently has only the basic prompt route plus tool-result and close helpers:

- `POST /acp/bears/{slug}/sessions/{session_id}/prompt`
- `POST /acp/bears/{slug}/sessions/{session_id}/tool-results/{call_id}`
- `POST /acp/bears/{slug}/sessions/{session_id}/close`
- `GET /acp/bears/{slug}/auth-check`

The prompt request body has an optional `conversation_id`, but current code ignores non-default client-supplied ids and generates a pending ACP id of the form `new-acp-{client}-{stable_session_suffix(session_id)}`.

Den persists ACP session bindings in `acp_sessions`, including:

- ACP `session_id`
- Codepool `session_id`
- requested/generated `conversation_id`
- `resolved_conversation_id`
- client name and cwd

However, later ACP prompts do not yet select `resolved_conversation_id` when present.

Relevant files:

- `services/den/src/api/acp.rs`
- `services/den/src/core/acp_sessions.rs`
- `services/den/migrations/20260430121000_acp_sessions.up.sql`

### Adapter path

The local adapter currently implements basic chat only:

- `initialize`
- `session/new`
- `session/prompt`
- `session/close`

It advertises `loadSession: false`, hardcodes prompt `conversation_id` to `default`, and ignores Den `conversation_resolved` events.

Relevant files:

- `tools/bears-acp-adapter/src/main.rs`
- `tools/bears-acp-adapter/README.md`

## Target behavior

### Basic ACP new-session behavior

When an ACP client starts a new session and sends its first prompt without a selected conversation id:

1. Adapter creates an ACP `sessionId`.
2. Adapter sends no explicit conversation id, or sends a pending ACP `new-...` id.
3. Den creates/upserts an ACP session binding.
4. Den routes to Codepool with a pending `new-acp-...` conversation id.
5. Codepool calls Letta Code `createSession`.
6. Codepool emits `conversation_resolved` with the canonical `conv-...` id.
7. Den stores that id in `acp_sessions.resolved_conversation_id`.
8. Den forwards `conversation_resolved` to the adapter.
9. Adapter stores the resolved id in its local session context.
10. Future prompts route with the resolved `conv-...` id.

This must keep working even if Codepool evicts the warm session or restarts.

### Explicit ACP load/resume behavior

When an ACP client loads a saved BEARS conversation:

1. Adapter receives ACP `session/load` with a selected saved session/conversation identifier.
2. Adapter binds a new local ACP `sessionId` to the selected BEARS `conversation_id`.
3. Adapter sends subsequent prompts with that `conversation_id`.
4. Den validates the id and confirms the user/token can access the bear and conversation.
5. Den routes to Codepool with the selected `conv-...` id.
6. Codepool calls Letta Code `resumeSession(conv-...)`.

### Conversation id rules

Accepted conversation ids:

- `default`: the bear's main thread.
- `new-...`: a temporary client/Den pending conversation id that Codepool may create.
- `conv-...`: a canonical saved Letta/BEARS conversation id.

Rejected ids:

- empty ids after normalization except as equivalent to `default` where the route explicitly allows omission;
- ids that do not start with `default`, `new-`, or `conv-` as appropriate;
- ids containing characters outside ASCII alphanumeric, dash, and underscore;
- `conv-...` ids that do not belong to the requested bear's Letta agent.

## Implementation plan

## Slice 1: fix Den ACP conversation routing

Goal: make Den ACP route prompts to the correct existing/resolved/explicit conversation id.

Tasks:

1. Add or reuse a Den API-safe conversation id normalizer for ACP.
   - Match web semantics: `default`, `new-...`, or `conv-...`.
   - Return `400` for invalid ids.
2. In `prompt_inner`, authenticate and authorize as today, then load any existing `acp_sessions` binding for `(user_id, bear_id, acp_session_id)`.
3. Select the `conversation_id` in this priority order:
   - explicit valid request `conversation_id`, if provided;
   - existing `resolved_conversation_id`, if present;
   - existing stored `conversation_id`, if present and no resolved id exists;
   - generated pending `new-acp-{client}-{stable_session_suffix(session_id)}`.
4. For explicit `conv-...`, verify the conversation belongs to the bear's `letta_agent_id` before routing.
   - A practical first implementation can load the bear's active/all conversations via `load_agent_conversations` and check membership.
   - A later optimization can add a direct Letta conversation lookup if available.
5. Upsert the ACP session with the selected `conversation_id`.
6. Preserve `resolved_conversation_id` when the selected id is still the same pending id.
7. Update logs to report requested id, selected id, and selection source instead of saying loadSession is unsupported.
8. Update close semantics to archive the resolved conversation id when available, as it mostly does today.

Acceptance criteria:

- A second prompt for the same ACP session uses the previously resolved `conv-...` id.
- A prompt with explicit `conv-...` routes to that conversation.
- Invalid conversation ids are rejected.
- A `conv-...` not owned by the selected bear is rejected.

## Slice 2: adapter resolved-id tracking

Goal: make basic ACP sessions stable without requiring full `session/load` support first.

Tasks:

1. Extend adapter `SessionContext` with:
   - `conversation_id: Option<String>`
   - `resolved_conversation_id: Option<String>`
2. Handle Den event `type = "conversation_resolved"` in `handle_den_event`.
3. Store the resolved `conv-...` id in the context for the ACP `sessionId`.
4. Include the best known conversation id in future Den prompt requests:
   - `resolved_conversation_id` first;
   - then selected/local `conversation_id`;
   - otherwise omit it and let Den select/generate.
5. Change version/help wording from `conversation id mode: default` to something accurate, for example `ACP conversations: bound/resolved when available; loadSession disabled`.

Acceptance criteria:

- First prompt can start a new ACP conversation.
- Adapter observes and stores `conversation_resolved`.
- Second prompt includes the resolved `conv-...` id.

## Slice 3: Den ACP conversation list/history endpoints

Goal: provide an API-token-authenticated surface for adapters to discover and present resumable conversations.

Proposed routes:

- `GET /acp/bears/{slug}/conversations`
- `GET /acp/bears/{slug}/conversations/{conversation_id}/history`

Tasks:

1. Authenticate with ACP token or bearer principal using the same bear-scoped authorization model as prompt.
2. List conversations for the bear's `letta_agent_id` using `load_agent_conversations`.
3. Return adapter-friendly rows:
   - `id`
   - `title`
   - `last_message_at`
   - `archived`
4. For history, reuse Letta `list_conversation_messages` and map the response into ACP-friendly messages if needed.
5. Decide whether archived rows are hidden by default with an `include_archived=true` query flag.

Acceptance criteria:

- An ACP token holder can list only conversations for bears they are authorized to use.
- The endpoint does not require web session cookies.
- Conversation ids returned by list can be passed back to prompt/load.

## Slice 4: implement ACP `session/load` in the adapter

Goal: advertise and implement ACP load/resume once the exact client protocol shape is confirmed.

Tasks:

1. Confirm the current ACP `session/load` request/response schema against Zed/OpenCode.
2. Add adapter request handling for `session/load`.
3. Map the client-selected ACP session/conversation identifier to a BEARS `conversation_id` returned by Den's ACP conversation list endpoint.
4. Create/store an adapter `SessionContext` bound to that `conversation_id`.
5. Return the ACP session object expected by the client.
6. Set `agentCapabilities.loadSession = true` only after the implementation works end-to-end.
7. Update adapter README and configuration docs.

Acceptance criteria:

- ACP clients that support load session can select an existing BEARS conversation.
- Subsequent prompts resume that conversation.
- Adapter does not advertise `loadSession` until this works.

## Slice 5: harden Codepool explicit resume behavior

Goal: avoid silent conversation forks for explicit saved conversation ids.

Current Codepool behavior can fall back to `createSession` when a non-default non-pending conversation is missing. That is useful for some recovery flows but risky for explicit `conv-...` resume.

Tasks:

1. Keep `new-...` as the only create-new signal.
2. Keep `default` mapped to the agent default thread.
3. Treat explicit `conv-...` as strict resume.
4. If `resumeSession(conv-...)` fails because the conversation is missing, return a clear error instead of silently creating a new session.
5. If a fallback is still needed for legacy callers, require an explicit request flag such as `create_on_missing` rather than inferring it.

Acceptance criteria:

- Typo/deleted `conv-...` ids do not silently create new conversations.
- Pending `new-...` ids continue to create new conversations.

## Test plan

### Den ACP tests

- Prompt with no conversation id creates/selects a pending `new-acp-...` id.
- `conversation_resolved` updates the ACP session binding.
- Second prompt for the same ACP session routes to the resolved `conv-...` id.
- Prompt with explicit `conv-...` routes to that conversation.
- Prompt with invalid conversation id returns `400`.
- Prompt with a `conv-...` owned by a different bear returns `403` or `404`.
- `session/close` archives the resolved `conv-...`, not only the original pending `new-...`.

### Adapter tests

- `session/new` stores context without a conversation id.
- Den `conversation_resolved` event updates the session context.
- Future prompt payloads include the resolved `conversation_id`.
- `session/load` binds a selected `conversation_id` once implemented.
- `initialize` advertises `loadSession: true` only after `session/load` is implemented.

### Codepool tests

- `default` uses `resumeSession(agentId)`.
- `new-...` uses `createSession(agentId)`.
- `conv-...` uses `resumeSession(convId)`.
- missing `conv-...` produces a clear error and does not create a new conversation.

## Rollout order

1. Den ACP routing fix.
2. Adapter resolved-id tracking.
3. Den ACP conversation list/history endpoints.
4. Adapter `session/load` support and `loadSession: true`.
5. Codepool strict explicit-resume behavior.

This order makes ACP-created sessions durable first, then adds explicit user-facing load/resume support.

## Open questions

- What exact `session/load` request and response shape do current ACP clients expect?
- Should adapter-local session ids be stable across adapter process restarts, or is Den-side binding sufficient?
- Should ACP conversation list include archived conversations by default?
- Should Den expose conversation history through ACP, or should the adapter only need ids/titles for `session/load`?
- Should explicit `default` be allowed in `session/load`, or should clients use `session/new` for the main thread?
