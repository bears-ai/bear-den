# ADR: Work Handoff and Human Escalation

**Status:** Proposed
**Date:** 2026-05-13
**Deciders:** Hans

## Context

BEARS distinguishes Bear roles such as `talk`, `pair`, `work`, `curate`, and `watch`. The Workplace ADR defines a durable Bear-level work setting that groups plans, tasks, artifacts, memory, and activity across roles and runtimes.

`work` is expected to perform execution-oriented activity for a Workplace. It may run without an active ACP `pair` session or any technical channel where the human is currently collaborating. This means `pair` cannot be the only human escalation path for work that blocks on approval, clarification, missing authorization, or risk policy.

At the same time, `talk` is the most reliable human-facing route in many BEARS deployments. It can communicate naturally with the human even when there is no active editor/ACP session. However, `talk` does not yet provide a structured approval mechanism equivalent to ACP client permission requests.

Letta Code and related execution systems may have mechanisms for seeking approval through channels. BEARS should not directly inherit those channel assumptions as product authority because `work`, `talk`, and `pair` are separate agents with separate policy boundaries. Approval and handoff state must be represented in Den/Bear product records, not only in a role-local conversation.

## Decision

BEARS will model `work` approval needs and blockers as **durable Workplace/activity handoff records** with explicit routing to human-visible channels.

`work` must not assume an active `pair` channel exists. When `work` cannot proceed safely, it must create or update a durable handoff/blocker record and stop or sleep safely rather than waiting indefinitely in a live Letta approval state.

Human escalation routing is separate from approval authority:

1. **Durable handoff record** is the source of truth.
2. **`talk` notification** is the default human-visible route when no more specific channel is available.
3. **`pair` notification / ACP handoff** is optional and used when there is an active technical collaboration channel relevant to the Workplace.
4. **UI / task inbox / activity inbox** is the durable operator surface and eventual structured approval path.
5. If no notification route is available, the handoff remains blocked and visible in durable Bear/Workplace state.

Freeform `talk` conversation may request approval, explain blockers, and direct the human to an approval UI or action, but it is not itself the authoritative approval record unless and until Den implements an explicit structured approval capture for that route.

## Handoff record semantics

A handoff record should include enough information to route, review, approve/deny, and resume or safely abandon work.

Conceptual shape:

```text
work_handoffs
- id
- bear_id
- workplace_id nullable
- activity_id nullable
- task_id nullable
- source_role = work
- source_run_id nullable
- status = blocked | requested | approved | denied | revised | resumed | cancelled
- reason = requires_human_approval | needs_clarification | missing_authorization | risk_policy | blocked_dependency | other
- requested_decision_kind = approval | clarification | revision | delegation
- summary
- details
- options_json
- default_on_timeout = deny | stay_blocked
- routed_to_json
- approved_by_user_id nullable
- approved_at nullable
- resolved_at nullable
- resume_token nullable
- created_at
- updated_at
```

Near-term implementations may store this in existing activity/task/handoff structures before a dedicated table exists, but the semantics should remain the same.

## Role responsibilities

### `work`

When `work` needs approval or clarification:

1. Stop before the side effect.
2. Create/update a durable handoff/blocker record.
3. Route notification to `talk` and optionally `pair`/UI.
4. Mark the activity/task as blocked or waiting.
5. Exit, yield, or sleep safely.
6. Resume only after Den records an explicit approval, revised instruction, or denial.

`work` must not wait indefinitely for an interactive ACP-style permission response unless the runtime has an explicitly attached approval channel and a bounded timeout. Timeout should deny or keep the item blocked, not silently proceed.

### `talk`

`talk` is the default human communication path for work blockers.

`talk` may:

- notify the human that `work` is blocked;
- summarize what `work` wants to do;
- ask for clarification or approval in natural language;
- link or direct the human to a task/activity approval surface;
- explain how to open a `pair` session for technical review.

Until structured `talk` approvals exist, `talk` should not mark the handoff approved solely from freeform conversation. Den/task/UI records are the authority for approval state.

### `pair`

`pair` is an optional technical review/execution route.

Use `pair` when:

- an active ACP/editor session exists;
- the decision requires file/diff/workspace inspection;
- the human wants interactive technical collaboration;
- the handoff should become a pair-reviewed implementation plan.

`pair` should not be required for all human escalation.

### `curate`

`curate` may review durable results, memory promotion, and cross-role governance, but routine `work` blockers should not depend on `curate` unless they concern memory/core/Cabinet cleanliness or policy review.

## Approval authority

Approval authority is a Den/Bear product record, not merely a role message.

Valid future approval sources may include:

- explicit UI action;
- task/activity inbox decision;
- authenticated ACP/pair approval event;
- structured `talk` action when implemented;
- policy pre-authorization from task dispatch.

Freeform chat can be evidence or context, but should not be the sole durable approval mechanism for effectful `work` execution.

## Relationship to Letta Code/channel approvals

Letta Code may request approval through channel mechanisms. BEARS should adapt those requests into durable BEARS handoff/approval records:

1. `work` or its runtime surfaces an approval need.
2. Den records the handoff/blocker.
3. Den routes notification to `talk`, `pair`, UI, or task inbox.
4. Den records the human/policy decision.
5. The result is fed back to `work` as approval, denial, revision, or cancellation.

This preserves BEARS role boundaries and avoids letting runtime-specific channel behavior become the product authority.

## Runtime implications

This ADR complements runtime wedge prevention work:

- `work` should auto-deny or block on approval timeout rather than wait indefinitely.
- Missing approval routes should create durable blockers, not live pending Letta approvals.
- `requires_approval` from a non-interactive `work` runtime should be interpreted as a handoff/blocker unless a valid structured approval path is attached.
- Compaction/recovery is a fallback for malformed conversation state, not a primary approval mechanism.

## Consequences

### Positive

- `work` can operate safely when no `pair` channel exists.
- Humans can still be reached through `talk` or UI.
- Approval state becomes durable and auditable.
- Role boundaries remain clear: `talk` communicates, Den records approval, `work` executes.
- The model works for background/scheduled Workplace activity, not just interactive ACP sessions.

### Tradeoffs

- Requires a durable handoff/blocker surface before `work` can be fully autonomous.
- `talk` initially routes approval requests without being able to complete structured approval itself.
- Some work will remain blocked until UI/task approval mechanisms exist.
- More routing logic is needed to choose between `talk`, `pair`, UI, and inbox surfaces.

## Implementation direction

Near term:

1. Define a canonical handoff/blocker payload for `work` activity.
2. Extend activity/workplan tools to represent `handoff_required`, `requested_decision`, and `routed_to` fields.
3. Route `work` blockers to `talk` as human-readable notifications when no active `pair` session exists.
4. Ensure `work` timeouts default to denial/blocking, not indefinite approval wait.
5. Surface handoffs in Den UI or an activity/task inbox.

Follow-on:

1. Add structured approval actions for `talk` or UI.
2. Add a durable table for handoffs/approval requests if existing task/activity structures are insufficient.
3. Integrate Letta Code approval requests into this handoff model.
4. Add resume tokens so approved work can continue safely after human decision.

## Relationship to other ADRs

This ADR extends [Bear Workplaces for Planning and Work Activity](bear-workplaces.md). Work handoffs are Workplace/activity records, not role-local chat-only events.

It also relates to [Workflow State Ontology](workflow-state-ontology.md): handoffs belong primarily to the `activity` domain, approval gates for proposed implementation plans belong to `workplan`, durable lessons belong to `memory`, and current tool capability belongs to `execution`.

It complements [ACP Session Bindings](acp-session-bindings.md) and runtime wedge prevention work by ensuring non-interactive `work` does not depend on ACP-style live approvals.
