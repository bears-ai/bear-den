# Pair Reflection and Work Memory Sharing Plan

Status: focused architecture/design plan. Implementation status and sequencing live in [Memory Automation Roadmap](MEMORY_AUTOMATION_ROADMAP.md).

This plan defines the role boundary and data flow for making `pair` memory useful to `work` without exposing raw `pair/` memory. It assumes BEARS will go all the way to a dedicated **pair reflection loop** for improving `pair` role-local memory, and then use `curate` to share useful knowledge across spaces such as `core/`, Bear curated archives, Cabinet, and approved `work` task context.

Related docs:

- [Memory Automation Roadmap](MEMORY_AUTOMATION_ROADMAP.md) — canonical implementation status and sequencing.
- [Memory tools implementation plan](MEMORY_TOOLS_IMPLEMENTATION_PLAN.md)
- [Curate memory governance plan](CURATE_MEMORY_GOVERNANCE_PLAN.md)
- [Semantic Bear Memory ADR](../architecture/adr/semantic-bear-memory.md)
- [Bear Charter and Cabinet Missions](../concepts/BEAR_CHARTER_AND_CABINET_MISSIONS.md)
- [Memory model](../concepts/MEMORY_MODEL.md)

---

## Goal

Make `pair` memory useful without weakening role boundaries.

The target flow is:

```text
pair learns workplace knowledge
→ pair writes role-local memory
→ pair reflection consolidates pair-local memory
→ curate reviews pair outputs and proposals
→ curate shares durable knowledge through core/archive/Cabinet/task context
→ work consumes curated knowledge, not raw pair memory
```

---

## Key principles

1. `pair` may learn things useful to `work`, but `work` should not read raw `pair/` memory.
2. `pair` reflection maintains and improves `pair/` memory.
3. `curate` governs cross-role sharing.
4. `core/` remains compact shared Bear orientation.
5. Letta Archives are derived semantic retrieval indexes, not canonical memory.
6. Cabinet is human-facing canonical shared knowledge.
7. Approved task context is the primary way narrow implementation knowledge reaches `work`.

---

## Roles

| Actor | Responsibility |
|---|---|
| `pair` | Interactive coding/client collaboration. Writes role-local notes, logs, decisions, reflections, summaries, and review requests. |
| Pair reflection loop | Background/session-end maintenance of `pair/`: summarize, deduplicate, identify durable learnings, request curation when useful. |
| `curate` | Cross-role memory governance. Reads role branches, reviews proposals, keeps `core/` clean, indexes curated summaries, and prepares work-safe context. |
| `work` | Executes approved scoped tasks. Reads `core/`, `work/`, task context, and allowed archives; does not read raw `pair/`. |
| Den | Orchestrates loops, stores proposal/activity state, enforces policy, writes audit records, and exposes UI. |

---

## Architecture overview

```text
ACP pair session
  ↓
pair agent
  ↓ writes
pair/notes, pair/logs, pair/decisions, pair/summaries
  ↓
pair reflection loop
  ↓ writes
pair/reflections, pair/summaries, pair/decisions
  ↓ creates
`bear_memory_proposals`
  ↓ enqueues
queued `memory_curate` reflection run
  ↓ outcomes
core/ updates
Bear curated archive index entries
Cabinet proposals/updates
approved work task context
  ↓
work agent receives curated task context
```

---

## Pair reflection loop

### Purpose

Pair reflection is a role-local maintenance loop for `pair/` memory. It is inspired by Letta Code reflection but scoped to API-direct pair, which does not run in Letta Code.

It should:

- summarize completed or substantial pair sessions;
- extract durable technical decisions;
- identify repeated failure modes;
- clean noisy logs into useful summaries;
- preserve human attribution from ACP token identity;
- create review requests for `curate` when knowledge may benefit `core`, Cabinet, archives, or `work`;
- avoid writing `core/` or Cabinet directly.

### Non-goals

Pair reflection must not:

- write `core/`;
- write Cabinet;
- create approved work tasks directly;
- run external tools;
- read `talk/`, `work/`, `watch/`, or `curate/` branches;
- become a sixth Bear role.

It is a Den-orchestrated maintenance loop for the existing `pair` role.

### Triggers

Initial triggers:

1. **ACP session close** — summarize durable learnings from the session.
2. **N completed pair turns** — run after enough activity accumulates.
3. **Tool-heavy session** — run after many local file edits/searches.
4. **Manual UI trigger** — Bear admin/operator can request pair reflection.
5. **Review request backlog** — run when many unreviewed pair memories exist.

Later triggers:

- Letta compaction event, if exposed and useful;
- schedule-based nightly reflection;
- high-signal memory writes such as `decision` or `summary`.

### Inputs

Pair reflection receives bounded input:

- Bear identity and charter/purpose;
- authenticated human summary for relevant sessions;
- recent pair conversation metadata;
- recent pair memory entries;
- recent local tool activity summary;
- selected `core/` orientation files;
- pair memory search/browse/read tools;
- clear policy: role-local maintenance only.

### Outputs

Pair reflection can write:

```text
pair/summaries/
pair/reflections/
pair/decisions/
pair/notes/
```

It may create Den DB review proposals for `curate`. Automatic ACP-close summary proposals are described and tracked in the roadmap.

It should prefer concise summaries over raw transcript copies.

---

## Pair memory review requests

Pair reflection and the pair agent use the same underlying proposal queue: `bear_memory_proposals`.

Canonical future tool:

```text
den.memory.request_review
```

Model-visible provider:

```text
memory_request_review
```

Proposal fields should include:

- source role: `pair`;
- source memory paths;
- source commits/hashes;
- title;
- summary;
- rationale;
- suggested action:
  - `core_update`,
  - `archive_index`,
  - `cabinet_update`,
  - `task_context`,
  - `skill_review`,
  - `unspecified`; the automatic ACP-close summary proposal uses this initially;
- sensitivity;
- refs:
  - domains,
  - missions,
  - tasks,
  - artifacts,
  - Cabinet refs;
- human attribution.

The suggested action is only a hint. `curate` decides the final outcome.

---

## Curate sharing outcomes

Curate turns pair-local knowledge into forms `work` can safely use.

| Outcome | Destination | Work visibility |
|---|---|---|
| Shared orientation | `core/` | `work` can read. |
| Semantic recall | Bear curated archive | `work` may search if attached by policy. |
| Cross-Bear/shared docs | Cabinet | `work` can read if task policy permits. |
| Narrow implementation context | Approved task context | `work` receives with task. |
| Reusable procedure | Skill proposal/review | `work` may later use approved skill. |
| No-op/reject | Proposal closed | Not visible to `work`. |

Curate should distill and transform. It should not copy raw pair logs into `core` or task context.

---

## Work consumption model

`work` should consume pair-derived knowledge only through approved channels:

```text
Allowed:
- core/
- approved task definition
- task-attached memory/context excerpts
- work/
- Bear curated archive search
- Cabinet docs permitted by task policy

Denied:
- raw pair/
- raw talk/
- raw watch/
- raw curate/
```

When a task is created from pair learnings, Den/curate should attach:

- summary of relevant pair knowledge;
- source memory refs;
- source commit/hash;
- exact constraints;
- allowed tools/scope;
- archive/Cabinet pointers when useful.

---

## Letta Archives use

Pair reflection and curate should not create a BEARS vector store.

Use Letta Archives as derived indexes:

- Bear curated archive for shared Bear recall;
- Cabinet Mission archives only when a Cabinet Mission needs cross-Bear recall;
- optional role-local pair archive later if `pair/` MemFS plus Bear archive is insufficient.

Shared archive writes go through Den/curate indexing, not arbitrary pair agent insertion.

Passage metadata should include:

- canonical id;
- source URI;
- source path;
- source role;
- source commit/version;
- source hash;
- indexed timestamp;
- proposal id when applicable.

---

## Pre-turn memory retrieval

Even with pair reflection, `pair` still needs relevant memory at the time of work.

Use a small pre-turn memory briefing:

1. Include tiny `core/` orientation when available.
2. Include recent pair summary pointers/snippets.
3. Search `pair/` and `core/` by prompt terms when likely useful.
4. Later, use `memory_semantic_search` over Bear curated archives.
5. Provide paths/snippets, not large dumps.
6. Encourage `memory_read` before relying on details.

Reflection improves the quality of memory. Pre-turn retrieval decides what remembered information is relevant now.

---

## UI and observability

The Bear memory UI should surface:

- pair reflection runs;
- inputs considered;
- memory entries written;
- memory proposals created;
- curate decisions;
- `core/` updates;
- archive passages indexed;
- task contexts generated for `work`.

Humans should be able to inspect and override, but routine approval should not be required.

---

## Implementation tracker

Detailed phase status, completed work, and next implementation steps are tracked in [Memory Automation Roadmap](MEMORY_AUTOMATION_ROADMAP.md). This document should stay focused on the pair→curate→work design boundary.

---

## Open questions

1. Should pair reflection use the same pair Letta agent, a separate temporary reflection run, or a dedicated lightweight reflection prompt runner?
2. How much pair conversation content should reflection see versus only memory/tool activity summaries?
3. What is the first pre-turn memory hint budget?
4. Should pair reflection run before curate, or should curate be able to trigger pair reflection on demand?
5. When should pair-derived knowledge become task context versus `core` versus archive?
6. How should human-sensitive pair memories be redacted before `work` sees derived summaries?

---

## Recommended next step

Use [Memory Automation Roadmap](MEMORY_AUTOMATION_ROADMAP.md) for the current next step. As of this design, the next product concern is still the same boundary: make pair memory useful through `curate`, while ensuring `work` consumes only curated context and never raw `pair/`.
