# Reflection System

Reflection is BEARS' auditable background review and learning system. It lets a Bear periodically or eventfully review recent activity, curate memory, maintain derived indexes, inspect behavior, and propose bounded improvements.

## Summary

- **Reflection** is the umbrella system for background review, learning, maintenance, and improvement.
- A **Reflection run** is one bounded execution for a Bear, lane, role, or scope.
- A **Reflection lane** is a specific kind of work such as memory curation, archive indexing, introspection, or skill review.
- A **cycle runner** is Den's bounded orchestration loop for invoking a role such as `curate` on one Reflection lane.
- `curate` is a Bear role; Reflection is the broader process.
- Memory curation and skill adaptation are related but separate.
- Heartbeats are throttled: active Bears can reflect more frequently than dormant Bears.
- Reflection must be visible, auditable, budgeted, and policy-governed.

## Why "Reflection"

Use **Reflection** as the product and architecture term.

Reflection suggests:

- looking back over experience;
- extracting lessons;
- maintaining memory;
- improving future behavior;
- bounded review rather than hidden self-modification.

Avoid **subconscious** as an official term. It is evocative but implies hidden or opaque behavior, which conflicts with BEARS' goals of trust, auditability, and user/operator control.

Use narrower terms underneath Reflection:

| Term | Use |
|---|---|
| Curation | Memory governance and `core/` maintenance. |
| Introspection | Reviewing role behavior, tool use, failures, and performance. |
| Adaptation | Proposing or applying behavior, skill, prompt, or workflow changes. |
| Maintenance | Internal scheduler/run infrastructure where a dry operational term is useful. |

## Core vocabulary

| Term | Meaning |
|---|---|
| Reflection | Umbrella system for autonomous background review and improvement. |
| Reflection run | One bounded execution of Reflection. |
| Reflection lane | A specific kind of background work. |
| Reflection event | Auditable event emitted during a run. |
| Cycle runner | Den-side orchestrator that selects work, opens/reuses the right role conversation, invokes the role, enforces budgets/locks, and records activity. |
| Curate cycle | A Reflection run in which the cycle runner invokes `curate` for a bounded lane such as memory review. |
| Reflection proposal | A proposed durable change discovered through Reflection. |
| Memory proposal | Proposal to change Bear memory, `core/`, Cabinet links, or memory lifecycle. |
| Skill proposal | Proposal to change behavior, skills, workflows, prompts, or role instructions. |
| Heartbeat | Periodic trigger source for Reflection runs. |
| Dynamic heartbeat | Future trigger evaluator that runs Reflection based on activity pressure rather than a fixed cron alone. |

## Reflection lanes

Reflection should use a shared orchestration structure, but separate lanes. Lanes share scheduling, locking, budgets, run records, and audit logs. They do not share all tools, permissions, cadences, or approval policy.

| Lane | Purpose | Typical owner | Risk |
|---|---|---|---|
| `memory_curate` | Review role-local memory, maintain `core/`, compact/prune memory. | `curate` + Den | Medium |
| `archive_index` | Sync selected canonical sources into derived Letta Archives. | Den/indexer | Low/medium |
| `introspection` | Review role behavior, failures, tool use, and recurring patterns. | `curate` or future reviewer | Medium |
| `skill_review` | Draft skill/workflow/prompt proposals from evidence. | `curate` or future reviewer | Medium/high |
| `skill_apply` | Apply approved behavior changes. | Den + policy/human gate | High |
| `health_check` | Check agents, tools, queues, services, and drift. | Den | Low |
| `cleanup` | Remove or mark stale/superseded artifacts within policy. | Den + `curate` | Medium |
| `human_review_escalation` | Surface risky or unresolved items. | Den | Low |

## Cycle runner

The cycle runner is Den's orchestration loop for Reflection. It turns pending background work into bounded role runs.

For a `memory_curate` cycle, the runner should:

1. select pending memory proposals or recent memory activity;
2. acquire a Bear/lane lock so cycles do not collide;
3. resolve the role agent, usually `curate`;
4. open or reuse the correct Letta conversation;
5. build a bounded prompt with proposal IDs, source summaries, policy, and tool instructions;
6. invoke the role with only lane-appropriate tools;
7. record cycle status, tool activity, decisions, errors, and outputs;
8. surface the run in UI.

The runner is infrastructure. Semantic decisions belong to the role agent, usually `curate`, not to ad hoc Den heuristics.

### Conversation rollover

For `curate`, use one conversation per Bear + lane + UTC day in the first implementation:

```text
conversation_key = memory_review:YYYY-MM-DD
```

This preserves same-day continuity while preventing one unbounded lifelong curation conversation. Later rollover can also occur when the context window is near full or operator policy requests a reset.

### Pair reflection trigger

The P0 trigger is:

```text
pair reflection summary created
→ memory review request created
→ curate memory-review cycle queued or run
```

This is higher priority than a generic heartbeat because it directly connects pair learning to the cross-role sharing path.

## Heartbeats and activity throttling

A heartbeat is a periodic trigger for Reflection. Heartbeats are throttled by activity and policy.

An active Bear may have a rapid heartbeat, allowing Reflection to process memory and proposals close to the work that produced them. A dormant Bear may receive only occasional maintenance heartbeats.

Activity signals may include:

- recent conversations;
- recent `pair` sessions;
- memory writes;
- task/run activity;
- watch observations;
- pending proposals;
- unresolved human-review items;
- stale queues;
- recent errors or failed tool calls.

Cadence policy should include minimum and maximum intervals. The scheduler should also apply cooldowns after failures and should not let a highly active Bear run unbounded background work.

## Triggers beyond heartbeat

Reflection can also be triggered by events:

- manual "run reflection now" action;
- proposal creation;
- memory write;
- task completion;
- archive drift detection;
- service restart;
- failed tool use;
- human feedback;
- explicit schedule.

Heartbeat is therefore a trigger type, not the whole system. The cycle runner is the mechanism that turns heartbeat or event triggers into actual Reflection runs.

## Budgets and bounds

Every Reflection run should have explicit budgets.

Examples:

- maximum wall-clock time;
- maximum tool calls;
- maximum proposals reviewed;
- maximum files read;
- maximum files written;
- maximum `core/` files modified;
- maximum archive passages updated;
- maximum cleanup/deletion actions;
- maximum spend/cost budget;
- maximum follow-up proposals created.

Reflection should do useful bounded work and then stop. Long-running autonomous review should be split into multiple runs.

## Memory curation

Memory curation asks: **what should the Bear remember?**

The `memory_curate` lane reviews role-local memory from `talk/`, `pair/`, `work/`, `watch/`, and `curate/`, then decides whether to:

- retain it locally;
- summarize it;
- promote distilled knowledge into `core/`;
- mark it reviewed;
- mark it superseded;
- delete it when policy allows;
- create a Cabinet update proposal;
- create a skill proposal when the memory describes reusable behavior.

`core/` must remain compact and useful. It should not become an append-only dump of role memory.

## Archive indexing

Archive indexing asks: **what should be semantically retrievable?**

Letta Archives are derived indexes over canonical sources. Reflection can queue or perform indexing after curation changes selected canonical content.

Canonical sources remain:

- Bear `core/`;
- role branches;
- Cabinet;
- Den DB workflow state;
- Garage artifacts.

Archive passages should include provenance metadata and should be refreshed with delete-and-create semantics when the source changes.

## Introspection

Introspection asks: **what happened in the Bear's behavior?**

It may review:

- failed tool calls;
- repeated corrections from humans;
- role-specific mistakes;
- slow or costly workflows;
- missing context;
- repeated successful procedures;
- confusing or stale instructions;
- conflicts between skills, memory, and policy.

Introspection usually creates observations or proposals. It should not directly change high-risk behavior.

## Adaptation and skill learning

Adaptation asks: **how should the Bear behave differently?**

Skills are not memory. Memory stores what the Bear knows; skills shape how the Bear performs repeatable activities.

Recommended adaptation flow:

1. Evidence appears in memory, task results, failures, human feedback, or introspection.
2. Reflection creates a skill proposal.
3. The `skill_review` lane reviews the proposal and drafts a change.
4. The `skill_apply` lane applies the change only if policy allows.

Initially, high-risk adaptation should require human approval. This includes changing role prompts, Den tool permissions, global execution strategy, code-backed tools, or deployment/runtime configuration.

## Risk levels

| Risk | Examples | Default policy |
|---|---|---|
| Low | Create proposal, detect pattern, run health check, request human review. | Automatic with audit. |
| Medium | Append to constrained `core/` files, mark memory reviewed, draft skill file, compact current focus. | Automatic with constraints, budgets, and audit. |
| High | Modify role prompts, change tool permissions, delete large memory sets, overwrite skills, modify code/deployment. | Human approval or explicit strong policy gate. |

## Relationship to roles

| Role/service | Reflection responsibility |
|---|---|
| Den | Scheduler, policy, run/event records, locks, tool authorization, archive indexer, UI/API. |
| MemFS Manager | Git-backed canonical file operations under path policy. |
| `curate` | Memory curation, review, consolidation, cleanup recommendations, skill proposal drafting. |
| `pair` | Writes role-local memory and may request review. |
| `talk` | Writes conversation-derived local memory and may request review. |
| `work` | Writes task/run memory and may request review. |
| `watch` | Writes observations/logs; does not decide shared truth. |

## UI expectations

The UI should show:

- last Reflection run per Bear/lane;
- activity stream;
- pending memory proposals;
- pending skill proposals;
- curation outcomes;
- archive indexing state;
- health check results;
- human-review queue;
- run budgets and failure summaries;
- per-Bear heartbeat/cadence policy;
- cycle runner queue/status;
- daily curate conversation used for each lane;
- manual "run reflection now" controls.

Product language examples:

- "Reflection updated Bear memory."
- "Reflection proposed a skill change."
- "Reflection needs review."
- "This Bear is active, so Reflection runs more frequently."

Avoid:

- "The subconscious changed your skills."
- "The Bear secretly learned this."
- "Reflection has unlimited autonomy."

## Related docs

- [Reflection System ADR](../architecture/adr/reflection-system.md)
- [Memory model](MEMORY_MODEL.md)
- [Capabilities and skills](CAPABILITIES_AND_SKILLS.md)
- [Bear agent roles](BEAR_AGENT_ROLES.md)
- [Tasks and autonomy](TASKS_AND_AUTONOMY.md)
- [Semantic Bear Memory ADR](../architecture/adr/semantic-bear-memory.md)
- [Reflection system implementation plan](../planning/REFLECTION_SYSTEM_PLAN.md)
- [Curate memory governance plan](../planning/CURATE_MEMORY_GOVERNANCE_PLAN.md)
- [Memory automation roadmap](../planning/MEMORY_AUTOMATION_ROADMAP.md)
