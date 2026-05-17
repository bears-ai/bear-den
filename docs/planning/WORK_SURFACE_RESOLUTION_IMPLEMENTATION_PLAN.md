# Work-Surface Resolution Implementation Plan

## Status

Draft. Follows the Pair Letta message-boundary and tool-discovery work.

## Problem

BEARS now distinguishes Bear memory, Workplaces, work surfaces, threads, and turns. `session_info` can expose work-surface hints, but hints are not the same as resolution. A Bear should know whether it is operating with no known work surface, one likely candidate, multiple candidates, a resolved work surface, or a user-confirmed work surface.

This resolution state should be visible to the Bear so it can communicate uncertainty, ask the user to verify, or ask the user to choose between candidates when scope affects memory, artifacts, plans, or actions.

## Goals

1. Represent work-surface resolution explicitly.
2. Keep resolution separate from persisted user-message content.
3. Surface resolution through `session_info` and orientation tools.
4. Let the Bear communicate assumptions and ask for confirmation when needed.
5. Preserve user-confirmed resolution as provenance for later memory/plans/artifacts.
6. Avoid overconfident automatic classification.

## Non-goals

- Do not build a full Workplace/work-surface registry UI in the first slice.
- Do not require every turn to resolve a work surface.
- Do not make work-surface resolution an authorization boundary.
- Do not create a vector store or alternate semantic memory system.
- Do not infer aggressively from weak hints.

## Concepts

### Resolution state

| State | Meaning | Agent behavior |
|---|---|---|
| `unresolved` | No useful candidate is known. | Avoid broad-memory assumptions; ask or inspect when scope matters. |
| `candidate` | One likely candidate exists. | Proceed for low-risk work; state assumption when scope matters. |
| `ambiguous` | Multiple plausible candidates exist. | Ask user to choose or inspect to disambiguate. |
| `resolved` | Evidence identifies the work surface. | Use work-surface-first grounding. |
| `confirmed` | User explicitly confirmed the work surface. | Treat as authoritative for this thread unless contradicted. |
| `rejected` | Candidate was explicitly rejected. | Avoid that candidate unless new evidence appears. |

### Confidence

Use a simple confidence scale:

- `none`
- `low`
- `medium`
- `high`
- `confirmed`

### Evidence kinds

Initial evidence kinds:

- `explicit_channel_metadata`
- `user_reference`
- `workspace_root`
- `runtime_target`
- `conversation_selection`
- `memory_anchor`
- `git_remote` (later)
- `cabinet_mission` (later)
- `docket_project` (later)

## Target data shape

`session_info.work_surface` should evolve toward:

```json
{
  "status": "candidate",
  "confidence": "medium",
  "needs_user_confirmation": false,
  "active_candidate": {
    "slug": "bears-monorepo",
    "name": "BEARS monorepo",
    "kind": "repository",
    "confidence": "medium",
    "evidence": [
      { "kind": "workspace_root", "value": "/workspace" }
    ]
  },
  "candidates": [],
  "agent_guidance": {
    "may_state_assumption": true,
    "should_ask_user_when": [
      "multiple plausible work surfaces",
      "memory/action depends on scope",
      "user asks to continue prior work but current surface is unclear"
    ],
    "confirmation_examples": [
      "Should I treat this as the BEARS monorepo work surface?",
      "Are we working in Den or Codepool?"
    ]
  },
  "recommended_grounding_order": [
    "current conversation",
    "session_info",
    "current work-surface anchors",
    "role-local memory",
    "core memory",
    "workspace artifacts"
  ]
}
```

## Phase 1: Enrich read-only orientation output

### 1.1 Add resolution fields to `infer_work_surface_hint`

Current output already includes candidates. Add:

- `status`
- `confidence`
- `needs_user_confirmation`
- `agent_guidance`
- `recommended_grounding_order`

Initial rules:

- no candidates → `status=unresolved`, `confidence=none`, `needs_user_confirmation=false`
- one candidate → `status=candidate`, `confidence=medium`, `needs_user_confirmation=false`
- multiple candidates → `status=ambiguous`, `confidence=low`, `needs_user_confirmation=true`

Do not mark anything `resolved` yet unless confirmed by memory anchors in a later phase.

### 1.2 Update `session_info` tests

Tests should verify:

- no hints returns unresolved/none,
- one hint returns candidate/medium,
- multiple hints returns ambiguous/low and `needs_user_confirmation=true`,
- guidance contains confirmation examples.

### 1.3 Keep descriptors unchanged unless needed

Existing descriptors already point to `session_info` and `memory_orient_work_surface`.

## Phase 2: Canonical anchor confirmation

### 2.1 Add anchor probe to `memory_orient_work_surface`

`memory_orient_work_surface` already has access to memory/MemFS. It should become the first tool that can move from candidate to resolved.

Given a candidate slug, check for canonical anchors:

```text
core/work_surfaces/<slug>/index.md
core/work_surfaces/<slug>/overview.md
core/work_surfaces/<slug>/architecture.md
core/work_surfaces/<slug>/decisions.md
pair/work_surfaces/<slug>/current-understanding.md
```

If anchors exist:

- `status=resolved`
- confidence `high`
- include canonical anchor paths
- include evidence kind `memory_anchor`

If no anchors exist:

- keep `candidate`
- suggest scaffold creation only when appropriate

### 2.2 Tests

- candidate slug with no anchors remains candidate.
- candidate slug with anchors becomes resolved.
- returned anchors distinguish `core/` and role-local paths.

## Phase 3: User confirmation state

### 3.1 Add session-level confirmation storage

Add a lightweight session/thread-level record for work-surface resolution.

Possible storage options:

- extend ACP/session runtime metadata,
- new `bear_session_work_surfaces` table,
- JSON field on a future shared pair session record.

Fields:

- `bear_id`
- `role`
- `channel_family`
- `session_id`
- `conversation_id`
- `work_surface_slug`
- `status` (`confirmed`, `rejected`)
- `confirmed_by_user_id`
- `source_text` / `confirmation_text`
- `created_at`, `updated_at`

### 3.2 Add a confirmation tool

Tentative provider name:

- `work_surface_choose`

Purpose:

- confirm one candidate,
- reject a candidate,
- set active thread work surface based on explicit user instruction.

Constraints:

- role/session scoped,
- not a memory write by itself,
- no broad Bear-global claim unless curate later promotes it.

### 3.3 Update `session_info`

If confirmed state exists:

- `status=confirmed`
- `confidence=confirmed`
- include `confirmed_by` and timestamp
- include original evidence/confirmation text if safe.

## Phase 4: Stronger evidence sources

Add evidence sources gradually.

### 4.1 Repo metadata

For ACP, Den cannot inspect client files directly. Options:

- adapter reports git remote / repo root metadata in client context,
- agent uses local tools to inspect `.git/config` or package metadata,
- user explicitly identifies repo.

Do not make Den server assume local filesystem access.

### 4.2 Explicit user references

Detect explicit user references conservatively:

- “in the Den service”
- “for Codepool”
- “this BEARS repo”

These should create candidates or increase confidence, but not silently confirm unless the user clearly confirms.

### 4.3 External references

Later:

- Cabinet Mission ids,
- Docket project ids,
- deployment environment ids,
- service registry entries.

## Phase 5: Provenance propagation

When memory/plans/artifacts are created after resolution:

- include work-surface status/confidence,
- include confirmed work-surface slug when available,
- record whether confirmation came from user, memory anchor, workspace metadata, or inference.

This prevents role-local observations from becoming overbroad Bear-global facts.

## UX expectations

The Bear may say:

- “I’m treating this as the BEARS monorepo based on the workspace root.”
- “This could be Den or Codepool. Which work surface should I use?”
- “I don’t yet know which work surface this thread is about. Should I use the current workspace?”
- “Got it — I’ll treat this thread as Codepool.”

The Bear should not ask on every turn. It should ask when ambiguity materially affects retrieval, memory, planning, or action.

## Implementation checklist

### Immediate

- [ ] Add resolution fields to `infer_work_surface_hint`.
- [ ] Update `session_info` tests for unresolved/candidate/ambiguous behavior.
- [ ] Include recommended grounding order and agent guidance in `session_info.work_surface`.

### Next

- [ ] Enhance `memory_orient_work_surface` with canonical anchor confirmation.
- [ ] Add tests for anchor-based resolution.
- [ ] Design persistence for user-confirmed work-surface state.
- [ ] Add confirmation tool only after read-only orientation proves useful.

## Related docs

- `docs/concepts/MEMORY_MODEL.md`
- `docs/architecture/adr/bear-workplaces.md`
- `docs/architecture/adr/pair-tool-discovery-and-scope-orientation.md`
- `docs/planning/PAIR_TOOL_DISCOVERY_AND_SCOPE_POLICY.md`
- `docs/planning/PAIR_LETTA_MESSAGE_BOUNDARY_PLAN.md`
