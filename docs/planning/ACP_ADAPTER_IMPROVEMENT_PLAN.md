# ACP Adapter Improvement Plan

Status: active follow-up plan after initial ACP direct local tool rollout.

This plan captures the current state, known caveats, and next steps for hardening the BEARS ACP adapter / Den direct local tool runtime. It intentionally focuses on adapter resilience and continuation behavior, not expanding the toolset.

## Current milestone

We have confirmed real end-to-end ACP tool operation in Zed:

1. Den advertises adapter-supported local tools via `client_tools`.
2. Letta emits ACP local tool calls.
3. Den maps tool calls to adapter-facing `tool_request` SSE events.
4. The adapter requests Zed local approval.
5. Zed approval UI appears.
6. The adapter executes local tools.
7. The adapter posts results to Den.
8. Den returns tool results to Letta.
9. Zed sees completed tool-call updates with useful result previews.

Confirmed tools include:

- `fs_list_directory`
- `fs_read_text_file`
- `fs_edit_file`

## Recent reliability fixes

### Capability negotiation

The adapter now sends both a legacy and a structured capability shape:

```json
{
  "direct_tools": {
    "fs_read_text_file": true,
    "fs_list_directory": true,
    "fs_search_files": true,
    "fs_edit_file": true
  },
  "adapter": {
    "name": "bears-acp-adapter",
    "version": "...",
    "direct_tools": {
      "fs_read_text_file": { "supported": true, "version": 1 },
      "fs_list_directory": { "supported": true, "version": 1 },
      "fs_search_files": { "supported": true, "version": 1 },
      "fs_edit_file": { "supported": true, "version": 1 }
    }
  }
}
```

Den filters `client_tools` from that capability context. If no capabilities are present, Den falls back to `fs_read_text_file` only.

### Adapter JSON-RPC response routing

The adapter now routes JSON-RPC responses at the stdin-reader boundary into a shared `pending_responses` map, so local adapter requests such as `session/request_permission` can be resolved even while prompt handling is awaiting a result.

This fixed the previous deadlock:

1. adapter sent `session/request_permission`;
2. Zed returned approval;
3. adapter main loop was blocked and failed to route the response;
4. Den timed out waiting for a tool result.

### Tool result preview in Zed

On successful local tool execution, the adapter now includes a capped preview of the tool result in the Zed tool-call update instead of only saying `Local tool completed`.

For example, `fs_list_directory` completion should show the directory listing in the tool call UI.

### Prompt guidance

Den's ACP prompt context now clarifies:

- only tools actually callable this turn are mentioned;
- `fs_edit_file` invocation is how the model requests local approval;
- the model should not ask for edit approval in chat;
- for edits, the model should read/discover, call the edit tool, verify with read, and summarize.

## Known current caveat: continuation behavior

The main remaining issue is continuation after tool results.

Observed behavior:

- The agent can list/read/patch successfully.
- After intermediate tool calls, it may stop and wait for the user to type `continue`.
- It may claim it is blocked by approval even though `fs_edit_file` is callable.

This appears to be primarily model / tool-loop behavior rather than basic ACP transport failure.

## Important rollback: do not inject instructions into tool return content

We tried appending a continuation instruction to the `tool_return` content sent back to Letta. This caused undesirable tool-loop behavior, including attempts to call unrelated tools such as `bash`.

Decision:

- Tool return content must remain data, not instructions.
- Continuation nudges should live in system/prompt context or a clearly separate protocol message, not inside tool output.

## Letta / Letta Code research notes

### Letta Code SDK flow

Codepool uses Letta Code SDK sessions rather than raw REST tool-return loops:

```text
sessionOpts.tools = tools
sessionOpts.allowedTools = tools.map(tool => tool.name)
sessionOpts.permissionMode = "bypassPermissions"
sessionOpts.canUseTool = canUseRegisteredTool(tools)

session = createSession(...) or resumeSession(...)
await session.send(userText)
for await (const msg of session.stream()) yield msg
```

The SDK appears to own more of the tool loop. Codepool mostly streams SDK messages out.

### Approval events in Codepool

Codepool treats some Letta approval protocol events as recoverable/noisy:

- `approval_request_message` often maps to no downstream event;
- `stop_reason = requires_approval` often maps to no downstream event;
- optional `session.recoverPendingApprovals({ timeoutMs: 5000 })` exists behind `ACP_APPROVAL_RECOVERY_ENABLED`.

### Raw Letta API tool-return shape

Letta's documented preferred client tool return shape is:

```json
{
  "type": "tool_return",
  "tool_returns": [
    {
      "type": "tool",
      "status": "success",
      "tool_call_id": "...",
      "tool_return": "..."
    }
  ]
}
```

Den was changed to use this shape even for approval-originated client tool calls. Zed/ACP owns local approval; Letta gets the tool result.

## Current design direction

### Treat tool results as valid UI output

Letta may acknowledge a client tool result with `tool_return_message` but not produce final assistant prose in the same continuation. The adapter should not treat that as an error.

Zed should still show useful tool results in the tool-call UI.

### Keep continuation as a separate focused problem

Do not keep adding transport hacks to force the model to continue. Collect examples and handle continuation later as a model prompting / Letta runtime behavior project.

Potential future approaches:

1. Stronger system prompt / agent instructions about autonomous tool loops.
2. A separate clearly scoped continuation message after tool return, not embedded in tool output.
3. A workboard/checklist mechanism that reminds the agent of incomplete tasks.
4. Investigate whether Letta exposes a better run/step continuation API analogous to Letta Code SDK sessions.
5. Consider whether `max_steps`, `include_return_message_types`, or client-tool configuration affects post-tool assistant generation.

## Adapter resilience refactor backlog

The adapter has improved but still needs structural hardening.

### 1. Extract a JSON-RPC transport abstraction

Create a dedicated transport layer around stdin/stdout JSON-RPC:

```rust
struct JsonRpcTransport {
    pending: Arc<Mutex<HashMap<String, oneshot::Sender<Value>>>>,
    inbound_requests: mpsc::Receiver<JsonRpcRequest>,
}

impl JsonRpcTransport {
    async fn request(&self, method: &str, params: Value, timeout: Duration) -> Result<Value>;
    async fn notify(&self, method: &str, params: Value) -> Result<()>;
    async fn respond(&self, id: Option<Value>, result: Result<Value, Value>) -> Result<()>;
}
```

Goals:

- single owner for request IDs;
- consistent timeout handling;
- no scattered `write_json` calls;
- easy tests for request/response routing.

### 2. Make tool tasks first-class

Tool requests should be represented as task state:

```text
received
permission_requested
permission_granted | permission_denied | permission_timeout
execution_started
execution_succeeded | execution_failed
result_posted | result_post_failed
```

The current background task wrapper is a step in this direction, but task lifecycle should become explicit and testable.

### 3. Normalize local tool statuses

Introduce a typed status enum in the adapter:

```rust
enum LocalToolStatus {
    Ok,
    Error,
    PermissionDenied,
    Timeout,
    Cancelled,
    Unsupported,
}
```

All Den result posts should derive from this enum.

### 4. Improve local error packet consistency

Every adapter failure should include:

```json
{
  "component": "bears-acp-adapter",
  "phase": "...",
  "session_id": "...",
  "tool_call_id": "...",
  "tool_name": "...",
  "status": "...",
  "message": "...",
  "hint": "..."
}
```

### 5. Add concurrency / runtime tests

Current unit tests cover tool logic well but not runtime concurrency. Add tests for:

- permission approval response while prompt/tool handler is awaiting;
- permission denial response;
- permission timeout;
- unrelated client request during pending permission;
- Den tool result endpoint accepts late/duplicate results with clear diagnostics;
- tool return acknowledgement with no assistant prose;
- adapter does not crash if local tool task panics/fails.

### 6. Cancellation handling

Handle `session/cancel` while a tool request is pending:

- cancel local tool task if possible;
- post `cancelled` to Den if a tool call is active;
- clean pending response waiters;
- surface cancellation clearly to Zed.

## Operational debugging checklist

When ACP tool availability seems wrong:

1. Check adapter startup/session logs for `direct_tools`.
2. Check Den descriptor log:
   - `phase="descriptor_advertised"`
   - `tools=[...]`
3. If Den advertises only `fs_read_text_file`, inspect adapter `client_context` / session context.
4. If Den advertises `fs_edit_file` but model says unavailable, inspect Letta `client_tools` request payload / provider behavior.

When tool approval appears but tool result never posts:

1. Check adapter `requesting permission` log.
2. Check for post-approval execution log (`list_directory`, `read_text_file`, etc.).
3. If absent, suspect JSON-RPC response routing or permission response shape.
4. If execution log appears but no `posted tool result`, inspect Den result endpoint response.

When tool succeeds but agent stops:

1. Confirm Zed tool-call UI contains useful result preview.
2. Confirm Den stream summary has `tool_return_message` / `status_text` and no error.
3. Treat as continuation behavior, not adapter transport failure.

## Next recommended work

Do not add more tools until the current adapter loop has baked.

Recommended next slices:

1. Extract `JsonRpcTransport`.
2. Add runtime tests for permission response routing.
3. Add cancellation handling.
4. Collect continuation examples and design a separate continuation strategy.
