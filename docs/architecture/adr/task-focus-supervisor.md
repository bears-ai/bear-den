---
title: Den Task Focus Supervisor for Letta Agents
description: Introduce a minimal Den supervision layer that keeps pair and work agents focused on task completion without duplicating Letta runtime functionality.
status: proposed
date: 2026-05-06
---

# ADR: Den Task Focus Supervisor for Letta Agents

## Context

BEARS is using the Letta API directly for agent execution, including interactive `pair` agents and more autonomous `work` agents.

We want stronger task continuity, especially in coding environments:

- agents should continue after intermediate progress when more work is obviously needed
- agents should stop only at meaningful completion or real blockers
- `work` agents should be driven even harder toward completion than `pair` agents

At the same time, we do **not** want Den to re-implement functionality that already exists in Letta.

Letta already provides:

- agent identity
- instructions/persona
- conversation continuity
- memory
- tool use
- the inner act/observe loop

What Letta does not inherently own is the full set of external signals that matter for task completion in BEARS environments, such as:

- ACP/editor context
- diagnostics remaining
- whether a coding task actually produced edits
- whether a candidate response is only an intermediate checkpoint
- whether a role should continue more aggressively (`work`) or yield naturally (`pair`)

## Decision

Den will implement a **Task Focus Supervisor**: a thin, ephemeral supervision layer around Letta sessions/runs.

The supervisor will:

1. maintain a small session- or run-scoped **task focus** record
2. classify the execution profile (`pair` or `work`)
3. evaluate simple **continue vs yield** heuristics around Letta responses
4. inject short hidden **focus nudges** when an agent appears to stop too early
5. use the same mechanism for both `pair` and `work`, with a stronger continuation bias for `work`

Den will **not** implement:

- a second planner
- a second memory system for ephemeral task progress
- a second tool orchestration engine
- a second transcript/conversation engine
- a general workflow DSL

## Rationale

This design follows a **supervisor / executor split**:

- **Letta** remains the executor
- **Den** becomes a thin supervisor of focus and termination

It also preserves the value of Letta’s internal ReAct-style behavior:

- Letta still reasons, acts, observes, and continues
- Den only supervises whether a candidate yield looks premature

This improves completion behavior without creating a second agent runtime in Den.

## Design

### Letta responsibilities

Letta remains responsible for:

- reasoning about the user’s request
- selecting and calling tools
- maintaining agent memory and conversation continuity
- deciding local next actions
- following durable role instructions

### Den responsibilities

Den is responsible for:

- maintaining ephemeral task focus
- evaluating whether a response is terminal or premature
- nudging for continuation or verification when appropriate
- incorporating external signals from ACP or other channels
- applying stronger completion bias for `work` than `pair`

### ACP / channel responsibilities

ACP and other channel adapters should provide structured signals where available, such as:

- active surface/channel
- files modified
- diagnostics remaining
- tests run
- tool failure category
- active file or selection hints

These signals help Den supervise effectively without forcing Letta to infer everything from prose.

## Task Focus

Den should keep a small ephemeral record per active pair session or work run.

This is:

- **not** durable memory
- **not** a task list
- **not** a project plan

It is only the minimum state needed to answer:

> What is the agent currently trying to finish, and should it likely keep going?

Suggested shape:

\```yaml
task_focus:
  id: <session-or-run-id>
  role: pair | work
  goal: <short normalized goal>
  state: active | awaiting_user | awaiting_approval | verifying | blocked | completed

  completion_mode: answer_only | investigation | coding_task | edit_then_explain
  channel: zed | web | slack | background | other

  approval_required: false
  needs_user_input: false
  blocker: null

  observed:
    agent_claimed_done: false
    files_modified: unknown
    diagnostics_remaining: unknown
    tests_run: unknown
    verification_attempted: false

  nudges_sent: 0
\```

## State Model

Den should keep the state model intentionally small:

- `active`
- `awaiting_user`
- `awaiting_approval`
- `verifying`
- `blocked`
- `completed`

These states are sufficient for continuation control without introducing workflow complexity.

## Role Profiles

### Pair profile

`pair` is interactive and user-facing.

Default bias:

- continue when there is a clear next step
- yield naturally when:
  - task is complete
  - user input is genuinely needed
  - approval is required
  - no confident next step exists

### Work profile

`work` is more autonomous and should be driven harder toward completion.

Default bias:

- continue more aggressively
- ask the user less often
- verify more before yielding
- yield mainly on:
  - completion
  - hard blocker
  - policy boundary
  - explicit approval requirement
  - exhausted options / no viable next step

The mechanism is shared; the continuation bias differs by role.

## Supervisor Loop

Den should intervene only at a few key points:

1. **Initialize focus**
   - derive task focus from the incoming request or assigned job
   - determine role and completion mode

2. **Send to Letta**
   - pass the user/task message to Letta
   - optionally include a short hidden focus reminder

3. **Letta executes normally**
   - reason
   - call tools
   - act
   - observe
   - respond

4. **Evaluate candidate yield**
   - if Letta appears done, blocked, or awaiting input, Den checks whether that is plausible
   - if the response appears premature, Den sends a short hidden nudge and continues the same Letta session/run

## Heuristics

Heuristics should remain conservative and boring.

### Continue heuristics

Default toward continuation when the agent has only:

- found a file
- completed a search
- summarized evidence
- identified a likely issue
- completed one intermediate step

### Coding-task heuristics

For coding-oriented tasks, hesitate to yield if:

- no file was modified despite a request for a fix
- the agent claims completion without verification
- diagnostics still remain
- tests were expected but not run
- the response reports investigation, not outcome

### Pair-specific behavior

`pair` may yield earlier when:

- the user asked for analysis only
- the next step depends on subjective user preference
- the interaction is clearly in a decision loop

### Work-specific behavior

`work` should continue harder when:

- there is a plausible next action
- more inspection or retry is possible
- verification is still pending
- the response is only an intermediate checkpoint

## Nudges

Nudges should be:

- hidden
- brief
- role-aware
- used sparingly

Examples:

### Continue nudge

> The task is still active. Continue if there is a clear next step. Do not yield yet unless complete, blocked, approval-required, or user input is truly needed.

### Verification nudge

> You appear near completion. Verify the result if reasonable before yielding.

### Work-strengthening nudge

> This is a work task. Prefer autonomous continuation over yielding. Continue until complete, hard-blocked, approval-required, or no viable next step remains.

Den should cap nudges to avoid loops or adversarial supervision.

## Non-goals

This supervisor must not become:

- a planning system
- a workflow engine
- a durable memory mechanism
- a replacement for Letta instructions
- a second tool caller
- a second transcript manager

If Den begins reconstructing reasoning state, maintaining detailed step plans, or duplicating tool logic, the design has become too heavy.

## Consequences

### Benefits

- stronger task completion behavior for `pair`
- even stronger completion behavior for `work`
- better use of ACP/channel-specific signals
- no duplicate planning or memory system
- minimal new architecture around Letta

### Costs

- Den gains a small supervisory loop
- some yield decisions remain heuristic
- tuning and observability will be needed

### Risks

- over-nudging could become noisy
- overly aggressive continuation could suppress valid clarification
- `pair` and `work` could drift if their profiles are not kept distinct

## Rollout

### Phase 1

Implement the smallest useful version:

- task focus record
- tiny state model
- `pair` and `work` continuation profiles
- continue + verify nudges
- basic coding-task heuristics
- minimal telemetry

### Phase 2

Add richer ACP/channel signals:

- files modified
- diagnostics remaining
- tests run
- environment blocker classification

### Phase 3

Tune profiles:

- softer yield behavior for `pair`
- stronger completion behavior for `work`

## Summary

Den should add a **minimal Task Focus Supervisor** around Letta sessions/runs.

- Letta remains the executor
- Den supervises focus and termination
- ACP/channels provide external execution signals

This lets BEARS improve completion behavior for both `pair` and `work` agents without re-implementing core Letta functionality.
