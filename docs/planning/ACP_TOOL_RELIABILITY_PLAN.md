# ACP tool reliability plan

## Goal

Make ACP local editor tools reliable enough for end-to-end Zed testing. The immediate focus is reducing false tool timeouts and making every timeout diagnosable.

The target flow is:

1. Letta Code calls a BEARS-provided ACP external tool.
2. Codepool emits `client_tool_request` and waits for a continuation result.
3. Den persists/streams the request to the adapter.
4. The adapter calls Zed's ACP client method (`fs/read_text_file` / `fs/write_text_file`).
5. The adapter posts the result to Den.
6. Den forwards to Codepool.
7. Codepool resolves the waiter and returns a clear tool result to Letta Code.

## Current suspected failure modes

- Adapter reads stdin synchronously while waiting for a tool response. This is fragile with interleaved `session/cancel`, responses, and other client messages.
- Codepool waiters are in-memory. A restart, timeout, or identifier mismatch results in `delivered=false` or timeout without enough context.
- Tool timeout messages are model-visible but not sufficiently specific about which infrastructure hop failed.
- Large tool results or proxy failures can cause Den result-post failures that look like generic prompt failures.

## Implementation plan

### Phase 1: Adapter message dispatcher

Refactor the adapter so only one task reads stdin.

- Add a stdin reader task that parses every JSON-RPC line into `serde_json::Value` and sends it over an internal channel.
- Replace all direct `lines.next_line()` calls outside the reader with channel receives.
- Update tool waiters to wait on the channel for matching response IDs.
- While waiting for a tool response, handle `session/cancel` for the active session immediately and return a cancellation error/result.
- Preserve existing request handling for `initialize`, `authenticate`, `session/new`, `session/prompt`, `session/close`, and `session/cancel`.
- Add adapter stderr logging for ignored/interleaved messages, tool response wait starts, timeouts, cancellation, and matched responses.

Acceptance:

- Adapter compiles.
- Chat still works.
- Read/write tool requests no longer rely on directly reading stdin in tool functions.
- `session/cancel` observed during a tool wait stops the wait promptly.

### Phase 2: Codepool waiter diagnostics

Improve visibility into continuation waiters.

- Give Codepool a process-local `runtime_id`.
- Track waiter metadata: session id, request id, call id, tool name, created time, timeout, conversation id.
- Return a delivery reason from `deliverResult`, e.g. `delivered`, `no_waiter`.
- Log continuation receipt with runtime id, delivery result, pending waiter count, and result size.
- Add internal debug endpoint for current waiters.
- Update tests for delivered and undelivered result cases.

Acceptance:

- Codepool tests cover delivery and no-waiter reason.
- Logs identify whether a result arrived after the waiter disappeared.

### Phase 3: Den continuation diagnostics

Make Den's tool result endpoint report delivery details clearly.

- Preserve Den's current behavior of accepting a tool result even if Codepool delivery fails.
- Include Codepool delivery reason in the response when available.
- Persist/log Codepool delivery failures with call id, request id, session id, tool name, and result size.

Acceptance:

- Adapter errors include Den response details.
- Den logs distinguish proxy/Den receipt from Codepool continuation failure.

### Phase 4: Model-facing timeout clarity

Make timeout/error tool results explicit and less confusing.

- If Codepool waiter times out, return text explaining that the local editor tool did not return a result before timeout.
- If Den/Codepool delivery fails after local success, surface that as infrastructure delivery failure, not file absence.

Acceptance:

- The bear should no longer describe a timeout as a normal successful local file read.

## Out of scope for this pass

- Durable cross-restart tool waiters.
- Terminal tools.
- General MCP-over-ACP.
- Making ACP sessions canonical conversations.

## Progress log

- Created plan.
- Began Phase 1 adapter dispatcher refactor:
  - Added a single stdin reader task that sends parsed JSON-RPC values over an internal channel.
  - Replaced direct tool-response stdin reads with channel receives in `wait_for_json_rpc_response`.
  - Preserved current sequential request handling while removing duplicate stdin readers.
  - Adapter compiles after this intermediate step.
  - Remaining Phase 1 work: true concurrent pending response map/oneshot dispatcher, cancellation propagation to active prompt, and tests/manual Zed verification.
- Began Phase 2 Codepool waiter diagnostics:
  - Added waiter metadata and `listWaiters()`.
  - Changed `deliverResult` to return `{ delivered, reason }` instead of a boolean.
  - Added Codepool runtime id and included it in continuation responses/logs.
  - Added internal `GET /internal/bear_channel/tool-waiters` endpoint.
  - Updated Codepool tests for delivery reasons.
- Began Phase 3 Den continuation diagnostics:
  - Den now deserializes Codepool `reason` and `runtime_id` fields.
  - Den tool-result response now includes `reason` and `runtime_id` for adapter/proxy-visible diagnostics.
