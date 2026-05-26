# BearWire Rust design

**Status:** Draft  
**References:** [ADR-0030: BearWire resource-oriented event model](../decisions/adr-0030-bearwire-resource-oriented-event-model.md), [BearWire JSON specification](bearwire-json-spec.md), [ADR-0007: BearWire protocol](../decisions/adr-0007-bearwire-protocol.md)

## Purpose

This document describes a Rust-oriented design for BearWire semantic types and wire projection.

It focuses on:

- strongly typed semantic event modeling;
- identifiers and resource references;
- event envelope types;
- payload enums and lifecycle modeling;
- projection between internal semantic facts and BearWire JSON events; and
- implementation guidance for replay, continuation, and callback handling.

This document follows ADR-0030's resource-oriented event model and is intended to pair with the JSON-oriented BearWire specification.

## Design goals

The Rust model should:

- preserve semantic clarity between runs, messages, tool calls, permissions, and resources;
- make illegal state transitions harder to express;
- support explicit projection to BearWire JSON event envelopes;
- remain evolvable as new event variants are added;
- support replay and bounded persistence metadata; and
- avoid leaking backend/provider payload structure into BearWire-facing types.

## Type-structure overview

A recommended layering is:

1. **internal semantic event types**
2. **wire projection types**
3. **transport binding helpers**

### Layer 1: semantic event types

These are Rust enums/structs representing Den-owned runtime facts.

### Layer 2: wire projection types

These convert semantic events into stable BearWire event names and JSON payloads.

### Layer 3: transport binding helpers

These serialize into JSON-RPC notifications and responses.

## Identifiers

BearWire should use typed identifiers at the Rust level rather than unconstrained strings.

Example newtypes:

```rust
pub struct EventId(pub String);
pub struct SessionId(pub String);
pub struct RunId(pub String);
pub struct MessageId(pub String);
pub struct ToolCallId(pub String);
pub struct PermissionRequestId(pub String);
pub struct ResourceId(pub String);
```

Additional ids may include:

```rust
pub struct BearId(pub String);
pub struct RoleAgentId(pub String);
pub struct HumanId(pub String);
pub struct ResumeToken(pub String);
```

These can later gain validation helpers, parsing, display impls, or compact storage forms.

## Resource model

A typed resource reference is the generic identity model used across BearWire.

```rust
pub struct ResourceRef {
    pub kind: ResourceKind,
    pub id: ResourceId,
    pub uri: Option<String>,
    pub display_name: Option<String>,
    pub version: Option<String>,
    pub metadata: Option<serde_json::Value>,
}
```

Recommended `ResourceKind` enum:

```rust
pub enum ResourceKind {
    Session,
    Run,
    Message,
    ToolCall,
    PermissionRequest,
    Workspace,
    Repository,
    ReviewRequest,
    ReflectionRun,
    Other(String),
}
```

This preserves the generic resource-oriented model while keeping common kinds explicit.

## Event envelope model

The BearWire event envelope should separate common metadata from typed payload.

```rust
pub struct BearWireEvent {
    pub event_id: EventId,
    pub sequence: Option<u64>,
    pub scope: EventScope,
    pub source: String,
    pub event_type: BearWireEventType,
    pub subject: Option<String>,
    pub time: chrono::DateTime<chrono::Utc>,
    pub bear_id: Option<BearId>,
    pub role: Option<String>,
    pub role_agent_id: Option<RoleAgentId>,
    pub human_id: Option<HumanId>,
    pub session_id: Option<SessionId>,
    pub run_id: Option<RunId>,
    pub resource_refs: Vec<ResourceRef>,
    pub data: BearWireEventData,
}
```

### Event scope

```rust
pub enum EventScope {
    Persistent,
    Ephemeral,
}
```

## Event type enum

The Rust model should keep event type taxonomy explicit.

```rust
pub enum BearWireEventType {
    ConnectionOpened,
    ConnectionCapabilities,
    ConnectionHeartbeat,
    ConnectionWarning,
    ConnectionClosing,
    ConnectionLost,

    SessionOpened,
    SessionBound,
    SessionResumed,
    SessionState,
    SessionClosed,
    SessionInvalidated,

    RunAccepted,
    RunStarted,
    RunProgress,
    RunPaused,
    RunResumed,
    RunCompleted,
    RunFailed,
    RunCancelled,
    RunExpired,
    RunWarning,

    MessageStarted,
    MessageDelta,
    MessagePart,
    MessageCompleted,
    MessageAborted,

    ToolCallRequested,
    ToolCallDispatched,
    ToolCallBlocked,
    ToolCallStarted,
    ToolCallProgress,
    ToolCallCompleted,
    ToolCallFailed,
    ToolCallCancelled,
    ToolCallWarning,

    PermissionRequested,
    PermissionGranted,
    PermissionDenied,
    PermissionExpired,
    PermissionRevoked,

    ResourceDetected,
    ResourceBound,
    ResourceUpdated,
    ResourceUnbound,
    ResourceRejected,

    DiagnosticReported,
    HealthReported,
    VersionReported,
    MemoryReviewRequested,
    MemoryWriteRecorded,
    ReflectionRunStarted,
    ReflectionRunCompleted,
    ReflectionProposalCreated,
}
```

A helper should map these enum variants to wire strings such as:

- `RunStarted` → `run.started`
- `MessageDelta` → `message.delta`
- `ResourceBound` → `resource.bound`

This avoids stringly typed event construction in most implementation paths.

## Event payload enum

The payload enum should group event-specific structured data.

```rust
pub enum BearWireEventData {
    SessionOpened(SessionOpenedData),
    SessionBound(SessionBoundData),
    SessionResumed(SessionResumedData),
    SessionState(SessionStateData),
    SessionClosed(SessionClosedData),
    SessionInvalidated(SessionInvalidatedData),

    RunAccepted(RunAcceptedData),
    RunStarted(RunStartedData),
    RunProgress(RunProgressData),
    RunPaused(RunPausedData),
    RunResumed(RunResumedData),
    RunCompleted(RunCompletedData),
    RunFailed(RunFailedData),
    RunCancelled(RunCancelledData),
    RunExpired(RunExpiredData),
    RunWarning(RunWarningData),

    MessageStarted(MessageStartedData),
    MessageDelta(MessageDeltaData),
    MessagePart(MessagePartData),
    MessageCompleted(MessageCompletedData),
    MessageAborted(MessageAbortedData),

    ToolCallRequested(ToolCallRequestedData),
    ToolCallDispatched(ToolCallDispatchedData),
    ToolCallBlocked(ToolCallBlockedData),
    ToolCallStarted(ToolCallStartedData),
    ToolCallProgress(ToolCallProgressData),
    ToolCallCompleted(ToolCallCompletedData),
    ToolCallFailed(ToolCallFailedData),
    ToolCallCancelled(ToolCallCancelledData),
    ToolCallWarning(ToolCallWarningData),

    PermissionRequested(PermissionRequestedData),
    PermissionGranted(PermissionGrantedData),
    PermissionDenied(PermissionDeniedData),
    PermissionExpired(PermissionExpiredData),
    PermissionRevoked(PermissionRevokedData),

    ResourceDetected(ResourceDetectedData),
    ResourceBound(ResourceBoundData),
    ResourceUpdated(ResourceUpdatedData),
    ResourceUnbound(ResourceUnboundData),
    ResourceRejected(ResourceRejectedData),

    DiagnosticReported(DiagnosticReportedData),
    HealthReported(HealthReportedData),
    VersionReported(VersionReportedData),
    MemoryReviewRequested(MemoryReviewRequestedData),
    MemoryWriteRecorded(MemoryWriteRecordedData),
    ReflectionRunStarted(ReflectionRunStartedData),
    ReflectionRunCompleted(ReflectionRunCompletedData),
    ReflectionProposalCreated(ReflectionProposalCreatedData),
}
```

Implementations may use nested module organization to keep payload definitions manageable.

## Recommended key payload types

### Run progress

```rust
pub struct RunProgressData {
    pub run_id: RunId,
    pub kind: RunProgressKind,
    pub text: Option<String>,
    pub phase: Option<String>,
    pub progress: Option<f32>,
    pub detail: Option<serde_json::Value>,
}

pub enum RunProgressKind {
    StatusText,
    Phase,
    Queue,
    Heartbeat,
    Other(String),
}
```

### Run pause

```rust
pub struct RunPausedData {
    pub run_id: RunId,
    pub reason: RunPauseReason,
    pub resume_token: Option<ResumeToken>,
    pub expires_at: Option<chrono::DateTime<chrono::Utc>>,
}

pub enum RunPauseReason {
    AwaitingContinuation,
    AwaitingInput,
    AwaitingApproval,
    AwaitingResourceBinding,
    Other(String),
}
```

### Message delta

```rust
pub struct MessageDeltaData {
    pub message_id: MessageId,
    pub run_id: RunId,
    pub index: u32,
    pub delta: String,
}
```

### Tool call blocked

```rust
pub struct ToolCallBlockedData {
    pub tool_call_id: ToolCallId,
    pub run_id: RunId,
    pub reason: ToolCallBlockedReason,
    pub permission_request_id: Option<PermissionRequestId>,
}

pub enum ToolCallBlockedReason {
    PermissionRequired,
    MissingCapability,
    PolicyHold,
    QuotaHold,
    AwaitingRuntime,
    Other(String),
}
```

### Resource binding

```rust
pub struct ResourceBindingTarget {
    pub kind: ResourceKind,
    pub id: String,
}

pub struct ResourceDetectedData {
    pub binding_target: ResourceBindingTarget,
    pub resource: ResourceRef,
    pub confidence: Option<f32>,
    pub evidence: Option<serde_json::Value>,
}

pub struct ResourceBoundData {
    pub binding_target: ResourceBindingTarget,
    pub resource: ResourceRef,
}
```

Note: if stronger target typing becomes valuable later, `ResourceBindingTarget` can become an enum or reuse `ResourceRef` directly.

## Semantic events versus BearWire events

The recommended implementation model is:

- internal runtime code produces semantic events in Den-native types;
- a projection layer maps those semantic events into BearWire event types and payloads;
- the transport layer serializes them into JSON-RPC event notifications.

This keeps BearWire from becoming the internal-only source of truth for all runtime semantics while still preserving a stable wire contract.

## Recommended projection mapping

Examples:

| Internal semantic fact | BearWire event type | Notes |
| --- | --- | --- |
| assistant text delta | `message.delta` | Direct projection. |
| status text update | `run.progress` | Use `RunProgressKind::StatusText`. |
| tool call requested | `tool_call.requested` | Direct projection. |
| waiting for continuation | `run.paused` | Use `RunPauseReason::AwaitingContinuation`. |
| turn completed | `run.completed` | Direct projection. |
| turn failed | `run.failed` | Direct projection. |
| turn cancelled | `run.cancelled` | Direct projection. |
| backing resource bound | `resource.bound` or `session.bound` | Depends on whether session binding or typed resource binding is primary. |

## JSON projection strategy

A BearWire event should serialize as:

- JSON-RPC notification method `event`
- envelope metadata fields
- wire `type` string derived from `BearWireEventType`
- `data` serialized from `BearWireEventData`

Recommended helper shape:

```rust
impl BearWireEventType {
    pub fn as_wire_str(&self) -> &'static str {
        // map enum to dotted BearWire string
        unimplemented!()
    }
}
```

For `Other(String)` variants in subtype enums, JSON serialization should preserve the user-defined string value where allowed by schema policy.

## Continuation semantics

Continuation should be represented through normal run lifecycle states.

At the Rust type level:

- paused execution is `RunPausedData`
- resumed execution is `RunResumedData`
- expiration is `RunExpiredData`

Avoid using ad hoc continuation-only event types if the state can be represented with `RunPauseReason`.

## Error handling model

The Rust model should keep three layers distinct:

1. **transport/RPC errors**
   - request framing
   - invalid params
   - unauthorized
   - unsupported method

2. **streamed warnings/diagnostics**
   - `RunWarningData`
   - `ToolCallWarningData`
   - `DiagnosticReportedData`

3. **terminal execution outcomes**
   - `RunFailedData`
   - `ToolCallFailedData`
   - `SessionInvalidatedData`

Avoid using one generic `Error` payload enum as the primary model for all three categories.

## Replay and persistence guidance

Replay-related metadata belongs in the envelope, not only in payloads.

Important fields:

- `event_id`
- `sequence`
- `scope`

A persistence/replay subsystem may choose to persist only persistent-scoped events. Ephemeral events may remain transient while still using the same Rust type model.

## Suggested module organization

A possible Rust module layout:

```text
bearwire/
  mod.rs
  ids.rs
  resource.rs
  envelope.rs
  event_type.rs
  event_data.rs
  progress.rs
  tool_calls.rs
  permissions.rs
  session.rs
  run.rs
  message.rs
  diagnostics.rs
  reflection.rs
  jsonrpc.rs
  projection.rs
```

Responsibilities:

- `ids.rs` — typed identifiers
- `resource.rs` — `ResourceKind`, `ResourceRef`, binding targets
- `envelope.rs` — `BearWireEvent`, `EventScope`
- `event_type.rs` — `BearWireEventType` and wire-string mapping
- `event_data.rs` — top-level payload enum
- `projection.rs` — semantic event to BearWire event projection
- `jsonrpc.rs` — JSON-RPC request/response/event wrappers

## Evolution guidance

When adding new variants:

- prefer adding new optional fields before new top-level event types if the distinction is subtype-level only;
- add new event types when the semantic fact is stable and lifecycle-relevant;
- preserve backward-compatible wire-string mapping;
- keep Rust enums explicit even if JSON allows looser open-ended strings in some places.

## Open design questions

- Which ids deserve dedicated validated types versus lightweight string newtypes.
- Whether `ResourceBindingTarget` should become a first-class reusable resource reference form.
- Whether `run.accepted` should always be emitted in all runtimes.
- Whether to derive JSON schema directly from Rust types for subsets of the protocol.
- Whether stable crates/modules should separate semantic types from transport types at crate boundaries.
