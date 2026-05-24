# Schema-first Den-generated path strategy — Architecture Decision Record

> Scope note: this ADR applies to **schema-owned durable artifacts** such as task intents, approved tasks, observations, run results, and handoffs. It does not require every Bear memory object to be schema-owned, Cabinet-backed, or promoted to `core/`. See [Semantic Bear Memory](semantic-bear-memory.md) for the broader memory model.

## Status: Accepted

## Date: 2026-05-05

---

## Context

BEARS uses Den as the control plane for product workflows and MemFS as the durable Markdown-backed memory/artifact substrate for agent-facing work. The system now has several kinds of state that look path-like but have different ownership, mutability, and trust properties.

This ADR focuses on schema-owned workflow artifacts. It is intentionally narrower than the semantic memory model, which also covers role-local notes, logs, decisions, reflections, scratch, summaries, Cabinet references, and lifecycle metadata.

Path-like state includes:

- live work planning state used for UI projection and fast status updates;
- durable task intent, approved task, observation, and run-result artifacts;
- role-local notes, logs, decisions, and raw memory written by bears within their role branch.

If agents choose schema-owned durable artifact paths directly, path selection becomes part of the agent trust boundary. That creates avoidable risks:

- path traversal or branch-boundary bypass;
- accidental writes into another role's namespace;
- title-derived slug leaks and collisions;
- inconsistent naming across Den tools;
- difficult audit trails when handoffs or approvals materialize artifacts.

Den already owns the domain schema for work plans, handoffs, task approval, runs, and observations. The path strategy for those artifacts should therefore remain conservative and schema-first: agents provide semantic inputs, and Den generates durable artifact paths based on validated schema context.

Role-local semantic memory entries can be flexible and metadata-driven, but schema-owned durable artifacts remain Den-path-owned.

---

## Decision

BEARS will separate path-like state into three categories and keep schema-owned durable artifact path generation inside Den schema tools.

| Category | Owner | Path examples | Rule |
|---|---|---|---|
| Live workboard | Den DB | `bear_work_plans`, `bear_work_plan_events` | No MemFS path yet. Fast mutable state, status, UI projection. |
| Durable intent/task/result artifacts | MemFS | `talk/tasks/...`, `pair/tasks/...`, `core/tasks/...`, `work/results/...` | Written only by Den schema tools. |
| Role-local semantic memory | Role branch | `talk/notes/...`, `pair/logs/...`, `pair/decisions/...`, `work/summaries/...` | Flexible role-local memory entries such as notes, logs, decisions, reflections, scratch, and summaries. Paths are conventional and role-restricted; semantics live in metadata. |

The important boundary is:

> Agents do not choose schema-owned durable artifact paths directly. Agents provide semantic inputs; Den chooses the path.

This applies to Den-hosted workflow tools that materialize durable task, handoff, observation, and result artifacts. The agent-facing input schema must not expose arbitrary durable artifact `path` fields for these tools.

For role-local semantic memory entries, agents should still provide semantic fields such as `kind`, `title`, `body`, references, and lifecycle hints rather than arbitrary paths when using Den-hosted memory tools. However, those entries are not schema-owned workflow artifacts and do not automatically imply promotion to `core/` or Cabinet.

---

## Path rules

### `den.work_plan.update`

- Does not write a MemFS artifact.
- Updates only Den DB workboard state.
- May store `handoff_intent_path` later, but only after handoff materialization.

### `den.work_plan.request_handoff`

- Agent passes semantic fields such as `plan_id`, selected `item_ids`, title, summary, and desired outcome.
- Den writes an intent artifact using a Den-generated path based on the caller role:
  - `talk/tasks/intent-<timestamp>-<shortid>.md`
  - `pair/tasks/intent-<timestamp>-<shortid>.md`
- Den records the generated path in `bear_work_plans.handoff_intent_path`.
- The implementation should use the same shared task-intent writer as `den.task.write_intent`.

### `den.task.write_intent`

- Uses the same Den-generated path strategy as handoff.
- Is role-constrained to `talk` and `pair`.
- Does not accept a `path` field from the agent.

### Curate approval

- Den writes the approved task artifact at:
  - `core/tasks/task-<timestamp>-<shortid>.md`
- Den updates source intent review metadata through control-plane logic.
- Curate does not perform raw writes into `talk/` or `pair/` paths.

### `den.run.write_result`

- Den writes the run result artifact at:
  - `work/results/<task-id>/<run-id>.md`
- `task_id` and `run_id` should come from Den run context where possible, not arbitrary agent-provided strings.

### `den.observation.write`

- Den writes the observation artifact at:
  - `watch/observations/observation-<timestamp>-<shortid>.md`
- Requires Den-issued event or subscription context.

### `den.memory.write_entry`

- Writes flexible role-local semantic memory entries such as `note`, `log`, `decision`, `reflection`, `scratch`, or `summary`.
- The agent provides semantic fields, references, lifecycle hints, and provenance-relevant source data.
- Den or the memory tool chooses a conventional role-local path, for example:
  - `pair/notes/<timestamp>-<shortid>.md`
  - `pair/logs/<timestamp>-<shortid>.md`
  - `pair/decisions/<timestamp>-<shortid>.md`
- These paths are conventional, not schema-owned workflow artifact identities.
- A role-local entry can be final with `promotion: none`; it does not require Cabinet mapping or `core/` promotion.

---

## Naming

Schema-owned durable artifact paths use a timestamp plus short random or id suffix. They do not use user-provided or agent-provided titles.

Role-local semantic memory entries should also avoid depending on title slugs for identity. Human-friendly slugs may be acceptable as display-oriented suffixes when collision-safe IDs remain present and when the slug does not leak sensitive content.

Reference examples:

```text
talk/tasks/intent-2026-05-05T184233Z-a1b2c3.md
pair/tasks/intent-2026-05-05T184233Z-d4e5f6.md
core/tasks/task-2026-05-05T184233Z-9a8b7c.md
watch/observations/observation-2026-05-05T184233Z-11aa22.md
work/results/task-2026-05-05T184233Z-9a8b7c/run-2026-05-05T190015Z-33bb44.md
```

Title-derived slugs are intentionally avoided for schema-owned durable artifacts because they:

- leak task content into path names;
- collide more easily;
- require subtle sanitization rules;
- create unnecessary rename pressure when titles change.

---

## Rationale

This strategy keeps each storage layer aligned with its responsibility:

- Live planning remains mutable and fast without polluting durable memory.
- Durable task artifacts remain stable, auditable, and schema-shaped.
- Role-local semantic memory remains flexible without weakening schema-owned artifact boundaries.
- Den can enforce path restrictions before writing to MemFS.
- Handoff state becomes explicitly visible: the DB work plan records `handoff_intent_path`, and curate approval records `handoff_task_id`.
- The trust model remains clear: `talk` and `pair` request intent, `curate` promotes, and `work` reports results.

---

## Consequences

### Positive

- Schema-owned durable artifact paths become deterministic by tool and role, not by agent choice.
- Path traversal and role-branch boundary bypass risks are reduced.
- UI and audit trails can link workboard state to materialized artifacts by explicit Den-owned columns.
- Naming is stable even when user-facing titles change.
- The same path generator can be reused across handoff, task intent, approval, observation, and run-result tooling.

### Negative / Trade-offs

- Agents have less direct control over schema-owned artifact organization.
- Den must own and test a small path-generation utility.
- Existing or future tools that accept arbitrary paths need schema review before being used for schema-owned durable artifacts.

### Mitigations

- Keep role-local semantic entries available under role branches for notes, logs, decisions, reflections, scratch, summaries, and exploratory memory.
- Make generated paths visible in tool responses and Den UI after materialization.
- Centralize path generation and validation so new Den tools reuse the same rules.

---

## Implementation direction

The next implementation slice should be **Phase 4 handoff**.

The first concrete piece is to add:

1. a Den path generator for schema-owned durable artifact paths;
2. a shared task-intent writer;
3. `den.work_plan.request_handoff`, which writes a `talk/tasks` or `pair/tasks` intent artifact through that shared writer and records the generated path on the work plan.

Implementation should preserve the core boundary from this ADR: agent input schemas provide semantic fields, never arbitrary schema-owned durable artifact paths.

---

## Related ADRs

- [Semantic Bear Memory](semantic-bear-memory.md)
- [Bear Memory Tool Boundary](bear-memory-tool-boundary.md)
- [MemFS Sidecar Repo Views](memfs-sidecar-repo-views.md)
