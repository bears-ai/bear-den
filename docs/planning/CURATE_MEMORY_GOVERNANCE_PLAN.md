# Curate memory governance plan

Status: proposed design plan.

This plan designs how memories move between role-local branches and shared Bear memory. It focuses on the `memory_curate` lane of BEARS **Reflection** system and the `curate` role as the only role allowed to integrate role-local memory into shared `core/` memory or propose/promote Cabinet updates.

Related docs:

- [Reflection system implementation plan](REFLECTION_SYSTEM_PLAN.md)
- [Memory tools implementation plan](MEMORY_TOOLS_IMPLEMENTATION_PLAN.md)
- [Reflection System ADR](../architecture/adr/reflection-system.md)
- [Semantic Bear Memory ADR](../architecture/adr/semantic-bear-memory.md)
- [MemFS Sidecar Repo Views ADR](../architecture/adr/memfs-sidecar-repo-views.md)
- [Multi-agent architecture ADR](../architecture/adr/multi-agent-architecture.md)
- [Semantic memory context](../context/SEMANTIC_MEMORY.md)
- [Memory model](../concepts/MEMORY_MODEL.md)

---

## Goal

Give BEARS a governed, inspectable mechanism for memory movement:

1. Role-local memories can remain local forever.
2. Role-local memories can be proposed for promotion when useful beyond one role.
3. `curate` can review all role branches and decide what to do.
4. Approved durable shared knowledge is written to `core/`.
5. Cabinet-worthy knowledge is proposed or written through Cabinet-specific workflows, not silently copied from MemFS.
6. Every movement records provenance and leaves an audit trail.
7. Letta Archives are used as derived semantic retrieval indexes; BEARS does not introduce a separate vector store.
8. Curation runs as a bounded Reflection lane that can be triggered by heartbeat, proposals, memory writes, or manual request.
9. Heartbeat cadence is throttled: active Bears can run memory curation more frequently than dormant Bears.

---

## Non-goals

- Do not automatically promote all role memories.
- Do not let `talk`, `pair`, `work`, or `watch` write directly to `core/`.
- Do not let `work` read channel branches or watch branches.
- Do not require promoted memories to have Cabinet objects.
- Do not treat Cabinet as a mirror of Bear memory.
- Do not allow agents to run destructive Git resets or MemFS operator overrides.
- Do not let every role independently archive `core/` content.
- Do not make Letta Archives the source of truth.

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
- a future Reflection heartbeat or event-triggered curation run surfaces it.

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

## Role responsibilities

| Role | Memory responsibility |
|---|---|
| `talk` | Writes conversational role-local memory and may propose durable shared updates. |
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
| `den.memory.tree` | `memory_tree` | Browse all role branches and `core/`. |
| `den.memory.read` | `memory_read` | Read memory files from any branch. |
| `den.memory.search` | `memory_search` | Search path and content across all branches. |
| `den.memory.status` | `memory_status` | Inspect memory health/status across roles. |
| `den.memory.history` | `memory_history` | Inspect file/commit history. |
| `den.memory.diff` | `memory_diff` | Inspect proposed or committed changes. |

### Proposal tools for non-curate roles

Non-curate roles should not write `core/` or Cabinet directly. They can request review of role-local memory.

| Canonical | Provider-safe | Roles | Purpose |
|---|---|---|---|
| `den.memory.request_review` | `memory_request_review` | `talk`, `pair`, `work`, `watch` | Request curation of role-local memory without choosing the final outcome. |

`den.memory.request_review` supersedes narrower producer-side names such as `den.memory.propose_core_update`, `den.memory.propose_core_write`, and `den.memory.propose_cabinet_update`. The request may include a `suggested_action`, such as `summarize_into_core`, `promote_to_core`, `cabinet_update`, `skill_review`, `retain_role_local`, `delete_after_review`, `human_review`, or `unspecified`.

Review requests should reference source memory paths rather than embedding all source content.

Initial `den.memory.request_review` input shape:

- `source_paths`: role-local memory paths owned by the caller role;
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
| `den.memory.apply_core_update` | `memory_apply_core_update` | Apply a reviewed shared memory update into `core/` with provenance. |
| `den.memory.supersede_entry` | `memory_supersede_entry` | Mark or record that a role-local entry has been superseded by a core/Cabinet entry. |

### Cabinet tools

Cabinet should remain a separate capability surface.

Candidate future tools:

| Canonical | Provider-safe | Purpose |
|---|---|---|
| `cabinet.propose_update` | `cabinet_propose_update` | Create a Cabinet update proposal. |
| `cabinet.create_or_update` | `cabinet_create_or_update` | Curate/human-approved Cabinet write. |
| `cabinet.link_memory` | `cabinet_link_memory` | Record a link from memory entry to Cabinet object. |

---

## Proposal storage

Use Den DB as the source of truth for proposal lifecycle.

Rationale:

- easier UI queries;
- explicit review status;
- avoids hidden state in Markdown frontmatter;
- easier authorization/audit;
- can link to multiple source files and targets;
- can still mirror summary files into MemFS later if useful to agents.

Suggested table: `bear_memory_proposals`.

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

## Letta Archives integration

Letta Archives should be used for semantic retrieval, not canonical storage. This follows Letta's recommended pattern: keep external systems canonical and treat archival memory as a derived retrieval index.

### Canonical ownership

Canonical stores own IDs, versions, ACLs, deletes, and full content:

| Canonical source | Store |
|---|---|
| Shared Bear orientation | `core/` MemFS |
| Role-local memory | role MemFS branches |
| Human-facing shared knowledge | Cabinet |
| Workflow state | Den DB and schema artifacts |
| Files/results | Garage artifacts |

Archive passages should usually be summaries or pointers. On retrieval hit, tools should fetch the canonical object by ID/path when exact truth matters.

### Archive types

| Archive | Purpose | Writers | Readers |
|---|---|---|---|
| Bear curated archive | Shared semantic recall over selected `core/` summaries, approved proposals, durable references. | Den/curate indexer | Attached role agents by policy |
| Cabinet Mission archive | Optional semantic recall for a Cabinet Mission, shared by assigned Bears/roles when needed. | Den/curate indexer | Bears/roles assigned to that Cabinet Mission |
| Role-local archive | Optional later role-specific long-tail recall. | Role or Den by policy | Owning role |

Prefer Cabinet Mission archives over a broad generic “technical” archive when knowledge needs to be shared across Bears for a Cabinet Mission. For Bear-scoped knowledge, the Bear curated archive is usually sufficient.

### Passage provenance

Archive passage `metadata` should include:

- `canonical_id`
- `source_uri`
- `source_path`
- `source_role`
- `version` or source commit
- `hash`
- `updated_at`
- `indexed_by`
- `index_kind`

Tags should be coarse query filters, for example:

- `bear:{bear_id}`
- `source:core`
- `mission:{mission_id}`
- `kind:decision`
- `role:curate`

### Index mapping

Letta passage create has no first-class external id/idempotency key and archive passages have no update endpoint. Den should maintain a source-to-passage mapping table for indexed material.

Suggested table: `bear_archive_index_entries`.

Fields:

- `id uuid primary key`
- `bear_id uuid not null`
- `archive_id text not null`
- `passage_id text not null`
- `canonical_kind text not null`
  - `core_memory`, `role_memory`, `cabinet`, `artifact_summary`, `proposal_summary`
- `canonical_id text not null`
- `source_uri text not null`
- `source_path text null`
- `source_role text null`
- `source_version text not null`
- `source_hash text not null`
- `chunk_key text not null`
- `chunk_index integer not null default 0`
- `text_hash text not null`
- `metadata jsonb not null default '{}'`
- `tags text[] not null default '{}'`
- `indexed_at timestamptz not null default now()`
- `deleted_at timestamptz null`

Recommended uniqueness:

```text
unique (bear_id, archive_id, canonical_id, chunk_key)
```

Sync behavior:

1. If source hash is unchanged, do nothing.
2. If source hash changed, delete the old passage and create a new passage.
3. If canonical source is deleted, delete the passage and mark the index row deleted.
4. Search results should verify canonical id/hash where strict correctness matters.

### Write boundary

Agents may search attached archives, but shared archives should not be collaboratively maintained by every agent. Shared archive writes should go through Den/curate indexing workflows using `/v1/archives/{archive_id}/passages` rather than agent-scoped `archival_memory_insert`.

`pair` can contribute technical notes to role-local memory and request curation. `curate` decides whether to index a summary into the Bear curated archive, a Cabinet Mission archive, Cabinet, or `core/`.

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

Every core write should include frontmatter or inline provenance:

- proposal id;
- source paths;
- source roles;
- reviewer role/agent;
- timestamp;
- rationale;
- source commit(s) when available.

---

## Lifecycle state changes for source entries

Promotion should not necessarily delete source memories.

When a proposal is approved, Den should optionally update source entry metadata or write a small sidecar marker indicating:

- `promotion: approved`
- `status: superseded` or `active`
- `promoted_to: core/...` or Cabinet ref
- `proposal_id`
- `result_commit`

Do not require this for MVP if editing source frontmatter is too expensive. A DB proposal record with `source_paths` is enough for the first slice.

---

## Curate cycle design

Curate is expected to be autonomous. Human intervention is a last resort for sensitive, ambiguous, or policy-blocked cases.

Curate can run as:

1. **Scheduled curation cycle** controlled by Den.
2. **Event-triggered review** after enough proposals or memory activity accumulates.
3. **Manual/human-triggered review** from Den UI as an override or debugging mechanism.

Initial implementation should support autonomous review first, with human review as an escalation path rather than the default path.

A curate cycle prompt should include:

- Bear identity and purpose;
- role, policy, and relevant Domains;
- pending proposals;
- recent role-local memory activity;
- search/read tools;
- semantic search tools over attached Letta Archives when available;
- explicit instruction that role-local memory can remain local;
- explicit instruction to prefer concise `core/` summaries over copying raw logs;
- explicit instruction that archive passages are derived indexes and should point back to canonical sources.

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

Human admins should be able to inspect, override, and manually review, but manual review is an operations fallback. The primary product posture is that the Bear can maintain its own memory through the curate role.

---

## First implementation slices

### Slice 1 — DB-backed proposal/review queue

Deliverables:

1. `bear_memory_proposals` migration.
2. Den DB functions for create/list/get/update status.
3. UI action to create proposal from selected memory files.
4. Curation queue page.
5. Curate-visible proposal listing so autonomous curation can operate on the queue.

### Slice 2 — Pair requests memory review

Deliverables:

1. `den.memory.request_review` descriptor for `pair`.
2. Tool implementation writes proposal rows, not `core` or Cabinet.
3. ACP pair prompt guidance: request curation for cross-role or durable shared candidates; do not write core.
4. Tests for proposal creation and role authorization.

### Slice 3 — Curate read/review tools

Deliverables:

1. Expose read/tree/search/status across all branches to `curate`.
2. `den.memory.list_proposals` for `curate`.
3. `den.memory.read_proposal` for `curate`.
4. Tests for curate visibility and non-curate denial.

### Slice 4 — Curate proposal resolution and core write tools

Deliverables:

1. `den.memory.resolve_proposal` for `curate`.
2. `den.memory.apply_core_update` or equivalent constrained core-write tools for `curate`.
3. `den.memory.compact_core` or equivalent bounded core-cleanup tool.
4. Den writes normal Git commits to policy-approved `core/` paths.
5. Registered views are reconciled/reset as needed.
6. Proposal records result path/commit.
7. Tests for path policy and provenance.

### Slice 5 — Curate cycle runner and activity surfacing

Deliverables:

1. Curate cycle runner invokes Letta API-direct curate agent with pending proposals and recent memory activity.
2. Guardrails: no external tools, no arbitrary paths, no Cabinet writes unless explicitly granted.
3. Den records curation cycle activity: inputs considered, decisions made, proposals approved/rejected, core files changed, compactions performed, and escalations.
4. UI surfaces the extent of curate activity so humans can understand what the god-agent has been doing without approving every action.

### Slice 6 — Letta Archives indexing

Deliverables:

1. Create/provision Bear curated archives in Letta.
2. Attach Bear curated archives to selected role agents by policy.
3. Add `bear_archive_index_entries` or equivalent source-to-passage mapping.
4. Add Den indexer operations for selected `core/` summaries and approved proposal outcomes.
5. Add `den.memory.semantic_search` backed by Letta passage search / attached archives.
6. Add Cabinet Mission archive design hooks once Cabinet Missions and Bear↔Mission assignments are defined.

### Slice 7 — Cabinet proposal integration

Deliverables:

1. `den.memory.request_review` supports `suggested_action: cabinet_update` and Cabinet target hints.
2. Cabinet UI/tooling can accept, edit, or reject proposed updates.
3. Link approved Cabinet entries back to source memory proposals and derived archive passages where applicable.

---

## Safety rules

- `curate` is the only agent role that can approve shared `core/` writes.
- Human Bear admins can inspect, override, or manually approve through UI, but this is a fallback rather than the default workflow.
- Non-curate roles can propose, not promote.
- Work and watch should not see raw talk/pair memory except through `core/` or approved proposals.
- Promotion should summarize and distill; do not copy raw logs into `core/`.
- Cabinet promotion requires separate Cabinet policy.
- Letta Archives are derived indexes; Den/curate owns shared archive indexing.
- Non-curate roles must not independently archive `core/` content.
- Destructive cleanup remains admin/operator action, not curate autonomy.

---

## Open questions

1. Should proposals live only in Den DB, or also be mirrored into `curate/proposals/` for agent visibility?
2. Should manual human approval and curate-agent approval use the same API path?
3. Should source entry frontmatter be updated after approval, or is DB provenance sufficient for MVP?
4. What are the first allowed `core/` paths and section conventions?
5. Should `core/` writes live on the `curate` branch, a separate `core` branch, or a sidecar-managed projection into all role views?
6. How should Cabinet proposal permissions differ from `core` proposal permissions?
7. What bounded compaction/dreaming operations are safe enough for autonomous curate to perform without human approval?
8. What is the initial Bear archive attachment policy by role?
9. How should Cabinet Mission archives be scoped and attached once Cabinet Missions and Bear↔Mission assignments are defined?
10. Which `core/` sections should be indexed into Archives as summaries/pointers, and which should remain only in MemFS?

---

## Recommended next step

Build **Slice 1: DB-backed proposal/review queue** and add both UI and agent-tool ways to mark selected memories for curation.

Then prioritize the autonomous curate loop and core-cleaning tools before broad manual approval UX. Human UI should make curate's activity visible and overrideable, not make human approval the normal path.
