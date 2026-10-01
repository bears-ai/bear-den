# BearWire armature wire — implementation plan

**Status:** In progress — Phases 0–2 complete; Phase 3 implementation complete, opt-in smoke/parity validation pending  
**Date:** 2026-06-16  
**Last updated:** 2026-06-19  
**Decision:** [ADR-0034: BearWire as the Den ↔ armature wire](../decisions/adr-0034-bearwire-as-den-armature-wire.md)  
**Specs:** [BearWire JSON specification](../architecture/bearwire-json-spec.md), [BearWire Rust design](../architecture/bearwire-rust-design.md)  
**Refinement roadmap:** [BearWire v1 protocol refinement roadmap](BEARWIRE_V1_PROTOCOL_REFINEMENT_ROADMAP.md)

## Purpose

Replace the bespoke **adapter-SSE** + `/acp/**` HTTP dialect between Den and trusted armatures with **BearWire v1** — while keeping user-visible behavior stable and allowing `bears-acp-adapter` to own all ACP stdio translation.

This plan is the delivery checklist for ADR-0034. It assumes the protocol-neutral core work (neutral turn/session names, `GatewayEvent`, golden traces) is either done or in flight; this plan starts at the **wire boundary**.

## End state

```text
Editor ──ACP stdio──► bears-acp-adapter
                           │  BearWire v1 (HTTP RPC + SSE events)
                           ▼
                       den-bearwire edge  (evolved from den-acp)
                           │  RuntimeSemanticEvent / den-runtime
                           ▼
                       agent loop, memory, bears
```

- No adapter-SSE `type` strings on the Den ↔ armature hop.
- `/acp/**` removed or thin-shimmed to BearWire internally.
- `den-acp` renamed or superseded by `den-bearwire` (name optional; boundary is what matters).
- Product/model-facing language uses **Bear stances** (`chat`, `pair`, `curate`, `work`, `watch`) for Bear posture, and reserves **session mode** for `ask` / `plan` / `write` tool-policy state.

## Principles

1. **Semantic parity before transport churn** — lock event mapping with tests before switching the adapter default.
2. **Parallel operation** — legacy `/acp` + adapter-SSE stays until `bears-acp-adapter` ships BearWire client support.
3. **Serializable wire types** — no `oneshot` channels or in-process-only fields on BearWire structs.
4. **Armature owns ACP** — no new ACP-shaped JSON in `den-runtime`.

## Current inventory (baseline)

### HTTP control plane (`/acp/bears/{slug}/…`)

| Route | Role today |
| --- | --- |
| `GET …/sessions` | List session bindings |
| `GET …/sessions/{id}` | Session detail + runtime snapshot |
| `GET …/sessions/{id}/runtime` | Runtime health / waiting state |
| `GET …/sessions/{id}/prompt-memory` | Prompt memory read |
| `POST …/sessions/{id}/prompt` | Start turn (SSE response stream) |
| `POST …/sessions/{id}/tool-results/{tool_call_id}` | Deliver client tool result |
| `POST …/sessions/{id}/permissions/{permission_id}` | Permission decision |
| `POST …/sessions/{id}/mode` | Plan / write mode |
| `POST …/sessions/{id}/adapter-environment` | Adapter capability report |
| `POST …/sessions/{id}/cancel` | Cancel active run |
| `POST …/sessions/{id}/close` | Close session |
| `POST …/sessions/{id}/compact` | Compaction trigger |
| `GET …/conversations` | Conversation list |
| `GET …/conversations/{id}/history` | History |
| `GET …/auth-check` | Token + membership check |

### Internal (not BearWire public)

| Route | Role |
| --- | --- |
| Former `POST /internal/den-tools/invoke` | Historical baseline only. The pre-release branch removed this caller-context HTTP RPC; Den-hosted tools now use the in-process runtime invoker. |

### Event stream (adapter-SSE today)

`gateway_event_to_adapter_sse` in `den-runtime` emits `data: {"type":"<adapter_sse_type>",…}\n\n`.

| Adapter-SSE `type` | Source `GatewayEvent` variant |
| --- | --- |
| `assistant_text_delta` | `AssistantTextDelta` |
| `status_text` | `StatusText` |
| `tool_request` | `ToolRequest` |
| `permission_request` | `PermissionRequest` |
| `turn_complete` | `TurnComplete` |
| `turn_result` | `TurnResult` |
| `error` | `Error` |
| `plan_update` | `PlanUpdate*` |
| `mode_update` | `ModeUpdate` |
| `conversation_resolved` | `ConversationResolved` |
| `session_info_update` | `SessionInfoUpdate` |

## Phase 0 — Mapping lock (no wire change) — Complete

**Goal:** Pin the adapter-SSE → BearWire mapping in tests and docs.

| Task | Owner | Done when |
| --- | --- | --- |
| Add `docs/architecture/bearwire-adapter-sse-migration.md` table (or extend JSON spec §Compatibility) with every adapter-SSE type mapped to BearWire `type` + payload notes | docs | Table reviewed; matches ADR-0034 §6 |
| Add `bearwire_projection/bearwire_wire_migration_tests.rs` in `den-runtime` | runtime | For each golden `RuntimeSemanticEvent` fixture, assert both adapter-SSE JSON and BearWire `event` notification JSON (stub BearWire serializer) |
| Document v1 HTTP+SSE binding in `bearwire-json-spec.md` (§Transport binding — v1 HTTP profile) | docs | Spec describes `POST /bearwire/v1/rpc` + SSE `event` stream |

**Exit gate:** Tests compile; mapping table complete; no production wire change.

## Phase 1 — Wire types + projection in core — Complete

**Goal:** Introduce serializable BearWire wire types and projection from `RuntimeSemanticEvent`.

| Task | Location | Notes |
| --- | --- | --- |
| Add `den-core` or `den-runtime` module `bearwire::wire` | `crates/den-core/src/bearwire/` or `den-runtime/src/bearwire/` | Follow [BearWire Rust design](../architecture/bearwire-rust-design.md): `EventEnvelope`, `EventId`, `RunId`, payload enums |
| Implement `runtime_semantic_event_to_bearwire_events()` | beside existing `bearwire_projection` | Parallel to `runtime_semantic_event_to_bearwire_gateway_events`; no channels on output |
| Implement `bearwire_event_to_json_rpc_notification()` | same module | JSON-RPC `method: "event"` per JSON spec |
| Keep `GatewayEvent` for in-process orchestration only | `gateway_events.rs` | Document as transitional; new armature path bypasses adapter-SSE |
| Golden tests | `golden_traces_tests.rs` + new migration tests | Same scenarios produce equivalent *semantic* outcomes |

**Exit gate:** `cargo test -p den-runtime bearwire` green; golden traces extended.

## Phase 2 — BearWire HTTP edge (parallel to `/acp`) — Complete

**Goal:** Serve BearWire v1 alongside legacy routes.

| Task | Location | Notes |
| --- | --- | --- |
| Add `den-bearwire` crate **or** `den-acp/src/bearwire/` module | edge crate | Prefer new module first; rename crate when `/acp` is gone |
| `POST /bearwire/v1/rpc` — JSON-RPC dispatcher | edge router | Methods: `initialize`, `session.open`, `session.resume`, `session.close`, `session.state`, `run.start`, `run.cancel`, `client.tool.result`, `client.permission.result`, `resource.update` |
| `GET /bearwire/v1/sessions/{session_id}/events/page` — JSON event pages | edge router | Return ordered event page items with server-owned `next_after`; `sequence` monotonic per session |
| Wire auth | reuse `den-oauth` bearer + ACP token paths | Same scopes as `/acp/auth-check` |
| Map handlers to existing den-runtime / session store calls | reuse `den-acp` handler bodies | Thin RPC façade over current logic |
| Binary composition | `services/den/src/lib.rs` | Mount `/bearwire` peer router (same pattern as post-ADR-0043 composition) |
| Feature flag | retired; BearWire mounts with `RUN_API=true` | Den advertises BearWire in `initialize` result when the API listener is enabled |

**Exit gate:** Complete. `den-bearwire` integration coverage now exercises RPC `run.start` with a mock OpenAI-compatible provider, then replays BearWire JSON event pages and asserts `run.accepted`, `run.started`, `message.delta`, and `run.completed`.

## Phase 3 — `bear-armature` BearWire client — Implementation complete; smoke validation pending

**Goal:** Armature speaks BearWire to Den; translates to ACP stdio locally. Legacy binary/package aliases (`bears-acp-adapter`) remain compatibility shims during migration.

| Task | Location | Notes |
| --- | --- | --- |
| BearWire HTTP client (RPC + SSE consumer) | `tools/bear-armature` | Implemented opt-in path: `initialize`, `session.open`, `run.start`, event replay, fallback to legacy `/acp` unless `BEARS_BEARWIRE_REQUIRED=1` |
| BearWire client result methods | `tools/bear-armature` + `den-bearwire` | Implemented routing for `client.tool.result` and `client.permission.result` when BearWire events carry `run_id`; core `turn_obligations` are the intended source of truth for waits/continuations |
| BearWire session lifecycle/resource methods | `tools/bear-armature` | Implemented BearWire `session.close`, `run.cancel`, and `resource.update` paths with legacy fallback |
| Move any remaining ACP-specific descriptor framing | adapter + den-bearwire | BearWire forwards armature `client_context`; Den derives Pair local tool descriptors from armature direct-tool capabilities. Descriptor vocabulary cleanup remains Phase 6 terminology/neutrality work |
| Dual-mode operation | adapter config | Implemented opt-in `BEARS_BEARWIRE=1`, auto-probe with `BEARS_BEARWIRE=auto`, and strict mode via `BEARS_BEARWIRE_REQUIRED=1`; default-to-BearWire remains Phase 4 |
| Parity test suite | adapter + den integration | Code-side tests pass; external Zed ACP smoke still needed for plain chat, file tools, permission-gated edits, cancellation, close, and legacy fallback before Phase 4 |

**Exit gate:** Implementation complete. Remaining validation before Phase 4 is an external Zed ACP smoke test against Den with BearWire enabled, plus parity confidence for plain chat, file tools, permission-gated edits, cancellation, close, and fallback behavior.

## Phase 3.1 — BearWire obligation authority cleanup — Complete

**Goal:** Make persisted core turn obligations the only source of truth for tool/permission waits and continuations. The legacy active fields on the run table (`active_tool_call_id`, `active_permission_id`, `active_request_id`) should become derived/debug-only and then be removed.

| Task | Location | Notes |
| --- | --- | --- |
| Treat `turn_obligations` as authoritative in handlers | `den-bearwire` | Complete. `client.tool.result` and `client.permission.result` validate expected responder action and state from obligation rows |
| Stop writing active obligation fields for new logic | `den-bearwire` / `den-runtime` | Complete. `turn_runs.state` remains lifecycle summary; active tool/permission/request IDs are no longer part of the run model |
| Add active obligation query helpers | `den-runtime::turn_obligations` | Partially complete. Helpers exist for specific tool/permission obligations; add session/run list helpers later if `/status` needs richer obligation inspection |
| Update run cancellation/failure/completion to settle obligations | `den-bearwire` | Complete. Terminal run states now settle outstanding obligation rows as `continued`, `failed`, or `cancelled`; richer stale-obligation reporting remains optional observability work |
| Remove legacy active tool/permission/request columns | migration + Rust structs | Complete. Added drop-column migration and removed fields from turn run rows/queries |
| Add regression tests for obligation authority | `den-bearwire` | Complete. Added wrong-method, fresh-state/reconnect-shaped persisted obligation, duplicate retry, duplicate conflict, and cancel-settlement coverage |

**Exit gate:** Met: no BearWire code reads or writes legacy active run fields, the columns are dropped by migration, client result handlers validate persisted turn obligations, terminal run states settle outstanding obligations, and hardening tests cover wrong-method/reconnect-shaped/cancel/duplicate-result cases. Remaining work is optional richer stale-obligation reporting if `/status` needs it.

## Phase 3.2 — State isolation and continuation audit — Complete

**Goal:** Prove BearWire/ACP session, run, tool, permission, and conversation state cannot leak across sessions or turns, and make failures diagnosable when in-memory continuation state is unavailable.

| Task | Location | Notes |
| --- | --- | --- |
| Audit runtime/session identity keys | `den-runtime`, `den-bearwire`, `den-acp`, `bear-armature` | Confirm every continuation path carries `bear_id`, `human/user_id`, `session_id`, `run_id`, `conversation_id`, `tool_call_id`, and `permission_id` where applicable |
| Same-session multi-run isolation tests | `den-bearwire` | Complete for active-run isolation: DB constraint rejects a second active run in one ACP session |
| Cross-session collision tests | `den-bearwire` / `den-runtime` | Complete for BearWire tool results: same `tool_call_id` in two sessions remains isolated by session/run and wrong-session delivery is rejected |
| Conversation continuity tests | `den-bearwire` / `bear-armature` | Complete for Den/BearWire: reused ACP session with resolved `den-conv-*` uses that native conversation as runtime `upstream_target`, and an integration test asserts prior transcript reaches the LLM request |
| Restart / missing session-store behavior | `den-runtime` / `den-bearwire` | Complete for BearWire client results: missing native in-memory session returns `continuation_unavailable` and does not record/settle the obligation |
| ADR-0044 session-state appendix | docs | Complete. ADR-0044 now records stream progress invariants plus session/run/conversation ownership rules |

**Exit gate:** Met for Den/BearWire state isolation: tests cover same-session active-run isolation, cross-session id collisions, resolved-conversation continuity, and missing in-memory continuation state. Armature-side parity remains part of Phase 3 smoke validation.

## Phase 4 — Deprecate adapter-SSE and `/acp/**` — Pending

**Goal:** Single armature wire.

| Task | Notes |
| --- | --- |
| Default adapter to BearWire | In progress. `bear-armature` now defaults to BearWire auto mode; force legacy with `BEARS_LEGACY_ACP_HTTP=1` for one release |
| Remove `gateway_event_to_adapter_sse` from hot path | Keep function behind `#[deprecated]` until adapter release ships |
| Redirect or 410 `/acp/**` routes | Or internal shim: `/acp/prompt` → `run.start` RPC (optional compatibility layer) |
| Rename `den-acp` → `den-bearwire` | Crate + docs; no gateway feature flag remains |
| Update `ROUTES.md`, deploy docs, `.env.example` | |

**Exit gate:** No production adapter release depends on adapter-SSE; `/acp` documented as removed.

## Phase 5 — WebSocket transport (v2, optional) — Optional / Pending

**Goal:** Align transport with JSON spec preferred binding.

| Task | Notes |
| --- | --- |
| `wss://<den>/bearwire/v1` JSON-RPC bidirectional | Same methods + `event` notifications on socket |
| Adapter prefers WebSocket when available | HTTP+SSE remains fallback |
| Load balancer / Coolify notes | Sticky sessions or SSE-friendly proxy for v1 |

**Exit gate:** Adapter uses WebSocket in CI; HTTP+SSE still supported.

## Phase 6 — Bear stance terminology cleanup — Final cleanup / Pending

**Goal:** Remove product/model-facing confusion between Bear **stances** and session **modes** after the BearWire cutover is stable.

A **stance** is a named posture of the same Bear (`chat`, `pair`, `curate`, `work`, `watch`) that selects prompt framing, memory scope, and tool surface. A stance is not a separate Bear or separate identity. A **session mode** remains only the Pair coding-session tool-policy state (`ask`, `plan`, `write`).

| Task | Notes |
| --- | --- |
| Add or update architecture docs defining “Bear stance” | Explicitly reserve “mode” for `ask` / `plan` / `write`; avoid using “profile”, “role”, or “agent” in user/model-facing text except for compatibility notes |
| Scrub prompts and self-descriptions | Prompts should say “You are {BearName}, currently operating in the {stance} stance”; they must not imply each stance is a different Bear |
| Scrub UI and docs | Replace user-visible “profile/role” language with “stance” where it refers to `chat` / `pair` / `curate` / `work` / `watch` |
| Introduce internal code aliases before broad renames | Prefer `BearStance` wrappers/aliases around existing `BearProfile` first; avoid destabilizing the BearWire migration with a large mechanical rename |
| Migrate internal names deliberately | Later rename `BearProfile`, `NativeCapabilityProfile`, `bear_profile_bindings`, `profile` columns/fields, and related API fields only with explicit migration/compatibility plan |
| Keep compatibility boundaries clear | Legacy DB/API names may remain temporarily; model-facing tool labels and prompts should use “stance” immediately after this phase |

**Exit gate:** User/model-facing language consistently distinguishes Bear stances from session modes; Bear self-description no longer presents stances as different Bears; internal legacy `profile` names are either migrated or wrapped behind stance-named APIs.

## Control-method mapping (complete)

| Legacy HTTP | BearWire method | Notes |
| --- | --- | --- |
| `GET …/auth-check` | `initialize` (subset) or preflight on `session.open` | Returns bearer validity + bear membership |
| `GET …/sessions` | `session.state` (list) | Pagination params in RPC |
| `GET …/sessions/{id}` | `session.state` | |
| `POST …/adapter-environment` | `resource.update` | `kind: "acp_adapter"` capabilities |
| `POST …/mode` | `session.state` + plan side effects | Plan mode is core; wire is state update |
| `POST …/prompt` | `run.start` | Returns `run_id`; events on SSE stream |
| `POST …/tool-results/{id}` | `client.tool.result` | Resumes paused run |
| `POST …/permissions/{id}` | `client.permission.result` | |
| `POST …/cancel` | `run.cancel` | |
| `POST …/close` | `session.close` | |
| `POST …/compact` | `run.start` with `kind: "compaction"` or dedicated `run.start` params | TBD in JSON spec |
| `GET …/history` | `session.state` or `memory.read` (future) | v1: keep as RPC wrapper over existing handler |

## Event mapping (complete)

| Adapter-SSE | BearWire `type` | Payload notes |
| --- | --- | --- |
| `assistant_text_delta` | `message.delta` | `text`, `message_id`, `run_id` subjects |
| `status_text` | `run.progress` | `kind: "status_text"`, `text` |
| `tool_request` (approval required) | `tool_call.blocked` | `reason: "permission_required"` + tool args |
| `tool_request` (no approval) | `tool_call.requested` | Delegation to armature |
| `permission_request` | `permission.requested` | |
| `turn_complete` | `run.completed` | `outcome: "ok"` |
| `turn_result` (ok) | `run.completed` | Merge `turn_result` diagnostics into payload |
| `turn_result` (error) | `run.failed` | `reason`, `retryable`, diagnostics |
| `error` (terminal) | `run.failed` | |
| `error` (non-terminal) | `run.warning` or `diagnostic.reported` | Classify in projection layer |
| `conversation_resolved` | `session.bound` | `conversation_id` as resource ref |
| `mode_update` | `session.state` | `mode` field |
| `plan_update` | `resource.updated` | `kind: "plan"` |
| `session_info_update` | `session.state` | title/meta |

## Test strategy

| Layer | What |
| --- | --- |
| Unit | `runtime_semantic_event` → BearWire JSON snapshots |
| Golden | Existing `golden_traces_tests` + BearWire SSE output |
| Integration | `den` test harness: RPC + SSE against test DB |
| Adapter | Parity tests: legacy HTTP vs BearWire for same prompt fixtures |
| Manual | Zed session: read file, tool approval, cancel |

## Risks

| Risk | Mitigation |
| --- | --- |
| Adapter-SSE and BearWire diverge during parallel run | Phase 0 mapping tests; single semantic source (`RuntimeSemanticEvent`) |
| WebSocket delayed indefinitely | v1 HTTP+SSE is full BearWire semantics, not a throwaway format |
| Large adapter change | Dual-mode + feature flag; legacy default until Phase 4 |
| `GatewayEvent` channels entangled with SSE | Phase 1 wire types bypass channels; handlers attach settlement server-side |
| Duplicate run/obligation state machines diverge | Phase 3.1 made core `turn_obligations` authoritative and removed/deprecated legacy active run fields |
| Stance/profile/mode terminology remains confused | Phase 6 explicitly scrubs product/model-facing language and reserves “mode” for `ask` / `plan` / `write` only |

## Out of scope

- Public third-party BearWire API
- Replacing `/v1.0` REST or web UI channels (they may consume BearWire internally later)
- Renaming Postgres `acp_sessions` table (orthogonal DB migration)

## Milestone summary

| Phase | Milestone | User-visible |
| --- | --- | --- |
| 0 | Mapping locked in tests | Complete |
| 1 | BearWire types in den-runtime | Complete |
| 2 | `/bearwire/v1` served | Complete; still parallel to legacy `/acp` |
| 3 | Armature BearWire client | Implementation complete; opt-in via `BEARS_BEARWIRE=1` or `BEARS_BEARWIRE=auto`; smoke/parity validation pending |
| 3.1 | BearWire obligation authority cleanup | Complete, including active-field removal, terminal obligation settlement, and wrong-method/reconnect-shaped/cancel/duplicate-result tests |
| 3.2 | State isolation and continuation audit | Complete for Den/BearWire; armature-side parity remains covered by Phase 3 smoke validation |
| 4 | Legacy removed | Pending; default path after parity confidence |
| 5 | WebSocket | Optional; faster reconnect |
| 6 | Bear stance terminology cleanup | Pending final cleanup; user/model-visible clarity |

## Related documents

- [ACP Runtime Contract](../architecture/acp-runtime-contract.md) — update when `/acp` is shimmed
- [ADR-0003 session bindings](../decisions/adr-0003-acp-session-bindings.md) — session store unchanged; only wire changes
- [ACP Direct Local Tool Runtime Plan](archives/ACP_CLIENT_TOOL_RELAY_PLAN.md) — armature/local tool boundary (still valid)
