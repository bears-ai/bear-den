# Role Runtime Wedge Prevention Plan

## Status

Active near-term reliability work.

For ACP `pair`, the near-term implementation strategy is now the [ACP Lifecycle Reset Plan](./ACP_LIFECYCLE_RESET_PLAN.md). That plan narrows this document's runtime hygiene invariant into an explicit ACP prompt-turn state machine and test suite.

## Context

BEARS now exposes a simpler ontology for per-turn state: `workplan`, `activity`, `memory`, and `execution`. That reduces model confusion, but it does not by itself prevent Letta runtime wedges.

A runtime wedge happens when a Letta conversation/run is left waiting on an unresolved approval or tool result. The next prompt can then fail with an upstream error such as `waiting for approval`, even when the current turn-state summary is correct.

This is a runtime orchestration problem, not a prompt problem.

## Roles and channels at risk

The risk is not unique to ACP `pair`.

Any role/channel that uses Letta runs, tools, approvals, or streamed continuations can wedge if every tool request is not settled or explicitly cancelled. Risk is highest for:

- `pair` over ACP, because it combines local client tools, permission requests, Den server tools, and Letta continuation;
- `work`, because it is expected to run longer execution-oriented tasks and may not have an interactive ACP approval loop;
- `watch`, when observations or monitors invoke tools and must report results;
- `curate`, when review/memory tools are used in longer governance flows.

## Invariant

Every role executor must satisfy this invariant:

> If a Letta tool/approval request is surfaced to Den or an adapter, it must eventually be settled with success/error/denial/timeout, or the owning Letta run must be explicitly cancelled before the next turn starts.

For ACP `pair`, use the stronger turn-terminal invariant from the lifecycle reset:

> A terminal ACP prompt result requires all local/server tool obligations for that prompt turn to be settled, cancelled, failed, or timed out.

## Current recovery stance

BEARS should not claim that `/unwedge` is a reliable user-facing recovery for malformed Letta approval state. The preferred recovery ladder for a stale approval on a bound ACP conversation is:

1. settle the approval/tool request normally;
2. if a local tool or approval wait times out, send an explicit denial/error result to Letta;
3. if the conversation still reports stale approval, try Letta conversation compaction on the same bound `conv-*`;
4. if compaction succeeds, retry the prompt once;
5. if compaction fails or retry still wedges, stop and tell the user to start a new ACP session.

Compaction preserves the ACP session's conversation binding when it works. BEARS should not silently rebind an existing ACP session to a blank new conversation as recovery, because conversation history is the main reason to retain the session.

## Prioritized implementation sequence

### 1. Active-turn lock for ACP `pair`

Add a structural active-turn guard for ACP `pair` prompts.

- Key initially by `acp_session_id`; include `resolved_conversation_id` in diagnostics when available.
- Acquire at prompt start and release when the stream completes or is dropped.
- Reject overlapping prompts with a clear machine-readable error such as `turn_already_active`.
- Later, support an explicit cancel-and-replace flow, but reject-first is safer.

This prevents a common wedge source: a second prompt entering the same Letta conversation while a prior run/tool approval is still active.

### 2. Terminal `turn_result` event

Den should emit a terminal stream event with a stable shape instead of requiring the adapter to infer terminal status from visible output, tool activity, errors, and stream EOF.

Conceptual event:

```text
{
  "type": "turn_result",
  "status": "ok | failed | recovered | needs_new_session | cancelled",
  "reason": "end_turn | compacted_retry | stale_approval | timeout | turn_already_active",
  "request_id": "...",
  "session_id": "...",
  "retryable": false,
  "diagnostics": {}
}
```

This event should be useful to both ACP adapters and future role/channel runtimes.

### 3. Minimal shared role/turn runtime helper

Before adding more ACP-specific code, introduce a small shared runtime helper used first by ACP `pair`.

Initial responsibilities:

- acquire/release active-turn locks;
- expose current pending tool/approval diagnostics;
- centralize timeout-as-deny policy;
- centralize compaction fallback decision;
- shape terminal `turn_result` diagnostics.

This should be intentionally small. It is a stepping stone toward shared `pair`/`work` orchestration, not a full role runtime framework yet.

### 4. Runtime health endpoint for ACP session state

Add an operator/adapter-visible health endpoint for ACP session runtime state.

It should return:

- active turn lock status;
- pending tool/approval turns;
- expired pending turns;
- current conversation id / resolved conversation id;
- last compaction attempt if tracked;
- last terminal turn result if tracked;
- current canonical turn state (`workplan`, `activity`, `memory`, `execution`).

This is a debugging accelerator and should be cheaper than a full transcript system.

### 5. Durable pending approval/tool ledger

Move beyond process-local pending turn state once the runtime shapes stabilize.

A Postgres-backed ledger should track:

- `bear_id`
- `role`
- `channel_kind`
- `channel_id`
- `conversation_id`
- `run_id`
- `request_id`
- `tool_call_id`
- `approval_request_id`
- `tool_name`
- `status`
- `created_at`
- `deadline_at`
- `settled_at`
- `settlement_kind`
- `continuation_status`
- `diagnostic`

This allows Den to detect orphaned approvals after restarts and produce reliable diagnostics.

### 6. `work` approval policy and handoff outcome

Implement the Work Handoff ADR for `work` runtime policy.

- `work` must not wait indefinitely for interactive ACP-style approval.
- Approval timeout should deny or block.
- Approval needs become durable Workplace/activity handoff records.
- `talk` is the default human notification route.
- `pair` is optional when an active technical channel exists.

### 7. Den-owned visible transcript

Add a Den-owned visible transcript after the runtime/event model is stable.

This should preserve user-visible prompts, assistant-visible output, status/recovery notices, tool summaries, and terminal turn results independently from Letta conversation internals.

It is intentionally later because it has broader product semantics around history, replay, and audit.

## Cross-role follow-on design

The ACP patches should later be generalized for all tool-bearing role channels.

### Shared runtime helper

Add a role/channel-scoped runtime hygiene helper that accepts:

- `bear_id`
- `role`
- `agent_id`
- `conversation_id` or channel/session id
- optional `run_ids`
- reason enum such as `before_new_turn`, `stale_approval_detected`, `tool_result_post_failed`, `session_close`, or `channel_cancel`

The helper should cancel stale Letta runs, clear local or durable pending tool state, and return a structured diagnostic payload.

### Durable pending-tool ledger

Move beyond in-memory pending tool state for role channels that can outlive a single ACP stream. A Postgres-backed ledger should track:

- `bear_id`
- `role`
- `conversation_id` / channel id
- `run_id`
- `tool_call_id`
- `approval_request_id`
- `tool_name`
- `status`
- `created_at`
- `updated_at`
- `settled_at`
- `failure_reason`

This allows Den to detect orphaned approvals after restarts and to make cleanup idempotent.

### `work` policy

`work` should not accidentally depend on interactive ACP-style approval. Work execution should be governed by Den policy, task authorization, and explicit handoff/approval records. If `work` encounters an interactive approval request with no authorized approval path, Den should structurally deny or cancel it instead of waiting indefinitely.

## Acceptance criteria

- ACP `pair` preflights stale runtime state before new prompts.
- ACP streams detect orphaned `requires_approval` stops.
- ACP tool-return continuation failures trigger cleanup when they look like stale approval/runtime conflicts.
- Recovery is surfaced as visible status, not a hidden upstream error that fails the prompt.
- Tests cover preflight cleanup, orphaned `requires_approval`, and continuation failure cleanup.
- The plan for cross-role runtime hygiene and durable pending-tool tracking is documented for `work` and other roles.
