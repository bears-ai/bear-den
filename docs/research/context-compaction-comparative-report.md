# Context Compaction Comparative Report

**Status:** Draft research note
**Date:** 2026-05-30
**Audience:** Hans, Builder Bear contributors, Den architecture work
**Related ADR:** [adr-0032-den-context-compaction-architecture.md](../decisions/adr-0032-den-context-compaction-architecture.md)

## Executive summary

Den is replacing Letta-native agent functionality. The highest-risk area is long-running context handling: when an agent session grows beyond model context limits, the system must reduce prompt size without losing the information necessary to continue work effectively.

This report compares several approaches to context compaction and memory management across current agent systems and frameworks, including Letta, OpenClaw, OpenCode, Kagenti, Microsoft Agent Framework, ADK-style approaches, and Factory.ai’s published evaluation methodology.

The main conclusion is:

> Den should not rely on one-shot transcript summarization as its primary context-management mechanism. Instead, it should adopt a layered architecture that combines explicit runtime compaction, structured iterative summaries, and separate durable memory promotion.

More specifically:

- **Letta** is best understood as a **memory-centric architecture**, not a best-in-class compaction system.
- **OpenClaw** provides the strongest evidence of a **runtime compaction lifecycle** suitable for long-running agent sessions.
- **Microsoft Agent Framework** provides the clearest **compaction abstraction model**, especially around grouped messages, triggers, targets, and strategy pipelines.
- **Factory.ai** provides the strongest public argument for **anchored iterative summarization** and the best published **evaluation methodology**.
- **Kagenti** shows that practical operators want explicit **compaction configuration knobs**, but its current model appears to delegate most semantics to an underlying framework.
- **OpenCode** demonstrates the value of extensibility, but also the fragmentation risk of relying primarily on plugins.

The recommended Den design is:

1. Keep a recency-preserving active working set.
2. Compact history using a pipeline over semantic message groups.
3. Maintain a persistent structured summary that is updated incrementally.
4. Promote only stable, reusable facts into durable memory.
5. Evaluate compaction quality using task-continuation probes, not only compression metrics.

## Problem statement

Long-running agent sessions accumulate:

- user requests
- assistant reasoning and replies
- tool calls and tool outputs
- file or artifact references
- decisions and constraints
- partial plans and failed attempts

If the full session transcript is always resent to the model:

- prompts eventually exceed context limits
- cost rises unnecessarily
- latency increases
- the system becomes brittle

The naive solution is to summarize old history into a shorter blob. In practice, this often fails because agent continuity depends on more than general topic recall. A coding or task agent must retain:

- exact artifacts touched
- decisions already made
- constraints that still apply
- failed paths that should not be retried
- what must happen next

The core design challenge is therefore not just "how do we shrink context?" but:

> How do we preserve actionable working state while reducing prompt size?

## Evaluation criteria

A useful compaction system should be judged on:

- **continuity**: can the agent continue without re-reading large amounts of context?
- **artifact retention**: does it preserve files, tools, URLs, services, and outputs that matter?
- **decision retention**: does it preserve why past choices were made?
- **constraint retention**: does it preserve instructions, boundaries, and user preferences?
- **operational safety**: does it avoid invalid tool-call histories and broken state?
- **predictability**: is compaction explicit, observable, and controllable?
- **memory separation**: does it distinguish temporary prompt compaction from durable memory?

## System-by-system analysis

## Letta

### What Letta is good at

Letta’s public architecture is best understood as **memory-centric**. Its distinguishing value is persistent agent state, archival memory, and retrieval-oriented memory organization.

### What Letta is not especially good at

The Letta summarizer implementation appears relatively primitive. That suggests Letta’s core advantage does **not** come from a sophisticated transcript compaction pipeline.

### Likely design pattern

Letta appears to mitigate context pressure primarily by:

- moving important information into memory structures
- retrieving relevant information into active context
- relying less on advanced transcript compaction as the primary mechanism

### Implication

Letta is a strong reference for:

- **separating durable memory from active prompt state**
- agent-state persistence
- memory promotion and retrieval

It is a weaker reference for:

- modern runtime compaction strategy design
- structured iterative transcript summarization
- evaluation of compaction quality

### Lesson for Den

Den should adopt Letta’s separation of:

- **active context**
- **durable memory**
- **retrieved working facts**

But Den should not copy Letta’s summarizer as its main compaction mechanism.

## OpenClaw

### What stands out

OpenClaw treats compaction as a visible runtime capability, not a hidden implementation detail. Its documentation exposes concepts such as:

- auto-compaction
- manual compaction
- configuration
- identifier preservation
- active transcript byte guard
- successor transcripts
- compaction notices
- memory flush
- compaction vs pruning
- pluggable compaction providers

### Why this matters

This is a richer operational model than simple "summarize if context is too long."

In particular:

- **identifier preservation** suggests awareness that references and handles must survive compaction
- **byte guard** suggests operational safety limits
- **successor transcripts** suggest compaction may create a continuation artifact rather than overwrite history blindly
- **compaction notices** suggest users and runtime can observe compaction events
- **compaction vs pruning** suggests separate semantics for different kinds of context reduction

### Strengths

- explicit lifecycle
- user and runtime observability
- operational safeguards
- richer compaction semantics than naive summarization

### Weaknesses

Without code-level confirmation, it is still unclear how structured the underlying summaries are and how much semantic preservation is guaranteed.

### Lesson for Den

Den should treat compaction as a **first-class runtime event** with:

- triggers
- notices and telemetry
- successor state handling
- clear distinction from pruning and memory flush

## OpenCode

### What stands out

OpenCode appears to rely significantly on:

- pruning approaches
- plugins
- ecosystem tools such as context optimizers or "supermemory" add-ons

### Strengths

- extensibility
- experimentation-friendly architecture

### Weaknesses

- fragmented behavior
- quality can vary across plugin combinations
- lack of a clearly dominant first-party compaction model

### Lesson for Den

Den should allow extension points, but core compaction behavior should be **first-party and opinionated**. Compaction should not depend on external plugins for correctness.

## Kagenti / kagent

### What stands out

Kagenti evidence suggests real operational pressure from context growth, especially when many MCP tools are present. In the issue reviewed, users requested exposing framework-level context management features such as:

- compaction or context compression
- overlap size
- summarizer choice
- context cache
- max-length compression triggers

### Strengths

- good signal about real-world operator needs
- clear demand for explicit configuration knobs

### Weaknesses

The evidence suggests Kagenti is more of a **consumer of framework compaction primitives** than a source of novel compaction design.

### Lesson for Den

Den should expose practical operator-facing controls, including:

- thresholds
- overlap
- budget targets
- summarizer pluggability

But Den should own the semantics of compaction rather than merely surfacing a lower-level framework option.

## Microsoft Agent Framework

### What stands out

Microsoft provides the clearest public abstraction for compaction.

Key concepts include:

- **MessageIndex**: structured view over flat history
- **MessageGroup**: atomic units that must be compacted together
- group kinds such as:
  - system
  - user
  - assistant text
  - tool call
  - summary
- explicit distinction between:
  - **trigger**: when compaction starts
  - **target**: when compaction stops

### Strategy types

Microsoft documents a family of strategies:

- truncation
- sliding window
- tool result compaction
- summarization
- selective tool-call compaction
- sequential pipelines
- token-budget composed strategies

### Why this matters

This is the strongest publicly documented compaction control plane:

- compaction over **semantic groups**, not raw messages
- layered degradation from gentle to aggressive
- explicit backstops
- preserved recent floor
- tool-call and result integrity

### Weaknesses

The framework is marked experimental, and the semantics are still fairly generic; on their own, they do not guarantee preservation of artifacts, decisions, or task structure.

### Lesson for Den

Den should adopt Microsoft’s general architecture:

- compact over semantic groups
- separate trigger from target
- use a strategy pipeline
- preserve recent working context
- treat tool bundles as atomic

## ADK-style framework compaction

### What stands out

ADK-style approaches surface explicit context-management primitives such as:

- compaction intervals
- overlap size
- LLM summarizers
- context window compression
- caching

### Strengths

- configurable
- operationally simple
- useful for framework consumers

### Weaknesses

These approaches tend to be generic unless paired with richer task or artifact semantics.

### Lesson for Den

Framework-level knobs are useful, but Den should enrich them with task-aware and artifact-aware structure.

## Factory.ai

### What stands out

Factory provides the strongest public methodology for evaluating context compression in software-agent settings.

Its key ideas:

- the correct optimization target is **tokens per task**, not tokens per request
- traditional summary metrics are insufficient
- compaction should be judged by whether the agent can **continue working**
- structured, persistent, iteratively updated summaries outperform freeform or opaque compression approaches

### Evaluation model

Factory uses probe-based tests after compression:

- recall
- artifact trail
- continuation
- decision retention

And evaluates dimensions such as:

- accuracy
- context awareness
- artifact retention
- completeness
- continuity
- instruction following

### Lesson for Den

Den should adopt:

- **anchored iterative summarization**
- **probe-based evaluation**

This is arguably the most important methodological contribution in the comparison.

## Comparative summary

| System | Primary strength | Main weakness | Best lesson for Den |
| --- | --- | --- | --- |
| Letta | Durable memory and state architecture | Primitive summarization | Separate memory promotion from prompt compaction |
| OpenClaw | Explicit runtime compaction lifecycle | Summary semantics less clear from docs alone | Make compaction visible, operational, and distinct from pruning |
| OpenCode | Extensibility | Fragmentation | Allow plugins, but keep a strong first-party default |
| Kagenti | Operator-facing compaction knobs | Mostly delegated semantics | Expose config controls without outsourcing architecture |
| Microsoft Agent Framework | Clean compaction abstraction and pipeline model | Generic semantics, experimental | Compact over semantic groups with layered strategies |
| ADK-style approaches | Configurable framework primitives | Generic behavior | Provide intervals, overlap, and budget knobs |
| Factory.ai | Best evaluation method; strong structured-summary case | Less public implementation detail | Use anchored iterative summaries and continuation probes |

## Recommended Den architecture

Den should use a **layered context architecture**.

## Layer 1: active recency window

Always preserve:

- system and developer instructions
- active plan or task state
- unresolved recent exchanges
- current tool interactions
- latest artifact references

This protects short-horizon continuity.

## Layer 2: compaction pipeline over semantic groups

Compaction should operate over grouped runtime units, not raw messages. Recommended group types:

- system
- user turn
- assistant text
- tool bundle
- summary
- plan state
- artifact update
- memory candidate

Recommended pipeline order:

1. collapse or summarize older tool-heavy groups
2. merge older history into structured summary state
3. apply recency or sliding-window limits
4. truncate as emergency backstop only

## Layer 3: anchored iterative summary

Maintain a persistent structured summary with sections such as:

- user goal or session intent
- current state
- decisions made
- artifacts touched
- attempts and outcomes
- constraints
- open risks or questions
- next steps

When compaction triggers:

- summarize only the newly truncated span
- merge into the existing structured summary
- do not regenerate the whole summary from scratch unless necessary

## Layer 4: durable memory promotion

Extract only stable, reusable facts into Den memory, such as:

- project conventions
- user preferences
- environment constraints
- stable architecture facts
- validated findings

Do not treat the rolling session summary as durable memory by default.

## Recommended compaction policies

Den should support multiple triggers:

- soft token threshold
- hard token threshold
- turn or group count threshold
- tool-chatter threshold
- artifact churn threshold
- topic or subtask boundary

Den should also separate:

- **trigger**: when to start compacting
- **target**: what safe budget to compact down to

This avoids repeated oscillation at a single threshold.

## Recommended compaction algorithm

1. Detect compaction need.
2. Build or refresh semantic message groups.
3. Preserve system messages and recent working floor.
4. Compact old tool or result-heavy groups first.
5. Summarize the next oldest compactable span into the structured summary.
6. Merge summary incrementally.
7. Preserve a small overlap tail where needed.
8. Recompute budget.
9. Apply recency or sliding-window enforcement if still needed.
10. Apply truncation only as final backstop.
11. Extract durable-memory candidates separately.
12. Emit compaction telemetry and notices.

## Evaluation strategy for Den

Den should not evaluate compaction primarily with compression ratio or lexical overlap.

Instead, use post-compaction probes like:

### Recall probes

- What problem started this session?
- What specific error or requirement was identified?

### Artifact probes

- Which files, tools, URLs, or services were touched?
- What changed in each?

### Decision probes

- What alternatives were considered?
- What was decided and why?

### Continuation probes

- What should happen next?
- What remains unresolved?

### Constraint probes

- What instructions or scope limits still apply?

Score responses on:

- accuracy
- completeness
- continuity
- artifact retention
- context awareness
- instruction following

This should become the standard test harness for Den compaction quality.

## Design decision recommendation

Den should explicitly adopt the following design principle:

> Compaction is a structured runtime state transition, not a one-shot summarization fallback.

That means:

- compaction must be explicit
- compaction must be observable
- compaction must preserve working semantics
- durable memory must remain separate from compacted prompt state

## Source notes

Primary sources consulted in this research pass include:

- Microsoft Agent Framework compaction docs: <https://learn.microsoft.com/en-us/agent-framework/agents/conversations/compaction>
- Factory.ai article on evaluating compression: <https://factory.ai/news/evaluating-compression>
- OpenClaw compaction concepts doc: <https://github.com/openclaw/openclaw/blob/d389a52494cb0519cbdfa69646ba5ff6a3185b04/docs/concepts/compaction.md>
- Letta summarizer implementation: <https://github.com/letta-ai/letta/blob/5da764a65ceccb7a1fb6c53215f5eb04a20cf236/letta/services/summarizer/summarizer.py>
- Letta services tree: <https://github.com/letta-ai/letta/tree/5da764a65ceccb7a1fb6c53215f5eb04a20cf236/letta/services>
- Kagenti repository README: <https://github.com/kagenti/kagenti/blob/main/README.md>
- kagent issue requesting context window management and summarization support: <https://github.com/kagent-dev/kagent/issues/1173>
- ADK compaction docs: <https://adk.dev/context/compaction/>
- Nir Diamant Agent Memory Techniques memory compaction materials: <https://github.com/NirDiamant/Agent_Memory_Techniques/tree/main/all_techniques/15_memory_compaction>

## Open questions

Follow-up code reading would still be valuable for:

- Letta services beyond the summarizer, especially memory, pruning, and context assembly
- OpenClaw implementation behind the compaction docs
- Microsoft Agent Framework source for exact group and pipeline behavior
- ADK implementation details for overlap and compaction interval semantics

Those reads are likely to refine implementation details, but they are unlikely to overturn the main architectural recommendation in this report.
