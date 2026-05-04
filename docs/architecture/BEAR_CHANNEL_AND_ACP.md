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
  -> Den resolves `bear_agents(role='pair')` strictly
  -> Den streams prompts to the pair Letta agent through the Letta API
  -> Future slices delegate client tools back through Den / ACP permission flows
```

`bear_channel` is the internal Den -> Codepool channel contract for Letta Code-backed chat/work surfaces such as browser chat and Slack. ACP is the external protocol for coding clients and routes to the API-direct `pair` role; it must not use Codepool `bear_channel`. OpenAI-compatible APIs remain compatibility/browser-facing surfaces, not the canonical Den -> runtime boundary.

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

## ACP session persistence model

ACP sessions are **not** canonical BEARS conversations.

Den may persist ACP session rows, but those rows are only protocol/client bindings that map an ACP client session to BEARS runtime state. They should be treated as lifecycle and routing metadata, not as chat history or the source of truth for conversations.

The canonical conversation history remains the Letta/BEARS conversation identity:

- `default` for the bear's main thread;
- `conv-...` for saved Letta conversations;
- temporary `new-...` ids only until Codepool/Letta resolves them.

An ACP session binding may store:

- ACP `sessionId`;
- BEARS user and bear ids;
- ACP client name, such as `zed`;
- client working directory (`cwd`);
- runtime session/binding id (historically named `codepool_session_id`; migrate to `runtime_session_id`);
- pending/generated conversation id;
- resolved `conv-...` id when available;
- protocol lifecycle timestamps such as closed or archived.

Future development should not build conversation listing, history, memory, search, or archive semantics from ACP session rows directly. Those features should operate on canonical BEARS/Letta conversations and use ACP session bindings only to translate ACP lifecycle events, such as `session/close`, into the appropriate conversation operation.

If this table/model is renamed later, prefer names that emphasize binding rather than conversation ownership, such as `acp_session_bindings` or `client_session_bindings`.

See also: [ACP Session Bindings ADR](adr/acp-session-bindings.md).

## Current implementation status

Implemented:

- Codepool `BearChannelRequest` and `BearChannelEvent` TypeScript contract for Letta Code-backed surfaces.
- Codepool internal route:
  - `POST /internal/bear_channel/sessions/:sessionId/messages`
- Codepool cancellation route:
  - `POST /internal/bear_channel/sessions/:sessionId/cancel` cancels active bear-channel runs where possible.
- Den `CodePoolClient::post_bear_channel_message_streaming` for browser chat and future Slack/talk surfaces.
- Den web chat (`POST /v1/chat/send`) calls `bear_channel` internally while preserving the browser-facing SSE contract and sends the `talk` role id in the trusted payload.
- Den maps `bear_channel` events back to the current Deep Chat / Letta-shaped SSE payloads:
  - `assistant_delta` -> `assistant_message`
  - `reasoning_delta` -> `reasoning_message`
  - `error` -> `error_message`
- Den API ACP gateway routes to `bear_agents(role='pair')` through Letta API direct, not Codepool:
  - `GET /acp/bears/{slug}/sessions`
  - `GET /acp/bears/{slug}/sessions/{session_id}`
  - `POST /acp/bears/{slug}/sessions/{session_id}/prompt`
  - `POST /acp/bears/{slug}/sessions/{session_id}/cancel`
  - `POST /acp/bears/{slug}/sessions/{session_id}/close`
  - `GET /acp/bears/{slug}/conversations`
  - `GET /acp/bears/{slug}/conversations/{conversation_id}/history`
  - `GET /acp/bears/{slug}/auth-check`
- ACP tokens with `acp:chat` scope, per-bear grants, hashing, last-used tracking, and revocation.
- Local `bears-acp-adapter` for ACP JSON-RPC over stdio to Den HTTPS/SSE.
- Adapter `session/list`, `session/resume`, `session/load`, `session/cancel`, and `session/close` support.
- Text-only `session/load` history replay for user/assistant messages.
- Absolute `cwd` validation for ACP sessions.
- Stable keyset pagination for ACP session listing.
- ACP-provided `mcpServers` are rejected until stdio MCP lifecycle support exists.
- Basic ACP chat validated with Zed.

Reserved for later:

- Dedicated non-Letta-Code ACP runtime that can own ACP client tool execution directly.
- Richer ACP history replay if runtime services expose faithful historical tool/status/error/resource events.
- MCP relay.
- Rich web UI rendering of server tool, sub-agent, and memory events.
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
| Cancel | Codepool session cancel endpoint via Den |

The former Letta Code ACP client-tool relay (`capabilities.client_tools`, `client_tool_request`, and tool-result continuation endpoints) has been removed from the active implementation. Future ACP client tools should be owned by a dedicated ACP runtime rather than tunneled through Letta Code external-tool closures.

Den is the ACP gateway, not a blind proxy. It authenticates the ACP client, authorizes bear access, and injects trusted context. The original Letta Code client-tool relay plan is retained as historical planning material only; future ACP tool support should use a dedicated runtime architecture.

## Rollout approach

1. Keep browser APIs stable.
2. Move Den -> Codepool runtime traffic to `bear_channel`.
3. Add ACP gateway in Den after the internal channel is stable.
4. Add richer event rendering after ACP session basics are proven.
5. Build future ACP client tool support in a dedicated non-Letta-Code runtime.
