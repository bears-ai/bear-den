# ACP direct local tool runtime implementation plan

Status: proposed implementation plan.

Owner boundary: `bears-acp-adapter` is the local ACP stdio edge and owns calls to editor/client capabilities. Den is the auth, policy, audit, session, and Letta conversation gateway. Letta remains the durable conversation/memory service. Codepool/Letta Code are not in the ACP direct path.

Related docs:

- [`../architecture/BEAR_CHANNEL_AND_ACP.md`](../architecture/BEAR_CHANNEL_AND_ACP.md)
- [`../architecture/adr/acp-conversation-resolver.md`](../architecture/adr/acp-conversation-resolver.md)
- [`../architecture/adr/acp-session-bindings.md`](../architecture/adr/acp-session-bindings.md)
- [`../architecture/adr/provider-safe-tool-naming.md`](../architecture/adr/provider-safe-tool-naming.md)
- [`archives/ACP_CLIENT_TOOL_RELAY_PLAN.md`](archives/ACP_CLIENT_TOOL_RELAY_PLAN.md) — historical Codepool/Letta Code relay plan; do not use for the direct ACP runtime.
- [`archives/ACP_TOOL_RELIABILITY_PLAN.md`](archives/ACP_TOOL_RELIABILITY_PLAN.md) — historical reliability work for the Codepool waiter model; reuse diagnostics lessons, not the architecture.

---

## Goal

Implement a complete ACP local tooling surface for coding clients while preserving the direct ACP route:

```text
ACP client/editor ⇄ bears-acp-adapter ⇄ Den ACP gateway ⇄ Letta conversation API
```

The local adapter should look like a normal ACP agent to Zed/OpenCode. Den-specific details stay below the adapter boundary except in diagnostic metadata.

Implemented tools should cover, in this order:

1. Read file.
2. List directory.
3. Search files.
4. Edit/write file.
5. Shell/terminal execution.
6. Local MCP server relay.

---

## Key architectural decision

Do **not** resurrect the old Letta Code / Codepool ACP tool relay for direct ACP.

Instead, use a dedicated **Den ACP tool-turn protocol** between Den and the adapter:

1. Den sends the user's prompt to Letta.
2. Den parses native Letta stream events.
3. If Letta asks for an action that requires local workspace access, Den emits an explicit adapter transport event: `tool_request`.
4. The adapter translates `tool_request` into normal ACP client operations such as `fs/read_text_file`, `session/request_permission`, or terminal methods.
5. The adapter sends a `POST` result back to Den.
6. Den resumes the same logical ACP turn by submitting tool output back into the Letta conversation with structured context.
7. The loop continues until Letta produces assistant text or a terminal error.

This is a **turn-level orchestration loop**, not Codepool waiter relay. The live waiter owner for local client operations is the adapter. Den owns turn state, policy, audit, and continuation with Letta.

### Why not raw Letta tools?

Letta server-side tools cannot directly read a user's local editor workspace. The adapter is the only process with access to ACP client capabilities. Therefore local file/shell/MCP tools must terminate at the adapter.

### Why not make the adapter Letta-aware?

The adapter should not parse Letta stream internals or own BEARS authorization. Den remains the Letta anti-corruption layer and policy authority.

---

## Core design principles

1. **Normal ACP at the editor boundary**
   - The adapter calls standard ACP client methods.
   - The adapter emits standard `session/update` tool-call progress notifications.
   - Healthy client logs should look like ordinary ACP, not BEARS internals.

2. **One live waiter owner per hop**
   - Adapter owns editor/client JSON-RPC waiters.
   - Den owns Den/adapter tool-turn correlation and audit.
   - Letta owns durable conversation state.
   - No split-brain pending tool tables.

3. **Every empty turn is an error**
   - If Letta/Den/adapter completes without assistant output, status, or explicit error, the user gets a diagnostic failure.

4. **Structured diagnostics everywhere**
   - Every tool request/result carries correlation ids.
   - Logs identify the failing component: `adapter`, `den`, `letta`, `client`, `mcp`, `terminal`.
   - Errors include enough counts/samples to debug without leaking secrets.

5. **Capability- and policy-gated tools**
   - Adapter reports raw ACP client capabilities.
   - Den filters by token scope, bear policy, user role, workspace context, and risk class.
   - Adapter still checks client capability before invoking any client method.

6. **Progressive privilege**
   - Read-only tools first.
   - Writes, shell, and MCP require explicit permission and stronger audit.

---

## Den ⇄ adapter transport v3

Current direct ACP transport events:

- `assistant_text_delta`
- `status_text`
- `turn_complete`
- `error`
- `conversation_resolved`

Add:

### `tool_request`

```json
{
  "type": "tool_request",
  "request_id": "den-request-uuid",
  "turn_id": "turn-uuid",
  "tool_call_id": "call-uuid",
  "tool_name": "fs_read_text_file",
  "title": "Read file",
  "kind": "read",
  "args": {
    "path": "/absolute/path/to/file.rs",
    "line": 1,
    "limit": 200
  },
  "approval": {
    "required": false,
    "reason": "read-only workspace file"
  },
  "diagnostic": {
    "component": "den.acp",
    "transport_version": 3
  }
}
```

### `tool_progress`

Optional Den-originated progress/status around tool orchestration:

```json
{
  "type": "tool_progress",
  "tool_call_id": "call-uuid",
  "status": "started|waiting_for_permission|running|completed|failed|cancelled|timeout",
  "message": "Reading file"
}
```

### Tool result endpoint

Adapter posts results to Den:

```text
POST /acp/bears/{slug}/sessions/{session_id}/tool-results/{tool_call_id}
```

Payload:

```json
{
  "turn_id": "turn-uuid",
  "request_id": "den-request-uuid",
  "tool_name": "fs_read_text_file",
  "status": "ok|error|cancelled|timeout|permission_denied|unsupported",
  "content": "file contents or short result text",
  "structured_content": {},
  "diagnostic": {
    "adapter_version": "...",
    "client_method": "fs/read_text_file",
    "duration_ms": 42,
    "bytes": 1234,
    "truncated": false
  }
}
```

Den response:

```json
{
  "accepted": true,
  "reason": "delivered|turn_missing|already_settled|stale_turn|invalid_shape",
  "turn_id": "turn-uuid",
  "tool_call_id": "call-uuid"
}
```

---

## Correlation identifiers

Every prompt/turn/tool log line should include:

- `request_id`: Den HTTP request id for the active prompt stream.
- `turn_id`: one logical ACP prompt turn, including tool continuations.
- `tool_call_id`: one tool invocation.
- `acp_session_id`: ACP session id.
- `bear_id` and `bear_slug` where available.
- `conversation_id` / `resolved_conversation_id` where available.
- `adapter_runtime_id`: generated by adapter at startup.
- `client_request_id`: JSON-RPC id used by adapter when calling the ACP client.

---

## Consistent error model

Use these statuses across Den and adapter:

| Status | Meaning | Retry? |
| --- | --- | --- |
| `ok` | Tool completed successfully. | No |
| `error` | Tool ran and failed. | Maybe, depending on detail |
| `cancelled` | User/client cancelled. | User initiated |
| `timeout` | A hop timed out. | Maybe |
| `permission_denied` | User or policy denied action. | No unless policy changes |
| `unsupported` | Client/Den/tool runtime cannot do this action. | No until feature added |
| `invalid_request` | Bad arguments or protocol mismatch. | Fix caller |
| `transport_error` | HTTP/SSE/JSON-RPC transport failed. | Maybe |

Error payload shape:

```json
{
  "type": "error",
  "message": "Human-readable summary",
  "detail": "Actionable diagnostic details",
  "error_type": "unsupported_tool|timeout|permission_denied|invalid_request|transport_error|empty_mapped_turn",
  "request_id": "...",
  "context": {
    "component": "adapter|den|letta|client|mcp|terminal",
    "turn_id": "...",
    "tool_call_id": "...",
    "event_counts": {},
    "samples": []
  }
}
```

Rules:

- No silent `turn_complete` after a failed or empty tool turn.
- Unknown event types are logged with truncated samples.
- Raw file contents are not logged; log byte counts, hashes, path, truncation, and line ranges.
- Permission denials are normal tool results, not infrastructure crashes.

---

## Tool catalog

### 1. `fs_read_text_file` / ACP client `fs/read_text_file`

ACP client method:

- `fs/read_text_file`

Arguments:

- `path`: absolute local path.
- `line`: optional 1-based line start.
- `limit`: optional max lines.

Policy:

- Path must be under the validated session `cwd` or workspace roots unless Den policy explicitly allows otherwise.
- Read-only; no user permission by default for workspace files.
- Den may require permission for files outside roots or hidden/sensitive paths.

Diagnostics:

- path, normalized path, root match, duration, bytes, line range, truncated.
- never log full contents.

### 2. `fs.list_directory`

ACP client method options:

- Prefer a standard ACP filesystem listing method if the client advertises one.
- If the client does not provide directory listing, implement adapter-local read via OS APIs only after explicit Den policy allows it.

Arguments:

- `path`
- `recursive`: false initially
- `limit`
- `include_hidden`: false initially

Policy:

- Root-contained paths only.
- Limit entries and total response size.

Diagnostics:

- path, entry count, truncated flag, duration.

### 3. `fs.search_files`

Implementation options:

- Adapter-local search using Rust filesystem traversal.
- Respect `.gitignore` if practical.
- Do not depend on shell commands for search.

Arguments:

- `query` or regex/pattern
- `path` root
- `include_glob`
- `exclude_glob`
- `limit`

Policy:

- Root-contained paths only.
- Den-configurable maximum results and bytes.

Diagnostics:

- query hash/summary, roots searched, match count, truncated flag, duration.

### 4. `fs.write_text_file` / edit file

ACP client methods:

- `fs/write_text_file`
- future patch/edit method if ACP client supports it.

Arguments:

- `path`
- `content` or patch operations
- optional expected current hash for optimistic concurrency

Policy:

- Requires `acp:tools` and write tool policy enabled.
- Requires `session/request_permission` before write.
- Path must be under workspace root.
- Den may block sensitive paths (`.env`, secrets, credentials) unless explicit elevated policy exists.

Diagnostics:

- path, old hash if known, new hash, bytes changed, permission request id, duration.
- never log full file content.

### 5. `terminal.run_command`

ACP client methods:

- `terminal/create`
- `terminal/output`
- `terminal/wait_for_exit`
- `terminal/kill`
- `terminal/release`

Arguments:

- command argv, not shell string when possible
- cwd
- timeout
- max output bytes

Policy:

- Disabled by default.
- Requires permission every time initially.
- Den may allowlist commands later.
- No long-running background commands in first slice.

Diagnostics:

- command preview, cwd, exit code, timeout, output bytes, duration.
- redact env and secrets.

### 6. Local MCP server relay

ACP client input:

- `mcpServers` from `session/new` / `initialize`.

Plan:

- Phase 1: continue rejecting non-empty MCP servers with a clear unsupported error.
- Phase 2: adapter owns MCP subprocess lifecycle locally.
- Den only receives normalized tool descriptors and policy metadata, not raw local credentials.
- Adapter maps Den `tool_request` to MCP tool calls and returns results using the same tool result endpoint.

Policy:

- Den filters which MCP tools are exposed by name/server/risk class.
- User permission required for unknown/high-risk tools.

Diagnostics:

- MCP server name, tool name, duration, status, result size, stderr summary.
- never log credentials or full payloads by default.

---

## Letta continuation strategy

Direct Letta ACP cannot assume Letta will natively pause for external local tool results. Implement an explicit Den turn loop:

1. User prompt goes to Letta.
2. Den maps assistant/status output as normal.
3. If Letta emits an unsupported tool-call-like native event, Den maps it to an explicit unsupported-tool diagnostic until the tool-call schema is implemented.
4. Once a reliable native Letta tool-call schema is identified, Den converts that schema to `tool_request`.
5. After adapter returns a tool result, Den posts a follow-up message into the same resolved conversation with structured tool-result context.
6. Den continues streaming Letta's next response to the adapter.

Acceptance for the first real tool slice requires demonstrating that Letta can reliably request `fs_read_text_file` in a parseable schema. Letta/OpenAI-compatible tool names must match `^[a-zA-Z0-9_-]+$`; Den maps this sanitized Letta tool name to the ACP client method `fs/read_text_file`. If Letta cannot produce a stable tool-call stream event through the conversation API, build a small Den-side tool intent parser as a temporary controlled bridge only for explicit tool intents, and mark it experimental.

---

## Implementation phases

### Phase 0: diagnostics hardening (already in progress)

- Den stream summaries for native Letta event counts and unmapped samples.
- Adapter Den-stream summaries and unknown event samples.
- Empty-turn errors on both Den and adapter.
- Transport v3 names: `assistant_text_delta`, `status_text`, `turn_complete`.

Acceptance:

- No silent no-response turns.
- Unknown Den/Letta event shapes are visible in logs and user-facing errors.

### Phase 1: adapter dispatcher and capability normalization

Tasks:

- Ensure the adapter has exactly one stdin reader task.
- Route JSON-RPC responses by id through an internal waiter registry.
- Store normalized `initialize.clientCapabilities`.
- Add an adapter `runtime_id` generated at startup.
- Include `adapter_runtime_id`, version, client capabilities, and transport version in Den prompt context.
- Add JSON-RPC request helper with timeout/cancel handling and structured logs.

Acceptance:

- Chat/session flows still work.
- Adapter can safely issue `fs/read_text_file` while still receiving interleaved client messages.
- Logs show request ids, response ids, durations, and timeout/cancel outcomes.

### Phase 2: Den tool policy and tool-turn state

Tasks:

- Add Den in-memory active turn registry keyed by `(user_id, bear_id, acp_session_id, turn_id)`.
- Store only live turn metadata; do not create durable pending-call rows.
- Add `POST /tool-results/{tool_call_id}` endpoint.
- Validate token has `acp:chat` for prompt and `acp:tools` for tool results.
- Add tool descriptor/policy builder from client capabilities.
- Add Den audit logs for tool request and result.

Acceptance:

- Den can emit a synthetic `tool_request` in a test and accept an adapter result.
- Late/duplicate/stale tool results return diagnostic `reason` values.

### Phase 3: read file vertical slice

Tasks:

- Define `fs_read_text_file` descriptor and schema, mapped by the adapter to ACP client method `fs/read_text_file`.
- Adapter maps `tool_request` to:
  - `session/update` `tool_call` started;
  - optional `session/request_permission` if requested;
  - ACP `fs/read_text_file` request;
  - `session/update` completed/failed;
  - Den tool result POST.
- Den injects successful tool result back into Letta as structured context and continues the turn.
- Add size limits and truncation.

Acceptance:

- User asks to read a file.
- Client shows a normal read tool call.
- File contents are returned to Letta.
- Final assistant response references file content.
- Failures are visible as permission/path/timeout/unsupported errors.

### Phase 4: directory listing and search

Tasks:

- Add `fs.list_directory` and `fs.search_files` descriptors.
- Implement adapter-local directory/search where ACP client lacks direct methods, behind Den policy.
- Add result truncation and response summaries.

Acceptance:

- Bear can inspect workspace shape and locate files without shell.
- Search/list diagnostics identify limits/truncation.

### Phase 5: edit/write tools

Tasks:

- Add write/edit descriptor.
- Require permission via ACP `session/request_permission`.
- Add optimistic concurrency via expected hash where possible.
- Add sensitive path denylist.
- Add audit event for every write attempt and result.

Acceptance:

- Bear can write or edit a workspace file after permission.
- Den/adapter logs include permission and write diagnostics without content leakage.

### Phase 6: terminal tools

Tasks:

- Implement terminal create/output/wait/kill/release orchestration.
- Add strict timeout/output limits.
- Require permission for each command.
- Stream terminal progress as `status_text` and ACP tool updates.

Acceptance:

- Bear can run bounded commands with permission.
- Timeout/kill/release are reliable and diagnosed.

### Phase 7: MCP relay

Tasks:

- Accept non-empty ACP `mcpServers` after capability negotiation.
- Adapter owns local MCP lifecycle.
- Adapter reports normalized MCP tool descriptors to Den.
- Den filters tools by policy.
- Adapter invokes MCP tools and posts results via the same tool-result endpoint.

Acceptance:

- MCP tools are visible only after policy filtering.
- Tool calls/results follow the same diagnostics/error model.

---

## Logging and metrics checklist

### Adapter logs

- startup: version, git sha, runtime id, client name.
- initialize: capability summary.
- prompt start/end: session id, turn id, Den request id if known.
- Den stream summary: frames, event types, unknown samples.
- tool request: tool name, call id, permission policy, args summary.
- client method call: method, JSON-RPC id, timeout, duration.
- tool result delivery: HTTP status, Den reason, duration.

### Den logs

- prompt routing: session selection, upstream target, transport version.
- native Letta stream summary.
- tool request emitted: policy decision, call id, args summary.
- tool result received: status, size, duration, reason.
- continuation posted to Letta: conversation id, turn id.
- empty/unsupported turn errors.

### Metrics

Add later once names stabilize:

- `den_acp_prompts_total{outcome}`
- `den_acp_stream_unmapped_events_total{message_type,type}`
- `den_acp_tool_requests_total{tool,status}`
- `den_acp_tool_duration_seconds{tool,status}`
- `adapter_acp_client_method_duration_seconds{method,status}`
- `adapter_acp_unknown_den_events_total{type}`

---

## Security and privacy rules

- Do not log raw bearer tokens.
- Do not log full file contents or command output by default.
- Truncate raw event samples.
- Include hashes/byte counts instead of content where possible.
- Require permission for writes, shell, and high-risk MCP tools.
- Keep local filesystem credentials and MCP secrets in the adapter/client environment only.
- Den should store policy/audit metadata, not durable local workspace secrets.

---

## Architecture hardening plan

The current direct ACP file-tool implementation proves the right native Letta direction, but it is still a vertical slice. Before expanding beyond read-only file access, harden these areas.

### Concern 1: `AcpLettaSseStream` owns too many responsibilities

Current issue:

- one stream state machine reads upstream Letta SSE bytes;
- parses native Letta events;
- maps events to Den ACP domain events;
- persists side effects;
- registers tool turns;
- waits for adapter tool results;
- posts Letta tool returns;
- swaps upstream continuation streams;
- records diagnostics;
- handles empty-turn behavior.

This makes failures hard to isolate and makes future tool classes risky to add.

Improvement plan:

1. Extract `NativeLettaEventMapper` for JSON → `AcpGatewayEvent`.
2. Extract `AcpAdapterEventSerializer` for `AcpGatewayEvent` → Den/adapter SSE JSON.
3. Extract `AcpToolTurnCoordinator` for live tool turn registration, result delivery, timeout, duplicate/stale result handling, and cleanup.
4. Extract `AcpTurnRunner` for the high-level loop: Letta stream → adapter events → tool result → Letta tool return → continuation stream.
5. Keep `AcpLettaSseStream` as a thin `Stream` adapter around `AcpTurnRunner` output.

Acceptance:

- each component has focused unit tests;
- stream code no longer contains tool registry or Letta continuation details;
- adding a second tool does not require editing the raw SSE parser state machine.

### Concern 2: active tool turns use a process-global registry

Current issue:

- the live turn registry is a `OnceLock<Arc<Mutex<HashMap<...>>>>`;
- it is process-local and has no TTL cleanup;
- it is not tied to request/session lifecycle;
- stale entries can remain if a stream disconnects mid-tool;
- tests have to share global state.

Improvement plan:

1. Move active tool turn state into `ApiState`.
2. Add explicit cleanup on:
   - stream end;
   - stream error;
   - session cancel;
   - session close;
   - tool result delivery;
   - timeout.
3. Add result reasons:
   - `delivered`;
   - `turn_missing`;
   - `already_settled`;
   - `stale_turn`;
   - `timed_out`;
   - `cancelled`.
4. Add a bounded recently-settled cache for duplicate/late result diagnostics.
5. Add metrics for live turns, settled turns, timeouts, and stale results.

Acceptance:

- no live tool turn remains after stream close/cancel;
- duplicate and late results are distinguished from unknown results;
- tests can create isolated coordinator instances.

### Concern 3: adapter file reads are adapter-local, not normal ACP client filesystem calls

Current issue:

- `bears/read_text_file` reads the local OS filesystem directly inside the adapter;
- this bypasses editor/client file virtualization, remote workspace mapping, and client permission behavior;
- it is useful for a local vertical slice but not ideal ACP fidelity.

Target design:

```text
Den tool_request -> adapter -> ACP client fs/read_text_file -> adapter -> Den tool result
```

Improvement plan:

1. Implement adapter JSON-RPC request waiters for client responses.
2. For read file, prefer standard ACP method `fs/read_text_file` when `clientCapabilities.fs.readTextFile` is true.
3. Keep adapter-local reads only as an explicit fallback mode for clients/environments that do not expose file methods.
4. Add per-client compatibility tests for Zed/OpenCode method shapes.
5. Make fallback use visible in logs and diagnostics.

Acceptance:

- healthy Zed/OpenCode file reads appear as normal ACP client `fs/read_text_file` calls;
- adapter-local read fallback is opt-in or clearly diagnosed;
- remote/devcontainer workspaces can work through client path mapping.

### Concern 4: tool schemas and statuses are still loosely typed

Current issue:

- HTTP payloads and internal state use flexible JSON in several places;
- tool name, status, args, diagnostics, and result content are not represented by focused Rust types;
- future tools will increase shape drift risk.

Improvement plan:

1. Introduce typed enums/structs:
   - `AcpToolName`;
   - `AcpToolStatus`;
   - `AcpToolRequest`;
   - `AcpToolResult`;
   - `AcpToolDiagnostic`;
   - per-tool args/result structs.
2. Validate at Den boundary before inserting into live turn state.
3. Validate again in adapter before invoking local/client methods.
4. Keep raw JSON only at HTTP/SSE/JSON-RPC edges.

Acceptance:

- malformed tool args produce `invalid_request` with component and call id;
- unsupported tools produce `unsupported` rather than parser fall-through;
- tests cover every status conversion to Letta `success`/`error`.

### Concern 5: Letta stream tool-call parsing is currently defensive and under-tested

Current issue:

- parser accepts several possible field locations (`tool_name`, `name`, `args`, `arguments`, `id`, `tool_call_id`);
- docs indicate tool calls may be nested under `tool_call` or `tool_calls`;
- actual deployed Letta stream shape should be captured and tested.

Improvement plan:

1. Add parser support for documented nested shapes:
   - `tool_call.name`;
   - `tool_call.arguments`;
   - `tool_call.tool_call_id`;
   - first supported entry in `tool_calls[]`.
2. If `arguments` is a JSON string, parse it as JSON object.
3. Add unit fixtures copied from real Letta SSE samples, redacted where needed.
4. Add unknown tool-call-shape diagnostics with safe truncated samples.

Acceptance:

- deployed Letta `tool_call_message` shape is covered by a fixture;
- unknown tool-call shapes produce visible diagnostics;
- no silent empty turn if Letta emits a tool-call-like message Den cannot parse.

### Concern 6: tool waits need explicit timeout and cancellation

Current issue:

- the stream waits on a one-shot channel for the adapter result;
- if adapter/client never returns, the prompt can hang until HTTP/client timeout.

Improvement plan:

1. Add per-tool timeout policy, initially read file ≤ 30 seconds.
2. On timeout, emit an adapter-visible error and send a Letta tool return with `status=error` if safe.
3. Cancel active tool turns on session cancel/close.
4. Adapter should also apply client-method timeouts and return `timeout` to Den.

Acceptance:

- hung adapter/client file reads produce `timeout` diagnostics;
- Den and adapter logs name the timed-out hop;
- session cancel resolves active tool waiters promptly.

### Concern 7: Den policy is not explicit enough yet

Current issue:

- adapter enforces workspace root containment;
- Den currently advertises the tool but does not make a complete policy decision per request;
- future write/shell/browser/MCP tools need Den-side policy and audit before execution.

Improvement plan:

1. Add a Den policy decision object to every `tool_request`:
   - allowed roots;
   - max bytes/lines/results;
   - approval requirement;
   - sensitive path behavior;
   - user role and token-scope basis.
2. Adapter enforces Den policy plus local/client capability checks.
3. Log policy decision metadata without sensitive contents.
4. Require `acp:tools` for tool-enabled prompts/results once token UI is updated.

Acceptance:

- Den logs explain why a tool was allowed/denied;
- adapter rejects requests outside Den-provided policy even if local filesystem permits them;
- write/shell tools cannot be enabled without policy objects.

### Concern 8: token scopes and UI need alignment

Current issue:

- chat works with `acp:chat`;
- tool-result authorization path currently relies on the same ACP token authentication;
- long-term Code tokens should make tool brokerage explicit.

Improvement plan:

1. Update Code-token creation to include `acp:chat` and `acp:tools` for new tool-enabled tokens.
2. For legacy `acp:chat`-only tokens, keep chat working but omit local tool descriptors.
3. Surface Code-token capabilities in token listing UI.
4. Log `acp_tools_scope_missing` when tools are filtered out.

Acceptance:

- users can audit which tokens can broker local tools;
- missing tool scope degrades to chat-only with clear diagnostics;
- tool-result endpoint requires `acp:tools` before write/shell/MCP phases ship.

---

## Follow-up capability families

The phased tool catalog above is enough for a useful first coding-agent runtime, but it is not the complete ACP/product surface. Track these as follow-up capability families once the read/list/search/edit/terminal/MCP spine is stable.

### Rich prompt inputs

Potential capabilities:

- image inputs;
- screenshots supplied by the editor/client;
- audio inputs;
- binary/file attachments;
- selected text and active editor buffer metadata;
- embedded resources referenced by URI.

Design notes:

- Keep Den as the input normalization and policy boundary.
- Do not send large binary payloads directly through unbounded JSON bodies.
- Prefer artifact/object storage for large inputs, with short metadata in the ACP prompt payload.
- Log content type, byte size, hash, and origin; do not log raw binary/text payloads by default.

### Rich outputs and artifacts

Potential capabilities:

- image/artifact cards;
- downloadable generated files;
- structured tables;
- links to Cabinet/Garage artifacts;
- conversation attachments;
- generated reports or patches as first-class resources.

Design notes:

- Separate transient ACP display updates from durable artifacts.
- Use Garage/Cabinet artifact plans for durable storage.
- Adapter should render normal ACP updates; Den should return artifact metadata and URLs only after authz checks.

### Structured edits, diffs, and review flows

Potential capabilities:

- multi-file patch application;
- unified diff preview;
- editor-native diff review;
- file rename/move/delete;
- conflict-aware patching;
- optimistic concurrency using file hashes;
- formatting after edit;
- undo/revert metadata.

Design notes:

- Prefer reviewable patch operations over blind full-file replacement for non-trivial edits.
- Require explicit permission for writes and destructive operations.
- Log paths, hashes, byte counts, and diff stats; avoid logging complete patches when they may contain secrets.

### Language intelligence / LSP tools

Potential capabilities:

- diagnostics for file/workspace;
- go to definition;
- find references;
- document symbols;
- workspace symbols;
- semantic rename;
- code actions;
- formatting;
- type hover/signature information.

Design notes:

- Treat LSP-derived results as local client capabilities exposed through the adapter.
- Keep language-server process ownership with the editor/client where possible.
- Include language id, path, result counts, and truncation in diagnostics.

### Git and source-control tools

Potential capabilities:

- git status;
- diff;
- branch info;
- log/blame;
- stage/unstage;
- commit;
- revert;
- PR/patch generation.

Design notes:

- Prefer explicit structured source-control tools over raw shell commands for common operations.
- Require permission for commit, push, branch mutation, destructive revert, and remote operations.
- Log command-equivalent summary, repo root, changed file counts, and exit/status metadata.

### Browser control, web preview, and screenshotting

Potential capabilities:

- open URL or local preview;
- capture screenshot of page or element;
- inspect DOM/accessibility tree;
- read console logs;
- inspect network requests;
- interact with page elements;
- emulate viewport/device/color scheme;
- capture performance traces;
- control local dev-server preview sessions.

Design notes:

- Treat browser control as a high-risk local capability, not as generic shell execution.
- Require explicit permission before navigating to arbitrary URLs or interacting with authenticated pages.
- Screenshots may contain secrets; log only dimensions, URL origin, hash, and byte size by default.
- Prefer client/editor-provided browser APIs where available; otherwise adapter-owned browser automation may be a separate optional runtime with clear lifecycle and sandboxing.
- For local dev-server inspection, tie browser access to workspace roots and approved localhost ports.
- Add redaction/truncation controls for console/network logs before sending them to Letta.

### Long-running/background tasks

Potential capabilities:

- persistent terminal sessions;
- background build/test/watch tasks;
- reconnect to running task after prompt continuation;
- task cancellation and cleanup;
- progress streaming over long durations.

Design notes:

- Keep initial terminal tools bounded and foreground-only.
- Persistent tasks need explicit lifecycle ownership, cancellation, timeout, output retention, and resume semantics.

### Multi-root, remote, and path-mapping policy

Potential capabilities/policies:

- multiple workspace roots with different trust levels;
- symlink traversal rules;
- case-insensitive path handling;
- WSL/container path mapping;
- remote SSH/devcontainer workspaces;
- editor URI to local path resolution.

Design notes:

- Writes and terminal commands must resolve through the same canonical path policy.
- Diagnostics should report both requested and normalized paths when safe.

### Permission memory and trust policy

Potential capabilities:

- remember approval for one tool call;
- remember for session;
- remember for workspace;
- allowlist command/path patterns;
- denylist sensitive paths/actions;
- revoke remembered approvals.

Design notes:

- Default to no remembered permission for writes/shell/browser/MCP.
- Store durable trust decisions in Den only if they are user-visible and revocable.
- Keep client-local permission behavior compatible with ACP client expectations.

### Tool discovery and operator UX

Potential capabilities:

- show available client tools for the active session;
- explain why a tool is disabled;
- show client capability matrix;
- show Den policy filtering decisions;
- expose last tool use, failures, and audit summaries.

Design notes:

- Add Den operator/debug pages only after backend logs and transport diagnostics are stable.
- Avoid leaking local paths or sensitive tool names to users who lack access.

### Cross-client compatibility matrix

Maintain a tested matrix for Zed, OpenCode, and future ACP clients:

| Capability | Zed | OpenCode | Notes |
| --- | --- | --- | --- |
| `fs/read_text_file` | TBD | TBD | method shape and limits |
| `fs/write_text_file` | TBD | TBD | permission behavior |
| terminal | TBD | TBD | lifecycle support |
| MCP servers | TBD | TBD | stdio/http/sse support |
| images/screenshots | TBD | TBD | input/output content model |
| browser preview | TBD | TBD | client-provided vs adapter-owned |
| LSP diagnostics/symbols | TBD | TBD | if exposed through ACP/client extension |

### Protocol conformance and simulation tests

Add a simulator suite that can exercise:

- capability negotiation;
- JSON-RPC interleaving;
- permission approval/denial;
- cancel during tool wait;
- malformed client responses;
- duplicate responses;
- timeout;
- stream reconnect/load;
- browser screenshot success/failure;
- terminal cancellation;
- MCP tool errors.

These tests should run without a real editor and should produce concise failure diagnostics.

---

## Open questions

1. Which native Letta conversation stream event represents a tool call in the deployed Letta version?
2. Can Letta conversation API reliably continue a turn after an externally supplied local tool result, or do we need a Den-side explicit tool-intent bridge for the first slice?
3. Which ACP filesystem methods are supported by each target client (`Zed`, `OpenCode`) today?
4. Should Den UI-created Code tokens always include `acp:tools`, or should existing token flows expose a safer chat-only option?
5. What default max file bytes, search results, terminal output bytes, and timeouts should ship first?

---

## Definition of done for “ACP tools complete enough”

- Read/list/search/edit/shell tools are implemented with permission and policy controls.
- Non-empty MCP server config is supported or rejected with actionable per-server diagnostics.
- Every tool request/result is correlated across adapter and Den logs.
- Empty turns and unknown event shapes produce visible errors, not silent `end_turn`.
- Integration tests cover success, unsupported capability, permission denial, timeout, stale result, duplicate result, oversized result, and cancellation.
- User-facing behavior in Zed/OpenCode looks like ordinary ACP tool use.
