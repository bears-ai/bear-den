# ACP tool reliability plan — historical Codepool waiter design

For the canonical role model and current role names, see [bear roles](../../architecture/bear-roles.md).
Status: historical / superseded for ACP direct mode. Do not use this as the active implementation plan for `pair` role ACP tools.

This plan documents reliability work for the older Codepool/Letta Code external-tool waiter relay. ACP direct local tools now use Den's direct Letta conversation API path and the Den ⇄ adapter tool-turn runtime. Use [`ACP_DIRECT_LOCAL_TOOL_RUNTIME_PLAN.md`](ACP_DIRECT_LOCAL_TOOL_RUNTIME_PLAN.md) for active implementation work. The diagnostics principles here remain useful background, but the Codepool waiter architecture is not the target for ACP direct mode.

## Historical goal

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

## Boring waiter simplification plan

### Goal

Make ACP client-tool relay deliberately boring: Codepool is the only owner of live tool waiters, Den is a stateless authenticated stream/proxy, and every expected tool outcome resolves through one result path.

### Target flow

1. Letta Code calls a Codepool ACP external tool.
2. Codepool creates the in-memory waiter before the request is visible outside Codepool.
3. Codepool emits one `client_tool_request` event on the active SSE stream.
4. Den maps that event to the adapter-facing ACP SSE event without persisting tool-call state.
5. The adapter calls the editor/client and POSTs the result to Den.
6. Den authenticates the ACP token/session, forwards the result directly to Codepool, and returns Codepool's delivery details.
7. Codepool resolves the waiter with `ok`, `error`, `cancelled`, or `timeout` and returns a structured tool result to Letta Code.

### Design decisions

- Codepool waiter state is intentionally process-local and non-durable.
- Den does not validate tool results against a pending-call DB table. It only authenticates the ACP token, resolves the ACP session to the Codepool session, validates the result status shape, and forwards.
- `request_id` + `call_id` remain the correlation identifiers between adapter, Den, and Codepool.
- Waiters are registered before SSE emission to avoid fast-result races.
- Expected outcomes (`ok`, `error`, `cancelled`, `timeout`) resolve to payloads; promise rejection is reserved for duplicate waiters and unexpected infrastructure/programmer failures.
- Request context for ACP tools is captured per Codepool request instead of stored in a session-scoped mutable map.

### Implementation tasks

- Add a Codepool waiter cancellation/removal API so failed SSE emission can clean up a pre-registered waiter.
- Update Codepool ACP external tools to create the waiter before `emit(...)` and to clean it up if emission fails.
- Normalize abort/cancellation into a `cancelled` payload instead of rejecting the waiter promise.
- Remove the session-scoped `acpToolContexts` map in Codepool and capture immutable per-request context in the tool closures.
- Add Codepool tests for the fast-result race: a fake `emit(...)` immediately delivers the result, and the tool still succeeds.
- Simplify Den's ACP tool result endpoint so it does not query or update `acp_client_tool_calls`.
- Remove Den stream-side persistence of `client_tool_request` events; keep logging and event mapping.
- Remove the Den `acp_client_tools` core module from the code path.
- Add a new migration that drops `acp_client_tool_calls`. Do not edit the existing migration that created it.
- Update migration docs to mention the drop migration.
- Run Codepool tests/typecheck and Den tests/checks as far as practical.

### Acceptance

- A tool result that arrives while `emit(...)` is still unwinding can still resolve the Codepool waiter.
- Codepool has no request-context map keyed only by session id.
- Den compiles without references to `acp_client_tool_calls` runtime helpers.
- New installations create then drop the obsolete table through migrations; existing installations drop it safely through the new migration.
- Existing ACP tool result responses still expose `delivered`, `reason`, and `runtime_id` diagnostics from Codepool.

## Prioritized future improvement plan

The boring waiter simplification makes Codepool the single owner of live ACP client-tool waiters and keeps Den stateless. Future ACP reliability work should focus on lifecycle cleanup and observability rather than reintroducing durable Den-side pending-call state.

### Priority 1: Bulk waiter cleanup by request/session

Goal: prevent orphaned waiters when the active stream, request, or session lifecycle ends before a tool result arrives.

Tasks:

- Add `AcpToolResultRegistry` helpers:
  - `cancelWaitersForRequest(sessionId, requestId, message)`
  - `cancelWaitersForSession(sessionId, message)`
  - optionally `listWaitersForRequest(sessionId, requestId)`
- Use these helpers from Codepool when:
  - the HTTP request closes
  - the SSE response errors
  - the stream handler exits abnormally
  - Codepool receives a session cancellation request
  - Codepool closes/tears down a channel session
- Resolve these waiters as normal `cancelled` or `error` payloads rather than leaving them to timeout.
- Add tests for request-close and session-cancel cleanup.

Acceptance:

- A disconnected ACP client does not leave waiters pending until timeout.
- Cancelling an ACP session resolves all active tool waiters for that session.
- Waiter cleanup logs include session id, request id, call id count, and reason.

### Priority 2: Short-lived recently-settled waiter cache

Goal: make late, duplicate, or retried tool results diagnosable after the live waiter is gone.

Tasks:

- Add a small in-memory TTL/LRU cache of recently settled waiter keys.
- Record settled metadata for a few minutes after `ok`, `error`, `cancelled`, or `timeout`:
  - session id
  - request id
  - call id
  - tool name
  - final status
  - settled timestamp
  - timeout vs explicit result/cancel
- Extend `deliverResult(...)` delivery reasons where possible:
  - `delivered`
  - `no_waiter`
  - `already_settled`
  - `expired_recently`
- Include the richer reason in Codepool logs and Den result responses.
- Add tests for duplicate result after success and late result after timeout.

Acceptance:

- A duplicate result immediately after success is distinguishable from a result for an unknown call.
- A result arriving shortly after timeout is visible as recently expired/settled.
- The cache remains bounded by size and TTL.

### Priority 3: ACP client-tool metrics

Goal: make ACP reliability visible without log archaeology.

Tasks:

- Add Prometheus metrics for:
  - `acp_client_tool_requests_total{tool,status}`
  - `acp_client_tool_wait_duration_seconds{tool,status}`
  - `acp_client_tool_timeouts_total{tool}`
  - `acp_client_tool_cancellations_total{tool}`
  - `acp_client_tool_no_waiter_results_total{reason}`
  - `acp_client_tool_pending_waiters`
- Record wait duration from waiter creation to settlement.
- Expose pending waiter gauge from the registry.
- Add dashboard/runbook notes once production metric names stabilize.

Acceptance:

- We can tell whether failures are timeouts, cancellations, no-waiter results, or Codepool forwarding failures.
- We can see which ACP tools are slow or unreliable.
- Pending waiter count is observable during active ACP sessions.

### Priority 4: Stronger Codepool result validation

Goal: keep Den stateless while validating results at the live waiter owner.

Tasks:

- Compare optional result `tool_name` against waiter metadata and log/reject mismatches.
- Compare optional `conversation_id` against waiter metadata and log mismatches.
- Add result-size validation and clearer errors for oversized payloads.
- Keep Den's role limited to auth, session binding, shape validation, and forwarding.

Acceptance:

- Incorrect `tool_name` or `conversation_id` cannot silently resolve the wrong waiter.
- Oversized or malformed tool results get clear diagnostics.

### Priority 5: Per-tool timeout policy

Goal: avoid forcing every ACP client tool into one global timeout.

Tasks:

- Allow descriptor-level timeout defaults.
- Keep `ACP_CLIENT_TOOL_TIMEOUT_MS` as a global default or cap.
- Add per-tool timeout overrides for likely future tool classes:
  - file read/write
  - search
  - terminal or long-running tools
- Include timeout policy in logs and waiter metadata.

Acceptance:

- Fast tools retain short timeouts.
- Future slower tools can have explicit longer timeouts without weakening all tools.

### Priority 6: Adapter retry policy for result delivery

Goal: prevent blind retries and make adapter behavior match Codepool/Den result reasons.

Tasks:

- Treat `delivered: true` as final success.
- Treat `delivered: false, reason: "no_waiter"` or future `already_settled` as non-retryable.
- Treat `codepool_forward_error` as potentially retryable with bounded backoff.
- Treat auth errors and session-not-found errors as non-retryable.
- Log retry decisions with request id and call id.

Acceptance:

- The adapter does not create retry storms for stale/no-waiter results.
- Transient Den-to-Codepool failures can still be retried safely.

### Priority 7: Client-visible cancellation/expiry events

Goal: help ACP clients abandon local work when Codepool no longer needs a tool result.

Tasks:

- Investigate whether the ACP stream can express tool cancellation or expiry events naturally.
- If protocol-compatible, emit events such as `client_tool_cancelled` or `client_tool_expired` when Codepool cancels waiters.
- If not protocol-compatible, keep this as logging/diagnostics only.

Acceptance:

- The adapter/client can stop unnecessary local work where the protocol allows it.
- No custom event is introduced unless clients can safely ignore or support it.

## Progress log

- Completed boring waiter simplification pass:
  - Codepool now registers ACP client-tool waiters before emitting `client_tool_request` over SSE.
  - Codepool cancellation now resolves as a `cancelled` payload instead of rejecting expected control flow.
  - Codepool has explicit waiter cleanup APIs for failed request emission.
  - Removed the session-scoped `acpToolContexts` map; ACP tool closures now capture immutable per-request context.
  - Added a fast-result regression test where `emit(...)` delivers a result before returning.
  - Den no longer persists ACP client tool calls or validates results against `acp_client_tool_calls`.
  - Den now resolves ACP session binding and forwards tool results directly to Codepool.
  - Removed Den stream-side `client_tool_request` persistence; the stream still logs and maps events.
  - Removed the Den `acp_client_tools` runtime module.
  - Added `20260502120000_drop_acp_client_tool_calls.up.sql` to drop the obsolete table.
  - Adapter now includes `tool_name` in result POST payloads so Den can continue forwarding diagnostic metadata without the DB row.
  - Validated with Codepool tests, Den `cargo check`, Den ACP unit tests, adapter `cargo check`, and project diagnostics.
- Created plan.
- Began Phase 1 adapter dispatcher refactor:
  - Added a single stdin reader task that sends parsed JSON-RPC values over an internal channel.
  - Replaced direct tool-response stdin reads with channel receives in `wait_for_json_rpc_response`.
  - Preserved current sequential request handling while removing duplicate stdin readers.
  - Adapter compiles after this intermediate step.
  - Added pending response map with `oneshot` waiters for adapter-to-client requests.
  - Stdin reader now routes JSON-RPC responses directly to pending waiters before forwarding requests/notifications to the main handler.
  - File read and file write requests now use `send_client_request_with_waiter` instead of consuming the inbound channel directly.
  - Removed adapter-initiated `session/request_permission` before writes; Zed's `fs/write_text_file` request is the local approval boundary and the extra permission round-trip caused repeated-turn write calls to time out before the actual file request was sent.
  - Remaining Phase 1 work: cancellation propagation to active prompt, adapter unit tests for dispatcher routing, and manual Zed verification.
- Began Phase 2 Codepool waiter diagnostics:
  - Added waiter metadata and `listWaiters()`.
  - Changed `deliverResult` to return `{ delivered, reason }` instead of a boolean.
  - Added Codepool runtime id and included it in continuation responses/logs.
  - Added internal `GET /internal/bear_channel/tool-waiters` endpoint.
  - Updated Codepool tests for delivery reasons.
  - Fixed warm-session stale request context by storing latest ACP tool context per Codepool channel session; reused external tool closures now read the current Den request id instead of the first-turn request id.
  - Added Codepool logs when ACP external tools emit requests and when their waiter receives results.
  - Made ACP client tool timeout configurable with `ACP_CLIENT_TOOL_TIMEOUT_MS` and lowered default to 15s while debugging repeated-turn stalls.
  - Made ACP client tool request emission await the SSE `res.write` callback/drain path before starting the Codepool waiter, and added `acp_client_tool_request_sse_written` logs.
  - `acp_client_tool_request_sse_written` now includes write duration and whether Node reported response backpressure.
  - Added `GET /internal/bear_channel/debug` for Codepool runtime id, pending tool waiters, waiter metadata, and pool stats.
  - Restored default `ACP_CLIENT_TOOL_TIMEOUT_MS` to 30s; it remains configurable for experiments.
  - Added bounded `recoverPendingApprovals` attempt when Letta Code emits `approval_request_message` during a streaming turn. Logs `letta_code_approval_request_recovery` and the recovery result/error.
  - Moved approval recovery behind `ACP_APPROVAL_RECOVERY_ENABLED` (default false) to reduce control-traffic noise during ACP tool debugging.
  - Added dedicated `letta_code_approval_request_event` logging with extracted approval tool names and a redacted/truncated preview.
  - Added ACP strict client-tools mode (`ACP_STRICT_CLIENT_TOOLS`, default true) to deny Codepool-local filesystem tools and steer workspace access to ACP client tools.
- Began Phase 3 Den continuation diagnostics:
  - Den now deserializes Codepool `reason` and `runtime_id` fields.
  - Den tool-result response now includes `reason` and `runtime_id` for adapter/proxy-visible diagnostics.
- Completed initial Phase 4 model-facing timeout clarity:
  - Codepool ACP external tools now return explicit text for `timeout`, `cancelled`, and non-ok tool results.
  - Timeout text explains this is a local editor tool delivery failure, not evidence that the file is absent.
