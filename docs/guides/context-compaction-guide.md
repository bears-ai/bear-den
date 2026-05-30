# Context Compaction Guide

This guide explains how Den context compaction works at a practical level and how to think about it when building, testing, documenting, or later describing Bear Den behavior externally.

It complements:

- [ADR-0032: Den Context Compaction Architecture](../decisions/adr-0032-den-context-compaction-architecture.md)
- [Den Context Compaction Contract](../architecture/den-context-compaction-contract.md)
- [Den Context Compaction Schema Direction](../architecture/den-context-compaction-schema.md)
- [Den Context Compaction Implementation Plan](../roadmap/DEN_CONTEXT_COMPACTION_IMPLEMENTATION_PLAN.md)
- [Letta Migration Plan](./letta-migration-plan.md)

## Why compaction exists

Bear Den supports long-running sessions that can include:

- user messages,
- assistant replies,
- tool calls and tool results,
- approval requests and approval decisions,
- workplan and workflow state,
- artifact references,
- and other runtime events.

If Den simply keeps replaying the entire transcript forever, context windows eventually become too large, prompts become slower and more expensive, and continuation quality becomes fragile.

Context compaction is Den's answer to that problem.

The key idea is:

- keep the **canonical transcript**,
- preserve the **active working set** directly,
- and convert older eligible history into **derived compacted state** that is still usable for continuation.

This is not the same thing as deleting history, and it is not the same thing as promoting content into durable memory.

## The basic mental model

Den treats long-lived runtime context as three distinct layers:

### 1. Canonical transcript

The canonical transcript is the source-of-truth session history.

It contains the actual ordered runtime record: user turns, assistant replies, tool events, approval events, workflow changes, and related system/runtime events.

This is the durable history of what happened.

### 2. Active working set

The active working set is the portion of context that must remain directly present for safe continuation.

Examples include:

- current instructions,
- active workflow/workplan state,
- unresolved tool interactions,
- unresolved approval requests,
- recent constraints and decisions,
- active artifact references,
- and recent dialogue needed for the next step.

This is the part Den should not summarize away prematurely.

### 3. Derived compacted state

Derived compacted state is prompt-ready context created from older transcript history.

Examples include:

- iterative summaries,
- collapsed tool bundles,
- structured summaries of older workflow spans.

This state helps Den continue coherently without replaying every historical event in full.

## What compaction is not

Compaction is **not**:

- durable memory promotion,
- transcript deletion,
- a lossy sliding-window policy by default,
- or a hidden mutation of history.

Den's architecture intentionally keeps these concerns separate.

## Why Den uses semantic groups

Den does not want to compact arbitrary individual messages in isolation.

Instead, it groups runtime history into **semantic groups** such as:

- user turns,
- assistant replies,
- tool interaction bundles,
- approval interaction bundles,
- workflow/plan updates,
- artifact/reference updates,
- prior compaction artifacts.

This matters because the unit of continuation is often larger than a single message.

For example:

- a tool call and its result belong together,
- an approval request and its decision belong together,
- a workflow update may need to stay intact,
- an artifact reference may be more important than the surrounding prose.

Compaction becomes safer when it operates on these grouped units instead of raw message lines.

## Protected spans and safety floors

Some runtime history must not be compacted across.

Den calls these boundaries **safety floors** or **protected spans**.

Important examples:

- unresolved tool interactions,
- unresolved approval interactions,
- active workflow/workplan state,
- recent decisions and constraints still governing the session,
- artifact references that are still needed for continuation.

This is especially important for `pair`, where ongoing tool use and approvals make context safety stricter than simple chat.

## Prompt assembly after compaction

After compaction, Den should still assemble prompt context explicitly from separate layers:

- instructions and role/runtime policy,
- active workflow/workplan state,
- recent uncompacted semantic groups,
- derived compacted state,
- and any separately governed memory or retrieval inputs.

This is a major architectural point.

Den should not flatten everything into an indistinguishable blob if that would erase provenance, reduce explainability, or make recovery/debugging difficult.

## Continuation evaluation scaffolding

One of the most important implementation ideas is **continuation evaluation**.

This does **not** mean model-judged evaluation in the first phase.

It means building regression tests that ask:

> After compaction, does Den still preserve the information needed to continue correctly?

### What continuation evaluation should test

For representative sessions, tests should verify preservation of:

- active user goals,
- important constraints,
- decisions already made,
- artifact references,
- workflow/workplan continuity,
- unresolved follow-ups,
- unresolved tool/approval awareness,
- and next-step continuity.

### What first-phase scaffolding looks like

The first version should be deterministic and CI-friendly.

It should include:

1. **Representative session fixtures**
   - tool-heavy `pair` sessions
   - long `chat` sessions
   - workflow/plan-heavy sessions

2. **Expected continuity assertions**
   - what goal must still be present
   - what constraint must still be present
   - what artifact/workflow reference must still be present
   - what unresolved state must still be visible

3. **Pre/post compaction comparisons**
   - compare the prompt-assembly view before compaction and after compaction
   - ensure the compacted form still preserves the critical continuity signals

4. **Probe helpers**
   - small deterministic helper assertions such as:
     - preserve constraint
     - preserve artifact ref
     - preserve workflow state
     - do not hide unresolved approval

### Why this matters

Without continuation evaluation, compaction can look successful while actually degrading the runtime.

For example, a summary might save tokens but accidentally drop:

- the user's real goal,
- a key constraint,
- the file being edited,
- or the fact that an approval is still unresolved.

That would be a regression even if context usage improved.

## Why this matters for migration

In the Letta migration, context compaction is one of the places where Den must become a true runtime owner rather than just an adapter.

Replacing Letta means Den must own:

- transcript behavior,
- prompt continuity behavior,
- compaction semantics,
- operator visibility,
- and regression safety.

Compaction is therefore part of the core migration surface, not a cosmetic optimization.

## Why this matters for future storytelling

This guide is also useful groundwork for later external explanation.

A marketing-style explanation of Bear Den should eventually be able to say something like:

- Bear Den remembers the right things without replaying everything forever.
- It preserves active work, tools, approvals, and decisions while compressing older history intelligently.
- It keeps durable memory, active session context, and compacted session summaries as distinct layers.
- It is built to stay explainable and testable, not just smaller.

Those claims should be backed by the architecture and regression tests described here.

## Current implementation status

As of the current slice:

- the architecture and contract are documented,
- semantic grouping and initial compaction policy scaffolding exist in code,
- iterative summary and prompt-assembly scaffolding exist in code,
- and continuation-evaluation scaffolding is identified as a necessary regression-testing layer.

The next natural step is to add deterministic continuation-evaluation fixtures and probes for `pair` and `chat` baselines.
