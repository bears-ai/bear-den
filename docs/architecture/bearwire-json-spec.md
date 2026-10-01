# BearWire JSON specification

**Status:** Draft  
**References:** [ADR-0030: BearWire resource-oriented event model](../decisions/adr-0030-bearwire-resource-oriented-event-model.md), [ADR-0007: BearWire protocol](../decisions/adr-0007-bearwire-protocol.md), [ADR-0034: BearWire as the Den ↔ armature wire](../decisions/adr-0034-bearwire-as-den-armature-wire.md)

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

BearWire v1 supports an HTTP profile for JSON-RPC control methods, durable server-owned event pages, and a bounded best-effort livestream feed. A future WebSocket profile may combine durable replay and live delivery on a long-lived push binding.

### v1 HTTP profile

Control methods use JSON-RPC 2.0 over HTTP:

```text
POST https://<den-host>/bearwire/v1/rpc
Authorization: Bearer <token>
BearWire-Version: 1
Content-Type: application/json
```

Events are fetched through the server-owned JSON page endpoint:

```text
GET https://<den-host>/bearwire/v1/sessions/{session_id}/events/page?after=<sequence>&limit=<n>
Authorization: Bearer <token>
BearWire-Version: 1
Accept: application/json
```

Response:

```json
{
  "ok": true,
  "session_id": "acp-...",
  "events": [
    {
      "sequence": 234820,
      "event": { "type": "tool_call.requested", "data": {} }
    }
  ],
  "next_after": 234820,
  "has_more": false
}
```

`next_after` is authoritative for the durable event sequence. Polling clients must feed it back as the next `after` cursor instead of deriving a cursor from the maximum sequence they successfully processed. If a page is empty, `next_after` remains the previous cursor; clients should not advance merely because an HTTP response was received.

The page contains **persistent** events only. Events whose envelope scope is `ephemeral` are delivered on the livestream feed and are neither assigned a durable sequence nor replayed from this endpoint. A livestream client must obtain a derived current snapshot when it connects or reconnects, then treat subsequent ephemeral updates as best-effort replaceable observations.

### WebSocket profile (preferred future binding)

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

### Armature-actionable obligation invariant

Den must not emit an armature-actionable wait event unless the exact corresponding BearWire obligation has already been durably persisted and can be answered by the method named in the event.

For permission-mediated tool calls this means:

1. Den creates or updates the `client.permission.result` obligation first.
2. The streamed event includes `data.obligation_id`, `data.expected_client_method`, `data.tool_call`, and `data.permission`.
3. The armature renders permission UI only from that answerable event and returns the user's decision against the referenced obligation.
4. Den validates that returned results match the persisted obligation's run, session, tool call, permission id, expected client method, and open state before continuing the run.

For armature-local tools that do not require permission, Den creates a `client.tool.result` obligation but does not emit `client.waiting`; the armature answers the obligation after handling `tool_call.requested`. Den-owned/display-only tools must not create armature client obligations.

This invariant prevents unanswerable permission prompts and avoids reconstructing continuation state from loosely matched permission IDs, transcript text, or rendered error strings.

### Replayable tool activity invariant

Tool activity carried over BearWire must be self-describing enough to replay into a future model transcript and to project into a human UI without edge-local archaeology.

For every model-relevant tool call, Den must be able to persist and later reconstruct:

- the assistant tool-call part: stable `tool_call_id`, canonical `tool_name`, optional human title, and typed arguments;
- the corresponding tool-result part: same `tool_call_id`, same `tool_name`, status, structured result or structured error, and bounded text output/summary;
- the surface projection: visible input and output/error summaries plus bounded raw/structured detail.

A `tool_call.completed` event that only carries `{ "status": "OK" }`, or UI content such as `Tool completed`, is not sufficient as the sole durable/projection source. If a later event is intentionally sparse, the referenced tool-call record must already be persisted and queryable by `tool_call_id`; otherwise the completion event must repeat enough detail for replay.

### Model-context visibility invariant

Every non-secret item delivered to a model must be durably represented so a user-visible surface can render it without guessing from edge-local state. The compact projection may be a structured runtime card, but expansion must expose the exact delivered content or a stable reference, source and triggering reason, canonical order, invocation/run identity, and redaction status.

The record must distinguish whether content was shown to the user, persisted in the transcript, sent to the model, or used to derive another context artifact. These are independent facts. A protected value may be withheld only through explicit, visible redaction metadata; it must not disappear silently.

### Non-blocking structured update invariant


Examples include conversation-title updates, advisory in-flight task status, and durable work-progress metadata. They may be persisted and projected immediately, but they must not by themselves create open client obligations, require `client.tool.result`, or trigger model continuation. If an update changes state the model must observe before safely continuing, represent it as a blocking tool exchange or client obligation instead.

Model-facing names may remain concrete and tool-like for legibility (`set_conversation_title`, `report_progress`, `update_task_status`), but descriptor metadata owns whether the runtime treats the action as blocking, non-blocking, durable, replayable, or surface-only.

### Message/reasoning separation invariant

`message.delta` is assistant answer content only. Den must not project provider/model reasoning, thinking, scratchpad, checkpoint synthesis, status text, or diagnostic progress as `message.delta`.

Provider/model reasoning belongs on `message.reasoning.delta`. Runtime status/progress belongs on `run.progress`. Clients that receive a malformed compatibility event carrying reasoning/thinking metadata on `message.delta` must treat it as reasoning display, not assistant answer content, and should not count it as visible assistant output for completion/liveness checks.

This invariant prevents private/provisional model deliberation from becoming user-visible transcript text and keeps assistant answers, reasoning display, and runtime progress separately projectable.

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

The current HTTP/SSE event-polling profile may emit a synthetic `session.state` for an initial empty poll when no cursor is provided. Incremental empty polls with a cursor should return no events rather than repeatedly emitting synthetic `session.state`; otherwise liveness diagnostics are obscured by heartbeat-like state spam.

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
  "state": "active",
  "open_obligations": [
    {
      "id": "obl_123",
      "run_id": "run_123",
      "kind": "tool_result",
      "expected_responder_action": "tool_result",
      "tool_call_id": "tc_123",
      "permission_id": null,
      "state": "waiting_for_client"
    }
  ]
}
```

`open_obligations` is optional and diagnostic. When present, it exposes Den-owned liveness state so armatures can report or recover from missed local-tool/permission obligations without inferring state from rendered transcript text.

#### `diagnostic.state_transition`

A persistent, replayable tracing event for a major Den-owned control-plane transition. It records the transition accepted by the authoritative reducer; it is not a second state store, a command acknowledgement, or authority for subsequent execution.

```json
{
  "aggregate": "pair_execution",
  "resource_id": "ses_123",
  "transition": "start_accepted",
  "from": "paused",
  "to": "running",
  "reason_code": "focus_command",
  "correlation_id": "cmd_123",
  "causation_event_id": "evt_000041",
  "state_version": 18,
  "refs": {
    "task_id": "task_123",
    "execution_run_id": "run_123",
    "attempt_id": "attempt_123",
    "fence": 7
  },
  "summary": "Pair execution acquired and first slice queued"
}
```

Emit this event for major, user-relevant transitions such as execution start/acquisition, pause, resume, steering interruption, obligation wait/clear, reconciliation, terminal settlement, and rejected transitions. Do not emit every internal write or heartbeat. Payloads must be bounded, typed, redacted, and derived from the same transition result that commits authoritative state; free-form `summary` is presentation help, never machine authority.

These events use `scope = "persistent"` and the canonical session replay stream so post-incident views can reconstruct what Den decided without scraping logs or fabricated transcript messages. They are control-plane transcript artifacts, not model-role messages: normal model-history projection excludes them unless a context policy explicitly requests a bounded diagnostic summary.

Den sends authorized subscribers the same event stream regardless of whether a debug panel is open. Debug visibility is client-local presentation state; opening it must not mutate session state or change which transitions Den records. Subscription authorization and resource scope remain Den-side. A future bandwidth-sensitive filtered subscription may omit diagnostics only as an explicit transport optimization with cursor/replay semantics; it must not make recording conditional on a session debug flag. Clients that do not render this event must ignore it as an optional compatibility event.

For inspectability, an initial `session.state` snapshot should include the current authoritative aggregate and version, while subsequent `diagnostic.state_transition` events explain changes. Clients detect a sequence/version gap and reload the snapshot rather than inventing intermediate state.

#### `runtime.objective_orientation`

A non-blocking, advisory runtime-context update. It describes the current conversation objective/orientation for diagnostics and compatible surface projections. It does not create a client obligation, alter run liveness, require a response, or require model continuation.

```json
{
  "conversation_id": "conv_123",
  "kind": "focused",
  "orientation": {
    "job": {
      "active_task_ref": {
        "job_id": "job_123",
        "task_id": "task_123"
      }
    }
  }
}
```

Armatures that do not project this metadata must ignore it. They should classify it as an optional compatibility event rather than reporting it as an unknown protocol event.

#### `model.selection.changed`

Emitted when the conversation-scoped model selection for a BearWire/ACP session changes.

```json
{
  "session_id": "ses_123",
  "conversation_id": "den-conv-123",
  "selection_mode": "explicit",
  "selected_model": "openai/gpt-4.1"
}
```

`selection_mode = "auto"` means the session inherits Den's Bear/profile model policy for the conversation. `selected_model` may be `null` in auto mode.

#### `session.metadata.updated`

Non-blocking structured update for durable or replayable session metadata.

```json
{
  "session_id": "ses_123",
  "conversation_id": "den-conv-123",
  "updates": {
    "conversation_title": "Fix Coolify sandbox roots"
  },
  "replay_policy": "replay",
  "model_visibility": "surface_only"
}
```

This event is a notification, not a client wait. It does not require a tool result or model continuation. Clients may project it live and replay it into session load/history UI according to `replay_policy`.

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

#### Terminal run event invariant

For a persisted run, `run.completed`, `run.failed`, and `run.cancelled` are the durable completion contract. Den commits the terminal run state, obligation settlement, active-step settlement, and exactly one matching terminal event in one transaction. Clients must not infer run completion from assistant output, tool completion/failure, progress events, or stream EOF.

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

#### `work.progress.updated`

Non-blocking structured update for durable or semi-durable work progress that should be visible to humans or future context assembly, but does not supply data needed for immediate model continuation.

```json
{
  "run_id": "run_123",
  "task_ref": "task_456",
  "status": "in_progress",
  "summary": "Found likely watchdog mismatch after local tool result.",
  "advisory": true,
  "replay_policy": "replay"
}
```

Use `run.progress` for ephemeral status text; use `work.progress.updated` or a task-specific update when the state is durable enough to replay or feed later context. Terminal task state changes may still require a blocking gate, validation, or handoff.

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

Assistant answer-content delta. This is the content that clients may render as the Bear's user-visible assistant message.

```json
{
  "message_id": "msg_123",
  "run_id": "run_123",
  "index": 0,
  "delta": "Hello"
}
```

Rules:

- assistant answer content only;
- may be persisted/replayed according to conversation transcript policy;
- must not carry provider reasoning, thinking, status/progress, checkpoint reports, or diagnostics;
- clients should treat reasoning-tagged `message.delta` compatibility payloads as malformed reasoning display and render them as thought, not assistant content.

#### `message.reasoning.delta`

Provider/model reasoning delta intended for live deliberation display.

```json
{
  "message_id": "msg_123",
  "run_id": "run_123",
  "index": 0,
  "delta": "I should inspect the relevant file first.",
  "source": "provider_reasoning",
  "replay_policy": "none"
}
```

Rules:

- display-only by default;
- not assistant answer content;
- not included in model transcript replay;
- not persisted as canonical conversation history;
- not Docket/task state;
- should render as thought/deliberation UI when the client supports it;
- must not satisfy assistant-output/completion checks;
- clients that do not support reasoning display may ignore it.

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
  "tool_call": {
    "id": "tc_123",
    "name": "fs_read_text_file",
    "title": "Read file",
    "kind": "function",
    "arguments": {
      "path": "/workspace/README.md",
      "limit": 2000
    },
    "display": {
      "input_summary": "Read /workspace/README.md"
    }
  },
  "approval_required": false,
  "execution_target": "armature_local",
  "policy": {
    "risk": "read_only",
    "permission_class": "read_files"
  }
}
```

`tool_call.arguments` are typed JSON arguments intended for replay, not a rendered string. `tool_call.display.input_summary` is optional but recommended so edge adapters can render meaningful UI without guessing from tool-specific argument names. `execution_target` is descriptor-owned; currently expected values are `armature_local` and `den`. Armatures execute only `armature_local` requests and treat `den` requests as display/replay state.

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

#### `client.waiting`

Canonical BearWire v1 event for an armature-actionable wait. Permission waits use nested `tool_call` and `permission` objects so the armature does not infer prompt state from flat or legacy fields.

```json
{
  "obligation_id": "obl_123",
  "expected_responder_action": "permission_decision",
  "expected_client_method": "client.permission.result",
  "turn_step_id": "step_001",
  "tool_call": {
    "id": "tc_123",
    "name": "fs_edit_file",
    "title": "Edit file",
    "kind": "function",
    "arguments": {
      "path": "/workspace/README.md"
    },
    "display": {
      "input_summary": "Edit /workspace/README.md"
    }
  },
  "permission": {
    "id": "perm_123",
    "reason": "permission_required"
  },
  "approval_required": true,
  "execution_target": "armature_local",
  "policy": {
    "risk": "write",
    "permission_class": "edit_files"
  }
}
```

The event must include a `client_obligation` resource ref and the `obligation_id` must identify a persisted open obligation. `expected_client_method` is the method the armature must call to answer the wait; current permission waits use `client.permission.result`.

#### `tool_call.blocked` legacy projection

Older BearWire draft projections used `tool_call.blocked` for permission-mediated waits:

```json
{
  "tool_call_id": "tc_123",
  "run_id": "run_123",
  "reason": "permission_required",
  "permission_request_id": "perm_123"
}
```

New Den ↔ armature code should prefer `client.waiting`. Armatures may accept `tool_call.blocked` during migration only when it contains enough information to answer a persisted permission obligation.

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
  "tool_call": {
    "id": "tc_123",
    "name": "fs_read_text_file"
  },
  "status": "ok",
  "summary": "Read /workspace/README.md (62,357 bytes; truncated)",
  "content": "# README\n...",
  "structured_content": {
    "bytes": 62357,
    "truncated": true
  },
  "compacted": {
    "output_summary": "Read /workspace/README.md (62,357 bytes; truncated)",
    "output_preview": "# README\n..."
  }
}
```

The completion event must either repeat `tool_call.name` and replay-relevant result fields or reference an already-persisted tool-call record by `tool_call.id`. `summary`, `content`, `structured_content`, and `compacted.output_summary`/`output_preview` are bounded presentation/model-continuity helpers; Den's durable tool-result record remains keyed by the same tool-call id and status.

#### `tool_call.failed`

```json
{
  "tool_call": {
    "id": "tc_123",
    "name": "fs_read_text_file"
  },
  "status": "error",
  "summary": "Could not read /workspace/PLAN.md: file not found",
  "error_message": "File not found: /workspace/PLAN.md",
  "error": {
    "category": "resource_not_found",
    "message": "File not found: /workspace/PLAN.md",
    "retryable": false
  },
  "compacted": {
    "output_summary": "Could not read /workspace/PLAN.md: file not found"
  }
}
```

Tool execution errors are normal tool results for model replay unless the BearWire transport or coordinator itself failed. They must be tied to the same `tool_call.id` so the next model turn can see what the agent tried and what happened.

#### `tool_call.cancelled`

```json
{
  "tool_call": {
    "id": "tc_123",
    "name": "fs_read_text_file"
  },
  "status": "cancelled",
  "summary": "Tool call cancelled"
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
hats.list
session.hat.select
session.model.get
session.model.set
```

`hats.list` requires authenticated Bear membership and may take `session_id`; it returns Bear-owned hats, the Bear's optional IDE default hat ID, and the session conversation's selected hat ID. `session.hat.select` takes `session_id` and an exact hat UUID. Den only changes the authenticated human's canonical conversation while it has no persisted messages or turns and no Work binding. This is a first-interaction choice, not a connection-level permission or a way to switch hats after a turn. The armature's `/hat` command supplies the human-facing selection; normal IDE sessions bind the configured default on creation.

`session.model.get` returns conversation-scoped model state for the session, including `selection_mode`, `requested_model`, `selected_model`, `effective_model`, and `model_options` for ACP UI controls.

`session.model.set` accepts:

```json
{
  "session_id": "ses_123",
  "selection_mode": "explicit",
  "model": "openai/gpt-4.1"
}
```

or auto/inherit mode:

```json
{
  "session_id": "ses_123",
  "selection_mode": "auto",
  "model": null
}
```

Den validates explicit models against Bifrost availability. The selected model is conversation-scoped and should stick for subsequent turns in that session/conversation.

### Run methods

```text
run.start
run.cancel
run.state
run.timeline
```

`run.state` returns Den's current run row plus obligations, results, and recent BearWire events. It is the recovery/diagnostic source of truth when an armature suspects an event stream missed a terminal event or local-client obligation. Tool execution ownership in recovered obligations remains descriptor-owned: Den-owned requests are display-only to armatures, while armature-local requests may be serviced as client obligations.

If compatibility requires an operation-oriented transport family, implementations may retain transitional method names, but the BearWire semantic model remains run-oriented.

### Event methods

```text
event
event.ack
event.replay
```

### Tool and permission methods

```text
client.tool.claim
client.tool.renew
client.tool.result
client.permission.result
```

`client.tool.claim` transactionally claims an open `tool_result` obligation for armature-local execution. Only after claim succeeds may the armature invoke the tool. The request identifies the session, run, obligation, and tool call:

```json
{
  "jsonrpc": "2.0",
  "id": "claim_123",
  "method": "client.tool.claim",
  "params": {
    "session_id": "ses_123",
    "run_id": "run_123",
    "obligation_id": "0191d6cc-6e92-7aa0-a738-29f92354baf1",
    "tool_call_id": "call_123"
  }
}
```

A successful response returns an opaque attempt token and server-selected timing:

```json
{
  "jsonrpc": "2.0",
  "id": "claim_123",
  "result": {
    "ok": true,
    "status": "claimed",
    "attempt_token": "opaque-bearer-token",
    "lease_expires_at": "2026-07-30T10:33:00Z",
    "renew_after_ms": 10000
  }
}
```

A concurrent or stale claim returns `{"ok":false,"status":"claim_rejected"}` and does not authorize execution. The caller must reconcile through `run.state`; it must not invoke or retry the tool.

`client.tool.renew` conditionally extends the claimed obligation using the same durable identity plus the attempt token:

```json
{
  "jsonrpc": "2.0",
  "id": "renew_123",
  "method": "client.tool.renew",
  "params": {
    "session_id": "ses_123",
    "run_id": "run_123",
    "obligation_id": "0191d6cc-6e92-7aa0-a738-29f92354baf1",
    "tool_call_id": "call_123",
    "attempt_token": "opaque-bearer-token"
  }
}
```

Successful renewal returns `ok: true`, `status: "renewed"`, a new `lease_expires_at`, and the next `renew_after_ms`:

```json
{
  "jsonrpc": "2.0",
  "id": "renew_123",
  "result": {
    "ok": true,
    "status": "renewed",
    "lease_expires_at": "2026-07-30T10:33:10Z",
    "renew_after_ms": 10000
  }
}
```

A stale, expired, or otherwise lost claim returns `{"ok":false,"status":"lease_lost"}`. `lease_lost` is definitive: the armature stops treating itself as execution owner and reconciles `run.state`; it never launches a replacement command.

Den's database clock determines lease deadlines. The current server policy grants 30-second leases and asks for renewal after 10 seconds. These are defaults, not client protocol constants: clients must schedule from each successful response's `renew_after_ms` and treat `lease_expires_at` as diagnostic/server authority rather than derive it locally. A transient renewal transport error is not the same as `lease_lost`; clients may retry while preserving the same attempt identity, but cannot assume ownership beyond Den's last confirmed deadline.

`client.tool.result` answers a claimed `tool_result` obligation and includes its attempt token:

```json
{
  "jsonrpc": "2.0",
  "id": "result_123",
  "method": "client.tool.result",
  "params": {
    "session_id": "ses_123",
    "run_id": "run_123",
    "tool_call_id": "call_123",
    "attempt_token": "opaque-bearer-token",
    "status": "ok",
    "tool_name": "run_command",
    "content": "tests passed",
    "structured_content": {"exit_code": 0}
  }
}
```

Identical duplicate submissions are idempotent; conflicting or stale submissions are rejected without starting continuation. Result, expiry, cancellation, and renewal are compare-and-set transitions with one canonical winner.

The attempt token is a bearer capability for one execution attempt. It must be transmitted only in claim-dependent requests and must not be returned by `run.state`, event history, snapshots, logs, or user-visible diagnostics. `run.state` may expose typed lease status and expiry so reconnecting clients can inspect safely without acquiring execution authority.

`client.permission.result` answers an open `permission_decision` obligation emitted via `client.waiting`. A client-local approval is followed by a separately claimed armature-local tool obligation; an approved Den-hosted `web_fetch` instead resumes Den's own tool execution. The typed `allow_hat_host` decision is accepted only for a canonical Den web-fetch obligation and a current Bear-admin conversation creator. Once the result wins the obligation, Den derives the hat and exact HTTPS host from the canonical conversation and stored request, writes its tool+host grants together, and then resumes the original fetch. A duplicate or late result cannot create the grant. The armature cannot manufacture a hat grant from a client-supplied hat ID or a displayed permission label.

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

Unclaimed client obligations that remain unanswered past their deadline fail the run with `reason`/`error_type` such as `client_obligation_timeout`. A claimed armature-local command is different: while its lease is current it remains an open running obligation, and expiry after execution was claimed fails with `outcome_unknown` because the command may still be running or may already have made changes. That outcome prohibits automatic re-execution and directs recovery through `run.state` plus process/workspace inspection. Failure to fetch event pages alone is not a run outcome; clients reconcile `run.state` before synthesizing a terminal transport error.

Failure events should include enough context for the armature and user to identify the affected obligation without inspecting Den internals; `run.state` remains the structured recovery endpoint for full obligation/result/event details.

After Den accepts `client.tool.result` or `client.permission.result` and starts model continuation, continuation liveness is phase-aware. Before the first resumed runtime event, the watchdog must allow the native LLM handshake window plus the continuation idle window; after the first runtime event, the shorter continuation idle window applies between events. `continuation_watchdog_timeout` context may include `watchdog_phase`, `handshake_timeout_ms`, `idle_watchdog_timeout_ms`, `first_event_watchdog_timeout_ms`, `runtime_event_count`, and `last_event_kind`.

It should avoid using a single generic streamed `error` event as the primary model for all failure types.

## Compatibility guidance

Implementations migrating from older event models should prefer these mappings:

| Older semantic label | BearWire event type |
| --- | --- |
| assistant text delta | `message.delta` |
| provider reasoning delta | `message.reasoning.delta` |
| status text | `run.progress` with `kind: "status_text"` |
| tool call requested | `tool_call.requested` |
| waiting for continuation | `run.paused` with `reason: "awaiting_continuation"` |
| turn completed | `run.completed` |
| turn failed | `run.failed` |
| turn cancelled | `run.cancelled` |

Compatibility rule: if an older or malformed stream sends reasoning/thinking content as `message.delta` with fields such as `kind=reasoning_delta`, `source=provider_reasoning`, `reasoning`, `thinking`, or `thought`, clients must reclassify it as reasoning display. They must not render it as assistant answer text or count it as visible assistant output.

## Open design questions

- Whether `run.accepted` should always precede `run.started` or remain optional.
- Which resource kinds deserve stronger first-class schemas in v1.
- Whether `session.bound` and `resource.bound` need additional normalization rules in multi-binding flows.
