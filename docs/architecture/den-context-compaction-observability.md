# Den Context Compaction Observability

This note describes the initial observability shape for Den-owned context compaction.

It complements:

- [Den Context Compaction Contract](./den-context-compaction-contract.md)
- [Den Context Compaction Schema Direction](./den-context-compaction-schema.md)
- [Context Compaction Guide](../guides/context-compaction-guide.md)

## Why observability matters

Compaction must remain explainable.

Operators and developers need to answer questions such as:

- Did a session compact or skip compaction?
- What trigger caused compaction evaluation?
- Which source range was compacted?
- What artifact was produced?
- Which policy version was used?
- Why did compaction skip or fail?

Without this visibility, compaction becomes hard to debug and hard to trust.

## Initial event shape

The current implementation introduces a first runtime event model in code:

- `RuntimeCompactionEventStatus`
  - `Applied`
  - `Skipped`
  - `Failed`
- `RuntimeCompactionEvent`

The initial event carries:

- `conversation_id`
- `trigger`
- `policy_version`
- `status`
- `boundary`
- `source_group_start`
- `source_group_end`
- `artifact`
- `diagnostic`

This is intentionally small but enough to support:

- operator/debug visibility,
- provenance tracking,
- regression assertions,
- and eventual persistence/reporting integration.

## Applied vs skipped events

### Applied

An applied event should record:

- the trigger,
- the selected source group range,
- the retained/compacted boundary,
- the artifact produced,
- and the policy version.

### Skipped

A skipped event should record:

- the trigger,
- the policy version,
- and a diagnostic reason such as:
  - no eligible groups,
  - only protected spans remain,
  - recency window prevented compaction.

Skipped events are important because they explain why Den did **not** compact even when evaluation happened.

## Relationship to future persistence

The event shape is compatible with the schema direction described in the compaction schema note.

Over time, these runtime events can back:

- stored compaction-event rows,
- admin/debug read models,
- session history inspection,
- and rollout analysis.

## Relationship to continuation evaluation

Observability events and continuation evaluation serve different purposes.

- observability explains **what compaction did**,
- continuation evaluation checks **whether continuation quality was preserved**.

Both are needed for a safe rollout.

## Current implementation status

As of the current slice, Den has:

- initial runtime compaction event types,
- helper builders for applied and skipped events,
- and unit tests for event provenance and diagnostics.

The next natural step is to connect these events to actual runtime execution and any operator-facing read models.
