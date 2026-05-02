# ACP Session Bindings — Architecture Decision Record

## Status: Accepted

## Date: 2026-05-02

---

## Context

BEARS exposes Agent Client Protocol (ACP) to coding clients through a local `bears-acp-adapter`. The adapter calls Den's ACP gateway, and Den routes work to Codepool's `bear_channel` runtime.

ACP sessions are protocol/client lifecycle objects. BEARS conversations are canonical Letta/BEARS conversation identities:

- `default` for a bear's main thread;
- `conv-...` for saved Letta conversations;
- temporary `new-...` ids until Codepool/Letta resolves them.

A durable ACP session therefore needs a binding from ACP `sessionId` to Codepool session id and BEARS/Letta conversation id, without treating ACP session rows as the source of truth for chat history.

---

## Decision

BEARS will model ACP sessions as **session bindings**, not canonical conversations.

Den may persist ACP session rows containing routing and lifecycle metadata, including:

- ACP `sessionId`;
- user and bear identifiers;
- ACP client name;
- absolute local client `cwd`;
- Codepool session id;
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

`session/load` replays all historical events BEARS can faithfully reconstruct. The current implementation replays user/assistant text messages only. Tool calls/results, reasoning/status chunks, errors, resource/image/audio content, and richer session configuration events are not claimed as reconstructable until Den/Letta/Codepool expose faithful historical event data.

### Cancellation and close

ACP `session/cancel` is plumbed through adapter -> Den -> Codepool where possible. `session/close` must call the same cancellation path before marking a session closed or archived. Pending ACP client tool calls for a cancelled/closed session are marked cancelled so late duplicate results are not treated as active work.

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
