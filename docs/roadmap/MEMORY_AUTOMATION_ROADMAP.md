# Memory Automation Roadmap

For the canonical role model and current role names, see [bear roles](../architecture/bear-roles.md).
Status: implementation roadmap; P0 pair-reflection proposal enqueue is implemented for ACP close.

This roadmap sequences the remaining work needed for `pair` learning to become useful to `work` through reflection, review governance, `core/`, Cabinet, task context, and Letta Archives.

Related docs:

- [Pair Reflection and Work Memory Sharing Plan](PAIR_REFLECTION_AND_WORK_MEMORY_PLAN.md) — focused pair→curate→work boundary design.
- [Review Memory Governance Plan](CURATE_MEMORY_GOVERNANCE_PLAN.md) — focused memory proposal and core-write governance design.
- [Reflection System Shared Infrastructure Plan](REFLECTION_SYSTEM_PLAN.md) — queue, runner, scheduler, and shared control-plane design.
- [Memory Tools Implementation Plan](MEMORY_TOOLS_IMPLEMENTATION_PLAN.md)
- [Memory Model](../concepts/../architecture/memory-model.md)

---

## Target end state

```text
pair learns useful workplace knowledge
→ pair writes role-local memory
→ pair reflection summarizes/consolidates pair memory
→ pair reflection creates `bear_memory_proposals`
→ queued `memory_curate` reflection run processes proposals
→ review updates core / indexes archives / prepares task context / creates Cabinet proposals
→ work receives approved task context and can search permitted archives
```

`work` must never read raw `pair/`. It should benefit through curated channels only.

---

## P0 — Pair reflection to review trigger

Status: implemented for the ACP close path, except UI surfacing.

### Goal

Pair reflection should immediately feed curation without waiting for manual human action.

### Deliverables

1. ✅ Pair reflection writes a `pair/summaries/` entry on ACP close.
2. ✅ Pair reflection creates a `bear_memory_proposals` row referencing that summary.
3. ✅ Pair reflection enqueues a queued `bear_reflection_runs` row with `lane = memory_curate` and `trigger = pair_reflection`.
4. ✅ ACP close remains responsive; curation does not run inline during ACP close.
5. Pending: UI shows the generated proposal and queued review run.

### Notes

- The proposal uses `suggested_action: unspecified` initially.
- The automatic proposal has `source_role = pair`, `source_paths = [pair/summaries/...]`, `sensitivity = normal`, and `requires_human = false`.
- Review decides final outcome.
- Human review is an escalation path, not the default workflow.

---

## P1 — Automated review conductor

### Goal

Make `review` autonomous for memory review and `core/` cleanliness.

### Conversation policy

Use one review conversation per Bear + lane + UTC day.

```text
conversation_key = memory_curate:YYYY-MM-DD
```

Rollover:

- MVP: new conversation per UTC day.
- Later: also roll over when context is near full or operational policy requests reset.

### Trigger policy

MVP triggers:

- pair reflection creates a memory proposal;
- manual admin/operator trigger.

Future trigger:

- dynamic heartbeat based on memory pressure signals.

### Data model

Implemented lane-neutral storage:

- `bear_reflection_runs`
  - `id`
  - `bear_id`
  - `lane`
  - `trigger`
  - `status`
  - `role_agent_id`
  - `conversation_id`
  - `conversation_key`
  - `conversation_date`
  - `input_summary jsonb`
  - `output_summary jsonb`
  - `error text`
  - `started_at`
  - `completed_at`
  - `created_at`
- `bear_reflection_run_items`
  - optional per-run item links; MVP stores proposal IDs in `input_summary`
- `reflection_conversations`
  - `bear_id`
  - `role_agent_id`
  - `lane`
  - `conversation_date`
  - `conversation_key`
  - `conversation_id`
  - `created_at`
  - `last_used_at`

Unique key:

```text
bear_id + lane + conversation_date
```

### Runner behavior

A memory-review cycle should:

1. Load pending proposals.
2. Resolve or create the daily review conversation.
3. Prompt the review role with bounded context.
4. Allow only approved Den memory/proposal tools.
5. Record cycle state and outputs.
6. Surface cycle activity in UI.

### Dynamic heartbeat later

Not MVP, but design for:

```text
evaluate_curate_memory_pressure(bear_id) -> should_run, lane, reason, priority
```

Signals:

- pending proposal count;
- age of oldest proposal;
- recent pair reflection volume;
- watch observations;
- work results;
- stale `core/` indicators;
- failed/queued cycles.

---

## P2 — Model-assisted pair reflection

### Goal

Upgrade deterministic pair summaries into useful role-local reflection.

### Behavior

A model-assisted pair reflection pass should extract:

- durable technical decisions;
- repeated failure modes;
- repo/workplace conventions;
- human preferences relevant to pair work;
- candidate memory proposals;
- items that should remain local.

### Inputs

- recent ACP pair messages;
- local tool activity summary;
- pair memory entries created during the session;
- relevant `core/` orientation;
- authenticated human identity from ACP token.

### Outputs

Writes only to `pair/`:

```text
pair/summaries/
pair/reflections/
pair/decisions/
pair/notes/
```

May create memory proposals.

### Constraints

- No `core/` writes.
- No Cabinet writes.
- No external tools.
- No raw cross-role branch reads.
- Not a sixth Bear role.

---

## P3 — Letta Archive indexing

### Goal

Use Letta Archives as semantic retrieval indexes over canonical BEARS sources without introducing a BEARS vector store.

### Archive types

| Archive | Purpose |
|---|---|
| Bear curated archive | Semantic recall over selected `core/`, approved proposal outcomes, durable summaries. |
| Cabinet Mission archive | Optional cross-Bear semantic recall for Cabinet Missions. |
| Role-local archive | Optional later role-specific long-tail recall; not for duplicating `core/`. |

### Data model

Add `bear_archives`:

- `id`
- `bear_id`
- `archive_id`
- `archive_kind`
  - `bear_curated`
  - `cabinet_mission`
  - `role_local`
- `mission_ref null`
- `role null`
- `created_at`
- `updated_at`

Add `bear_archive_index_entries`:

- `bear_id`
- `archive_id`
- `passage_id`
- `canonical_kind`
- `canonical_id`
- `source_uri`
- `source_path`
- `source_role`
- `source_version`
- `source_hash`
- `chunk_key`
- `chunk_index`
- `text_hash`
- `metadata jsonb`
- `tags text[]`
- `indexed_at`
- `deleted_at`

### Sync behavior

- unchanged source hash: no-op;
- changed source hash: delete old passage, create new passage;
- deleted canonical source: delete passage, mark index row deleted;
- search results should point back to canonical sources.

### Write boundary

Shared archive writes go through Den/curate indexing workflows. Role agents should not collaboratively maintain shared archives.

---

## P4 — Semantic archive search for work

### Goal

Allow `work` to benefit from curated pair/curate learning without reading raw `pair/`.

### Tool

Canonical:

```text
den.memory.semantic_search
```

Provider:

```text
memory_semantic_search
```

### Work policy

`work` may search:

- Bear curated archive;
- Cabinet Mission archives attached to the task/Bear;
- task-permitted Cabinet/Archive scopes.

`work` must not search:

- raw pair-local archives unless explicitly curated/attached;
- raw `pair/` MemFS;
- unrelated Bear archives.

### Outputs

Search results must include:

- passage snippet;
- archive id/kind;
- canonical id;
- source URI/path;
- source hash/version;
- tags;
- instruction to fetch canonical source when exact truth matters.

---

## P5 — Work task context bridge

### Goal

Attach curated pair-derived knowledge to approved `work` tasks.

### Flow

```text
pair memory / pair reflection
→ memory proposal
→ review review
→ task context attachment
→ work receives scoped context
```

### Deliverables

1. Extend task/work schemas with memory source refs and curated context excerpts.
2. Review can attach selected proposal summaries to task context.
3. Work task prompt includes:
   - curated summary;
   - source refs;
   - relevant `core/` paths;
   - permitted archive refs;
   - explicit scope and tools.
4. UI shows which pair-derived memory informed a task.

### Constraints

- Attach distilled summaries, not raw pair logs.
- Include provenance.
- Respect human/sensitivity policy.
- Keep task context bounded.

---

## P6 — Core and archive outcome refinement

### Goal

Keep shared memory clean and searchable.

### Deliverables

1. `memory_apply_core_update` supports bounded append/create/replace workflows.
2. Review can compact `core/` files/sections.
3. Review can index curated summaries into Bear Archive.
4. UI shows source proposal → core path → archive passage mapping.
5. Revert/rollback flow is designed for bad shared memory updates.

---

## Observability requirements

The UI should surface:

- pair reflection runs;
- memory proposals;
- queued and completed reflection runs;
- proposal decisions;
- `core/` updates;
- archive indexing changes;
- work task context attachments;
- failures/skips/retries.

Humans should see what the system is doing and override when necessary, without approving every routine memory operation.

---

## Immediate next implementation sequence

1. ✅ Pair reflection creates a `bear_memory_proposals` row and enqueues a `memory_curate` run.
2. ✅ Add lane-neutral `bear_reflection_runs`, `bear_reflection_run_items`, and `reflection_conversations` storage.
3. Next: add manual/queued conductor runner for the `memory_curate` lane.
4. Next: surface generated proposals and queued reflection runs in UI.
5. Later: add model-assisted pair reflection.
6. Later: add Bear curated archive provisioning and index table.
7. Add `memory_semantic_search` for review/pair/work by policy.
8. Add work task context bridge.
