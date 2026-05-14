# ACP Session Bindings — Architecture Decision Record

## Status: Accepted

## Date: 2026-05-02
## Updated: 2026-05-13

---

## Context

BEARS exposes Agent Client Protocol (ACP) to coding clients through a local `bears-acp-adapter`. The adapter calls Den's ACP gateway, and Den routes the `pair` role through the direct Letta conversation API. Earlier ACP designs routed through Codepool `bear_channel`; that is historical context only for the `pair` role.

ACP sessions are protocol/client lifecycle objects. BEARS conversations are canonical Letta/BEARS conversation identities:

- `default` for a bear's main thread;
- `conv-...` for saved Letta conversations;
- temporary `new-...` ids until Den/Letta resolves them.

A durable ACP session therefore needs a binding from ACP `sessionId` to Den's selected/resolved BEARS/Letta conversation id, without treating ACP session rows as the source of truth for chat history.

---

## Decision

BEARS will model ACP sessions as **session bindings**, not canonical conversations.

Den may persist ACP session rows containing routing and lifecycle metadata, including:

- ACP `sessionId`;
- user and bear identifiers;
- ACP client name;
- absolute local client `cwd`;
- Den runtime session id;
- pending/generated conversation id;
- resolved `conv-...` id when available;
- closed/archive timestamps.

Canonical conversation listing, history, search, memory, and archive behavior remain based on BEARS/Letta conversations, not ACP session rows.

### Filesystem context

ACP-facing session setup requires a truthful absolute local `cwd`.

The local adapter must prefer explicit `params.cwd`, then accept known workspace URI/folder fallbacks only if they normalize to absolute local paths. Den must persist only validated absolute `cwd` values for ACP prompt-created bindings. `session/list` must not return ACP rows with missing or non-absolute `cwd`; legacy invalid rows may be omitted and logged.

### Session listing

`session/list` lists persisted/resumable Den ACP session bindings only. Adapter-local sessions created by `session/new` but never prompted are transient and intentionally absent from persisted history.

Pagination uses opaque keyset cursors over `(updated_at, id)` in descending order. Offset cursors are not used because session updates can otherwise skip or duplicate rows while a client pages through results.

### Load/resume behavior

`session/resume` restores adapter routing context without replaying history.

`session/load` replays all historical events BEARS can faithfully reconstruct. The current implementation replays user/assistant text messages only. Tool calls/results, reasoning/status chunks, errors, resource/image/audio content, and richer session configuration events are not claimed as reconstructable until Den/Letta expose faithful historical event data.

### Cancellation, close, and stuck Letta approvals

ACP `session/cancel` is plumbed through adapter -> Den where possible. `session/close` should cancel active direct ACP tool turns before marking a session closed or archived. Pending ACP client tool calls for a cancelled/closed session are marked cancelled so late duplicate results are not treated as active work.

A stuck Letta approval on a bound `conv-*` is not safely fixed by silently rebinding the ACP session to a fresh conversation. The main value of retaining the ACP session is conversation continuity/history; replacing the bound conversation inside the same ACP session violates that expectation.

When Letta reports stale approval state for a bound ACP session, BEARS should first attempt Letta conversation compaction on the same `conv-*` via `/v1/conversations/{conversation_id}/compact`. If compaction succeeds, the adapter may retry the prompt once against the same ACP session and conversation binding. If compaction fails or the retry still reports stale approval, BEARS should stop and ask the user to start a new ACP session rather than pretending the old session was repaired.

This amends the earlier “unwedge” idea: cancellation of runs and cleanup of local in-memory tool turns may still be useful internal hygiene, but it is not a reliable user-facing recovery for malformed conversation approval state and should not be advertised as such.

### MCP server handling

ACP-provided `mcpServers` are not supported by the BEARS local adapter today. Non-empty `mcpServers` are rejected with a clear unsupported-feature error. Empty/null values are tolerated. The adapter must not advertise HTTP/SSE MCP support until implemented.

If stdio MCP support is added later, the adapter process owns local MCP subprocess lifecycle. Den remains the policy and audit authority for BEARS tools, and local MCP credentials must not be persisted in Den.

### Auth policy

External ACP backing endpoints require bear-scoped ACP Code tokens:

- prompt/list/get/history/close/cancel require `acp:chat`;
- tool-result delivery additionally requires `acp:tools`.

Generic bearer principals should use separate internal/admin API surfaces rather than external ACP adapter endpoints.

---

## Consequences

- ACP session rows are useful for protocol lifecycle and routing, but are not product conversation records.
- `cwd` failures are surfaced early and deterministically instead of being silently persisted as empty or relative paths.
- Clients page through ACP sessions with stable opaque cursors.
- Never-prompted local ACP sessions do not survive adapter restart and are not listed as resumable sessions.
- MCP behavior is predictable and conservative until real stdio MCP lifecycle management exists.
- ACP adapter traffic has a consistent token model across backing endpoints.
- Stuck approval recovery preserves conversation binding when possible through compaction, and otherwise fails explicitly so users can start a new ACP session with a fresh conversation.
