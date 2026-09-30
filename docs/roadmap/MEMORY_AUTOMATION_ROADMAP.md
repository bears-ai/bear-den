# Memory Automation Roadmap

> **Direction changed (2026-09).** The legacy proposal→`core/` auto-promotion and model-facing `memory_apply_core_update` tool are retired on all Bears; historical core-action proposals are rejected without publication or a human-review queue. Future autonomous sharing must verify canonical source and hat/Work audience ([hats plan](HATS_AND_SESSION_MEMORY_BOUNDARIES_PLAN.md)). Canonical memory is per-Bear SQLite ([ADR-0031](../decisions/adr-0031-sqlite-first-canonical-store-for-bear-agent-memory-and-tasks.md)); Letta Archives and `pair/` MemFS branches are removed. Long-term recall is a **derived Qdrant index** over canonical SQLite ([ADR-0038](../decisions/adr-0038-platform-embedding-standard-and-derived-recall-index.md)); the engine that *fills* it (extraction-first **harvest** + **consolidation** by supersession) and recall scoring are defined in [ADR-0041](../decisions/adr-0041-archival-recall-and-async-curation.md). Canonical target: [Den runtime](../architecture/den-runtime.md) ([runtime plan](DEN_RUNTIME_PLAN.md)).

For the canonical stance model and current stance names, see [bear stances](../architecture/bear-stances.md).
Status: implementation roadmap; P0 pair-reflection proposal enqueue and P2 compaction-summary-assisted pair reflection proposals are implemented for ACP close. Remaining automation must now prioritize proving positive memory quality over adding more queue plumbing: compaction may schedule and scope harvest, but reflection must perform memory-specific extraction over evidence (ADR-0041 2026-07 amendment).

This roadmap sequences the work needed for `pair` learning to become useful to `work` through reflection, curation, `core/`, Cabinet, task context, and the **derived recall index**. The safety-critical flywheel is now in place; further automation should wait for concrete operational evidence.

Related docs:

- [Pair Reflection and Work Memory Sharing Plan](PAIR_REFLECTION_AND_WORK_MEMORY_PLAN.md) — focused pair→curate→work boundary design.
- [Memory Curation Plan](MEMORY_CURATION_PLAN.md) — focused memory proposal and core-write curation design.
- [Reflection System Shared Infrastructure Plan](REFLECTION_SYSTEM_PLAN.md) — queue, runner, scheduler, and shared control-plane design.
- [Memory Tools Implementation Plan](MEMORY_TOOLS_IMPLEMENTATION_PLAN.md)
- [Memory Model](../concepts/../architecture/memory-model.md)

---

## Target end state

```text
pair learns useful workplace knowledge
→ pair writes role-local memory (per-Bear SQLite)
→ pair reflection summarizes/consolidates pair memory
→ pair reflection creates memory proposals
→ archive_harvest mines older closed sessions for more candidates
→ queued `memory_curate` reflection run resolves proposals (dedup + supersession)
→ curate updates core / requests recall indexing / prepares task context / creates Cabinet proposals
→ work receives approved task context and can search permitted recall scopes
```

`work` must never read raw `pair/`. It should benefit through curated channels only.

---

## P0 — Pair reflection to curate trigger

Status: implemented for the ACP close path, except UI surfacing.

### Goal

Pair reflection should immediately feed curation without waiting for manual human action.

### Deliverables

1. ✅ Pair reflection writes a `pair/summaries/` entry on ACP close.
2. ✅ Pair reflection creates a `bear_memory_proposals` row referencing that summary.
3. ✅ Pair reflection enqueues a queued `bear_reflection_runs` row with `lane = memory_curate` and `trigger = pair_reflection`.
4. ✅ ACP close remains responsive; curation does not run inline during ACP close.
5. 🟡 UI shows generated memory proposals on the Bear memory dashboard, including native SQLite proposal rows, and recent reflection-run performance (queue wait/runtime/outcomes); dedicated run detail/retry UI remains pending.

### Notes

- The proposal uses `suggested_action: unspecified` initially.
- The automatic proposal has `source_role = pair`, `source_paths = [pair/summaries/...]`, `sensitivity = normal`, and `requires_human = false`.
- Curate decides final outcome.
- Human review is an escalation path, not the default workflow.

---

## P1 — Automated curation conductor

### Goal

Make `curate` autonomous for memory review and `core/` cleanliness.

### Conversation policy

Use one curate conversation per Bear + lane + UTC day.

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
2. Resolve or create the daily curate conversation.
3. Prompt the `curate` role with bounded context.
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

Status: 🟡 v1 implemented for ACP close by extracting durable-looking candidates from the latest structured compaction summary. This is now treated as a stopgap guardrail, not the target architecture: compaction can schedule/scope reflection, but meaningful memory requires a memory-specific extraction pass over source evidence.

### Goal

Upgrade deterministic pair summaries into useful role-local reflection.

### Behavior

A model-assisted pair reflection pass should extract (v1 does this from structured compaction summaries):

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

### Current v1

- ACP `session.close` runs pair compaction, then reads the latest structured compaction summary.
- Decisions and constraints become `retain_profile_local` memory proposals only when the deterministic guardrail can state a durable-looking claim.
- Goals, workflow refs, artifact-only refs, and unresolved follow-ups are treated as continuation state and are not proposed as durable memory unless a later extraction pass can tie them to a durable preference, decision, constraint, fact, or lesson.
- Secret/person/external-risk signals use proposal sensitivity and force human review.
- This path is intentionally conservative: avoiding bad memory is useful, but it does not prove that meaningful memory works.

### Replacement target

Pair reflection should move from bucket promotion/filtering to the same extraction-first contract as `archive_harvest`:

1. Use compaction/session-close as the trigger and bounded source window.
2. Read source evidence for that window: user/assistant messages, relevant tool summaries, and any pair memory entries written during the session.
3. Run a memory-specific extractor that emits structured candidates and discards:

   ```json
   {
     "candidates": [
       {
         "kind": "preference|decision|fact|constraint|lesson",
         "content": "durable semantic statement",
         "rationale": "why this helps future sessions",
         "source_message_ids": ["..."],
         "confidence": 0.0,
         "sensitivity": "normal|person|secret_risk|external_untrusted",
         "suggested_action": "retain_profile_local|human_review|discard"
       }
     ],
     "discarded": [
       {
         "source": "message/span/ref",
         "reason": "transient follow-up|assistant prose|task state|artifact ref without semantic claim|duplicate"
       }
     ]
   }
   ```

4. Create proposals only for future-useful semantic candidates with evidence.
5. Store discard reasons in run/proposal metadata so quality failures are debuggable.

### Deferred until evidence

- Richer confidence explanations beyond the initial schema.
- Promotion from goals/workflow buckets when they encode durable preferences or conventions.
- Semantic dedup/consolidation beyond exact-claim metadata.

### Shared extraction contract v0

The replacement path uses one backend-agnostic contract for pair reflection and archive harvest. Compaction may create the job and provide hints, but the extractor input must keep source evidence separate from those hints.

Input bundle:

```json
{
  "source_kind": "pair_session|compaction_span|conversation_archive",
  "source_ref": "stable session/span/archive identifier",
  "bear_id": "...",
  "conversation_id": "...",
  "session_id": "...",
  "compaction": {
    "artifact_id": "optional scheduler/provenance id",
    "policy_version": "optional compaction policy",
    "source_message_start_seq": 1,
    "source_message_end_seq": 99,
    "hints": ["possible_preference", "possible_decision"]
  },
  "messages": [
    {
      "id": "message id or seq",
      "seq": 1,
      "role": "user|assistant|tool|system",
      "content": "bounded source text",
      "created_at": "optional timestamp"
    }
  ],
  "artifacts": [
    {
      "id": "optional artifact id/path",
      "kind": "tool_summary|memory_entry|file_ref|other",
      "content": "bounded source text"
    }
  ]
}
```

Output:

```json
{
  "candidates": [
    {
      "kind": "preference|decision|fact|constraint|lesson",
      "content": "durable semantic statement, not transcript prose",
      "rationale": "why this helps future sessions",
      "source_message_ids": ["..."],
      "source_artifact_ids": ["..."],
      "confidence": 0.0,
      "sensitivity": "normal|person|secret_risk|external_untrusted",
      "suggested_action": "retain_profile_local|human_review|discard"
    }
  ],
  "discarded": [
    {
      "source_message_ids": ["..."],
      "source_artifact_ids": ["..."],
      "reason": "transient_followup|assistant_only|task_state|artifact_ref_without_semantic_claim|duplicate|not_durable|unsafe|invalid_candidate"
    }
  ]
}
```

Validation before proposal creation:

- `content` must be non-empty, future-useful, and distinct from raw transcript/bucket labels.
- every candidate must have at least one source message or artifact ref;
- user-authored preferences, decisions, and constraints must include user-message evidence, not only assistant summaries;
- `kind`, `sensitivity`, and `suggested_action` are allowlisted;
- invalid candidates become discarded entries with `invalid_candidate` rather than crashing or silently disappearing;
- discard-only output is a valid harvest result.

The deterministic test fixture uses the same contract with a fake extractor backend. Its source bundle contains one user preference, one assistant acknowledgement, and one transient reminder/task follow-up. The fake output contains one semantic candidate grounded in the user message plus discard reasons for assistant-only and transient material. This tests the pipeline contract without pretending to evaluate model quality.

### Positive smoke test

Before more automation, add one runnable end-to-end memory smoke test:

- Input: one synthetic closed pair session/span where the user states one durable preference or decision, plus assistant suggestions and transient task follow-up residue.
- Expected: exactly one meaningful memory proposal whose content is a future-useful semantic statement grounded in user/source evidence.
- Expected discards: assistant-only claims, unresolved follow-ups, workflow refs, and artifact refs without semantic claims are discarded with reasons.
- Failure mode: if the extractor emits zero candidates or only bucket-shaped summaries, the reflection path is not product-ready.

### Constraints

- No `core/` writes.
- No Cabinet writes.
- No external tools.
- No raw cross-role branch reads.
- Not a sixth Bear role.

---

## P2.5 — Proactive archive harvest (`archive_harvest`)

### Goal

Turn closed session archives into durable memory candidates, not just the active session. This is the [ADR-0041](../decisions/adr-0041-archival-recall-and-async-curation.md) `archive_harvest` Reflection lane.

### Behavior

- 🟡 Compaction-artifact harvest is implemented as a conservative stopgap: scan **un-mined** compaction artifacts, keep only durable-looking decisions/constraints/goals, and discard transient follow-up/workflow/artifact-only residue. This reduces junk but is not the target architecture.
- Next target: use compaction artifacts only to schedule/scope candidate spans, then run a memory-specific extractor over episodic evidence from the underlying session/messages/artifacts.
- ✅ Emit memory proposals (candidate durable entries) with harvest provenance (`source_hash`, `run_id`, source refs); do not write `core/`. The former proposal→core `memory_curate` write path is retired; a future verified-source/audience path is required.
- 🟡 Apply a deterministic quality/risk filter before a candidate becomes a proposal: transient follow-up-only artifacts and goal/workflow-only summaries are marked harvested without a proposal; durable decisions/constraints receive confidence metadata; artifact-only candidates are discarded unless a semantic claim can be stated; person/secret/external-risk signals set proposal sensitivity for human review. Fixture coverage exists for each deterministic branch. Richer model-assisted extraction is no longer deferred on "proposal quality metrics" alone; it is required to prove positive meaningful memory.

### Triggers

- `session_archived`;
- `cumulative_salience_threshold`;
- throttled/adaptive heartbeat (never a fixed cron).

### Data model

✅ `memory_harvest_marks` (per-Bear SQLite) is implemented for idempotency: `source_kind` (`conversation` | `compaction_artifact` | `observation` | `pair_summary`), `source_ref`, `source_hash`, `harvested_at`, `run_id`, `proposal_ids_json`. Current compaction-artifact harvest records `source_hash` and the reflection `run_id`. See [ADR-0041](../decisions/adr-0041-archival-recall-and-async-curation.md) schema deltas.

### Constraints

- transcripts are source material, never auto-promoted to memory;
- bounded token budget per run;
- feeds `memory_curate` for dedup, supersession, and promotion. Deterministic exact-claim/different-path matches now add human-review `consolidation_review` metadata; broader semantic duplicate/supersession scoring is deferred until duplicate/conflict misses become a real review burden.

---

## P3 — Derived recall indexing (`archive_index`)

### Goal

Maintain a **derived Qdrant recall index** over canonical SQLite (and Cabinet) sources ([ADR-0038](../decisions/adr-0038-platform-embedding-standard-and-derived-recall-index.md)). This replaces Letta Archives. Vectors are derived and rebuildable; SQLite remains canonical.

### What is indexed

Per [ADR-0038](../decisions/adr-0038-platform-embedding-standard-and-derived-recall-index.md) §4 and [ADR-0041](../decisions/adr-0041-archival-recall-and-async-curation.md): `visibility=normal` shared records and role-local `note`/`decision`/`summary` (**latest non-invalid head only**, respecting `supersedes_memory_id`/`invalid_at`); approved proposal outcomes; Cabinet material where approved. Excluded by default: `scratch`, raw `log` streams, pending proposals/observations, superseded/archived bodies, and transcripts.

### Data model (passage registry, not vectors)

Vectors live in **Qdrant**; Den **Postgres** holds passage-registry metadata only ([ADR-0038](../decisions/adr-0038-platform-embedding-standard-and-derived-recall-index.md) §3): passage id, `embedding_standard`, `source_class`, canonical source ids, `content_hash`, chunk bounds, `indexed_at`, supersession/delete state. Detailed schema lives in the [Derived recall index implementation plan](DERIVED_RECALL_INDEX_IMPLEMENTATION_PLAN.md).

### Sync behavior

- unchanged `content_hash`: no-op;
- changed `content_hash`: re-embed and replace the passage;
- superseded/deleted/archived canonical source: delete passages by source id + hash or skip on reconcile;
- bear package import rebuilds vectors from `memory.sqlite`; vectors are never shipped;
- search results point back to canonical sources.

### Write boundary

Indexing runs through Den/curate workflows; role agents do not maintain the shared index. `archive_harvest` produces canonical records; `archive_index` indexes them — the two lanes stay separate.

---

## P4 — Semantic recall search for work

### Goal

Allow `work` to benefit from curated pair/curate learning without reading raw `pair/`.

### Tool

Upgrade `memory_search` to **hybrid** (vector recall over Qdrant when configured, else SQL `LIKE`), ranked by `recency × relevance × importance × freshness_trend` ([ADR-0041](../decisions/adr-0041-archival-recall-and-async-curation.md)); degrade gracefully to anchors + `LIKE` when Qdrant is unavailable. Vector/keyword/graph hits now carry salience plus lifecycle/freshness indicators. An optional `den.memory.recall` / `memory_recall` may be added if a dedicated recall entry point is preferred over overloading `memory_search`.

### Work policy

`work` may search:

- shared `core/` recall;
- Cabinet Mission recall scopes attached to the task/Bear;
- task-permitted Cabinet/recall scopes.

`work` must not search:

- raw role-local memory of other roles unless explicitly curated/attached;
- raw `pair/`;
- unrelated Bears (ACL by membership + identity scope, [ADR-0015](../decisions/adr-0015-multi-user-memory.md)).

### Outputs

Recall results must include:

- passage snippet;
- `source_class` and canonical id;
- source path/ref;
- `content_hash`/version;
- relevance score;
- instruction to fetch the canonical source when exact truth matters.

---

## P5 — Work task context bridge

### Goal

Attach curated pair-derived knowledge to approved `work` tasks.

### Flow

```text
pair memory / pair reflection
→ memory proposal
→ curate review
→ task context attachment
→ work receives scoped context
```

### Deliverables

1. Extend task/work schemas with memory source refs and curated context excerpts.
2. Curate can attach selected proposal summaries to task context.
3. Work task prompt includes:
   - curated summary;
   - source refs;
   - relevant `core/` paths;
   - permitted recall scopes;
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

1. **Revised:** replace the retired model-facing `memory_apply_core_update` path with typed, canonically sourced promotions to a verified hat and, separately, reviewed hat→`core/` publication. No proposal path/sensitivity label grants a shared write ([hats plan](HATS_AND_SESSION_MEMORY_BOUNDARIES_PLAN.md)).
2. Curate can compact `core/` records/sections via supersession (not destructive overwrite).
3. Curate can request recall indexing of curated summaries (`archive_index`).
4. UI shows source proposal → `core/` record → recall passage mapping.
5. Revert/rollback flow is designed for bad shared memory updates (supersession chain is the audit trail).

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

1. ✅ Pair reflection creates a memory proposal and enqueues a `memory_curate` run.
2. ✅ Add lane-neutral `bear_reflection_runs`, `bear_reflection_run_items`, and `reflection_conversations` storage.
3. ✅ **Tool exposure (read side)** — `chat`, `pair`, `curate`, `work`, and `watch` have read/status/search descriptors; write/review policy for `work`/`watch` remains open.
4. ✅ Apply core [ADR-0041](../decisions/adr-0041-archival-recall-and-async-curation.md) store deltas (`salience` on `memory_records`, `valid_from`/`invalid_at`, store-level supersession invalidation, `memory_harvest_marks`) and deterministic proposal safety metadata.
5. ✅ Derived Qdrant recall index (P3) and hybrid scored `memory_search` (P4) are landed; remaining recall work is live ops exercise and deeper salience/freshness scoring.
6. **Next: prove positive memory extraction.** Add the golden smoke fixture from P2: one durable user preference/decision plus residue must yield one semantic proposal and discard reasons.
7. Replace pair-reflection bucket promotion with a shared memory-extraction module that consumes bounded source evidence and emits the structured `candidates`/`discarded` schema.
8. Wire `archive_harvest` to the same extractor: compaction artifacts schedule/scope source spans; they no longer provide proposal bodies.
9. Surface extractor quality in UI: proposal evidence, discard reasons, and counts for no-op harvests.
10. Then: add manual/queued conductor runner for `memory_curate` and model-assisted consolidation/supersession policy.
11. Then: work task context bridge (P5).
