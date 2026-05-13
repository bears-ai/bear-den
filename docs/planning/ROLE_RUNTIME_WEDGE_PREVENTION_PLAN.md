# Role Runtime Wedge Prevention Plan

## Status

Active near-term reliability work.

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

## Near-term ACP safety patches

ACP `pair` gets immediate safety patches first because it is the live path where stale approval failures are visible.

1. **Pre-turn runtime hygiene**
   - Before Den sends a new ACP prompt to Letta, clear stale local ACP tool-turn state and cancel stale Letta runs for the pair agent.
   - Reconcile the ACP mode from plan-mode state before prompting.
   - Log the preflight action so operators can distinguish normal hygiene from recovery.

2. **`requires_approval` without a pending result is stale**
   - `requires_approval` is normal only while Den has a matching pending tool result/continuation path.
   - If a Letta stream ends at `requires_approval` with no pending tool result, Den should treat it as an orphaned approval, perform stale-state cleanup, and surface a visible retryable status instead of silently completing an empty turn.

3. **Continuation failure cleanup**
   - When Den fails to continue Letta after posting a local tool result, and the failure looks like stale approval/run conflict, Den should cancel stale runs and clean local tool-turn state immediately.
   - The stream should surface a visible recovery message rather than leaving the next prompt to discover the wedge.

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
