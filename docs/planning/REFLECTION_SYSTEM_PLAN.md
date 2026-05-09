# Reflection system implementation plan

Status: proposed implementation plan.

This plan introduces BEARS' broader **Reflection** system before starting the next memory-governance build-out. Reflection is the auditable background review and learning system that includes memory curation, archive indexing, introspection, skill review, health checks, cleanup, and human-review escalation.

Related docs:

- [Reflection System ADR](../architecture/adr/reflection-system.md)
- [Reflection system concept](../concepts/REFLECTION_SYSTEM.md)
- [Semantic Bear Memory ADR](../architecture/adr/semantic-bear-memory.md)
- [Memory model](../concepts/MEMORY_MODEL.md)
- [Capabilities and skills](../concepts/CAPABILITIES_AND_SKILLS.md)
- [Curate memory governance plan](CURATE_MEMORY_GOVERNANCE_PLAN.md)
- [Memory tools implementation plan](MEMORY_TOOLS_IMPLEMENTATION_PLAN.md)

---

## Goal

Create a shared, policy-governed Reflection infrastructure that can safely run multiple autonomous maintenance and learning lanes.

Reflection should let BEARS:

1. Schedule bounded background runs for active and dormant Bears.
2. Throttle heartbeat cadence based on Bear activity and policy.
3. Track Reflection runs and events for auditability.
4. Run memory curation without conflating it with skill adaptation.
5. Queue and process memory proposals.
6. Queue and process skill proposals later.
7. Trigger archive indexing from curated canonical changes.
8. Surface activity and human-review needs in the UI.

---

## Non-goals

- Do not build unrestricted autonomous self-modification.
- Do not let Reflection bypass Den tool authorization.
- Do not let every lane mutate every resource.
- Do not require memory curation to implement skill application.
- Do not replace Letta conversation compaction.
- Do not make Letta Archives canonical memory.
- Do not run destructive cleanup without explicit policy and audit.
- Do not use "subconscious" as the official product/API term.

---

## Architecture shape

Reflection should be implemented as a Den-owned orchestration layer with lane-specific policies.

Shared infrastructure:

- scheduler;
- heartbeat throttling;
- run records;
- event/audit records;
- locks;
- budgets;
- retry/cooldown policy;
- manual run API;
- UI activity views.

Lane-specific infrastructure:

- tools;
- permissions;
- proposal tables;
- target resources;
- approval rules;
- prompts;
- risk policy.

Initial lanes:

| Lane | Initial implementation stance |
|---|---|
| `memory_curate` | First full lane after shared run/event scaffolding. |
| `archive_index` | Add after curated source changes exist. |
| `health_check` | Can be introduced early because it is low-risk. |
| `introspection` | Add once enough run/task/tool telemetry exists. |
| `skill_review` | Add after memory proposals and evidence collection are working. |
| `skill_apply` | Later, human-gated or tightly policy-gated. |
| `cleanup` | Add after reviewed/superseded state exists. |
| `human_review_escalation` | Add alongside proposal queues and UI. |

---

## Data model

### Shared Reflection tables

Suggested table: `bear_reflection_runs`.

Fields:

- `id uuid primary key`
- `bear_id uuid not null`
- `lane text not null`
- `trigger_kind text not null`
  - `heartbeat`
  - `manual`
  - `memory_write`
  - `task_completed`
  - `proposal_created`
  - `schedule`
  - `service_restart`
  - `archive_drift`
  - `tool_failure`
  - `human_feedback`
- `status text not null`
  - `queued`
  - `running`
  - `succeeded`
  - `partial`
  - `failed`
  - `cancelled`
  - `skipped`
- `scope jsonb not null default '{}'`
- `budget jsonb not null default '{}'`
- `budget_used jsonb not null default '{}'`
- `owner_kind text not null`
  - `den`
  - `role_agent`
  - `human`
- `owner_ref text null`
- `started_at timestamptz null`
- `completed_at timestamptz null`
- `error_summary text null`
- `needs_human_review boolean not null default false`
- `created_at timestamptz not null default now()`

Suggested table: `bear_reflection_events`.

Fields:

- `id uuid primary key`
- `run_id uuid null references bear_reflection_runs(id)`
- `bear_id uuid not null`
- `lane text not null`
- `event_kind text not null`
- `severity text not null default 'info'`
- `message text not null`
- `refs jsonb not null default '{}'`
- `created_at timestamptz not null default now()`

Suggested table: `bear_reflection_locks` or equivalent advisory-lock usage.

Lock dimensions:

- Bear;
- lane;
- target scope, such as `core`, archive id, proposal id, or role branch.

### Lane-specific tables

Start with separate typed tables rather than one generic table for every proposal.

| Table | Purpose |
|---|---|
| `bear_memory_proposals` | Memory/core/Cabinet proposal queue. |
| `bear_skill_proposals` | Skill/workflow/prompt proposal queue. |
| `bear_archive_index_entries` | Canonical source to Letta passage mapping. |

---

## Heartbeat scheduler

The scheduler should compute due Reflection lanes per Bear.

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

Active Bears can have a more rapid heartbeat than dormant Bears. Dormant Bears still receive occasional maintenance heartbeats for stale queues, health checks, and deferred proposals.

Example cadence policy:

| Bear state | Example heartbeat behavior |
|---|---|
| Active | Check due lanes every few minutes; run only lanes with work and budget. |
| Warm | Check less frequently; process pending proposals and archive drift. |
| Dormant | Occasional maintenance heartbeat; mostly health and stale-queue review. |
| Disabled | No autonomous Reflection except explicit admin/manual operations. |

Each lane should still enforce its own minimum interval and cooldown. A rapid Bear heartbeat does not mean every lane runs every time.

---

## Budgets

Budgets should be explicit at run start and recorded at completion.

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

Budget exhaustion should produce a `partial` or `succeeded` run with a clear event, not an unbounded continuation.

---

## Phase 0: finalize Reflection vocabulary and docs

Deliverables:

- ADR for Reflection naming and architecture.
- Concept doc for Reflection vocabulary, lanes, heartbeats, and risk boundaries.
- Planning doc for shared scheduler and first lane implementation.
- Links from doc indexes and related memory/skill docs.

Status: documentation slice.

---

## Phase 1: shared run/event foundation

Implement Den DB and service support for:

- `bear_reflection_runs`;
- `bear_reflection_events`;
- run creation/update helpers;
- event logging helpers;
- basic lane enum/string validation;
- status transitions;
- failure recording;
- admin/internal list API.

Deliverable:

- Den can record and display bounded Reflection runs even before autonomous scheduling is enabled.

---

## Phase 2: heartbeat scheduler and throttling

Implement scheduler logic that decides which Bears and lanes are due.

Deliverables:

- per-Bear activity classification: active, warm, dormant, disabled;
- per-lane cadence config;
- minimum/maximum heartbeat intervals;
- cooldown after failures;
- lock checks;
- dry-run logging;
- manual admin trigger for a Reflection lane.

Start with a conservative no-op or health-check lane before invoking agents.

---

## Phase 3: health-check lane

Add a low-risk lane first to validate scheduler, budgets, and UI.

Health checks may inspect:

- MemFS Manager reachability;
- role agent provisioning status;
- stale queues;
- archive index drift counts;
- recent failed tool calls;
- pending human-review items.

Deliverable:

- Reflection can run automatically and produce useful audit events without mutating memory.

---

## Phase 4: memory proposal queue

Add DB-backed memory proposals.

Suggested table: `bear_memory_proposals`.

Important fields:

- Bear ID;
- source role;
- source paths/refs;
- proposal type;
- title;
- summary;
- rationale;
- proposed target;
- proposed content/patch if any;
- status;
- sensitivity;
- requires-human flag;
- reviewer fields;
- result refs;
- timestamps.

Deliverable:

- humans, roles, and future Reflection lanes have a durable memory governance queue.

---

## Phase 5: proposal creation from roles and UI

Add ways to create memory proposals.

Role/tool path:

- add a broad review-request tool such as `den.memory.request_review` / `memory_request_review`;
- allow `pair`, `talk`, `work`, and maybe `watch` to request review of their own memory outputs;
- require source refs where possible instead of embedding all content.

UI path:

- memory browser action to select files;
- create curation proposal;
- choose suggested action;
- add human note/rationale.

Deliverable:

- memory producers and humans can ask `curate` to review memory without granting direct `core/` write access.

---

## Phase 6: `curate` proposal read/review tools

Expose tools for `curate`:

- list pending memory proposals;
- read proposal details;
- read referenced source memory;
- comment on proposal;
- defer proposal;
- reject proposal;
- mark proposal resolved without file mutation.

Deliverable:

- `curate` can process the queue and leave auditable outcomes before it can mutate `core/`.

---

## Phase 7: constrained `core/` write tools

Add structured mutation tools for `curate`.

Possible tools:

- read `core/` file;
- append to approved `core/*.md` files;
- patch approved `core/*.md` files;
- write/replace approved `core/*.md` files;
- create files under approved subdirectories such as `core/results/`;
- mark source proposal resolved with result path/commit.

MemFS Manager may need endpoints for safe Git-backed `core/` writes or patches. Den should own authorization and intent; MemFS Manager should enforce path safety and commit changes.

Deliverable:

- `curate` can consolidate role-local memory into shared Bear `core/` with provenance.

---

## Phase 8: heartbeat-driven memory curation

Wire the `memory_curate` lane into Reflection scheduling.

A bounded run should:

1. Check pending memory proposals.
2. Check recent role-local memory changes if enabled.
3. Review up to the run budget.
4. Update `core/` only through constrained tools.
5. Mark proposal outcomes.
6. Emit Reflection events.
7. Queue archive indexing for touched curated sources.

Deliverable:

- Active Bears can autonomously curate memory more frequently than dormant Bears, with budgets and audit logs.

---

## Phase 9: Reflection UI

Add user/admin visibility.

Views:

- Reflection activity stream;
- run detail page;
- last run per Bear/lane;
- memory proposal queue;
- human-review queue;
- heartbeat/cadence status;
- run-now controls;
- failed/skipped run diagnostics.

Deliverable:

- operators can see what Reflection did, why it ran, what it changed, and what needs review.

---

## Phase 10: cleanup and source lifecycle

Add safe cleanup operations after curation works.

Capabilities:

- mark source memory reviewed;
- mark source memory superseded;
- move source files into role-local reviewed/archive areas;
- delete selected low-risk files under policy;
- compact noisy role-local files;
- surface high-risk cleanup for human review.

Deliverable:

- role-local memory can remain useful without becoming infinite clutter.

---

## Phase 11: archive indexing lane

Add derived semantic indexing for curated sources.

Tasks:

- provision Bear curated archives;
- attach archives to role agents by policy;
- add `bear_archive_index_entries` mapping;
- index selected `core/` summaries, decisions, results, and approved proposal outcomes;
- use delete-and-create for changed sources;
- provide semantic search tools once archives are reliable.

Deliverable:

- Reflection keeps Letta Archives in sync with selected canonical Bear memory.

---

## Phase 12: introspection and skill review

Add behavior-learning without direct self-modification.

Introspection may inspect:

- failed tool calls;
- repeated human corrections;
- repeated successful workflows;
- task failures/successes;
- role-local reflections;
- conflicts between memory, prompts, skills, and policy.

Skill review may create `bear_skill_proposals` for:

- new skill;
- skill revision;
- skill deprecation;
- checklist addition;
- tool-use guidance;
- domain playbook;
- role instruction change;
- human review.

Deliverable:

- Reflection can propose behavior improvements without automatically applying high-risk changes.

---

## Phase 13: gated skill application

Only after skill proposals are working, add application paths.

Initial policy:

- auto-apply low-risk draft/non-authoritative playbook additions only if configured;
- require human approval for role prompt changes;
- require admin/operator approval for tool permission changes;
- require code review for code-backed tools;
- record all changes as Reflection events.

Deliverable:

- BEARS can adapt behavior under explicit policy and audit.

---

## Immediate memory build-out sequence

After the broader Reflection view is refined, kick off memory governance using this practical sequence:

1. **Add Den proposal queue.** Create DB model, API/service layer, and basic statuses so Den can create/list/read/resolve memory proposals.
2. **Add UI proposal creation.** From the memory browser, select memory files, create review proposals, and view pending proposals.
3. **Add `pair` review-request tool.** Expose `den.memory.request_review` / `memory_request_review` so `pair` can ask for curation.
4. **Add `curate` proposal tools.** Let `curate` list/read/resolve proposals and read source memory.
5. **Add constrained `core/` write tools.** Let `curate` read and safely update approved `core/` files with provenance.
6. **Add heartbeat invocation.** Den periodically asks `curate` to process bounded curation work.
7. **Add curation activity UI.** Show runs, outcomes, changed files, queue state, and failures.
8. **Add cleanup tools.** Let `curate` mark memory reviewed/superseded and eventually delete or compact low-risk sources.
9. **Add archive indexing.** Index selected curated memory into Letta Archives with source mapping.
10. **Add domains/projects/Cabinet integration.** Link curated memory to Bear Domains, projects/tasks/runs, and Cabinet Missions when appropriate.

---

## Open questions

- What exact activity thresholds classify a Bear as active, warm, or dormant?
- Should Reflection cadence be globally configured, per Bear, per tenant, or all three?
- Which lanes can run concurrently for one Bear?
- What is the first UI surface: activity stream, queue page, or Bear detail panel?
- Should `curate` perform skill review initially, or should there eventually be a distinct reviewer role?
- Which `core/` files are safe for autonomous append vs patch vs rewrite?
- What cleanup actions require human approval by default?
- How should Reflection runs surface model/tool cost?
- How should manual run requests interact with cooldowns and locks?
