# Semantic Memory Schema ADR Implementation Plan

## Goal

Implement the ADR in `docs/architecture/adr/semantic-memory-schema-and-work-surfaces.md` by updating semantic memory write contracts, routing, retrieval assumptions, and documentation so Bear memory aligns with the new schema.

This plan assumes:
- authorship is provenance, not default pathing
- work surfaces are first-class semantic memory containers
- `plans` is not a semantic memory kind
- authenticated usernames should not be the default namespace for shared technical memory

---

## Desired end state

After implementation:

- semantic memory writes no longer default to `<kind>/<username>/...`
- Bear role authorship and human identity are recorded in provenance metadata
- durable technical memory can be written under `pair/work_surfaces/<work_surface_slug>/...`
- work-surface memory supports a standard scaffold:
  - `overview.md`
  - `concepts/`
  - `standards/`
  - `roadmap/`
  - `decisions/`
  - `notes/`
- `plan` is removed from semantic-memory write validation and documentation
- retrieval/orientation logic can prefer work-surface memory for technical tasks
- legacy memory remains readable during a transition period

---

## Workstreams

## 1. Write-contract changes in Den

### Objective

Extend the semantic memory write contract so it can express ADR-compliant routing without relying on authenticated username as the default path segment.

### Changes

Update the Den-side memory write request shape to support explicit routing hints.

Recommended additions:
- `scope`: `human | work_surface | conversation | shared`
- `work_surface_slug`: optional
- `work_surface_section`: optional
  - `overview`
  - `concepts`
  - `standards`
  - `roadmap`
  - `decisions`
  - `notes`
- optional future field: `subject_human`

Recommended changes to existing semantics:
- stop treating authenticated username as top-level `author`
- encode Bear role authorship and human/session/request data in provenance/source metadata
- remove `plan` from accepted semantic memory kinds

### Likely files

- `services/den/src/core/den_tools.rs`
- `services/den/src/core/memory_manager_head.rs`

### Deliverables

- updated request structs
- updated argument schema/validation
- updated write request construction
- tests covering request construction and validation

---

## 2. Routing and path derivation in MemFS

### Objective

Make MemFS the authoritative implementation of ADR-compliant semantic memory path routing.

### Changes

Replace current default routing logic with scope-aware routing.

Desired routing behavior:

#### Work-surface scope
- `pair/work_surfaces/<work_surface_slug>/overview.md`
- `pair/work_surfaces/<work_surface_slug>/<section>/<title-slug>.md`

#### Human scope
- `pair/<kind>/humans/<username>/<title-slug>.md`

#### Conversation scope
- `pair/<kind>/conversations/<conversation-id>/<title-slug>.md`

#### Shared scope
- `pair/<kind>/shared/<title-slug>.md`

#### When scope is omitted
Infer in this priority order:
1. canonical work surface when confidently known
2. conversation scope when clearly thread-local
3. shared fallback

Do not default to human scope purely because the human is authenticated.

Additional changes:
- validate allowed work-surface sections
- implement collision handling for work-surface entries
- update path policy to allow `pair/work_surfaces/...`
- remove `plan` from kind-directory mappings for general semantic memory

### Likely files

- `services/memfs-manager/git_memfs_server.py`

### Deliverables

- new derivation helpers
- updated path policy rules
- tests for each scope and section
- tests proving username is not the default namespace

---

## 3. Work-surface scaffold support

### Objective

Treat work surfaces as structured memory containers rather than ad hoc directories.

### Changes

Ensure tooling can create and maintain a standard work-surface scaffold:
- `overview.md`
- `concepts/`
- `standards/`
- `roadmap/`
- `decisions/`
- `notes/`

This may be implemented by:
- extending existing work-surface scaffold logic
- or adding a focused helper in the memory/scaffold path

### Likely files

- Den work-surface orientation/scaffold code
- potentially MemFS helper code if canonical creation lives there

### Deliverables

- scaffold creation support for the new layout
- tests for idempotent scaffold creation
- documentation/examples for expected structure

---

## 4. Retrieval and orientation alignment

### Objective

Make the system actually benefit from the new structure by preferring work-surface memory during technical tasks.

### Changes

Update memory orientation, browse, and retrieval logic so that when a work surface is known, the system prefers:
- `overview.md`
- `standards/`
- `concepts/`
- `roadmap/`
- `decisions/`
- `notes/`

Potential behavior changes:
- work-surface orientation tool surfaces canonical paths under `pair/work_surfaces/<slug>/...`
- technical memory reads/searches consult work-surface memory before broad generic role memory when confidence is high
- retrieval remains backward-compatible with legacy paths during transition

### Likely files

- Den work-surface orientation logic
- any helper code that prioritizes memory paths

### Deliverables

- updated orientation output
- retrieval preference rules
- tests for work-surface-first technical memory orientation

---

## 5. Documentation cleanup and consolidation

### Objective

Align docs with the ADR and eliminate stale or competing memory-schema guidance.

### Changes

- keep the ADR as the primary source of truth for semantic memory schema
- remove stale references to old flat pathing as authoritative schema
- remove `plan` from semantic memory documentation/examples
- ensure work-surface-first structure is reflected in planning and architecture docs where relevant

### Current repo status

This cleanup has already started:
- `docs/context/` has been removed
- ADR created at `docs/architecture/adr/semantic-memory-schema-and-work-surfaces.md`

### Deliverables

- no stale docs treating `plan` as a semantic memory kind
- no stale docs treating `<kind>/<username>` as the default routing model
- updated references where semantic memory structure is mentioned

---

## 6. Legacy compatibility and migration

### Objective

Adopt the new schema without breaking access to existing memory abruptly.

### Recommended approach

Start with soft migration:
- new writes use the new schema
- old files remain readable in legacy locations
- browse/search/orientation can consider both old and new layouts during transition

Future optional step:
- targeted migration of clearly shared technical memory from `<kind>/<username>/...` into work-surface paths

Avoid immediate full migration unless clear value emerges.

### Deliverables

- compatibility stance documented in code/comments/tests
- search/read behavior that does not strand legacy entries

---

## Sequence

### Phase 1: Contract and validation
1. Remove `plan` from semantic memory kind validation
2. Add routing fields to Den write request and validation
3. Shift Bear/human identity handling into provenance semantics

### Phase 2: MemFS routing
4. Implement scope-aware path derivation in MemFS
5. Add support for `pair/work_surfaces/<slug>/...`
6. Add tests for work-surface, human, conversation, and shared routing

### Phase 3: Scaffold support
7. Implement or extend work-surface scaffold creation
8. Verify standard internal layout

### Phase 4: Retrieval/orientation
9. Update work-surface orientation to prefer new canonical paths
10. Update retrieval logic to favor work-surface memory for technical tasks

### Phase 5: Cleanup and compatibility
11. Align docs and remove stale references
12. Preserve legacy readability and decide later on targeted migration

---

## Open design questions

These should be resolved during implementation:

1. Should `scope` be optional with inference, or required for direct write callers?
2. Should `overview.md` be written only through dedicated scaffold/orientation flows, or also allowed via generic write-entry with `work_surface_section: overview`?
3. How should work-surface inference be represented in provenance when the caller did not specify it explicitly?
4. Do we want a dedicated top-level memory tool for work-surface writes later, or should generic `memory_write_entry` remain the only writer?
5. How aggressively should retrieval prefer work-surface memory over legacy role-local paths during transition?

---

## Success criteria

This ADR is implemented successfully when:

- semantic memory no longer defaults to username-based routing for shared technical content
- work-surface memory has a stable first-class on-disk shape
- `plan` is absent from semantic memory kind validation and docs
- Bear/human provenance is preserved without driving default path namespace
- technical memory retrieval can orient through work-surface paths
- legacy memory remains accessible during transition
