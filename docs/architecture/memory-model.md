# Memory Model

**Current architecture and partial cutover.** Profile-local and shared memory below still describe unbound legacy sessions. When a conversation or eligible Work run is bound to a hat, model-facing memory tools and turn-start context now use its own source-local notes, hat-curated records, and Bear `core/` instead; this does not yet imply that all Bears or UI views have session isolation. See the verified [Bear memory and hats topic](../topics/bear-memory-hats.md) and the remaining [active plan](../roadmap/HATS_AND_SESSION_MEMORY_BOUNDARIES_PLAN.md).

Bear memory is the durable knowledge a Bear can use across stances, work surfaces, channels, and time.

In the current architecture, canonical Bear cognition lives in **per-Bear SQLite**. Memory is not the same thing as transcript history, task state, or external retrieval indexes.

## Summary

- canonical Bear cognition is stored in per-Bear SQLite
- shared knowledge and stance-local knowledge are distinct
- work-surface grounding matters as much as Bear-global memory
- transcript history is not the Bear's memory store
- Docket jobs/tasks are infrastructure, not cognition
- derived recall indexes support retrieval but are not canonical truth

## Core distinctions

### Shared vs stance-local memory

Shared memory is durable Bear knowledge that should be usable across stances and surfaces.

Stance-local memory is scoped knowledge that may remain local indefinitely or later be promoted in legacy unbound sessions. Bound conversations and Work runs instead write raw notes under their canonical source ID; promotion to a hat requires a separate review path, which is not yet wired.

### Bear-global vs work-surface-local memory

Some memory is useful across the whole Bear.

Examples:

- charter and purpose
- glossary terms
- shared conventions
- broad architectural facts

Some memory is specific to one work surface.

Examples:

- repository architecture
- service-specific terminology
- local decisions and conventions
- current understanding of one project or deployment

### Memory vs transcript

- transcript history records what happened in a conversation or tool exchange
- memory records what the Bear should keep and reuse later

### Memory vs tasks

- memory is Bear cognition
- tasks/jobs are managed work state in Docket and Den Postgres

## Canonical storage

Per-Bear SQLite is the canonical store for:

- memory records
- links and supersession chains
- proposals and observations
- promotions and curation outcomes
- reflection outcomes tied to cognition

Den Postgres stores related but non-cognitive state such as:

- conversations and transcript artifacts
- approvals and client obligations
- prompt-memory blocks
- Docket tasks/jobs
- reflection queue and scheduling state

## Work-surface grounding

Many user questions are really about one current work surface rather than Bear memory in the abstract.

Examples of work surfaces:

- a repository
- a local workspace
- a service or deployment
- a Docket project
- a Cabinet Mission
- a long-running responsibility

Recommended retrieval order for local-understanding questions:

1. current conversation and trusted session briefing
2. current stance/channel/work-surface resolution state
3. canonical work-surface anchors
4. work-surface stance-local memory
5. Bear-global shared anchors
6. broader Bear memory search
7. direct artifact inspection or external docs
8. general world knowledge

## Canonical memory layout

The architecture still uses familiar logical paths such as `core/` and role-local branches, but those are logical-path projections over canonical memory records rather than a separate canonical filesystem.

Important conceptual areas:

| Logical area | Meaning |
|--------------|---------|
| `core/` | shared curated Bear memory |
| `pair/` | pair-local notes and learnings |
| `chat/` | chat-local notes and learnings |
| `review/` | review/curation-local notes |
| `work/` | execution-local notes and summaries |
| `watch/` | observation-local notes |

## Promotion and curation

Memory does not automatically become shared truth.

Typical flow:

1. a stance writes local memory, an observation, or a proposal
2. review/curation examines it
3. it is retained locally, summarized, promoted, superseded, or rejected
4. if promoted, shared memory is updated in canonical Bear cognition

## Derived recall

Semantic retrieval is a **derived recall layer**, not a second memory truth.

It exists to support:

- vector-style retrieval over canonical memory and approved linked sources
- bounded turn-start recall
- hybrid on-demand search

Canonical truth remains in SQLite memory records and curated shared memory.

## Complexity budget

The memory system optimizes for a two-sentence operator mental model:

> Memory is SQLite. Set `QDRANT_URL` and you get semantic recall; don't, and you get anchors + keyword.

`QDRANT_URL` is the **only operational dial** for memory behavior. There are no memory tiers, feature flags, or per-lane toggles at the operations layer; everything else is an internal architecture decision recorded in ADRs, invisible to a self-hoster.

To keep it that way, additions to the memory system carry a standing evidence rule:

- **Any new recall lane, ranking signal, projection tier, or curation pass must cite the observed failure it fixes before it lands.** "Would plausibly help" is not sufficient; a described failure mode with an example is.
- **Speculative lanes are resolved by subtraction, not configuration.** A lane that cannot cite its failure is deferred — removed from the hot path, with a "revisit when \<evidence\>" note in its owning ADR — never hidden behind a flag that operators must learn.
- **Landed lanes are kept while they stay cheap.** Removal churn is itself complexity; a landed, tested lane is removed only when it demonstrates maintenance or quality cost, with the decision recorded in its owning ADR's status.

| Lane | Owning ADR | Status | Revisit trigger |
|------|-----------|--------|-----------------|
| Vector + keyword recall | [ADR-0038](../decisions/adr-0038-platform-embedding-standard-and-derived-recall-index.md) | keep (core) | — |
| Temporal filtering (validity windows) | [ADR-0041](../decisions/adr-0041-archival-recall-and-async-curation.md) | keep (landed; feeds contradiction surfacing) | — |
| Bounded graph expansion | [ADR-0041](../decisions/adr-0041-archival-recall-and-async-curation.md) §6 / [ADR-0042](../decisions/adr-0042-memory-entity-relationships-and-bear-entity-layer.md) | keep (landed) | remove if recall-quality evals show no lift, or maintenance cost appears |
| Freshness-trend ranking | [ADR-0041](../decisions/adr-0041-archival-recall-and-async-curation.md) §5 | keep (landed, derived-only) | remove if it never changes a curation or ranking outcome in practice |
| Entity anchor projection | [ADR-0042](../decisions/adr-0042-memory-entity-relationships-and-bear-entity-layer.md) §8 | keep (landed, explicit-anchor-only) | derived-fallback projection stays deferred until curation policy is mature |
| Rerank (cross-encoder) | [ADR-0038](../decisions/adr-0038-platform-embedding-standard-and-derived-recall-index.md) §5 | deferred | recall-quality complaints attributable to ranking, not coverage |

## Prompt-time memory use

At turn time, the runtime does not dump all Bear memory into the prompt.

Instead it assembles:

- compiled role prompt
- key memory projection
- optional derived recall passages
- prompt-memory blocks
- runtime supplements

See [den-runtime](den-runtime.md) for the exact context assembly model.

## Related docs

- [den runtime](den-runtime.md)
- [bears and den](bears-and-den.md)
- [pair stance](pair-stance.md)
- [reflection system](reflection-system.md)
- [tasks and autonomy](tasks-and-autonomy.md)

## Open-session reflection and memory

Reflection may happen before a session is closed. These open-session reflection runs are checkpoints over bounded activity windows, not implicit promotion of session material into shared memory. Durable shared-memory changes still follow the normal memory curation path.
