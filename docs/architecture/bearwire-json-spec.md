# BearWire JSON specification

**Status:** Draft  
**References:** [ADR-0030: BearWire resource-oriented event model](../decisions/adr-0030-bearwire-resource-oriented-event-model.md), [ADR-0007: BearWire protocol](../decisions/adr-0007-bearwire-protocol.md)

## Purpose

This document describes a JSON-based BearWire protocol shape for trusted Den-connected runtimes.

It specifies:

- event envelope conventions;
- method families;
- resource-oriented identifiers and subjects;
- canonical event payload shapes; and
- guidance for replay, continuation, tool execution, and permission mediation.

This document follows ADR-0030's resource-oriented semantic model.

## Scope

This is a BearWire wire-shape and schema-oriented design document.

It does not fully define:

- authentication token issuance;
- every possible event subtype;
- storage/replay retention policy;
- all administrative operations; or
- Den-internal implementation structure.

## Transport binding

Preferred BearWire binding:

```text
JSON-RPC 2.0 over WebSocket
```

Example endpoint:

```text
wss://<den-host>/bearwire/v1
Authorization: Bearer <token>
BearWire-Version: 1
```

## Core conventions

### JSON-RPC framing

All BearWire requests and responses use standard JSON-RPC 2.0 framing.

Example request:

```json
{
  "jsonrpc": "2.0",
  "id": "req_123",
  "method": "session.open",
  "params": {
    "session_id": "ses_123"
  }
}
```

Example successful response:

```json
{
  "jsonrpc": "2.0",
  "id": "req_123",
  "result": {
    "ok": true
  }
}
```

Example error response:

```json
{
  "jsonrpc": "2.0",
  "id": "req_123",
  "error": {
    "code": -32010,
    "message": "Permission denied",
    "data": {
      "error_type": "permission_denied",
      "component": "adapter"
    }
  }
}
```

### Event notification framing

All streamed BearWire events are sent as JSON-RPC notifications using method `event`.

```json
{
  "jsonrpc": "2.0",
  "method": "event",
  "params": {
    "event_id": "evt_000042",
    "sequence": 42,
    "scope": "persistent",
    "source": "den.pair",
    "type": "message.delta",
    "subject": "resource/run/run_123",
    "time": "2026-05-26T12:00:00Z",
    "bear_id": "bear_123",
    "role": "pair",
    "role_agent_id": "agent_123",
    "human_id": "human_123",
    "session_id": "ses_123",
    "run_id": "run_123",
    "resource_refs": [
      {
        "kind": "message",
        "id": "msg_123"
      }
    ],
    "data": {
      "message_id": "msg_123",
      "delta": "Hello"
    }
  }
}
```

## Common schemas

### Resource reference

A lightweight resource identity used across event payloads.

```json
{
  "kind": "workspace",
  "id": "repo_123",
  "uri": "git+https://github.com/example/project",
  "display_name": "example/project",
  "version": "main",
  "metadata": {}
}
```

#### Fields

| Field | Required | Meaning |
| --- | --- | --- |
| `kind` | yes | Resource type such as `session`, `run`, `message`, `tool_call`, `workspace`, `permission_request`. |
| `id` | yes | Identifier within BearWire scope. |
| `uri` | no | Canonical or externally meaningful URI. |
| `display_name` | no | Human-readable label. |
| `version` | no | Revision/version marker. |
| `metadata` | no | Bounded structured metadata. |

### Event envelope

```json
{
  "event_id": "evt_000042",
  "sequence": 42,
  "scope": "persistent",
  "source": "den.pair",
  "type": "run.started",
  "subject": "resource/run/run_123",
  "time": "2026-05-26T12:00:00Z",
  "bear_id": "bear_123",
  "role": "pair",
  "role_agent_id": "agent_123",
  "human_id": "human_123",
  "session_id": "ses_123",
  "run_id": "run_123",
  "resource_refs": [
    {
      "kind": "run",
      "id": "run_123"
    }
  ],
  "data": {}
}
```

#### Required envelope fields

| Field | Meaning |
| --- | --- |
| `event_id` | Unique event id. |
| `type` | Stable BearWire event type. |
| `source` | Emitting component. |
| `time` | Event timestamp. |
| `scope` | `persistent` or `ephemeral`. |
| `data` | Event-specific payload. |

#### Recommended envelope fields

| Field | Meaning |
| --- | --- |
| `sequence` | Monotonic sequence within replay scope. |
| `subject` | Primary resource-oriented subject string. |
| `bear_id` | Bear identity. |
| `role` | Role identity. |
| `role_agent_id` | Role agent id. |
| `human_id` | Authenticated human id. |
| `session_id` | Session id. |
| `run_id` | Run id. |
| `resource_refs` | Related resources. |

### Subject naming

Recommended subject forms:

```text
resource/session/ses_123
resource/run/run_123
resource/message/msg_123
resource/tool_call/tc_123
resource/workspace/repo_123
resource/permission_request/perm_123
```

## Event types and payloads

### Session events

#### `session.opened`

```json
{
  "session_id": "ses_123"
}
```

#### `session.bound`

Use when the session is bound to a backing runtime context.

```json
{
  "session_id": "ses_123",
  "binding": {
    "conversation_id": "conv_123",
    "agent_id": "agent_123",
    "role": "pair"
  }
}
```

#### `session.resumed`

```json
{
  "session_id": "ses_123",
  "last_event_id": "evt_000041",
  "last_sequence": 41
}
```

#### `session.state`

```json
{
  "session_id": "ses_123",
  "active_run_ids": ["run_123"],
  "state": "active"
}
```

#### `session.closed`

```json
{
  "session_id": "ses_123",
  "reason": "normal"
}
```

#### `session.invalidated`

```json
{
  "session_id": "ses_123",
  "reason": "expired"
}
```

### Run events

#### `run.accepted`

```json
{
  "run_id": "run_123",
  "session_id": "ses_123"
}
```

#### `run.started`

```json
{
  "run_id": "run_123",
  "session_id": "ses_123",
  "run_kind": "turn"
}
```

#### `run.progress`

```json
{
  "run_id": "run_123",
  "kind": "status_text",
  "text": "Reviewing workspace files"
}
```

Recommended `kind` values include:

- `status_text`
- `phase`
- `queue`
- `heartbeat`

#### `run.paused`

```json
{
  "run_id": "run_123",
  "reason": "awaiting_continuation",
  "resume_token": "rsm_456",
  "expires_at": "2026-05-26T12:00:00Z"
}
```

Recommended pause reasons include:

- `awaiting_continuation`
- `awaiting_input`
- `awaiting_approval`
- `awaiting_resource_binding`

#### `run.resumed`

```json
{
  "run_id": "run_123",
  "resume_token": "rsm_456"
}
```

#### `run.completed`

```json
{
  "run_id": "run_123",
  "status": "completed"
}
```

#### `run.failed`

```json
{
  "run_id": "run_123",
  "status": "failed",
  "category": "runtime_error",
  "message": "Tool execution failed"
}
```

#### `run.cancelled`

```json
{
  "run_id": "run_123",
  "status": "cancelled",
  "reason": "user_cancelled"
}
```

#### `run.expired`

```json
{
  "run_id": "run_123",
  "reason": "resume_window_expired"
}
```

#### `run.warning`

```json
{
  "run_id": "run_123",
  "category": "degraded_mode",
  "message": "Replay unavailable; snapshot required"
}
```

### Message events

#### `message.started`

```json
{
  "message_id": "msg_123",
  "run_id": "run_123",
  "index": 0
}
```

#### `message.delta`

```json
{
  "message_id": "msg_123",
  "run_id": "run_123",
  "index": 0,
  "delta": "Hello"
}
```

#### `message.part`

```json
{
  "message_id": "msg_123",
  "run_id": "run_123",
  "part_kind": "citation",
  "value": {
    "title": "README.md"
  }
}
```

#### `message.completed`

```json
{
  "message_id": "msg_123",
  "run_id": "run_123"
}
```

#### `message.aborted`

```json
{
  "message_id": "msg_123",
  "run_id": "run_123",
  "reason": "run_cancelled"
}
```

### Tool-call events

#### `tool_call.requested`

```json
{
  "tool_call_id": "tc_123",
  "run_id": "run_123",
  "tool_name": "acp.fs.read_text_file",
  "arguments": {
    "path": "/workspace/README.md"
  }
}
```

#### `tool_call.dispatched`

```json
{
  "tool_call_id": "tc_123",
  "runtime": {
    "kind": "acp_adapter",
    "id": "adapter_local"
  }
}
```

#### `tool_call.blocked`

```json
{
  "tool_call_id": "tc_123",
  "run_id": "run_123",
  "reason": "permission_required",
  "permission_request_id": "perm_123"
}
```

#### `tool_call.started`

```json
{
  "tool_call_id": "tc_123",
  "run_id": "run_123"
}
```

#### `tool_call.progress`

```json
{
  "tool_call_id": "tc_123",
  "run_id": "run_123",
  "message": "Reading file"
}
```

#### `tool_call.completed`

```json
{
  "tool_call_id": "tc_123",
  "run_id": "run_123",
  "result": {
    "status": "OK"
  }
}
```

#### `tool_call.failed`

```json
{
  "tool_call_id": "tc_123",
  "run_id": "run_123",
  "category": "permission_denied",
  "message": "The user denied editing this file."
}
```

#### `tool_call.cancelled`

```json
{
  "tool_call_id": "tc_123",
  "run_id": "run_123",
  "reason": "run_cancelled"
}
```

### Permission events

#### `permission.requested`

```json
{
  "permission_request_id": "perm_123",
  "tool_call_id": "tc_123",
  "permission_class": "edit_files",
  "display": {
    "title": "Edit README.md",
    "approval_summary": "Allow editing this workspace file."
  }
}
```

#### `permission.granted`

```json
{
  "permission_request_id": "perm_123"
}
```

#### `permission.denied`

```json
{
  "permission_request_id": "perm_123",
  "reason": "user_denied"
}
```

#### `permission.expired`

```json
{
  "permission_request_id": "perm_123"
}
```

### Resource events

#### `resource.detected`

```json
{
  "binding_target": {
    "kind": "session",
    "id": "ses_123"
  },
  "resource": {
    "kind": "workspace",
    "id": "repo_123",
    "uri": "git+https://github.com/example/project"
  },
  "confidence": 0.96,
  "evidence": {
    "cwd": "/Users/alice/dev/project",
    "git_remote": "git@github.com:example/project.git",
    "branch": "main"
  }
}
```

#### `resource.bound`

```json
{
  "binding_target": {
    "kind": "session",
    "id": "ses_123"
  },
  "resource": {
    "kind": "workspace",
    "id": "repo_123"
  }
}
```

#### `resource.updated`

```json
{
  "resource": {
    "kind": "workspace",
    "id": "repo_123",
    "version": "main"
  },
  "changes": {
    "branch": "main"
  }
}
```

#### `resource.unbound`

```json
{
  "binding_target": {
    "kind": "session",
    "id": "ses_123"
  },
  "resource": {
    "kind": "workspace",
    "id": "repo_123"
  },
  "reason": "session_closed"
}
```

#### `resource.rejected`

```json
{
  "binding_target": {
    "kind": "session",
    "id": "ses_123"
  },
  "resource": {
    "kind": "workspace",
    "id": "repo_123"
  },
  "reason": "user_rejected"
}
```

### Diagnostic and governance events

#### `diagnostic.reported`

```json
{
  "category": "transport",
  "severity": "warning",
  "message": "Replay unavailable; snapshot required"
}
```

#### `health.reported`

```json
{
  "component": "bears-acp-adapter",
  "status": "ok"
}
```

#### `version.reported`

```json
{
  "runtime": {
    "name": "bears-acp-adapter",
    "version": "0.1.0",
    "build_git_sha": "abc123"
  }
}
```

## Method families

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

```text
run.start
run.resume
run.cancel
run.result
run.ack
```

If compatibility requires an operation-oriented transport family, implementations may retain transitional method names, but the BearWire semantic model remains run-oriented.

### Event methods

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

## Replay and resume

Replay is bounded and explicit.

A runtime may request session resume using:

```json
{
  "jsonrpc": "2.0",
  "id": "resume_123",
  "method": "session.resume",
  "params": {
    "session_id": "ses_123",
    "last_event_id": "evt_000042",
    "last_sequence": 42
  }
}
```

Replay may yield:

- missed events replayed in sequence;
- session resumed but replay unavailable;
- session unknown;
- session expired;
- unauthorized.

Replay retention is intentionally bounded in v1.

## Error model

BearWire uses:

- JSON-RPC errors for request/response boundary failures;
- lifecycle events for terminal run and tool failures; and
- warning/diagnostic events for recoverable issues.

It should avoid using a single generic streamed `error` event as the primary model for all failure types.

## Compatibility guidance

Implementations migrating from older event models should prefer these mappings:

| Older semantic label | BearWire event type |
| --- | --- |
| assistant text delta | `message.delta` |
| status text | `run.progress` with `kind: "status_text"` |
| tool call requested | `tool_call.requested` |
| waiting for continuation | `run.paused` with `reason: "awaiting_continuation"` |
| turn completed | `run.completed` |
| turn failed | `run.failed` |
| turn cancelled | `run.cancelled` |

## Open design questions

- Which event types require durable persistence versus ephemeral delivery only?
- Whether `run.accepted` should always precede `run.started` or remain optional.
- Which resource kinds deserve stronger first-class schemas in v1.
- Whether `session.bound` and `resource.bound` need additional normalization rules in multi-binding flows.
