# ACP Boring Waiters — Architecture Decision Record

## Status: Superseded for ACP direct mode

Superseded by the direct Den ⇄ adapter local tool runtime documented in [`../../planning/ACP_DIRECT_LOCAL_TOOL_RUNTIME_PLAN.md`](../../planning/ACP_DIRECT_LOCAL_TOOL_RUNTIME_PLAN.md). This ADR remains historical context for the older Codepool/Letta Code relay design and should not guide new `pair` role ACP implementation.

## Date: 2026-05-03

---

## Context

BEARS supports Agent Client Protocol (ACP) local editor tools through this runtime path:

1. ACP adapter talks to Den's ACP gateway.
2. Den routes prompts to Codepool over `bear_channel`.
3. Codepool runs Letta Code and registers ACP client tools as external Letta tools.
4. When Letta Code invokes an ACP client tool, Codepool emits `client_tool_request` on the active SSE stream.
5. Den maps that event to the adapter-facing ACP stream.
6. The adapter invokes the editor/client and POSTs the tool result back through Den.
7. Den forwards the result to Codepool.
8. Codepool resolves the original external-tool waiter.

This bridges request/response semantics over SSE. The hard part is waiter ownership and lifecycle: request emission, result correlation, cancellation, timeouts, stream closure, retries, and warm-session reuse.

Earlier designs persisted ACP client tool calls in Den's database. That made Den a second pending-call registry in addition to Codepool's in-memory waiter registry. The extra registry improved audit/debug potential, but it also made the relay path harder to reason about:

- Codepool owned the actual blocked Letta external-tool execution.
- Den owned a separate persisted pending-call row.
- A result could be accepted by Den but fail to resolve Codepool.
- A result could race ahead of Codepool waiter registration.
- Session/request context could diverge between warm Codepool sessions and current SSE responses.

For ACP tool reliability, we prefer a boring, single-owner model over a more durable but split-brain model.

---

## Decision

BEARS will use **Codepool-owned, process-local ACP client-tool waiters**.

Den will not persist ACP client tool pending calls and will not validate tool results against a Den-side pending-call table. Den's ACP tool-result endpoint is a stateless authenticated proxy:

- authenticate the ACP token and required scope;
- resolve the ACP session binding to the Codepool session id;
- validate the result status shape;
- forward the result to Codepool;
- return Codepool's delivery diagnostics (`delivered`, `reason`, `runtime_id`) to the adapter.

Codepool is the sole owner of live ACP client-tool waiter state. A waiter is keyed by:

- Codepool channel/session id;
- ACP prompt/request id;
- tool call id.

Expected tool outcomes resolve through the waiter as payloads:

- `ok`;
- `error`;
- `cancelled`;
- `timeout`.

Promise rejection is reserved for unexpected infrastructure/programmer failures such as duplicate waiter registration or failure to emit the request.

### Waiter lifecycle rules

Codepool waiter handling must remain deliberately simple:

1. Allocate/receive the tool call id.
2. Register the waiter before emitting `client_tool_request`.
3. Emit the request over the active SSE response.
4. If emission fails, explicitly remove/reject the waiter.
5. Resolve the waiter exactly once on result, timeout, or cancellation.
6. Remove the waiter immediately after settlement.
7. Log lifecycle events with session id, request id, call id, tool name, status/reason, and Codepool runtime id where available.

### Request context

ACP tool closures must use immutable per-request context where possible. A session-scoped mutable context map keyed only by session id is not an acceptable long-term source of request id or response writer state because warm sessions can outlive an individual HTTP/SSE response.

When warm Letta Code sessions reuse external tool closures, Codepool must ensure those closures cannot write to stale/ended SSE responses. If dynamic rebinding is not supported by the SDK, Codepool should fail fast with clear diagnostics or reopen ACP-tool-enabled sessions rather than writing to an ended response.

### Den database policy

The `acp_client_tool_calls` table is obsolete. Existing installations drop it through a new migration. Historical migration files that created the table remain unchanged to preserve SQLx migration checksums.

Den may continue to persist ACP session bindings because those are routing/lifecycle records, not live tool waiter records.

---

## Consequences

### Positive

- There is only one authoritative live waiter registry.
- Fast tool results cannot race ahead of waiter creation when Codepool follows register-before-emit.
- Den's ACP result endpoint is simpler and has fewer state divergence modes.
- Late or duplicate results report Codepool's actual delivery state.
- The mental model is easier to debug: if a waiter is pending or missing, inspect Codepool.

### Negative / trade-offs

- ACP client tool waiters are not durable across Codepool restarts.
- Den can no longer independently audit every pending/completed local tool call through a database row.
- A Codepool restart or warm-session mismatch can produce `no_waiter` for otherwise valid adapter results.
- Some future observability must be implemented in Codepool metrics/logs rather than Den SQL queries.

These trade-offs are intentional. ACP local editor tools are interactive, short-lived continuations of an active prompt stream. If Codepool dies or loses the waiter, the correct behavior is for the prompt/tool attempt to fail clearly and be retried by the user/model, not for Den to preserve a pending call that Codepool can no longer resolve.

---

## Follow-up work

Future improvements should preserve the single-owner model:

1. Add bulk waiter cleanup by request/session so stream closure and session cancellation resolve pending waiters promptly.
2. Add a short-lived recently-settled waiter cache to distinguish `no_waiter`, late results, duplicate results, and recently timed-out calls.
3. Add ACP client-tool metrics for requests, wait durations, timeouts, cancellations, no-waiter results, and pending waiter count.
4. Strengthen Codepool-side result validation for optional `tool_name`, `conversation_id`, status, and result size.
5. Add per-tool timeout policy rather than one global timeout for all ACP client tools.
6. Define adapter retry behavior based on Codepool/Den delivery reasons.
7. Investigate protocol-compatible client-visible cancellation/expiry events.

See `docs/planning/ACP_TOOL_RELIABILITY_PLAN.md` for the prioritized implementation plan.
