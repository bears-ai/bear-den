# Reflection System — Architecture Decision Record

## Status: Accepted

## Date: 2026-05-09

---

## Context

BEARS is adding durable memory governance and future learning loops around Bear operation. The immediate memory plan gives Den and the `curate` role the ability to review role-local memories, maintain `core/`, and coordinate archive indexing. That work sits inside a broader need: Bears should be able to periodically review what happened, extract durable lessons, maintain their own state, and propose bounded improvements.

The design needs a name and architecture that are understandable, inspectable, and safe. Candidate names included "subconscious", "introspection", "reflection", and "adaptation".

The system must cover more than memory. Likely background work includes:

- memory curation;
- archive indexing;
- role and tool-use introspection;
- skill review and skill-change proposals;
- health checks;
- cleanup;
- human-review escalation.

These loops have different risk levels. Memory curation can often run autonomously within constraints. Skill modification is higher risk because it may change how a Bear behaves. Archive indexing is mostly derived-state maintenance. Health checks should be frequent and low-risk.

The system also needs variable cadence. An active Bear should be able to receive a more rapid heartbeat than a dormant Bear, while preserving budgets, locks, and auditability.

---

## Decision

BEARS will call the umbrella autonomous background review system **Reflection**.

Reflection is the system by which Den and Bear roles periodically or eventfully review recent activity, maintain durable state, and propose or apply bounded improvements. Reflection is not hidden or mystical; it is an auditable, policy-governed maintenance and learning process.

Use the following vocabulary:

| Term | Meaning |
|---|---|
| Reflection | Umbrella system for background review, learning, maintenance, and improvement. |
| Reflection run | One bounded execution of Reflection for a Bear, role, lane, or scope. |
| Reflection lane | A specific kind of background work, such as memory curation or skill review. |
| Memory curation | Lane that reviews role-local memory, updates `core/`, and maintains memory hygiene. |
| Archive indexing | Lane that syncs selected canonical sources into derived Letta Archive passages. |
| Introspection | Lane that reviews role behavior, failures, tool use, and operational patterns. |
| Adaptation | Lane that proposes or applies skill, workflow, prompt, or behavior changes. |
| Reflection proposal | A proposed durable change discovered through Reflection. |
| Memory proposal | A proposal to change Bear memory, `core/`, Cabinet links, or memory lifecycle. |
| Skill proposal | A proposal to change behavior, skills, workflows, prompts, or role instructions. |

`curate` remains a Bear role. Reflection is the process/system. `curate` performs memory curation and may participate in skill review, but Reflection can also include Den-owned lanes such as archive indexing and health checks.

---

## Naming rationale

### Chosen: Reflection

Reflection is the best umbrella term because it communicates:

- looking back over experience;
- extracting lessons;
- improving future operation;
- maintaining memory;
- reviewing recent activity;
- bounded learning without implying unrestricted self-modification.

### Not chosen: Subconscious

"Subconscious" is evocative but unsuitable as the official system name. It implies opaque or hidden behavior. BEARS needs user trust, visibility, and auditability, especially when background processes can mutate memory or propose behavior changes.

### Scoped term: Introspection

Introspection is useful for self/behavior analysis, but too narrow for the whole system. Use it for lanes that inspect role behavior, tool-use failures, and performance.

### Scoped term: Adaptation

Adaptation implies behavior change. Use it for skill, workflow, prompt, and policy modifications. Do not use it for the full system because not every Reflection run changes behavior.

---

## Reflection lanes

Reflection uses one orchestration structure with separate lanes. Lanes share scheduling, budgets, locking, run records, and audit logs, but they do not share the same permissions or risk policy.

Initial lane model:

| Lane | Purpose | Typical owner | Risk | Cadence |
|---|---|---|---|---|
| `memory_curate` | Review role memory, maintain `core/`, compact/prune memory. | `curate` + Den | Medium | Frequent when active |
| `archive_index` | Sync selected canonical memory/Cabinet sources to Letta Archives. | Den/indexer | Low/medium | After curation or periodic |
| `introspection` | Review role behavior, tool-use issues, failures, and patterns. | `curate` or future reviewer | Medium | Periodic or event-triggered |
| `skill_review` | Draft skill/workflow/prompt proposals from evidence. | `curate` or future reviewer | Medium/high | Less frequent |
| `skill_apply` | Apply approved skill/workflow/prompt changes. | Den + human-gated policy | High | Manual or tightly gated |
| `health_check` | Check agents, services, tools, stale queues, and drift. | Den | Low | Frequent |
| `cleanup` | Remove or mark stale/superseded artifacts within policy. | Den + `curate` | Medium | Periodic |
| `human_review_escalation` | Surface risky or unresolved items to humans/admin UI. | Den | Low | Frequent |

---

## Heartbeats and throttling

A heartbeat is one trigger source for Reflection runs. Heartbeats are not the whole model; Reflection can also be triggered by events.

Trigger kinds include:

- periodic heartbeat;
- manual request;
- memory write;
- task completed;
- proposal created;
- schedule;
- service restart;
- archive drift detected;
- failed tool use;
- human feedback.

Heartbeat cadence is throttled by Bear activity and policy. An active Bear may receive a rapid heartbeat so it can curate memory and process proposals close to the work that produced them. A dormant Bear may receive only occasional maintenance heartbeats.

The scheduler should consider:

- recent conversations or pair sessions;
- recent memory writes;
- task/run activity;
- pending proposals;
- unresolved human-review items;
- stale queues;
- last Reflection run per lane;
- configured minimum and maximum cadence;
- compute/cost budgets;
- cooldowns after failures.

Even for active Bears, each run must have bounded budgets such as maximum proposals reviewed, files read, tool calls, wall-clock time, core files modified, archive operations, and destructive actions.

---

## Risk boundaries

Reflection actions are classified by risk.

### Low risk

Usually safe to run automatically:

- create a proposal;
- add an observation;
- add a summary;
- detect repeated patterns;
- mark derived archive entries for reindexing;
- run health checks;
- surface human-review requests.

### Medium risk

Can run automatically with constraints and audit:

- append to selected `core/` files;
- compact `core/current-focus.md`;
- mark local memory reviewed or superseded;
- create draft skill files or playbooks;
- update non-authoritative summaries;
- clean up stale proposals.

### High risk

Should initially require human approval or explicit policy gates:

- modify role system prompts;
- change Den tool permissions;
- alter autonomous behavior policy;
- delete large amounts of memory;
- overwrite existing skills;
- change task execution strategy globally;
- modify code-backed tools;
- modify deployment/runtime configuration.

The invariant is that Reflection may learn and recommend freely, but behavior-changing adaptation is governed.

---

## Memory curation vs skill adaptation

Memory curation answers: what should the Bear remember?

Skill adaptation answers: how should the Bear behave differently?

These must stay separate. Memory curation may notice repeated patterns, failures, or reusable procedures, but it should generally create skill proposals rather than directly modifying durable skills or prompts.

Recommended skill flow:

1. Observe evidence through memory, task results, failures, human feedback, or introspection.
2. Create a skill proposal with references and rationale.
3. Review the proposal under `skill_review`.
4. Apply only if policy permits, and initially require human approval for high-risk changes.

---

## Data model implications

Use shared run/event tables plus lane-specific proposal tables.

Implemented MVP shared tables:

| Table | Purpose |
|---|---|
| `bear_reflection_runs` | One row per bounded queued/running/completed Reflection run. |
| `bear_reflection_run_items` | Optional normalized item links for a run; first queue path stores proposal IDs in `input_summary`. |
| `reflection_conversations` | Daily Bear + lane conversation mapping for role-backed Reflection runs. |

Future shared tables:

| Table | Purpose |
|---|---|
| `bear_reflection_events` | Audit log events emitted by Reflection runs. |
| `bear_reflection_locks` | Optional coordination locks by Bear/lane/scope, or equivalent advisory locks. |

Suggested lane-specific tables:

| Table | Purpose |
|---|---|
| `bear_memory_proposals` | Memory/core/Cabinet review queue. |
| `bear_skill_proposals` | Skill/workflow/prompt change queue. |
| `bear_archive_index_entries` | Source-to-Letta-passage mapping for derived archive indexes. |

Avoid forcing every lane into one generic proposal table too early. Memory proposals and skill proposals have different targets, statuses, tools, risks, and approval rules.

The implemented MVP `bear_reflection_runs` records include:

- Bear ID;
- lane;
- trigger;
- status;
- role agent ID;
- conversation ID/key/date;
- input summary;
- output summary;
- error;
- started/completed timestamps;
- created timestamp.

Later richer run records or event tables should add:

- budgets requested and consumed;
- affected source references beyond `input_summary`;
- emitted events;
- richer error summaries;
- explicit human-review flags where status alone is insufficient.

---

## Tool and permission implications

Den remains the control plane for Reflection authorization and scheduling.

Reflection tools should be role-scoped and lane-scoped. For example:

- non-curate roles may request memory review;
- `curate` may list/read/resolve memory proposals;
- `curate` may write constrained `core/` updates;
- skill review may draft proposals;
- skill application requires stronger policy gates;
- Den/indexer owns shared archive writes;
- MemFS Manager performs canonical Git-backed file operations under path policy.

Do not give a lane broad arbitrary filesystem mutation if a structured operation is enough.

---

## UI implications

The UI should make Reflection visible and controllable.

Useful views:

- Reflection activity stream;
- last run per Bear/lane;
- pending memory proposals;
- pending skill proposals;
- curation outcomes;
- archive indexing state;
- health check status;
- human-review queue;
- manual "run reflection now" action;
- per-Bear heartbeat/cadence policy.

User-facing language should say things like:

- "Reflection updated Bear memory.";
- "Reflection proposed a skill change.";
- "Reflection needs review.";
- "Last reflection run: memory curation.";
- "This Bear is active, so reflection runs more frequently.".

Avoid language suggesting hidden uncontrolled activity, such as "the subconscious changed your skills".

---

## Consequences

Positive consequences:

- BEARS has a single vocabulary for background review and learning.
- Memory curation, archive indexing, skill review, and health checks can share scheduler infrastructure.
- High-risk behavior changes remain separated from lower-risk memory maintenance.
- Users and operators can audit what Reflection did and why.
- Heartbeat throttling supports both active and dormant Bears.

Tradeoffs:

- More concepts than a single "curation heartbeat".
- Requires run/event tracking before the full system feels coherent.
- Skill adaptation needs explicit approval policy to avoid unsafe self-modification.

---

## Related docs

- [Semantic Bear Memory ADR](semantic-bear-memory.md)
- [Dynamic skills, reflection subagents, and bear configuration ADR](dynamic-skills-subagents.md)
- [Routines automation ADR](routines-automation.md)
- [Reflection system concept](../../concepts/REFLECTION_SYSTEM.md)
- [Reflection system implementation plan](../../planning/REFLECTION_SYSTEM_PLAN.md)
- [Curate memory governance plan](../../planning/CURATE_MEMORY_GOVERNANCE_PLAN.md)
