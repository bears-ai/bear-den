# ADR: BearWire as the Den ↔ armature wire

**Status:** Proposed  
**Date:** 2026-06-16  
**Deciders:** Hans

**Related:**

- [ADR-0007](adr-0007-bearwire-protocol.md) — BearWire protocol scope and trust boundary
- [ADR-0029](adr-0029-den-structured-runtime-events.md) — Den structured runtime (semantic) events
- [ADR-0030](adr-0030-bearwire-resource-oriented-event-model.md) — BearWire resource-oriented event taxonomy
- [ADR-0003](adr-0003-acp-session-bindings.md) — ACP session bindings (edge-fed into Den session store)
- [ADR-0048](adr-0048-core-turn-client-obligation-coordinator.md) — protocol-neutral turn/client-obligation coordinator; BearWire is transport, not the continuation state machine
- [BearWire JSON specification](../architecture/bearwire-json-spec.md)
- [BearWire Rust design](../architecture/bearwire-rust-design.md)
- [BearWire armature wire implementation plan](../roadmap/BEARWIRE_ARMATURE_WIRE_IMPLEMENTATION_PLAN.md)

## Context

BEARS has three protocol layers that are easy to conflate:

```text
Editor (Zed, Cursor, …)
  ⇄ ACP JSON-RPC on stdio
bears-acp-adapter  (client armature)
  ⇄ Den HTTP + bespoke adapter-SSE today
Den ACP gateway edge (den-acp / `/acp/**`)
  ⇄ protocol-neutral agent loop (den-runtime)
```

ADR-0007 and ADR-0030 already decided that **BearWire** is the trusted runtime/control protocol **between Den and BEARS-controlled edge runtimes** (ACP adapters, desktop companions, CI sidecars, reflection workers). ADR-0029 says that boundary should carry **semantic runtime facts**, and that **ACP should be a channel adapter toward the editor**, not Den's internal parser.

In code, the internal seam is partly built:

- `RuntimeSemanticEvent` / `RuntimeStreamEvent` in `den-runtime` (semantic facts)
- `bearwire_projection` maps semantic events toward `GatewayEvent`
- `gateway_event_to_adapter_sse` projects `GatewayEvent` into **adapter-SSE JSON** (`assistant_text_delta`, `tool_request`, …)

That last hop is a **private, expedient wire** grown for `bears-acp-adapter`. It is **not** BearWire (no `message.delta`, no resource subjects, no JSON-RPC `event` envelope) and **not** ACP (ACP is stdio toward the editor). The Den gateway edge therefore owns two jobs that should split:

1. **Server control plane** — auth, session bindings, Postgres-backed session state, bear-scoped policy, server-executed Den tools, history/compaction APIs. Something on Den must always own this.
2. **Armature wire** — stable event stream + control methods between Den and trusted armatures.

Today both jobs live behind `/acp/**` and adapter-SSE, which names the edge after the wrong protocol and forces every future armature to learn a bespoke HTTP dialect.

`GatewayEvent` is also not wire-safe: it carries in-process concerns (e.g. `oneshot` channels for tool settlement). BearWire needs a separate, serializable wire projection layer as described in [BearWire Rust design](../architecture/bearwire-rust-design.md).

## Decision

### 1. BearWire is the Den ↔ armature wire

Trusted armatures (`bears-acp-adapter` first; later desktop companion, CI runner, reflection worker) speak **BearWire** to Den — not adapter-SSE, not ACP-shaped HTTP.

ACP translation (stdio framing, editor permission UX, local tool execution) stays entirely in the **client armature**. Den never parses ACP.

### 2. Evolve the gateway edge into the BearWire edge — do not delete it

The crate/module that today serves `/acp/**` (typically `den-acp`, composed by the binary) **evolves** into the **BearWire HTTP edge** (`den-bearwire` is an acceptable rename when the migration completes). It is a transport/projection edge over Den's protocol-neutral turn/client-obligation coordinator; it must not own model-continuation decisions (see ADR-0048). It remains responsible for:

- transport termination (auth, TLS, rate limits at the HTTP boundary)
- mapping BearWire control methods to den-runtime coordinator operations
- emitting BearWire `event` notifications on the run stream
- session binding persistence and multi-tenant authorization

It is **not** responsible for ACP stdio or editor UX.

### 3. v1 transport binding: HTTP control + JSON event pages

The [BearWire JSON specification](../architecture/bearwire-json-spec.md) may later add WebSocket JSON-RPC at `wss://<den>/bearwire/v1`. **v1 implementation** uses a pragmatic HTTP binding that reuses existing Axum patterns:

| Concern | v1 binding |
| --- | --- |
| Control methods | `POST /bearwire/v1/rpc` — single JSON-RPC 2.0 request/response endpoint (or method-scoped REST aliases during transition) |
| Event projection | `GET /bearwire/v1/sessions/{session_id}/events/page` — JSON page with ordered events and server-owned `next_after` cursor |
| Auth | Same bearer/OAuth machinery as today; `Authorization: Bearer`, `BearWire-Version: 1` |
| Capability negotiation | `connection.capabilities` event + `initialize` method on connect |

WebSocket JSON-RPC at `/bearwire/v1` remains a possible future binding (same semantic model, different transport). Buffered SSE is not part of BearWire v1; adapter-SSE and `/acp/**` are deprecated once parity is proven.

JSON event-page delivery remains stateless, run/session-scoped, and cursor-based. Den must not require server-held waiters or process-local session waiter registries for correctness. Failure to fetch an event page is not evidence that the run failed: before synthesizing a terminal transport outcome, an armature reconciles through `run.state` and prefers Den's canonical terminal state when available.

An armature that owns a current tool-obligation lease must not abandon the run because an arbitrary number of event-page requests failed. Claim, renewal, and result methods project coordinator transitions defined by ADR-0048; they do not make the HTTP connection, polling loop, or BearWire edge the owner of execution. Losing event delivery neither transfers execution ownership nor authorizes re-execution.

### 4. Canonical projection path

```text
RuntimeSemanticEvent  (den-runtime, in-process)
  → BearWire wire event (serializable, versioned)
  → JSON event page item (`sequence`, `event`)
  → armature projects to client protocol (ACP stdio, etc.)
```

`GatewayEvent` may remain an **in-process orchestration** type during transition, but it must not be the armature wire. New code projects `RuntimeSemanticEvent` → BearWire wire types directly.

### 4a. Surface replay invariant

BearWire is not only a live stream. For every user-visible armature surface, Den must provide a **typed surface-event history** that can replay the same UI semantics as the live stream.

The following invariant is mandatory:

> A live BearWire event stream and a later session/history replay for the same run must project to equivalent user-visible armature state: assistant message chunks remain assistant chunks; provider reasoning remains thought/reasoning display or is omitted by explicit replay policy; tool calls remain tool cards with stable ids, names, inputs, display metadata, status, and bounded output; session title/mode/plan updates remain typed session/resource updates. The replay path must not collapse these typed surface facts into plain assistant text.

Corollaries:

1. **Complete tool-call start records are required for every surfaced tool.** Every user-visible or model-relevant tool call must have a durable full tool-call surface record before any completion/failure event is projected. This applies regardless of execution location: Den-hosted tools, armature-local tools, forwarded MCP tools, runtime checkpoint tools, and future channel-local tools. A sparse completion event may reference a full persisted tool-call resource, but it must not be the only source from which an armature is expected to render the card.
2. **History replay must not use text-only conversation history for armature UI.** Human-readable conversation history may remain a flattened text projection, but ACP/session load must use the typed BearWire/surface replay projection when reconstructing UI state.
3. **Reasoning is typed display telemetry, not assistant answer content.** Provider reasoning deltas must be represented as reasoning/thought surface events with explicit replay policy. They must not be persisted or replayed as visible assistant text. The supported replay policies are intentionally limited to `none` (omit from replay) and `thought` (replay as thought/reasoning UI); unsupported policies must not be replayed as assistant text.
4. **Session metadata updates are first-class surface events.** Conversation title, mode, plan/task updates, and similar UI state changes must not depend on parsing arbitrary tool-result text at the armature edge. If a tool changes session-visible state, Den must emit and persist the corresponding typed surface event.
5. **The armature projects; it does not perform archaeology.** The ACP adapter may translate BearWire surface events into ACP wire objects, but it must not be responsible for reconstructing missing action identity, display metadata, raw inputs, or replay policy from flattened transcript prose.

This invariant strengthens §4. It does not make ACP canonical; it requires Den/BearWire to carry enough typed surface state for any trusted armature to project live and replayed UI consistently.

The shared Rust DTOs for this wire contract should live in a narrow `bearwire-protocol` crate. `bearwire-protocol` may contain serde DTOs and lightweight validation helpers for BearWire, but it must not depend on Den runtime, Den service, HTTP server/client code, database crates, ACP, or model/provider clients. Armatures should depend on `bearwire-protocol` directly rather than importing broad Den-internal protocol crates for BearWire surface replay.

A dedicated persisted surface-event stream is intentionally deferred. The current implementation may merge canonical conversation rows, selected BearWire events, and session metadata into `conversation.surface_history`; a dedicated stream should be introduced only if exact cross-source ordering/pagination becomes product-critical.

### 5. Control-method inventory replaces `/acp/**` routes

Existing HTTP routes map to BearWire methods (see implementation plan for the full table). Examples:

| Today (`/acp/...`) | BearWire method |
| --- | --- |
| `POST …/prompt` | `run.start` |
| `POST …/tool-results/{id}` | `client.tool.result` (or `run.resume` with tool payload) |
| `POST …/permissions/{id}` | `client.permission.result` |
| `POST …/cancel` | `run.cancel` |
| `POST …/close` | `session.close` |
| `POST …/adapter-environment` | `resource.update` (adapter capability resource) |
| `POST …/mode` | `session.state` / plan-mode side effect |
| `GET …/sessions`, `GET …/history` | `session.state`, history read methods |

Den-hosted tools are **server-executed**, not a second public BearWire surface. The former `/internal/den-tools/invoke` RPC was retired on the pre-release branch because a service token could not prove its caller-supplied tool context; current execution uses the in-process Den runtime invoker.

### 6. Event rename: adapter-SSE → BearWire

| Adapter-SSE `type` (today) | BearWire `type` (target) |
| --- | --- |
| `assistant_text_delta` | `message.delta` |
| `status_text` | `run.progress` (`kind: "status_text"`) |
| `tool_request` | `tool_call.requested` or `tool_call.blocked` (`reason: "permission_required"`) |
| `permission_request` | `permission.requested` |
| `turn_complete` / `turn_result` (success) | `run.completed` |
| `turn_result` (failure) / `error` (terminal) | `run.failed` |
| `error` (recoverable) | `run.warning` or `diagnostic.reported` |
| `conversation_resolved` | `session.bound` |
| `mode_update` | `session.state` |
| `plan_update` | `resource.updated` (plan artifact resource) |
| `session_info_update` | `session.state` |

Exact payload schemas follow [BearWire JSON specification](../architecture/bearwire-json-spec.md) and ADR-0030 semantic mapping guidance.

### 7. Versioning and compatibility

- BearWire wire version **`1`** for the first shipped armature binding.
- Den serves **`/acp/**` + adapter-SSE in parallel** during migration (minimum one release cycle; gated by adapter version negotiation).
- Golden traces assert **semantic parity**: same user-visible outcomes for `adapter-SSE` and BearWire projections from the same `RuntimeSemanticEvent` fixtures.
- `bears-acp-adapter` negotiates `bearwire_version` via `initialize`; falls back to legacy HTTP when Den does not advertise BearWire.

## Rationale

### Why not eliminate the server edge crate?

Armatures are **client processes** on user machines. Den must still own Postgres session bindings, OAuth, multi-tenant authorization, server-side tool execution, and audit. That edge is real; only its **wire shape** was wrong.

### Why not expose `GatewayEvent` directly?

It mixes orchestration (channels, in-process state) with semantics. BearWire wire types must be serializable, versioned, and stable across processes.

### Why HTTP+SSE before WebSocket?

Matches today's Axum deployment, allows route-by-route migration, and keeps one semantic model ([BearWire JSON spec](../architecture/bearwire-json-spec.md)) with a simpler first transport. WebSocket is an upgrade, not a redesign.

### Alignment with prior ADRs

This ADR **implements** ADR-0007/0030/0029 together — it does not replace them. It closes the gap between "BearWire is the boundary" (decided) and "adapter-SSE is the boundary" (accidental).

## Consequences

### Positive

- One stable contract for all armatures; ACP knowledge quarantined in `bears-acp-adapter`.
- Removes the extra projection hop (core → GatewayEvent → adapter-SSE → armature → ACP).
- Enables desktop companion / CI runner without copying `/acp/**` dialect.
- Makes golden-trace tests assert BearWire envelopes directly.

### Costs

- `bears-acp-adapter` must implement BearWire client + ACP server (larger armature, clearer boundary).
- Parallel `/acp` and `/bearwire` maintenance during migration.
- BearWire v1 schemas must be pinned and reviewed (public contract).

## Non-goals

- Replacing ACP toward editors — armature still speaks ACP stdio.
- Exposing BearWire to untrusted third parties — remains Den-authorized armatures only.
- Changing the protocol-neutral agent loop (ADR-0035) — only the edge wire changes.
- WebSocket transport in v1 — deferred to v2 per §Decision 3.

## Follow-on

Implementation sequencing, file-level tasks, and test gates: [BEARWIRE_ARMATURE_WIRE_IMPLEMENTATION_PLAN.md](../roadmap/BEARWIRE_ARMATURE_WIRE_IMPLEMENTATION_PLAN.md).

Update [BearWire JSON specification](../architecture/bearwire-json-spec.md) with the HTTP+SSE v1 binding section when Phase 1 starts.
