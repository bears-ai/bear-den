# ACP Lifecycle Reset Plan

## Status

Active implementation. This plan supersedes narrow ACP lifecycle patching as the near-term reliability strategy for `pair` over ACP.

### Progress snapshot

Completed:

- Added `AcpTurnController` in `services/den/src/core/acp_turn_controller.rs`.
- Added pure lifecycle tests for text-only completion, adapter-local tool waits, Den-server tools, unsupported tools, timeout, cancellation, late results, orphaned approvals, status snapshots, and deduped status updates.
- Added rigid Letta continuation payload tests for successful approvals, failed/timeout denials, and no-approval `tool_return` messages.
- Added Den stream integration tests for adapter-local continuation, no terminal before local tool result, `session_info` Den-server routing, pending local tool timeout, and pending local tool cancellation.
- Added test-configurable ACP local tool timeout via `BEARS_ACP_TOOL_TIMEOUT_MS`.
- Wired `AcpTurnController` into `AcpLettaSseStream` as an observer and terminal-gating participant.
- Added controller snapshot data to stream terminal diagnostics.
- Cleaned Den-server route duplication by routing through active `route_direct_den_tool_request` / `route_web_fetch_tool_request` helpers while preserving `web_fetch` policy behavior.
- Verified key lifecycle tests and `cargo check --manifest-path services/den/Cargo.toml --lib` pass.

Still in progress:

- Full replacement of legacy stream lifecycle fields with controller authority.
- Real production `/cancel` endpoint signaling into active streams; current stream cancel signal is test/integration-hook level.
- Normalized late-result API statuses replacing scary compatibility responses such as `turn_missing`.
- User-facing session health/status surfaces.
- Adapter-side overlap, mode-race, MCP-log, and cancellation tests.
- Slow `session_info` stream test cleanup; it currently exercises a DB-unavailable branch and takes about 60 seconds. Attempted to make `session_info` degrade on member-count and empty MemFS config, but the stream test still takes ~60s, likely due another lazy DB query path.

## Summary

BEARS has been treating several ACP issues as independent bugs: premature terminal events, late local tool results, stale Letta approvals, session lifecycle races, cancellation overlap, MCP tool-surface noise, and history replay drift. These are symptoms of one missing abstraction: an explicit, testable prompt-turn lifecycle.

The reset does **not** mean rewriting Den, Letta integration, memory, tools, or the adapter wholesale. It means rebuilding the ACP-facing session/turn/tool lifecycle around a small state machine, using the official ACP Rust crate for protocol grounding and taking implementation inspiration from successful ACP agents such as Goose and fast-agent.

## References and inspiration

### Official ACP Rust crate

Use `agent-client-protocol` as the protocol correctness reference.

Priorities:

- Prefer official ACP types and lifecycle shapes over hand-rolled JSON where practical.
- Validate stop reasons, session updates, permission requests, tool-call statuses, and session metadata against the SDK model.
- Use SDK examples to check ordering of `session/new`, notifications, `session/prompt`, `session/cancel`, and prompt responses.

### Goose

Use Goose as the primary architectural inspiration for a stateful ACP agent harness.

Relevant patterns to study:

- multiple concurrent ACP sessions with isolated state;
- mapping ACP sessions to persisted agent sessions/history;
- client-forwarded MCP server handling;
- client file/terminal delegation;
- prompt lifecycle ownership in a stateful runtime.

### fast-agent

Use fast-agent as the secondary inspiration for a compact ACP turn runner.

Relevant patterns to study:

- cancellation of streaming LLM work;
- tool/workflow progress updates;
- permission request flow and remembered approvals;
- local shell versus client terminal execution abstraction;
- slash commands and lightweight session operations;
- status/diagnostic commands that make runtime state understandable to users.

## Principles

### 1. ACP prompt turns are request/response lifecycles, not loose streams

A `session/prompt` request owns exactly one prompt turn. Streaming updates are subordinate to that request. The prompt response must not be produced until the turn has reached a terminal state.

### 2. Exactly one terminal outcome per prompt turn

Every prompt turn ends once, with one terminal outcome:

- `ok`
- `failed`
- `cancelled`
- `recovered`
- `needs_new_session`

Duplicate terminal events are bugs. Late internal events after terminal state must be ignored or recorded as diagnostics, never allowed to resurrect the turn.

### 3. Terminal response is gated by obligations

A turn cannot be terminal while required obligations are pending.

Obligations include:

- adapter-local tool execution;
- client permission requests;
- Den-server tool execution that must be returned to Letta;
- unsupported tool settlement;
- cancellation cleanup;
- timeout settlement.

Core invariant:

```text
terminal prompt result requires all turn obligations to be settled, cancelled, failed, or timed out
```

### 4. Tool execution route is separate from approval semantics

A tool may require model/provider approval and still be Den-executed, adapter-local, or unsupported. These concepts must remain independent.

Execution route:

- `DenServer`
- `AdapterLocal`
- `Unsupported`

Approval state:

- approval not required;
- approval requested;
- approval granted;
- approval denied;
- approval stale/orphaned.

Do not infer execution route from approval flags.

### 5. Den is authoritative for turn obligations

Den owns the canonical turn lifecycle and obligation ledger because Den bridges Letta state, Bear policy, session binding, and adapter-local result posting.

The adapter should be a UI/client-tool executor bridge:

- display tool calls;
- request client permission;
- execute adapter-local tools;
- call MCP tools forwarded by the ACP client;
- post results back to Den;
- tolerate late-result acknowledgements.

The adapter should not decide whether the Den/Letta turn is still alive.

### 6. Cancellation is a state transition, not an error path

Cancellation must:

- mark the prompt turn as cancelling/cancelled;
- cancel or settle outstanding obligations;
- stop Letta work when possible;
- respond to `session/prompt` with a cancelled stop reason;
- ignore late tool results and late stream frames.

Cancellation errors from child tasks should be caught and normalized.

### 7. Session lifecycle notifications must not race session creation

The adapter must not emit session notifications that clients can observe before the client has accepted the session returned from `session/new` or `session/load`.

Session updates, mode notifications, title updates, and command updates should either:

- be included in the session setup response where supported; or
- be queued until after setup response completion.

### 8. Stateful backend recovery is explicit and bounded

Letta stale approval/run recovery should be explicit, visible, and bounded. Recovery should not silently rebind sessions or loop indefinitely.

Recovery ladder:

1. settle the current obligation normally;
2. timeout/deny/error the obligation;
3. cancel stale run/approval if clearly orphaned;
4. compact/recover when supported;
5. stop with a visible retry/new-session recommendation.

### 9. MCP and tool surfaces should be summarized and scoped

MCP tool descriptors are important for model behavior, but logs and prompts should not dump unbounded schemas by default.

Operational logs should record:

- server count;
- tool count;
- server names/statuses;
- tool names;
- descriptor byte counts/hashes if needed.

Full schemas belong behind explicit debug logging.

### 10. User-visible state is part of lifecycle correctness

A prompt turn should not feel like an opaque spinner. The user should be able to tell whether BEARS is thinking, waiting on a local tool, waiting for permission, continuing after a tool result, cancelling, recovering, or blocked.

Lifecycle state should therefore produce concise UX status, not only logs. Status should be terse, deduplicated, and distinct from final assistant answer text.

### 11. Session health and context pressure should be visible

BEARS should expose a compact session health view, inspired by Zed's built-in context-budget UX and fast-agent/Goose status surfaces.

Useful health fields include:

- active turn phase;
- pending tool/permission counts;
- requested and effective mode;
- conversation binding;
- work-surface resolution status;
- MCP server/tool summary;
- last recovery attempt/outcome;
- context budget if exact or estimated usage is available.

Context budget must be honestly labeled as `exact`, `estimated`, or `unavailable`; never show fake precision.

### 12. Tests precede refactor

The reset starts by documenting executable behavior through focused tests. The implementation should then move code behind a state machine until the tests are boring.

## Priorities

### Priority 0: Stop widening the patch surface

Avoid adding more one-off stream/adapter fixes unless needed to keep the system usable. New behavior should be expressed as lifecycle tests first.

### Priority 1: Establish the lifecycle test suite

Status: mostly complete for Den lifecycle and stream behavior. Adapter-side lifecycle tests remain.

Create a small fake harness that does not require live Zed or live Letta.

The harness should simulate:

- ACP prompt request;
- Den stream frames;
- Letta tool requests/stops/continuations;
- adapter-local result delivery;
- cancellation;
- timeout;
- late results.

### Priority 2: Introduce `AcpTurnController`

Status: initial implementation complete; deeper stream authority still in progress.

Add a small core type that owns turn phase, obligations, terminal gating, cancellation, timeout, and late-result handling.

This should begin as a pure or mostly pure Rust state machine with unit tests before being wired into HTTP/SSE code.

The controller should also expose a stable status snapshot for UX and diagnostics, including phase, open obligation counts, route-specific counts, terminal status, and late-result/recovery flags.

### Priority 3: Wire Den stream handling through the controller

Status: partially complete. The controller observes stream/tool/cancel lifecycle and participates in terminal gating, but legacy fields still exist.

Replace scattered lifecycle fields with controller calls.

Current transitional state to consolidate:

- `active_tool_call_ids`
- `AcpToolTurnCoordinator`
- `pending_tool_result`
- `deferred_turn_result`
- duplicate terminal guards
- cancellation cleanup branches

### Priority 4: Align adapter with ACP Rust crate patterns

Status: not started beyond documentation/research. Den-side lifecycle tests took priority.

Use the ACP Rust crate and examples to reduce hand-rolled lifecycle handling in the adapter where practical.

Near-term adapter changes should focus on:

- prompt response gating;
- session notification ordering;
- cancellation normalization;
- late-result tolerance;
- compact tool/MCP logging.

### Priority 5: Add session health/status UX

Status: planned. Controller snapshots exist in stream diagnostics, but they are not yet exposed through `session_info`, `/status`, or user-facing status updates.

Add a visible status surface once the controller snapshot exists.

Initial options:

- enhance `session_info` with runtime/session health;
- add or improve `/status` slash command output;
- emit concise status updates on meaningful turn phase transitions;
- include MCP summary and mode/work-surface state.

Context budget visibility is desirable but can be added after we determine whether Letta/provider usage data is exact enough. If only Den estimates are available, label them as estimates.

### Priority 6: Durable obligation ledger later

Status: deferred.

Keep the initial controller in-memory. Add a Postgres-backed obligation ledger only after behavior stabilizes.

Durability matters for process restarts and operator diagnostics, but adding persistence before the lifecycle is stable will make the refactor harder.

## Target architecture

### `AcpTurnController`

Conceptual shape:

```text
AcpTurnController {
  turn_id
  request_id
  acp_session_id
  conversation_id
  phase
  obligations
  terminal
  diagnostics
}
```

Phases:

```text
Created
Streaming
WaitingForObligations
ContinuingAfterTool
Cancelling
Terminal
```

Obligation states:

```text
Pending
Running
Settled
Failed
TimedOut
Cancelled
LateIgnored
```

Execution routes:

```text
DenServer
AdapterLocal
Unsupported
```

Terminal decisions:

```text
None
Ready(status, reason)
Emitted(status, reason)
```

### Required controller operations

```text
on_stream_started()
on_tool_request(tool_call_id, tool_name, route, approval_state)
on_den_tool_settled(tool_call_id, result)
on_adapter_tool_result(tool_call_id, result)
on_tool_timeout(tool_call_id)
on_requires_approval_stop()
on_stream_end(stop_reason)
on_stream_error(error)
on_cancel(reason)
on_late_tool_result(tool_call_id, result)
may_emit_terminal()
take_terminal_event()
diagnostics()
```

### Turn status snapshot

`AcpTurnController` should expose a compact snapshot that can feed diagnostics, `session_info`, `/status`, and status updates.

Conceptual shape:

```text
AcpTurnStatusSnapshot {
  phase
  open_obligations
  pending_adapter_tools
  pending_den_tools
  pending_permissions
  terminal_status
  terminal_reason
  orphaned_requires_approval
  late_results_ignored
}
```

This snapshot should be safe to expose: it contains lifecycle metadata, not raw tool payloads or sensitive file contents.

### Den/adapter boundary

Den should return structured result-post responses:

```text
accepted
late_result_ignored
turn_cancelled
turn_timed_out
unknown_turn
unknown_tool_call
```

`turn_missing` should become an internal compatibility fallback, not the primary operational status.

## UX plan

### Turn phase visibility

Map lifecycle phases to concise status updates.

Examples:

```text
Thinking…
Waiting for local tool result…
Continuing after tool result…
Cancelling turn…
Recovering stale model approval…
```

Status messages should be deduplicated and should not flood the conversation.

### Tool obligation visibility

Tool calls should expose the distinction between:

- Den-server tools;
- adapter-local tools;
- client-forwarded MCP tools;
- unsupported tools.

They should also expose permission and execution state:

```text
pending
waiting_for_permission
running
completed
failed
timed_out
cancelled
late_result_ignored
```

### Session health

Add a compact session health surface available through `session_info` and/or `/status`.

Suggested fields:

```text
session_id
conversation_id
requested_mode
effective_mode
active_turn.present
active_turn.phase
active_turn.pending_obligations
work_surface.status
work_surface.confidence
mcp.server_count
mcp.tool_count
last_recovery.status
context_budget.status
context_budget.used_tokens
context_budget.remaining_tokens
```

### Context budget

Investigate Letta/provider usage data before implementation.

If exact data is available, expose it as exact. If Den estimates usage from messages/tool payloads, expose it as estimated. If neither is available, expose `unavailable` rather than inventing numbers.

### Recovery UX

Recovery should be visible, bounded, and non-scary.

Preferred wording pattern:

```text
BEARS detected a stale model approval from the previous turn. Clearing it and retrying once…
```

Then either:

```text
Recovery succeeded. Continuing…
```

or:

```text
Recovery did not complete safely. Please start a new ACP session for this conversation.
```

### MCP UX

MCP availability should be summarized, not dumped.

Show:

- server names;
- server status;
- tool counts;
- selected tool names;
- privacy/security notes for high-sensitivity servers such as browser/devtools MCP.

For browser MCP, show once per session when relevant:

```text
Chrome DevTools MCP can inspect browser page content, console logs, network requests, and storage. Avoid using it on pages with sensitive data.
```

### Mode UX

Mode changes should show requested versus effective mode and policy reason when they differ.

Example:

```text
Mode: Write requested, Ask effective. Den session is not bound yet, so edits remain gated.
```

### Overlap/cancel UX

Same-conversation overlap should visibly explain cancellation/supersession:

```text
Previous turn cancelled because you sent a new message.
```

Different-conversation overlap should avoid confusing cross-talk by gating stale UI updates by turn token.

### Slash command UX

Prefer action-oriented slash commands:

- `/status`
- `/recover`
- `/cancel`
- `/tools`
- `/context`
- `/work-surface`
- `/mode`

`/status` should be the first target because it gives users and operators a single place to inspect health.

## Test plan

### Phase A: pure lifecycle tests

Add tests for the controller before changing stream code.

#### `acp_turn_text_only_completes_once`

Status: complete.

Expected:

- stream starts;
- assistant output occurs;
- stream ends;
- no obligations;
- exactly one terminal outcome.

#### `acp_turn_waits_for_adapter_local_tool_before_terminal`

Status: complete.

Expected:

- adapter-local tool request registers obligation;
- Letta emits `requires_approval` / stream end;
- terminal is not ready;
- adapter result settles obligation;
- continuation can start;
- terminal emits once after final stream end.

#### `acp_turn_den_server_tool_does_not_create_adapter_obligation`

Status: complete.

Expected:

- `session_info` / Den-server tool request is routed `DenServer`;
- no adapter-local obligation is registered;
- Den settles tool internally;
- no `tool_request` is emitted to adapter for `session_info`.

#### `acp_turn_unsupported_tool_settles_without_hanging`

Status: complete.

Expected:

- unsupported route creates deterministic failed settlement;
- no indefinite wait;
- terminal emits once.

#### `acp_turn_timeout_settles_pending_adapter_tool`

Status: complete.

Expected:

- adapter-local obligation remains pending;
- timeout transitions obligation to `TimedOut`;
- terminal error/recovered outcome becomes ready;
- late result is ignored.

#### `acp_turn_cancel_settles_pending_adapter_tool`

Status: complete.

Expected:

- adapter-local obligation pending;
- cancel transitions turn to cancelling/cancelled;
- outstanding obligation becomes `Cancelled`;
- terminal cancelled outcome emits once;
- late result is ignored.

#### `acp_turn_late_result_after_terminal_is_ignored`

Status: complete.

Expected:

- terminal already emitted;
- result arrives for known tool call;
- state records `LateIgnored` diagnostic;
- terminal remains unchanged.

#### `acp_turn_orphaned_requires_approval_triggers_recovery_path`

Status: complete.

Expected:

- `requires_approval` stop occurs with no matching active obligation;
- controller marks stale/orphaned approval;
- recovery terminal/status is produced once;
- no infinite cleanup loop.

#### `acp_turn_status_snapshot_reports_phase_and_obligations`

Status: complete.

Expected:

- controller snapshot includes current phase;
- open obligation count is accurate;
- adapter-local versus Den-server obligation counts are accurate;
- terminal fields are absent before terminal and present after terminal.

#### `acp_turn_status_updates_are_deduplicated`

Status: complete.

Expected:

- repeated transitions to the same user-visible status do not emit duplicate status text;
- meaningful phase changes do emit one concise status update.

### Phase B: Den stream integration tests

Adapt existing tests around `AcpLettaSseStream`.

#### `acp_stream_does_not_emit_terminal_before_local_tool_result`

Status: complete under current test name `acp_stream_does_not_emit_turn_result_before_local_tool_result`; keep and strengthen later if terminal event names are normalized.

Current targeted regression. Keep and strengthen it.

Expected:

- Letta emits local tool request plus `requires_approval` stop;
- before adapter result: no terminal event / no prompt completion;
- after adapter result: continuation is posted;
- exactly one terminal event.

#### `acp_stream_routes_session_info_as_den_server_tool`

Status: complete but slow; test currently takes about 60 seconds due to DB-unavailable `session_info` path.

Expected:

- Letta emits `session_info` tool call;
- Den classifies route as `DenServer`;
- adapter never receives local `tool_request`;
- Den returns tool result to Letta continuation;
- terminal emits once.

#### `acp_stream_timeout_pending_local_tool`

Status: complete.

Expected:

- local tool request emitted;
- adapter never posts result;
- timeout fires;
- Den settles/cancels Letta state;
- terminal failed/recovered emits once;
- no indefinite stream hang.

#### `acp_stream_cancel_pending_local_tool`

Status: complete for test-hook stream cancellation; production `/cancel` endpoint signaling remains.

Expected:

- local tool request emitted;
- Den cancel endpoint called;
- obligation cancelled;
- terminal cancelled emits once;
- late adapter result receives `late_result_ignored` or equivalent.

#### `acp_stream_late_tool_result_after_timeout_does_not_resurrect_turn`

Status: partially covered by `acp_stream_timeout_pending_local_tool`, which asserts a late result receives settled/missing delivery. Add a dedicated result API status test later.

Expected:

- timeout terminal emitted;
- adapter posts result later;
- Den returns late/ignored status;
- no continuation is posted to Letta;
- no new terminal event.

### Phase C: adapter tests

#### `adapter_defers_mode_update_until_den_session_exists`

Status: behavior implemented defensively; test not yet added.

Expected:

- Den mode endpoint returns `ACP session not found`;
- adapter responds without JSON-RPC failure;
- mode remains local/default;
- later prompt can bind session.

#### `adapter_summarizes_mcp_context_in_session_logs`

Status: behavior implemented defensively; test not yet added.

Expected:

- full MCP descriptors are present in runtime context;
- stderr summary includes counts/names;
- stderr does not include full schemas by default.

#### `adapter_same_conversation_overlap_cancels_previous_turn`

Status: not yet added.

Expected:

- prompt B starts in same conversation while prompt A active;
- adapter cancels/supersedes A;
- stale updates from A are dropped;
- B owns UI output.

#### `adapter_different_conversation_overlap_does_not_cancel_previous_runtime`

Status: not yet added.

Expected:

- prompt B starts in a different conversation;
- A is not cancelled solely because B exists;
- UI updates are token-gated.

#### `adapter_explicit_session_cancel_cancels_active_turn_and_tools`

Status: not yet added.

Expected:

- active tool task exists;
- `session/cancel` cancels it;
- prompt response resolves cancelled;
- permission requests receive cancelled where applicable.

### Phase D: UX/status tests

#### `session_info_includes_runtime_health_snapshot`

Status: not yet added. Requires active-turn runtime snapshot registry beyond stream-local controller.

Expected:

- `session_info` includes active turn presence/phase when a turn is active;
- pending obligation counts are present;
- no raw tool payloads or sensitive contents are exposed.

#### `slash_status_reports_session_health`

Status: not yet added.

Expected:

- `/status` reports mode, conversation binding, active turn phase, pending tools, MCP summary, work-surface status, and context budget status;
- output remains concise.

#### `context_budget_marks_estimates_as_estimated`

Status: not yet added. Requires Letta/provider usage data investigation.

Expected:

- exact usage is marked exact when sourced from provider/Letta;
- Den-derived estimates are marked estimated;
- unavailable usage is marked unavailable.

#### `mcp_browser_privacy_notice_is_shown_once_per_session`

Status: not yet added.

Expected:

- browser/devtools MCP privacy note is shown once when server is configured;
- repeated prompts do not spam the notice.

#### `recovery_status_is_visible_and_bounded`

Status: not yet added.

Expected:

- stale approval recovery emits a visible start notice;
- success/failure emits exactly one outcome notice;
- recovery does not loop indefinitely.

## Migration strategy

### Step 1: Add documentation and principles

This document is step 1.

### Step 2: Add pure controller module and tests

Status: complete.

Create a small module, likely in Den first:

```text
services/den/src/core/acp_turn_controller.rs
```

Keep it independent from Axum, SQLx, Reqwest, and Letta client code.

### Step 3: Add Den stream tests around current behavior

Status: mostly complete for Den-side lifecycle hazards.

Before wiring the controller deeply, add/keep stream tests that capture known bugs.

### Step 4: Wire controller into `AcpLettaSseStream`

Status: partially complete. Controller observes lifecycle and participates in terminal gating; legacy state remains.

Move lifecycle decisions out of stream polling branches and into controller calls.

### Step 5: Normalize Den tool-result API statuses

Status: not yet complete. Late result behavior is tolerated in adapter/stream tests, but API status names remain transitional.

Add structured late/cancelled/timeout statuses while keeping compatibility with existing adapter handling.

### Step 6: Adapter alignment

Status: not yet complete.

Use ACP Rust crate patterns and reference implementations to clean up:

- session setup response versus notifications;
- prompt response semantics;
- cancellation;
- tool update lifecycle;
- logging summaries;
- status/session health update shapes.

### Step 7: Add UX/status surface

Status: not yet complete.

Wire the controller snapshot into `session_info` and/or `/status`.

Start with lifecycle and session metadata. Add context budget only after usage data source quality is understood.

### Step 8: Reference implementation comparison

Status: not yet complete.

Create a short research note comparing Goose, fast-agent, and ACP Rust SDK examples against the BEARS lifecycle and UX model.

Suggested output:

```text
Question                                  Goose   fast-agent   Rust SDK example   BEARS target
session/prompt handler shape
where turn state lives
when final response is sent
how tool calls are tracked
how cancellation propagates
how permission requests are cancelled
how client fs/terminal calls are awaited
how MCP tools are loaded
how multiple sessions are isolated
how session notifications are ordered
how late tool results are handled
how context/session health is surfaced
how tool progress is summarized
how recovery/cancellation is explained to users
```

## Acceptance criteria

- The lifecycle principles are documented and linked from runtime reliability docs.
- Pure controller tests cover normal, tool, timeout, cancel, late-result, and orphaned-approval paths.
- Den stream tests prove no terminal event is emitted while adapter-local obligations are pending.
- `session_info` and other Den-server tools never become adapter-local obligations.
- Adapter tests cover startup mode race, MCP log summarization, overlap, and cancellation.
- Exactly-once terminal behavior is enforced by state-machine logic rather than scattered guards.
- Late tool results are acknowledged as ignored instead of reported as surprising `turn_missing` failures.
- Users can inspect active turn phase, pending obligations, mode, MCP summary, work-surface state, and context budget status through `session_info` and/or `/status`.
- Recovery, cancellation, timeout, and waiting-on-tool states are visible as concise status updates.

## Non-goals

- Rewriting Den.
- Rewriting Letta integration.
- Replacing all adapter code with a Goose or fast-agent port.
- Adding a durable obligation ledger in the first pass.
- Solving all history replay or work-surface resolution issues in this lifecycle reset.
- Providing exact context budget numbers when Letta/provider data is unavailable.
- Building a full custom ACP UI beyond protocol-compatible status/session updates.

## Open questions

- Should the adapter eventually implement the official ACP `Agent` trait directly, or should it continue using current JSON-RPC plumbing with official types at the boundaries?
- Should Den expose prompt turns as a typed internal stream instead of SSE JSON events to the adapter?
- What exact compatibility response should replace `turn_missing` for late results: `late_result_ignored`, `turn_cancelled`, or `unknown_turn`?
- How much of MCP tool execution should remain adapter-local versus being represented as Den-managed external obligations?
- Should session mode updates be persisted before first prompt, or remain adapter-local until Den session binding exists?
- Which ACP surface is best for context budget and session health: `session_info`, slash `/status`, `session_info_update`, or a future session usage/status extension?
- Can Letta provide exact context-window usage for the current run, or do we need an explicitly estimated Den-side budget?

## Immediate next action

Recommended next implementation order:

1. Make `acp_stream_routes_session_info_as_den_server_tool` fast and deterministic; it currently passes but takes about 60 seconds by exercising a DB-unavailable branch.
2. Add real production `/cancel` endpoint signaling into active streams, replacing the test-only cancellation hook as the primary path.
3. Normalize late result API responses from compatibility-style `turn_missing`/settled variants toward explicit `late_result_ignored`, `turn_cancelled`, and `turn_timed_out` statuses.
4. Continue replacing legacy stream lifecycle state with controller authority one piece at a time.
5. Add session health/status UX via `session_info` and/or `/status` after an active-turn snapshot registry exists.
6. Add adapter tests for overlap, mode startup race, MCP log summarization, and explicit cancellation.
