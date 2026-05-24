# Reflection Run Taxonomy

Reflection runs are bounded background executions that help Bears learn, maintain memory, review work, and surface changes without blocking user turns.

## Summary

- **Reflection** is the overall background review, learning, and maintenance system.
- The **conductor** is Den infrastructure that advances Reflection runs.
- A **Reflection lane** is a type of background work.
- A **Reflection run** is one bounded execution in a lane.
- Role agents such as `curate` make semantic decisions during runs; the conductor coordinates and records them.

## Top-level model

```text
Reflection
├── conductor
│   ├── observes triggers
│   ├── selects lanes/work
│   ├── opens or reuses role conversations
│   ├── enforces locks and budgets
│   └── records outcomes
│
├── lanes
│   ├── pair_reflect
│   ├── memory_curate
│   ├── archive_index
│   ├── watch_observation_review
│   ├── work_result_review
│   ├── skill_review
│   ├── skill_apply
│   ├── introspection
│   ├── health_check
│   ├── cleanup
│   └── human_review_escalation
│
└── runs
    └── one bounded execution of a lane
```

## Run lanes

| Lane | Run name | Primary owner | Purpose |
|------|----------|---------------|---------|
| `pair_reflect` | Pair reflection run | Den / pair reflection process | Maintain `pair/` memory and create review requests. |
| `memory_curate` | Curate memory run | `curate` | Review role-local memory and maintain `core/`. |
| `archive_index` | Archive indexing run | Den / indexer | Sync selected canonical sources into Letta Archives. |
| `watch_observation_review` | Watch observation review run | `curate` | Review `watch` observations before memory/action. |
| `work_result_review` | Work result review run | `curate` | Review `work` results and promote useful summaries. |
| `skill_review` | Skill review run | `curate` or reviewer | Review proposed reusable skills/procedures. |
| `skill_apply` | Skill apply run | Den + policy | Apply approved skill changes. |
| `introspection` | Introspection run | `curate` or reviewer | Review behavior, tool failures, cost, and patterns. |
| `health_check` | Health check run | Den | Check role agents, services, queues, and drift. |
| `cleanup` | Cleanup run | Den / `curate` | Mark stale/superseded items or clean policy-allowed data. |
| `human_review_escalation` | Human review escalation run | Den | Surface unresolved or sensitive items to humans. |

## `pair_reflect`

Purpose:

- summarize substantial ACP pair sessions;
- extract durable technical decisions;
- identify repeated failure modes;
- improve `pair/` role-local memory;
- create `memory_request_review` proposals when useful.

Inputs:

- ACP pair conversation activity;
- local tool activity summaries;
- pair memory written during the session;
- authenticated human identity;
- relevant `core/` orientation.

Outputs:

```text
pair/summaries/
pair/reflections/
pair/decisions/
pair/notes/
memory review requests
```

Constraints:

- does not write `core/`;
- does not write Cabinet;
- does not create approved work tasks;
- does not read other role branches;
- is not a sixth Bear role.

## `memory_curate`

Purpose:

- review role-local memory;
- process memory proposals;
- keep `core/` compact and useful;
- decide what becomes shared memory;
- prepare knowledge for archives, Cabinet, or work task context.

Inputs:

- memory review proposals;
- role-local memory references;
- pair reflection summaries;
- recent memory activity;
- relevant `core/` files;
- policy/sensitivity context.

Outputs:

- proposal resolutions;
- `core/` updates;
- `core/` compaction;
- archive indexing requests;
- Cabinet proposals;
- work task context attachments;
- deferred/rejected/human-review-needed proposals.

Constraints:

- no external communication tools;
- no arbitrary path writes;
- do not copy raw logs into `core/`;
- record provenance for shared outcomes.

## `archive_index`

Purpose:

- maintain Letta Archives as derived semantic indexes over canonical BEARS sources.

Inputs:

- selected `core/` paths;
- approved proposal outcomes;
- Cabinet references;
- artifact summaries;
- source hashes and versions;
- source-to-passage index records.

Outputs:

- Letta Archive passages;
- updated source-to-passage mappings;
- deleted/recreated stale passages;
- archive indexing activity records.

Constraints:

- Letta Archives are not source of truth;
- no BEARS vector store;
- changed source hashes use delete-and-create passage sync;
- search results must point back to canonical sources.

## `watch_observation_review`

Purpose:

- review observations written by `watch` from inbound events.

Inputs:

- `watch/observations/`;
- event metadata;
- subscription context;
- relevant `core/`;
- sensitivity and trust policy.

Outputs:

- no-op/ignored observations;
- promoted summaries;
- task intents when action is warranted;
- Cabinet proposals;
- archive indexing requests.

Constraint:

- watch observations are not shared truth until reviewed.

## `work_result_review`

Purpose:

- review completed `work` outputs and decide what should be remembered or surfaced.

Inputs:

- `work/results/`;
- task definition;
- run status;
- artifacts;
- logs and summaries;
- relevant `core/`.

Outputs:

- `core/results/` summaries;
- memory updates;
- Cabinet proposals;
- follow-up task proposals;
- archive indexing requests.

Constraints:

- artifacts remain in Garage;
- memory stores summaries/pointers;
- work output is not automatically shared memory.

## `skill_review`

Purpose:

- review proposed reusable skills/procedures.

Inputs:

- skill proposals;
- evidence from role memory and task results;
- repeated failures or successes;
- existing skill manifest.

Outputs:

- approved skill updates;
- rejected skill proposals;
- revision requests;
- role applicability hints;
- audit records.

Constraint:

- behavior changes are higher risk than memory notes and may require human approval.

## `skill_apply`

Purpose:

- apply approved skill changes to manifests/materialized runtime.

Inputs:

- approved skill proposal;
- skill content;
- target roles;
- manifest state;
- policy and approval state.

Outputs:

- updated `bear_skills_manifest`;
- materialized skill files;
- role sync/reprovision work;
- audit trail.

Constraint:

- high-risk lane; should be policy-gated.

## `introspection`

Purpose:

- review Bear and role behavior.

Inputs:

- failed tool calls;
- user corrections;
- repeated plan failures;
- latency/cost events;
- memory review decisions;
- work result patterns.

Outputs:

- memory proposals;
- skill proposals;
- policy recommendations;
- human-review escalations.

Constraint:

- introspection proposes changes; it should not directly alter high-risk behavior.

## `health_check`

Purpose:

- check operational health.

Inputs:

- role agent status;
- MemFS view status;
- Letta/Codepool connectivity;
- archive/index health;
- queue depth;
- failed runs.

Outputs:

- health events;
- operator UI status;
- alerts;
- maintenance proposals.

Constraint:

- mostly deterministic and low semantic risk.

## `cleanup`

Purpose:

- remove or mark stale/superseded data within policy.

Inputs:

- stale proposals;
- superseded memory;
- old artifacts;
- old Reflection runs;
- policy TTLs.

Outputs:

- archived proposals;
- stale/superseded markers;
- deleted ephemeral artifacts;
- cleanup audit trail.

Constraint:

- destructive cleanup must be policy-gated and visible.

## `human_review_escalation`

Purpose:

- surface sensitive or unresolved items to humans.

Inputs:

- proposals marked `needs_human_review`;
- sensitive person data;
- secret-risk detections;
- failed curate decisions;
- policy conflicts.

Outputs:

- UI review items;
- notifications;
- blocked proposal status;
- audit entries.

Constraint:

- does not resolve automatically.

## Run statuses

Common run statuses:

```text
queued
running
completed
failed
cancelled
skipped
needs_human_review
```

## Trigger types

Common triggers:

```text
manual
heartbeat
adaptive_heartbeat
pair_reflection
memory_proposal_created
watch_observation_created
work_result_completed
skill_proposal_created
archive_drift_detected
health_check_due
cleanup_due
```

## Curate conversation policy

For `curate` runs, use one conversation per Bear + lane + UTC day:

```text
memory_curate:2026-05-09
watch_observation_review:2026-05-09
work_result_review:2026-05-09
```

Deterministic Den-only runs such as `archive_index` and `health_check` do not need Letta conversations unless an agent is invoked.

## Immediate priority path

```text
pair_reflect
→ memory_curate
→ archive_index
→ work task context bridge
```

This path turns `pair` learning into curated knowledge that `work` can safely consume without reading raw `pair/`.
