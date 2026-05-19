# Notes on ACP

These notes capture lessons from implementing and testing BEARS' ACP adapter path. The current `pair` role ACP implementation is direct: local adapter ⇄ Den ⇄ Letta conversation API. Earlier Codepool/Letta Code relay lessons remain useful where noted, but new ACP work should not route pair-role tools through Codepool.

## 1. Keep ACP sessions separate from canonical conversations

ACP `sessionId` values are protocol/client lifecycle identifiers. They are not the canonical conversation model.

Use ACP session records only as bindings between:

- ACP `sessionId`;
- client name, such as `zed`;
- local workspace context, such as `cwd`;
- authenticated BEARS user/bear;
- canonical BEARS/Letta `conv-*` id, when one exists.

For the direct ACP `pair` role, new ACP sessions should create distinct Letta conversations with `POST /v1/conversations/?agent_id=...`, then send prompts to `POST /v1/conversations/{conv_id}/messages` with `streaming=true` and `stream_tokens=false`. Do not route new ACP sessions through agent-default endpoints; those can share conversation state.

Conversation history, archive semantics, search, memory, and title generation should operate on canonical BEARS conversations, not on ACP session rows directly.

A good name for this kind of table is `acp_session_bindings` or `client_session_bindings`, not a name that suggests it owns conversation history.

## 2. Implement session lifecycle early

Zed expects agents to advertise and implement session capabilities if they want thread reload/close behavior to work well.

At minimum, support:

- `session/new`;
- `session/prompt`;
- `session/cancel`;
- `session/close`.

For a production-quality Zed experience, also strongly consider:

- `session/list`;
- `session/load`;
- `session/resume`.

If these are not advertised, Zed may show messages about the agent not supporting loading or resuming sessions. If sessions are not mapped to stable conversation ids, new Zed threads can accidentally share backend conversation state.

## 3. Treat `session/close` and archive/delete carefully

ACP `session/close` means “cancel active work and free resources.” It does not necessarily mean “archive this conversation.”

Mapping `session/close` to BEARS conversation archive can be useful, but it is a product choice and should be based on Zed's actual behavior.

`session/delete` is a better semantic fit for removing a session from a list, but at the time of this work it was still RFD-level rather than clearly stable.

## 4. Use ACP auth error codes precisely

ACP reserves JSON-RPC error code `-32000` for authentication required.

Do **not** use `-32000` for generic prompt failures, upstream errors, malformed tool results, or service problems. Zed may respond by showing its authentication UI.

Recommended pattern:

- Use `-32000` only for genuinely missing/invalid adapter auth.
- Use standard JSON-RPC codes or another `-320xx` value for generic BEARS failures.
- Include structured diagnostic data in `error.data` for request ids, component names, and remediation hints.

## 5. ACP auth methods can improve Zed UX

Zed has an authentication UI for ACP. If an agent returns `-32000` without advertising useful `authMethods`, the UI can be confusing or empty.

An env-var auth method is promising for BEARS Code tokens:

```json
{
  "id": "bears_den_token",
  "name": "BEARS Code Token",
  "type": "env_var",
  "vars": [
    {
      "name": "BEARS_DEN_TOKEN",
      "label": "BEARS Code Token",
      "secret": true
    }
  ],
  "description": "Paste a Den Code token. Code tokens include acp:chat and acp:tools scopes."
}
```

If using this, implement `authenticate` so it re-reads the environment/config and validates the token against Den.

## 6. Make token scopes visible and explicit

For BEARS, a Den UI-created “Code token” should include both:

- `acp:chat`;
- `acp:tools`.

If a token lacks `acp:tools`, do not fail basic chat silently. Instead:

- continue chat when `acp:chat` is present;
- omit local client tools;
- emit an explicit status/diagnostic explaining that local editor tools are unavailable because `acp:tools` is missing.

This avoids the model or user interpreting missing tools as an ACP/Zed capability problem.

## 7. Tool descriptors must be filtered by client capabilities

Only advertise local editor tools when the ACP client says it supports the corresponding client method.

Examples:

- `clientCapabilities.fs.readTextFile` gates `fs/read_text_file`;
- `clientCapabilities.fs.writeTextFile` gates `fs/write_text_file`.

Clients may use slightly different shapes or legacy naming in practice, so normalize common variants defensively, but emit canonical ACP concepts downstream.

## 8. Prefer ACP client tools over backend-local filesystem tools in ACP mode

For a Zed user, “read the workspace file” means read from the editor workspace, including editor-local paths and potentially unsaved state.

If the backend runtime also has local filesystem tools, the model may choose those instead and inspect the server/container filesystem. That is surprising and often wrong.

An ACP mode should disable or strongly deprioritize backend-local filesystem tools and steer the model toward ACP client tools:

- provider-safe Letta tool name: `fs_read_text_file`;
- canonical Den tool identity: `acp.fs.read_text_file`;
- ACP client method: `fs/read_text_file`;
- adapter-local fallback method: `bears/read_text_file`.

This was a recurring source of confusion when the model reported only seeing backend files.

Current read-file flow:

1. Den advertises Letta `client_tools[].name = "fs_read_text_file"`.
2. Letta emits native `approval_request_message` / `tool_call_message` for that tool.
3. Den maps it to private adapter event `tool_request`.
4. Adapter requests ACP permission with `session/request_permission`.
5. Adapter prefers ACP client method `fs/read_text_file` when advertised by the client.
6. Adapter falls back to `bears/read_text_file` only when the client does not advertise `fs.readTextFile`.
7. Adapter posts the result to Den.
8. Den sends a Letta approval/tool return back to the same `conv-*` conversation.

Use `stream_tokens=false` on the conversation-scoped Letta messages endpoint so Letta emits step-level native tool/message events rather than OpenAI token/tool-call deltas where possible.

## 9. Match ACP tool-call update schemas exactly

Zed is strict about ACP schemas. One concrete failure we saw:

- `tool_call.content` was sent as a single object;
- ACP expected a sequence/array of `ToolCallContent`;
- Zed rejected the update with `invalid type: map, expected a sequence`.

Correct shape:

```json
{
  "sessionUpdate": "tool_call",
  "toolCallId": "call_...",
  "title": "Read text file",
  "kind": "read",
  "status": "pending",
  "content": [
    {
      "type": "content",
      "content": {
        "type": "text",
        "text": "Reading README.md"
      }
    }
  ]
}
```

Use schema-driven tests for every ACP notification/request shape.

## 10. Include `sessionId` and absolute paths in file requests

ACP `fs/read_text_file` and `fs/write_text_file` requests require `sessionId` and an absolute path.

For workspace-relative tool arguments, the adapter/server should resolve against `session/new.cwd` before calling the client method.

Read request shape:

```json
{
  "jsonrpc": "2.0",
  "id": "...",
  "method": "fs/read_text_file",
  "params": {
    "sessionId": "acp-...",
    "path": "/workspace/README.md"
  }
}
```

Write request shape:

```json
{
  "jsonrpc": "2.0",
  "id": "...",
  "method": "fs/write_text_file",
  "params": {
    "sessionId": "acp-...",
    "path": "/workspace/CAPYBARA.md",
    "content": "..."
  }
}
```

## 11. Do not echo embedded resources as user-typed text

When a user uses Zed `@file` context, Zed may send the file contents as an embedded ACP resource.

The runtime/model may receive the resource contents, but the visible `user_message_chunk` should only echo text the user typed. Do not synthesize visible user text like:

```text
Referenced resource: file:///workspace/README.md (...)
```

That makes it look as though the user typed content they did not type.

For Letta, do not use message/content `type` as a provenance channel; `type` is the schema discriminator (`message`, `text`, `image`, `tool_return`, etc.). Prefer reference-plus-tool semantics for files: send the user's actual message plus the file id/path as referenced host context, then let file contents enter Letta history as a tool return when the agent calls the file-read tool. Inline small non-fetchable snippets only with explicit `<host_context>` provenance delimiters, and do not use `role: system` for arbitrary file contents.

## 12. Implement a real JSON-RPC dispatcher

An ACP adapter/server should have exactly one stdin reader.

Do not have tool code directly call `stdin.next_line()` while the main request loop also reads stdin. It is fragile under interleaved responses, notifications, and cancellation.

Recommended architecture:

- one stdin reader task;
- pending response map keyed by JSON-RPC id;
- `oneshot` waiters for adapter/server-initiated client requests;
- inbound request/notification queue for client-initiated messages;
- cancellation handling for active sessions/prompts.

This is especially important when tools are invoked during a long-running `session/prompt` stream.

## 13. Flush and observe SSE boundaries

If an implementation bridges streaming systems, be explicit about SSE framing and flushing.

Lessons learned:

- Buffer across byte chunks; do not assume a full SSE event arrives in one chunk.
- Split only on complete SSE delimiters (`\n\n` or `\r\n\r\n`).
- Support multiple `data:` lines per SSE event.
- Log parse failures with short previews.
- Disable proxy buffering where relevant.
- If using Node/Express, set all headers before `flushHeaders()`.
- If using `res.write`, log whether write returned backpressure and when the frame was actually written/drained.

Missing or delayed SSE delivery can look exactly like a tool timeout.

## 14. Register tool waiters before request emission, but start timeout clocks intentionally

A tool result can arrive surprisingly quickly. If the request becomes visible to the client before the waiter exists, a fast result can be delivered back to the server and discarded as `no_waiter`.

Recommended lifecycle:

1. allocate/receive the tool call id;
2. register the waiter before the request is visible outside the process;
3. emit the request over the downstream transport;
4. if emission fails, explicitly remove/reject/cancel the waiter;
5. resolve the waiter exactly once on result, cancellation, timeout, or stream teardown.

The timeout clock deserves separate thought. Ideally, register the waiter early but avoid burning the whole tool timeout while output is buffered behind SSE/proxy backpressure. If that is too complex, prefer correctness first: register-before-emit prevents lost fast results. Then add emission timing/backpressure logs so timeout behavior is diagnosable.

## 15. Make timeouts explain the failed hop

A model-visible timeout should say what timed out.

Bad:

```json
{ "status": "timeout" }
```

Better:

```text
The local editor tool acp_fs_read_text_file timed out before BEARS received a result. This is a tool delivery failure, not evidence that the requested file does not exist.
```

This keeps the model from confidently concluding that a file is missing when the infrastructure path failed.

## 16. Log approval requests with tool names

Letta Code produced many `approval_request_message` events. These were often noisy and sometimes streamed argument fragments. Logging extracted tool names helped distinguish:

- approval for ACP tools;
- approval for backend-local tools;
- unrelated approval/planning events.

For a new non-Letta-Code ACP implementation, still keep this lesson: when a runtime has an approval model, log the tool name and call id at approval boundaries.

## 17. Keep diagnostic metadata structured but non-sensitive

Healthy ACP logs should look like ordinary ACP.

BEARS internals such as Den, Codepool, bearer scopes, and internal request ids should not pollute normal protocol content. But when something fails, structured diagnostic metadata is useful:

- `request_id`;
- `call_id`;
- component name;
- version/git SHA;
- required scope;
- delivery reason;
- runtime id.

Do not include secrets or full file contents in diagnostics.

## 18. Capture version/SHA in adapter output

When debugging Zed, it is easy to run an older adapter binary than expected.

The adapter should report both:

- build-time git SHA;
- local HEAD SHA when running from a git checkout containing the adapter.

This makes stale binary problems obvious in Zed agent logs.

## 19. Prefer direct ACP implementation over multi-hop tool relays

The Den → Codepool → Letta Code → Codepool → Den → adapter → Zed loop was difficult to reason about. Failures could occur at many async boundaries.

A future implementation without Letta Code should strongly consider:

- treating ACP as the primary runtime interface;
- directly managing model tool calls and ACP client tool execution;
- avoiding a second tool-calling runtime that has its own approval model and local filesystem semantics;
- reducing the number of independent stream protocols involved in one prompt turn.

The fewer translation layers between the model's tool call and the ACP client method, the easier it will be to make Zed behavior predictable.

## 20. Keep live waiter ownership boring and single-owner

A waiter system is easiest to reason about when one component owns live waiters.

For BEARS, the most reliable shape is:

- the runtime that is blocked waiting for the tool result owns the waiter;
- other components authenticate, route, validate shape, and forward;
- durable databases may store session bindings and audit records, but should not become a second live pending-call registry unless durable replay is a real requirement.

A split-brain waiter design is tempting because it improves observability, but it introduces hard questions:

- Which registry is authoritative?
- What happens if Den accepts a result that Codepool cannot deliver?
- What happens after a runtime restart?
- What happens if the result arrives before the live waiter exists?

If a tool call is an interactive continuation of an active prompt, process-local waiters are acceptable. On runtime loss, fail clearly and let the user/model retry.

## 21. Avoid request-bound closures in warm sessions

A warm runtime session must not retain closures that capture per-request resources such as:

- an HTTP response object;
- an SSE writer;
- a request id that changes each turn;
- an AbortSignal for one prompt;
- a per-turn client capability set.

This caused ACP tools to try to emit `client_tool_request` on an SSE response that had already ended, producing errors such as `write after end` or `Cannot emit client_tool_request; bear channel SSE response is closed`.

If warm sessions are reused, tool handlers need dynamic rebinding to the current turn's emitter/context. If the SDK/runtime cannot support safe rebinding, reopen ACP-tool-enabled sessions per prompt or keep tool handlers independent of request-bound transport objects.

## 22. Treat stream closure as part of waiter lifecycle

SSE streams are not just output; they are often the only way a tool request reaches the ACP client. If the stream closes, pending tool waiters should not be allowed to linger until timeout.

A robust implementation should explicitly settle waiters when:

- the HTTP request closes;
- the SSE response errors or ends unexpectedly;
- the session is cancelled;
- the runtime tears down a channel/session;
- the prompt turn exits abnormally.

Useful registry helpers:

- `cancelWaitersForRequest(sessionId, requestId, reason)`;
- `cancelWaitersForSession(sessionId, reason)`;
- `listWaitersForRequest(sessionId, requestId)`.

This is better than letting every lifecycle failure look like a generic tool timeout.

## 23. Keep a short-lived settled-waiter memory for diagnostics

After a waiter resolves, late duplicate results become difficult to distinguish from completely unknown results.

A small in-memory TTL cache of recently settled waiter keys can make diagnostics much clearer:

- `delivered`;
- `no_waiter`;
- `already_settled`;
- `expired_recently`.

This does not need to be durable. Even a one-to-five-minute bounded cache can explain whether the adapter retried, the client returned late, or the runtime timed out first.

## 24. Do not let model-visible protocol failures masquerade as file facts

When ACP file tools fail because the transport or waiter system failed, the model may otherwise infer that a file does not exist or that the user did something wrong.

Tool errors should distinguish:

- client/editor returned an actual file-not-found error;
- adapter could not call the ACP client method;
- Den could not forward the result;
- runtime had no waiter;
- SSE stream closed before request emission;
- waiter timed out waiting for a result.

This matters because the model will use the tool result as evidence. Infrastructure failures should be described as infrastructure failures.

## 25. Metrics are not optional for ACP reliability

Logs are useful but insufficient once ACP involves multiple async boundaries.

Expose metrics for:

- tool requests by tool and status;
- waiter duration by tool and final status;
- timeouts;
- cancellations;
- no-waiter/late-result deliveries;
- pending waiter count;
- SSE write/backpressure timing if SSE is in the path.

These metrics make it possible to distinguish model/tool behavior from protocol delivery failures.
