# Memory curation plan

> **Direction changed (2026-06).** The curation lanes stand, but the canonical store is per-Bear SQLite ([ADR-0031](../decisions/adr-0031-sqlite-first-canonical-store-for-bear-agent-memory-and-tasks.md)) — `memory_records`/`memory_promotions`/`memory_proposals` — not MemFS `core/`/stance branches or Letta Archives. Semantic recall is a **derived Qdrant index** over SQLite ([ADR-0038](../decisions/adr-0038-platform-embedding-standard-and-derived-recall-index.md)); harvest, consolidation by supersession, and recall scoring are defined in [ADR-0041](../decisions/adr-0041-archival-recall-and-async-curation.md). Canonical target: [Den runtime](../architecture/den-runtime.md) ([runtime plan](DEN_RUNTIME_PLAN.md)).
>
> **Note.** `core/` paths denote logical-path projections over SQLite records, not files on a branch. The legacy proposal→`core/` auto-promotion and model-facing `memory_apply_core_update` tool are retired for both hat and no-hat Bears. See the [hats plan](HATS_AND_SESSION_MEMORY_BOUNDARIES_PLAN.md) for the verified source/audience replacement; do not interpret the older tool table below as deployed behavior.

For the canonical stance model and current stance names, see [bear stances](../architecture/bear-stances.md).
Status: focused design plan. Implementation status and sequencing live in [Memory Automation Roadmap](MEMORY_AUTOMATION_ROADMAP.md).

This plan designs how memories move between role-local branches and shared Bear memory. It focuses on the `memory_curate` lane of BEARS **Reflection** system and the `curate` role as the only role allowed to integrate role-local memory into shared `core/` memory or propose/promote Cabinet updates.

Related docs:

- [Memory Automation Roadmap](MEMORY_AUTOMATION_ROADMAP.md) — canonical implementation status and sequencing.
- [Reflection system shared infrastructure plan](REFLECTION_SYSTEM_PLAN.md)
- [Memory tools implementation plan](MEMORY_TOOLS_IMPLEMENTATION_PLAN.md)
- [Derived recall index implementation plan](DERIVED_RECALL_INDEX_IMPLEMENTATION_PLAN.md)
- [ADR-0041 — Archival recall and asynchronous curation](../decisions/adr-0041-archival-recall-and-async-curation.md)
- [ADR-0018 — Reflection system](../decisions/adr-0018-reflection-system.md)
- [ADR-0021 — Semantic bear memory](../decisions/adr-0021-semantic-bear-memory.md)
- [ADR-0031 — SQLite-first canonical store](../decisions/adr-0031-sqlite-first-canonical-store-for-bear-agent-memory-and-tasks.md)
- [Memory model](../architecture/memory-model.md)

---

## Goal

Give BEARS a governed, inspectable mechanism for memory movement:

1. Role-local memories can remain local forever.
2. Role-local memories can be proposed for promotion when useful beyond one role.
3. `curate` can review all role branches and decide what to do.
4. Approved durable shared knowledge is written to `core/`.
5. Cabinet-worthy knowledge is proposed or written through Cabinet-specific workflows, not silently copied.
6. Every movement records provenance and leaves an audit trail.
7. A derived Qdrant recall index ([ADR-0038](../decisions/adr-0038-platform-embedding-standard-and-derived-recall-index.md)) provides semantic retrieval over canonical SQLite; it is derived and rebuildable, never the source of truth.
8. Curation runs as a bounded Reflection lane that can be triggered by heartbeat, proposals, memory writes, session archival, or manual request.
9. Heartbeat cadence is throttled: active Bears can run memory curation more frequently than dormant Bears.

---

## Non-goals

- Do not automatically promote all role memories.
- Do not let `chat`, `pair`, `work`, or `watch` write directly to `core/`.
- Do not let `work` read channel branches or watch branches.
- Do not require promoted memories to have Cabinet objects.
- Do not treat Cabinet as a mirror of Bear memory.
- Do not allow agents to run destructive store operations or operator overrides.
- Do not let every role independently request recall indexing of `core/` content.
- Do not make the derived recall index the source of truth.

---

## Memory movement concepts

### Local memory

A memory entry written under a role branch such as `pair/decisions/...` or `watch/logs/...`.

Local memory may be final. It does not need promotion.

### Candidate

A local memory that has been identified as potentially useful elsewhere.

Candidate sources:

- explicit proposal from the writing role;
- `curate` finds it during review;
- a human marks it for review in the Den UI;
- the **`archive_harvest`** Reflection lane mines closed session archives/compaction artifacts via an extraction-first pass ([ADR-0041](../decisions/adr-0041-archival-recall-and-async-curation.md));
- a Reflection heartbeat or event-triggered curation run surfaces it.

### Proposal

A structured review object that asks `curate` to take an action.

Proposal destinations:

- `core` update;
- Cabinet update;
- no promotion / reject;
- archive/supersede local entry;
- task/skill proposal handoff, if the memory is actually a task or procedure.

Do not implement a separate `local_final` lifecycle in the first slices. Keeping memory local is the default outcome when no proposal is approved.

### Promotion

A reviewed action that writes durable shared Bear memory under `core/` or submits/creates a Cabinet update.

Promotion should be a new commit with provenance, not a raw file copy.

---

## Proactive harvest and consolidation

> See [ADR-0041 — Archival recall and asynchronous curation](../decisions/adr-0041-archival-recall-and-async-curation.md) for the decision; this section is the design detail.

Curation must do more than drain a proposal queue. It is the bears equivalent of **sleep-time compute** ([Letta](https://www.letta.com/blog/sleep-time-compute)): a background engine that, off the live turn, turns raw experience into durable, recallable knowledge. Two new responsibilities sit alongside proposal review:

### Harvest (`archive_harvest` lane)

Proactively scan **un-mined** closed sessions and compaction artifacts and run an **extraction-first** pass — distill atomic facts, decisions, preferences, and lessons; discard conversational filler — emitting memory proposals rather than indexing raw messages. This is what fills archival memory.

- **Idempotency:** record processed sources in `memory_harvest_marks` (source kind + ref + hash); never re-harvest unchanged sources.
- **Triggers:** `session_archived`, `cumulative_salience_threshold`, and a throttled/adaptive heartbeat — not a fixed cron.
- **Provenance:** every candidate links back to source `conversation_messages`.
- **Quality filter:** drop low-confidence extractions before they become proposals (guards against hallucination propagation). The current deterministic compaction-artifact filter keeps decisions/constraints/artifact refs, drops follow-up-only and goal/workflow-only summaries, tags artifact-only candidates as medium confidence, and routes person/secret/external-risk signals to human review; richer model-assisted confidence scoring is deferred until proposal quality metrics show a problem.

### Consolidation

Before writing `core/`, reconcile candidates against existing canonical memory:

- **Dedup** — semantically identical candidate ⇒ no-op (optionally bump salience). Exact duplicate core updates already record a `dedupe_core_noop` promotion without writing a new memory record. Proposal creation now also adds human-review `consolidation_review` metadata when an exact normalized claim already exists at a different active logical path; broader model-assisted semantic dedup is deferred until duplicate proposals become a real review burden.
- **Supersession, not overwrite** — a contradicting candidate writes a *new* record that sets `supersedes_memory_id` and encodes the transition ("previously X; now Y"); the old record is marked `invalid_at` and preserved as history. This is the bears-native form of temporal fact invalidation, without a graph database.
- **Synthesis** — when cumulative `salience` over recent records crosses a threshold, synthesize a higher-level `reflection` record (Generative-Agents-style) and store it as retrievable memory.

### Schema deltas (sketch)

Additive; preserves append-only and single-writer-per-Bear ([ADR-0031](../decisions/adr-0031-sqlite-first-canonical-store-for-bear-agent-memory-and-tasks.md)):

- `memory_records.salience` — first-class importance on durable memory (today only `memory_observations` has it); drives reflection triggering and recall ranking.
- `memory_records.valid_from` / `memory_records.invalid_at` — bi-temporal-lite; `created_at` stays transaction time.
- **Begin writing `supersedes_memory_id`** (present in schema, currently unused).
- `memory_harvest_marks` — harvest provenance/idempotency.

Recall ranking then becomes `recency × relevance × importance`, degrading to anchors + `LIKE` without Qdrant ([ADR-0038](../decisions/adr-0038-platform-embedding-standard-and-derived-recall-index.md) §5, [ADR-0041](../decisions/adr-0041-archival-recall-and-async-curation.md)).

---

## Role responsibilities

| Role | Memory responsibility |
|---|---|
| `chat` | Writes conversational role-local memory and may propose durable shared updates. |
| `pair` | Writes coding/pairing notes, logs, decisions, reflections, and summaries; may propose shared updates. |
| `watch` | Writes observations/logs from inbound events; should not decide shared truth. |
| `work` | Writes task/run-bound logs, decisions, summaries, and results; may propose durable lessons. |
| `curate` | Reads all branches, reviews candidates/proposals, writes `core/`, rejects/no-ops noisy memory, and manages memory integration state. |

---

## Tool model

### Read tools for `curate`

`curate` needs broad read access:

| Canonical | Provider-safe | Purpose |
|---|---|---|
| `den.memory.browse` | `memory_browse` | Browse all role branches and `core/`. |
| `den.memory.read` | `memory_read` | Read memory files from any branch. |
| `den.memory.search` | `memory_search` | Search path and content across all branches. |
| `den.memory.status` | `memory_status` | Inspect memory health/status across roles. |
| `den.memory.history` | `memory_history` | Inspect file/commit history. |
| `den.memory.diff` | `memory_diff` | Inspect proposed or committed changes. |

### Proposal tools for non-curate roles

Non-curate roles should not write `core/` or Cabinet directly. They can request review of role-local memory.

| Canonical | Provider-safe | Roles | Purpose |
|---|---|---|---|
| `den.memory.request_review` | `memory_request_review` | `chat`, `pair`, `work`, `watch` | Request curation of role-local memory without choosing the final outcome. |

`den.memory.request_review` supersedes narrower producer-side names such as `den.memory.propose_core_update`, `den.memory.propose_core_write`, and `den.memory.propose_cabinet_update`. The model-advertised request may include supported hints such as `propose_hat`, `cabinet_update`, `skill_review`, `retain_profile_local`, `delete_after_review`, `human_review`, or `unspecified`. `propose_hat` requires a canonical source-note UUID in a Bear-admin opted-in Work-off hat: Curate's tool-free synthesis may publish only newly authored text after Den rechecks source and audience. The model does not choose a target hat or authorization. The former `summarize_into_core` and `promote_to_core` hints are rejected for new model requests; historical proposals with those actions are resolved as rejected without publication or a human-review queue.

Configured-Bear Pair intake uses `source_memory_id` (a current UUID note from this conversation), not a role/path label. Den derives the hat from the authenticated conversation and records it separately from proposal JSON. Legacy no-hat Pair requests still reference role-local `source_paths`; neither request itself publishes shared content.

Initial `den.memory.request_review` input shape:

- `source_paths`: legacy no-hat Pair role-local paths; alternatively, `source_memory_id` supports a bound Pair's own canonical source-local note when autonomous learning is enabled for its Work-off hat;
- `title`: concise review title;
- `summary`: what the source memory says;
- `rationale`: why review is useful;
- `suggested_action`: optional action hint;
- `target_ref`: optional `core/`, Cabinet, skill, domain, project, or freeform target hint;
- `refs`: optional semantic references;
- `sensitivity`: `normal`, `person`, `secret_risk`, `external_untrusted`, or `unknown`;
- `requires_human`: optional human-review flag.

### Review tools for `curate`

| Canonical | Provider-safe | Purpose |
|---|---|---|
| `den.memory.list_proposals` | `memory_list_proposals` | List pending memory proposals. |
| `den.memory.read_proposal` | `memory_read_proposal` | Read one proposal with source pointers and status. |
| `den.memory.resolve_proposal` | `memory_resolve_proposal` | Resolve a proposal as approved, rejected, retained local, deferred, superseded, or human-review-needed. |
| ~~`den.memory.apply_core_update`~~ | ~~`memory_apply_core_update`~~ | **Retired:** not advertised/executable; see the [hats plan](HATS_AND_SESSION_MEMORY_BOUNDARIES_PLAN.md) for replacement. |
| `den.memory.mark_lifecycle` | `memory_mark_lifecycle` | Mark an existing memory record as `stale`, `superseded`, `archived`, `archive-candidate`, or back to `active` without rewriting content. |
| `den.memory.supersede_entry` | `memory_supersede_entry` | Future specialized helper; admin-reviewed hat→core writes use `supersedes_memory_id`, and `memory_mark_lifecycle` covers explicit lifecycle marking. |

### Cabinet tools

Cabinet should remain a separate capability surface.

Candidate future tools:

| Canonical | Provider-safe | Purpose |
|---|---|---|
| `cabinet.propose_update` | `cabinet_propose_update` | Create a Cabinet update proposal. |
| `cabinet.create_or_update` | `cabinet_create_or_update` | Review/human-approved Cabinet write. |
| `cabinet.link_memory` | `cabinet_link_memory` | Record a link from memory entry to Cabinet object. |

---

## Proposal storage

Proposals are durable records, not hidden state in document frontmatter.

> **Native runtime.** Under `AGENT_RUNTIME=native` proposals persist to per-Bear SQLite `memory_proposals` ([ADR-0031](../decisions/adr-0031-sqlite-first-canonical-store-for-bear-agent-memory-and-tasks.md); `den-memory` crate) — `proposal_id`, `source_memory_id`, `suggested_action`, `sensitivity`, `requires_human`, `status`, `payload_json`, `created_at`, `reviewed_at`. The Postgres `bear_memory_proposals` table below is the legacy (non-native) control-plane shape; its richer columns map into the SQLite `payload_json`. Treat the field list as the logical proposal model regardless of backing store.

Rationale for a structured store:

- easier UI queries;
- explicit review status;
- avoids hidden state in document frontmatter;
- easier authorization/audit;
- can link to multiple source records and targets.

Legacy Postgres table: `bear_memory_proposals`.

Fields:

- `id uuid primary key`
- `bear_id uuid not null`
- `source_role text not null`
- `source_agent_id text null`
- `source_paths text[] not null default '{}'`
- `source_refs jsonb not null default '[]'`
  - each source ref records role, path, canonical commit, and optional content hash at proposal time
- `proposal_type text not null default 'memory_review'`
  - `memory_review`
  - future specialized values only if needed
- `suggested_action text not null default 'unspecified'`
  - `unspecified`
  - `summarize_into_core`
  - `promote_to_core`
  - `cabinet_update`
  - `skill_review`
  - `retain_role_local`
  - `delete_after_review`
  - `human_review`
- `target_ref text null`
  - e.g. `core/charter.md`, `core/projects.md`, `cabinet:missions/bears`, a skill/playbook hint, or freeform target hint
- `title text not null`
- `summary text not null`
- `rationale text not null default ''`
- `proposed_content text null`
- `proposed_patch text null`
- `refs jsonb not null default '{}'`
- `sensitivity text not null default 'normal'`
  - `normal`, `person`, `secret_risk`, `external_untrusted`, `unknown`
- `requires_human boolean not null default false`
- `status text not null`
  - `pending`
  - `in_review`
  - `approved`
  - `rejected`
  - `retained_local`
  - `deferred`
  - `superseded`
  - `needs_human_review`

- `reviewer_role text null`
- `reviewer_agent_id text null`
- `review_notes text null`
- `decision_summary text null`
- `result_path text null`
- `result_commit text null`
- `created_at timestamptz not null default now()`
- `reviewed_at timestamptz null`

The UI should show proposal state without assuming a fixed list of memory kinds.

---

## Derived recall index integration

Semantic recall is a **derived Qdrant index** over canonical SQLite ([ADR-0038](../decisions/adr-0038-platform-embedding-standard-and-derived-recall-index.md)), not a canonical store. Keep SQLite/Cabinet canonical and treat the index as a rebuildable, ephemeral retrieval view.

### Canonical ownership

Canonical stores own IDs, versions, ACLs, deletes, and full content:

| Canonical source | Store |
|---|---|
| Shared Bear orientation (`core/`) | per-Bear SQLite `memory_records` (scope `shared`) |
| Role-local memory | per-Bear SQLite `memory_records` (scope `profile_local`) |
| Human-facing shared knowledge | Cabinet |
| Workflow state (tasks/jobs) | Den Postgres (Docket) and schema artifacts |
| Files/results | Garage artifacts |

Recall passages are summaries or pointers. On a retrieval hit, tools should fetch the canonical record by `memory_id`/logical path when exact truth matters.

### Recall scopes

| Scope | Purpose | Writers | Readers |
|---|---|---|---|
| Bear recall | Semantic recall over shared `core/` and approved role-local heads, approved proposal outcomes, durable references. | Den/curate `archive_index` runs | Attached role agents by policy + ACL |
| Cabinet Mission recall | Optional cross-Bear semantic recall for a Cabinet Mission. | Den/curate indexer | Bears/roles assigned to that Mission |

Cross-corpus recall (Bear ↔ Cabinet) is policy-gated, not a default global merge ([ADR-0038](../decisions/adr-0038-platform-embedding-standard-and-derived-recall-index.md) §7).

### Passage registry

Vectors live in Qdrant; Den **Postgres** holds passage-registry metadata only ([ADR-0038](../decisions/adr-0038-platform-embedding-standard-and-derived-recall-index.md) §3): passage id, `embedding_standard`, `source_class`, canonical source ids (`bear_id`, `memory_id`, `scope_type`, `scope_profile`, `logical_path`, `kind`, `visibility`), `content_hash`, chunk bounds, `indexed_at`, supersession/delete state. The detailed registry schema and indexing job live in the [Derived recall index implementation plan](DERIVED_RECALL_INDEX_IMPLEMENTATION_PLAN.md).

Sync behavior:

1. unchanged `content_hash`: no-op;
2. changed `content_hash`: re-embed and replace the passage;
3. superseded/deleted canonical source: delete passages by source id + hash;
4. recall results point back to canonical sources; verify `memory_id`/hash where strict correctness matters.

### Write boundary

Agents may search recall scopes by policy, but the shared index is not collaboratively maintained by every agent. Indexing goes through Den/curate `archive_index` workflows. `pair` contributes role-local notes and requests curation; **`curate`** decides whether a summary is promoted to `core/`, proposed to Cabinet, and/or requested for recall indexing. Harvest (`archive_harvest`) produces canonical records; `archive_index` indexes them — the two lanes stay separate ([ADR-0041](../decisions/adr-0041-archival-recall-and-async-curation.md)).

## Core write strategy

`curate` writes `core/` through Den-mediated tools, not raw arbitrary paths.

The highest-risk part of this design is keeping `core/` clean. `core/` should not become an append-only dumping ground for role-local memories. Curate must be able to **dream**, consolidate, defragment, rewrite, and prune shared memory so `core/` remains compact, current, and useful.

This makes memory maintenance a first-class curate responsibility, not a later cleanup task.

### Initial allowed core paths

Start with a small, human-readable set:

```text
core/charter.md
core/domains.md
core/projects.md
core/people.md
core/knowledge.md
core/decisions.md
core/policies.md
core/current-focus.md
core/results/
```

Later, allow more paths by policy.

### Write and maintenance modes

Initial write modes:

1. **Append section** — useful for structured logs such as decisions, but should not be the only mode.
2. **Replace exact text** — requires exact old text and current base commit.
3. **Create file** — allowed only under approved `core/` paths.
4. **Rewrite curated section** — replace a bounded generated/curated section with a cleaner summary.
5. **Compact file** — summarize, deduplicate, and prune a whole `core/` file or a named section.

Curate dreaming/defragmentation should prefer cleaned summaries over raw copies. Broad patch application can come later, but curate needs enough authority to maintain quality rather than only append.

### Provenance

Every core write should record provenance in the record's `metadata_json` (and a `memory_promotions` row):

- proposal id;
- source memory ids / logical paths;
- source roles;
- reviewer role/agent;
- timestamp;
- rationale;
- source commit(s) when available.

---

## Lifecycle state changes for source entries

Promotion should not necessarily delete source memories. SQLite is append-only ([ADR-0031](../decisions/adr-0031-sqlite-first-canonical-store-for-bear-agent-memory-and-tasks.md)); lifecycle changes are expressed by new records and links, not destructive edits.

When a proposal is approved, Den records the outcome via:

- a `memory_promotions` row (source → target, action, reviewer, notes);
- `supersedes_memory_id` set on a superseding record, with `invalid_at` marking the old head as no longer current ([ADR-0041](../decisions/adr-0041-archival-recall-and-async-curation.md));
- the resolved `memory_proposals` row (`status`, `reviewed_at`, payload pointers to `promoted_to`/Cabinet ref/`proposal_id`).

The superseded record is preserved as history (the audit trail), not deleted. A proposal record referencing `source_memory_id` is sufficient for the first slice.

---

## Curate run design

Curate is expected to be autonomous. Human intervention is a last resort for sensitive, ambiguous, or policy-blocked cases.

Curate can run as:

1. **Scheduled curation cycle** controlled by Den.
2. **Event-triggered review** after enough proposals or memory activity accumulates.
3. **Manual/human-triggered review** from Den UI as an override or debugging mechanism.

Initial implementation should support autonomous review first, with human review as an escalation path rather than the default path.

A curate run prompt should include:

- Bear identity and purpose;
- role, policy, and relevant Domains;
- pending proposals;
- recent role-local memory activity;
- search/read tools;
- hybrid recall over the derived Qdrant index when configured (else SQL `LIKE`);
- explicit instruction that role-local memory can remain local;
- explicit instruction to prefer concise `core/` summaries over copying raw logs;
- explicit instruction that recall passages are derived indexes and should point back to canonical sources.

Curate should produce one of:

- approve core update;
- propose Cabinet update;
- reject/no-op;
- supersede or compact existing core memory;
- ask for human review when privacy/sensitivity or policy is unclear.

---

## UI design

Extend the Bear memory UI with a **Curation** panel.

### Memory browser additions

For each selected memory file:

- `Propose for core`
- `Propose for Cabinet`
- `Mark for curate review`
- `Mark local/final` (admin/curate only)

### Curation queue

New route:

```text
/bear/{slug}/details/memory/curation
```

Shows:

- pending proposals;
- source role;
- source paths;
- proposed target;
- title/summary;
- created by;
- status;
- review actions.

### Review page

New route:

```text
/bear/{slug}/details/memory/curation/{proposal_id}
```

Shows:

- proposal details;
- source file previews;
- proposed content/patch;
- core target preview;
- approve/reject forms;
- resulting commit/path after approval.

Human admins should be able to inspect, override, and manually review, but manual review is an operations fallback. The primary product posture is that the Bear can maintain its own memory through the `curate` role.

---

## Implementation tracker

Detailed phase status, completed work, and next implementation steps are tracked in [Memory Automation Roadmap](MEMORY_AUTOMATION_ROADMAP.md). This document should stay focused on memory curation rules, tool boundaries, proposal lifecycle, core-write policy, archive integration design, and curation UI behavior.

---

## Safety rules

- `curate` is the only agent role that can approve shared `core/` writes.
- Human Bear admins can inspect, override, or manually approve through UI, but this is a fallback rather than the default workflow.
- Non-curate roles can propose, not promote.
- Work and watch should not see raw chat/pair memory except through `core/` or approved proposals.
- Promotion should summarize and distill; do not copy raw logs into `core/`.
- Cabinet promotion requires separate Cabinet policy.
- The derived recall index is a derived view; Den/curate owns shared recall indexing.
- Non-curate roles must not independently request recall indexing of `core/` content.
- Destructive cleanup remains admin/operator action, not curate autonomy.

---

## Open questions

1. Should proposals be readable by agents through a tool/projection, or remain Den-internal records surfaced only in UI?
2. Should manual human approval and curate-agent approval use the same API path?
3. Is a `memory_promotions` row + resolved proposal sufficient provenance for MVP, or is additional outcome metadata needed?
4. What are the first allowed `core/` logical paths and section conventions?
5. What `salience` scale and cumulative-salience threshold should trigger consolidation/synthesis ([ADR-0041](../decisions/adr-0041-archival-recall-and-async-curation.md))?
6. How should Cabinet proposal permissions differ from `core` proposal permissions?
7. What bounded compaction/consolidation operations are safe enough for autonomous curate to perform without human approval?
8. What is the initial recall-scope attachment policy by role?
9. How should Cabinet Mission recall scopes be scoped and attached? A Mission is a Cabinet page and "assignment" is page membership ([Cabinet contract](../architecture/cabinet-contract.md)), so the open part is whether a recall scope follows a page's subtree and how it is filtered by inherited page policy.
10. Which `core/` records should be indexed into the recall index, and which should remain canonical-only (e.g. `scratch`, raw `log`)?

---

## Recommended next step

Use [Memory Automation Roadmap](MEMORY_AUTOMATION_ROADMAP.md) for the current next step.

The product priority remains: make curate activity visible and overrideable, while keeping routine memory curation autonomous rather than making human approval the normal path.
