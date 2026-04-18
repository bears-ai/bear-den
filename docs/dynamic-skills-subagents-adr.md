# Dynamic skills, reflection subagents, and bear configuration — Architecture Decision Record

## Status: Proposed

## Date: 2026-04-18

---

## Context

**End goal:** Skills should be **dynamic**:

- Operators (and users, where policy allows) attach **predefined skills** from a **catalog** (existing [Den-managed skills](architecture/DEN_ARCHITECTURE.md#den-managed-skills) pattern).
- A **bear** can also **create and refine its own** skills over time (procedural memory that evolves).

**Upstream capability (Letta / Letta Code):** Recent upstream features align with this without Den reimplementing a skill runtime:

- **Letta Code** can ship with a **skills-creation** skill that enables agents to author or extend filesystem skills in supported layouts.
- **Letta** exposes a **`reflection` subagent type** (and related mechanisms) that can drive **auto-discovery** and structured improvement loops—exact APIs and harness wiring depend on deployed Letta and Letta Code versions; confirm against your images and OpenAPI.

**Concept model change:** A **bear** is still the primary user-facing assistant (one Letta **primary** agent in Den’s registry), but **bear configuration** must evolve to include **predefined subagents**—for example **reflection** agents or other Letta subagent types the operator enables per bear—so provisioning and GitOps remain reproducible.

**Relationship to other ADRs:** [multi-user-memory-adr.md](multi-user-memory-adr.md) covers **blocks and conversations**; this ADR covers **skills lifecycle** and **subagent** topology. Cabinet (Outline) remains the long-lived shared knowledge layer in later phases ([PLAN.md](planning/PLAN.md)).

---

## Decision (preliminary)

1. **Runtime ownership:** **Letta Code** remains the harness that loads skills, runs tool loops, and brokers subagent execution; **Letta** persists agent state. Den does **not** implement skill execution or reflection logic in Rust.

2. **Catalog vs dynamic skills:** **Den** stays the **system of record** for **which catalog skills** are attached to which bear (materialization unchanged in spirit). **Bear-authored** and **improved** skills live where **Letta Code / Letta** write them (typically under agent-scoped paths on shared volumes); Den may **surface** existence/size in **bear detail** (read-only assurance) when APIs or filesystem layout allow—same spirit as memory visibility ([PHASE1_DECISIONS.md](planning/PHASE1_DECISIONS.md) decisions 7–8).

3. **Subagents in bear configuration:** **Den’s bear model** and **provision/update** path must include **predefined subagent configuration**—at minimum which **subagent types** (e.g. `reflection`) are enabled and any **parameters** or **templates** Letta’s API requires. Exact schema is **TBD** and should follow the expert-suggested approach below once integrated.

4. **Security:** Dynamic skills are **trusted code** adjacent to the agent; subagents multiply capability surface. **Operator policy** (who may enable reflection, catalog publish rules, size caps) stays in Den; execution policy stays aligned with Letta/Letta Code docs.

---

## Expert-suggested approach

*This section is intentionally incomplete.* A Letta expert provided a concrete wiring approach (reflection agents, auto-discovery, integration with skills-creation). **Paste the expert’s recommendation here** (verbatim or summarized) and update **Status** to **Accepted** when reviewed.

**Placeholder — to fill in:**

- Recommended **Letta / Letta Code** version constraints or flags.
- How **`reflection`** subagents are created or linked to the primary agent.
- How **skills-creation** interacts with **reflection** (scheduling, triggers, handoff).
- Any **cron**, **Letta Code** scheduling, or **Den** responsibilities (e.g. materialized env only vs Den API calls).

---

## Consequences

- **Den schema / APIs:** Extend `bears` (or related tables) and provisioning payloads to store **subagent configuration**; extend operator **bear detail** and harness materialization (`letta-code.yaml` or equivalent) so the harness and Letta receive a **reproducible** definition.

- **Documentation:** [DEN_ARCHITECTURE.md](architecture/DEN_ARCHITECTURE.md) and [PLAN.md](planning/PLAN.md) reference this ADR; terminology distinguishes **primary agent (bear)** from **subagents**.

- **Open questions (until expert text lands):** Exact REST fields for subagents; whether reflection runs in-process or as separate agent records; backup scope for agent-local skill files on volumes.

---

## References

- [Den-managed skills](architecture/DEN_ARCHITECTURE.md#den-managed-skills)
- [Agent Skills open standard](https://agentskills.io/)
- Letta Code skills: [Letta Code — Skills](https://docs.letta.com/letta-code/skills/) (verify against your version)
