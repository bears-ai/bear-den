# Dynamic skills, reflection subagents, and bear configuration — Architecture Decision Record

## Status: Proposed

*Expert inspirational sketch recorded below (2026-04-18); not a binding implementation spec until reviewed.*

## Date: 2026-04-18

---

## Context

**End goal:** Skills should be **dynamic**:

- Operators (and users, where policy allows) attach **predefined skills** from a **catalog** (existing [Den-managed skills](../DEN_ARCHITECTURE.md#den-managed-skills) pattern).
- A **bear** can also **create and refine its own** skills over time (procedural memory that evolves).

**Upstream capability (Letta / Letta Code):** Recent upstream features align with this without Den reimplementing a skill runtime:

- **Letta Code** can ship with a **skills-creation** skill that enables agents to author or extend filesystem skills in supported layouts.
- **Letta** exposes a **`reflection` subagent type** (and related mechanisms) that can drive **auto-discovery** and structured improvement loops—exact APIs and harness wiring depend on deployed Letta and Letta Code versions; confirm against your images and OpenAPI.

**Concept model change:** A **bear** is still the primary user-facing assistant (one Letta **primary** agent in Den’s registry), but **bear configuration** must evolve to include **predefined subagents**—for example **reflection** agents or other Letta subagent types the operator enables per bear—so provisioning and GitOps remain reproducible.

**Relationship to other ADRs:** [multi-user-memory.md](multi-user-memory.md) covers **blocks and conversations**; this ADR covers **skills lifecycle** and **subagent** topology. Cabinet (Outline) remains the long-lived shared knowledge layer in later phases ([PLAN.md](../../planning/PLAN.md)).

---

## Decision (preliminary)

1. **Runtime ownership:** **Letta Code** remains the harness that loads skills, runs tool loops, and brokers subagent execution; **Letta** persists agent state. Den does **not** implement skill execution or reflection logic in Rust.

2. **Catalog vs dynamic skills:** **Den** stays the **system of record** for **which catalog skills** are attached to which bear (materialization unchanged in spirit). **Bear-authored** and **improved** skills live where **Letta Code / Letta** write them (typically under agent-scoped paths on shared volumes); Den may **surface** existence/size in **bear detail** (read-only assurance) when APIs or filesystem layout allow—same spirit as memory visibility ([PHASE1_DECISIONS.md](../../planning/PHASE1_DECISIONS.md) decisions 7–8).

3. **Subagents in bear configuration:** **Den’s bear model** and **provision/update** path must include **predefined subagent configuration**—at minimum which **subagent types** (e.g. `reflection`) are enabled and any **parameters** or **templates** Letta’s API requires. Exact schema is **TBD**; use [§ Inspirational wiring](#inspirational-wiring-letta-expert-sketch) as non-binding input.

4. **Security:** Dynamic skills are **trusted code** adjacent to the agent; subagents multiply capability surface. **Operator policy** (who may enable reflection, catalog publish rules, size caps) stays in Den; execution policy stays aligned with Letta/Letta Code docs.

5. **Human / user control:** Automation should keep **people in the loop** for what merges into durable, deploy-visible skill trees—**not** “as little change as possible by default.” The expert sketch below leans **conservative** (bias toward `NO_ACTION`, branch-only writes). BEARS may adopt different thresholds per org: the invariant is **user and operator agency** over skill promotion, not maximal conservatism.

6. **Routines excluded by default:** **Scheduled / unattended** runs ([routines-automation.md](routines-automation.md)) **must not** feed automatic skill-curator or reflection learning loops unless explicitly designed. That preserves **user control** and avoids polluting skills from background jobs.

---

## Inspirational wiring (Letta expert sketch)

**Not normative.** The following is a **concrete pattern** shared by a Letta expert: a **`skill-curator`** custom subagent, a **memory block** on the primary agent to invoke it via `Task()`, an optional **`SubagentStop` hook** chaining reflection → curator, and a **git-based human review** loop. Paths (`.letta/agents/`, `.letta/skills/`, `settings.json` hooks) follow **Letta / Letta Code** conventions—confirm against your deployed version.

### Product stance (BEARS)

The expert framed the curator as **conservative** (minimal noise, `NO_ACTION` bias). In BEARS, **“conservative” is not the product goal**—**users and operators staying in control** of promoted skills is. The same mechanism (staged branches, no direct writes to `main`, optional hooks) supports **strong review** without requiring **minimal** skill updates. Policy knobs (thresholds, when to invoke `Task`) are tunable per deployment and may eventually surface in **Den** as org policy.

### Summary of the pattern

| Piece | Role |
|-------|------|
| **`skill-curator` subagent** (`.letta/agents/skill-curator.md` or global `~/.letta/agents/`) | Reviews trajectory; drafts or edits skills under `.letta/skills/`; **does not merge**—stages on a **git branch** for review. |
| **Primary-agent memory block** (`system/skill-learning.md` or similar) | Instructs when to `Task(subagent_type="skill-curator", …)` (e.g. after coherent multi-tool work; not mid-debug). |
| **Optional `SubagentStop` hook** (`settings.json`) | After **`reflection`** subagent stops, prompt the primary to optionally invoke the curator if patterns warrant it. |
| **Human review** | Operator uses `git branch`, `git log`, `git diff`, merge or discard—optionally **branch protection** on a dedicated skills repo / submodule. |

### Expert: known, assumed, unknown

- **Known:** `.letta/agents/<name>.md` with YAML frontmatter is a documented **custom subagent** format; `tools` as comma-separated or `all`; `model` can inherit or override; **`SubagentStop`** is a real hook event with **`matcher`** support.
- **Assumed:** Skills directory is **git-tracked** (staging branches are the review surface). If not, fallback: write to `.letta/skills-staging/<timestamp>/` and diff against `.letta/skills/`.
- **Unknown:** Whether `SubagentStop` **`matcher`** matches the built-in **`reflection`** subagent **by name** vs only **tool** names—the expert suggests **verify with a logging hook**; if it never fires, use another hook (e.g. `Stop`) and gate on trajectory length.

### Example: `skill-curator` subagent spec (expert draft)

````markdown
---
name: skill-curator
description: Reviews recent conversation trajectory and drafts or refines skills when repeated patterns or missed gotchas warrant it. Invoked proactively after non-trivial work or explicitly via Task(). Does NOT merge changes — always stages on a branch for human review.
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
2. Enumerate existing skills: `Glob` on `~/.letta/skills/**/SKILL.md` and `.letta/skills/**/SKILL.md`.
3. Load the `creating-skills` skill before drafting new ones.
4. Create a staging branch in the skills repo: `git checkout -b skill-curator/$(date +%Y%m%d-%H%M%S)`.
5. Make the change (NEW skill or Edit to existing). Keep SKILL.md under 500 lines; split into `references/` if longer.
6. Commit with message: `skill(<name>): <verb> — <one-line reason>`. Include trajectory evidence in the body (which turns triggered this).
7. Do NOT checkout main. Do NOT merge. Do NOT push unless a remote is configured for skills.
8. Output a structured summary (see Output format).

# Hard constraints

- Only write under `.letta/skills/` or `~/.letta/skills/`.
- Never delete a skill. Mark as `deprecated: true` in frontmatter instead.
- Never modify SKILL.md on `main`/default branch directly.
- If a proposed skill would duplicate an existing one, edit the existing one instead.
- If uncertain whether a pattern is a skill or just a one-off, output NO_ACTION.

# Output format

```
BRANCH: skill-curator/20260418-0022
CHANGES:
  - NEW: skills/deploying-to-railway/SKILL.md  (agent redeployed with same 5-step flow 3x)
  - EDIT: skills/processing-pdfs/SKILL.md      (added Gotchas section: encrypted PDFs)
RATIONALE: <2-3 sentences>
NEXT STEP: Review with `git log skill-curator/... --stat` then merge or discard.
```
````

### Example: primary-agent memory block (expert draft)

```markdown
## Skill learning policy

At natural pause points (task completes, user says "thanks", topic shifts), if the just-completed work involved 3+ tool calls toward a coherent outcome, invoke:
Task(subagent_type="skill-curator",
         prompt="Review the last ~20 messages. Propose skill changes only if the evaluation criteria in your system prompt clearly apply.")

Do NOT invoke during active debugging or mid-task. Do NOT invoke on casual conversation. The curator will stage changes on a branch — you do not need to review its output further unless the user asks.
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

### Example: human review commands (expert draft)

```bash
cd ~/.letta/skills   # or wherever your skills live, ideally a git repo
git branch --list 'skill-curator/*'
git log skill-curator/20260418-0022 --stat
git diff main..skill-curator/20260418-0022
# merge:   git checkout main && git merge --no-ff skill-curator/...
# discard: git branch -D skill-curator/...
```

### Tuning knobs (expert)

- **"2+ times"** in evaluation criteria — raise to 3+ if noise is too high (or lower if product prefers more capture; ties to **user control** stance).
- **"3+ tool calls"** in primary policy — raise to bias toward fewer curator invocations.
- **`model: auto-fast`** on curator — cheaper triage; escalate for drafting only when needed.
- **Separate skills repo + branch protection** — PR review at the git host.

### Relationship to Den / BEARS

- **Materialization:** Den continues to lay down **catalog** skills and paths Letta Code reads; **bear-local** or **staged** branches may coexist with org GitOps—**merge policy** is human/operator (and optionally CI), not Den’s chat path.
- **Future:** Den might record **which** subagent templates (`skill-curator`, `reflection`) are enabled per bear and template **memory block** text—still **inspiration-level** until schemas exist.

---

## Consequences

- **Den schema / APIs:** Extend `bears` (or related tables) and provisioning payloads to store **subagent configuration**; extend operator **bear detail** and harness materialization (`letta-code.yaml` or equivalent) so the harness and Letta receive a **reproducible** definition.

- **Documentation:** [DEN_ARCHITECTURE.md](../DEN_ARCHITECTURE.md) and [PLAN.md](../../planning/PLAN.md) reference this ADR; terminology distinguishes **primary agent (bear)** from **subagents**.

- **Open questions:** Exact REST fields for subagents on self-hosted Letta; whether reflection runs in-process or as separate agent records; backup scope for agent-local skill files on volumes; **verify** `SubagentStop` + `matcher: reflection` on your build; map **.letta/** paths to Coolify volume layouts for BEARS.

---

## References

- [Den-managed skills](../DEN_ARCHITECTURE.md#den-managed-skills)
- [Agent Skills open standard](https://agentskills.io/)
- Letta Code skills: [Letta Code — Skills](https://docs.letta.com/letta-code/skills/) (verify against your version)
