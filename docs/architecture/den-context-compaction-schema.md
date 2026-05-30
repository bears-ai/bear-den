# Den Context Compaction Schema Direction

This document defines the initial schema direction for Den-owned context compaction.

It implements the persistence-oriented portion of the [Den Context Compaction Contract](./den-context-compaction-contract.md) and should guide database and read-model work for transcript ownership, semantic grouping, derived compaction artifacts, and compaction telemetry.

## Goals

The schema direction must ensure that Den can:

- retain canonical transcript history as the source of truth,
- represent semantic groups as compaction units,
- store derived compaction artifacts without conflating them with transcript or durable memory,
- attribute compaction outcomes to policy version and trigger,
- and expose sufficient provenance for debugging, rebuilds, and operator inspection.

## Core separation of concerns

The schema should model four distinct concerns.

### 1. Canonical transcript state

Canonical transcript state stores the durable ordered session history.

It should be able to represent:

- user messages,
- assistant messages,
- tool calls,
- tool results,
- approval requests and decisions,
- workflow/workplan state updates,
- artifact/reference events,
- and system/runtime events needed for replay or diagnostics.

This state is append-oriented and remains the source of truth.

### 2. Semantic-group metadata

Semantic-group metadata stores the grouping of transcript rows into compaction units.

It should be able to represent:

- group kind,
- conversation/session ownership,
- source transcript range,
- message/event count,
- protected-floor status,
- and lifecycle state if regrouping or reclassification is ever needed.

### 3. Derived compaction artifacts

Derived compaction artifacts store prompt-assembly artifacts derived from transcript history.

They should be able to represent:

- artifact kind,
- artifact payload,
- source semantic-group range,
- policy version,
- trigger reason,
- creation timestamp,
- and supersession/rebuild lineage when strategy versions change.

Derived artifacts are not canonical transcript and not durable memory.

### 4. Compaction events and telemetry

Compaction-event state records when compaction happened and why.

It should be able to represent:

- trigger type,
- conversation/session ownership,
- policy version,
- compacted source group range,
- artifact ids produced,
- token estimate or pressure signals before/after where available,
- and success/failure diagnostics.

## Recommended logical model

The exact table names may change, but the logical model should cover these entities.

### A. Transcript events

Recommended logical fields:

- `id`
- `conversation_id`
- `runtime_binding_id` or equivalent runtime/session binding key where applicable
- `sequence_no` or another stable ordering key
- `event_kind`
- `role` where applicable
- `message_id` or provider/source id where applicable
- `payload_json`
- `created_at`

Notes:

- Transcript ordering should be Den-owned and deterministic even if upstream/provider ids are messy.
- Provider/source ids may be stored for reconciliation, but should not be the sole ordering key.

### B. Semantic groups

Recommended logical fields:

- `id`
- `conversation_id`
- `group_index`
- `group_kind`
- `start_event_id`
- `end_event_id`
- `event_count`
- `protected_floor_kind` or equivalent protected marker
- `protected_until_state` or equivalent eligibility marker where needed
- `created_at`
- `updated_at`

Notes:

- `group_index` should provide stable ordering among groups within a conversation.
- Protection should be explicit rather than inferred only at query time if that helps deterministic policy behavior.

### C. Compaction artifacts

Recommended logical fields:

- `id`
- `conversation_id`
- `artifact_kind`
- `artifact_version`
- `policy_version`
- `trigger_kind`
- `source_group_start`
- `source_group_end`
- `payload_json`
- `supersedes_artifact_id` nullable
- `created_at`

Notes:

- The payload should remain structured JSON rather than opaque text-only blobs where practical.
- Artifact version and policy version should both exist if payload schema and policy evolve independently.

### D. Compaction events

Recommended logical fields:

- `id`
- `conversation_id`
- `trigger_kind`
- `policy_version`
- `source_group_start`
- `source_group_end`
- `retained_group_count`
- `compacted_group_count`
- `artifact_ids` or a join table
- `token_estimate_before` nullable
- `token_estimate_after` nullable
- `status`
- `diagnostic_json` nullable
- `created_at`

Notes:

- If multiple artifacts can be produced in one compaction run, prefer a join table over packing ids into one field.

## Provenance requirements

The schema must preserve enough provenance to answer these questions:

- Which transcript events contributed to this semantic group?
- Which semantic groups were compacted into this artifact?
- Which policy version and trigger produced this artifact?
- Which artifact superseded a prior artifact?
- Which compaction event created or failed to create this artifact?

If the schema cannot answer those questions, it is not sufficient for v1.

## Rebuild and supersession expectations

The schema should allow Den to rebuild derived compaction artifacts from canonical transcript plus grouping metadata if:

- compaction policy changes,
- artifact schema changes,
- a prior artifact is found to be deficient,
- or operator tooling needs regeneration.

This implies:

- transcript state must remain intact,
- source group provenance must be preserved,
- and artifacts should support explicit supersession rather than silent replacement.

## Read-model implications

Admin/read-model surfaces should eventually be able to show:

- whether a conversation has compaction artifacts,
- the latest artifact kind and creation time,
- recent compaction events,
- and enough source-range provenance for debugging.

This does not require exposing raw JSON everywhere, but the schema should make these views inexpensive to construct.

## Relationship to prompt assembly

Prompt assembly should consume:

- active recent transcript groups,
- workflow/workplan state,
- and selected derived compaction artifacts.

The schema should therefore support efficient queries for:

- recent uncompacted groups,
- the latest active artifact set,
- and the current compaction boundary for a conversation.

## Relationship to durable memory

No compaction table should double as durable memory storage.

If a memory-governance workflow promotes facts from a compacted session into durable memory, that should happen through a separate memory subsystem with only reference-level linkage back to transcript or compaction artifacts.

## Minimum v1 schema bar

A v1 schema is acceptable if it provides:

- canonical transcript event retention,
- semantic-group records with stable ordering,
- derived compaction artifacts with source-group provenance,
- compaction event records with trigger and policy attribution,
- and enough linkage to rebuild or supersede artifacts safely.

## Suggested implementation sequence

1. Add transcript event ordering guarantees where missing.
2. Add semantic-group metadata tables or equivalent model support.
3. Add compaction artifact storage with provenance.
4. Add compaction event telemetry storage.
5. Add read-model queries for current artifact/boundary visibility.

## Open questions

- Should semantic-group protection be materialized in storage or computed at compaction time?
- Should artifact payloads be normalized further for reporting, or remain JSON-first in v1?
- What is the best conversation/session binding key for multi-surface runtime history?
- When policy versions change, should artifacts be rebuilt lazily, eagerly, or on operator demand?
