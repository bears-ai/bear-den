# Phase 1 (Den) — locked decisions

**Status:** Active  
**Last updated:** 2026-04-18  
**Context:** Product choices for BEARS Phase 1, aligned with [PHASE1_BOOTSTRAP.md](PHASE1_BOOTSTRAP.md) §17. Decision 4 revised 2026-04-16: no template table; duplicate bear instead. Decision 8 added 2026-04-18: memory dashboard **weight** vs bear-detail **state**.

## Decisions

| # | Area | Choice |
|---|------|--------|
| 1 | Operator console | **Server-rendered** with MiniJinja + forms. **Mobile-first** layout for restraint. **No live updates** required (refresh/redirect OK). **JavaScript:** progressive enhancement only; no SPA baseline. |
| 2 | Chat streaming (`POST /v1/chat/send`) | **SSE** (`text/event-stream`) as the canonical contract; Den chat UI is the reference client; **browsers only** in v1. Deployment uses **Traefik (Coolify)** — ensure the streaming route avoids response buffering and has adequate read/write timeouts. |
| 3 | Public bear identity in JSON | **`bear_id` only** (no `agent_id` alias in v1). **`slug`** included on bears in list/API surfaces where a human-stable handle is needed. Letta’s `letta_agent_id` remains internal and is not exposed in public user APIs unless a separate admin/debug surface is added later. |
| 4 | Letta agent provisioning | **Per-bear customization** — operators set system prompt (and related create fields) on each bear. **No shared template table** in v1; low bear count; use a **duplicate bear** action when two configs should match. |
| 5 | Conversation threading | **Harness-owned, Letta-native conversations** — Den treats conversation/thread identifiers as pass-through context and does **not** become the source of truth for conversation lifecycle in v1. Den forwards stable identity + channel/thread metadata to **Letta Code**, and the harness resolves/creates Letta conversations (including per-channel thread separation as configured). Den may cache hints for UX, but canonical mapping and message posting stay in Letta Code/Letta. |
| 6 | Open WebUI authentication | **Deferred** until optional **M6b**. Revisit: cookie vs server-side API token when domains and Open WebUI integration path are known. |
| 7 | Phase 1 memory model (Idea 3) | **Curated blocks vs findable history:** User-facing promise is **small always-in-context memory blocks** + **longer material retrievable** via Letta **archival** and tools (on-demand retrieval; not “all knowledge in every prompt”). **No Den memory store** in Phase 1 — only Letta APIs. **Scope:** 1:1 per `(user, bear)` for web; no new shared household memory layer in Den. See [PLAN.md](PLAN.md) § Phase 1 memory model. |
| 8 | Phase 1 memory visibility (Idea 2) | **Two UIs:** (1) **Memory dashboard:** **`human`** readout plus a **holistic memory weight** per member bear (cross-bear comparison — which bear has **learned / stored** the most about users, projects, and other Letta-visible memory). Use **weight** framing, **not** “pressure” or proximity to limits; **no** capacity warnings or Den-side consolidation — **assurance and comparison** only; Letta owns memory automation. (2) **Bear detail (operator):** full read-only **Letta state summary** — **all** blocks + **archival** where the API exposes them; prefer **tokens**, else whatever Letta returns. **Not** a management affordance in Den. |
| 9 | Dynamic skills & subagents (Idea 1) | **Goal:** catalog skills **plus** bear-created/improved skills over time, using **Letta Code** (e.g. skills-creation skill) and Letta **`reflection`** (and related) subagents for auto-discovery. **Den** extends **bear configuration** to include **predefined subagents** and keeps catalog attach + materialization as system of record for org skills; runtime remains Letta Code/Letta. See [dynamic-skills-subagents-adr.md](../dynamic-skills-subagents-adr.md) — includes **inspirational** expert sketch (`skill-curator`, hooks, git review); **users/operators in control** of promotion beats “maximal conservatism” as the default product goal. |

## Implementation order (after these decisions)

1. Postgres schema: `users`, `bears`, `user_bear` (and migrations); per-bear `system_prompt`, nullable `letta_agent_id` until provisioned.
2. Auth + operator roles (`is_admin`, bootstrap), rate limits on login.
3. Admin JSON APIs + operator console (MiniJinja): users, bears (create / **duplicate**, provision), membership, harness deploy preview (`letta-code.yaml`).
4. `POST /v1/chat/send` with membership enforcement, **Den → Letta Code** proxy, **SSE**, and pass-through conversation/thread context.
5. Den-hosted **chat UI** at `/bear/{slug}`.
6. Optional **M6b** Open WebUI + deferred auth choice; then polish (rate limits, readiness, deploy notes).

## References

- Phase 1 bootstrap milestones and API sketch: [PHASE1_BOOTSTRAP.md](PHASE1_BOOTSTRAP.md)
- Stack roadmap: [PLAN.md](PLAN.md) (including [Phase 1 memory model](PLAN.md#phase-1-memory-model-user-promise-persistence-and-ux)); decisions **7–9** (memory model + visibility + dynamic skills/subagents)
- Dynamic skills & reflection subagents: [dynamic-skills-subagents-adr.md](../dynamic-skills-subagents-adr.md)
