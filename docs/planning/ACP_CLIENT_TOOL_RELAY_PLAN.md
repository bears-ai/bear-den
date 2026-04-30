# ACP client tool relay plan

Status: design-ready plan; not implemented.

Owner boundary: Den remains the security, policy, and audit authority. `bears-acp-adapter` remains the local ACP stdio edge process. Codepool remains the private Letta Code runtime owner.

Related docs:

- [`BEAR_CHANNEL_PLANS.md`](BEAR_CHANNEL_PLANS.md)
- [`BEAR_CAPABILITY_MANAGEMENT_PLAN.md`](BEAR_CAPABILITY_MANAGEMENT_PLAN.md)
- [`../architecture/BEAR_CHANNEL_AND_ACP.md`](../architecture/BEAR_CHANNEL_AND_ACP.md)

## Goal

Let a bear running in Codepool request session-scoped tools from an ACP client such as Zed while preserving Den authorization, user approval semantics, auditability, and disconnect safety.

The first implementation should prove one end-to-end local read-only tool before enabling writes, terminals, MCP relay, or background work.

## Non-goals for the first relay slice

- No arbitrary background local tool use after the ACP client disconnects.
- No persisted durable local workspace credentials in Den.
- No exposing BEARS built-in server tools as ACP client tools.
- No terminal execution in the first tool slice.
- No write/edit tools until read-only tool relay has tests and operator-visible audit.
- No session resume/load dependency. Continue using `conversation_id = "default"` until the session lifecycle is designed separately.

## ACP protocol facts this design depends on

ACP is JSON-RPC over stdio between the client and agent process.

The client sends `initialize` with `clientCapabilities`, including:

- `fs.readTextFile`
- `fs.writeTextFile`
- `terminal`

The agent can send JSON-RPC requests back to the client while handling `session/prompt`, including:

- `fs/read_text_file`
- `fs/write_text_file`
- `terminal/create`
- `terminal/output`
- `terminal/wait_for_exit`
- `terminal/kill`
- `terminal/release`
- `session/request_permission`

The agent reports tool progress to the client with `session/update` notifications using:

- `tool_call`
- `tool_call_update`

`bears-acp-adapter` is the ACP agent from Zed's point of view, so it is responsible for sending these JSON-RPC requests/notifications to Zed and correlating responses.

## ACP legibility / normal-provider guidance

This design should look like a normal ACP agent to clients such as Zed. Den and Codepool are implementation details behind the local adapter.

Normal ACP shape:

- The ACP client starts a local agent process over stdio.
- The client sends `initialize` with `clientCapabilities`.
- The agent checks those capabilities before using client methods.
- During `session/prompt`, the agent reports work with `session/update` notifications.
- For local filesystem access, the agent calls standard client methods such as `fs/read_text_file` and `fs/write_text_file`.
- For user approval, the agent calls standard `session/request_permission`.
- Tool progress is represented with standard `tool_call` and `tool_call_update` updates.
- The agent eventually replies to the original `session/prompt` with a standard `stopReason`.

BEARS-specific pieces must remain below the adapter boundary during normal operation:

- Den bearer tokens and `acp:chat` / `acp:tools` scopes are BEARS auth, not ACP protocol concepts.
- Den SSE events and `tool-results/{call_id}` endpoints are adapter-to-Den internals, not ACP messages.
- Codepool `bear_channel` and `client_tool_request` are internal runtime protocol, not ACP messages.
- Internal capability names such as `acp_fs_read_text_file` are acceptable inside Den/Codepool, but ACP-facing updates should use normal titles/kinds and standard methods.

Diagnostic exception:

- It is appropriate to expose implementation details in structured error/debug metadata when they help operators identify the failing boundary.
- Examples: Den `/version` metadata, `request_id`, `call_id`, HTTP status from Den, Codepool request id, adapter version/git SHA, and high-level failing component such as `adapter`, `den_api`, `codepool`, or `letta_code`.
- These details should appear in JSON-RPC error `data`, adapter stderr logs, Den/Codepool structured logs, and user-facing troubleshooting text when useful.
- They should not replace the normal ACP flow or become protocol requirements for the client.
- Do not expose secrets, raw bearer tokens, full local file contents, or overly detailed internal URLs in client-visible errors.

Legibility rule:

> If someone opens Zed's ACP logs during a healthy flow, they should see ordinary ACP: `initialize`, `session/new`, `session/prompt`, `session/update` tool calls, optional `session/request_permission`, standard `fs/read_text_file` requests, and a final `session/prompt` response. They should not need to understand Den, Codepool, `bear_channel`, or BEARS token scopes to recognize the protocol flow. When something fails, structured diagnostic metadata may name those BEARS components so the user/operator can find the right logs.

Implications for the implementation:

- Store raw `initialize.clientCapabilities` and use them exactly as ACP defines them.
- Do not call `fs/read_text_file` unless `clientCapabilities.fs.readTextFile` is true.
- Do not present Den's internal `client_tool_request` event directly to the ACP client.
- Adapter must translate internal requests into:
  - `session/update` `tool_call` / `tool_call_update`
  - optional `session/request_permission`
  - `fs/read_text_file` or other standard client methods
- Use ACP `ToolKind` values (`read`, `edit`, `execute`, etc.) and `ToolCallLocation` for UI legibility.
- Keep BEARS auth failures as JSON-RPC errors on `session/prompt` or setup diagnostics, not as fake ACP tool failures.
- Include correlation ids and component/version metadata in error `data` where useful, while keeping the top-level JSON-RPC error message concise and ACP-normal.

## High-level architecture

1. Zed starts `bears-acp-adapter` over stdio.
2. Zed sends `initialize` with client capabilities.
3. Adapter normalizes those capabilities and includes them in Den prompt requests.
4. Den authenticates the token, authorizes bear access, applies policy, and forwards only allowed client tool descriptors to Codepool via `bear_channel.capabilities.client_tools`.
5. Codepool/Letta Code may emit `client_tool_request` events only for declared tools.
6. Den records a pending tool call and streams the request to the adapter.
7. Adapter maps the request to ACP client methods, including optional `session/request_permission`.
8. Zed executes the local operation or returns an error/cancelled response.
9. Adapter sends the result to Den.
10. Den audits the result and forwards it to Codepool through a tool-result continuation endpoint.
11. Codepool resumes the model/tool loop and continues streaming assistant output.

## First vertical slice

Use a single read-only local file tool.

Internal name in Den/Codepool capability descriptor:

- `acp_fs_read_text_file`

ACP-facing operation used by adapter:

- Standard client method `fs/read_text_file`

ACP-facing tool-call UI:

- `session/update` `tool_call` with a human title such as “Read file”
- `kind: "read"`
- `locations: [{ path, line? }]` when available

Supported arguments:

- `path`: absolute path in the ACP client's workspace context
- `line`: optional 1-based start line
- `limit`: optional maximum lines

Result shape returned to Codepool:

- `ok: true`
- `content`: file text
- `metadata`: optional line range, truncation notes, byte count

This slice proves:

- capability discovery from `initialize`
- Den policy filtering
- Codepool tool request emission
- Den pending-call persistence
- adapter JSON-RPC request correlation
- ACP client result mapping
- Codepool continuation after the tool result
- audit records for request and result

## Capability descriptor shape

Den should pass normalized descriptors into `bear_channel.capabilities.client_tools`. These are trusted by Codepool because Den constructs them.

Required fields:

- `id`: stable Den capability id, e.g. `acp_fs_read_text_file`
- `name`: runtime callable name, initially same as `id`
- `title`: human-readable label
- `description`: model-facing description
- `provider`: `acp_client`
- `execution_target`: `acp_client`
- `scope`: `client_connection` or `session`
- `client`: `zed` or `opencode`
- `permissions`: normalized permissions such as `filesystem`, `read`, `write`, `shell`
- `approval_policy`: `never`, `on_write`, `on_sensitive_action`, or `always`
- `input_schema`: JSON schema for runtime arguments
- `output_schema`: JSON schema for results
- `acp`: ACP mapping metadata
  - `method`: e.g. `fs/read_text_file`
  - `requires_client_capability`: e.g. `fs.readTextFile`

Initial descriptor rules:

- If `clientCapabilities.fs.readTextFile` is false, omit `acp_fs_read_text_file`.
- If `clientCapabilities.fs.writeTextFile` is true, still omit write tools in slice 1.
- If `clientCapabilities.terminal` is true, still omit terminal tools in slice 1.
- Den may further filter by token scopes, bear policy, user role, and future per-bear capability settings.

## Den product/UI changes

Yes: enabling client tool relay should update Den's token generation and token listing UI so users understand what a **Code token** can do.

Product assumption for Den UI: a bear-scoped **Code token** authorizes both `acp:chat` and `acp:tools`. Users do not choose between separate “chat-only” and “tools-enabled” ACP tokens in the primary UI. The token is for coding clients such as Zed/OpenCode, and local tool access is governed by the active client/session, Den policy, and ACP permission flows.

### Token generation UI

Keep the bear “Code with …” token generation page focused on creating a single kind of token:

1. **Code token**
   - Scopes stored by Den: `acp:chat`, `acp:tools`
   - Copy: “Use this token to chat with this bear from Zed/OpenCode and allow approved local editor tools, such as reading workspace files.”
   - Include a warning that tool availability depends on the active client/session and Den policy, and that the editor may still ask for permission before sensitive local actions.

The UI should not imply that `acp:tools` grants every possible tool. It only authorizes Den to broker policy-approved client tools exposed by the active ACP client.

Recommended controls:

- Token name input, as today.
- A permissions summary for the Code token:
  - chat with this bear from coding clients;
  - broker approved local editor tools exposed by the active ACP client;
  - tools are session-scoped and unavailable after the client disconnects.
- After generation, show Zed config plus a reminder that the raw token is shown once.

### Token listing UI

Update the account token table to make Code tokens easy to audit without forcing users to reason about internal scope names.

Add or surface:

- `Type` or compact badge: `Code`
- Optional advanced/details view showing stored scopes: `acp:chat`, `acp:tools`
- Bear restriction, as today.
- Last used, as today.
- Revoked status, as today.
- Optional future: last tool use timestamp or tool-use count.

For Code tokens, include a visual affordance that the token can broker local editor tools. This helps users audit and revoke higher-privilege tokens.

### Token creation backend

Current `acp_tokens::create_for_bear` uses a fixed default scope. Update the Code-token creation path:

- Always store both `acp:chat` and `acp:tools` for Code tokens created by Den UI.
- Keep scope validation against an allowlist so future token types or API-driven creation cannot persist unknown scopes.
- Store scopes in the existing `acp_tokens.scopes` JSONB column.

### Token authorization behavior

- Basic prompt route continues to require `acp:chat`.
- Tool-enabled prompt requests and tool-result endpoints require `acp:tools`.
- Den UI-created Code tokens should satisfy both requirements because they include both scopes.
- If Den sees a legacy/API-created token that has `acp:chat` but lacks `acp:tools`, Den should omit all client tool descriptors and log `acp_tools_scope_missing`, so chat still works.

## Den API changes

### Extend ACP prompt request

Current adapter request body contains `message`, `conversation_id`, and `client`.

Add optional fields:

- `client_capabilities`: raw ACP client capabilities as sent to adapter in `initialize`
- `client_context`: normalized context from `session/new`
  - `cwd`
  - `client_name`
  - `client_version`
  - `adapter_version`
- `client_tools`: adapter-normalized tool descriptors, if we prefer adapter-side normalization

Recommended: send raw `client_capabilities` plus minimal `client_context` first, and let Den construct trusted descriptors. Adapter-side descriptors can be added later for MCP/custom extensions.

### Stream tool requests to adapter

Den already streams SSE from `/prompt` to adapter. Extend the Den adapter-facing SSE event set with:

- `client_tool_request`
- `client_tool_result_accepted`
- `client_tool_result_rejected`

A `client_tool_request` event should include:

- `request_id`: Den request id for the prompt turn
- `session_id`: ACP session id
- `conversation_id`: currently `default`
- `call_id`: Codepool call id
- `tool_name`: normalized tool name
- `arguments`: JSON arguments
- `descriptor`: selected Den descriptor snapshot
- `approval_policy`: effective approval policy
- `timeout_ms`: Den-selected timeout

### Add tool result endpoint

Add an adapter-to-Den endpoint:

- `POST /acp/bears/{slug}/sessions/{session_id}/tool-results/{call_id}`

Request body fields:

- `request_id`: original prompt request id
- `conversation_id`: currently `default`
- `status`: `ok`, `error`, `cancelled`, or `timeout`
- `result`: JSON result for `ok`
- `error`: structured error for non-ok statuses
- `client_observation`: optional adapter/client metadata for audit/debugging

Auth rules:

- Same bearer token requirements as prompt.
- Token must resolve to the same user and bear.
- Tool result must match an active pending call for that user, bear, ACP session, request id, and call id.
- Reject duplicate terminal results unless the previous result is in a retryable state.

## Den persistence

Add a pending-call table. Proposed name:

- `acp_client_tool_calls`

Columns:

- `id UUID PRIMARY KEY`
- `user_id INTEGER NOT NULL`
- `bear_id UUID NOT NULL`
- `bear_slug TEXT NOT NULL`
- `acp_session_id TEXT NOT NULL`
- `codepool_session_id TEXT NOT NULL`
- `conversation_id TEXT NOT NULL`
- `request_id UUID NOT NULL`
- `call_id TEXT NOT NULL`
- `tool_name TEXT NOT NULL`
- `arguments JSONB NOT NULL`
- `descriptor JSONB NOT NULL`
- `status TEXT NOT NULL`
  - `pending`
  - `sent_to_client`
  - `approved`
  - `rejected`
  - `result_received`
  - `forwarded_to_codepool`
  - `failed`
  - `timed_out`
  - `cancelled`
- `result JSONB NULL`
- `error JSONB NULL`
- `approval_outcome JSONB NULL`
- `created_at TIMESTAMPTZ NOT NULL DEFAULT now()`
- `sent_at TIMESTAMPTZ NULL`
- `approved_at TIMESTAMPTZ NULL`
- `result_received_at TIMESTAMPTZ NULL`
- `forwarded_at TIMESTAMPTZ NULL`
- `expires_at TIMESTAMPTZ NOT NULL`

Indexes:

- unique `(request_id, call_id)`
- `(user_id, bear_id, acp_session_id, status)`
- `(expires_at)` for cleanup

Use this same table as the audit source for MVP, or add a separate append-only audit table if we want immutable events immediately.

## Letta Code SDK research findings

Current BEARS Codepool uses:

- `@letta-ai/letta-code-sdk` `0.1.14`
- `@letta-ai/letta-code` `0.23.8` via package override/patching

The SDK already has the primitive we need for externally resolved tools. It does **not** expose a method literally named “pause”, but custom tools are asynchronous and the Letta Code CLI waits for the SDK process to return an external tool result.

Relevant SDK surface:

- `CreateSessionOptions.tools?: AnyAgentTool[]`
- `AgentTool.execute(toolCallId, args, signal?, onUpdate?) => Promise<AgentToolResult>`
- SDK registers tools with the CLI using a `register_external_tools` control request.
- When the model calls an external tool, the CLI sends `execute_external_tool` back to the SDK.
- The SDK calls `await tool.execute(...)`.
- Only after the promise resolves does the SDK send `external_tool_result` back to the CLI.

Current Codepool already uses this for Den server tools in `services/codepool/src/den-tools.ts`: `makeDenExternalTools` creates SDK `AnyAgentTool`s whose `execute` function awaits a Den HTTP call. This proves the SDK supports async external tool execution in the SDK process.

Conclusion:

- We do **not** need a new Letta Code SDK pause/resume feature for the MVP.
- We can implement ACP client tools as SDK external tools whose `execute` promise blocks until Den/adapter returns a result or a timeout/cancel occurs.
- The “pause” boundary is the external tool `execute` promise: Codepool waits inside that promise, the CLI/model turn is effectively paused, and then the returned `AgentToolResult` continues the turn.

Caveats:

- This is turn-local waiting, not durable pause/resume across Codepool process restarts.
- If Codepool dies while waiting, the active tool call is lost unless we add higher-level recovery.
- The SDK passes an optional `AbortSignal` into `execute`; Codepool should honor it once cancellation support is wired.
- The SDK external tool response only sends `content` and `is_error` back to the CLI; `details` are useful to Codepool/Den but may not be visible to Letta Code unless rendered into text content.
- Codepool currently creates sessions once and only passes `tools` when opening a new session. If a warm session already exists, newly authorized tools may not be registered unless we close/recreate the session or the SDK supports registering additional external tools after init. MVP should either key sessions by tool descriptor set or recreate the session when client tools change.

Recommended implementation adjustment:

- Implement a Codepool-side `makeAcpClientExternalTools(...)` alongside `makeDenExternalTools(...)`.
- Each ACP external tool `execute` should:
  1. create/record a pending waiter keyed by `request_id` and `toolCallId`,
  2. emit a `client_tool_request` event to Den,
  3. await the result posted back to Codepool's internal tool-result endpoint,
  4. resolve to `AgentToolResult` on success,
  5. resolve to an error result on timeout/cancel/error.
- Because the SDK already awaits `execute`, no separate SDK pause API is required.

## Codepool contract changes

### `bear_channel` request

Den sends non-empty `capabilities.client_tools` only after policy filtering.

Each descriptor should be treated as authoritative. Codepool must not invent client tool names.

### Runtime tool request event

Codepool emits:

- `type`: `client_tool_request`
- `call.id`: unique within the prompt turn/session
- `call.name`: one of the declared `capabilities.client_tools[*].name`
- `call.arguments`: JSON arguments

Recommended additions:

- `call.title`: optional UI title
- `call.kind`: normalized ACP `ToolKind`, e.g. `read`, `edit`, `execute`
- `call.locations`: optional affected file locations
- `call.requires_approval`: optional bool; Den remains final authority

### Tool result continuation endpoint

Add an internal Codepool endpoint for Den:

- `POST /internal/bear_channel/sessions/{session_id}/tool-results`

Request body:

- `conversation_id`
- `request_id`
- `call_id`
- `tool_name`
- `status`
- `result`
- `error`

This endpoint should deliver the tool result to the active runtime loop waiting on the tool call.

Important constraint: Codepool must support an active prompt turn waiting for a tool result. Research confirms the current SDK supports this through async external tools: expose ACP tools to the SDK as `AnyAgentTool`s, emit `client_tool_request` from `execute`, and keep the `execute` promise pending until Den posts the result or timeout/cancel occurs.

## Adapter behavior

The adapter is the ACP agent from the client's perspective. Its stdio transcript must remain standard ACP even though it bridges to Den internally.

### Initialization

Store from ACP `initialize`:

- protocol version
- client info
- client capabilities
- whether `fs/read_text_file` is available
- whether `fs/write_text_file` is available
- whether terminal methods are available

Continue returning agent capabilities with current prompt support. Do not advertise `loadSession`, MCP, terminal, or write/edit support as agent capabilities unless implemented.

### Prompt

When calling Den prompt endpoint, include:

- prompt text
- `client`
- `conversation_id`
- `client_capabilities`
- `client_context.cwd` from `session/new`
- adapter version and git SHA if available

### Handling Den `client_tool_request`

For internal request `acp_fs_read_text_file`:

1. Emit ACP `session/update` `tool_call` with:
   - `toolCallId = call_id`
   - `title = "Read file"` or a path-specific title
   - `kind = "read"`
   - `status = "pending"`
   - `rawInput = { path, line, limit }`
   - `locations = [{ path, line }]` when available
2. If Den says approval is required, call ACP `session/request_permission` with the same `toolCall` context.
3. If rejected/cancelled, emit `tool_call_update` with `status = "failed"` or cancellation text, then POST Den tool result with `status = rejected` or `cancelled`.
4. If approved or no approval required, emit `tool_call_update` with `status = "in_progress"`.
5. Send JSON-RPC request to the client method `fs/read_text_file`.
6. On success, emit ACP `tool_call_update` with `status = "completed"`, `rawOutput`, and a short content preview.
7. POST result to Den.
8. On error, emit ACP `tool_call_update` with `status = "failed"` and POST structured error to Den.

Adapter must support concurrent JSON-RPC requests while a prompt is in progress. The current line-by-line handler will need a request dispatcher with pending response correlation by JSON-RPC id.

## Approval policy

Initial Den defaults:

- read-only filesystem: `never` or `on_sensitive_action` depending on product stance
- write filesystem: `always` when introduced
- terminal execution: `always` when introduced
- delete/move: `always` when introduced

For the first slice, choose one:

- Safer default: require `session/request_permission` for every local file read.
- Smoother developer default: allow reads without ACP permission if Zed has already granted the external agent access, but audit every read.

Recommendation: use `on_sensitive_action` for file reads, where Den can later classify sensitive paths. For MVP, implement as `never` for reads but keep the descriptor field and audit policy in place.

Sensitive-path policy should eventually force approval or deny for:

- `.env`
- private keys
- credential files
- SSH config/keys
- large binary-looking files
- paths outside the workspace root, if known

## Timeout, cancellation, and disconnect semantics

Timeouts:

- Den sets a per-tool timeout, initially 30 seconds for file reads.
- Adapter also applies a client request timeout slightly below Den timeout.
- If timeout expires, adapter posts `status = timeout` when possible.
- Den marks pending call timed out and forwards timeout to Codepool.

Cancellation:

- If ACP client sends `session/cancel`, adapter should:
  - mark prompt turn as cancelling
  - respond to pending `session/request_permission` as cancelled if possible
  - stop issuing new client tool requests
  - POST cancellation for pending calls to Den
  - call future Den cancel endpoint when available
- Until full runtime cancellation exists, Den should at least reject late tool results after cancellation/timeout.

Disconnect:

- If adapter process exits or stdio closes, pending calls eventually expire.
- Den cleanup marks them `timed_out` or `cancelled`.
- Codepool receives timeout/cancel result so the runtime can stop waiting.

Duplicate/late results:

- Den accepts only the first terminal result for a pending call.
- Late results after `forwarded_to_codepool`, `timed_out`, or `cancelled` return `409 Conflict` with a structured error.

## Error model

Use structured errors consistently:

- `kind`: `permission_denied`, `not_found`, `invalid_arguments`, `client_capability_missing`, `client_error`, `timeout`, `cancelled`, `transport_error`, `policy_denied`
- `message`: short human-readable message
- `detail`: optional detail safe for logs/UI
- `retryable`: bool
- `client_error_code`: optional ACP/JSON-RPC code

Mapping examples:

- ACP `-32002 Resource not found` -> `not_found`
- JSON-RPC timeout -> `timeout`
- permission rejected -> `permission_denied`
- Den policy block -> `policy_denied`

## Security and policy rules

- Clients never send trusted bear/user context.
- Den constructs all capability descriptors after authentication and membership checks.
- Codepool can only request declared client tools.
- Den rejects any requested tool name not in the descriptor snapshot for the prompt turn.
- Den validates arguments before forwarding to adapter and again before forwarding results to Codepool.
- Den enforces token scope:
  - `acp:chat` remains enough for basic chat.
  - `acp:tools` is required before Den brokers client tools.
  - Den UI-created Code tokens include both scopes by default.
- Den checks membership at prompt and result time.
- Den records enough context to answer: who requested what local tool, from which client, for which bear/session, with which arguments, and what result/error came back.
- Den should redact or summarize large/sensitive result content in durable audit where appropriate. The full result may be forwarded to Codepool for the active turn, but audit storage should have size and sensitivity limits.

## Observability

Add structured logs and metrics at each boundary.

Den logs/events:

- `acp_client_capabilities_received`
- `acp_client_tools_authorized`
- `acp_client_tool_request_received_from_codepool`
- `acp_client_tool_request_sent_to_adapter`
- `acp_client_tool_result_received`
- `acp_client_tool_result_forwarded_to_codepool`
- `acp_client_tool_call_failed`
- `acp_client_tool_call_timed_out`

Useful dimensions:

- `request_id`
- `call_id`
- `user_id`
- `bear_id`
- `bear_slug`
- `client`
- `tool_name`
- `status`
- `duration_ms`
- `error_kind`

Metrics:

- count tool requests by tool/status/client
- duration histogram by tool/status/client
- timeout count
- policy denial count
- rejected approval count
- duplicate/late result count

## Tests

### Den tests

- Prompt with `client_capabilities.fs.readTextFile = true` includes `acp_fs_read_text_file` in the Codepool request.
- Prompt with missing/false read capability omits client tools.
- Den UI-created Code tokens persist both `acp:chat` and `acp:tools`.
- Prompt with a legacy/API-created `acp:chat` token but no `acp:tools` omits client tool descriptors and logs `acp_tools_scope_missing`.
- Tool-result endpoint rejects tokens without `acp:tools`.
- Den rejects Codepool `client_tool_request` for undeclared tool name.
- Den persists a pending call for declared tool request.
- Den streams adapter-facing `client_tool_request` with expected payload.
- Tool result endpoint rejects missing auth, wrong bear, wrong session, wrong request id, wrong call id, duplicate result, and expired call.
- Tool result endpoint forwards successful result to fake Codepool endpoint.
- Timeout cleanup marks pending calls and forwards timeout to fake Codepool.

### Adapter tests

- `initialize` stores client capabilities.
- `session/new` stores `cwd` by ACP session id.
- Prompt request to Den includes `client_capabilities` and `client_context`.
- Den `client_tool_request` maps to ACP `fs/read_text_file` request.
- JSON-RPC response correlation works while prompt streaming is active.
- ACP read success posts Den result with expected shape.
- ACP read error posts structured error.
- Permission rejected maps to cancelled/rejected result.
- Tool call and tool call update notifications are emitted with correct `toolCallId`, status, kind, title, raw input, and raw output summary.

### Codepool tests

- Codepool exposes only declared client tools to the runtime.
- Undeclared client tool names are rejected before emitting events.
- Tool-result continuation wakes the waiting runtime/tool bridge.
- Tool-result timeout unblocks the runtime with a structured error.

## Implementation sequence

1. **Descriptor groundwork in Den**
   - Add normalized `AcpClientCapabilities` parsing.
   - Add `acp_fs_read_text_file` descriptor construction.
   - Update Code-token creation so Den UI stores both `acp:chat` and `acp:tools`; keep legacy/API-created chat-only tokens from receiving tool descriptors.

2. **Adapter capability forwarding**
   - Store `initialize.clientCapabilities`.
   - Store `session/new.cwd`.
   - Include capability/context fields in Den prompt requests.

3. **Den -> Codepool descriptor propagation**
   - Pass authorized descriptors in `bear_channel.capabilities.client_tools`.
   - Add integration tests around request construction.

4. **Codepool runtime bridge**
   - Expose declared client tools to Letta Code as async runtime tools.
   - Emit `client_tool_request` and wait for result.
   - Add internal `tool-results` endpoint.

5. **Den pending-call and adapter-facing relay**
   - Persist pending calls.
   - Convert Codepool `client_tool_request` events to adapter SSE events.
   - Add adapter-to-Den tool result endpoint.
   - Forward result to Codepool.

6. **Adapter JSON-RPC client calls**
   - Refactor stdio loop into request/response dispatcher.
   - Implement `fs/read_text_file` request, timeout, error handling, and result POST.
   - Emit ACP tool call progress notifications.

7. **Hardening**
   - Add timeout cleanup.
   - Add cancellation handling.
   - Add audit redaction/size limits.
   - Add user-visible debugging guidance.

## Open questions

1. Should first-slice file reads require ACP `session/request_permission`, or is Zed's external-agent trust boundary sufficient for MVP?
2. Should existing chat-only ACP tokens be migrated to Code tokens with `acp:tools`, or remain chat-only until regenerated?
3. How should Codepool handle warm sessions when the authorized client tool descriptor set changes between ACP sessions or prompt turns?
4. Should Den stream tool requests over the existing prompt SSE response, or split tool relay onto a dedicated bidirectional-ish long-poll/SSE channel?
5. How much local file content should Den store in audit, if any, versus storing only hashes/summaries?
6. Should absolute paths be allowed as-is, or should adapter enforce that paths stay under `cwd` before calling the ACP client?

## Recommended MVP decisions

- First tool: `acp_fs_read_text_file` only.
- Require `acp:tools` for tool relay.
- Den UI-created Code tokens include both `acp:chat` and `acp:tools`.
- Legacy/API-created ACP tokens without `acp:tools` remain chat-only until regenerated, migrated, or explicitly upgraded.
- No terminal or write descriptors in MVP even if client supports them.
- Use existing prompt SSE stream for Den -> adapter tool requests.
- Add `POST /acp/bears/{slug}/sessions/{session_id}/tool-results/{call_id}` for adapter -> Den results.
- Add Codepool internal `POST /internal/bear_channel/sessions/{session_id}/tool-results` for Den -> Codepool continuation.
- Audit metadata and result summaries; do not persist full file contents by default.
- Enforce a 30-second read timeout.
- Treat adapter disconnect as timeout/cancel and unblock Codepool.