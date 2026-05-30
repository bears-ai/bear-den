# Den Context Compaction Contract

This document is the implementation-facing contract for Den-owned context compaction.

It refines the architecture in [ADR-0032: Den Context Compaction Architecture](../decisions/adr-0032-den-context-compaction-architecture.md) and the rollout sequence in [DEN Context Compaction Implementation Plan](../roadmap/DEN_CONTEXT_COMPACTION_IMPLEMENTATION_PLAN.md).

The goal is to make context compaction concrete enough for transcript storage, prompt assembly, runtime policy, and observability work to proceed without semantic drift.

## Purpose

Den must support long-lived runtime sessions without relying on unbounded transcript replay. Context compaction is the Den-owned mechanism that reduces prompt pressure while preserving continuation quality and keeping transcript ownership explicit.

This contract defines:

- the canonical runtime objects compaction operates on,
- what compaction may and may not change,
- the protected runtime floors compaction must respect,
- and the minimum behavior required for prompt assembly and operator visibility.

## Non-goals

This contract does not define:

- the exact database schema,
- exact token thresholds,
- exact summary prompt templates,
- or the full archival retrieval design.

Those are follow-on implementation details, but they must remain consistent with this contract.

## Canonical terms

### Canonical transcript

The **canonical transcript** is the durable ordered Den-owned record of runtime session history.

It includes, as applicable:

- user turns,
- assistant replies,
- tool calls,
- tool results,
- approval requests and approval decisions,
- plan/workflow state updates,
- artifact references surfaced during the session,
- and runtime/system events relevant to replay, diagnostics, or continuity.

The canonical transcript is the source of truth for session history. Compaction must not redefine older history as if the transcript never existed.

### Semantic group

A **semantic group** is the smallest runtime unit that compaction may target, preserve, or collapse as a coherent block.

Compaction operates on semantic groups rather than raw messages.

Initial semantic-group types should include:

- user turn,
- assistant reply,
- tool interaction bundle,
- approval interaction bundle,
- workflow/plan update,
- artifact/reference update,
- prior compaction artifact reference.

A semantic group may contain one or more transcript rows/events but is treated as one runtime continuity unit.

### Active working set

The **active working set** is the part of runtime context that must remain directly available to prompt assembly without being summarized away.

At minimum, it includes:

- current system and developer instructions,
- active role/runtime policy,
- active workflow/workplan state,
- unresolved tool interactions,
- unresolved approval interactions,
- recent constraints and decisions still governing the next step,
- recent artifact references still required for continuation,
- and recent user/assistant exchanges needed for immediate continuity.

### Derived compaction artifact

A **derived compaction artifact** is a Den-owned prompt-assembly artifact created from canonical transcript history to reduce prompt size while preserving continuity.

Examples include:

- anchored iterative summaries,
- collapsed tool-result bundles,
- structured summaries of older workflow spans.

Derived compaction artifacts are not canonical transcript and are not durable memory by default.

### Safety floor

A **safety floor** is a boundary compaction may not cross.

If a semantic group is inside or adjacent to a protected floor, compaction must treat it as ineligible until the floor clears or policy explicitly changes.

### Trigger and target

A **trigger** is the reason compaction evaluation begins.

Examples:

- token-pressure threshold,
- semantic-group-count threshold,
- manual/operator trigger,
- model-window safety margin.

A **target** is the eligible older history span selected for compaction once policy determines compaction is needed.

Triggers decide when compaction evaluation starts. Targets decide what history may be compacted.

### Compaction boundary

A **compaction boundary** is the point between retained active context and compacted older context.

Prompt assembly must preserve this distinction explicitly.

## Invariants

The following invariants are mandatory.

### 1. Transcript ownership is canonical

Den-owned transcript history remains canonical even after compaction.

Compaction may derive summaries or collapsed artifacts from transcript history, but it may not silently replace transcript semantics in storage or operator understanding.

### 2. Derived compaction artifacts are explicit

Prompt assembly must treat compacted state as explicit derived context, not as raw transcript replay.

The runtime should be able to identify which context comes from:

- direct transcript replay,
- derived compaction artifacts,
- or other runtime state such as workflow status.

### 3. Durable memory is separate

Compaction does not imply durable memory promotion.

Any promotion of facts, learnings, or summaries into longer-lived memory must occur through separate memory-governance flows.

### 4. Compaction may not cross protected active spans

Compaction must not cross unresolved or currently governing spans, including:

- unresolved tool spans,
- unresolved approval spans,
- active workflow/workplan state needed for the next step,
- active constraints or decisions still governing the session,
- and active artifact references still required for continuation.

### 5. Pair/ACP is the strictest baseline

Initial safety behavior must satisfy the strictest interactive/tool-using runtime, which is `pair`.

If a policy would be unsafe for active ACP continuation, it is not a valid default policy.

## Protected semantic floors

The following floors must exist in v1.

### Unresolved tool floor

Any semantic group containing or directly surrounding an unresolved tool interaction is protected.

Compaction must not collapse across that interaction until the tool span is settled.

### Unresolved approval floor

Any semantic group containing or directly surrounding an unresolved approval request/decision is protected.

Compaction must not collapse across that span until the approval state is resolved.

### Active workflow floor

The current workflow/workplan state and immediately relevant updates must remain in the active working set.

### Constraint and decision floor

Recent decisions, commitments, or constraints that still shape the next turn must remain active and not be compacted into lossy form prematurely.

### Active artifact floor

Artifact references still needed for continuation must remain directly represented in active context or preserved with equivalent explicit fidelity.

## Role-sensitive policy requirements

The architecture is shared, but policy is role-sensitive.

### Pair

`pair` must preserve:

- unresolved tool/approval spans,
- current coding/task constraints,
- active artifact references,
- immediate workplan/workflow state,
- and recent user/assistant continuity required for the next step.

### Chat

`chat` may compact older conversational material more aggressively than `pair`, but it must still preserve:

- current user intent,
- active commitments,
- unresolved workflow items if present,
- and immediately governing constraints.

### Future roles

`work`, `watch`, and `review` may define role-specific floors, but they must still honor the transcript/derived-state split and protected active-span model in this contract.

## Compaction lifecycle requirements

Compaction must support a lifecycle with these stages:

1. detect trigger,
2. inspect current working set and protected floors,
3. select eligible older semantic groups,
4. apply compaction strategy,
5. emit derived compaction artifact(s),
6. record compaction event metadata,
7. assemble future prompt context using retained active state plus derived compacted state.

The preferred strategy order is:

1. collapse tool-heavy older spans when safe,
2. update structured iterative summaries,
3. enforce recency window,
4. truncate only as final backstop.

## Prompt assembly contract

After compaction, prompt assembly must be able to distinguish these inputs:

- instructions and role/runtime policy,
- active workflow/workplan state,
- recent uncompacted semantic groups,
- derived compaction artifacts,
- and any separately governed memory/retrieval inputs.

The runtime must not flatten these into an indistinguishable blob if doing so would erase provenance or semantics important for debugging and recovery.

## Operator and diagnostics contract

Compaction must be observable.

At minimum, Den should be able to expose:

- whether a session has been compacted,
- which trigger initiated compaction,
- what source span or semantic groups were compacted,
- what artifact was produced,
- and what policy version was used.

Compaction should remain debuggable over time even if the exact summary strategy evolves.

## Evaluation contract

Compaction quality must be evaluated primarily by continuation quality, not just token reduction.

At minimum, evaluation should test post-compaction ability to preserve:

- current constraints,
- important decisions,
- artifact continuity,
- workflow/workplan continuity,
- and next-step correctness.

`pair`/ACP continuation cases are mandatory in the baseline evaluation set.

## Minimum v1 expectations

A v1 Den compaction implementation is acceptable if it provides:

- Den-owned canonical transcript retention,
- semantic-group compaction units,
- explicit protected floors for unresolved tool and approval spans,
- at least one derived summary artifact type,
- explicit prompt-assembly support for compacted state,
- operator-visible compaction events,
- and continuation-quality tests covering `pair` and `chat`.

## Open implementation questions

These remain open but must stay within this contract:

- exact summary artifact schema,
- exact persistence shape for artifacts and telemetry,
- exact trigger thresholds,
- and exact integration boundaries between transcript store, runtime assembly, and retrieval.
