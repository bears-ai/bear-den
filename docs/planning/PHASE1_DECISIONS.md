# Phase 1 (Den) — locked decisions

**Status:** Active  
**Last updated:** 2026-04-16  
**Context:** Product choices for BEARS Phase 1, aligned with [PHASE1_BOOTSTRAP.md](PHASE1_BOOTSTRAP.md) §17. Decision 4 revised same day: no template table; duplicate bear instead.

## Decisions

| # | Area | Choice |
|---|------|--------|
| 1 | Operator console | **Server-rendered** with MiniJinja + forms. **Mobile-first** layout for restraint. **No live updates** required (refresh/redirect OK). **JavaScript:** progressive enhancement only; no SPA baseline. |
| 2 | Chat streaming (`POST /v1/chat/send`) | **SSE** (`text/event-stream`) as the canonical contract; Den chat UI is the reference client; **browsers only** in v1. Deployment uses **Traefik (Coolify)** — ensure the streaming route avoids response buffering and has adequate read/write timeouts. |
| 3 | Public bear identity in JSON | **`bear_id` only** (no `agent_id` alias in v1). **`slug`** included on bears in list/API surfaces where a human-stable handle is needed. Letta’s `letta_agent_id` remains internal and is not exposed in public user APIs unless a separate admin/debug surface is added later. |
| 4 | Letta agent provisioning | **Per-bear customization** — operators set system prompt (and related create fields) on each bear. **No shared template table** in v1; low bear count; use a **duplicate bear** action when two configs should match. |
| 5 | Conversation threading | **Harness-owned, Letta-native conversations** — Den treats conversation/thread identifiers as pass-through context and does **not** become the source of truth for conversation lifecycle in v1. Den forwards stable identity + channel/thread metadata to **Letta Code**, and the harness resolves/creates Letta conversations (including per-channel thread separation as configured). Den may cache hints for UX, but canonical mapping and message posting stay in Letta Code/Letta. |
| 6 | Open WebUI authentication | **Deferred** until optional **M6b**. Revisit: cookie vs server-side API token when domains and Open WebUI integration path are known. |

## Implementation order (after these decisions)

1. Postgres schema: `users`, `bears`, `user_bear` (and migrations); per-bear `system_prompt`, nullable `letta_agent_id` until provisioned.
2. Auth + operator roles (`is_admin`, bootstrap), rate limits on login.
3. Admin JSON APIs + operator console (MiniJinja): users, bears (create / **duplicate**, provision), membership, harness deploy preview (`letta-code.yaml`).
4. `POST /v1/chat/send` with membership enforcement, **Den → Letta Code** proxy, **SSE**, and pass-through conversation/thread context.
5. Den-hosted **chat UI** at `/bear/{slug}`.
6. Optional **M6b** Open WebUI + deferred auth choice; then polish (rate limits, readiness, deploy notes).

## References

- Phase 1 bootstrap milestones and API sketch: [PHASE1_BOOTSTRAP.md](PHASE1_BOOTSTRAP.md)
- Stack roadmap: [PLAN.md](PLAN.md)
