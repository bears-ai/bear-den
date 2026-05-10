# Memory tools implementation plan

Status: proposed implementation plan.

This plan implements Den-hosted memory tools for BEARS agents, prioritizing the ACP/API-direct `pair` role.

Related docs:

- [Semantic Bear Memory ADR](../architecture/adr/semantic-bear-memory.md)
- [Schema-first Den-generated path strategy ADR](../architecture/adr/schema-first-path-strategy.md)
- [Bear Memory Tool Boundary ADR](../architecture/adr/bear-memory-tool-boundary.md)
- [MemFS Sidecar Repo Views ADR](../architecture/adr/memfs-sidecar-repo-views.md)
- [Semantic memory context](../context/SEMANTIC_MEMORY.md)
- [Memory model](../concepts/MEMORY_MODEL.md)
- [ACP direct local tool runtime](ACP_DIRECT_LOCAL_TOOL_RUNTIME_PLAN.md)
- [Den-specific bear tools implementation plan](DEN_SPECIFIC_TOOLS_PLAN.md)

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
- Do not replace Letta Code-native MemFS tools for harness-backed roles where native tools are the better low-latency path.
- Do not implement destructive rollback or MemFS operator overrides as agent tools.
- Do not call the situation briefing “context.”

---

## Priority: why `pair` first

`pair` should be first because:

- ACP `pair` is API-direct: ACP adapter ⇄ Den ⇄ Letta API.
- It does not run through Codepool / Letta Code, so native Letta Code MemFS tools are not the natural path.
- ACP local filesystem tools operate on the user's workspace, not Bear memory.
- Pair sessions produce useful local tactical memory: coding notes, logs, decisions, debugging records, and summaries.
- Pair memory tools can reuse the existing Den ACP server-tool path used for Den-hosted pair tools such as `web_fetch` and `web_search`.

---

## Tool set

### P0 — pair vertical slice

| Canonical | Provider-safe | Role | Purpose |
|---|---|---|---|
| `den.session.info` | `session_info` | `pair` first, then all roles | Return trusted briefing for the current interaction. |
| `den.memory.write_entry` | `memory_write_entry` | `pair` first | Write role-local semantic entries under `pair/`. |
| `den.memory.status` | `memory_status` | `pair` first | Return MemFS health for the current bear/role. |

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
- no access to `talk/`, `curate/`, `work/`, or `watch/` branches.

### P2 — broaden read-only memory tools to other roles

| Role | Read/search scope |
|---|---|
| `talk` | `talk/`, `core/` |
| `curate` | all role branches and `core/` |
| `work` | `work/`, `core/`, dispatched task context |
| `watch` | `watch/`, `core/`, delivered event/subscription context |

`talk` and `work` may still use native Letta Code memory tools for ordinary MemFS editing. Den read/search/status APIs remain useful for policy, diagnostics, and UI.

### P3 — role-local write entries for selected roles

Extend `den.memory.write_entry` beyond `pair` only after pair is stable.

| Role | Recommended write kinds | Notes |
|---|---|---|
| `talk` | `note`, `log`, `decision`, `reflection`, `scratch`, `summary` | Avoid replacing native Letta Code memory tools unless Den adds audit/policy value. |
| `curate` | `note`, `log`, `decision`, `reflection`, `summary` | Useful for review notes and rejected-promotion rationale. |
| `work` | `log`, `decision`, `summary`, `scratch` | Should usually be bound to a task/run. Prefer `write_run_result` for results. |
| `watch` | `log`, `summary`, `scratch` | Prefer `write_observation` for observations. |

### P4 — governed review, promotion, and history

Future tools:

| Canonical | Provider-safe | Roles | Purpose |
|---|---|---|---|
| `den.memory.request_review` | `memory_request_review` | `talk`, `pair`, `work`, `watch` | Request curation of role-local memory without choosing the final outcome. |
| `den.memory.list_proposals` | `memory_list_proposals` | `curate` | List memory review proposals. |
| `den.memory.read_proposal` | `memory_read_proposal` | `curate` | Read one memory review proposal with source pointers and status. |
| `den.memory.resolve_proposal` | `memory_resolve_proposal` | `curate` | Resolve a proposal as approved, rejected, retained local, deferred, superseded, or human-review-needed. |
| `den.memory.apply_core_update` | `memory_apply_core_update` | `curate` | Apply a reviewed `core/` update with provenance. |
| `den.memory.supersede_entry` | `memory_supersede_entry` | `curate` | Mark or record that source memory has been superseded by a `core`/Cabinet outcome. |
| `den.memory.history` | `memory_history` | role-scoped, curate broader | Inspect commit/file history. |
| `den.memory.diff` | `memory_diff` | role-scoped, curate broader | Inspect diffs between commits or proposal states. |
| `den.memory.semantic_search` | `memory_semantic_search` | role-scoped by archive attachment/policy | Search Letta Archives as derived semantic indexes. |
| `den.memory.index_curated_summary` | `memory_index_curated_summary` | `curate` / Den internal | Index selected curated summaries or pointers into Bear/mission Letta Archives. |

`den.memory.request_review` supersedes narrower producer-side names such as `den.memory.propose_core_write` or `den.memory.propose_core_update`. The caller may provide a `suggested_action`, but `curate` decides the final outcome.

### P5 — Letta Archives semantic retrieval

Use Letta Archives instead of introducing a BEARS vector store.

Planned behavior:

- Create one Bear curated archive per Bear.
- Add Cabinet Mission archives once Cabinet Missions and Bear↔Mission assignments are defined.
- Attach shared archives to role agents by policy.
- Den/curate owns writes to shared archives through `/v1/archives/{archive_id}/passages`.
- Non-curate roles may search attached archives but must not independently archive `core/`.
- Passage metadata stores canonical IDs, source URIs, version/hash, updated timestamps, and provenance.
- Den maintains source-to-passage mappings because Letta does not provide passage external IDs/upsert.

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

---

## Frontmatter format

Memory entries should be written as Markdown with YAML frontmatter.

Example:

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

Pair is API-direct and does not naturally use Letta Code MemFS tools, so Den-hosted memory tools are the right first path.
```

Implementation may serialize frontmatter directly; a full YAML parser is not required for the first write path if output is controlled. Search/filter can begin with path/text and later parse frontmatter.

---

## Path conventions

Den or the MemFS tool chooses paths. Agents do not provide arbitrary paths to `den.memory.write_entry`.

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

## MemFS Manager changes

The existing MemFS Manager has a pair-only role note endpoint. Backward compatibility is not required, so it can be replaced by the role memory-entry endpoint once Den no longer calls it.

### Existing behavior

- Endpoint writes only pair notes.
- Writes under `pair/notes/<timestamp>-<slug>.md`.
- Rejects roles other than pair.
- Resets/updates role view repos after note writes.

This endpoint should be removed or left unreachable after `den.memory.write_entry` is implemented. Do not keep model-visible compatibility aliases.

### New endpoints

Add management endpoints that are not exposed directly to agents:

```text
POST /v1/management/bears/{bear_id}/roles/{role}/memory-entries
GET  /v1/management/bears/{bear_id}/roles/{role}/memory-status
GET  /v1/management/bears/{bear_id}/roles/{role}/memory-tree
GET  /v1/management/bears/{bear_id}/roles/{role}/memory-files/{path}
GET  /v1/management/bears/{bear_id}/roles/{role}/memory-search
```

Endpoint naming can be adjusted during implementation; the important point is that Den talks to MemFS Manager, and agents talk to Den.

### Role initialization

Update canonical branch initialization to create conventional directories:

```text
talk/notes/
talk/logs/
talk/decisions/
talk/reflections/
talk/scratch/
talk/summaries/
pair/notes/
pair/logs/
pair/decisions/
pair/reflections/
pair/scratch/
pair/summaries/
curate/notes/
curate/logs/
curate/decisions/
curate/reflections/
curate/summaries/
work/logs/
work/decisions/
work/scratch/
work/summaries/
watch/logs/
watch/scratch/
watch/summaries/
```

Keep existing reserved directories:

```text
pair/tasks/
curate/core/tasks/
curate/core/results/
work/results/
watch/observations/
watch/subscriptions/
```

Actual canonical path policy must be checked before expanding writable prefixes.

---

## Den changes

### Tool descriptors

Add descriptors in Den's built-in tool catalog:

- `den.session.info`
- `den.memory.write_entry`
- `den.memory.status`
- `den.memory.browse`
- `den.memory.read`
- `den.memory.search`

For ACP pair exposure, add provider-safe names:

- `session_info`
- `memory_write_entry`
- `memory_status`
- `memory_browse`
- `memory_read`
- `memory_search`

Initially expose only P0 for pair, then P1.

### Tool invocation

Extend Den's tool dispatcher:

- `den.session.info` returns trusted invocation context, memory scopes, relevant policy, and health summary.
- `den.memory.write_entry` validates role, kind, lifecycle, refs, tags, and body limits; calls MemFS Manager; returns path, entry id, commit, and view status.
- `den.memory.status` calls MemFS Manager health/status endpoints.
- `den.memory.browse/read/search` call MemFS Manager read endpoints with role-aware scope checks.

### ACP pair integration

Update ACP direct pair descriptors:

- Continue exposing existing client/local tools filtered by adapter capabilities.
- Continue exposing `web_fetch` and `web_search` initially.
- Remove `den_write_note` from ACP pair descriptors when `memory_write_entry` is added.
- Add `session_info`, `memory_write_entry`, and `memory_status` in P0.
- Add read/search/tree after MemFS Manager read endpoints are ready.

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
- commit/hash display;
- MemFS health/quarantine indicators;
- Cabinet links when present;
- clear display for memories with no Cabinet mapping.

Do not design agent-only payloads that the UI cannot reuse.

---

## Implementation slices

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
5. Include MemFS configured/unconfigured status if cheap; otherwise return unknown with diagnostic.
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
4. Return canonical tip, role/view tip if known, quarantine state, last reconcile status, and diagnostic.
5. Tests for configured/unconfigured MemFS sidecar behavior.

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

1. Role matrix implementation.
2. Curate can read all branches.
3. Talk/work/watch scopes enforced.
4. Codepool or Letta Code integration decision for harness-backed roles.
5. Tests by role.

### Slice 6 — broaden write entries selectively

Goal: add role-local semantic entries for roles where Den adds value.

Deliverables:

1. Enable `talk`, `curate`, `work`, and/or `watch` according to policy.
2. Work writes require task/run binding when relevant.
3. Watch observations remain behind `write_observation`; generic entries avoid `kind: observation` initially.
4. Tests by role and kind.

### Slice 7 — review/promotion/history tools

Goal: implement governed memory lifecycle in a Reflection-compatible way.

Deliverables:

1. `den.memory.request_review` for producer roles, starting with `pair`.
2. Curate proposal list/read.
3. Curate proposal resolution through `den.memory.resolve_proposal`.
4. Constrained `core/` updates through `den.memory.apply_core_update` or equivalent structured core-write tools.
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

- Den `den.memory.write_entry` writes through MemFS Manager.
- ACP pair sees and can call memory tools.
- `den_write_note` is no longer advertised once `memory_write_entry` is available.
- Cross-role reads/writes denied.
- MemFS unavailable returns clear configuration error.

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
4. Should `den.memory.write_entry` be visible to `talk` if native Letta Code memory tools are available?
5. Should `scratch` entries have automatic retention/cleanup, or just lifecycle metadata at first?
6. Should person references be accepted as opaque strings in P0, or deferred until privacy policy is designed?

---

## Recommended first implementation target

Implement these three tools for `pair` first:

1. `session_info`
2. `memory_write_entry`
3. `memory_status`

Retire `den_write_note` rather than maintaining compatibility.

This creates a useful pair memory vertical slice while avoiding the harder read/search and promotion work until the write model and situation briefing are proven.
