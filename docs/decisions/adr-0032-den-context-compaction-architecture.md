# ADR: Den Context Compaction Architecture

**Status:** Proposed
**Date:** 2026-05-30
**Deciders:** Hans
**Related research:** [docs/research/context-compaction-comparative-report.md](../research/context-compaction-comparative-report.md)

## Context

Den is replacing Letta-native agent functionality. A core risk area is long-running context handling for agent sessions that include user turns, assistant replies, tool calls, tool results, artifact references, plans, and intermediate decisions.

If Den always resends the full session transcript to the model:

- prompts will eventually exceed model context limits,
- token cost will grow unnecessarily,
- latency will increase,
- and the runtime will become brittle in long-lived sessions.

A naive approach would be to summarize older transcript history into a single compact blob. Research across Letta, OpenClaw, OpenCode, Kagenti, Microsoft Agent Framework, ADK-style context compaction, and Factory.ai suggests that this is not sufficient for coding and task-oriented agents.

The most important findings are:

- Letta is primarily valuable as a **memory and state architecture** reference, not as a transcript-compaction reference.
- OpenClaw demonstrates the value of an explicit **runtime compaction lifecycle**.
- Microsoft Agent Framework demonstrates a strong abstraction based on **semantic message groups**, **trigger vs target** semantics, and **strategy pipelines**.
- Factory.ai provides the strongest public case for **anchored iterative summaries** and **probe-based evaluation**.

We therefore need a Den-native design that preserves working continuity without conflating rolling prompt compaction with durable memory.

## Decision

Den will adopt a **layered context compaction architecture** with the following properties:

1. **Active working context is preserved explicitly**
   - System and developer instructions, active plan state, recent unresolved exchanges, and current tool interactions remain in the live prompt working set.

2. **Compaction operates over semantic groups, not raw messages**
   - Den should compact grouped runtime units such as user turns, assistant text replies, tool bundles, summaries, and plan or artifact state updates.

3. **Compaction uses a layered strategy pipeline**
   - Prefer gentler strategies first, such as tool-result collapse and structured summarization.
   - Use recency-window enforcement next.
   - Use truncation only as a backstop.

4. **Den maintains an anchored iterative summary**
   - A persistent structured summary is updated incrementally from newly compacted spans rather than rewritten from scratch on every compaction cycle.

5. **Durable memory promotion remains separate from prompt compaction**
   - Stable, reusable facts may be promoted into Den memory, but rolling compaction summaries are not treated as durable memory by default.

6. **Compaction is explicit, observable runtime behavior**
   - The runtime should support compaction triggers, telemetry, notices, and clear distinction between compaction, pruning, and memory-promotion flows.

7. **Compaction quality is evaluated by continuation performance**
   - Den should use probe-based evaluation focused on recall, artifact retention, decision retention, constraint retention, and next-step continuity.

## Consequences

### Canonical vs derived state

Den should treat context-compaction state as three distinct layers:

- **Canonical transcript state**: the durable ordered record of session/runtime history, including user turns, assistant replies, tool calls, tool results, approvals, and other runtime events.
- **Derived compaction state**: summaries, collapsed tool bundles, and other prompt-assembly artifacts produced from transcript history to keep active context bounded.
- **Durable memory state**: separately governed memory entries promoted for longer-lived reuse beyond the current session continuation.

This ADR treats the transcript as canonical. Compaction artifacts are derived state that should be inspectable, attributable, and, where practical, rebuildable from transcript history plus compaction policy. Durable memory promotion remains a separate governance flow and must not be implied automatically by compaction.

### Runtime invariants

Compaction policy must preserve Den runtime safety and continuation invariants. In particular, Den should not compact away or blur:

- unresolved tool interactions,
- unresolved approval requests,
- active plan/workflow state needed for the next step,
- artifact references still required for continuation,
- current constraints, decisions, or commitments that materially shape the next turn,
- and recent exchanges still inside the active working set.

For ACP and other tool-heavy runtimes, unresolved tool/approval spans should be treated as semantic floors that compaction may not cross.

### Replay and resume semantics

Den should define compaction so that a resumed session is assembled from:

- active instructions and role policy,
- active plan/workflow state,
- uncompacted recent semantic groups,
- and derived compaction artifacts representing older history.

Compaction should therefore support a clear replay/resume model:

- operators can inspect canonical transcript history separately from derived compacted state,
- runtime prompt assembly can inject structured compacted summaries as explicit context objects rather than pretending they are raw transcript,
- and continuation behavior should remain explainable after compaction boundaries are crossed.

### Trigger classes and safety floors

This ADR does not fix exact thresholds, but implementation should support at least these trigger classes:

- token-pressure triggers,
- turn-count or semantic-group-count triggers,
- explicit/manual compaction triggers for maintenance or operator workflows,
- and model-window safety-margin triggers.

It should also support safety floors that prevent compaction from crossing protected boundaries such as active tool spans, approval spans, and the current plan/workflow working set.

### Role-sensitive policy

This architecture should be shared across Den roles, but compaction policy may vary by role risk surface and execution mode.

Examples:

- `pair` should preserve unresolved tool/approval spans, current coding constraints, active artifacts, and immediate workplan state.
- `chat` may allow more aggressive compaction of older conversational material while still preserving current commitments and user intent.
- `work`, `watch`, and `review` may require role-specific preservation rules around observations, queued work, audits, or synthesis artifacts.

### Positive

- Den will have a clearer separation between:
  - active prompt state,
  - compacted session state,
  - and durable memory.
- Long-running sessions should degrade more gracefully under context pressure.
- Tool-heavy sessions will be safer because compaction can preserve semantic group integrity.
- The architecture creates a strong foundation for testing compaction quality with realistic task-continuation probes.
- The design aligns with the strongest observed ideas from Letta, OpenClaw, Microsoft Agent Framework, and Factory.ai without copying any one system wholesale.

### Costs and obligations

- Den must maintain semantic grouping metadata or an equivalent grouped-history abstraction.
- Den must implement structured summary merge behavior, not only one-shot summarization.
- Den must define compaction triggers, targets, floors, and backstops.
- Den should add telemetry and traceability around compaction events.
- Den should build an evaluation harness for post-compaction continuation quality.
- Den must keep transcript ownership, derived compaction artifacts, and durable memory promotion as distinct implementation concerns.
- Den should make compaction artifacts inspectable in operator/admin read models so compaction remains auditable rather than hidden prompt mutation.
- Den should version or otherwise attribute compaction artifacts so changes in policy or summary strategy are debuggable over time.

### Non-goals

This ADR does not yet fix:

- the exact summary schema,
- the exact runtime storage representation,
- the exact trigger thresholds,
- or the exact implementation module boundaries.

Those details should be refined during implementation planning, but they should remain consistent with this layered architecture.

## Alternatives considered

## 1. Naive transcript summarization

Summarize old transcript history into a single freeform blob whenever context is too long.

Rejected because:

- it tends to lose artifacts, decisions, and constraints,
- it is hard to validate,
- and it does not clearly separate rolling prompt state from durable memory.

## 2. Letta-style memory-first approach without explicit runtime compaction

Rely mainly on memory promotion and retrieval while keeping transcript compaction relatively simple.

Rejected as a complete solution because:

- Letta’s memory architecture is valuable,
- but a memory-first approach alone does not provide enough runtime control or observability for transcript pressure in tool-heavy long sessions.

## 3. Pure truncation or sliding-window compaction

Drop oldest history or keep only the last N turns.

Rejected as the primary strategy because:

- it is predictable but too lossy,
- it discards decisions and artifact trails,
- and it forces the agent to re-discover prior work.

It remains acceptable as a final backstop.

## 4. Generic framework compaction without Den-specific semantics

Expose framework-level compaction knobs only and delegate the rest.

Rejected because:

- it leaves Den dependent on generic semantics,
- and it does not model Den-specific runtime units such as plans, artifacts, or memory-candidate boundaries.

## Rationale

This decision combines the strongest observed ideas from the systems reviewed:

- **Letta** for memory and state separation
- **OpenClaw** for explicit runtime compaction lifecycle
- **Microsoft Agent Framework** for semantic grouping and layered strategy pipelines
- **Factory.ai** for anchored iterative summaries and probe-based evaluation

This combination best fits Den’s needs as a multi-turn, tool-using, artifact-aware agent runtime.

## Migration implications

This ADR should be read as Den's replacement direction for Letta-era long-context handling. In migration planning, parity should be evaluated not only by token reduction but by:

- continuation quality after compaction,
- retention of active constraints, decisions, artifacts, and plans,
- auditability of what was compacted and why,
- and recoverability when policies or summaries need to be rebuilt.

This means transcript persistence, compaction artifacts, and operator read models must evolve together rather than as isolated subsystems.

## Implementation guidance

Implementation planning should assume:

- a semantic-group history model,
- trigger vs target compaction controls,
- structured iterative summary updates,
- durable-memory extraction as a separate flow,
- compaction telemetry,
- post-compaction evaluation probes,
- explicit prompt-assembly handling for derived compacted state,
- canonical transcript retention separate from compacted summaries,
- and role-sensitive compaction floors for active tool, approval, workflow, and artifact state.

## Status

Proposed.
