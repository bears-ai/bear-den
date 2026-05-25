# Workflow State Overview

Workflow management in Bear Den should be grounded in a single authoritative current-turn state model rather than in several overlapping mode systems or scattered prompt hints. This document provides an architecture-level overview of that model, explains the model-facing derived `operational_focus` concept, and gives both a derivation table and a prompt/render example.

## Purpose

The workflow-state model exists to keep agents and clients oriented around:

- what kind of work is currently happening;
- what kinds of actions are allowed this turn;
- which ontology lane a user request belongs to;
- what kind of next move should be preferred.

The core design goal is to reduce category confusion such as:

- treating active planning as durable memory;
- treating formal workplan approval as if it were execution permission;
- treating prior-turn assumptions as more authoritative than current-turn state;
- forcing models or clients to reconcile several partially overlapping state machines.

## Normative principle

The authoritative source of workflow truth is the **canonical current-turn workflow state**.

Everything else, including human-facing stance labels or model-facing guidance like `operational_focus`, must be downstream of that state rather than a peer authority.

In particular:

- current-turn capabilities are authoritative;
- prior-turn assumptions must not override current-turn state;
- derived guidance may compress or summarize the current situation but must not contradict the canonical state;
- if any derived label disappears, the canonical state should still be sufficient to determine what is allowed.

## The four workflow domains

Workflow state should distinguish four normative domains.

### Workplan

The proposal and approval lane.

Examples:
- drafting a plan;
- entering planning mode;
- submitting an implementation plan for approval;
- revising, approving, or cancelling a proposed plan.

### Activity

The live tactical progress lane.

Examples:
- visible task lists;
- the current item in progress;
- blockers, completion, and handoff state;
- short-horizon work tracking.

### Memory

The durable semantic capture lane.

Examples:
- decisions;
- notes;
- summaries;
- reflections;
- other knowledge-oriented durable records.

### Execution

The current-turn capability and side-effect lane.

Examples:
- whether write or mutation tools are available;
- whether browser or process execution is enabled;
- whether approval is required for effectful actions;
- whether execution is currently unlocked.

## Canonical workflow-state shape

The exact schema may evolve, but the authoritative turn-state surface should expose a small stable set of concepts.

Representative fields include:

- `permission_mode`
- `tool_classes`
- `workplan.state`
- `workplan.approval_status`
- `activity.plan_id`
- `activity.status`
- `execution.execution_unlocked`
- `memory.active_plan_write_allowed`
- `state_authority`
- `state_version`

The important architectural property is not the precise field spelling but the fact that the state is:

- current-turn scoped;
- ontology-aware;
- structurally distinct across workplan, activity, memory, and execution;
- authoritative for both agent and client reasoning.

## Human-facing stance versus model-facing focus

Human users may find labels like **discovery**, **planning**, and **execution** intuitive. Those can remain useful as friendly descriptive terms.

For the model, however, the recommended equivalent is not a separate stance machine. Instead, the model should receive:

1. the canonical workflow-state object; and
2. an optional small derived `operational_focus` field.

The reason for preferring `operational_focus` over a peer `stance` field is architectural discipline:

- `operational_focus` is explicitly a summary or recommendation;
- the canonical workflow state remains the only authority;
- the system avoids inventing a second partially overlapping state model.

## What `operational_focus` is for

Raw affordances tell the model what tools and permissions exist, but they do not always clearly express what kind of next move is preferred.

A derived `operational_focus` field can help answer:

- should the model keep clarifying before acting?
- should it convert the conversation into explicit planning state?
- should it continue execution?
- should it wait for human approval?
- should it summarize or recover context before proceeding?

The focus field should stay compact and generic. A small vocabulary is preferable, for example:

- `clarify`
- `plan`
- `execute`
- `await_approval`
- `summarize`
- `recover_context`
- `handoff`

This is enough to bias the model toward the right kind of move without creating a second ontology.

## Constraints on derived focus

A sound derived focus layer should obey these constraints:

1. **Derived, never authoritative**  
   `operational_focus` must be computed from canonical state and explicit current-turn conditions.

2. **Low-cardinality**  
   Keep the vocabulary small and stable. Do not let it expand into dozens of bespoke micro-modes.

3. **Never contradict authority**  
   If `execution.execution_unlocked = false`, a focus value must not imply that mutation is allowed.

4. **Helpful but removable**  
   If the focus field is omitted, the canonical state should still be sufficient for safe behavior.

5. **Compression, not substitution**  
   The field should summarize what kind of move to prefer, not replace the state that justifies that recommendation.

## Derivation table: canonical state to `operational_focus`

The table below illustrates recommended derivation logic. It is not a rigid finite state machine; rather, it is a deterministic prioritization layer built on authoritative current-turn state.

| Canonical conditions | Derived `operational_focus` | Reasoning |
|---|---|---|
| User request is primarily exploratory; execution is unavailable or premature; relevant context is still missing | `clarify` | Prefer inspection, constraint gathering, and understanding before artifact creation or execution |
| User explicitly asks to make/create/update/track a plan; planning-state tools are available | `plan` | Prefer representing work in planning/activity state rather than only prose or durable memory |
| Workplan has been submitted and is waiting on human approval | `await_approval` | The next move is to wait, clarify, or revise rather than execute |
| Activity has an active in-progress item and execution is unlocked | `execute` | Prefer continuing the approved/allowed work and updating progress |
| Conversation or session has resumed with insufficient context confidence; active work exists but needs re-grounding | `recover_context` | Reconstruct the current work surface, plan, and constraints before continuing |
| User asks for a concise recap, closure, or durable takeaway after substantive work | `summarize` | Prefer synthesis rather than more planning or execution |
| Active work should move from current role/session into reviewed background work | `handoff` | Prefer preparing a handoff/task-intent path instead of local continuation |
| No single strong action lane is justified yet, but read/search tools are available and ambiguity remains | `clarify` | Safe default when understanding is still incomplete |

## Suggested derivation order

When several candidate focuses appear plausible, the derivation logic should resolve them in a consistent priority order.

A practical order is:

1. `await_approval`
2. `recover_context`
3. `plan`
4. `execute`
5. `handoff`
6. `summarize`
7. `clarify`

This order should be interpreted carefully:

- approval waiting usually dominates, because it blocks many forward actions;
- context recovery should dominate execution when session continuity is uncertain;
- explicit user plan requests should dominate continued loose exploration;
- execution should dominate once intent is settled and capability is unlocked;
- `clarify` remains the safe fallback when no stronger focus is warranted.

Implementations may refine the exact precedence, but they should keep the principle that the focus is derived from current-turn authority and explicit intent.

## Prompt/render format recommendation

The model should receive both the authoritative state and any derived focus in a way that makes the dependency obvious.

A simple pattern is:

1. present canonical workflow state first;
2. present derived focus second;
3. state explicitly that the focus is advisory and derived from the current-turn state.

### Example render

```text
[WORKFLOW STATE]
state_authority=current_turn_capabilities
state_version=1

permission_mode=Plan
execution.tool_classes=read_only
execution.execution_unlocked=false
execution.approval_required_for_mutation=false

workplan.state=drafting
workplan.approval_status=drafting
workplan.execution_unlocked_when_approved=false

activity.status=inactive
activity.current_item=none

memory.active_plan_write_allowed=false
memory.write_for_active_workplan_allowed=false

[DERIVED OPERATIONAL FOCUS]
value=plan
reason=The user asked for a plan and planning-state handling is preferred over prose-only output when available.

[INTERPRETATION RULE]
Operational focus is derived guidance, not a second authority. When focus and prior assumptions conflict, follow the current-turn workflow state and current tool availability.
```

### Example JSON-shaped representation

```json
{
  "workflow_state": {
    "schema": "bears.turn_state/v1",
    "state_authority": "current_turn_capabilities",
    "state_version": 1,
    "execution": {
      "permission_mode": "Plan",
      "tool_classes": ["read_only"],
      "execution_unlocked": false,
      "approval_required_for_mutation": false
    },
    "workplan": {
      "state": "drafting",
      "approval_status": "drafting",
      "execution_unlocked_when_approved": false
    },
    "activity": {
      "status": "inactive",
      "current_item": null
    },
    "memory": {
      "active_plan_write_allowed": false,
      "write_for_active_workplan_allowed": false
    }
  },
  "operational_focus": {
    "value": "plan",
    "reason": "The user explicitly requested planning and planning-state handling is preferred when available.",
    "derived_from": "workflow_state"
  }
}
```

## Practical interpretation rules for the model

When consuming workflow state and derived focus, the model should follow these rules:

1. Treat current-turn workflow state as authoritative.
2. Use `operational_focus` as a prioritization hint among actions that are consistent with authority.
3. Do not infer permission from focus alone.
4. Do not write active plans to durable memory unless the user explicitly asks for durable capture and doing so fits memory policy.
5. Prefer planning/activity state for direct plan requests when such handling is available.
6. When context confidence is low, prefer `recover_context` or `clarify` behavior over speculative continuation.

## Example interpretations

### Example A: explicit plan request

Canonical state:
- planning-state tools available;
- execution locked;
- no active workplan submitted.

Derived focus:
- `plan`

Preferred behavior:
- create or update visible planning state;
- avoid responding with only conversational bullets;
- avoid storing the active plan as durable memory.

### Example B: approved execution continuation

Canonical state:
- current activity item is in progress;
- execution is unlocked;
- relevant context is already grounded.

Derived focus:
- `execute`

Preferred behavior:
- continue the work;
- keep progress tracking current;
- surface blockers when they arise.

### Example C: resumed but under-grounded session

Canonical state:
- active work exists;
- session resumed;
- current context confidence is weak or relevant anchors need re-reading.

Derived focus:
- `recover_context`

Preferred behavior:
- re-read the relevant plan, work surface, or nearby anchors;
- avoid assuming stale context remains correct;
- resume execution only after re-grounding.

## Non-goals

This overview does not propose:

- replacing canonical workflow state with a single coarse focus label;
- introducing a second free-standing stance machine;
- encoding all workflow reasoning in prose rather than structured state;
- making every role or client use a large custom set of operational-focus values.

## Bottom line

For models, the right equivalent to user-friendly stance labels is a small derived `operational_focus` field. The field is useful because it summarizes the preferred kind of next move, but it remains subordinate to the canonical current-turn workflow state.

The architecture should therefore provide:

1. one authoritative ontology-aware current-turn workflow-state object;
2. one small derived `operational_focus` summary; and
3. an explicit reminder that current-turn state and tool availability override prior-turn assumptions or conflicting expectations.
