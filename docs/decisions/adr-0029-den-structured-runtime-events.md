# ADR: Den structured runtime events

**Status:** Proposed  
**Date:** 2026-05-26  
**Deciders:** Hans

## Context

During the Letta → Den migration, ACP execution and continuation flows have inherited too much upstream transport structure:

- raw HTTP streaming responses;
- SSE frame assembly in ACP;
- JSON parsing in ACP;
- Letta-shaped event interpretation in ACP.

This makes ACP act too much like a backend protocol interpreter instead of a client/channel adapter.

At the same time, BEARS controls the full stack between:

- model/provider APIs on the backend edge; and
- client protocols such as ACP on the frontend edge.

That means Den is free to define its own meaningful internal event vocabulary instead of preserving backend wire-format structure through the stack.

The BearWire ADR also establishes a durable distinction between:

- **Den server semantics**; and
- **trusted armatures / edge runtimes**.

So Den should own a structured runtime event vocabulary that can be consumed by:

- ACP orchestration;
- BearWire adapters;
- future Den-native runtime flows;
- diagnostics and replay layers.

## Decision

Den will define and progressively adopt **structured runtime stream events** as the internal event vocabulary for turn execution and continuation flows.

These events are Den-owned semantic facts, not backend wire frames.

The initial structured event vocabulary includes:

- `AssistantTextDelta`
- `StatusText`
- `ToolCallRequested`
- `Error`
- `ConversationResolved`
- `WaitingForContinuation`
- `TurnCompleted`
- `TurnFailed`
- `TurnCancelled`

A transitional escape hatch may remain temporarily:

- `JsonValue`

but only for migration compatibility where Letta-shaped payloads have not yet been fully normalized.

## Rationale

### Den owns meaning

Den should interpret provider/runtime stream payloads and expose semantic events upward.

Consumers inside Den or at Den-controlled boundaries should not need to understand:

- SSE framing;
- Letta message-type strings;
- provider-specific JSON layouts;
- backend stop-reason quirks.

### ACP should be a channel adapter, not a backend parser

ACP should focus on:

- client protocol emission;
- tool persistence side effects;
- permission mediation;
- client/session UX.

It should not own backend transport parsing responsibility.

### BearWire should carry semantic runtime facts

BearWire is the durable Den ↔ armature boundary. That boundary should carry meaningful structured events, not raw backend payload residue.

Structured runtime events therefore align both with:

- current Letta-removal work; and
- future BearWire event design.

## Consequences

### Positive

- Letta-specific parsing logic moves inward toward Den-owned code.
- ACP becomes simpler and more clearly a channel/adapter layer.
- Future BearWire event delivery can reuse Den-owned event meanings.
- Diagnostics and tests can assert stable semantic events rather than backend payload fragments.
- Removing Letta becomes easier because protocol interpretation is centralized.

### Costs

- Den must own more normalization code.
- Some event fields may need iterative refinement as semantics are clarified.
- Temporary duplication may exist while old Letta-shaped paths and new structured event paths coexist.

## Event design guidance

Structured runtime events should:

- represent semantic facts, not transport details;
- use Den-owned field names;
- preserve enough information for policy, diagnostics, and persistence;
- avoid embedding provider-specific formatting assumptions;
- support both turn start and continuation paths.

Examples:

### Good

- `AssistantTextDelta { text }`
- `ToolCallRequested { tool_call_id, tool_name, arguments, approval_required, ... }`
- `WaitingForContinuation`
- `TurnCompleted`
- `TurnFailed { category, message }`

### Transitional only

- `JsonValue { value }`

### Avoid as long-term contract

- raw SSE frames
- raw HTTP response handles
- Letta-specific message documents exposed outside normalization code

## Migration plan

### Phase 1

Move continuation flow from:

- raw HTTP response
- raw bytes
- SSE framing in ACP
- JSON parsing in ACP

into:

- runtime-owned event streams
- structured Den events where practical
- transitional `JsonValue` only where not yet normalized

### Phase 2

Replace remaining `JsonValue` continuation handling with structured Den runtime events.

### Phase 3

Use the same structured event vocabulary consistently across:

- turn start streaming
- turn continuation streaming
- BearWire event projection
- runtime diagnostics/replay surfaces

## Status note

As of this ADR's introduction:

- continuation framing and JSON parsing have already started moving into the runtime-side Letta compatibility layer;
- structured Den runtime events are now being introduced for continuation semantics;
- `JsonValue` remains as a temporary compatibility event while ACP persistence and mapping logic are still being migrated.
