# ADR: BearWire resource-oriented event model

**Status:** Proposed  
**Date:** 2026-05-26  
**Deciders:** Hans

## Context

BearWire is the trusted runtime/control protocol between Den and BEARS-controlled edge runtimes such as ACP adapters, desktop companions, remote daemons, CI/devcontainer runners, and Reflection-related workers.

As BearWire evolves, two closely related design needs have become clear:

1. BearWire needs a stable semantic event vocabulary that feels idiomatic to engineers familiar with distributed control planes, job/run orchestration, and agent runtimes.
2. BearWire needs a generic resource-oriented model that can represent many protocol-addressable objects without baking historically specific concepts into the top-level event taxonomy.

Earlier BearWire thinking established event envelopes, capability negotiation, session lifecycle, tool callbacks, permission mediation, diagnostics, and replay/resume concerns. Separately, Den runtime work has also introduced structured semantic events for turn execution and continuation flows.

Those lines of work are directionally aligned, but the mapping between internal runtime semantics and BearWire wire events must be made more explicit and more idiomatic.

In particular, BearWire needs to distinguish cleanly between:

- execution lifecycle;
- streamed output lifecycle;
- delegated tool-call lifecycle;
- permission gates;
- typed resource binding and discovery; and
- transport/RPC errors versus normal runtime failure outcomes.

We also want BearWire vocabulary to generalize well beyond a single historically specific concept such as "work surface" while still supporting typed workspace-like bindings where needed.

## Decision

BearWire will adopt a **resource-oriented semantic event model**.

The model has three layers:

1. **semantic facts** — what happened in the system;
2. **wire events** — stable BearWire event types and payloads streamed over the protocol; and
3. **control methods** — JSON-RPC methods used to start, continue, cancel, inspect, acknowledge, or otherwise control work.

BearWire will use **resource** as the generic protocol abstraction for identifiable runtime objects and typed bindings.

BearWire event taxonomy will be organized around stable semantic domains rather than backend payload formats or UI labels:

- connection
- session
- run
- message
- tool_call
- permission
- resource
- diagnostic
- health
- version
- memory
- reflection

BearWire will prefer lifecycle-centered event names such as:

- `run.started`
- `run.paused`
- `message.delta`
- `tool_call.blocked`
- `resource.bound`

rather than preserving narrowly scoped or transport-shaped names such as raw backend event documents or one-off status labels.

## Rationale

### Distributed-systems idiom

Control-plane and remote-runtime protocols are easier to understand and extend when they model:

- identifiable resources;
- lifecycle transitions;
- explicit pause/resume/cancel semantics;
- clear separation between execution state and output streams; and
- structured distinction between transport failures and domain failures.

This makes BearWire feel familiar to engineers who know RPC systems, schedulers, workflow engines, debuggers, and orchestration runtimes.

### Agent-runtime idiom

Agent systems naturally distinguish between:

- a bounded execution attempt;
- streamed assistant output;
- delegated tool use;
- human approval gates; and
- resumable execution.

The BearWire model should preserve those distinctions directly.

### Generalized resource model

A generic `resource` namespace lets BearWire represent:

- workspace-like contexts;
- repositories;
- bound runtime targets;
- future durable artifacts;
- permission requests;
- other typed objects

without forcing the event taxonomy to be redesigned every time a new object class becomes important.

### Better continuity between internal semantics and wire semantics

Den may continue to use typed internal runtime event representations. BearWire should act as the stable wire projection of those semantic facts, not as a thin wrapper around backend transport residue.

## Event model

### Core semantic domains

BearWire event taxonomy is organized around these semantic domains:

- `connection`
- `session`
- `run`
- `message`
- `tool_call`
- `permission`
- `resource`
- `diagnostic`
- `health`
- `version`
- `memory`
- `reflection`

### Resource-oriented identity

BearWire should use typed resource identity wherever generic protocol object identity is needed.

Example:

```json
{
  "resource": {
    "kind": "workspace",
    "id": "repo_123",
    "uri": "git+https://github.com/example/project",
    "display_name": "example/project"
  }
}
```

Recommended resource fields:

- `kind`
- `id`
- `uri` (optional)
- `display_name` (optional)
- `version` (optional)
- `metadata` (optional, bounded)

### Event envelope

BearWire events continue to use a common JSON-RPC notification envelope with a stable `type` and event-specific `data`.

The envelope should support resource-oriented subjects such as:

```text
resource/session/ses_123
resource/run/run_123
resource/message/msg_123
resource/tool_call/tc_123
resource/workspace/repo_123
resource/permission_request/perm_123
```

Shorter forms may also be accepted if used consistently.

## Canonical event taxonomy

### Connection lifecycle

- `connection.opened`
- `connection.capabilities`
- `connection.heartbeat`
- `connection.warning`
- `connection.closing`
- `connection.lost`

### Session lifecycle

- `session.opened`
- `session.bound`
- `session.resumed`
- `session.state`
- `session.closed`
- `session.invalidated`

### Run lifecycle

A **run** is one bounded execution attempt, such as a prompt turn, continuation step, reflection run, or comparable unit of work.

- `run.accepted`
- `run.started`
- `run.progress`
- `run.paused`
- `run.resumed`
- `run.completed`
- `run.failed`
- `run.cancelled`
- `run.expired`
- `run.warning`

### Message lifecycle

- `message.started`
- `message.delta`
- `message.part`
- `message.completed`
- `message.aborted`

### Tool-call lifecycle

- `tool_call.requested`
- `tool_call.dispatched`
- `tool_call.blocked`
- `tool_call.started`
- `tool_call.progress`
- `tool_call.completed`
- `tool_call.failed`
- `tool_call.cancelled`
- `tool_call.warning`

`tool_call.blocked` should use payload reasons such as:

- `permission_required`
- `missing_capability`
- `policy_hold`
- `quota_hold`
- `awaiting_runtime`

### Permission lifecycle

- `permission.requested`
- `permission.granted`
- `permission.denied`
- `permission.expired`
- `permission.revoked`

### Resource lifecycle

The `resource.*` family is the generic BearWire mechanism for typed object detection, binding, and update.

- `resource.detected`
- `resource.bound`
- `resource.updated`
- `resource.unbound`
- `resource.rejected`

These events must indicate the resource kind explicitly.

### Diagnostics and governance

- `diagnostic.reported`
- `health.reported`
- `version.reported`
- `memory.review_requested`
- `memory.write_recorded`
- `reflection.run_started`
- `reflection.run_completed`
- `reflection.proposal_created`

## Semantic mapping guidance

The following semantic mappings should guide BearWire projections:

| Semantic fact | BearWire event type | Notes |
| --- | --- | --- |
| assistant text delta | `message.delta` | Canonical streamed text output event. |
| status text update | `run.progress` | Use payload kind such as `status_text`. |
| tool call requested | `tool_call.requested` | Initial delegated-capability request. |
| generic runtime issue | classify before projection | Prefer `run.warning`, `run.failed`, `tool_call.failed`, `diagnostic.reported`, or RPC `error`. |
| backing context resolved | `session.bound` or `resource.bound` | Use `session.bound` for interactive session binding; use `resource.bound` when the typed resource is the focus. |
| waiting for continuation | `run.paused` | Use payload reason `awaiting_continuation`. |
| turn completed | `run.completed` | Canonical terminal success event. |
| turn failed | `run.failed` | Canonical terminal failure event. |
| turn cancelled | `run.cancelled` | Canonical terminal cancellation event. |

## Continuation semantics

Continuation should be modeled as normal execution lifecycle state.

When a run is waiting on an external action, BearWire should use `run.paused` with an explicit reason, for example:

- `awaiting_continuation`
- `awaiting_input`
- `awaiting_approval`
- `awaiting_resource_binding`

When execution resumes, BearWire should use `run.resumed`.

This keeps BearWire aligned with common workflow and remote-runtime patterns:

- started
- progress
- paused
- resumed
- completed / failed / cancelled / expired

## Error model

BearWire must distinguish between:

1. **RPC/transport errors** — represented as JSON-RPC error responses;
2. **streamed warnings and diagnostics** — represented as events such as `run.warning`, `tool_call.warning`, or `diagnostic.reported`; and
3. **terminal execution failures** — represented as lifecycle events such as `run.failed` or `tool_call.failed`.

BearWire should avoid relying on a generic top-level streamed `error` event as the main runtime taxonomy.

## Control method alignment

BearWire control methods should align with the semantic domains above.

### Connection methods

```text
initialize
shutdown
heartbeat
```

### Session methods

```text
session.open
session.resume
session.close
session.state
```

### Run methods

A run-oriented naming model is preferred where practical:

```text
run.start
run.resume
run.cancel
run.result
run.ack
```

If transitional compatibility requires a more generic operation-oriented method family, the semantic model should still treat those methods as run lifecycle controls.

### Event transport methods

```text
event
 event.ack
 event.replay
```

### Tool and permission methods

```text
client.tool.call
client.tool.cancel
client.permission.request
client.permission.result
```

### Resource methods

```text
resource.register
resource.update
resource.unregister
resource.bind
resource.reject
```

### Diagnostics and governance methods

```text
diagnostic.report
health.check
version.report
memory.review_requested
reflection.run_requested
```

## Consequences

### Positive

- Gives BearWire a more idiomatic and extensible semantic vocabulary.
- Separates execution state from output state and tool state more clearly.
- Creates a generic resource model that can support workspace-like bindings without hard-coding one legacy concept into the protocol surface.
- Improves continuity between Den internal semantic events and BearWire wire events.
- Makes continuation, pause/resume, and blocking semantics easier to reason about.
- Produces a more legible basis for future JSON and Rust BearWire specs.

### Trade-offs

- Introduces a more deliberate semantic layer that must be documented and kept coherent.
- Requires migration guidance from older BearWire terminology and any operation-first naming already in flight.
- Generic `resource.*` events can be too abstract if payload schemas are not kept precise.

## Non-goals

- Do not collapse all domains into a single undifferentiated `resource.*` stream.
- Do not treat backend/provider event shapes as the canonical BearWire vocabulary.
- Do not use generic streamed `error` as the sole model for warnings, transport failures, and terminal run failures.
- Do not remove typed workspace-like bindings; instead represent them as typed resources.

## Follow-on documents

The resource-oriented event model defined by this ADR should be reflected in:

- a JSON-based BearWire protocol document describing event envelopes, payload schemas, and method families; and
- a Rust-based BearWire design document describing type models, enums, identifiers, and projection rules.

## Related documents

- [ADR-0007: BearWire protocol](adr-0007-bearwire-protocol.md)
- [ADR-0029: Den structured runtime events](adr-0029-den-structured-runtime-events.md)
- [ADR-0024: terminology: actuators, resources, and role names](adr-0024-terminology-actuators-resources-and-role-names.md)
- [Den conversation runtime schema](../architecture/den-conversation-runtime-schema.md)
- [Reflection run taxonomy](../architecture/reflection-run-taxonomy.md)
