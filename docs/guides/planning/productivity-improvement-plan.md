# Productivity Improvement Plan

## Purpose

This document outlines a practical plan for improving agent productivity during implementation and repair loops, especially when commands fail after local edits. The focus is on sustainable, toolchain-agnostic infrastructure that helps the agent continue making progress without unnecessary handoff to the user.

The plan is organized around four major areas:

1. Command compression readiness
2. Sustainable diagnostic extraction for known and unknown toolsets
3. Sustainable source-context surfacing around failures
4. Task context for changed files, validation state, and failure linkage

A fifth investigation area covers autonomous implementation-loop behavior and bounded repair budgets, which may align with future `work` role execution.

---

## 1. Command compression readiness

### Goal

Preserve the key state needed for repair loops when command output is compressed in the future (for example, with RTK or similar techniques).

### Problem

Large command outputs can overwhelm the context window. If those outputs are compressed too aggressively, the agent may lose the critical information needed to continue repairing a failure:

- what command ran
- whether it failed or succeeded
- which files were implicated
- whether the failure was likely actionable locally
- whether the failure is still unresolved

This can cause the agent to stop or re-investigate unnecessarily after compression.

### Plan

Introduce a compact execution-outcome summary artifact per command. This artifact should survive compression even when raw stdout/stderr is truncated or collapsed.

Suggested fields:

- command identity
- working directory
- exit status
- normalized failure class
- implicated files and spans
- whether the failure appears locally actionable
- whether the failure is still unresolved
- whether the failure likely relates to recently edited files

Maintain a rolling "latest unresolved diagnostics" structure in task context so that compression preserves the currently active repair thread.

Compression should preferentially retain summaries for:

- the latest failing command
- the latest successful validation command
- commands associated with recently edited files
- unresolved failures still linked to the current task

### Why this matters

When compression arrives, the agent should still be able to answer:

- what just failed
- where it failed
- whether it should continue repairing
- what command should be rerun next

---

## 2. Sustainable diagnostic extraction for known and unknown toolsets

### Goal

Provide structured, actionable failure information without requiring bespoke support for every tool a user might bring.

### Problem

Tool-specific diagnostic parsing can be valuable, but it does not scale if the environment assumes advance knowledge of every toolchain. Users will bring unfamiliar compilers, test runners, linters, scripts, build tools, and wrappers.

If the system depends entirely on hand-built adapters, the productivity improvement will degrade whenever a toolset is new.

### Plan

Adopt a layered diagnostic pipeline.

#### 2.1 Generic parser layer

Start with generic, tool-agnostic parsing that recognizes common output patterns such as:

- `path:line`
- `path:line:column`
- stack-trace-like references
- assertion/test failure markers
- syntax/type/lint/build failure cues
- timeout, permission, network, missing dependency, and missing file signals

This layer should work reasonably well even for unseen tools.

#### 2.2 Heuristic classifier layer

Classify failures into broad categories even when the underlying tool is unfamiliar.

Examples:

- syntax or parse failure
- semantic/type failure
- test assertion failure
- linter or formatting failure
- timeout or stall
- environment or dependency failure
- permission or access failure
- unknown failure

Include a confidence score where appropriate.

#### 2.3 Optional tool adapters

Over time, add higher-fidelity parsers for widely used ecosystems. These should improve quality but not be required for baseline operation.

The system should degrade gracefully:

- if an exact parser exists, use it
- if no parser exists, emit partial structured diagnostics plus a raw excerpt fallback

### Output shape

A structured diagnostic should ideally include:

- file path
- line and column when available
- severity
- message
- tool or command source
- probable failure category
- confidence
- raw excerpt fallback

### Why this matters

The agent does not need perfect understanding to continue productively. In many cases, it is enough to know:

- the failure is actionable
- the failure points into a recently edited file
- the next likely step is to inspect and patch locally

---

## 3. Sustainable source-context surfacing around failures

### Goal

Attach the right local source context to diagnostics in a way that works across languages and unknown toolsets.

### Problem

Even when a diagnostic identifies a file and line, the agent still needs source context to repair it. Relying on language-specific semantic tooling for every ecosystem is not sustainable.

### Plan

Start with universal text-based context retrieval.

#### 3.1 Baseline context retrieval

For any diagnostic with a file and line reference, automatically retrieve:

- a window of lines before and after the reported span
- the most recent edit hunk touching that area, if available
- optionally the nearest surrounding block using simple heuristics where possible

This should work for any text file, regardless of language.

#### 3.2 Optional semantic enrichment

Add language-aware enrichment later when available, such as:

- containing function
- containing class
- containing test block
- config section boundaries

These should be additive improvements, not prerequisites.

#### 3.3 Unknown formats

For unfamiliar languages or formats, always fall back to:

- nearby lines
- recent diff hunk
- raw diagnostic excerpt

### Why this matters

Nearby text context is cheap, sustainable, and broadly effective. It enables repair loops even when the environment has never seen the toolchain before.

---

## 4. Task context for changed files, validation state, and failure linkage

### Goal

Maintain durable task-local context about edits, validation, and unresolved failures so the agent can decide whether to continue repairing.

### Problem

After making edits and running validation, the system often lacks a coherent task-local picture of:

- which files changed
- which changed files have been validated since the last edit
- whether the latest failure points into one of those files
- what unresolved issues remain active

This weakens the agent’s ability to stay in a productive repair loop.

### Plan

Introduce a first-class task-context model.

#### 4.1 Core tracked state

Track, at minimum:

- files changed in the current task or session
- edit order or timestamps
- last known validation status per changed file
- unresolved diagnostics
- linkage between diagnostics and recent edits
- latest successful validation command relevant to the task
- latest failing validation command relevant to the task

#### 4.2 Failure linkage

When a diagnostic points into a recently changed file, mark that linkage explicitly.

Useful labels include:

- changed and unvalidated
- changed and implicated by latest failure
- changed and validated since latest edit
- unresolved diagnostic still active

#### 4.3 UI alignment

This work should align with editor behaviors such as showing changed files in UI. For example, if Zed or another client provides visibility into changed files, the task context can support a coherent shared picture between UI and agent runtime.

#### 4.4 Stretch scope

As an extension of this same task-context concept, preserve compact unresolved-task state across longer interactions, including:

- last commands run
- unresolved diagnostics
- implicated files
- validation status

This extends naturally into lightweight per-task memory rather than being a separate feature.

### Why this matters

This combines and extends the ideas of:

- tracking changed files since last successful validation
- linking failures to recent edits
- retaining compact unresolved-task state

Together, these give the agent a stable working model of what changed, what broke, and what remains to be fixed.

---

## 5. Investigation: autonomous implementation-loop mode and bounded repair budgets

### Goal

Investigate whether the environment should explicitly support a mode in which the agent continues implementation and repair work without waiting for human input after each actionable local failure.

### Motivation

This may align with scenarios where:

- the `pair` role is in an explicit implementation loop
- the `work` role is asked to operate without human input
- a task can safely continue until validated, blocked, or out of budget

### Plan

Explore a session or task mode that signals:

- implementation loop active
- continue automatically on actionable local failures
- stop only when validated, blocked, uncertain, or budget exhausted

Pair this with bounded auto-repair budgets, such as:

- maximum repair iterations
- maximum validation reruns
- scope limits to changed or related files unless broadened deliberately

This mode should be visible in runtime state and ideally in client UI.

### Why this matters

A bounded implementation-loop mode could formalize the desired behavior of continuing through obvious local repairs while avoiding runaway autonomous loops.

---

## Recommended sequencing

### Phase 1: Task context foundation

Build the shared task-context model first:

- changed files
- edit ordering
- validation state
- unresolved diagnostics
- edit/failure linkage

This provides the substrate for the rest.

### Phase 2: Generic diagnostic extraction

Add the generic parser and heuristic classifier layers so failures become structurally actionable even for unfamiliar tools.

### Phase 3: Automatic source-context surfacing

Attach nearby source and recent edit context to parsed diagnostics.

### Phase 4: Compression preservation

Ensure command compression retains the task-context state and execution-outcome summaries needed to preserve the active repair thread.

### Phase 5: Implementation-loop investigation

Prototype explicit autonomous implementation mode and bounded repair budgets, potentially shared across `pair` and `work` role execution.

---

## Success criteria

This plan is successful if, after an actionable local failure, the environment usually enables the agent to continue without user intervention.

Concretely, the system should be able to preserve and surface:

- what changed
- what failed
- where it failed
- whether the failure is linked to recent edits
- whether the failure is still unresolved
- which validation step should happen next

And this should remain true even when:

- command output is compressed
- the toolchain is unfamiliar
- the language is unknown to the environment
- the session spans multiple repair iterations

---

## Summary

The core design principle is to favor sustainable, toolchain-agnostic task context over brittle tool-specific assumptions.

The environment does not need perfect understanding of every tool. It needs to reliably preserve enough structured task state that the agent can continue through implementation and repair loops with confidence.
