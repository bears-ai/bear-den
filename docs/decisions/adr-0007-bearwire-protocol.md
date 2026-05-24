# ADR: BearWire protocol

**Status:** Proposed  
**Date:** 2026-05-21  
**Deciders:** Hans

## Context

BEARS currently has several protocol boundaries:

- ACP clients talk to `bears-acp-adapter` over ACP JSON-RPC on stdio.
- `bears-acp-adapter` talks to Den over a BEARS-private HTTPS/SSE transport.
- Den routes ACP `pair` traffic to the `pair` role through the Letta conversation API.
- Den web chat and other channel surfaces use their own browser/channel-facing contracts.
- Future external agent interoperability may use Agent2Agent (A2A).
- Future remote-control or multi-device UX may use a Happy-like sync protocol.

The current Den ⇄ adapter transport was intentionally small and pragmatic. It is now carrying more responsibility:

- session lifecycle;
- prompt/turn execution;
- streaming assistant/status events;
- cancellation;
- local client tool requests;
- user permission mediation;
- adapter capability negotiation;
- diagnostics;
- work-surface hints;
- memory/review signals;
- and potentially desktop-app or remote-daemon administration surfaces.

We need a clearer long-term protocol shape for Den ⇄ trusted BEARS edge runtimes without confusing it with ACP, A2A, OpenAI-compatible APIs, or user-facing sync.

## Decision

BEARS will define **BearWire** as the trusted runtime/control protocol between Den and BEARS-controlled edge or runtime processes.

BearWire's first implementation target is:

```text
ACP client
  ⇄ bears-acp-adapter
    ⇄ BearWire
      ⇄ Den
        ⇄ pair Letta agent
```

BearWire may later also connect Den to:

- a BEARS desktop companion app;
- a remote development daemon;
- a devcontainer or CI runner sidecar;
- a diagnostics collector;
- a Reflection/curate worker;
- a runtime manager replacing or evolving older Den ⇄ runtime contracts.

BearWire is not a public agent interoperability standard. It is a BEARS-owned protocol for Den-authorized runtimes.

## Scope

BearWire owns protocol concerns for Den ⇄ trusted runtime/edge processes:

- connection and capability negotiation;
- authenticated identity and Bear/role authorization context;
- session/task/operation lifecycle;
- streaming runtime events;
- local capability callbacks;
- permission requests and results;
- cancellation;
- reconnect/resume;
- diagnostics and health reporting;
- workspace/work-surface registration;
- memory/review event handoff;
- adapter and runtime version reporting.

BearWire does not own:

- external agent-to-agent interoperability — use A2A;
- browser/mobile remote-control sync — use a Happy-like sync layer if needed;
- OpenAI-compatible provider/model APIs;
- Cabinet or Docket product APIs;
- raw memory storage access;
- Den in-process module calls;
- Letta persistence internals.

## Transport

The preferred BearWire transport is:

```text
JSON-RPC 2.0 over WebSocket
```

This gives BearWire a full-duplex request/response/notification model while staying simple, inspectable, and close to ACP/MCP/LSP/DAP conventions.

Initial connection shape:

```text
wss://<den-host>/bearwire/v1
Authorization: Bearer <token>
BearWire-Version: 1
```

HTTP/SSE compatibility endpoints may remain during migration, but the long-term BearWire shape should assume bidirectional transport.

## Inspirations and boundaries

BearWire borrows deliberately from existing protocols:

| Source | Borrow | Do not borrow |
| --- | --- | --- |
| ACP | client/editor tool and permission semantics | ACP as Den ⇄ adapter transport |
| LSP/DAP | initialize, capabilities, cancellation, progress, request/notification split | editor/debugger-specific models |
| MCP | tool/capability descriptors and structured tool results | treating Den as merely a tool server |
| A2A | task/message/artifact vocabulary where useful | using A2A as the local adapter control protocol |
| CloudEvents | event envelope discipline: id, type, source, subject, time, data | CloudEvents as the whole RPC protocol |
| Happy | persistent vs ephemeral event distinction, reconnect thinking | encrypted multi-device sync semantics |

## Core concepts

### Connection

A BearWire connection is one authenticated channel between Den and a runtime process.

The runtime process may be:

- a local ACP adapter;
- a desktop companion;
- a remote workspace daemon;
- a CI/devcontainer runner;
- a Den-side worker.

The connection is not itself a Bear, a conversation, or a task. It is a runtime capability channel.

### Runtime

A BearWire runtime is a BEARS-controlled process that can expose capabilities to Den.

Examples:

```text
bears-acp-adapter
bears-desktop-companion
bears-remote-daemon
bears-ci-runner
curate-worker
```

### Session

A session is a client/runtime lifecycle binding, such as an ACP session. Sessions are not canonical BEARS conversations. They may map to Letta `conv-*` conversations through Den-owned resolution.

### Operation

An operation is one bounded unit of runtime work, such as one prompt turn, one tool invocation, one reflection run, or one workspace registration action.

Operations have explicit ids and terminal states.

### Task

BearWire may use A2A-inspired task vocabulary for durable or long-running work, but BearWire tasks are BEARS runtime tasks, not necessarily public A2A tasks.

### Event

Events are typed runtime facts emitted by Den or the connected runtime.

Events are either:

- **persistent** — reconstructable or audit-worthy; or
- **ephemeral** — progress, activity, presence, or transient status.

## Initial method families

BearWire v1 should stay narrow.

### Connection lifecycle

```text
initialize
shutdown
heartbeat
```

### Session lifecycle

```text
session.open
session.resume
session.close
session.state
```

### Operation lifecycle

```text
operation.start
operation.cancel
operation.ack
operation.result
```

For ACP, `operation.start` may correspond to sending a prompt turn to Den. Later implementations may introduce more specific methods such as `session.send` if that reads better in code.

### Events

```text
event
 event.ack
 event.replay
```

### Local runtime callbacks

```text
client.tool.call
client.tool.cancel
client.permission.request
client.permission.result
```

### Workspace/work-surface signals

```text
workspace.register
workspace.update
workspace.unregister
work_surface.confirm
work_surface.reject
```

### Diagnostics

```text
diagnostic.report
health.check
version.report
```

### Governed memory and Reflection signals

```text
memory.review_requested
reflection.event
reflection.run_requested
```

These are event/method families, not a final exhaustive API. New families must be added only when they fit BearWire's Den ⇄ trusted-runtime boundary.

## Message envelope

All BearWire JSON-RPC requests use normal JSON-RPC 2.0 framing:

```json
{
  "jsonrpc": "2.0",
  "id": "req_123",
  "method": "session.open",
  "params": {}
}
```

All runtime events should use a common event envelope inside JSON-RPC notifications:

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
    "subject": "session/acp_ses_123",
    "time": "2026-05-21T00:00:00Z",
    "bear_id": "bear_123",
    "role": "pair",
    "role_agent_id": "agent_123",
    "human_id": "human_123",
    "session_id": "acp_ses_123",
    "operation_id": "op_123",
    "data": {}
  }
}
```

Required event envelope fields:

| Field | Meaning |
| --- | --- |
| `event_id` | Unique event id. |
| `type` | Stable event type, such as `message.delta`. |
| `source` | Emitting component, such as `den.pair` or `adapter.local`. |
| `time` | Event timestamp. |
| `scope` | `persistent` or `ephemeral`. |
| `data` | Event-specific payload. |

Recommended fields when applicable:

| Field | Meaning |
| --- | --- |
| `sequence` | Monotonic sequence within the stream/replay scope. |
| `subject` | Resource-like subject, such as `session/<id>`. |
| `bear_id` | Den Bear id. |
| `role` | Bear agent role, such as `pair`. |
| `role_agent_id` | Letta agent id for the selected role. |
| `human_id` | Den-authenticated human id. |
| `session_id` | Client/runtime session id. |
| `operation_id` | Current operation/turn/run id. |
| `work_surface_id` | Resolved work surface, when known. |

## Event taxonomy

Initial event types should use stable dotted names.

Session events:

```text
session.opened
session.resumed
session.closed
session.error
```

Operation events:

```text
operation.started
operation.progress
operation.completed
operation.cancelled
operation.failed
```

Message events:

```text
message.created
message.delta
message.completed
```

Tool events:

```text
tool_call.created
tool_call.progress
tool_call.requires_permission
tool_call.completed
tool_call.failed
tool_call.cancelled
```

Permission events:

```text
permission.requested
permission.granted
permission.denied
permission.expired
```

Workspace and work-surface events:

```text
workspace.registered
workspace.updated
workspace.unregistered
work_surface.candidate
work_surface.confirmed
work_surface.rejected
```

Memory and Reflection events:

```text
memory.review_requested
memory.write_recorded
reflection.run_started
reflection.run_completed
reflection.proposal_created
```

Diagnostics:

```text
diagnostic.reported
health.reported
version.reported
connection.warning
```

## Capability negotiation

The `initialize` request/response must negotiate:

- protocol version;
- runtime name/version/build SHA;
- client type, if applicable;
- supported method families;
- supported local tools;
- supported permission classes;
- supported event types;
- supported replay/resume mode;
- supported max payload size;
- workspace roots and current `cwd`, when applicable;
- adapter/client capabilities for ACP-facing runtimes.

Example runtime capability shape:

```json
{
  "runtime": {
    "name": "bears-acp-adapter",
    "version": "0.1.0",
    "build_git_sha": "...",
    "kind": "acp_adapter"
  },
  "bearwire": {
    "version": 1,
    "methods": ["session.open", "operation.start", "operation.cancel", "client.tool.call"],
    "event_replay": { "supported": true, "scope": "session" }
  },
  "capabilities": {
    "local_tools": {
      "fs_read_text_file": { "supported": true, "version": 1 },
      "fs_replace_text": { "supported": true, "version": 1 }
    },
    "permissions": ["read_files", "edit_files"],
    "workspace": {
      "cwd": "/Users/alice/dev/project",
      "roots": ["/Users/alice/dev/project"]
    }
  }
}
```

Den must treat runtime-advertised local capabilities as authoritative for adapter-executed tools. If a runtime does not advertise a local tool, Den must not expose that local tool to the model for the current operation.

## Tool and permission model

BearWire tool calls must use descriptor-owned names and permission classes.

A Den-originated local tool call looks like:

```json
{
  "jsonrpc": "2.0",
  "id": "tool_req_123",
  "method": "client.tool.call",
  "params": {
    "session_id": "acp_ses_123",
    "operation_id": "op_123",
    "tool_call_id": "call_123",
    "provider_name": "fs_read_text_file",
    "canonical_name": "acp.fs.read_text_file",
    "permission_class": "read_files",
    "arguments": {
      "path": "/Users/alice/dev/project/README.md"
    },
    "display": {
      "title": "Read README.md",
      "approval_summary": "Allow reading this workspace file."
    },
    "policy": {
      "allowed_roots": ["/Users/alice/dev/project"],
      "max_bytes": 200000,
      "approval_required": false
    }
  }
}
```

The runtime must independently enforce local policy and client capability constraints. Den authorization does not force the local runtime to perform unsafe actions.

Permission denials are normal results, not transport failures.

## Identity and authorization

Den is authoritative for:

- human identity;
- Bear identity;
- membership role;
- allowed Bear roles;
- token scopes;
- runtime session bindings;
- tool permission policy;
- admin/operator authorization.

BearWire clients must not self-assert trusted human identity, Bear membership, or authorization scope. They may report local facts such as `cwd`, workspace roots, adapter version, and available local capabilities.

BearWire tokens should be explicit about allowed protocol families. Initial scope names may include:

```text
bearwire:connect
bearwire:session
bearwire:tools
bearwire:workspace
bearwire:diagnostics
bearwire:admin
```

ACP Code tokens may map to a subset of these scopes for adapter use. Admin/operator use must require stronger authentication than normal ACP chat/tool use.

## Desktop app administration

A BEARS desktop app may use BearWire for:

- adapter installation health;
- login and token status;
- local workspace registration;
- adapter diagnostics;
- notifications;
- opening local files/URLs;
- managing local runtime processes;
- presenting Den-originated permission prompts.

A desktop app may also expose Den administration features such as creating, duplicating, provisioning, or reconciling Bears. Those actions remain **Den control-plane operations**, not local runtime capabilities.

Rules for desktop administration:

1. The desktop app acts as an authenticated Den admin/operator client.
2. Den remains the system of record for Bear provisioning, membership, skills, MCP attachments, role agents, and runtime configuration.
3. BearWire may carry admin operation requests only under explicit admin/operator authorization such as `bearwire:admin` plus the required Den membership/operator role.
4. Admin operations must use the same Den policy, audit, validation, and reconciliation paths as the browser operator console and admin JSON APIs.
5. BearWire must not become a backdoor that lets a local machine mutate Bear configuration merely because it is connected.
6. High-risk admin operations should remain explicit, auditable, and user-visible.

In other words: a desktop app can be a Den admin UI, but BearWire does not move Bear provisioning authority out of Den.

## Memory, Reflection, and review events

BearWire may carry memory and Reflection events, especially from `pair` activity into Den/curate workflows.

Examples:

```text
memory.review_requested
reflection.run_requested
reflection.proposal_created
```

BearWire must not provide raw cross-role memory access. In particular:

- `work` must not read raw `pair/` memory through BearWire.
- `pair` may request review of role-local memories.
- Den/Reflection/`curate` decide whether information moves into `core/`, Cabinet, archives, or work task context.
- BearWire memory events are requests, signals, or audit events; they are not the canonical memory store.

This preserves the intended path:

```text
pair/
  → review request
    → curate / Reflection
      → core / archive / Cabinet / task context
        → work
```

## Workspace and work-surface registration

BearWire runtimes may report workspace facts:

- current `cwd`;
- workspace roots;
- Git root;
- Git remotes;
- current branch;
- client/editor name;
- local runtime id;
- observed checkout paths.

Den may use these as evidence for work-surface resolution. They are not execution authorization boundaries by themselves.

A workspace registration may look like:

```json
{
  "jsonrpc": "2.0",
  "id": "req_456",
  "method": "workspace.register",
  "params": {
    "session_id": "acp_ses_123",
    "cwd": "/Users/alice/dev/project",
    "roots": ["/Users/alice/dev/project"],
    "git": {
      "root": "/Users/alice/dev/project",
      "remote": "git@github.com:example/project.git",
      "branch": "main"
    },
    "client": {
      "name": "zed"
    }
  }
}
```

Den may create, update, or confirm work-surface candidates from this evidence, but user confirmation and stronger durable anchors still matter.

## Reconnect and replay

BearWire should support explicit reconnect/resume.

A runtime should track:

- connection id;
- session ids;
- active operation ids;
- last event id;
- last sequence number.

On reconnect, the runtime may call:

```json
{
  "jsonrpc": "2.0",
  "id": "resume_123",
  "method": "session.resume",
  "params": {
    "session_id": "acp_ses_123",
    "last_event_id": "evt_000042",
    "last_sequence": 42
  }
}
```

Den may respond with:

- resumed and replaying missed events;
- resumed but replay unavailable, use snapshot;
- session expired;
- unauthorized;
- unknown session.

BearWire does not require permanent event retention in v1. Bounded replay plus explicit diagnostic failure is acceptable.

## Internal-to-Den use

BearWire has two layers:

1. **BearWire semantic model** — event envelopes, method families, error shapes, capability descriptors.
2. **BearWire wire binding** — JSON-RPC 2.0 over WebSocket across process or network boundaries.

Den in-process code should use normal Rust modules/traits, not JSON-RPC to itself. However, Den-adjacent workers and sidecars may use BearWire across process boundaries.

Good BearWire boundaries:

```text
Den ⇄ bears-acp-adapter
Den ⇄ desktop companion
Den ⇄ remote workspace daemon
Den ⇄ CI/devcontainer runner
Den ⇄ diagnostics worker
Den ⇄ Reflection/curate worker
Den ⇄ future runtime manager
```

Bad BearWire boundaries:

```text
Den handler ⇄ Den service module in the same process
Den auth module ⇄ Den provisioning module in the same process
```

This keeps BearWire from becoming unnecessary internal ceremony while still giving BEARS one runtime event/control vocabulary.

## Error model

BearWire errors should be structured and actionable.

Use standard JSON-RPC errors for framing problems:

```text
-32700 parse error
-32600 invalid request
-32601 method not found
-32602 invalid params
-32603 internal error
```

Use BEARS-specific error data for domain failures:

```json
{
  "jsonrpc": "2.0",
  "id": "req_123",
  "error": {
    "code": -32010,
    "message": "Permission denied",
    "data": {
      "error_type": "permission_denied",
      "component": "adapter",
      "session_id": "acp_ses_123",
      "operation_id": "op_123",
      "tool_call_id": "call_123",
      "required_scope": "bearwire:tools",
      "detail": "The user denied editing this file."
    }
  }
}
```

Domain error types should include:

```text
unauthenticated
authorization_denied
unsupported_method
unsupported_capability
invalid_state
permission_denied
timeout
cancelled
transport_error
replay_unavailable
policy_denied
runtime_unavailable
```

Diagnostics must not include secrets, raw tokens, full file contents, or unbounded command output.

## Versioning

BearWire has a protocol version separate from tool descriptor versions.

Do not bump the BearWire protocol version for:

- adding optional fields;
- adding new event types;
- adding new method families that old clients can ignore;
- adding new tools behind capability negotiation;
- changing display labels or prompt guidance.

Bump the BearWire protocol version for incompatible changes such as:

- removing accepted fields;
- changing required fields;
- changing result settlement semantics;
- changing authentication requirements incompatibly;
- changing event ordering/replay guarantees incompatibly.

Missing BearWire version metadata should remain accepted during migration from the current HTTPS/SSE adapter transport unless Den intentionally ships a breaking BearWire-only endpoint.

## Migration path

1. Keep the current HTTPS/SSE Den ⇄ adapter transport stable while BearWire is designed.
2. Define BearWire core types and event envelopes in docs and tests.
3. Add a BearWire WebSocket endpoint in Den alongside current ACP gateway endpoints.
4. Add BearWire client support to `bears-acp-adapter` behind a flag or capability probe.
5. Move prompt streaming to BearWire typed events.
6. Move local tool callbacks and permission mediation to BearWire requests.
7. Add reconnect/resume and bounded replay.
8. Keep A2A, Happy-like sync, and OpenAI-compatible APIs separate.
9. Deprecate the older Den ⇄ adapter HTTPS/SSE transport only after BearWire has baked across supported clients.

## Alternatives considered

### Use A2A for Den ⇄ adapter

Rejected as the primary adapter transport. A2A is a strong candidate for external agent interoperability, but its core model is agent-to-agent task collaboration, not trusted local runtime control. A2A does not naturally model editor workspace tools, full-duplex Den-to-adapter callbacks, or local permission mediation without BEARS-specific extensions.

### Use Happy's protocol

Rejected as the primary adapter transport. Happy's protocol is useful for remote-control and multi-device sync ideas, especially persistent vs ephemeral events and reconnect thinking. It is broader than the adapter problem and is oriented around encrypted user-level sync rather than Den-authorized runtime control.

### Continue with HTTPS/SSE plus POST result endpoints forever

Rejected as the long-term direction. It works for the current slice but becomes awkward as soon as Den needs bidirectional callbacks, richer permission flows, reconnect/replay, desktop runtime management, diagnostics, and multiple edge/runtime process types.

### Use gRPC/Connect

Deferred. gRPC or Connect could provide stronger schemas and generated clients, but it is heavier to debug and operate for a local adapter/runtime protocol. BearWire can revisit a typed binding later after the JSON-RPC semantics stabilize.

### Use GraphQL subscriptions

Rejected for this boundary. GraphQL is useful for human UI queries and subscriptions, but BearWire is command/event/RPC-heavy and needs server-to-runtime callbacks.

## Consequences

### Positive

- Gives the ACP adapter path a clear long-term protocol identity.
- Keeps Den as the trusted gateway and policy authority.
- Avoids conflating ACP, A2A, Happy-like sync, and OpenAI-compatible APIs.
- Supports local tool callbacks and permission mediation cleanly.
- Creates a shared event vocabulary for diagnostics, Reflection, workspace registration, and future runtimes.
- Lets a desktop app support both local runtime management and Den administration without moving provisioning authority out of Den.
- Provides a migration path from the current HTTPS/SSE adapter transport.

### Trade-offs

- BearWire is another BEARS-owned protocol to specify, test, and support.
- WebSocket lifecycle, reconnect, and replay introduce operational complexity.
- Custom protocol design requires discipline to avoid becoming a generic everything API.
- Desktop administration over BearWire must be carefully scoped so local runtime access does not imply admin authority.
- Some existing docs and troubleshooting runbooks will need updates as BearWire replaces the current private transport.

## Non-goals

- Do not use BearWire as a public external agent interoperability standard.
- Do not use BearWire as a browser/mobile multi-device sync protocol.
- Do not replace Den admin APIs or operator console semantics with local runtime authority.
- Do not move Bear provisioning, membership, skill, MCP attachment, or role-agent reconciliation out of Den.
- Do not expose raw cross-role memory access through BearWire.
- Do not require every Den-internal code path to serialize through JSON-RPC.
- Do not use BearWire to bypass ACP client permission UX for local effects.

## Related documents

- [ACP Session Bindings](acp-session-bindings.md)
- [ACP Conversation Resolver](acp-conversation-resolver.md)
- [Tool Naming and Execution Strategy](tool-naming-and-execution-strategy.md)
- [Pair Tool Discovery and Scope Orientation](pair-tool-discovery-and-scope-orientation.md)
- [Bear work surfaces for planning and work activity](bear-work-surfaces.md)
- [Semantic Bear Memory](semantic-bear-memory.md)
- [Reflection System](reflection-system.md)
- [Bear MCP Services](mcp-services.md)
- [ACP direct local tool runtime implementation plan](../../planning/ACP_DIRECT_LOCAL_TOOL_RUNTIME_PLAN.md)
- [ACP Adapter Improvement Plan](../../planning/ACP_ADAPTER_IMPROVEMENT_PLAN.md)
- [BEARS and Den](../../concepts/BEARS_AND_DEN.md)
- [Identity and Membership](../../concepts/IDENTITY_AND_MEMBERSHIP.md)
- [Agent and Bear Environments](../../concepts/AGENT_AND_BEAR_ENVIRONMENTS.md)
