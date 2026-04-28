# `bear_channel` Phase 7+ Plans

This document captures planned work after the initial `bear_channel` migration. These items are intentionally **not implemented** in the phase that introduced `bear_channel` for Den web chat.

See also: [`../architecture/BEAR_CHANNEL_AND_ACP.md`](../architecture/BEAR_CHANNEL_AND_ACP.md).

## 1. ACP client tool relay

Goal: Zed/OpenCode connect to Den using Agent Client Protocol (ACP), while the bear runtime can request client-side local tools.

### Scope

- Den ACP gateway authenticates client tokens or sessions.
- Den authorizes access to the selected bear.
- ACP client capabilities are mapped to `bear_channel.capabilities.client_tools`.
- Codepool/bear runtime emits `client_tool_request` events.
- Den translates `client_tool_request` to ACP tool requests.
- ACP client executes local tools and returns results.
- Den forwards client tool results back to Codepool.

### Design tasks

1. Confirm ACP transport and schema details for Zed/OpenCode.
2. Define Den ACP route shape, likely `/acp/bears/{slug}`.
3. Add scoped channel tokens in Den.
4. Add pending client-tool-call persistence keyed by session/call id.
5. Add timeout, cancellation, and error propagation semantics.
6. Audit every client tool request/result with user, bear, session, and request ids.

### Non-goals for first implementation

- Arbitrary async background use of local tools after the client disconnects.
- Exposing BEARS server tools as ACP client tools; server tools should remain internal unless explicitly needed.

## 2. BEARS skills and Cabinet over `bear_channel`

Goal: BEARS catalog skills and Cabinet tools are active for every channel that uses a bear, including Den web chat and ACP coding clients.

### Scope

- Define a central Den capability registry.
- Represent Cabinet tools as BEARS capabilities, not client-specific MCP-only tools.
- Load bear-assigned skills from the BEARS catalog into `bear_channel` runtime context.
- Resolve skill capability requirements against available server and client tools.

### Initial server tools

- `cabinet.search`
- `cabinet.read`
- `memory.search`
- `memory.remember`

### Design tasks

1. Define skill catalog schema: id, description, instructions, required capabilities, optional capabilities, memory/reflection policy.
2. Define tool descriptors with execution location: `server`, `client`, `remote_mcp`, `subagent`.
3. Add Den endpoint or bundle for Codepool to fetch trusted bear runtime capability context.
4. Decide whether Codepool executes Den tools directly through Den APIs or whether Den remains in the tool execution path.
5. Add audit records for Cabinet and memory tool usage.

## 3. Reflection and sub-agent event model

Goal: coding and product sessions can spawn reflection sub-agents that learn from the session and update relevant memory or Cabinet entries under policy.

### Scope

- Event stream includes sub-agent lifecycle events.
- Session transcript/tool-result excerpts are available for reflection.
- Reflection runs server-side and does not require local client tools in the first version.
- Memory writes are explicit/audited.

### Event types

- `subagent_started`
- `subagent_finished`
- `memory_update_proposed`
- `memory_update_recorded`
- `memory_update_rejected`

### Design tasks

1. Define reflection policy per bear and per skill.
2. Define transcript/event-log retention for reflection.
3. Decide when reflection is triggered: after significant turns, explicit request, skill completion, or session end.
4. Add a background job model for reflection sub-agents.
5. Add memory-write approval policy: automatic, proposal, explicit-only, or disabled.

## 4. Legacy endpoint deprecation

Goal: move Den and first-party clients to `bear_channel` while keeping compatibility for OpenWebUI and other generic clients.

### Current legacy endpoints

- Codepool `POST /v1/conversations/:conversationId/messages`
- Codepool `POST /v1/chat/completions`

### Plan

1. Keep endpoints active while Den web chat uses `bear_channel`.
2. Add metrics comparing legacy and `bear_channel` usage.
3. Document `bear_channel` as the canonical Den -> Codepool path.
4. Retain OpenAI-compatible endpoint as an edge compatibility adapter.
5. Deprecate only the Den-specific legacy conversation endpoint after all first-party traffic has migrated.

### Removal criteria

- Den web chat uses `bear_channel` in production for at least one release cycle.
- ACP gateway uses `bear_channel` for coding clients.
- No first-party service depends on `/v1/conversations/:conversationId/messages`.
- Runbook exists for third-party compatibility users.

## 5. Rich web UI event surfacing

Goal: Den web chat surfaces BEARS runtime activity without breaking normal chat readability.

### Candidate UI events

- skill selected
- Cabinet search started/finished
- memory search/write
- sub-agent started/finished
- artifact created
- reflection completed

### Design tasks

1. Decide which events appear inline, in a collapsible activity panel, or only in debug mode.
2. Extend Deep Chat rendering or surrounding Den UI to show activity events.
3. Preserve accessible status text for screen readers.
4. Add event filtering by channel/client capability.
5. Add design fixtures under `/design/chat` and keep real chat layout synchronized.

### Initial recommendation

Start with a small activity strip or collapsible timeline above the composer. Keep normal assistant/user transcript uncluttered.
