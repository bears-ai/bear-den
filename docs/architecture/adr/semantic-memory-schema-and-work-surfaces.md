# Semantic Memory Schema and Work Surfaces — Architecture Decision Record

## Status: Proposed

## Date: 2026-05-23

---

## Context

BEARS semantic memory has been evolving from opaque note-style storage toward more legible, policy-driven role-local memory in MemFS.

Several tensions have become clear in the current model:

1. **Role-local memory is durable memory, not just staging.** A role-local entry can be the correct final home for a memory without promotion to `core/` or Cabinet.
2. **Authenticated user identity has been overloaded.** Using authenticated username in the default path conflates provenance with scope.
3. **Authorship and provenance are not the same thing.** In most semantic memory writes, the Bear role agent composes the memory entry, while the authenticated human provides session context, initiation, approval, or steering.
4. **Work-surface memory needs stronger structure.** Durable technical understanding about a repo, subsystem, service, or workstream is not well served by a flat `<role>/<kind>/...` layout alone.
5. **`plans` as a semantic memory kind creates confusion.** It overlaps with ACP/Den planning tools, live work-plan state, and plan artifacts, and encourages mixing active planning state into semantic memory.

Historically, semantic memory documentation has also mixed several concerns at once: memory philosophy, path conventions, tool direction, archive strategy, and UI aspirations. This ADR narrows the decision surface to the schema and routing model for semantic memory in Bear MemFS.

---

## Decision

### 1. Authorship is part of provenance, not default pathing

Semantic memory paths should not default to authenticated user identity.

Instead:
- Bear role authorship belongs in provenance metadata.
- Authenticated human identity also belongs in provenance metadata.
- Provenance should record who composed the memory and which human/session/request led to it.
- Pathing should reflect retrieval scope and information structure, not incidental attribution.

In ordinary `den.memory.write_entry` flows, the composing author is the Bear role agent, not the human.

### 2. Work surfaces are first-class semantic memory containers

Durable shared technical memory should prefer a first-class work-surface family:

- `pair/work_surfaces/<work_surface_slug>/`

This is the preferred home for memory about a canonical repo, subsystem, service, artifact cluster, or workstream.

### 3. Work surfaces use a standard internal scaffold

Each work surface should support the following structure:

- `pair/work_surfaces/<work_surface_slug>/overview.md`
- `pair/work_surfaces/<work_surface_slug>/concepts/`
- `pair/work_surfaces/<work_surface_slug>/standards/`
- `pair/work_surfaces/<work_surface_slug>/roadmap/`
- `pair/work_surfaces/<work_surface_slug>/decisions/`
- `pair/work_surfaces/<work_surface_slug>/notes/`

These subpaths mean:

- `overview.md`: compact orientation to the work surface, its current state, and major anchors
- `concepts/`: durable conceptual understanding, architecture, terminology, and mental models
- `standards/`: normative guidance, conventions, invariants, and canonical patterns
- `roadmap/`: forward-looking shared intent, known gaps, and expected evolution
- `decisions/`: durable decisions specific to the work surface
- `notes/`: useful shared observations and findings not yet elevated elsewhere

### 4. General semantic memory kinds remain for non-work-surface memory

The following remain valid general semantic memory families:

- `notes`
- `summaries`
- `decisions`
- `reflections`
- `scratch`
- `logs`

These are used for memory that is not primarily organized as work-surface knowledge.

### 5. `plans` is not a semantic memory kind

`plans` must not be treated as a semantic memory kind.

Reasoning:
- it overlaps confusingly with ACP/Den planning tools
- it collides conceptually with live work-plan state
- it blurs the boundary between active execution planning and durable semantic memory

Planning should live in planning systems, work-plan artifacts, and dedicated tools—not as a general semantic memory kind.

### 6. Human-scoped memory is explicit, not default

Username should appear in semantic memory paths only when the memory is genuinely human-scoped.

Examples include:
- stable user preferences
- recurring user-specific constraints
- standing user-specific collaboration context

Suggested patterns:

- `pair/notes/humans/<username>/<title-slug>.md`
- `pair/summaries/humans/<username>/<title-slug>.md`
- `pair/decisions/humans/<username>/<title-slug>.md` (rare)

### 7. Conversation and shared fallbacks remain available

When a memory is not work-surface scoped and not human-scoped:

- use conversation-scoped paths if the memory is thread-local and no canonical work surface is known
- use shared paths if the memory is durable and reusable but not tied to a specific work surface or human

Suggested patterns:

- `pair/notes/conversations/<conversation-id>/<title-slug>.md`
- `pair/summaries/conversations/<conversation-id>/<title-slug>.md`
- `pair/notes/shared/<title-slug>.md`
- `pair/summaries/shared/<title-slug>.md`
- `pair/decisions/shared/<title-slug>.md`

---

## Provenance model

Authorship and human participation should be preserved in provenance metadata.

Conceptual provenance shape:

```json
{
  "bear": {
    "role": "pair",
    "agent_id": "agent-..."
  },
  "human": {
    "user_id": 2,
    "username": "gerwitz",
    "display_name": "Hans Gerwitz"
  },
  "session": {
    "conversation_id": "conv-...",
    "session_id": "acp-...",
    "acp_session_id": "acp-...",
    "runtime_target": "conv-...",
    "request_id": "..."
  }
}
```

Implications:
- The Bear is the composing author.
- The authenticated human is provenance, not default namespace.
- Provenance remains rich even when the path is shared or work-surface scoped.

---

## Routing guidance

### Prefer work-surface routing when

- the memory is about a canonical repo, service, subsystem, or workstream
- the memory should support future work on that same surface
- the memory is durable shared technical understanding

### Prefer human routing when

- the memory is specifically about that human
- the memory is intended as standing per-human context

### Prefer conversation routing when

- the memory is useful for the current thread
- no reliable work surface is known yet
- durability is uncertain or local

### Prefer shared routing when

- the memory is durable and reusable
- but does not fit a known work surface or human scope

---

## Consequences

### Positive

- Semantic memory paths become more legible and semantically meaningful.
- Authenticated usernames stop polluting paths for shared technical memory.
- Work-surface knowledge becomes easier to browse, retrieve, and maintain.
- The distinction between provenance and namespace becomes cleaner.
- Semantic memory is better separated from planning systems.

### Tradeoffs

- Write-path logic becomes more sophisticated.
- Work-surface identification must be reliable enough to route memory well.
- Existing path conventions and retrieval expectations will need migration.
- Some existing entries may need reclassification or grandfathered handling.

---

## Implementation direction

1. Stop default path derivation from authenticated username.
2. Record Bear authorship and human identity in provenance metadata.
3. Add first-class write support for `pair/work_surfaces/<slug>/...`.
4. Prefer work-surface routing for durable technical memory.
5. Retire `plan` from semantic-memory write validation for new general semantic memory entries.
6. Preserve human-specific pathing only for genuinely human-scoped memory.
7. Update retrieval/orientation logic to prefer work-surface memory for technical tasks.

---

## Status of prior documentation

Earlier semantic memory documentation mixed background philosophy, tool direction, pathing, and speculative future design in a single context document. This ADR supersedes earlier informal path/schema guidance with a formal decision focused on semantic memory structure and routing.
