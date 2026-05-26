# ADR: SQLite-First Canonical Store for Bear Agent Memory and Tasks

**Status:** Proposed
**Date:** 2026-05-26
**Deciders:** Hans

## Context

We are deprecating Letta and need a new canonical storage model for Bear agent memory and task state.

The replacement should optimize for:

- simplicity,
- low maintenance,
- legibility to developers,
- direct use from Rust services,
- and modest-concurrency append-oriented workloads.

The primary records we need to store are:

- role-scoped memory objects,
- shared/promoted memory,
- discovered references,
- tasks,
- and task or memory history.

We expect the data model to be:

- append-only by default,
- role-scoped for most memory writes,
- review-mediated for promotion into shared memory,
- and event-oriented for task state.

Earlier filesystem-and-git approaches remain attractive for human-authored artifacts, but they introduce awkward conflict handling for live machine-written state. Postgres remains a strong option for high-coordination workloads, but this Bear state layer does not require a separate operational database service if an embedded engine is sufficient.

We also want the Bear state layer to remain distinct from Den's administrative Postgres database.

## Decision

BEARS will use **SQLite as the canonical store for Bear agent memory and tasks**.

Each Bear will have a canonical SQLite database managed by Rust services through **`sqlx`**.

SQLite will store:

- role-scoped memory records,
- shared/promoted memory records,
- discovered references,
- task records,
- task event history,
- promotion/review records,
- and change-tracking metadata.

All writes to a Bear database will go through a **single logical write path** owned by application code. Within one process, that should be enforced with a dedicated write pool or connection configured for one writer. Across multiple processes, SQLite itself remains the serialization mechanism through WAL locking and `busy_timeout`; application pooling does not replace engine-level write serialization.

Git remains the canonical store for human-authored artifacts such as:

- skills documentation,
- prompts,
- policies,
- schema definitions and migrations,
- design artifacts,
- tests, fixtures, and examples,
- and optionally exported curated summaries.

Den's Postgres database remains the control-plane and administrative store for Den concerns and is not the canonical store for Bear runtime memory and tasks.

## Why SQLite

### 1. Good fit for the workload

The expected Bear workload is append-oriented, modest in volume, and not a high-contention shared transaction system. SQLite is sufficient for this shape of data if we keep the model disciplined.

### 2. Strong Rust integration

Using SQLite through `sqlx` gives us a direct application-owned storage layer with well-understood migrations and compile-time query checking for schema-backed code.

### 3. Low operational overhead

SQLite avoids introducing a separate Bear-state database product or service layer when an embedded database is enough.

### 4. Developer legibility

SQLite remains easy to inspect with standard SQL and off-the-shelf tools. Legibility comes from clear schema, history views, and good inspection tools, not only from line-oriented file diffs.

### 5. Clear separation from Den Postgres

This keeps Bear runtime state distinct from Den's administrative/control-plane Postgres concerns.

## Why not git/files for canonical Bear state

Git remains a good fit for human-authored and review-oriented artifacts.

Git is a poor fit for:

- live synchronized machine-written state,
- frequent append/update patterns,
- conflict-prone operational objects,
- and task coordination history.

We want the canonical synchronization model for Bear runtime state to be application-aware rather than merge-text-aware.

## Why not Postgres by default

Postgres remains available for future narrow coordination needs, but it is not the default canonical store here because:

- the workload is modest,
- writes are mostly append-only,
- memory writes are role-scoped,
- shared memory is review-mediated,
- and the goal is to avoid unnecessary infrastructure.

## Operational defaults

For the expected concurrent-reader / serialized-writer pattern, the SQLite configuration should default to:

- `PRAGMA journal_mode=WAL;`
- `PRAGMA synchronous=NORMAL;`
- `PRAGMA busy_timeout=5000;`

All writes should go through the single logical write path. In a single process, that should use a dedicated `SqlitePool` or connection configured with `max_connections(1)`. If multiple Bear roles write from separate processes, SQLite WAL locking and `busy_timeout` remain the actual cross-process serialization mechanism.

These defaults are intended to keep the operational model simple while allowing readers to proceed concurrently and writers to wait briefly instead of failing immediately under modest contention.

## Canonical model

Canonical Bear state in SQLite follows these rules:

1. **Append-only by default**
2. **Version/supersession instead of destructive overwrite**
3. **Role-scoped writes for memory**
4. **Explicit review/promotion flow for shared memory**
5. **Task state represented as event history plus projections**
6. **Strong provenance for all canonical records**
7. **Monotonic ordering for replay, export, and derived views**

## Write boundaries

### Memory

Memory writes should be role-scoped wherever possible.

Examples:

- `pair` writes pair-local memory,
- `work` writes work-local memory,
- `watch` writes watch-local memory,
- `review` reads role-local memory and writes shared/promoted records.

This turns shared memory into a promotion pipeline rather than a collaborative editing surface.

### Tasks

Tasks are more nuanced, but they should still be modeled to avoid freeform concurrent mutation.

Task changes should use:

- append-only events,
- constrained legal transitions,
- explicit ownership or handoff,
- and optional lease or claim semantics only where needed.

## Proposed schema shape

The exact schema may evolve, but the canonical structure should include the following families.

### Memory

#### `memory_records`
Append-only records for notes, summaries, references, decisions, and related memory objects.

Suggested fields:

- `memory_id`
- `bear_id`
- `sequence_no`
- `scope_type` (`role_local`, `shared`)
- `scope_role` nullable
- `kind`
- `entity_ref` nullable
- `author_role`
- `author_agent_id`
- `created_at`
- `content_text`
- `metadata_json`
- `supersedes_memory_id` nullable
- `visibility`

#### `memory_links`
Flexible links from memory records to tasks, artifacts, repositories, people, URLs, or other memory records.

Suggested fields:

- `link_id`
- `bear_id`
- `sequence_no`
- `src_memory_id`
- `dst_ref_type`
- `dst_ref`
- `link_type`
- `created_at`

### Tasks

#### `tasks`
Stable task identity and intent.

Suggested fields:

- `task_id`
- `bear_id`
- `sequence_no`
- `created_at`
- `created_by_role`
- `kind`
- `title`
- `intent_text`
- `metadata_json`

#### `task_events`
Canonical append-only task history.

Suggested fields:

- `task_event_id`
- `bear_id`
- `sequence_no`
- `task_id`
- `event_type`
- `actor_role`
- `actor_agent_id`
- `created_at`
- `payload_json`
- `idempotency_key` nullable
- `supersedes_event_id` nullable

Example event types include:

- `created`
- `claimed`
- `started`
- `progressed`
- `blocked`
- `completed`
- `cancelled`
- `handed_off`

### Review / promotion

#### `memory_promotions`
Audit trail for review-mediated promotion and curation.

Suggested fields:

- `promotion_id`
- `bear_id`
- `sequence_no`
- `source_memory_id`
- `target_memory_id`
- `review_agent_id`
- `action`
- `created_at`
- `notes`

### Change tracking

All canonical appended records must participate in one global monotonic ordering.

This ADR chooses an explicit **shared sequence allocator** for that purpose. Each logical write first allocates the next global sequence value from a dedicated sequence mechanism in SQLite, then uses that value in the canonical row or rows written by that operation. We accept this as a serialization point because replay, sync, and "what changed since X" depend on a single Bear-wide ordering.

This ordering supports:

- replay,
- debugging,
- entity history,
- projection refresh,
- and future export workflows.

This ADR does **not** assume downstream replicas or local mirrors by default. If replicas, caches, or exported derivative stores are introduced later, that decision must explicitly re-evaluate drift and consistency tradeoffs rather than treating dual-store operation as the default path.

## Access model

The canonical SQLite database for a Bear is authoritative.

Rust services and runtimes write through application code backed by `sqlx`, not through ad hoc file mutation or unmanaged shared state.

Readers derive current state from append-only canonical records. Materialized projections may be introduced where useful, but append-only records remain the source of truth.

`sqlx` compile-time query checking is part of the intended workflow. That means builds and CI must either run against an appropriate schema-backed database at compile time or commit the `.sqlx` offline cache needed for checked queries.

This ADR does not fix the final hosting topology for each Bear database file. It selects SQLite as the canonical storage engine and `sqlx` as the primary Rust integration path.

## Consequences

### Positive

- avoids git-based conflict workflows for live machine-written state,
- fits the append-only/event-oriented Bear model,
- supports clean separation between role-local and shared memory,
- integrates directly into Rust crates through `sqlx`,
- avoids separate Bear-state database infrastructure,
- keeps Bear state distinct from Den administrative Postgres concerns,
- and preserves git for human-authored artifacts where git works best.

### Negative / tradeoffs

- requires a clear application-owned write topology,
- requires schema and migration discipline,
- requires good inspection and export tooling,
- introduces a canonical database substrate rather than plain files,
- and still requires careful task-transition design.

## Risks and mitigations

### SQLite write topology becomes ambiguous or overloaded
Mitigation: define a clear ownership model for each Bear database file and constrain writes through application code.

### Tasks drift into conflict-prone mutable objects
Mitigation: enforce append-only task events and constrained transitions; avoid arbitrary in-place edits.

### The system becomes opaque to developers
Mitigation: provide SQL-friendly schemas, entity history views, JSON export, simple CLI/UI inspectors, and clear schema documentation.

### Future export or replication becomes awkward
Mitigation: require monotonic sequence-based change tracking from the beginning.

## Non-goals

This ADR does not define:

- exact per-Bear file placement and lifecycle,
- final runtime hosting topology for each canonical SQLite database,
- full task state-machine semantics,
- any integration with Quack (a separate data-access/query protocol under evaluation),
- skills storage beyond retaining git for human-authored artifacts,
- or whether narrow future coordination primitives may use another backing store.

## Follow-up work

- define per-Bear SQLite database lifecycle and ownership,
- define minimal canonical schemas for memory and task events,
- implement the shared global sequence allocator,
- define migration strategy in Rust/`sqlx`,
- define promotion/review flow in detail,
- define task transition rules and ownership semantics,
- define developer inspection/export tooling,
- and evaluate whether Quack (a separate data-access/query protocol) should sit in front of Bear state later.
