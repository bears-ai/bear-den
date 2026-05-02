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

The ACP path has an MVP session base now, but it is not fully ACP-spec-complete yet:

- The adapter advertises and handles `session/list`, `session/resume`, and `session/load`.
- Den exposes adapter-facing session list/get and conversation history endpoints.
- The adapter tracks Den `conversation_resolved` events and uses loaded/resumed conversation mappings for future prompts.
- Den's ACP prompt route selects explicit, resolved, stored, or generated conversation ids and validates explicit `conv-...` ids against the bear.
- Remaining work is mostly spec hardening: absolute `cwd`, MCP server policy, richer history replay, cancellation/close semantics, stable pagination, and documentation of persisted-vs-local session listing.

The immediate goal is to finish those hardening slices, then revisit tool timeouts/cancellation on top of the stable session base.

## ACP session semantics note

ACP clients commonly treat `session/new` as the "new thread" signal and do not necessarily send a backend-specific conversation id on later prompts. When a prompt includes `conversation_id: "default"`, that can mean "no saved backend session was selected" rather than "append to the bear's canonical default thread".

BEARS should therefore keep the normal adapter pattern: bind each ACP `sessionId` to a backend BEARS/Letta conversation id. A fresh ACP session with no explicit saved `conv-...` should create or reuse a pending `new-acp-...` binding, then store the resolved canonical `conv-...`. Explicit `conv-...` ids are the saved-conversation resume path; explicit selection of the bear's true `default` thread should be reserved for a deliberate load/selection flow, not inferred from generic ACP prompt defaults.

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

The Den ACP gateway now includes prompt/session backing routes plus tool-result and close helpers:

- `GET /acp/bears/{slug}/sessions`
- `GET /acp/bears/{slug}/sessions/{session_id}`
- `POST /acp/bears/{slug}/sessions/{session_id}/prompt`
- `POST /acp/bears/{slug}/sessions/{session_id}/tool-results/{call_id}`
- `POST /acp/bears/{slug}/sessions/{session_id}/close`
- `GET /acp/bears/{slug}/conversations`
- `GET /acp/bears/{slug}/conversations/{conversation_id}/history`
- `GET /acp/bears/{slug}/auth-check`

The prompt request body has an optional `conversation_id`. Den selects the route target from explicit, resolved, stored, or generated pending ids and generates pending ACP ids of the form `new-acp-{client}-{stable_session_suffix(session_id)}` when needed.

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

The local adapter currently implements the session MVP:

- `initialize`
- `session/new`
- `session/list`
- `session/resume`
- `session/load`
- `session/prompt`
- `session/close`

It advertises `loadSession: true` and `sessionCapabilities.list/resume/close`. It tracks Den `conversation_resolved` events, restores Den session mappings on load/resume, and replays text history for `session/load`.

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

## ACP spec alignment hardening

The current session implementation is intentionally ACP-shaped, but these tightenings are required before treating it as fully spec-compliant rather than MVP-compatible.

### Hardening slice A: absolute `cwd` semantics

ACP treats `cwd` as the session's filesystem context. It is required for session setup/load/resume, must be an absolute path, and `session/list` rows expose `cwd` as a required field.

Tasks:

1. In the adapter, validate `cwd` for `session/new`, `session/load`, and `session/resume`.
   - Prefer the explicit `params.cwd` value when present.
   - Continue accepting known client workspace URI/folder fallbacks only when they normalize to an absolute local path.
   - Return a clear JSON-RPC validation error when no absolute `cwd` can be determined, or explicitly gate a compatibility fallback if a client is known to omit it.
2. Persist only valid absolute `cwd` values into Den `acp_sessions` for ACP sessions.
3. Ensure `session/list` never returns an empty `cwd` for ACP-facing rows.
   - If legacy rows have no `cwd`, either omit them from ACP list results, repair them from known context, or return a conservative absolute fallback only when it is truthful.
4. Add Den-side validation for the optional `cwd` filter on `GET /acp/bears/{slug}/sessions`.
   - Reject non-absolute paths instead of treating them as arbitrary strings.
5. Add logging/metrics for rejected or legacy missing-`cwd` sessions so we know which clients need compatibility handling.

Acceptance criteria:

- `session/new`, `session/load`, and `session/resume` have deterministic absolute `cwd` handling.
- `session/list` rows satisfy ACP's `SessionInfo.cwd` requirement.
- Relative, empty, or malformed `cwd` values produce clear errors or documented compatibility behavior.

### Hardening slice B: MCP server handling policy

ACP clients may pass `mcpServers` to `session/new`, `session/load`, and `session/resume`; stdio MCP support is part of the ACP session setup model. The current BEARS ACP adapter ignores these entries and instead exposes BEARS/Den/Codepool tools plus ACP client filesystem bridges.

Tasks:

1. Decide the product stance for ACP-provided `mcpServers`:
   - implement stdio MCP server lifecycle in the adapter; or
   - explicitly reject non-empty `mcpServers` with a clear unsupported-feature error; or
   - accept but ignore them only as a documented temporary compatibility compromise.
2. Do not advertise HTTP/SSE MCP support until implemented; keep `mcpCapabilities.http = false` and `mcpCapabilities.sse = false` unless that changes.
3. Update adapter README and ACP planning docs to state the current MCP behavior.
4. If implementing stdio MCP later, define ownership boundaries:
   - adapter process owns local MCP subprocess lifecycle;
   - Den remains policy/audit authority for BEARS tools;
   - local MCP server credentials must not be persisted in Den.

Acceptance criteria:

- A client that supplies `mcpServers` gets predictable behavior.
- The behavior is documented and reflected in tests.
- We do not imply full ACP MCP support until we actually have it.

### Hardening slice C: history replay completeness

ACP `session/load` requires replaying the conversation to the client through `session/update` notifications before responding. The current implementation replays user and assistant text messages only.

Tasks:

1. Document the current MVP as text-only replay of user/assistant messages.
2. Determine which historical event types are available from Letta/Codepool persistence:
   - tool calls;
   - tool results;
   - reasoning/status chunks;
   - errors;
   - resource/image/audio content;
   - session info, mode, or config updates.
3. Extend Den history APIs, if possible, to expose enough structured data for richer ACP replay.
4. Extend adapter replay mapping to produce ACP-native `session/update` notifications for any supported historical event types.
5. If richer data is unavailable, keep replay explicitly scoped and avoid claiming complete ACP event reconstruction.

Acceptance criteria:

- `session/load` clearly replays all event types we can faithfully reconstruct.
- Unsupported historical event types are documented rather than silently implied.
- Text-only replay remains stable for user/assistant conversation continuity.

### Hardening slice D: cancellation and close semantics

ACP baseline support includes `session/cancel`, and `session/close` must cancel ongoing work as if `session/cancel` had been called before freeing resources. The current implementation records close/archive state but does not yet cancel active Codepool/Letta work.

Tasks:

1. Implement adapter `session/cancel` handling for active prompts.
2. Add Den and/or Codepool cancellation plumbing keyed by ACP session id / Codepool session id / request id.
3. Ensure `session/close` triggers the same cancellation path before marking sessions closed or archived.
4. Define behavior for pending ACP client tool calls when a session is cancelled or closed:
   - mark pending calls cancelled/timed out;
   - stop waiting on client responses;
   - avoid forwarding late duplicate results.
5. Tie this into the planned tool timeout/cancellation work after the stable session base is in place.

Acceptance criteria:

- `session/cancel` stops an active prompt/tool wait where possible.
- `session/close` cancels active work before closing resources.
- Late tool/model events after cancellation are ignored or surfaced safely.

### Hardening slice E: stable session-list pagination

The current cursor is opaque to clients, which satisfies ACP shape, but it is offset-based internally. Offset pagination can skip or duplicate rows when session `updated_at` changes while a client pages through results.

Tasks:

1. Replace offset cursors with keyset cursors over `(updated_at, id)` using the existing `ORDER BY updated_at DESC, id DESC` ordering.
2. Encode the cursor as an opaque token containing the last row's `updated_at` and `id`.
3. Reject malformed or stale cursors with clear validation errors.
4. Keep enforcing an internal page-size limit.

Acceptance criteria:

- Pagination is stable while sessions update concurrently.
- Clients still treat cursors as opaque ACP tokens.

### Hardening slice F: listed sessions vs adapter-local active sessions

ACP says `session/list` discovers sessions known to the agent. Today Den only lists persisted sessions, while the adapter may also know about newly created but never-prompted local sessions.

Tasks:

1. Decide whether `session/list` should include adapter-local in-memory sessions that have not yet reached Den persistence.
2. If yes, merge local rows with Den rows and clearly mark unpublished rows in `_meta`.
3. If no, document `session/list` as listing persisted/resumable Den sessions only.
4. Ensure `session/load` and `session/resume` error clearly for unknown/unpersisted session ids after adapter restart.

Acceptance criteria:

- The product behavior is intentional and documented.
- Users are not surprised when never-used transient sessions are absent from persisted history.

### Hardening slice G: auth consistency for ACP session support endpoints

The Den ACP prompt/list/get/history endpoints currently share the broader `authenticate_acp_principal` path, while close/tool-result paths require bear-scoped ACP Code tokens. This may be intentional, but should be explicit.

Tasks:

1. Decide whether ACP session backing APIs should accept generic bearer principals with `acp:chat`, or only bear-scoped ACP Code tokens.
2. Apply the decision consistently across:
   - session list/get;
   - conversation list/history;
   - prompt;
   - close;
   - tool result.
3. Document the boundary between external ACP Code-token adapter traffic and internal/admin API traffic.

Acceptance criteria:

- Auth behavior is consistent or intentionally differentiated.
- Token scope errors are clear to users and operators.

## Test plan

### Den ACP tests

- `GET /acp/bears/{slug}/sessions` returns ACP-backed session rows with non-empty absolute `cwd` values or intentionally excludes legacy rows without valid `cwd`.
- Session list rejects malformed cursors and non-absolute `cwd` filters.
- Session list pagination remains stable when sessions update between pages.
- Session get/list/history auth behavior matches the chosen ACP Code-token vs generic bearer policy.
- Prompt with no conversation id creates/selects a pending `new-acp-...` id.
- `conversation_resolved` updates the ACP session binding.
- Second prompt for the same ACP session routes to the resolved `conv-...` id.
- Prompt with explicit `conv-...` routes to that conversation.
- Prompt with invalid conversation id returns `400`.
- Prompt with a `conv-...` owned by a different bear returns `403` or `404`.
- `session/close` archives the resolved `conv-...`, not only the original pending `new-...`.

### Adapter tests

- `session/new`, `session/load`, and `session/resume` validate or deterministically normalize absolute `cwd`.
- `session/list` maps Den rows to ACP `SessionInfo` with `sessionId`, absolute `cwd`, `updatedAt`, optional `title`, `_meta`, and `nextCursor`.
- Non-empty `mcpServers` produce the chosen documented behavior: implemented, rejected, or compatibility-ignored.
- `session/load` replays history before returning `null`; current MVP tests should explicitly assert user/assistant text replay.
- `session/resume` restores context without emitting history replay updates.
- `session/cancel` and `session/close` cancellation behavior is covered once cancellation plumbing lands.
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

- Should `session/list` include adapter-local in-memory sessions that have never been persisted to Den, or should it intentionally list persisted/resumable Den sessions only?
- Should Den ACP session support endpoints accept generic bearer principals with `acp:chat`, or require bear-scoped ACP Code tokens consistently?
- Should non-empty ACP `mcpServers` be implemented, rejected, or temporarily accepted-and-ignored?
- Which Letta/Codepool historical event types can be faithfully replayed through ACP `session/load` beyond user/assistant text?
- What exact `session/load` request and response shape do current ACP clients expect?
- Should adapter-local session ids be stable across adapter process restarts, or is Den-side binding sufficient?
- Should ACP conversation list include archived conversations by default?
- Should Den expose conversation history through ACP, or should the adapter only need ids/titles for `session/load`?
- Should explicit `default` be allowed in `session/load`, or should clients use `session/new` for the main thread?
