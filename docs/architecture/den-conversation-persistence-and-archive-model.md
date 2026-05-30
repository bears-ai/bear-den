# Den Conversation Persistence and Archive Model

## Status

Draft proposal for post-Letta ACP runtime persistence. This document defines a Den-owned canonical data model for live conversations, message history, compaction artifacts, and archive/read-model behavior so current runtime compaction work lands inside a stable persistence shape.

## Goals

- Replace Letta as the long-term system of record for ACP/runtime conversation state.
- Preserve an append-only, auditable conversation log for operator/admin and user-facing history retrieval.
- Support prompt assembly from Den-owned live messages plus compaction artifacts.
- Support archival, replay, restore, and migration from Letta-backed history.
- Keep workflow state, work plans, and session metadata linked but not conflated with the transcript log.

## Non-goals

- Defining model-provider-specific internal reasoning retention beyond current ACP-visible policy.
- Designing a full-text search subsystem.
- Designing blob/object storage for large binary artifacts.

## Design principles

1. **Den owns canonical conversation state**
   - After migration, Letta or any model runtime is execution infrastructure, not the source of truth for conversation history.
2. **Append-only event/message log**
   - Messages and compaction events are immutable after commit except for explicit redaction/audit mechanisms.
3. **Separate live transcript from derived state**
   - Workflow state, summaries, compaction artifacts, and archive projections are derived and linked, not mixed into primary message rows.
4. **Stable operator/admin read models**
   - ACP/admin surfaces should read from Den-owned tables or projections, not rehydrate opaque provider state on demand.
5. **Provider-agnostic transcript shape**
   - Persistence should model user/assistant/tool/system semantics without embedding Letta-specific transport assumptions.

## Canonical entities

### 1. conversations

Represents the durable logical conversation.

Suggested fields:

- `id UUID PRIMARY KEY`
- `bear_id UUID NOT NULL`
- `created_by_user_id INTEGER NULL`
- `source_acp_session_id TEXT NULL`
- `current_title TEXT NULL`
- `status TEXT NOT NULL`
  - `active | archived | deleted | migrated`
- `archive_state TEXT NOT NULL DEFAULT 'live'`
  - `live | archived | restored | superseded`
- `provider_binding JSONB NOT NULL DEFAULT '{}'`
  - execution/runtime linkage metadata; not canonical transcript
- `workspace_context JSONB NOT NULL DEFAULT '{}'`
- `created_at TIMESTAMPTZ NOT NULL`
- `updated_at TIMESTAMPTZ NOT NULL`
- `archived_at TIMESTAMPTZ NULL`
- `restored_from_conversation_id UUID NULL`

Notes:
- One ACP session may touch one or more conversations over time, but a conversation is the durable transcript root.
- `provider_binding` may temporarily store Letta/runtime ids during migration.

### 2. conversation_messages

Immutable ordered transcript entries.

Suggested fields:

- `id UUID PRIMARY KEY`
- `conversation_id UUID NOT NULL REFERENCES conversations(id)`
- `sequence_no BIGINT NOT NULL`
- `message_type TEXT NOT NULL`
  - `user | assistant | system | developer | tool_call | tool_result | workflow_event | compaction_marker`
- `role TEXT NULL`
  - normalized visible role where applicable
- `visibility TEXT NOT NULL DEFAULT 'default'`
  - `default | hidden_from_user | admin_only | diagnostic_only`
- `content_text TEXT NOT NULL DEFAULT ''`
- `content_json JSONB NOT NULL DEFAULT '{}'`
- `tool_name TEXT NULL`
- `tool_call_id TEXT NULL`
- `source_event_id TEXT NULL`
- `provider_message_id TEXT NULL`
- `created_at TIMESTAMPTZ NOT NULL`
- `created_by_user_id INTEGER NULL`
- `created_by_role TEXT NULL`
- `redacted_at TIMESTAMPTZ NULL`
- `redaction_reason TEXT NULL`

Constraints/properties:
- unique `(conversation_id, sequence_no)`
- append-only ordering
- `content_json` stores structured payloads for tool calls/results and provider metadata
- visible history APIs use `visibility` + message type policy

### 3. conversation_state_snapshots

Optional latest-state checkpoints for fast resume/reconstruction.

Suggested fields:

- `id BIGSERIAL PRIMARY KEY`
- `conversation_id UUID NOT NULL`
- `snapshot_kind TEXT NOT NULL`
  - `runtime_resume | prompt_context | archive_projection`
- `state_json JSONB NOT NULL`
- `source_sequence_no BIGINT NOT NULL`
- `created_at TIMESTAMPTZ NOT NULL`

Purpose:
- speed resume paths without mutating the message log
- support deterministic rebuild/debugging from a known source sequence boundary

### 4. conversation_compaction_artifacts

First-class persisted summary/compaction products.

Suggested fields:

- `id UUID PRIMARY KEY`
- `conversation_id UUID NOT NULL`
- `artifact_kind TEXT NOT NULL`
  - `iterative_summary | semantic_window | archive_summary | migration_summary`
- `policy_version TEXT NOT NULL`
- `trigger TEXT NOT NULL`
- `source_message_start_seq BIGINT NOT NULL`
- `source_message_end_seq BIGINT NOT NULL`
- `source_group_start INTEGER NULL`
- `source_group_end INTEGER NULL`
- `artifact_json JSONB NOT NULL`
- `superseded_by UUID NULL`
- `created_at TIMESTAMPTZ NOT NULL`

Purpose:
- prompt assembly should eventually consume these directly
- artifact lineage must point back to source message range/span

### 5. conversation_compaction_events

Append-only audit stream for compaction evaluation/execution.

Suggested fields:

- `id BIGSERIAL PRIMARY KEY`
- `conversation_id UUID NOT NULL`
- `artifact_id UUID NULL`
- `trigger TEXT NOT NULL`
- `policy_version TEXT NOT NULL`
- `status TEXT NOT NULL`
  - `applied | skipped | failed`
- `event_hash TEXT NOT NULL`
- `boundary JSONB NULL`
- `source_group_start INTEGER NULL`
- `source_group_end INTEGER NULL`
- `diagnostic TEXT NULL`
- `created_at TIMESTAMPTZ NOT NULL`

Notes:
- Current `runtime_compaction_events` is an early version of this entity.
- Long-term, rename or replace current table with this canonical shape.

### 6. conversation_archives

Archive projection/state for durable closed conversations.

Suggested fields:

- `id UUID PRIMARY KEY`
- `conversation_id UUID NOT NULL`
- `archive_version INTEGER NOT NULL`
- `archive_reason TEXT NOT NULL`
  - `user_closed | retention_rollup | migrated | superseded | admin_action`
- `summary_artifact_id UUID NULL`
- `archive_manifest JSONB NOT NULL`
- `created_at TIMESTAMPTZ NOT NULL`
- `restored_at TIMESTAMPTZ NULL`
- `restored_to_conversation_id UUID NULL`

Purpose:
- separate archive lifecycle from the live conversation row
- preserve multiple archive versions if necessary

## Relationships to existing Den tables

### acp_sessions

`acp_sessions` remains session/runtime UI state, not the canonical transcript log.

It should link to:
- current/default `conversation_id`
- session mode / adapter environment / current workflow state

It should not be overloaded to store transcript content.

### bear_work_plans / acp_plan_mode

These remain workflow/task entities.

They may reference:
- `source_acp_session_id`
- `source_conversation_id`

But they are not substitutes for transcript persistence.

### archived_conversations

Existing archived conversation support should converge toward `conversation_archives` as the canonical archive projection table or be refactored into that model.

## Archive semantics

### Live to archived

A conversation may be archived when:
- user closes session and no active runtime remains
- retention policy compacts a long-lived conversation
- migration freezes a Letta-backed conversation into Den-native storage
- an admin/operator explicitly archives it

Archiving does **not** delete message rows.
It creates or updates an archive projection and marks the conversation/archive state accordingly.

### Restore

Restoring should prefer creating a new live conversation linked by provenance rather than mutating old archive rows in place.

Recommended behavior:
- archived conversation remains immutable
- restore creates a new `conversations` row with `restored_from_conversation_id`
- selected summary/artifact state can seed the new live prompt context

## Prompt assembly model

Final target prompt assembly should read from Den-owned state in this order:

1. active instructions / policy
2. workflow state (`acp_sessions`, `acp_plan_mode`, `bear_work_plans`)
3. uncompacted recent `conversation_messages`
4. latest relevant `conversation_compaction_artifacts`
5. optional snapshot acceleration via `conversation_state_snapshots`

Prompt assembly should not require Letta history fetch once migration is complete.

## Migration from Letta

### Transitional phase

During migration, Den may use hybrid state:
- Letta remains source for historical transcript retrieval
- Den persists derived compaction events/artifacts and selected mirrored message rows
- conversation/provider binding is dual-tracked

### Cutover target

At cutover:
- Den-owned `conversations` + `conversation_messages` become canonical
- Letta/runtime provider ids become execution metadata only
- history APIs read only Den persistence

### Import strategy

Suggested migration flow:
1. enumerate Letta conversation bindings
2. create `conversations` rows in Den
3. import historical messages into `conversation_messages`
4. compute/import summary artifacts
5. mark imported provenance in `provider_binding` and migration metadata
6. switch read paths to Den-native sources

## How current compaction work fits

Current implemented pieces map as follows:

- transcript normalization from ACP history -> future `conversation_messages` ingestion logic
- runtime semantic grouping -> compaction engine over Den transcript rows
- iterative summaries -> `conversation_compaction_artifacts`
- `runtime_compaction_events` -> precursor to canonical `conversation_compaction_events`
- ACP history `compaction` / `compaction_history` -> operator/admin read model over canonical compaction tables

So the current work is directionally correct, but still transitional because it depends on Letta-backed history reads.

## Recommended next schema steps

1. Add canonical `conversations` table if not already present in equivalent form.
2. Add `conversation_messages` table with immutable sequence ordering.
3. Add first-class `conversation_compaction_artifacts` table.
4. Rename or supersede `runtime_compaction_events` with canonical conversation-scoped naming.
5. Add `conversation_id` linkage from `acp_sessions` to canonical conversation rows.
6. Introduce importer/mirror logic from Letta history into `conversation_messages`.

## Decision

Adopt this model as the intended Den-owned persistence target for post-Letta ACP/runtime conversation storage and archives. Current runtime compaction event persistence should be treated as a transitional subset of the future canonical conversation persistence layer.
