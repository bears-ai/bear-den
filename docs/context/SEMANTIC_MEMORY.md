# Semantic Memory Context

This summary explains how BEARS thinks about semantic memory across Cabinet, Bear MemFS, role-local memory, Reflection, and future operator UI.

## Core idea

Not every Bear memory belongs in Cabinet, and not every role-local memory needs promotion to `core/`.

A memory object can be complete and valuable while remaining local to one role forever. Role-local memory is not merely a staging area for shared truth.

## Memory surfaces

| Surface | Purpose |
|---|---|
| **Cabinet** | Shared, human-editable canonical knowledge. |
| **Bear `core/`** | Compact curated memory useful across Bear roles. |
| **Role memory** | Durable local memory for one role: `talk/`, `pair/`, `curate/`, `work/`, or `watch/`. |
| **Letta Archives** | Letta-native semantic retrieval indexes over canonical BEARS sources; not source of truth. |
| **Letta memory blocks** | Legacy/runtime state for BEARS direction; should not be the primary long-term memory architecture. |
| **Artifacts** | Files and outputs; may be referenced by memory but are not memory themselves. |
| **Conversation/session state** | Interaction-local state; not automatically durable memory. |
| **Reflection** | Auditable background review and learning system; memory curation is one lane. |

## Cabinet spaces

Cabinet should use three top-level semantic spaces:

| Space | Purpose |
|---|---|
| **People** | Knowledge about people, preferences, relationships, identity-sensitive facts, and stakeholders. |
| **Missions** | Shared work/knowledge containers that may contain multiple projects and may involve multiple Bears. |
| **Knowledge** | General reusable knowledge, policies, procedures, decisions, concepts, and references. |

Bear memory may reference these spaces, but it does not mirror Cabinet one-to-one. A role-local memory can relate to a Cabinet Mission without having a Cabinet page.

A Bear's **charter** is its durable purpose and responsibility boundary. Bear-specific knowledge lives under the Bear. **Domains** are durable areas of knowledge and responsibility within the Bear's scope. A Cabinet **Mission** is a shared work/knowledge container. The relationship is many-to-many: a Bear can participate in many Missions, and a Mission can involve many Bears.

## Semantic memory model

BEARS describes memory with four dimensions.

### 1. Locality

Where the memory lives and who should use it:

- `role-local`
- `core`
- `cabinet`
- `artifact`
- `conversation` / `session`

### 2. Kind

What type of memory it is:

| Kind | Meaning |
|---|---|
| `note` | Synthesized memory, reminder, fact, or explanation. |
| `log` | Chronological activity record. |
| `decision` | Tactical or strategic choice with rationale. |
| `reflection` | Self-analysis, lessons learned, or review note. |
| `scratch` | Temporary working memory. |
| `summary` | Condensed form of longer material. |
| `proposal` | Explicit request for review or promotion. |
| `observation` | Event-derived record, usually schema-managed. |
| `result` | Task/run output summary, usually schema-managed. |

General role-local entry tools should focus on `note`, `log`, `decision`, `reflection`, `scratch`, and `summary`. Schema-owned kinds such as `proposal`, `observation`, and `result` should use dedicated tools.

### 3. References

What the memory is about. References are optional.

Examples:

- `person_ref`
- `domain_ref`
- `mission_ref`
- `knowledge_ref`
- `cabinet_ref`
- `artifact_ref`
- `task_ref`
- `conversation_ref`

A `mission_ref` does not imply a Cabinet object. A `cabinet_ref` is explicit.

### 4. Lifecycle

What should happen to the memory over time:

| Field | Example values |
|---|---|
| `scope` | `role-local`, `core-candidate`, `cabinet-candidate` |
| `retention` | `session`, `short`, `durable`, `archive` |
| `promotion` | `none`, `maybe`, `proposed`, `rejected`, `approved` |
| `status` | `active`, `superseded`, `stale`, `archived` |

A role-local memory with `promotion: none` is a valid final memory, not a failed promotion.

## Letta Archives and semantic retrieval

BEARS should use Letta Archives rather than introducing a separate embedding strategy or vector store.

Key rules:

- Letta Archives are derived indexes over canonical stores such as `core/`, role branches, Cabinet, Den DB, and Garage.
- Archive passages should often be summaries or pointers, not full canonical truth.
- Passage metadata should include canonical IDs, source URIs, version/hash, updated timestamps, and provenance.
- Tags should be coarse filters such as Bear, mission, source, or kind.
- Because Letta passage creation has no first-class external ID or upsert, Den should maintain source-to-passage mapping for indexed material.
- Passage updates are delete-and-create when source hashes change.
- Agents may search attached archives, but shared archives should be written by Den/curate indexers, not collaboratively maintained by every agent.

Recommended archive types:

| Archive | Purpose |
|---|---|
| Bear curated archive | Shared semantic recall over selected `core/` summaries, approved proposals, and durable references. |
| Cabinet Mission archive | Optional semantic recall for a Cabinet Mission, shared by assigned Bears/roles when needed. |
| Role-local archive | Optional later role-specific long-tail recall; should not duplicate `core/`. |

`core/` remains canonical shared orientation for a Bear and its Domains. Letta Archives provide fuzzy recall over selected derived passages.

For a Bear with no cross-Bear Mission needs, the Bear curated archive may be sufficient. Add Cabinet Mission archives only when the Cabinet Mission needs semantic recall shared across Bears or role agents.

## BEARS Reflection

BEARS **Reflection** is the auditable background review and learning system. Memory curation is one Reflection lane; other lanes may handle archive indexing, introspection, skill review, health checks, cleanup, and human-review escalation.

Reflection may be triggered by heartbeat, manual request, memory write, task completion, proposal creation, tool failure, or other system events. Heartbeat cadence is throttled: active Bears can reflect more frequently than dormant Bears, while every run remains bounded by budgets and policy.

## Relationship to Letta reflection

Letta Code reflection should be used where Letta Code is the runtime, such as `talk` and `work`. BEARS Reflection should not duplicate Letta Code's local reflection for those roles.

`pair` and `watch` are API-direct. Do not add separate pair/watch dream agents initially. Instead:

- `pair` writes role-local entries;
- `watch` writes observations/logs;
- `curate` performs autonomous cross-role review, consolidation, and `core/` cleanup.

Letta conversation compaction remains Letta's responsibility. BEARS curation is durable memory governance, not context-window management.

## Situation briefings

Use **situation**, not “current context,” for the trusted Den briefing an agent can request about the current interaction.

Planned tool name:

| Canonical | Provider-safe |
|---|---|
| `den.situation.get` | `den_situation_get` |

A situation briefing may include:

- bear identity,
- role,
- user/channel/session identity,
- active mission hints,
- allowed memory scopes,
- relevant Cabinet spaces,
- policy reminders,
- MemFS health,
- available tools.

The word “context” should be avoided here because it can be confused with model context windows or compiled prompt context.

## Tool direction

Prefer flexible tools with semantic metadata over rigid tools for every category.

Important planned tools:

| Canonical | Provider-safe | Purpose |
|---|---|---|
| `den.situation.get` | `den_situation_get` | Trusted interaction situation briefing. |
| `den.memory.write_entry` | `den_memory_write_entry` | Write a role-local entry with kind, refs, lifecycle, and provenance. |
| `den.memory.request_review` | `den_memory_request_review` | Request Reflection curation of role-local memory without writing shared memory directly. |
| `den.memory.tree` | `den_memory_tree` | Browse allowed memory paths. |
| `den.memory.read` | `den_memory_read` | Read allowed memory files/entries. |
| `den.memory.search` | `den_memory_search` | Search memory by text, role, kind, refs, and lifecycle. |
| `den.memory.semantic_search` | `den_memory_semantic_search` | Search Letta Archives attached to or governed for this Bear/role. |
| `den.memory.history` | `den_memory_history` | Inspect memory history. |
| `den.memory.status` | `den_memory_status` | Inspect MemFS health. |

The retired `den.write_note` / `den_write_note` pair tool is replaced by `den.memory.write_entry` / `den_memory_write_entry`; notes are written with `kind: note`.

`den.memory.request_review` supersedes narrower producer-side memory proposal names such as `propose_core_update` or `propose_core_write`. A role can provide a `suggested_action`, but `curate` decides the final outcome.

## Role-local path conventions

Use predictable paths, but keep semantics in metadata:

```text
<role>/notes/
<role>/logs/
<role>/decisions/
<role>/reflections/
<role>/scratch/
<role>/summaries/
```

Reserved schema-owned paths should remain behind dedicated tools:

```text
<role>/tasks/
<role>/results/
<role>/observations/
<role>/subscriptions/
```

Do not create rigid directories such as `mission-notes/` by default. Use `kind: note` plus `mission_ref` instead.

## Future operator UI

The next product step is a UI that allows human operators to browse, search, and inspect a Bear's memory.

The UI should support:

- browsing by role and path,
- filtering by kind,
- filtering by mission/person/knowledge references,
- filtering by lifecycle,
- inspecting full entries and frontmatter,
- viewing provenance,
- viewing commit/history metadata,
- seeing MemFS health/quarantine status,
- opening Cabinet links when present,
- making clear that many memories have no Cabinet mapping by design.

## Related docs

- [Semantic Bear Memory ADR](../architecture/adr/semantic-bear-memory.md)
- [Memory Model](../concepts/MEMORY_MODEL.md)
- [Bear Memory Tool Boundary ADR](../architecture/adr/bear-memory-tool-boundary.md)
- [MemFS Sidecar Repo Views ADR](../architecture/adr/memfs-sidecar-repo-views.md)
- [Multi-User Memory ADR](../architecture/adr/multi-user-memory.md)
