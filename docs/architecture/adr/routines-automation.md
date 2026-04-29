# Routines and scheduled work — Architecture Decision Record

## Status: Proposed

## Date: 2026-04-18

---

## Context

**Idea 5 (automation):** BEARS needs **scheduled / unattended** work—briefings, checks, reminders—analogous to upstream [Letta Code scheduling](https://docs.letta.com/letta-code/scheduling) and host-level cron, but with a **first-class** operator and user story in **Den**.

This ADR records **decided** product rules for routines. **File outputs** from runs are stored per [artifacts-garage.md](artifacts-garage.md) (Garage / S3, **not** Letta). **Browsing / notification UX** for those links may still trail schedule + storage.

---

## Decisions (locked)

1. **Phase 1 scope:** **First-class routines in Den** — DB-backed **schedules**, operator (and where appropriate end-user) **UI** to define and manage them—not “docs-only” or “cron outside Den only.” See [PHASE1_DECISIONS.md](../../planning/PHASE1_DECISIONS.md) decision **10**.

2. **Bear assignment:** Every routine runs in the context of an **assigned bear** (one Letta primary agent). **Tool access, model policy, and MCP/skills attachments** are **inherited** from that bear—the same as interactive chat for that bear.

3. **Membership:** **Same as the assigned bear** — only users who may use the bear may **see or be notified by** routine outcomes once delivery exists; enforcement details follow whatever API/UI Den ships for routines.

4. **No spurious learning from background runs:** A bear **must not** treat **routine / scheduled / background** integrations as a source for **automatic skill creation or skill-curator-style learning** without explicit product design. **User control** over what gets promoted ([dynamic-skills-subagents.md](dynamic-skills-subagents.md)) implies **excluding** or **gating** reflection/skill-learning hooks on unattended runs until deliberately specified.

---

## Resolved: file outputs → Garage artifacts

**Binary and large** routine outputs are **not** stored in Letta. They are **S3 objects** in the **artifacts bucket** ([artifacts-garage.md](artifacts-garage.md)), with metadata including **`conversation_id`**, **`bear_id`**, **`routine_id`**, and provenance. **Garbage collection** applies per artifact policy—not the Cabinet bucket.

**Optional:** A Letta **conversation** may still hold **references** (message text pointing at artifact keys); that is harness/UI design, not the storage of bytes.

---

## Consequences

- **Schema:** New tables (or equivalent) for `routine`, schedule expression, `bear_id` FK, enabled flag, audit fields; optional `last_run`, `next_run` for UI; optional links to **artifact** keys for each run.
- **Harness:** Letta Code remains the **execution** path for agent turns triggered by Den or timer; exact **invoke** shape (HTTP to harness, internal queue, etc.) belongs in implementation notes and deploy docs.
- **Object storage:** Den (or tools) **upload** routine file outputs to Garage using the same artifact pipeline as chat- and skill-generated files.
- **Cross-link:** [PLAN.md](../../planning/PLAN.md) § Routines and scheduled work; [artifacts-garage.md](artifacts-garage.md).

---

## References

- [PLAN.md — Routines and scheduled work (Idea 5)](../../planning/PLAN.md#routines-and-scheduled-work-phase-1-idea-5)
- [Letta Code — Scheduling](https://docs.letta.com/letta-code/scheduling)
- [DEN_ARCHITECTURE.md](../DEN_ARCHITECTURE.md) — harness layer
