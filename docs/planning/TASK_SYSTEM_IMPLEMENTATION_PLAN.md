# Task System Implementation Plan

This plan turns the multi-agent task architecture into implementable Den work. It complements [`tasks-schema.md`](../architecture/tasks-schema.md), which remains the canonical file format for MemFS task intents, approved tasks, and work results.

This document now also tracks the task/activity portion of the single ontology-aware workflow-state model defined in [`../architecture/adr/workflow-state-ontology.md`](../architecture/adr/workflow-state-ontology.md). Workplan mode, live activity state, semantic memory, and execution capabilities must be represented as distinct domains rather than left as prompt-only conventions.

## Decision Summary

- Den owns a per-bear live activity board for current plans and status, aligned with Letta Code's lightweight `TodoWrite` / `UpdatePlan` progress layer.
- BEARS should support ACP `pair` Ask/Plan/Write modes aligned with common coding-agent clients: Ask and Plan expose read/search/inspect tools; Write enables mutation/execution/browser tools, with concrete effects still requiring Den policy, adapter safety checks, and ACP client approval.
- MemFS owns durable task artifacts: channel intent files, curate-approved task files, and work result files. Workplan/plan-mode artifacts are a separate ontology domain and must not be treated, described, or surfaced as MemFS semantic-memory documents, even if some current implementation details still use `pair/plans/` storage.
- ACP session rows remain protocol bindings only. They may reference an activity plan, but they do not own planning state.
- Channel agents never hand work directly to the work agent. They write or request task intents; curate approval is the promotion boundary.
- Letta Code-based agents interact with activity state through Den meta tools and short injected context, not by reading Den database rows directly.

## Data Model Foundation

The initial migration adds `bear_work_plans` and `bear_work_plan_events`.

`bear_work_plans` is the live, queryable activity board:

- `bear_id`: the logical Bear that owns the plan.
- `owner_role`: the role that owns the plan, usually `pair`, `talk`, or `work`.
- `owner_agent_id`: optional Letta role-agent id for audit and role lookup.
- `created_by_user_id`: nullable because Den workers or future system cycles may create plans.
- `source_conversation_id` and `source_acp_session_id`: bindings back to the surface that created the plan.
- `visibility`: controls what other agents may see.
- `status`: coarse plan lifecycle.
- `items`: JSON array of structured plan items. Application validation enforces at most one `in_progress` item.
- `handoff_intent_path` and `handoff_task_id`: links from live planning to the durable task pipeline when handoff occurs.

`bear_work_plan_events` is an append-only audit stream for status updates and future UI timelines. It is not the source of truth for current status.

## Tool Surface

Add Den meta tools before implementing runtime behavior.

These tools implement the live activity/progress layer, not the full pre-implementation planning gate:

- `den.work_plan.list`: list visible work plans for the current bear.
- `den.work_plan.get_status`: read one visible plan or the current session's plan projection.
- `den.work_plan.update`: create or update the current role's live plan.
- `den.work_plan.request_handoff`: request conversion of plan items into a task intent.

Tool policy:

- `talk`, `pair`, `curate`, and `work` may list/read visible plans.
- `talk`, `pair`, and `work` may update their own plans.
- `talk` and `pair` may request handoff into the task-intent pipeline.
- `work` must not execute directly from a channel plan. It executes only Den-dispatched approved tasks from `core/tasks/`.
- `watch` is intentionally excluded from activity tools until a concrete observation-status use case exists.

## Visibility Rules

Use these visibility values initially:

- `private_to_role`: only the owning role and Den admin/operator surfaces may read it.
- `same_user`: the originating user and Den admin/operator surfaces may read it across roles.
- `bear_visible`: any role in the Bear may read a redacted projection.
- `handoff_requested`: readable by curate and Den task tooling while handoff is pending.

The read API should return projections, not raw rows. For example, `talk` asking about `pair` work should receive title, status, current item, blockers, and timestamps, but not raw ACP workspace paths or unredacted local context unless policy explicitly allows it.

## Pair Plan Mode

ACP `pair` has `Ask`, `Plan`, and `Write` modes, aligned with common coding-agent clients. Modes are workflow/UI state, not a separate durable mutation gate.

Implementation status: schema, core model, Den tools, ACP prompt reminders, persisted `pair/plans/` workplan artifacts, native ACP `plan` updates, and native ACP `Ask` / `Plan` / `Write` mode/config updates are implemented. `record_plan_approval` records explicit authenticated-human approval when useful for workplan/audit. The current `pair/plans/` persistence is implementation detail, not a semantic-memory contract.

Acceptance:

- Pair can request entering plan mode and Den records active plan-mode state for the ACP session.
- Ask and Plan expose read/search/inspect tools; Write enables mutation/execution/browser tools. Concrete effects remain subject to Den policy, adapter safety checks, and ACP client approval.
- Pair can persist a markdown workplan artifact under `pair/plans/`, but that artifact is a workplan-domain record rather than semantic memory.
- Exiting plan mode stores or updates the markdown workplan artifact and marks it `submitted`.
- The authenticated human can approve a submitted plan in chat through `record_plan_approval`, which switches ACP UI to `Write` and records an audit event linking the plan artifact, ACP session, Bear, role, and user.
- Rejection keeps the session in Plan mode for revision unless the user cancels plan mode.

This is intentionally separate from `den.work_plan.update`. The activity board is the visible current status list; the workplan artifact is the reviewable proposal before mutation. Even when persisted under `pair/plans/`, it must not be described or treated as a MemFS semantic-memory document.

## Implementation Phases

### Phase 1: Schema and Models

Acceptance:

- Migration creates `bear_work_plans` and `bear_work_plan_events`.
- Rust model structs compile and serialize/deserialize plan items.
- Validation rejects more than one `in_progress` item.
- Den tool descriptors expose the planned activity tools with role-appropriate availability.

### Phase 2: Workboard CRUD

Acceptance:

- `create_or_update_work_plan` upserts a plan scoped to `(bear_id, owner_role, source_conversation_id, source_acp_session_id)` where available.
- Updates increment `version` and append `bear_work_plan_events` rows.
- Read paths enforce membership and visibility projection.
- `den.work_plan.update`, `den.work_plan.get_status`, and `den.work_plan.list` return useful JSON to agents.

### Phase 3: Prompt and Context Integration

Acceptance:

- ACP pair prompts receive a short current-plan summary when a plan exists for the ACP session.
- ACP pair tool descriptors expose `den.work_plan.update`, `den.work_plan.get_status`, `den.work_plan.list`, and `den.work_plan.request_handoff` alongside memory/search/situation tools.
- Letta Code `talk` and `work` contexts receive a compact activity summary through existing Den/Codepool trusted context paths.
- System prompts describe when to call `den.work_plan.update`.
- The agent-facing wording treats this as normal planning/status, not database manipulation.

### Phase 4: Handoff to Durable Tasks

Acceptance:

- `den.work_plan.request_handoff` validates selected plan items and writes a task intent through the same code path as `den.task.write_intent`.
- Handoff updates `handoff_intent_path`, sets visibility to `handoff_requested`, and appends an audit event.
- Curate review remains the only path from channel-originated intent to `core/tasks/`.
- Work dispatch only reads approved `core/tasks/` definitions.

### Phase 5: Task Runtime Completion

Acceptance:

- Den indexes approved `core/tasks/` files and stores scheduling/runtime state.
- Den dispatches one approved task at a time to the `work` role harness.
- Work runs receive only curated task definitions, allowed tools, scope, limits, and run id.
- `den.run.write_result` validates and writes `work/results/<task-id>/<run-id>.md`.
- Curate can promote summaries to `core/results/`.

### Phase 6: Operator and Chat UX

Acceptance:

- Operator UI shows active work plans, pending handoffs, approved tasks, runs, and failures per Bear.
- Talk can answer “what is pair working on?” from `bear_visible` or `same_user` projections.
- Pair can resume an ACP session and recover its live plan.
- High-risk task runs surface in a human approval queue before work executes.

## Workflow-state unification requirements

Treat this work as part of the near-term workflow-state ontology effort, not as a later cleanup.

Required additions:

- Add an authoritative per-turn workflow-state summary to ACP/Den surfaces that explicitly names:
  - permission mode;
  - available tool classes;
  - workplan state (`inactive`, `drafting`, `submitted_waiting_approval`, `approved`, `cancelled`);
  - workplan approval status;
  - whether execution is unlocked;
  - active activity plan id/summary when present.
- Ensure current-turn capability state is explicitly marked as authoritative over prior-turn assumptions.
- Make plan-mode tool outputs look workplan-native rather than like ordinary memory/file-write results.
- Keep workplan artifacts distinct from semantic memory in namespaces, labels, and response payloads.
- Do not describe `pair/plans/` persistence as MemFS semantic memory; it is a workplan-domain storage detail until a more workflow-native namespace lands.
- Expose a machine-readable ontology/domain field in relevant descriptors and UI surfaces so workplan, activity, memory, and execution remain separable even when provider-safe names stay concise.
- Align remaining Den API, operator, and audit surfaces to the same ontology rather than limiting the model to ACP reminders.

## Implementation Notes

- Keep the first DB implementation JSONB-backed. Normalize plan items only if querying individual items becomes painful.
- Do not store secrets, raw credentials, or long local file excerpts in work plans.
- Use optimistic concurrency with `version` on updates to prevent silent status clobbering.
- Keep work plans short. Durable details belong in MemFS task files, result files, or conversation history.
- If a plan item implies external effects, prefer `request_handoff` over continuing as live plan state.
- Planning state is not shared Bear memory by default. Use the activity board for tactical progress, a workplan artifact for approval, role-local memory for durable lessons, and curate review for anything that should enter `core/`.

## Open Questions

- Whether `same_user` should be based only on Den user id or also channel external identity mappings.
- Whether ACP local workspace paths should be redacted by default even for the same user in non-ACP surfaces.
- Whether completed activity plans should archive automatically after a fixed age.
- Whether operator edits to activity plans should be allowed or recorded only as administrative events.
- Exact workplan-artifact namespace and representation: separate Den-controlled artifact namespace, workplan-native ids without file-like surfacing, or a hybrid model.
