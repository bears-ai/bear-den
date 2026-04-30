# `bear_channel` and Agent Client Protocol (ACP)

## Summary

BEARS uses Den as the trusted gateway and Codepool as a private Letta Code runtime/warm-pool manager.

```text
Deep Chat / browser clients
  -> Den web chat API (`/v1/chat/*`)
  -> Den validates auth, membership, and bear policy
  -> Den calls Codepool `bear_channel`
  -> Codepool runs the Letta Code runtime

Zed / OpenCode / coding clients
  -> Agent Client Protocol (ACP) to local `bears-acp-adapter`
  -> Adapter calls Den API ACP gateway over HTTPS/SSE
  -> Den validates auth, membership, and bear policy
  -> Den maps ACP sessions to `bear_channel`
  -> Codepool runs the Letta Code runtime
  -> Future slices delegate client tools back through Den
```

`bear_channel` is the internal Den -> Codepool channel contract. ACP is the planned external protocol for coding clients. OpenAI-compatible APIs remain compatibility/browser-facing surfaces, not the canonical Den -> Codepool runtime boundary.

## Responsibilities

### Den

Den remains in control of:

- user authentication and channel tokens
- user <-> bear membership authorization
- bear slug/id resolution
- trusted runtime context injection
- web session and browser API compatibility
- future ACP gateway authorization
- audit/request metadata

### Codepool

Codepool remains intentionally simple:

- private service, reachable by Den on the stack network
- Letta Code SDK/runtime owner
- warm session pool manager
- `bear_channel` executor
- stream event producer

Codepool does not become the external security authority.

## Current implementation status

Implemented:

- Codepool `BearChannelRequest` and `BearChannelEvent` TypeScript contract.
- Codepool internal route:
  - `POST /internal/bear_channel/sessions/:sessionId/messages`
- Codepool cancellation placeholder:
  - `POST /internal/bear_channel/sessions/:sessionId/cancel` returns `501` until the warm pool supports cancellation.
- Den `CodePoolClient::post_bear_channel_message_streaming`.
- Den web chat (`POST /v1/chat/send`) now calls `bear_channel` internally while preserving the browser-facing SSE contract.
- Den maps `bear_channel` events back to the current Deep Chat / Letta-shaped SSE payloads:
  - `assistant_delta` -> `assistant_message`
  - `reasoning_delta` -> `reasoning_message`
  - `error` -> `error_message`
- Den API ACP gateway route:
  - `POST /acp/bears/{slug}/sessions/{session_id}/prompt`
- ACP tokens with `acp:chat` scope, per-bear grants, hashing, last-used tracking, and revocation.
- Local `bears-acp-adapter` for ACP JSON-RPC over stdio to Den HTTPS/SSE.
- Basic ACP chat validated with Zed.

In progress / next:

- ACP gateway integration tests for auth, membership, validation, request construction, and SSE mapping.

Reserved for later:

- ACP client tool relay implementation. The detailed design is in [`../planning/ACP_CLIENT_TOOL_RELAY_PLAN.md`](../planning/ACP_CLIENT_TOOL_RELAY_PLAN.md).
- Rich web UI rendering of server tool, sub-agent, and memory events.
- Full cancellation.
- Session resume/load/list.
- MCP relay.
- Adapter packaging polish.

## `bear_channel` request shape

The internal request includes trusted context from Den:

```json
{
  "session_id": "den-web:<bear_id>:<conversation_id>",
  "conversation_id": "default",
  "bear": {
    "id": "uuid",
    "slug": "dev",
    "name": "Dev Bear",
    "letta_agent_id": "agent-abc"
  },
  "user": {
    "id": 1,
    "username": "alice",
    "membership_role": "admin"
  },
  "channel": {
    "family": "browser_chat",
    "client": "den_web",
    "protocol": "den_chat"
  },
  "message": {
    "type": "text",
    "content": "Hello"
  },
  "capabilities": {
    "client_tools": [],
    "supports_cancellation": false,
    "supports_rich_events": true
  },
  "runtime_plan": {},
  "request_id": "uuid"
}
```

Clients must not be allowed to spoof this trusted context. Den constructs it after authentication and membership checks.

## `bear_channel` event shape

Initial events:

```json
{"type":"assistant_delta","text":"Hello","id":"..."}
{"type":"reasoning_delta","text":"Thinking","id":"..."}
{"type":"error","message":"Upstream error","detail":"...","request_id":"..."}
{"type":"done","outcome":"ok"}
```

Reserved richer events:

```json
{"type":"server_tool_started","tool":"cabinet_search"}
{"type":"server_tool_finished","tool":"cabinet_search","summary":"3 results"}
{"type":"client_tool_request","call":{"id":"call_1","name":"read_file","arguments":{}}}
{"type":"subagent_started","name":"reflection"}
{"type":"subagent_finished","name":"reflection","summary":"..."}
{"type":"memory_update_recorded","target":"product","summary":"..."}
```

The current Den browser bridge drops reserved richer events to preserve the existing Deep Chat contract.

## ACP mapping spike

Agent Client Protocol (ACP) is the external protocol for Zed/OpenCode-like agent clients. BEARS currently supports the basic-chat slice through a local stdio adapter and Den's HTTPS/SSE ACP gateway.

Expected mapping:

| ACP concept | `bear_channel` concept |
| --- | --- |
| Session start/resume | `session_id`, `conversation_id`, trusted Den context |
| User message | `message: { type: "text", content }` |
| Assistant delta | `assistant_delta` |
| Reasoning/status | `reasoning_delta` or future status events |
| Client capabilities | `capabilities.client_tools` |
| Client tool request | `client_tool_request` |
| Client tool result | adapter -> Den `tool-results/{call_id}` endpoint, then Den -> Codepool internal tool-result continuation endpoint |
| Cancel | planned cancellation endpoint |

Den is the ACP gateway, not a blind proxy. It authenticates the ACP client, authorizes bear access, injects trusted context, and will broker client tool requests/results using the design in [`../planning/ACP_CLIENT_TOOL_RELAY_PLAN.md`](../planning/ACP_CLIENT_TOOL_RELAY_PLAN.md). The first slice intentionally sends `conversation_id = "default"` and ignores client-supplied non-default conversation ids until session load/resume semantics are implemented.

## Rollout approach

1. Keep browser APIs stable.
2. Move Den -> Codepool runtime traffic to `bear_channel`.
3. Add ACP gateway in Den after the internal channel is stable.
4. Add client tool relay and richer event rendering after ACP session basics are proven.
5. Keep legacy Codepool endpoints until `bear_channel` has production soak time.
