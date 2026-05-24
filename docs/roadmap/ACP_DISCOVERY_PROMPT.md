# ACP Discovery Prompt

Status: historical discovery checklist kept for reference.

Use this checklist as historical background from the period before the direct ACP runtime was implemented.

## Goal

Originally, this document was used to confirm the concrete ACP transport, session, streaming, and tool schemas required by Zed/OpenCode so Den could act as an authenticated ACP gateway.

For current implementation work, use [`ACP_DIRECT_LOCAL_TOOL_RUNTIME_PLAN.md`](ACP_DIRECT_LOCAL_TOOL_RUNTIME_PLAN.md) and [`ACP_ADAPTER_IMPROVEMENT_PLAN.md`](ACP_ADAPTER_IMPROVEMENT_PLAN.md). ACP for `pair` is now in place on the direct Den ⇄ adapter ⇄ Letta path rather than a `bear_channel` mapping path.

## Questions to answer

### 1. Which ACP specification/package should BEARS target?

Please provide links or package names for the ACP implementation used by:

- Zed
- OpenCode
- Deep Chat, if relevant

For each, note the version.

### 2. What transport does the ACP client expect?

Options to confirm:

- stdio process
- HTTP streaming
- Server-Sent Events
- WebSocket
- JSON-RPC over a transport
- other

If stdio is required, decide whether Den should:

1. run a local sidecar/adapter process, or
2. expose scoped tokens and let a local adapter connect Den to Codepool.

### 3. How are sessions represented?

Need concrete schema for:

- create session
- resume session
- session id
- conversation/thread id
- cancellation
- reconnect

Map each field to `bear_channel`:

- `session_id`
- `conversation_id`
- `channel.family = coding_workspace`
- `channel.client = zed | opencode`
- `channel.protocol = agent_client_protocol`

### 4. How are client tools declared?

Confirm schema for:

- tool/function name
- JSON schema parameters
- descriptions
- tool approval metadata
- tool result messages
- errors

Decide how Den maps ACP tools to `bear_channel.capabilities.client_tools` and back to `client_tool_request` events.

### 5. How are streaming events represented?

Confirm support for:

- assistant deltas
- reasoning/status events
- tool request events
- tool result acknowledgements
- errors
- done/terminal events

Map ACP event types to `bear_channel` event types.

### 6. How should Den authenticate ACP clients?

Decide first supported mode:

- Den-issued bearer token
- browser session bootstrap to token
- OAuth flow
- local-only development token

Token must encode or resolve:

- user id
- allowed bear slugs/ids
- scopes: chat, client-tools, memory-write if applicable
- expiry

### 7. What should the first vertical slice prove?

Recommended first slice:

1. Zed/OpenCode connects to Den ACP endpoint for one bear.
2. Den authenticates and authorizes.
3. User sends text.
4. Den calls Codepool `bear_channel`.
5. Assistant text streams back.
6. No client tools yet.

Second slice:

1. Client declares one simple local read-only tool.
2. Bear emits `client_tool_request`.
3. Den maps it to ACP tool request.
4. Client returns result.
5. Bear continues.

## Output requested

Please collect:

- spec links
- package names/versions
- example ACP request/response payloads
- transport details
- any Zed/OpenCode config examples

Then update this document or create an implementation-ready plan under `docs/planning/`.
