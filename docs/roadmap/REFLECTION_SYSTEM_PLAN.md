# Reflection system shared infrastructure plan

For the canonical role model and current role names, see [bear roles](../architecture/bear-roles.md).
Status: focused shared-infrastructure plan. Implementation status and sequencing now live in [Memory Automation Roadmap](MEMORY_AUTOMATION_ROADMAP.md).

This document describes the shared Reflection control-plane infrastructure that supports multiple BEARS maintenance and learning lanes. It intentionally avoids tracking lane-specific implementation phases; use the roadmap for current/pending work.

Related docs:

- [Memory Automation Roadmap](MEMORY_AUTOMATION_ROADMAP.md) — canonical implementation status and sequencing.
- [Reflection System ADR](../architecture/adr/reflection-system.md) — durable architecture decision and risk boundaries.
- [Reflection system concept](../concepts/REFLECTION_SYSTEM.md) — vocabulary, conductor model, heartbeats, and lanes.
- [Reflection run taxonomy](../concepts/REFLECTION_RUN_TAXONOMY.md) — lane names and purposes.
- [Review memory governance plan](CURATE_MEMORY_GOVERNANCE_PLAN.md) — `memory_curate` lane behavior and proposal lifecycle.
- [Pair reflection and work memory plan](PAIR_REFLECTION_AND_WORK_MEMORY_PLAN.md) — pair-local memory sharing boundary.

---

## Scope

Reflection is a Den-owned orchestration layer for bounded background review and learning. It supports lane-specific work such as memory curation, archive indexing, health checks, introspection, skill review, cleanup, and human-review escalation.

This shared infrastructure plan covers:

- run records;
- queueing;
- lane and status vocabulary;
- conversation mapping for role-backed lanes;
- event/audit records;
- locks;
- budgets;
- scheduler and heartbeat policy;
- manual/admin run controls;
- operator visibility.

Lane-specific prompt design, tools, and mutation rules belong in the lane docs and the canonical roadmap.

---

## Goals

Shared Reflection infrastructure should let BEARS:

1. Queue bounded background runs for active and dormant Bears.
2. Track run state and outcomes for auditability.
3. Reuse one queue shape across lanes without forcing all proposal data into one generic table.
4. Route role-backed runs to stable daily conversations when useful.
5. Throttle cadence based on Bear activity, lane cooldowns, locks, and budgets.
6. Surface activity and human-review needs in the UI.
7. Keep Den as the control plane for scheduling and authorization.

---

## Non-goals

- Do not build unrestricted autonomous self-modification.
- Do not let Reflection bypass Den tool authorization.
- Do not let every lane mutate every resource.
- Do not force memory proposals, skill proposals, and archive indexes into one generic proposal table.
- Do not require memory curation to implement skill application.
- Do not make Letta Archives canonical memory.
- Do not run destructive cleanup without explicit policy and audit.

---

## Architecture shape

Shared infrastructure:

- queue/run records;
- optional per-run item records;
- conversation mapping for daily role-backed runs;
- event/audit records;
- locks or advisory locks;
- budgets;
- retry/cooldown policy;
- heartbeat scheduler;
- manual run API;
- UI activity views.

Lane-specific infrastructure:

- lane tools;
- permissions;
- proposal/index tables;
- target resources;
- prompts;
- approval rules;
- risk policy.

Initial lanes:

| Lane | Shared-infrastructure implication |
|---|---|
| `memory_curate` | First queued role-backed lane; uses memory proposals and daily review conversations. |
| `archive_index` | Later Den/indexer lane; uses archive source-to-passage mappings. |
| `health_check` | Low-risk lane useful for validating scheduler, events, and UI. |
| `introspection` | Later role-backed or Den-assisted lane over telemetry. |
| `skill_review` | Later proposal-review lane separate from memory proposals. |
| `skill_apply` | Later strongly policy-gated lane. |
| `cleanup` | Later lane requiring reviewed/superseded state and strict safety policy. |
| `human_review_escalation` | Queue/activity lane for surfacing sensitive or blocked outcomes. |

---

## Implemented MVP storage

### `bear_reflection_runs`

One row per bounded queued/running/completed Reflection run.

Fields:

- `id uuid primary key`
- `bear_id uuid not null`
- `lane text not null`
- `trigger text not null`
- `status text not null`
  - `queued`
  - `running`
  - `completed`
  - `failed`
  - `cancelled`
  - `skipped`
  - `needs_human_review`
- `role_agent_id text null`
- `conversation_id text null`
- `conversation_key text null`
- `conversation_date date null`
- `input_summary jsonb not null default '{}'`
- `output_summary jsonb not null default '{}'`
- `error text null`
- `started_at timestamptz null`
- `completed_at timestamptz null`
- `created_at timestamptz not null default now()`

For the first pair-reflection handoff path, Den enqueues:

```text
lane = memory_curate
trigger = pair_reflection
status = queued
input_summary = { proposal_ids: [...] }
```

### `bear_reflection_run_items`

Optional normalized per-run item links.

Fields:

- `id uuid primary key`
- `run_id uuid not null references bear_reflection_runs(id)`
- `item_kind text not null`
- `item_id text not null`
- `status text not null default 'queued'`
- `created_at timestamptz not null default now()`

MVP can keep proposal IDs in `input_summary`; this table is available when the runner needs item-level tracking.

### `reflection_conversations`

Daily Bear + lane conversation mapping for role-backed Reflection runs.

Fields:

- `id uuid primary key`
- `bear_id uuid not null`
- `role_agent_id text null`
- `lane text not null`
- `conversation_date date not null`
- `conversation_key text not null`
- `conversation_id text null`
- `created_at timestamptz not null default now()`
- `last_used_at timestamptz not null default now()`

Unique key:

```text
bear_id + lane + conversation_date
```

For `memory_curate`, use:

```text
conversation_key = memory_curate:YYYY-MM-DD
```

---

## Future shared storage

### `bear_reflection_events`

Use event rows for detailed audit trails that should not be packed into `output_summary`.

Suggested fields:

- `id uuid primary key`
- `run_id uuid null references bear_reflection_runs(id)`
- `bear_id uuid not null`
- `lane text not null`
- `event_kind text not null`
- `severity text not null default 'info'`
- `message text not null`
- `refs jsonb not null default '{}'`
- `created_at timestamptz not null default now()`

### Locks

Use `bear_reflection_locks` or PostgreSQL advisory locks.

Lock dimensions:

- Bear;
- lane;
- target scope, such as `core`, archive id, proposal id, or role branch.

The scheduler and runners should avoid concurrent mutation of the same Bear/lane/scope.

### Budgets

Budgets may stay in run summaries at first, then become first-class columns or JSONB fields if needed.

Useful budget keys:

- `max_wall_ms`
- `max_tool_calls`
- `max_model_calls`
- `max_files_read`
- `max_files_written`
- `max_proposals_reviewed`
- `max_core_files_modified`
- `max_archive_passages_changed`
- `max_deletions`
- `max_new_proposals`
- `max_cost_cents`

Budget exhaustion should produce a bounded status and event, not an unbounded continuation.

---

## Scheduler and heartbeat policy

The scheduler computes due Reflection lanes per Bear.

Inputs:

- Bear enabled/disabled state;
- recent Bear activity;
- recent role sessions;
- recent memory writes;
- pending proposal counts;
- recent task/run completions;
- watch observations;
- last run per lane;
- last failure per lane;
- active locks;
- configured minimum/maximum cadence;
- global and per-Bear cost budgets.

Example cadence policy:

| Bear state | Example heartbeat behavior |
|---|---|
| Active | Check due lanes every few minutes; run only lanes with work and budget. |
| Warm | Check less frequently; process pending proposals and archive drift. |
| Dormant | Occasional maintenance heartbeat; mostly health and stale-queue review. |
| Disabled | No autonomous Reflection except explicit admin/manual operations. |

Each lane still enforces its own minimum interval and cooldown. A rapid Bear heartbeat does not mean every lane runs every time.

---

## Runner responsibilities

A generic runner should:

1. Claim a queued run with a lock.
2. Mark the run `running` and set `started_at`.
3. Resolve or create a role conversation if the lane is role-backed.
4. Build bounded lane-specific input context from `input_summary` and policy.
5. Invoke the lane implementation with lane-approved tools only.
6. Record output summary, events, item states, and errors.
7. Mark the run `completed`, `failed`, `skipped`, `cancelled`, or `needs_human_review`.
8. Release locks.
9. Surface activity in UI.

Semantic decisions belong to the lane implementation, usually a role agent such as `review`, not to ad hoc Den heuristics in the generic runner.

---

## UI expectations

Shared Reflection UI should show:

- queued/running/recent runs by Bear and lane;
- run detail with input/output summaries;
- item links such as proposal IDs;
- conversation ID/key/date for role-backed runs;
- status and errors;
- human-review-needed runs;
- manual run controls;
- scheduler/cadence status.

Lane-specific UI, such as memory proposal review, stays in the lane docs and roadmap.

---

## Canonical implementation tracker

Do not duplicate phase checklists in this document. Track current status, next steps, and cross-lane sequencing in [Memory Automation Roadmap](MEMORY_AUTOMATION_ROADMAP.md).
