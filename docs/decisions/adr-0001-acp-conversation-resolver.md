# ACP Conversation Resolver — Architecture Decision Record

## Status: Accepted

## Date: 2026-05-04

---

## Context

BEARS exposes Agent Client Protocol (ACP) through a local `bears-acp-adapter` and Den's HTTPS/SSE ACP gateway. Den authenticates the ACP Code token, authorizes access to a bear, resolves the bear's `pair` role Letta agent, persists ACP session bindings, and streams prompts to Letta.

ACP session setup can happen before Letta has a durable conversation id. The system therefore needs a temporary way to bind an ACP client session to an eventual Letta conversation. Historically this was represented with BEARS-local pending ids such as `new-acp-zed-...`.

Letta conversation identifiers and BEARS-local placeholders are not the same namespace:

| Value shape | Owner | Meaning | May be sent to Letta conversation path? |
| --- | --- | --- | --- |
| `default` | Letta / Den convention | Bear or agent default thread target | Yes |
| `conv-...` | Letta | Durable Letta conversation | Yes |
| `agent-...` | Letta | Agent target used to create or continue an agent-scoped thread | Yes |
| `new-acp-...` / `new-...` | Den | Pending ACP session/thread placeholder before a real `conv-...` is known | No |

A recent ACP failure exposed the boundary issue: Den generated `new-acp-zed-...` and sent it as the Letta `POST /v1/conversations/{conversation_id}/messages` path parameter. Newer Letta validation rejects that path because it only accepts `default`, `conv-*`, or `agent-*` identifiers.

This is a symptom of a broader architectural problem: Den currently carries multiple concepts through a single string field named `conversation_id`.

---

## Decision

Den will introduce an explicit **ACP conversation resolver** layer.

The resolver is responsible for converting client/session input plus persisted ACP session state into a typed routing decision. No ACP handler should directly decide, from ad hoc string checks, which conversation id to persist, which upstream target to send to Letta, whether history is available, or whether close/archive is valid.

The resolver must preserve this boundary:

> BEARS-local pending identifiers are internal Den session-selection values and must never be sent as Letta conversation path identifiers.

### Resolver inputs

The resolver takes:

- authenticated user id;
- bear id and bear slug;
- resolved `pair` role Letta agent id;
- ACP `session_id`;
- optional client-provided `conversation_id`;
- existing ACP session binding, if any;
- ACP client name and trusted client context such as absolute `cwd`.

### Resolver outputs

The resolver returns a structured decision containing at least:

| Field | Meaning |
| --- | --- |
| `session_selection` | The Den-persisted ACP session selection: `default`, `conv-*`, or pending `new-*`. |
| `resolved_conversation_id` | Known durable resolved conversation id, usually `conv-*`, or `default` when deliberately using default semantics. Empty for unresolved pending sessions. |
| `upstream_target` | The Letta conversation path target: `default`, `conv-*`, or `agent-*`. Never `new-*`. |
| `selection_source` | Diagnostic source such as `explicit`, `resolved`, `stored`, or `generated`. |
| `history_target` | Optional target usable for replay/history. Only `default` or `conv-*`; never pending `new-*`. |
| `archive_target` | Optional durable conversation id that close/archive may archive. Only `conv-*`. |
| `requires_belongs_to_bear_check` | Whether Den must verify an explicit `conv-*` belongs to the selected bear/agent before use. |

### Selection semantics

The resolver chooses the session selection in this order:

1. Explicit non-`default` client `conversation_id`, if valid.
2. Existing session `resolved_conversation_id`, if present.
3. Existing session `conversation_id`, if it is usable (`conv-*` or valid pending `new-*`).
4. A newly generated compact pending ACP id.

For compatibility, explicit `conversation_id: "default"` from older adapters is treated as omitted in prompt handling, so existing resolved/stored session context can win.

### Upstream Letta target semantics

The resolver maps Den selections to Letta path targets as follows:

| Session selection | Upstream Letta target |
| --- | --- |
| `conv-*` | same `conv-*` |
| `default` | `default` |
| pending `new-*` | selected pair `agent-*` |

The pair `agent-*` target is used for pending ACP sessions because Letta owns conversation creation and will emit or otherwise expose the resulting durable `conv-*`.

### Resolution semantics

A pending ACP session is not considered resolved merely because Den generated a pending id. It becomes resolved only when Den learns a durable target, currently through a stream event such as `conversation_resolved` with a `conv-*` id.

When Den observes a valid resolved conversation id for an active ACP session, it persists it on the ACP session binding. Subsequent prompts for the same session should route to the resolved `conv-*` rather than the original pending id.

### History and load semantics

History replay is available only when the resolver can produce a `history_target` of `default` or `conv-*`.

Pending `new-*` sessions have no replayable durable conversation history yet. Adapters may still display the live prompt/response stream, but `session/load` and history endpoints must not attempt to fetch Letta history with a pending id.

### Close/archive semantics

Close may mark an ACP session binding closed regardless of whether it is resolved.

Archive is only valid for a durable `conv-*` `archive_target`. Den must not attempt to archive `default`, `agent-*`, or pending `new-*` values as conversations.

### Validation semantics

The resolver validates string shape separately from authorization:

- allowed Den ACP selections: `default`, `conv-*`, compact pending `new-*`;
- allowed Letta upstream targets: `default`, `conv-*`, `agent-*`;
- explicit `conv-*` selections require a belongs-to-bear/agent check before routing;
- pending ids must be compact enough for storage and client round-tripping, but their shape must not be constrained by Letta path validation because they are not Letta ids.

---

## Consequences

### Positive

- Den stops treating all conversation-like strings as interchangeable ids.
- Letta API calls are protected from Den-local placeholder leakage.
- Prompt routing, history replay, close/archive, and diagnostics share one consistent decision model.
- Session bindings remain useful for ACP lifecycle without pretending pending ids are canonical conversations.
- Future Letta API changes are isolated to resolver output mapping rather than scattered string checks.

### Negative / trade-offs

- The resolver adds a small internal abstraction and requires tests for each selection/routing state.
- Existing database column names such as `conversation_id` remain somewhat overloaded until a future schema cleanup.
- The system still depends on a reliable way to learn the durable `conv-*` created from an `agent-*` target.

---

## Implementation notes

The first implementation should be conservative and local to Den's ACP gateway:

1. Add an internal resolver module or type near `api/acp.rs` or `core/acp_sessions.rs`.
2. Replace prompt-path ad hoc selection logic with the resolver.
3. Reuse the resolver for conversation history and close/archive target selection where practical.
4. Keep the existing `acp_sessions` table shape initially: `conversation_id` stores the Den session selection, and `resolved_conversation_id` stores the durable known target.
5. Add focused unit tests for each resolver case:
   - new session, no explicit conversation;
   - legacy explicit `default` with no stored session;
   - legacy explicit `default` with resolved session;
   - explicit `conv-*`;
   - stored pending `new-*`;
   - stored resolved `conv-*`;
   - close/archive unresolved pending;
   - history unresolved pending.
6. Add logging that includes both `session_selection` and `upstream_target` so operators can see boundary decisions without leaking secrets.

A later schema cleanup may rename columns or introduce explicit fields such as `session_selection`, `pending_conversation_key`, and `resolved_conversation_id`. That cleanup is not required for the resolver layer and should be treated as a separate migration decision.

---

## Relationship to other ADRs

This ADR refines [ACP Session Bindings](acp-session-bindings.md). ACP sessions remain protocol bindings, not canonical product conversations. The resolver defines how those bindings select and route to canonical Letta conversations.

This ADR is independent from [ACP Boring Waiters](acp-boring-waiters.md), which governs ACP client-tool waiter ownership. The resolver concerns conversation/session identity and upstream routing, not tool-call lifecycle.
