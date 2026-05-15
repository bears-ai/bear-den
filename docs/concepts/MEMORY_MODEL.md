# Memory Model

Bear memory is the durable knowledge a Bear can use across surfaces and time. Raw interactions may enter role-specific memory first; durable shared knowledge can be promoted into `core/` by `curate` when it is useful across roles. Role-local memory can also be a final destination.

Because memory is Bear-scoped, a single Bear may accumulate knowledge across multiple Workplaces and multiple active **work surfaces**. A **Workplace** is the role-scoped operating and memory surface an agent acts in, such as `pair/`, `talk/`, `curate/`, `work/`, or `watch/`. A **work surface** is the currently engaged repo, service, deployment, Mission, project, or other coherent scope of work an agent may act on. That means local understanding should not default to all Bear memory at once. Agents should first ground themselves in the current work surface when one is known.

## Summary

- A Bear has shared memory and role-specific memory.
- `core/` is the shared, curated memory every role can use.
- `talk/`, `pair/`, `curate/`, `work/`, and `watch/` are role-specific memory areas.
- Raw inputs should not automatically become shared truth.
- Role-local memory is not merely a staging area; it may stay local forever.
- `curate` is responsible for deciding what becomes durable shared memory.
- Memory curation is a lane within the broader **Reflection** system.
- Letta Archives provide semantic retrieval indexes over selected canonical memory; they are not the source of truth.
- BEARS should not introduce its own embedding strategy or vector store while Letta Archives satisfy retrieval needs.
- Local grounding should be **work-surface-first within the current Workplace**: use the current work surface's canonical anchors before falling back to broader Bear-global memory.

## Workplace and work surface grounding

Memory is Bear-scoped, but many user questions are really about one **work surface** the agent is acting on while operating in the current **Workplace**.

Definitions:

- **Workplace** = the role-scoped operating and memory surface the agent acts in, such as `pair/`, `talk/`, `curate/`, `work/`, or `watch/`.
- **Work surface** = the currently engaged repo, deployment, service, Mission, project, or other coherent scope of work the agent may act on.

Examples of work surfaces:

- one repository among many the Bear works in,
- one deployment environment,
- one service in the BEARS stack,
- one Mission or project,
- one long-running operational responsibility.

When a user asks questions like:

- "what do you know about this project?"
- "how does this work here?"
- "what have we decided about this architecture?"
- "do you understand this codebase / service / mission?"

the right grounding unit is usually the **current work surface the agent is acting on while in the current Workplace**, not Bear memory in the abstract.

Recommended retrieval precedence:

1. current conversation and trusted situation/session briefing,
2. current Workplace and current work-surface hints,
3. current work-surface canonical anchors,
4. current work-surface role-local working memory,
5. Bear-global shared anchors,
6. broader Bear memory search,
7. local workspace artifacts or external docs as needed,
8. general world knowledge.

This keeps agents from answering with plausible but irrelevant Bear-wide knowledge when the user is really asking about one specific engaged work setting.

## Bear-global vs work-surface-local memory

The practical model should distinguish:

### Bear-global memory

Memory that is useful across multiple Workplaces and work surfaces, such as:

- the Bear's charter or overall purpose,
- cross-cutting glossary terms,
- persistent conventions,
- stable knowledge that should apply broadly.

### Work-surface-local memory

Memory that is specific to one engaged work surface, such as:

- repository architecture,
- service-specific terminology,
- local decisions,
- project conventions,
- current understanding and open questions for that work surface.

Work-surface-local memory may exist both as:

- curated shared `core/` memory for that work surface, and
- role-local working memory such as `pair/` notes for that work surface.

## Canonical work-surface anchors

To make work-surface-first retrieval easy and consistent across agents, each Bear should have a predictable work-surface registry and anchor layout.

Recommended shared canonical structure:

```text
core/bear-overview.md
core/bear-glossary.md
core/shared-conventions.md
core/work_surfaces/index.md
core/work_surfaces/<work_surface_slug>/index.md
core/work_surfaces/<work_surface_slug>/overview.md
core/work_surfaces/<work_surface_slug>/glossary.md
core/work_surfaces/<work_surface_slug>/architecture.md
core/work_surfaces/<work_surface_slug>/decisions.md
core/work_surfaces/<work_surface_slug>/conventions.md
```

Recommended role-local working structure:

```text
pair/work_surfaces/<work_surface_slug>/current-understanding.md
pair/work_surfaces/<work_surface_slug>/recent-findings.md
pair/work_surfaces/<work_surface_slug>/open-questions.md
```

The minimal first slice can be smaller, but the key idea is stable anchor paths. A model should not have to guess where the authoritative overview or glossary for a work surface lives.

## What is Bear memory?

Bear memory is the information a Bear keeps so it can remain useful beyond one conversation or task.

Memory may include:

- durable preferences,
- project or team facts,
- recurring patterns,
- summaries of completed work,
- decisions and rationales,
- and reviewed observations from external systems.

Memory is not just chat history. Chat history, task logs, observations, and durable knowledge are different things with different trust levels.

## Shared memory: `core/`

`core/` is the Bear's shared memory and canonical curated orientation layer.

It should contain durable knowledge that is useful across roles and surfaces. For example:

- stable user preferences,
- project conventions,
- team norms,
- approved task summaries,
- durable facts learned from reviewed work or observations.

`core/` should be curated, not treated as a dumping ground. The goal is to keep shared memory useful, compact, and trustworthy.

`core/` is not a semantic search index. If selected `core/` content is indexed into Letta Archives, the archive passage is a derived summary or pointer. `core/` remains the canonical object to inspect when exact truth matters.

## Role-specific memory

Each internal Bear agent role has its own memory area:

| Area | Purpose |
|------|---------|
| `talk/` | Notes and task intents from chat-like conversations. |
| `pair/` | Notes and task intents from client-side collaboration. |
| `curate/` | Reflection notes, review state, and integration work. |
| `work/` | Task execution notes, logs, and results. |
| `watch/` | Structured observations from inbound events. |
| `core/` | Shared durable memory curated for the whole Bear. |

Role-specific memory lets each role keep useful local memory without exposing every raw input to every other role. A role-local memory can be complete and valid even when it has no Cabinet reference and no `core/` promotion target.

Common role-local memory kinds include:

| Kind | Purpose |
|------|---------|
| `note` | Synthesized memory, reminder, fact, or explanation. |
| `log` | Chronological record of activity or attempts. |
| `decision` | Tactical or strategic choice with rationale. |
| `reflection` | Lessons learned, self-analysis, or review notes. |
| `scratch` | Temporary working memory. |
| `summary` | Condensed form of longer local material. |

## Semantic retrieval with Letta Archives

BEARS uses **Letta Archives** as the preferred semantic retrieval layer. Archives are collections of archival passages that can be shared between agents. They are useful for fuzzy recall, but they are derived indexes over canonical sources, not canonical memory.

Recommended archive rules:

- `core/`, role branches, Cabinet, Den DB, and Garage own canonical data.
- Letta Archive passages store summaries/pointers plus provenance metadata such as canonical id, source URI, version/hash, updated timestamp, and source role.
- Tags are coarse filters; rich provenance belongs in passage metadata and Den's indexing records.
- Since Letta passage creation has no first-class external id or upsert, Den should keep a source-to-passage mapping for indexed material.
- Passage updates are delete-and-create when the source hash changes.
- Role agents may search attached archives, but they should not collaboratively maintain shared archives.

Typical archives:

| Archive | Purpose |
|---------|---------|
| Bear curated archive | Shared semantic recall over selected `core/` summaries, approved proposals, and durable references. |
| Cabinet mission archive | Optional semantic recall for a Cabinet Mission, shared by the Bears/roles assigned to that Mission. |
| Role-local archive | Optional later long-tail recall for one role; should not duplicate `core/`. |

A Bear has a **charter**: the Bear's durable purpose and responsibility boundary. Bear-specific knowledge lives under the Bear. **Domains** are durable areas of knowledge and responsibility within the Bear's scope, such as smart home, renovations, billing, or infrastructure.

A **Workplace** is the Bear's role-scoped operating and memory surface, such as `pair/`, `talk/`, `curate/`, `work/`, or `watch/`. A **work surface** is a durable Bear-level scope of work that an agent may act on: a repo, service, deployment, Cabinet Mission, Docket project, or long-running responsibility. Work-surface references can help memories stay connected to what the agent was working on without making role branches or Cabinet the source of truth.

Cabinet **Missions** are different: they are shared knowledge/work containers that may contain multiple projects and may involve multiple Bears. The Bear↔Mission relationship is many-to-many.

For one Bear working on one long-lived responsibility, the Bear's curated archive over its Domains may be enough. Create Cabinet Mission archives only when a Cabinet Mission needs semantic recall shared across Bears or role agents.

## Reflection and memory curation

Memory curation is part of BEARS **Reflection**: the auditable background review and learning system. Reflection can be triggered by heartbeat, manual request, memory write, task completion, or other events. Heartbeat cadence is throttled, so an active Bear can reflect more frequently than a dormant Bear.

`curate` performs the memory curation lane of Reflection. Other Reflection lanes may handle archive indexing, introspection, health checks, cleanup, and skill review.

## How memory becomes shared

A common memory sharing flow is:

1. `talk`, `pair`, `work`, or `watch` writes role-specific notes, logs, decisions, results, intents, or observations.
2. `curate` reviews those branches on its cycle.
3. `curate` decides what is durable, useful, and safe to share.
4. `curate` promotes the distilled knowledge into `core/` when multiple roles should rely on it.
5. Other roles can use the updated `core/` on future turns or runs.

This keeps shared memory deliberate rather than accidental.

Promotion is optional. Some memories remain role-local because they are tactical, operational, noisy, private, or useful only to one role.

## What should not be remembered

Bear memory should not store:

- secrets,
- raw credentials,
- access tokens,
- private keys,
- unnecessary personal data,
- unreviewed webhook payloads as shared truth,
- or temporary details that will not matter later.

Secrets belong in secret-management systems, not in Bear memory.

## Relationship to Letta memory systems

Letta and Letta Code provide several memory mechanisms. BEARS should use them at the right layer:

| Mechanism | BEARS stance |
|-----------|--------------|
| Letta Code reflection | Use for Letta Code-backed roles such as `talk` and `work` where appropriate. Do not duplicate it for those roles. |
| Letta Archives / archival memory | Use for semantic retrieval. Do not build a separate BEARS vector store. |
| Letta conversation compaction | Letta owns context-window pressure and conversation summarization. |
| Letta memory blocks | Treat as legacy/runtime state for BEARS direction; do not make them the primary long-term memory architecture. |
| BEARS `curate` | Owns cross-role memory governance, `core/` cleanliness, and archive indexing policy. |

`pair` and `watch` are API-direct and do not naturally receive Letta Code reflection. Do not add separate dream agents for them at first: `pair` writes role-local entries and `watch` writes observations; `curate` reviews and consolidates their durable outputs.

## Cabinet and semantic references

Cabinet is the shared, human-editable knowledge base. Cabinet should use top-level spaces for **People**, **Missions**, and **Knowledge**.

A Cabinet **Mission** is not the same as a Bear's charter. A Mission can contain multiple projects and can map to zero, one, or many Bears. A Bear can participate in many Cabinet Missions, and a Cabinet Mission can involve many Bears.

Use **Domains** for Bear-specific knowledge areas. Do not describe bear-specific work as being under a Cabinet Mission unless it is actually part of a shared Cabinet Mission.

Bear memory may reference Cabinet objects, Workplaces, work surfaces, or semantic spaces, but it does not mirror Cabinet one-to-one. A role-local memory can relate to a Cabinet Mission, work surface, project, or person without having a Cabinet page.

Use **situation** for trusted interaction briefings, not “current context.” This avoids confusion with model context windows and compiled prompt context.

## Product language

Prefer:

- “The Bear remembers durable knowledge through curated memory.”
- “`core/` is canonical shared orientation; Letta Archives provide semantic recall.”
- “`core/` is shared memory; role branches hold local context.”
- “Raw interactions are reviewed before they become shared memory.”
- “Some memories are intentionally role-local.”
- “Cabinet is shared knowledge; Bear memory can reference Cabinet without mirroring it.”
- “A Bear has a charter; Domains organize bear-specific knowledge.”
- “Agents act in Workplaces and may act on work surfaces.”
- “Cabinet Missions are shared work/knowledge containers that can involve many Bears.”
- “Situation briefings tell the Bear where it is operating and what boundaries apply.”
- “`curate` decides what the Bear should carry forward.”

Avoid:

- “Everything the user says becomes memory.”
- “All roles see all history.”
- “Memory is just conversation history.”
- “Shared memory is automatically updated by every agent.”
- “Letta archival memory is the source of truth.”
- “Every role should independently archive `core/`.”
- “BEARS has its own vector store.”
- “Every memory must become Cabinet knowledge.”
- “Every role memory is waiting for promotion.”
- “A Cabinet Mission is the same thing as a Bear's purpose.”
- “Every Workplace must be a Cabinet Mission.”
- “Current context” when referring to Den’s situation briefing.

## Related docs

- [Bears and Den](BEARS_AND_DEN.md)
- [Bear agent roles](BEAR_AGENT_ROLES.md)
- [Tasks and autonomy](TASKS_AND_AUTONOMY.md)
- [Reflection system](REFLECTION_SYSTEM.md)
- [Observations and subscriptions](OBSERVATIONS_AND_SUBSCRIPTIONS.md)
- [Semantic memory context](../context/SEMANTIC_MEMORY.md)
- [Semantic Bear Memory ADR](../architecture/adr/semantic-bear-memory.md)
- [Bear Workplaces ADR](../architecture/adr/bear-workplaces.md)
- [Reflection System ADR](../architecture/adr/reflection-system.md)
- [Multi-agent architecture ADR](../architecture/adr/multi-agent-architecture.md)
- [Den Bear spec](../../services/den/docs/bear-spec.md)
