# Dynamic skills, reflection subagents, and bear configuration — Architecture Decision Record

## Status: Proposed

*Revised 2026-05-19 to incorporate Bear MemFS–backed skill storage, `bears.yaml` metadata, and sync decisions. The expert inspirational sketch recorded below (2026-04-18) remains useful input, but is not a binding implementation spec.*

## Date: 2026-04-18

---

## Context

**End goal:** Skills should be **dynamic**:

- Operators (and users, where policy allows) attach **predefined skills** from a **catalog** (existing [Den-managed skills](../DEN_ARCHITECTURE.md#den-managed-skills) pattern).
- A **bear** can also **create and refine its own** skills over time.

For BEARS, skills are best understood not merely as arbitrary local files but as a **special class of Bear memory artifact**: reusable operational know-how that can be discovered, reviewed, assigned to roles, synchronized into runtime layouts, shared across Bears, and retired with provenance.

**Upstream capability (Letta / Letta Code):** Recent upstream features align with this without Den reimplementing a skill runtime:

- **Letta Code** can ship with a **skills-creation** skill that enables agents to author or extend filesystem skills in supported layouts.
- **Letta** exposes a **`reflection` subagent type** (and related mechanisms) that can drive **auto-discovery** and structured improvement loops—exact APIs and harness wiring depend on deployed Letta and Letta Code versions; confirm against your images and OpenAPI.

**Concept model change:** A **Bear** remains the primary user-facing assistant identity, but in BEARS that identity is implemented through coordinated role agents rather than a single all-purpose runtime. Bear configuration must therefore evolve to include both the Bear’s role-agent topology and any **predefined subagents**—for example **reflection** agents or other Letta subagent types the operator enables per Bear—so provisioning and GitOps remain reproducible.

**Relationship to other ADRs and concepts:** [multi-user-memory.md](multi-user-memory.md) covers **blocks and conversations**; this ADR covers **skills lifecycle**, **storage/sync**, and **subagent** topology. It should be read alongside [semantic-bear-memory.md](semantic-bear-memory.md), [bear-memory-tool-boundary.md](bear-memory-tool-boundary.md), and the concepts docs for [Capabilities and Skills](../../concepts/CAPABILITIES_AND_SKILLS.md) and [Memory model](../../concepts/MEMORY_MODEL.md). Cabinet (Outline) remains the long-lived shared knowledge layer in later phases ([PLAN.md](../../planning/PLAN.md)).

---

## Decision

1. **Runtime ownership:** **Letta Code** remains the harness that loads skills, runs tool loops, and brokers subagent execution; **Letta** persists agent state. Den does **not** implement skill execution or reflection logic in Rust.

2. **Skills are special memory:** BEARS treats durable skills as a **special class of Bear memory artifact** rather than as anonymous host-local files. Skills remain filesystem-compatible for Letta Code, but their canonical identity, governance, and lifecycle belong to the Bear memory model.

3. **Canonical durable storage:** The canonical durable storage location for Bear skills is the Bear’s MemFS **`skills/`** namespace.

4. **Canonical bundle shape:** Each canonical Bear skill is a flat skill bundle under `skills/<skill-slug>/` containing:
   - **`SKILL.md`** — portable skill content, kept as close as practical to ecosystem conventions
   - **`bears.yaml`** — BEARS-specific metadata for governance, readiness, applicability, provenance, dependencies, sharing, and sync state

5. **No semantic path hierarchy:** The `skills/` namespace is flat at the skill-id level. BEARS must not encode authoritative role assignment, lifecycle state, provenance, or sharing semantics in directory trees such as role-based, topic-based, or status-based subdirectories. Per-skill directories are packaging containers, not semantic hierarchy.

6. **`SKILL.md` stays portable:** BEARS should avoid putting substantial BEARS-specific governance metadata into `SKILL.md` when a sidecar can express it cleanly. This preserves compatibility with external skill ecosystems and lets imported skills remain close to upstream form.

7. **`bears.yaml` is authoritative for BEARS metadata:** The authoritative BEARS-specific metadata for a skill lives in `bears.yaml`, not in the directory path and not primarily in `SKILL.md`.

8. **Sync/materialization model:** Letta-compatible skill trees under `.letta/` or agent-scoped runtime paths are **materialized views**, not canonical storage. Den and the harness may sync canonical Bear MemFS skills into the runtime layout Letta Code expects, but the runtime filesystem copy is a projection of canonical Bear skill memory.

9. **Catalog vs dynamic skills:** **Den** stays the **system of record** for which catalog skills are attached to which bear and for policy/governance metadata. Catalog, imported, and Bear-authored skills all converge on the same Bear skill model once adopted. Workspace-local or runtime-local skill files may exist as overlays or staging inputs, but they are not the canonical durable store unless explicitly imported into Bear memory.

10. **Role assignment is metadata-driven:** Skills may apply to one, many, or all roles. BEARS will not encode authoritative role assignment in directory structure. Role applicability, restrictions, and presentation are metadata and Den policy concerns.

11. **Dependencies are explicit:** Skills must be able to declare environmental dependencies and assumptions, including runtime/tool availability, terminal access, model/tool permissions, network access, and other prerequisites. Runtime selection should consider these declarations before surfacing or invoking a skill.

12. **Curation and review:** Durable skill changes must remain reviewable. Other roles may draft or propose skills from lived work, but `curate` is responsible for review, normalization, deduplication, promotion, deprecation, and broader sharing decisions unless a future dedicated skill-review lane supersedes part of that responsibility.

13. **Subagents in bear configuration:** **Den’s bear model** and **provision/update** path must include **predefined subagent configuration**—at minimum which **subagent types** (e.g. `reflection`) are enabled and any **parameters** or **templates** Letta’s API requires. Exact wire schema is implementation-specific; the architectural requirement is that subagent configuration be Den-managed and reproducible.

14. **Security:** Dynamic skills are **trusted code** adjacent to the agent; subagents multiply capability surface. **Operator policy** (who may enable reflection, catalog publish rules, size caps, import review, sharing policy) stays in Den; execution policy stays aligned with Letta/Letta Code docs.

15. **Human / user control:** Automation should keep **people in the loop** for what merges into durable, deploy-visible skill trees—**not** “as little change as possible by default.” The expert sketch below leans **conservative** (bias toward `NO_ACTION`, branch-only writes). BEARS may adopt different thresholds per org: the invariant is **user and operator agency** over skill promotion, not maximal conservatism.

16. **Routines excluded by default:** **Scheduled / unattended** runs ([routines-automation.md](routines-automation.md)) **must not** feed automatic skill-curator or reflection learning loops unless explicitly designed. That preserves **user control** and avoids polluting skills from background jobs.

---

## Canonical skill bundle format

Canonical Bear-managed skills should live under a flat MemFS namespace such as:

```text
skills/
  adr-drafting/
    SKILL.md
    bears.yaml
  repo-orientation/
    SKILL.md
    bears.yaml
  pr-review/
    SKILL.md
    bears.yaml
```

This ADR does **not** require directory names alone to determine full skill semantics. The bundle directory provides a stable skill container. Authoritative semantics must come from `bears.yaml`.

### Why a sidecar

BEARS needs metadata that is important for governance and runtime selection but is not necessarily portable across external skill ecosystems. Using a visible sidecar keeps:
- `SKILL.md` close to ecosystem conventions,
- BEARS-specific metadata explicit and inspectable,
- imported skills easier to preserve in near-upstream form,
- readiness and review state highly visible.

---

## `bears.yaml` v1

`bears.yaml` is the BEARS-specific metadata sidecar for a skill bundle.

A canonical Bear skill bundle contains:
- `SKILL.md` — portable skill content
- `bears.yaml` — BEARS-specific metadata for governance, applicability, provenance, dependencies, sharing, and sync

### Required fields

```yaml
id: adr-drafting
title: ADR Drafting
summary: Draft and revise architecture decision records in BEARS style.

lifecycle:
  status: approved

roles:
  allowed:
    - pair

dependencies:
  required:
    tools:
      - fs.read

sharing:
  shareable: true

sync:
  materialize_to_letta: true
```

### Recommended full example

```yaml
id: adr-drafting
title: ADR Drafting
summary: Draft and revise architecture decision records in BEARS style.

version: 1

lifecycle:
  status: approved
  deprecated: false
  supersedes: []
  superseded_by: null

roles:
  allowed:
    - pair
    - curate
  default:
    - pair

review:
  status: approved
  reviewed_by_role: curate
  reviewed_at: 2026-05-19T00:00:00Z
  notes: |
    Approved for Bear-local use after review of repeated ADR drafting work.

provenance:
  source_kind: bear_local
  source_bear_id: 006b37be-455d-4471-b040-a4fe99358cb5
  imported_from: null
  derived_from: []

dependencies:
  required:
    tools:
      - fs.read
      - fs.write
    permissions:
      - workspace_read
    environment:
      repo_required: true
      terminal_required: false
      browser_required: false
      network_required: false
  optional:
    tools:
      - git.diff
      - search.files
    permissions:
      - workspace_write
    environment:
      git_repo_preferred: true

sharing:
  shareable: true
  exportable: true
  visibility: bear_local

sync:
  materialize_to_letta: true
  materialization_targets:
    - letta_code
  last_synced_at: 2026-05-19T00:00:00Z
  sync_state: in_sync

tags:
  - architecture
  - writing
  - adr
```

### Field groups

#### Identity
- `id` — stable machine id, normally matching the bundle directory name
- `title` — human-readable title
- `summary` — short one-line description
- `version` — schema version for `bears.yaml`, not skill revision history

#### Lifecycle
```yaml
lifecycle:
  status: approved
  deprecated: false
  supersedes: []
  superseded_by: null
```

Recommended `lifecycle.status` values:
- `draft`
- `proposed`
- `approved`
- `deprecated`
- `archived`

#### Roles
```yaml
roles:
  allowed:
    - pair
    - curate
  default:
    - pair
```

- `allowed` — roles this skill may be surfaced to
- `default` — roles this skill should normally be surfaced to when otherwise eligible

#### Review
```yaml
review:
  status: approved
  reviewed_by_role: curate
  reviewed_at: 2026-05-19T00:00:00Z
  notes: |
    Approved for Bear-local use after review of repeated ADR drafting work.
```

Recommended `review.status` values:
- `unreviewed`
- `in_review`
- `approved`
- `rejected`

`review` is distinct from `lifecycle`: lifecycle is the product state of the skill, while review is the governance state of its evaluation.

#### Provenance
```yaml
provenance:
  source_kind: bear_local
  source_bear_id: 006b37be-455d-4471-b040-a4fe99358cb5
  imported_from: null
  derived_from: []
```

Recommended `source_kind` values:
- `bear_local`
- `catalog`
- `external_import`
- `shared_from_bear`
- `workspace_local`
- `runtime_generated`

#### Dependencies
```yaml
dependencies:
  required:
    tools:
      - fs.read
    permissions:
      - workspace_read
    environment:
      repo_required: true
  optional:
    tools:
      - git.diff
    permissions:
      - workspace_write
    environment:
      git_repo_preferred: true
```

Dependency groups should distinguish:
- `required`
- `optional`

Within each group, v1 supports:
- `tools`
- `permissions`
- `environment`

Suggested `environment` keys include:
- `repo_required`
- `workspace_required`
- `git_repo_preferred`
- `terminal_required`
- `browser_required`
- `network_required`

#### Sharing
```yaml
sharing:
  shareable: true
  exportable: true
  visibility: bear_local
```

Suggested `visibility` values:
- `bear_local`
- `workspace`
- `org`
- `public`
- `restricted`

#### Sync
```yaml
sync:
  materialize_to_letta: true
  materialization_targets:
    - letta_code
  last_synced_at: 2026-05-19T00:00:00Z
  sync_state: in_sync
```

Suggested `sync.sync_state` values:
- `pending`
- `in_sync`
- `drifted`
- `error`
- `not_materialized`

#### Tags
```yaml
tags:
  - architecture
  - writing
  - adr
```

Optional, but useful for discovery and curation.

---

## Metadata over directory semantics

The following concerns must be modeled in `bears.yaml`, not in filesystem hierarchy:

- allowed or preferred roles,
- lifecycle state,
- review state,
- source or import provenance,
- shareability and export policy,
- dependency declarations,
- replacement/supersession,
- sync/materialization state.

---

## Sync directions

The expected sync directions are:

1. **Catalog → Bear skill memory** when a Bear adopts a catalog skill.
2. **Workspace/runtime/local draft → Bear skill memory** when an external or temporary skill is explicitly imported or promoted.
3. **Bear skill memory → Letta runtime layout** when Den or the harness materializes skills for Letta Code execution.

The runtime copy may be regenerated. The canonical Bear MemFS bundle should be the source used for governance, review, sharing, and long-term retention.

### Drift handling

Drift between canonical Bear skill memory and runtime materializations should be treated as operational state, not a competing source of truth.

Recommended behavior:
- Bear MemFS remains canonical.
- Runtime trees may be regenerated from canonical bundles.
- Detected runtime-only edits should either:
  - be discarded during reconciliation, or
  - be explicitly imported back through a review/promotion path.
- Sync state should be visible in `bears.yaml` and/or Den-managed operational state.

---

## Sharing and portability

Inter-Bear sharing should preserve provenance. Importing a skill from another Bear or from an external catalog should create a Bear-local skill bundle with:
- preserved or translated `SKILL.md`,
- a local `bears.yaml`,
- origin metadata,
- trust/review state,
- any compatibility notes required for local use.

Sharing should be first-class at the metadata/policy level, while actual transfer may still happen through import/export workflows.

---

## Curate responsibilities for skills

`curate` should have primary responsibility for durable skill governance, including:

- reviewing proposed skills and revisions,
- checking whether the behavior is truly reusable,
- normalizing metadata and structure,
- assigning or confirming role applicability,
- validating dependency declarations,
- identifying duplication or conflict with existing skills,
- deciding whether a workspace-local or imported skill should be promoted into Bear memory,
- deciding whether a Bear-local skill is eligible for inter-Bear sharing,
- marking skills as approved, provisional, deprecated, superseded, or archived.

This extends the broader memory-governance role of `curate` into the skill domain.

---

## Environmental dependency model

Skills should declare the environment they expect. Relevant dependency categories include:

- required tools or tool classes,
- required permission modes,
- terminal access,
- browser access,
- network availability,
- runtime or language availability,
- repo/workspace presence,
- role restrictions,
- approval prerequisites.

These declarations should support both user-facing availability states and runtime filtering/selection.

---

## Inspirational wiring (Letta expert sketch)

**Not normative.** The following is a **concrete pattern** shared by a Letta expert: a **`skill-curator`** custom subagent, a **memory block** on the primary agent to invoke it via `Task()`, an optional **`SubagentStop` hook** chaining reflection → curator, and a **git-based human review** loop. Paths (`.letta/agents/`, `.letta/skills/`, `settings.json` hooks) follow **Letta / Letta Code** conventions—confirm against your deployed version.

BEARS should read this sketch through the storage and sync decisions above:

- `.letta/skills/` is best treated as a **runtime materialization target** or staging area, not the canonical durable store.
- Git branches may still be useful for review of materialized skill trees or exported skill bundles, but BEARS does not require canonical skill truth to live only in a git-tracked runtime directory.
- Any automated skill-curation flow should ultimately produce or update canonical Bear skill memory, then sync outward to runtime layouts as needed.

### Product stance (BEARS)

The expert framed the curator as **conservative** (minimal noise, `NO_ACTION` bias). In BEARS, **“conservative” is not the product goal**—**users and operators staying in control** of promoted skills is. The same mechanism (staged branches, no direct writes to `main`, optional hooks) supports **strong review** without requiring **minimal** skill updates. Policy knobs (thresholds, when to invoke `Task`) are tunable per deployment and may eventually surface in **Den** as org policy.

### Summary of the pattern

| Piece | Role |
|-------|------|
| **`skill-curator` subagent** (`.letta/agents/skill-curator.md` or global `~/.letta/agents/`) | Reviews trajectory; drafts or edits skills for later promotion into canonical Bear skill memory; may use `.letta/skills/` or another staging area as a runtime working tree; **does not merge** durable changes without review. |
| **Primary-agent memory block** (`system/skill-learning.md` or similar) | Instructs when to `Task(subagent_type="skill-curator", …)` (e.g. after coherent multi-tool work; not mid-debug). |
| **Optional `SubagentStop` hook** (`settings.json`) | After **`reflection`** subagent stops, prompt the primary to optionally invoke the curator if patterns warrant it. |
| **Human review** | Operator or curator reviews proposed changes, whether in git, in Den, or in Bear memory review tooling, then promotes or discards them. |

### Expert: known, assumed, unknown

- **Known:** `.letta/agents/<name>.md` with YAML frontmatter is a documented **custom subagent** format; `tools` as comma-separated or `all`; `model` can inherit or override; **`SubagentStop`** is a real hook event with **`matcher`** support.
- **Assumed:** A git-tracked runtime skills directory can still be a useful review surface even if it is not canonical storage.
- **Unknown:** Whether `SubagentStop` **`matcher`** matches the built-in **`reflection`** subagent **by name** vs only **tool** names—the expert suggests **verify with a logging hook**; if it never fires, use another hook (e.g. `Stop`) and gate on trajectory length.

### Example: `skill-curator` subagent spec (expert draft)

````markdown
---
name: skill-curator
description: Reviews recent conversation trajectory and drafts or refines skills when repeated patterns or missed gotchas warrant it. Invoked proactively after non-trivial work or explicitly via Task(). Does NOT merge changes — always stages proposals for review.
tools: Read, Write, Edit, Glob, Grep, Bash
model: auto
---

You are a skill curator. Identify when the primary agent has demonstrated a repeatable pattern worth capturing, or hit a pitfall worth documenting.

# Evaluation criteria (propose a change ONLY if one is clearly true)

1. The agent executed a procedure that has now appeared 2+ times across recent sessions, and no existing skill captures it.
2. The agent hit a failure mode or gotcha that an existing skill should have warned about.
3. An existing skill led the agent astray (wrong order, missing prerequisite, stale reference).

If none apply, output exactly `NO_ACTION: <one-line reason>` and exit. Bias toward NO_ACTION — noise is worse than a missed skill.

# Procedure

1. Read the prompt (recent conversation excerpt + any notes from the primary agent).
2. Enumerate canonical and materialized skills.
3. Load the `creating-skills` skill before drafting new ones.
4. Draft the change in the approved staging surface.
5. Preserve metadata for provenance, role applicability, dependencies, and review state.
6. Do not directly promote durable changes without review.
7. Output a structured summary for review.
````

### Example: primary-agent memory block (expert draft)

```markdown
## Skill learning policy

At natural pause points (task completes, user says "thanks", topic shifts), if the just-completed work involved 3+ tool calls toward a coherent outcome, invoke:
Task(subagent_type="skill-curator",
         prompt="Review the last ~20 messages. Propose skill changes only if the evaluation criteria in your system prompt clearly apply.")

Do NOT invoke during active debugging or mid-task. Do NOT invoke on casual conversation. The curator will stage or record proposals for review; you do not need to review its output further unless the user asks.
```

### Example: optional `SubagentStop` hook (expert draft)

```json
{
  "hooks": {
    "SubagentStop": [
      {
        "matcher": "reflection",
        "hooks": [
          {
            "type": "prompt",
            "prompt": "If the reflection subagent identified any repeated patterns or failure modes, invoke Task(subagent_type='skill-curator', prompt='Review the reflection output and the last ~20 messages.') Otherwise do nothing."
          }
        ]
      }
    ]
  }
}
```

### Tuning knobs (expert)

- **"2+ times"** in evaluation criteria — raise to 3+ if noise is too high (or lower if product prefers more capture; ties to **user control** stance).
- **"3+ tool calls"** in primary policy — raise to bias toward fewer curator invocations.
- **`model: auto-fast`** on curator — cheaper triage; escalate for drafting only when needed.
- **Git-backed review surface** — useful where operators prefer branch/PR review, but not required as canonical storage.

### Relationship to Den / BEARS

- **Materialization:** Den continues to lay down **catalog** skills and paths Letta Code reads, but canonical durable storage for adopted Bear skills is Bear MemFS `skills/`.
- **Sync:** Runtime views under `.letta/` or agent-scoped directories should be derived from canonical skill memory and reconciled when skill memory changes.
- **Future:** Den may record which subagent templates (`skill-curator`, `reflection`) are enabled per bear, template memory block text, skill review state, and sync status between canonical memory and runtime layouts.

---

## Consequences

- **Den schema / APIs:** Extend `bears` (or related tables) and provisioning payloads to store **subagent configuration**, canonical skill metadata, role applicability, dependency declarations, provenance, and sync/materialization state.

- **Memory model alignment:** Skills become a governed Bear-memory concept rather than only a runtime-filesystem concern. Existing conceptual docs that sharply separate skills from memory should be updated to reflect that skills are a special memory artifact.

- **Runtime integration:** Harness materialization (`letta-code.yaml` or equivalent) should treat Letta-visible skill trees as synchronized projections of canonical Bear skill memory.

- **Documentation:** [DEN_ARCHITECTURE.md](../DEN_ARCHITECTURE.md), [PLAN.md](../../planning/PLAN.md), and concepts docs should reference this ADR; terminology distinguishes **primary agent (bear)** from **subagents** and **canonical skill memory** from **runtime skill trees**.

- **Remaining follow-up:** confirm exact self-hosted Letta subagent API fields and verify whether `SubagentStop` + `matcher: reflection` behaves as expected in the deployed build. These are implementation validation tasks, not unresolved architectural direction.

---

## References

- [Den-managed skills](../DEN_ARCHITECTURE.md#den-managed-skills)
- [Capabilities and Skills](../../concepts/CAPABILITIES_AND_SKILLS.md)
- [Memory model](../../concepts/MEMORY_MODEL.md)
- [Agent Skills open standard](https://agentskills.io/)
- Letta Code skills: [Letta Code — Skills](https://docs.letta.com/letta-code/skills/) (verify against your version)
