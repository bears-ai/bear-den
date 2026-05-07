# Semantic Bear Memory — Architecture Decision Record

## Status: Accepted

## Date: 2026-05-07

---

## Context

BEARS has several memory and knowledge surfaces that must remain distinct:

- **Cabinet** is the shared, human-editable knowledge base.
- **Bear MemFS** is durable bear memory with role-scoped branches (`talk/`, `pair/`, `curate/`, `work/`, `watch/`) and curated shared `core/` memory.
- **Letta memory blocks and archival memory** remain Letta-native runtime memory.
- **Artifacts** store files and outputs, not memory.
- **Conversation/session state** is operational context for an interaction, not necessarily durable memory.

Earlier memory architecture established that raw role-local memory can be reviewed and promoted into `core/` by `curate`. That remains true, but it is incomplete if interpreted as a required pipeline. Some memory is useful only to one role and should stay role-local permanently. Some memories are too bear-local, tactical, noisy, private, or operational to belong in Cabinet. Some role memories never need promotion to `core/`.

The system also needs a semantic vocabulary for shared knowledge. Cabinet should contain designated spaces for:

- **People** — knowledge about people, relationships, preferences, and identity-sensitive facts.
- **Missions** — projects, goals, initiatives, responsibilities, and long-running areas of effort.
- **Knowledge** — general reusable knowledge, policies, procedures, decisions, concepts, and references.

The question is how this semantic structure should influence Bear memory without forcing every memory object to map to a Cabinet object or become a `core/` candidate.

---

## Decision

BEARS will treat semantic memory as a combination of **locality**, **kind**, **references**, and **lifecycle**.

A memory object is not defined solely by where it may eventually be promoted. It is valid for a memory object to remain role-local forever, have no Cabinet mapping, and have no `core/` promotion target.

### Locality

Memory locality answers: **where does this memory live and who is expected to use it?**

The primary localities are:

| Locality | Purpose |
|---|---|
| `role-local` | Durable memory for one role, such as `pair/` or `watch/`. This can be the final destination. |
| `core` | Curated cross-role Bear memory. |
| `cabinet` | Shared human-facing canonical knowledge. |
| `artifact` | File/output storage, not itself memory, but often referenced by memory. |
| `conversation` / `session` | Ephemeral or interaction-scoped state. |

Role-local memory is not a staging area by default. Promotion to `core/` or Cabinet is an explicit lifecycle state, not an assumption.

### Kind

Memory kind answers: **what type of memory is this?**

BEARS will use a small vocabulary of semantic kinds rather than a rigid directory tree or many narrowly named tools.

Initial memory kinds:

| Kind | Meaning | Usual lifecycle |
|---|---|---|
| `note` | Synthesized role-local memory, reminder, fact, or explanation. | Often remains local. |
| `log` | Chronological record of activity, attempts, runs, or operational sequence. | Usually remains local; may be summarized. |
| `decision` | Tactical or strategic choice with rationale. May be role-local, `core`, or Cabinet-worthy depending on scope. | Often remains local unless broadly useful. |
| `reflection` | Self-analysis, lessons learned, compaction output, or review notes. | May remain local or produce proposals. |
| `scratch` | Temporary working memory. | Usually expires or is archived. |
| `summary` | Condensed form of longer local material. | May remain local or be promoted. |
| `proposal` | Explicit request to change shared memory, Cabinet, skills, tasks, or other governed state. | Requires review. |
| `observation` | Event-derived or watch-derived record. | Usually schema-managed and reviewable. |
| `result` | Task/run output summary. | Usually schema-managed and reviewable. |

General-purpose role memory tools should initially handle low-risk kinds such as `note`, `log`, `decision`, `reflection`, `scratch`, and `summary`. Schema-sensitive kinds such as `proposal`, `observation`, and `result` should continue to use dedicated tools where validation and lifecycle matter.

### References

Memory references answer: **what is this memory about?**

References are optional. A memory can reference zero or more people, missions, knowledge areas, Cabinet pages, artifacts, tasks, runs, conversations, or source events.

Important distinction:

| Field | Meaning |
|---|---|
| `mission_ref` | This memory relates to a mission. It does not imply Cabinet mapping or promotion. |
| `person_ref` | This memory relates to a person. It may require privacy policy checks. |
| `knowledge_ref` | This memory relates to a knowledge area or concept. |
| `cabinet_ref` | This memory points to a Cabinet object. |
| `promotion_target` | This memory is intended or proposed for promotion. |

Mission/person/knowledge references must not imply that a Cabinet object exists. Cabinet references are explicit.

### Lifecycle

Memory lifecycle answers: **what should happen to this memory over time?**

Common lifecycle metadata:

| Field | Values |
|---|---|
| `scope` | `role-local`, `core-candidate`, `cabinet-candidate` |
| `retention` | `session`, `short`, `durable`, `archive` |
| `promotion` | `none`, `maybe`, `proposed`, `rejected`, `approved` |
| `status` | `active`, `superseded`, `stale`, `archived` |

A role-local memory with `promotion: none` is complete and valid. It is not a failed proposal.

### Cabinet semantic spaces

Cabinet will use these top-level semantic spaces as the canonical shared knowledge structure:

| Space | Purpose |
|---|---|
| `People` | Human/person knowledge, identity-sensitive facts, preferences, relationships, and stakeholders. |
| `Missions` | Projects, goals, initiatives, responsibilities, routines, and ongoing efforts. |
| `Knowledge` | General reusable knowledge, policies, procedures, decisions, concepts, and references. |

Bear memory may reference Cabinet spaces, but Bear memory does not mirror Cabinet one-to-one. Cabinet is the library. Bear memory is each role's working notebook, operational history, orientation map, and governance trail.

### Situation instead of current context

BEARS will use **situation** for the trusted interaction briefing concept, rather than “current context.”

The term “context” is overloaded with model context windows and compiled prompt context. A situation briefing describes what the agent needs to know about the current interaction and its boundaries.

The planned tool name is:

| Canonical | Provider-safe |
|---|---|
| `den.situation.get` | `den_situation_get` |

A situation briefing may include:

- bear identity,
- role,
- user/channel/session identity,
- active mission hints when known,
- allowed memory read/write scopes,
- relevant Cabinet spaces,
- policy reminders,
- MemFS health summary,
- available memory and Cabinet tools.

`den.situation.get` is not a memory read tool. It is a trusted Den briefing for safe operation.

---

## Tool Design Implications

### Prefer flexible entry tools over rigid category tools

BEARS should avoid tool sprawl such as `write_pair_mission_note`, `write_pair_log`, or `write_work_tactical_decision`.

Prefer a general role-local entry tool:

| Canonical | Provider-safe | Purpose |
|---|---|---|
| `den.memory.write_entry` | `den_memory_write_entry` | Write a role-local memory entry with `kind`, semantic references, lifecycle metadata, and provenance. |

The existing `den.write_note` / `den_write_note` pair tool can remain as a compatibility alias for a `den.memory.write_entry` call with `kind: note`.

### Keep directories conventional, not conceptual

Paths should be predictable for humans and tools, but the conceptual model should live in metadata.

Recommended role-local path conventions:

```text
<role>/notes/
<role>/logs/
<role>/decisions/
<role>/reflections/
<role>/scratch/
<role>/summaries/
```

Reserved schema-owned paths should stay behind dedicated tools:

```text
<role>/tasks/
<role>/results/
<role>/observations/
<role>/subscriptions/
```

A mission-related tactical decision should be represented as `kind: decision` with `mission_ref`, not forced into a special `mission-decisions/` path.

### Read/search/browse tools should support human and agent inspection

The next product step is a UI that lets human operators browse, search, and inspect Bear memory. Den memory tools and APIs should therefore expose memory in a shape useful to both agents and UI:

- tree browsing,
- role filters,
- kind filters,
- semantic reference filters,
- lifecycle filters,
- full entry inspection,
- provenance display,
- commit/history links,
- MemFS health/quarantine status,
- Cabinet reference links where present.

Candidate tool/API names:

| Canonical | Provider-safe | Purpose |
|---|---|---|
| `den.memory.tree` | `den_memory_tree` | Browse allowed memory paths. |
| `den.memory.read` | `den_memory_read` | Read an allowed memory file or entry. |
| `den.memory.search` | `den_memory_search` | Search memory with role/kind/reference filters. |
| `den.memory.history` | `den_memory_history` | Inspect commit/file history. |
| `den.memory.status` | `den_memory_status` | Inspect MemFS role/canonical health. |

---

## Role Implications

### `pair`

`pair` is API-direct through ACP and should receive Den-hosted memory tools earlier than harness-backed roles. It needs role-local notes, logs, tactical decisions, and session summaries that may never leave `pair/`.

Recommended first write capability:

- `den_memory_write_entry` for `note`, `log`, `decision`, `reflection`, `scratch`, and `summary`.

### `talk`

`talk` may use Letta Code-native memory tools for ordinary MemFS work, but Den tools remain useful for governed writes, situation briefings, and operator-visible memory entries. `talk` memory can remain local when it captures conversational tactics or channel-local observations.

### `curate`

`curate` is the semantic librarian and governance role. It should inspect all role memories, decide what is worth promoting, and record why some memories remain local or are rejected.

`curate` should distinguish:

- local tactical decisions,
- cross-role `core` decisions,
- Cabinet-worthy shared decisions,
- rejected or stale proposals.

### `work`

`work` memory should be task/run-bound. Logs and decisions are valuable, but should usually be attached to a task/run and not promoted automatically.

### `watch`

`watch` memory should emphasize observations and logs. Watch-derived data is not shared truth until reviewed. Some low-salience observations may remain local indefinitely.

---

## Consequences

### Positive

- Avoids treating role-local memory as merely failed promotion.
- Preserves Cabinet as canonical shared knowledge without forcing all Bear memories into Cabinet.
- Gives agents and future UI a consistent semantic vocabulary.
- Supports tactical local decisions and logs without over-promoting them.
- Reduces tool sprawl by using `kind`, references, and lifecycle metadata.
- Avoids confusion between situation briefings and model context windows.

### Tradeoffs

- Metadata quality matters; poorly tagged memories may be harder to browse.
- Search and UI need to understand both paths and metadata.
- `curate` must handle more nuanced lifecycle states than simple promote/reject.
- Some memory objects will have no Cabinet link, which is intentional but requires clear UI copy.

### Non-goals

- Do not mirror Cabinet into Bear memory one-to-one.
- Do not require every Bear memory to have a Cabinet object.
- Do not require every role memory to become `core/`.
- Do not use agent tools for destructive memory resets or operator overrides.
- Do not use “context” as the label for situation briefing tools.

---

## Follow-up Work

1. Add a Den `den.situation.get` design and eventually tool/API implementation.
2. Generalize `den.write_note` into `den.memory.write_entry` while keeping compatibility for ACP `pair`.
3. Add memory browse/search/read/status APIs that can support both agents and the future human operator UI.
4. Define the frontmatter schema for role-local memory entries.
5. Update MemFS Manager role branch initialization to include conventional directories as needed.
6. Design Cabinet metadata for `People`, `Missions`, and `Knowledge` spaces.
7. Design the operator UI for browsing, searching, inspecting, and understanding Bear memory without implying everything is Cabinet-backed.
