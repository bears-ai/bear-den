# Memory tools implementation plan

> **Direction changed (2026-06).** All stances use Den-hosted memory tools against per-Bear SQLite ([ADR-0031](../decisions/adr-0031-sqlite-first-canonical-store-for-bear-agent-memory-and-tasks.md)); the older split between different runtime families is removed. Semantic recall is a **derived Qdrant index** over SQLite ([ADR-0038](../decisions/adr-0038-platform-embedding-standard-and-derived-recall-index.md)); harvest/consolidation/recall scoring are in [ADR-0041](../decisions/adr-0041-archival-recall-and-async-curation.md). Canonical target: [Den runtime](../architecture/den-runtime.md#memory-model-under-sqlite) ([runtime plan](DEN_RUNTIME_PLAN.md)).
>
> **Note.** Memory "files"/"paths" are logical-path projections over SQLite `memory_records`, not files in a MemFS/git branch.

For the canonical stance model and current stance names, see [bear stances](../architecture/bear-stances.md).
Status: partially implemented. P0/P1 memory tools (`memory_write_entry`, `memory_status`, `memory_browse`, `memory_read`, `memory_search`, `memory_request_review`) exist against SQLite. `chat` has read/write exposure, `curate`/`work`/`watch` have read exposure. **Open gaps:** scoped write/review policy for `work`/`watch` and harvest/consolidation automation.

Related docs:

- [Memory Automation Roadmap](MEMORY_AUTOMATION_ROADMAP.md)
- [Memory Curation Plan](MEMORY_CURATION_PLAN.md)
- [Derived recall index implementation plan](DERIVED_RECALL_INDEX_IMPLEMENTATION_PLAN.md)
- [ADR-0027 — Workflow state ontology](../decisions/adr-0027-workflow-state-ontology.md)
- [ADR-0021 — Semantic bear memory](../decisions/adr-0021-semantic-bear-memory.md)
- [ADR-0020 — Schema-first path strategy](../decisions/adr-0020-schema-first-path-strategy.md)
- [ADR-0005 — Bear memory tool boundary](../decisions/adr-0005-bear-memory-tool-boundary.md)
- [ADR-0031 — SQLite-first canonical store](../decisions/adr-0031-sqlite-first-canonical-store-for-bear-agent-memory-and-tasks.md)
- [ADR-0041 — Archival recall and asynchronous curation](../decisions/adr-0041-archival-recall-and-async-curation.md)
- [Memory model](../architecture/memory-model.md)

## Implementation status (2026-06)

| Capability | Plan section | State |
|---|---|---|
| `session_info`, `memory_write_entry`, `memory_status` | P0 | Implemented (SQLite) |
| `memory_browse`, `memory_read`, `memory_search` (`LIKE`) | P1 | Implemented (SQLite) |
| `memory_request_review` + proposals/promotions | P4 | Implemented (SQLite `memory_proposals`) |
| Exposure to `chat` profile | P2 | **Done** — descriptors + keyword-gated web tool surface |
| Exposure to `work`/`watch` profiles | P2/P3 | **Partial** — read/status/search exposed; write/review policy still open |
| Hybrid/semantic recall (Qdrant) | P5 | **Landed** — turn-start recall + hybrid `memory_search` (vector + keyword + graph + temporal) when Qdrant configured ([DERIVED_RECALL_INDEX](DERIVED_RECALL_INDEX_IMPLEMENTATION_PLAN.md)) |
| ADR-0041 schema deltas (`salience` on records, `memory_harvest_marks`, consolidation supersession writes) | Data model | **Partial** — `valid_from`/`invalid_at`, record `salience`, harvest marks, and store-level supersession invalidation landed; curate/consolidation policy still open |

Memory tools are gated by per-profile `allowed_roles` (in `den-tools` descriptors). `chat` is granted read/write memory tools; web-chat turns use a keyword-gated tool surface so casual prompts stay tool-free while memory-aware prompts unlock the full roster. Proactive key-memory projection and derived recall run on every `chat` turn (same assembler path as `pair`).

---

## Goal

Give agents, especially `pair`, safe Den-hosted access to Bear memory:

1. Know the current **situation**: role, bear, user/session identity, memory scopes, policy, and health.
2. Browse/read/search allowed Bear memory.
3. Write role-local semantic memory entries such as notes, logs, decisions, reflections, scratch, and summaries.
4. Preserve the boundary between role-local semantic memory and schema-owned artifacts such as tasks, observations, and run results.
5. Shape APIs so a future operator UI can browse, search, and inspect Bear memory without a separate implementation path.

---

## Non-goals

- Do not mirror Cabinet into Bear memory.
- Do not require role-local memories to map to Cabinet or `core/`.
- Do not let agents write arbitrary `core/` paths.
- Do not let agents choose schema-owned durable artifact paths.
- Do not let semantic-memory tools masquerade as workplan, activity, task-intent, run-result, or workplan-artifact tools.
- Do not implement destructive rollback or operator overrides as agent tools.
- Do not call the session briefing “context.”
- Do not prefix model-facing provider names with implementation ownership such as `den_`; keep ownership in canonical names and descriptor metadata.
- Do not add new tool-name aliases outside the descriptor registry/resolver.

---

## Priority: why `pair` first

`pair` was implemented first because:

- ACP `pair` runs on the native in-process runtime (ACP adapter ⇄ Den), with Den-hosted server tools.
- ACP local filesystem tools operate on the user's workspace, not Bear memory — so Bear memory needs its own Den-hosted tools.
- Pair sessions produce useful local tactical memory: coding notes, logs, decisions, debugging records, and summaries.
- Pair memory tools reuse the Den ACP server-tool path already used for `web_fetch` and `web_search`.

> **Next priority is exposure, not pair.** With pair done, the highest-value remaining work is exposing memory tools to the user-facing `chat` profile (and `work`/`watch` by policy) so bears actually present memory tools — see [Implementation status](#implementation-status-2026-06) and P2/P3 below.

---

## Tool set

### P0 — pair vertical slice

| Canonical | Provider-safe | Role | Purpose |
|---|---|---|---|
| `den.session.info` | `session_info` | `pair` first, then all roles | Return trusted briefing for the current interaction. |
| `den.memory.write_entry` | `memory_write_entry` | `pair` first | Write role-local semantic entries under `pair/`. |
| `den.memory.status` | `memory_status` | `pair` first | Return SQLite store health for the current bear/role. |

All three tools should participate in the workflow-state ontology. At minimum, descriptor metadata and tool responses should make it clear that these belong to the `memory` or `execution` domains, not `workplan` or `activity`, even though provider-safe names must stay concise and dot-free.

P0 should retire the existing `den.write_note` / `den_write_note` pair tool and replace it with `den.memory.write_entry` / `memory_write_entry`. Backward compatibility is not required.

### P1 — pair read/search/browse

| Canonical | Provider-safe | Role | Purpose |
|---|---|---|---|
| `den.memory.browse` | `memory_browse` | `pair` first | Browse allowed memory paths. |
| `den.memory.read` | `memory_read` | `pair` first | Read allowed memory files/entries. |
| `den.memory.search` | `memory_search` | `pair` first | Search allowed memory by text, role, kind, references, and lifecycle. |

For `pair`, read/search scope should include:

- `pair/` role-local memory;
- `core/` curated shared memory when available;
- no access to `chat/`, `curate/`, `work/`, or `watch/` branches.

### P2 — broaden read-only memory tools to other roles

| Role | Read/search scope |
|---|---|
| `chat` | `chat/`, `core/` |
| `curate` | all role scopes and `core/` |
| `work` | `work/`, `core/`, dispatched task context |
| `watch` | `watch/`, `core/`, delivered event/subscription context |

All roles use the same Den-hosted SQLite memory tools; there is no separate native editing path. Read/search/status tools are the only memory access for these roles and also back policy, diagnostics, and UI.

### P3 — role-local write entries for selected roles

Extend `den.memory.write_entry` beyond `pair` only after pair is stable.

| Role | Recommended write kinds | Notes |
|---|---|---|
| `chat` | `note`, `log`, `decision`, `reflection`, `scratch`, `summary` | The user-facing conversational role; enabling writes here is part of closing the exposure gap. |
| `curate` | `note`, `log`, `decision`, `reflection`, `summary` | Useful for curate notes and rejected-promotion rationale. |
| `work` | `log`, `decision`, `summary`, `scratch` | Should usually be bound to a task/run. Prefer `write_run_result` for results. |
| `watch` | `log`, `summary`, `scratch` | Prefer `write_observation` for observations. |

### P4 — governed review, promotion, and history

Future tools:

| Canonical | Provider-safe | Roles | Purpose |
|---|---|---|---|
| `den.memory.request_review` | `memory_request_review` | currently `pair` | Legacy no-hat Pair uses role-local paths; bound Pair wearing an automatic-sharing-enabled hat may request `propose_hat` by its own canonical `source_memory_id`. The request itself does not publish. |
| `den.memory.list_proposals` | `memory_list_proposals` | `curate` | List memory review proposals. |
| `den.memory.read_proposal` | `memory_read_proposal` | `curate` | Read one memory review proposal with source pointers and status. |
| `den.memory.resolve_proposal` | `memory_resolve_proposal` | `curate` | Resolve a proposal as approved, rejected, retained local, deferred, superseded, or human-review-needed. |
| ~~`den.memory.apply_core_update`~~ | ~~`memory_apply_core_update`~~ | — | **Retired:** not advertised or executable; use the verified-source/audience replacement in the [hats plan](HATS_AND_SESSION_MEMORY_BOUNDARIES_PLAN.md). |
| `den.memory.supersede_entry` | `memory_supersede_entry` | `curate` | Mark or record that source memory has been superseded by a `core`/Cabinet outcome. |
| `den.memory.history` | `memory_history` | role-scoped, curate broader | Inspect record/supersession history. |
| `den.memory.diff` | `memory_diff` | role-scoped, curate broader | Inspect diffs between record versions or proposal states. |
| `den.memory.recall` | `memory_recall` | role-scoped by recall-scope/policy | Hybrid semantic recall over the derived Qdrant index ([ADR-0038](../decisions/adr-0038-platform-embedding-standard-and-derived-recall-index.md)). May be folded into a hybrid `memory_search` instead of a separate tool. |
| `den.memory.index_curated_summary` | `memory_index_curated_summary` | `curate` / Den internal | Request recall indexing of selected curated summaries/pointers. |

`den.memory.request_review` supersedes narrower producer-side names such as `den.memory.propose_core_write` or `den.memory.propose_core_update`. `propose_hat` intake records a Den-verified source/hat link in SQLite; tool-free Curate may rewrite a verified note into that opted-in hat for all authorized wearers, including eligible Job runs. Missing keys or synthesis failures leave the proposal private with durable delayed recovery; disabled-sharing and authorization denials do not retry. A bounded sweep can recover eligible pending proposals without a queued successor ([hats plan](HATS_AND_SESSION_MEMORY_BOUNDARIES_PLAN.md)). No path or model-supplied JSON grants publication.

### P5 — Derived recall (Qdrant)

Use the Den-owned **derived Qdrant recall index** ([ADR-0038](../decisions/adr-0038-platform-embedding-standard-and-derived-recall-index.md)) over canonical SQLite; do not store vectors in SQLite or bear packages.

Planned behavior:

- One unified recall collection per active `embedding_standard` (`bears-embed-v1`), with payload filters for Bear and Cabinet scopes.
- Add Cabinet Mission recall scopes once Cabinet page policy/membership lands (Cabinet Phase 2): a Mission is a page and assignment is page membership, so a scope follows the page's subtree.
- Recall scopes are policy-attached to role agents (ACL by membership + identity scope, [ADR-0015](../decisions/adr-0015-multi-user-memory.md)).
- Den/curate owns indexing via the `archive_index` reflection lane; role agents do not maintain the shared index.
- `memory_search` becomes hybrid (vector when Qdrant configured, else `LIKE`), ranked by `recency × relevance × importance`, degrading gracefully ([ADR-0041](../decisions/adr-0041-archival-recall-and-async-curation.md)).
- Passage registry (Den Postgres) stores canonical IDs, `content_hash`, chunk bounds, `indexed_at`, and supersession/delete state; vectors live in Qdrant and are rebuildable from canonical SQLite. See [Derived recall index implementation plan](DERIVED_RECALL_INDEX_IMPLEMENTATION_PLAN.md).

---

## Data model: memory entry

`den.memory.write_entry` should accept a semantic entry payload rather than arbitrary paths.

Initial input schema:

```json
{
  "type": "object",
  "properties": {
    "kind": {
      "type": "string",
      "enum": ["note", "log", "decision", "reflection", "scratch", "summary"]
    },
    "title": { "type": "string", "minLength": 1, "maxLength": 200 },
    "body": { "type": "string", "minLength": 1, "maxLength": 50000 },
    "tags": {
      "type": "array",
      "items": { "type": "string", "minLength": 1, "maxLength": 80 },
      "maxItems": 20
    },
    "refs": {
      "type": "object",
      "properties": {
        "people": { "type": "array", "items": { "type": "string" }, "maxItems": 20 },
        "domains": { "type": "array", "items": { "type": "string" }, "maxItems": 20 },
        "missions": { "type": "array", "items": { "type": "string" }, "maxItems": 20 },
        "knowledge": { "type": "array", "items": { "type": "string" }, "maxItems": 20 },
        "cabinet": { "type": "array", "items": { "type": "string" }, "maxItems": 20 },
        "artifacts": { "type": "array", "items": { "type": "string" }, "maxItems": 20 },
        "tasks": { "type": "array", "items": { "type": "string" }, "maxItems": 20 }
      },
      "additionalProperties": false
    },
    "lifecycle": {
      "type": "object",
      "properties": {
        "scope": { "type": "string", "enum": ["role-local", "core-candidate", "cabinet-candidate"] },
        "retention": { "type": "string", "enum": ["session", "short", "durable", "archive"] },
        "promotion": { "type": "string", "enum": ["none", "maybe", "proposed"] },
        "status": { "type": "string", "enum": ["active", "superseded", "stale", "archived"] }
      },
      "additionalProperties": false
    },
    "source": { "type": "object" }
  },
  "required": ["kind", "title", "body"],
  "additionalProperties": false
}
```

Defaults:

- `lifecycle.scope`: `role-local`
- `lifecycle.retention`: `durable` for `note`, `decision`, `summary`, `reflection`; `short` for `scratch`; `archive` or `durable` for `log` depending on role policy
- `lifecycle.promotion`: `none`
- `lifecycle.status`: `active`

> **SQLite mapping.** This payload is stored as a `memory_records` row (`den-memory`): `kind`, `content_text` (title + body), `metadata_json` (tags, refs, lifecycle, source), `scope_type`/`scope_profile` (role-local vs shared), `logical_path` (projection), `author_profile`. Per [ADR-0041](../decisions/adr-0041-archival-recall-and-async-curation.md) the entry also carries `salience` (drives reflection triggering and recall ranking); `valid_from`/`invalid_at` and `supersedes_memory_id` express lifecycle transitions instead of `lifecycle.status` mutation.

### Ontology-aware validation

To support the workflow-state model, `den.memory.write_entry` should also reject content that is structurally in the wrong domain, even when the title/body are superficially valid text. In particular, persisted workplan artifacts such as `pair/plans/...` are not semantic-memory entries and must not be written as `memory_records`.

Examples to reject or redirect:

- active implementation plans;
- live task lists intended for current work tracking;
- task intents;
- run results;
- observations;
- direct `core/` updates;
- Cabinet writes.

A lightweight structured content/domain class is preferred over verbose human-facing names. This is more natural for model/tooling boundaries and avoids overburdening users with long labels while still preserving provider-safe naming constraints.

---

## Entry shape

Memory entries are `memory_records` rows: structured fields plus `content_text` and `metadata_json` ([ADR-0031](../decisions/adr-0031-sqlite-first-canonical-store-for-bear-agent-memory-and-tasks.md)). The YAML-frontmatter view below is an illustrative *logical* projection (e.g. for `memory_read` rendering and UI); it is not a file on disk. Provenance/lifecycle fields live in `metadata_json`, not a file header.

Example (logical view):

```text
---
entry_id: "mem-20260507T182233Z-a1b2c3"
kind: "decision"
title: "Use Den-hosted memory tools for ACP pair"
role: "pair"
bear_id: "..."
created_at: "2026-05-07T18:22:33Z"
author: "alice"
source_role_agent_id: "agent-..."
source_conversation_id: "conv-..."
source_acp_session_id: "..."
tags:
  - "acp"
  - "memory-tools"
refs:
  missions:
    - "mission:bears"
lifecycle:
  scope: "role-local"
  retention: "durable"
  promotion: "none"
  status: "active"
---

# Use Den-hosted memory tools for ACP pair

Pair runs on the native runtime with Den-hosted server tools, so Den-hosted SQLite memory tools are the right first path.
```

Structured fields are stored as columns; tags/refs/lifecycle/source live in `metadata_json`. Search/filter can begin with `logical_path` + `content_text` (SQL `LIKE` today) and later add `metadata_json` filters and hybrid vector recall ([ADR-0038](../decisions/adr-0038-platform-embedding-standard-and-derived-recall-index.md)).

---

## Path conventions

Den derives the `logical_path` projection. Agents do not provide arbitrary paths to `den.memory.write_entry`.

Recommended path format:

```text
<role>/<kind-directory>/<entry-id>.md
```

Directory mapping:

| Kind | Directory |
|---|---|
| `note` | `notes` |
| `log` | `logs` |
| `decision` | `decisions` |
| `reflection` | `reflections` |
| `scratch` | `scratch` |
| `summary` | `summaries` |

Example paths:

```text
pair/notes/mem-20260507T182233Z-a1b2c3.md
pair/logs/mem-20260507T182300Z-d4e5f6.md
pair/decisions/mem-20260507T182355Z-a9b8c7.md
```

Do not use title-derived slugs as identity. Optional display slugs can be added later only if collision-safe IDs remain present and sensitive-title leakage is reviewed.

---

## Store: per-Bear SQLite (`den-memory`)

Memory tools write/read the per-Bear SQLite store, not a MemFS sidecar. (The legacy MemFS Manager pair-note endpoint is retired; do not keep model-visible compatibility aliases.)

### Store

- One SQLite DB per Bear under `BEAR_SQLITE_DATA_DIR` (`{bear_id}.sqlite`), managed by `MemoryStoreManager` (`den-memory`), WAL, single writer per Bear.
- Append-only `memory_records` with monotonic `sequence_no`; lifecycle via `supersedes_memory_id` (not destructive edits).
- Tools reach the store through `DenRoleMemoryStore` in the Den tool dispatcher.

### Optional management endpoints (UI/ops, not agent-facing)

UI/diagnostics can read the same store via management endpoints, for example:

```text
POST /v1/management/bears/{bear_id}/roles/{role}/memory-entries
GET  /v1/management/bears/{bear_id}/roles/{role}/memory-status
GET  /v1/management/bears/{bear_id}/roles/{role}/memory-browse
GET  /v1/management/bears/{bear_id}/roles/{role}/memory-records/{memory_id}
GET  /v1/management/bears/{bear_id}/roles/{role}/memory-search
```

These are conveniences over the same SQLite store; agents always go through Den tools.

### Logical-path conventions

There are no real directories; `logical_path` is a projection over records (`scope` + `kind` + work-surface). Conventional kind directories used in the projection:

```text
{profile}/notes/   {profile}/logs/   {profile}/decisions/
{profile}/reflections/   {profile}/scratch/   {profile}/summaries/
```

Schema-owned artifacts (tasks/results/observations) are not semantic-memory kinds and use their own stores (Docket/observations/Garage), not `memory_write_entry`.

---

## Den changes

### Tool descriptors

Built-in tool descriptors (`den-tools`), with provider-safe model-facing names:

| Canonical | Provider-safe |
|---|---|
| `den.session.info` | `session_info` |
| `den.memory.write_entry` | `memory_write_entry` |
| `den.memory.status` | `memory_status` |
| `den.memory.browse` | `memory_browse` |
| `den.memory.read` | `memory_read` |
| `den.memory.search` | `memory_search` |

Descriptors carry `allowed_roles`; **exposure is governed there**. The current gap is that `chat` is excluded — extend `allowed_roles` (and the per-profile roster assembly) to expose the appropriate read/write subset to user-facing profiles.

### Tool invocation

Den's tool dispatcher:

- `den.session.info` returns trusted invocation context, memory scopes, relevant policy, and health summary. Its `memory.available_tools` must reflect the role's real roster (today it is hardcoded — fix as part of exposure).
- `den.memory.write_entry` validates role, kind, lifecycle, refs, tags, and body limits; appends a `memory_records` row; returns `memory_id`, `logical_path`, and status.
- `den.memory.status` reports SQLite store health for the current Bear/role.
- `den.memory.browse/read/search` read the store with role-aware scope checks.

### ACP pair integration

ACP pair descriptors:

- Expose client/local tools filtered by adapter capabilities.
- Expose `web_fetch` and `web_search`.
- `den_write_note` is retired in favor of `memory_write_entry`.
- Expose `session_info`, `memory_write_entry`, `memory_status`, `memory_browse`, `memory_read`, `memory_search`, `memory_request_review` (the pair ACP surface).

Prompt guidance should say:

- Use `session_info` when you need trusted information about the current interaction, role, user, memory scopes, or policy.
- Use `memory_write_entry` for durable pair-local notes, logs, tactical decisions, reflections, scratch, and summaries.
- Do not use `memory_write_entry` for task intents, observations, run results, `core/` updates, or Cabinet writes.

---

## Authorization and policy

### Pair P0 policy

For `pair`:

- `den.session.info`: allow for authenticated ACP session with bear membership.
- `den.memory.write_entry`: allow only role `pair` initially.
- `den.memory.status`: allow role `pair` for current bear/role.

Write constraints:

- allowed role path: `pair/` only;
- allowed kinds: `note`, `log`, `decision`, `reflection`, `scratch`, `summary`;
- denied schema paths: `pair/tasks/` and anything under `core/`;
- no arbitrary path argument;
- max body size initially 50 KiB;
- max tags 20;
- max refs per ref kind 20;
- source object bounded or truncated/redacted.

### Future policy

- `curate` can read all branches and eventually approve/reject promotions.
- `work` writes should be task/run-bound.
- `watch` writes should prefer `write_observation` for observations.
- Person references require privacy review before adding person-specific memory write semantics.

---

## UI requirements to preserve during API design

The future operator UI should be able to use the same Den-side memory APIs or closely related internal APIs.

Design responses to support:

- bear-level memory overview;
- per-role browse;
- path tree;
- kind filters;
- lifecycle filters;
- semantic reference filters;
- full entry display;
- frontmatter/provenance inspection;
- record id / content-hash display;
- SQLite store health indicators;
- Cabinet links when present;
- clear display for memories with no Cabinet mapping.

Do not design agent-only payloads that the UI cannot reuse.

---

## Implementation slices

> **Status.** Slices 0–4 and most of slice 7 are implemented against SQLite (see [Implementation status](#implementation-status-2026-06)). The active slices are **exposure** (broaden `allowed_roles` beyond pair/curate, fix `session_info.memory`), **recall** (slice 8), and the ADR-0041 schema deltas. The slice text below is retained for the original sequencing rationale; read "MemFS Manager endpoint" as "SQLite store (`den-memory`)".

### Slice 0 — align docs and names

Deliverables:

1. Confirm canonical/provider-safe tool names.
2. Confirm memory entry schema and path conventions.
3. Add this plan to docs index.
4. Add tests/docs references confirming `den_write_note` is retired from model-visible descriptors once `memory_write_entry` is available.

### Slice 1 — pair `den.session.info`

Goal: first safe read-only vertical slice.

Deliverables:

1. Den descriptor for `den.session.info`.
2. ACP pair exposure as `session_info`.
3. Dispatcher implementation from trusted invocation context.
4. Include allowed memory scopes and available memory tools.
5. Include SQLite store availability if cheap; otherwise return unknown with diagnostic.
6. Tests for pair descriptor visibility and no arbitrary identity inputs.

### Slice 2 — pair `den.memory.write_entry`

Goal: generalize current pair note writing.

Deliverables:

1. MemFS Manager endpoint for role memory entries, initially allowing only pair.
2. Den descriptor and dispatcher implementation.
3. Validation of kind, lifecycle, refs, tags, size, and role.
4. Path generation based on role + kind + entry id.
5. Markdown/frontmatter writer.
6. Remove `den_write_note` provider/canonical mapping from ACP pair exposure and prefer `memory_write_entry` for all role-local entries, including notes.
7. Tests for allowed kinds, denied role, denied arbitrary path, and generated path.
8. ACP pair prompt guidance update.

### Slice 3 — pair memory status

Goal: visible health for pair memory and future UI.

Deliverables:

1. MemFS Manager role health endpoint or reuse existing management health.
2. Den `den.memory.status` descriptor and dispatcher.
3. ACP exposure as `memory_status`.
4. Return store availability, latest `sequence_no`, record counts by scope, and diagnostic.
5. Tests for available/degraded SQLite store behavior.

### Slice 4 — pair read/tree/search

Goal: let pair inspect Bear memory, not just write it.

Deliverables:

1. MemFS Manager tree/read/search endpoints with path and size bounds.
2. Den descriptors and dispatchers for `den.memory.browse`, `den.memory.read`, `den.memory.search`.
3. Role scope enforcement: pair can read `pair/` and `core/` only.
4. Search supports text query first; kind/ref/lifecycle filters can initially be best-effort or deferred until frontmatter parsing exists.
5. Tests for cross-role denial and bounded output.

### Slice 5 — broaden read-only tools

Goal: make read/tree/search/status available beyond pair.

Deliverables:

1. Role matrix implementation (extend `allowed_roles`; fix `session_info.memory`).
2. Curate can read all role scopes.
3. Chat/work/watch scopes enforced.
4. Tests by role.

### Slice 8 — derived recall

Goal: hybrid semantic recall over canonical SQLite.

Deliverables:

1. Qdrant collection + Postgres passage registry ([ADR-0038](../decisions/adr-0038-platform-embedding-standard-and-derived-recall-index.md)).
2. `archive_index` reflection lane to index canonical heads; rebuild-on-import.
3. Hybrid `memory_search` (vector when configured, else `LIKE`) scored `recency × relevance × importance` ([ADR-0041](../decisions/adr-0041-archival-recall-and-async-curation.md)); graceful degradation.
4. ACL/identity-scoped recall ([ADR-0015](../decisions/adr-0015-multi-user-memory.md)).
5. Tests for filter scoping, dedup, and degradation. See [Derived recall index implementation plan](DERIVED_RECALL_INDEX_IMPLEMENTATION_PLAN.md).

### Slice 6 — broaden write entries selectively

Goal: add role-local semantic entries for roles where Den adds value.

Deliverables:

1. Enable `chat`, `curate`, `work`, and/or `watch` according to policy.
2. Work writes require task/run binding when relevant.
3. Watch observations remain behind `write_observation`; generic entries avoid `kind: observation` initially.
4. Tests by role and kind.

### Slice 7 — review/promotion/history tools

Goal: implement governed memory lifecycle in a Reflection-compatible way.

Deliverables:

1. `den.memory.request_review` for producer roles, starting with `pair`.
2. Review proposal list/read.
3. Review proposal resolution through `den.memory.resolve_proposal`.
4. **Revised:** verified-source/audience hat promotion and separate hat→`core/` publication, not the retired proposal-based `den.memory.apply_core_update` path.
5. History/diff APIs.
6. UI-ready audit trail.

---

## Testing strategy

### Unit tests

- Tool descriptor visibility by role.
- Provider-safe name mapping.
- Input schema validation.
- Path generation by role/kind.
- Frontmatter escaping/serialization.
- Role scope enforcement.

### Integration tests

- Den `den.memory.write_entry` appends a `memory_records` row in per-Bear SQLite.
- ACP pair sees and can call memory tools; the exposed profile roster matches `allowed_roles`.
- `den_write_note` is no longer advertised.
- Cross-role reads/writes denied.
- SQLite store unavailable/unwritable returns a clear `memory_status` degraded result; turns do not fail.

### Smoke tests

Add or extend stack smoke coverage for:

1. pair situation briefing;
2. pair write note/log/decision entry;
3. pair memory status;
4. pair memory read/search once implemented.

---

## Open questions

1. Should P0 expose `den.memory.status`, or should status be included only in `den.session.info` until P1?
2. Should role-local entry writes use only opaque IDs, or include optional safe display slugs?
3. How much frontmatter parsing should P1 search implement versus deferring to UI work?
4. Which read/write subset should `chat` (the user-facing profile) get, given there is no separate native editing path? (This is the active exposure question.)
5. Should `scratch` entries have automatic retention/cleanup, or just lifecycle metadata at first?
6. Should person references be accepted as opaque strings in P0, or deferred until privacy policy is designed?

---

## Recommended next implementation target

The pair vertical slice (`session_info`, `memory_write_entry`, `memory_status`, read/search) is implemented. The next targets, in order:

1. **Scoped write/review policy** — decide whether `work`/`watch` get `memory_write_entry` and/or `memory_request_review`, and keep schema-owned observations/results out of generic memory writes.
2. **Consolidation policy** — use the landed ADR-0041 store primitives (`salience`, harvest marks, supersession + `invalid_at`) from `curate`/consolidation flows.
3. **Entity tools/anchors** — expose entity browse/resolve and promote resolved salient entities into anchors.

This closes the perceived "no memory tools" gap first, then makes memory semantically recallable.
