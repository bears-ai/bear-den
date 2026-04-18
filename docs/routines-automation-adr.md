# Routines and scheduled work — Architecture Decision Record

## Status: Proposed

## Date: 2026-04-18

---

## Context

**Idea 5 (automation):** BEARS needs **scheduled / unattended** work—briefings, checks, reminders—analogous to upstream [Letta Code scheduling](https://docs.letta.com/letta-code/scheduling) and host-level cron, but with a **first-class** operator and user story in **Den**.

This ADR records **decided** product rules and **open** design questions. Full implementation of **output delivery** may trail schedule definition and execution wiring until the open questions close.

---

## Decisions (locked)

1. **Phase 1 scope:** **First-class routines in Den** — DB-backed **schedules**, operator (and where appropriate end-user) **UI** to define and manage them—not “docs-only” or “cron outside Den only.” See [PHASE1_DECISIONS.md](planning/PHASE1_DECISIONS.md) decision **10**.

2. **Bear assignment:** Every routine runs in the context of an **assigned bear** (one Letta primary agent). **Tool access, model policy, and MCP/skills attachments** are **inherited** from that bear—the same as interactive chat for that bear.

3. **Membership:** **Same as the assigned bear** — only users who may use the bear may **see or be notified by** routine outcomes once delivery exists; enforcement details follow whatever API/UI Den ships for routines.

4. **No spurious learning from background runs:** A bear **must not** treat **routine / scheduled / background** integrations as a source for **automatic skill creation or skill-curator-style learning** without explicit product design. **User control** over what gets promoted ([dynamic-skills-subagents-adr.md](dynamic-skills-subagents-adr.md)) implies **excluding** or **gating** reflection/skill-learning hooks on unattended runs until deliberately specified.

---

## Open design: where routine results live

**Not decided — do not implement a single delivery model until this section is resolved.**

Candidates under discussion:

| Direction | Sketch |
|-----------|--------|
| **Artifacts** | Store routine outputs **outside** normal chat threads (e.g. blob/object or Den rows with metadata), linked to `bear_id` / `routine_id`. |
| **Dedicated conversation** | Each routine (or run) uses a **specific Letta conversation** users open in the web UI like a channel. |
| **Hybrid** | Default conversation transcript **plus** optional attachment of a single **artifact** (report, file summary). |

**Interim:** Den may ship **routine definitions + triggers** (and execution via Letta Code / harness) **before** committing to end-user **browsing** of results, if execution path is validated first—coordinate with [PHASE1_BOOTSTRAP.md](planning/PHASE1_BOOTSTRAP.md) milestones.

---

## Consequences

- **Schema:** New tables (or equivalent) for `routine`, schedule expression, `bear_id` FK, enabled flag, audit fields; optional `last_run`, `next_run` for UI.
- **Harness:** Letta Code remains the **execution** path for agent turns triggered by Den or timer; exact **invoke** shape (HTTP to harness, internal queue, etc.) belongs in implementation notes and deploy docs.
- **Cross-link:** [PLAN.md](planning/PLAN.md) § Routines and scheduled work.

---

## References

- [PLAN.md — Routines and scheduled work (Idea 5)](planning/PLAN.md#routines-and-scheduled-work-phase-1-idea-5)
- [Letta Code — Scheduling](https://docs.letta.com/letta-code/scheduling)
- [DEN_ARCHITECTURE.md](architecture/DEN_ARCHITECTURE.md) — harness layer
