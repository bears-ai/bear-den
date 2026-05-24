# Den-Owned Conversation and Runtime Schema

## Purpose

This document proposes the Phase 1 schema for moving BEARS conversation and runtime state out of Letta and into Den-owned persistence.

It is intended to support the migration plan in [`./letta-migration-plan.md`](./letta-migration-plan.md) and should be read alongside the current dependency inventory in [`./letta-dependency-matrix.md`](./letta-dependency-matrix.md).

The goals of this schema are to:

- make Den the source of truth for conversation metadata
- preserve enough fidelity to support current UI, ACP, and operational debugging
- support dual-write during Letta-backed execution
- support later cutover to Den-native runtime execution without another schema redesign

## Design principles

### 1. Separate metadata from execution state from transcript events

The schema should distinguish between:

- **conversation identity and metadata**
- **runtime runs and active execution state**
- **message/event transcript history**
- **tool-call and approval state**

This avoids overloading one table with both durable thread identity and ephemeral run-loop state.

### 2. Preserve provider neutrality

Even though Letta is the current provider, new schema should avoid making Letta the canonical conceptual model.

Where external provider ids are needed, they should be stored as:

- source/provider references
- compatibility mirrors
- opaque runtime handles

rather than as core business identity.

### 3. Support both API-direct and harness-backed roles

The schema should work for:

- `pair`, `curate`, `watch` (API-direct)
- `talk`, `work` (currently harness-backed)

That means the tables must not assume one transport or runtime family.

### 4. Treat the event log as the source for read-model updates

The append-only event log should be the primary source for read-model and projection updates.

That means:

- write-side runtime state changes emit structured events
- admin and UI views are served from explicit projections or read models derived from those events
- event append remains separate from projection/update concerns

For BEARS, this is especially useful because:

- ACP runtime status needs a trustworthy audit trail
- admin surfaces need to explain what happened, not just current row state
- migration off Letta will be easier if Den can compare projected state with provider-derived behavior

The recommended shape remains **conversation-scoped events with optional `run_id` linkage**, rather than a strictly run-scoped event table, because some important state transitions are thread-level rather than run-level.

### 5. Model the event stream, not just flattened messages

For migration, debugging, and replay, Den should preserve fine-grained event history, not just final assistant/user text.

That includes:

- message creation
- stream chunks / partial output if needed
- tool call requests
- tool results
- approval requests and responses
- run lifecycle events
- cancellation and error events
- thread-level metadata changes such as title/archive/provider-sync transitions

### 6. Treat messages as transcript artifacts, not runtime state

Messages should primarily represent the durable transcript/readable history.

They should not be the canonical place to infer:

- pending tool execution
- approval state
- suspension/resume state
- cancellation state

Those concerns belong in first-class runtime tables and the append-only event stream.

### 7. Keep tool calls and approvals as independent state machines

Tool calls and approvals should be represented as first-class records, not embedded solely in messages or inferred from run status.

This is important because:

- a run may be blocked on approval without being failed or cancelled
- a tool call may be requested, approved, dispatched, completed, denied, or cancelled independently of transcript rendering
- suspend/resume and recovery should be row/state transitions, not message archaeology

### 8. Preserve provider neutrality while keeping migration visibility explicit

Provider references should be stored as nullable compatibility fields, not as canonical identities.

During migration, this means Letta-origin ids can be stored in dedicated provider reference columns so that:

- Den ids remain canonical
- dual-write and verification are straightforward
- provider-specific fields can be removed cleanly later

### 9. Be compatible with staged adoption

The schema should support:

- Letta-backed dual-write
- mixed per-role runtime families
- incremental read cutover in UI/admin surfaces

## Scope

This schema is for:

- conversation/thread metadata
- runtime run records
- transcript/event history
- tool-call and approval lifecycle
- runtime-provider references

This schema is **not** for:

- canonical Bear memory content in MemFS/git
- semantic retrieval indexes
- work-plan/task-intent domain records already modeled elsewhere
- skill manifests and proposals already modeled elsewhere

## Recommended entity model

The proposed model has seven primary entities:

1. `runtime_instances`
2. `conversations`
3. `conversation_messages`
4. `conversation_events`
5. `conversation_tool_calls`
6. `conversation_approvals`
7. `conversation_runs`

Optional eighth/ninth entities are recommended for scale and debugging:

8. `conversation_participants`
9. `run_cancellations`

## 1. `runtime_instances`

### Purpose

Provider-neutral registry of role runtime identities.

This generalizes the current `bear_agents.letta_agent_id` concept into a runtime handle that can represent:

- Letta API-direct role runtimes
- Letta Code / Codepool harness runtimes
- future Den-native runtimes

### Relationship to existing schema

This should eventually subsume or complement the provider-specific identity role currently carried by `bear_agents`.

Short term, this can coexist with `bear_agents`.

### Suggested columns

| Column | Type | Notes |
|---|---|---|
| `id` | UUID PK | Den-owned identity |
| `bear_id` | UUID FK -> `bears(id)` | owning Bear |
| `role` | TEXT | `talk`, `pair`, `curate`, `work`, `watch` |
| `runtime_family` | TEXT | e.g. `letta_api`, `letta_code`, `den_native`, `codepool_native` |
| `runtime_provider` | TEXT | vendor/service label, e.g. `letta`, `den`, `codepool` |
| `runtime_handle` | TEXT | opaque provider handle; may mirror Letta agent id initially |
| `status` | TEXT | `pending`, `provisioning`, `ready`, `drifted`, `failed`, `disabled` |
| `config_hash` | JSONB | provider-neutral desired config hash |
| `runtime_policy_hash` | TEXT NULL | optional extracted/runtime policy fingerprint |
| `last_reconciled_at` | TIMESTAMPTZ NULL | when desired/applied config last matched |
| `last_error` | TEXT NULL | latest provider/runtime error |
| `created_at` | TIMESTAMPTZ | default now() |
| `updated_at` | TIMESTAMPTZ | default now() |

### Constraints

- unique `(bear_id, role)`
- optional unique `(runtime_family, runtime_handle)` when handle non-empty

### Why this table matters

Without this abstraction, Den remains locked into the assumption that a role runtime is a Letta agent.

## 2. `conversations`

### Purpose

Canonical thread metadata owned by Den.

This becomes the source of truth for:

- thread identity
- thread title
- archive/delete status
- runtime family/provider association
- human/session association
- user-facing and operator-facing listing state

### Suggested columns

| Column | Type | Notes |
|---|---|---|
| `id` | UUID PK | Den-owned conversation identity |
| `bear_id` | UUID FK -> `bears(id)` | owning Bear |
| `role` | TEXT | runtime role that owns the thread |
| `runtime_instance_id` | UUID FK -> `runtime_instances(id)` NULL | runtime that currently serves the thread |
| `surface` | TEXT | `web`, `acp`, `scheduler`, `watch`, `internal`, etc. |
| `human_user_id` | INTEGER FK -> `users(id)` NULL | authenticated human when applicable |
| `session_key` | TEXT NULL | ACP session id, web session id, scheduler dispatch key, etc. |
| `title` | TEXT NULL | Den-owned current title |
| `title_source` | TEXT NULL | `derived`, `human`, `runtime_sync`, `system` |
| `archived_at` | TIMESTAMPTZ NULL | Den archive state |
| `archived_by_user_id` | INTEGER FK -> `users(id)` NULL | actor if human-archived |
| `deleted_at` | TIMESTAMPTZ NULL | soft-delete marker |
| `provider_conversation_ref` | TEXT NULL | external conversation id, e.g. Letta conversation id |
| `provider_thread_ref` | TEXT NULL | optional harness/provider thread/session reference |
| `provider_metadata` | JSONB NOT NULL DEFAULT '{}'::jsonb | external compatibility fields |
| `last_message_at` | TIMESTAMPTZ NULL | latest durable transcript message time |
| `last_run_at` | TIMESTAMPTZ NULL | latest execution time |
| `created_at` | TIMESTAMPTZ | default now() |
| `updated_at` | TIMESTAMPTZ | default now() |

### Constraints and indexes

Recommended:

- index on `(bear_id, role, archived_at)`
- index on `(human_user_id, surface, archived_at)`
- partial unique index on `(runtime_instance_id, provider_conversation_ref)` when provider ref present
- index on `session_key`

### Notes

- Den-owned `id` should be the canonical thread identity for BEARS.
- Existing Letta conversation ids should be stored in `provider_conversation_ref` during migration.
- `acp_sessions` can later reference `conversations(id)` instead of carrying the raw conversation id as the main identity.

## 3. `conversation_messages`

### Purpose

Durable user/assistant/system message envelopes for transcript rendering and higher-level history operations.

This is the table UI and most read paths should eventually query first.

### Suggested columns

| Column | Type | Notes |
|---|---|---|
| `id` | UUID PK | Den-owned message id |
| `conversation_id` | UUID FK -> `conversations(id)` | owning thread |
| `run_id` | UUID FK -> `conversation_runs(id)` NULL | run that produced/received the message |
| `role` | TEXT | `user`, `assistant`, `system`, `tool`, `approval`, `event` |
| `author_type` | TEXT | `human`, `bear`, `runtime`, `system`, `tool`, `external_source` |
| `author_ref` | TEXT NULL | user id, runtime handle, tool name, source id |
| `content_text` | TEXT NULL | normalized visible text |
| `content_json` | JSONB NOT NULL DEFAULT '{}'::jsonb | structured payload |
| `status` | TEXT | `complete`, `partial`, `error`, `cancelled`, `hidden` |
| `visibility` | TEXT | `user_visible`, `operator_visible`, `internal_only` |
| `provider_message_ref` | TEXT NULL | external message id when present |
| `sequence_no` | BIGINT | monotonically increasing per conversation |
| `created_at` | TIMESTAMPTZ | message time |
| `updated_at` | TIMESTAMPTZ | default now() |

### Constraints and indexes

Recommended:

- unique `(conversation_id, sequence_no)`
- index on `(conversation_id, created_at)`
- index on `provider_message_ref`
- index on `(conversation_id, visibility, created_at)`

### Notes

This is intentionally message-oriented and does not try to store every granular event. That belongs in `conversation_events`.

## 4. `conversation_events`

### Purpose

Append-only low-level event log for replay, debugging, streaming diagnostics, and migration verification.

This is the table that should capture the fidelity currently hidden in Letta streaming and approval variants.

### Suggested columns

| Column | Type | Notes |
|---|---|---|
| `id` | UUID PK | Den-owned event id |
| `conversation_id` | UUID FK -> `conversations(id)` | owning thread |
| `run_id` | UUID FK -> `conversation_runs(id)` NULL | associated run |
| `message_id` | UUID FK -> `conversation_messages(id)` NULL | if event belongs to a durable message |
| `event_type` | TEXT | see event taxonomy below |
| `event_source` | TEXT | `runtime`, `tool`, `den`, `human`, `external` |
| `event_payload` | JSONB NOT NULL | full structured payload |
| `provider_event_ref` | TEXT NULL | external event id if any |
| `sequence_no` | BIGINT | monotonic per conversation |
| `created_at` | TIMESTAMPTZ | default now() |

### Suggested event types

Examples:

- `run_started`
- `run_completed`
- `run_failed`
- `run_cancel_requested`
- `run_cancelled`
- `message_started`
- `message_delta`
- `message_completed`
- `tool_call_requested`
- `tool_call_dispatched`
- `tool_call_completed`
- `tool_call_failed`
- `approval_requested`
- `approval_granted`
- `approval_denied`
- `conversation_compacted`
- `runtime_warning`
- `provider_sync`

### Constraints and indexes

Recommended:

- unique `(conversation_id, sequence_no)`
- index on `(run_id, created_at)`
- index on `(conversation_id, event_type, created_at)`

### Notes

This table is critical for ACP parity and poisoned-run debugging. It is the best place to preserve fine-grained causality.

It should also be treated as the canonical event log for read-model updates. UI/admin surfaces should preferably read from explicit projections derived from these events rather than reconstructing state ad hoc from multiple write-side tables.

## 5. `conversation_tool_calls`

### Purpose

First-class record of tool invocation lifecycle.

This should support both:

- API-direct tool continuation (`pair`)
- harness-backed tool execution visibility (`talk`, `work`)

### Suggested columns

| Column | Type | Notes |
|---|---|---|
| `id` | UUID PK | Den-owned tool call id |
| `conversation_id` | UUID FK -> `conversations(id)` | owning thread |
| `run_id` | UUID FK -> `conversation_runs(id)` NULL | associated run |
| `message_id` | UUID FK -> `conversation_messages(id)` NULL | originating assistant/tool message |
| `provider_tool_call_ref` | TEXT NULL | Letta or other runtime tool call id |
| `tool_name` | TEXT | canonical BEARS tool name |
| `tool_namespace` | TEXT NULL | e.g. `den`, `acp`, `codepool`, `external` |
| `origin` | TEXT | `runtime_requested`, `human_approved`, `system_dispatched` |
| `status` | TEXT | `requested`, `awaiting_approval`, `dispatched`, `completed`, `failed`, `cancelled`, `denied` |
| `request_payload` | JSONB NOT NULL DEFAULT '{}'::jsonb | requested args/schema payload |
| `result_payload` | JSONB NULL | tool return payload |
| `error_text` | TEXT NULL | normalized error |
| `requested_at` | TIMESTAMPTZ NOT NULL DEFAULT now() | request time |
| `completed_at` | TIMESTAMPTZ NULL | completion time |
| `created_at` | TIMESTAMPTZ NOT NULL DEFAULT now() | bookkeeping |
| `updated_at` | TIMESTAMPTZ NOT NULL DEFAULT now() | bookkeeping |

### Constraints and indexes

Recommended:

- index on `(conversation_id, requested_at)`
- index on `(run_id, status)`
- index on `provider_tool_call_ref`
- index on `(tool_name, requested_at)`

### Notes

This table should be the bridge between runtime behavior and ACP/Den permission logic.

Tool-call lifecycle should remain first-class here even if messages also carry user-visible summaries of tool activity. The transcript is not the source of truth for tool execution state.

## 6. `conversation_approvals`

### Purpose

Normalize approval lifecycle independent of the runtime provider.

This is important because pending approval state is currently one of the most Letta-specific and high-risk parts of ACP behavior.

### Suggested columns

| Column | Type | Notes |
|---|---|---|
| `id` | UUID PK | Den-owned approval request id |
| `conversation_id` | UUID FK -> `conversations(id)` | owning thread |
| `run_id` | UUID FK -> `conversation_runs(id)` NULL | associated run |
| `tool_call_id` | UUID FK -> `conversation_tool_calls(id)` NULL | associated tool call |
| `provider_approval_ref` | TEXT NULL | Letta approval request id or equivalent |
| `approval_kind` | TEXT | `tool_execution`, `plan_approval`, `mutation_permission`, `other` |
| `status` | TEXT | `pending`, `approved`, `denied`, `expired`, `cancelled`, `superseded` |
| `requested_by` | TEXT | `runtime`, `system`, `human` |
| `requested_payload` | JSONB NOT NULL DEFAULT '{}'::jsonb | prompt/context/approval details |
| `decided_by_user_id` | INTEGER FK -> `users(id)` NULL | human decision maker when applicable |
| `decision_payload` | JSONB NULL | approval/denial metadata |
| `requested_at` | TIMESTAMPTZ NOT NULL DEFAULT now() | request time |
| `decided_at` | TIMESTAMPTZ NULL | decision time |
| `created_at` | TIMESTAMPTZ NOT NULL DEFAULT now() | bookkeeping |
| `updated_at` | TIMESTAMPTZ NOT NULL DEFAULT now() | bookkeeping |

### Constraints and indexes

Recommended:

- index on `(conversation_id, status, requested_at)`
- index on `(run_id, status)`
- index on `provider_approval_ref`

### Notes

This table should be used for both current Letta approval interoperability and future Den-native approval state.

Approval lifecycle should remain decoupled from run status. A run may be `awaiting_approval` while the approval record independently transitions through `pending`, `approved`, `denied`, `expired`, or `superseded`.

## 7. `conversation_runs`

### Purpose

Represent each execution attempt/turn/run within a conversation.

This is the main table for run lifecycle, cancellation, errors, and execution-level observability.

### Suggested columns

| Column | Type | Notes |
|---|---|---|
| `id` | UUID PK | Den-owned run id |
| `conversation_id` | UUID FK -> `conversations(id)` | owning thread |
| `runtime_instance_id` | UUID FK -> `runtime_instances(id)` NULL | runtime used for this run |
| `request_id` | UUID NULL | Den/API request correlation id |
| `provider_run_ref` | TEXT NULL | Letta run id or provider equivalent |
| `trigger_type` | TEXT | `user_message`, `tool_continuation`, `event_delivery`, `scheduled_cycle`, `system_retry` |
| `status` | TEXT | `queued`, `running`, `awaiting_tool`, `awaiting_approval`, `completed`, `failed`, `cancel_requested`, `cancelled`, `timed_out` |
| `input_summary` | TEXT NULL | lightweight searchable summary |
| `input_payload` | JSONB NOT NULL DEFAULT '{}'::jsonb | request payload/context refs |
| `output_summary` | TEXT NULL | lightweight summary |
| `error_text` | TEXT NULL | normalized terminal or current error |
| `started_at` | TIMESTAMPTZ NOT NULL DEFAULT now() | start time |
| `ended_at` | TIMESTAMPTZ NULL | terminal time |
| `created_at` | TIMESTAMPTZ NOT NULL DEFAULT now() | bookkeeping |
| `updated_at` | TIMESTAMPTZ NOT NULL DEFAULT now() | bookkeeping |

### Constraints and indexes

Recommended:

- index on `(conversation_id, started_at DESC)`
- index on `(provider_run_ref)`
- index on `(runtime_instance_id, status)`
- index on `(request_id)`

### Notes

This is the primary table ACP should eventually use for active-turn bookkeeping instead of relying on Letta as the source of run lifecycle truth.

## 8. `conversation_participants` (recommended)

### Purpose

Normalize actors associated with a conversation.

This is optional early, but recommended if you expect multiple humans, multiple role surfaces, or external event sources.

### Suggested columns

- `id` UUID PK
- `conversation_id` UUID FK
- `participant_type` TEXT (`human`, `bear_role`, `runtime`, `external_source`, `system`)
- `participant_ref` TEXT
- `display_label` TEXT NULL
- `created_at` TIMESTAMPTZ

## 9. `run_cancellations` (recommended)

### Purpose

Provide explicit auditability for cancellation requests and outcomes.

This is optional if cancellation state is embedded in `conversation_runs`, but recommended because ACP concurrency safety makes cancellation especially important.

### Suggested columns

- `id` UUID PK
- `run_id` UUID FK -> `conversation_runs(id)`
- `requested_by_type` TEXT
- `requested_by_ref` TEXT NULL
- `reason` TEXT NULL
- `status` TEXT (`requested`, `acknowledged`, `completed`, `failed`, `ignored`)
- `provider_payload` JSONB
- `requested_at` TIMESTAMPTZ
- `completed_at` TIMESTAMPTZ NULL

## Relationship to existing tables

## `bear_agents`

Current state:

- stores role-to-`letta_agent_id`
- tracks provisioning status and config hash

Recommendation:

- keep during transition
- add `runtime_instances` rather than immediately replacing `bear_agents`
- later either:
  - merge `runtime_instances` into a generalized `bear_agents`, or
  - deprecate provider-specific fields from `bear_agents`

## `acp_sessions`

Current state:

- stores `conversation_id` as Letta-oriented thread identity
- stores `resolved_conversation_id`
- carries session wiring for ACP lifecycle

Recommendation:

- add `den_conversation_id UUID NULL REFERENCES conversations(id)`
- retain existing raw string fields temporarily as provider compatibility refs
- eventually make `den_conversation_id` canonical

## `archived_conversations`

Current state:

- Den-side patch over Letta archive-list limitations

Recommendation:

- fold this state into `conversations.archived_at` over time
- keep table during migration and backfill into `conversations`
- later deprecate once Den is source of truth

## `acp_session_conversation_titles`

Current state:

- titles live as ACP session metadata fields

Recommendation:

- make `conversations.title` canonical
- continue syncing ACP-specific conveniences if needed

## Suggested migration sequence for schema adoption

## Step 1 — introduce Den-owned canonical conversation identity

Add `conversations` and wire:

- `bear_id`
- `role`
- `surface`
- `human_user_id`
- provider refs

Then add `den_conversation_id` to `acp_sessions`.

## Step 2 — start dual-writing runs and messages

Add:

- `conversation_runs`
- `conversation_messages`
- `conversation_events`

Write these from current Letta-backed execution paths.

## Step 3 — persist tool and approval lifecycle

Add:

- `conversation_tool_calls`
- `conversation_approvals`
- optionally `run_cancellations`

This gives Den enough visibility to replace Letta runtime semantics later.

## Step 4 — move read paths

Repoint:

- web conversation lists
- ACP title/archive reads
- diagnostics surfaces

from Letta APIs to Den-owned tables.

## Step 5 — cut execution over per role

Once Den-native runtime exists, write directly to these tables without Letta as the producing source.

## Example migration-friendly DDL sketch

This is an illustrative sketch, not final migration SQL.

```sql
CREATE TABLE conversations (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    bear_id UUID NOT NULL REFERENCES bears(id) ON DELETE CASCADE,
    role TEXT NOT NULL CHECK (role IN ('talk', 'pair', 'curate', 'work', 'watch')),
    surface TEXT NOT NULL,
    human_user_id INTEGER NULL REFERENCES users(id) ON DELETE SET NULL,
    runtime_instance_id UUID NULL,
    session_key TEXT NULL,
    title TEXT NULL,
    title_source TEXT NULL,
    archived_at TIMESTAMPTZ NULL,
    archived_by_user_id INTEGER NULL REFERENCES users(id) ON DELETE SET NULL,
    deleted_at TIMESTAMPTZ NULL,
    provider_conversation_ref TEXT NULL,
    provider_thread_ref TEXT NULL,
    provider_metadata JSONB NOT NULL DEFAULT '{}'::jsonb,
    last_message_at TIMESTAMPTZ NULL,
    last_run_at TIMESTAMPTZ NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
```

```sql
CREATE TABLE conversation_runs (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    conversation_id UUID NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    runtime_instance_id UUID NULL,
    request_id UUID NULL,
    provider_run_ref TEXT NULL,
    trigger_type TEXT NOT NULL,
    status TEXT NOT NULL,
    input_summary TEXT NULL,
    input_payload JSONB NOT NULL DEFAULT '{}'::jsonb,
    output_summary TEXT NULL,
    error_text TEXT NULL,
    started_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    ended_at TIMESTAMPTZ NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
```

```sql
CREATE TABLE conversation_messages (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    conversation_id UUID NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    run_id UUID NULL REFERENCES conversation_runs(id) ON DELETE SET NULL,
    role TEXT NOT NULL,
    author_type TEXT NOT NULL,
    author_ref TEXT NULL,
    content_text TEXT NULL,
    content_json JSONB NOT NULL DEFAULT '{}'::jsonb,
    status TEXT NOT NULL,
    visibility TEXT NOT NULL DEFAULT 'user_visible',
    provider_message_ref TEXT NULL,
    sequence_no BIGINT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (conversation_id, sequence_no)
);
```

## Suggested enum strategy

Given the current repo style, keep status/type fields as constrained `TEXT` rather than Postgres enums at first.

Reasons:

- easier iteration during migration
- aligns with current migration style
- easier to evolve while runtime semantics settle

If categories stabilize later, some fields can move to enums or domain types.

## Read-model guidance

A useful mental model here is a Kubernetes-style controller or reconciler loop, but aimed at read models rather than external world state.

- the write side records facts and state transitions
- the event stream carries those transitions durably and in order
- projection workers consume the event stream and reconcile derived read models until they reflect the recorded facts

The key difference from Kubernetes reconciliation is that these projectors usually are not trying to make the outside world match a desired spec. Instead, they are trying to make Den-owned read models match what has already happened in the conversation/runtime state machine.

In that sense, `conversation_events` is the event log: the append-only stream of structured events used to update derived read models and operator/UI views.

The recommended pattern is:

- write-side tables record authoritative operational state
- `conversation_events` acts as the append-only event log
- explicit read models or projections serve admin/UI views

This keeps projection logic out of handlers and avoids forcing UI/admin paths to reconstruct state ad hoc from scattered write-side tables.

## User-facing chat transcript

Eventually use a transcript-oriented read model derived primarily from:

- `conversations`
- `conversation_messages`
- selected user-visible `conversation_events` where needed

Filter to `visibility = 'user_visible'`.

## Operator/runtime debugging

Use projections or operator views derived from:

- `conversation_runs`
- `conversation_events`
- `conversation_tool_calls`
- `conversation_approvals`
- optional `run_cancellations`

## ACP active turn status

Use projections derived from:

- latest `conversation_runs` row for current session/conversation
- outstanding `conversation_tool_calls`
- pending `conversation_approvals`
- recent `conversation_events`

This should replace Letta-derived pending-run assumptions over time.

## Open design questions

### 1. Should Den conversation identity be UUID-only or allow stable string ids?

Recommendation:

- use UUID PK internally
- allow provider refs and UI/public ids as separate columns if needed

### 2. Should stream deltas be persisted in full?

Recommendation:

- persist important lifecycle events and final messages always
- persist every delta only if needed for debugging or replay
- if delta volume is high, persist only bounded sampled payloads or a compressed event form

### 3. Should harness-backed and API-direct roles share one transcript model?

Recommendation:

- yes, at the schema layer
- let `surface`, `runtime_family`, and event payloads capture differences

### 4. Should Den backfill all historical Letta messages?

Recommendation:

- not required for Phase 1
- backfill metadata and recent active threads first
- preserve provider refs for on-demand lazy migration if historical depth matters

## Recommended immediate follow-up

1. Add a Phase 1 schema RFC or migration draft under `services/den/migrations/` planning notes.
2. Update `acp_sessions` design to include a Den conversation FK.
3. Define Rust structs for:
   - `Conversation`
   - `ConversationRun`
   - `ConversationMessage`
   - `ConversationToolCall`
   - `ConversationApproval`
4. Identify minimal write points in current Letta-backed flows for dual-write instrumentation.
5. Define which event types are mandatory vs optional for initial rollout.

## Conclusion

The schema proposed here is designed to let Den become the source of truth for conversation and runtime state without forcing an all-at-once runtime cutover.

The most important design choice is to treat:

- conversations
- runs
- messages
- events
- tool calls
- approvals

as **Den-owned first-class records**, while external provider ids remain compatibility references rather than canonical identities.

That structure should make it possible to:

- dual-write during Letta-backed execution
- migrate UI/admin reads off Letta
- replace `watch`, then `curate`, then `pair`
- later replace Codepool/Letta Code without another schema reset
