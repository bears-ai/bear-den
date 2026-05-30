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

## Implementation guidance

Implementation planning should assume:

- a semantic-group history model,
- trigger vs target compaction controls,
- structured iterative summary updates,
- durable-memory extraction as a separate flow,
- compaction telemetry,
- and post-compaction evaluation probes.

## Status

Proposed.
