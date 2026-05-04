# API service routes

Axum routes for the API server (`RUN_API=true`).

## Top-level

- `GET /health` — API liveness.
- `GET /version` — JSON build identity.
- `GET /healthcheck` — legacy liveness alias.
- `GET /health/ready` — database readiness check.
- `GET /api-docs/openapi.json` — OpenAPI document.

## OAuth

- `GET|POST /oauth/*` — OAuth 2.0 authorization server. See [`oauth/README.md`](oauth/README.md).

## v1.0 API

- `GET /v1.0/me` — bearer-token authenticated profile endpoint.
- `GET|POST /v1.0/oauth/*` — token management endpoints.

## ACP gateway

- `GET /acp/bears/{slug}/sessions` — bearer-token authenticated ACP session binding list. Requires `ACP_GATEWAY_ENABLED=true`, `RUN_API=true`, `LETTA_BASE_URL`, and a bearer token with `acp:chat` scope.
- `GET /acp/bears/{slug}/sessions/{session_id}` — bearer-token authenticated ACP session binding detail. Response uses `runtime_session_id` (not historical `codepool_session_id`).
- `POST /acp/bears/{slug}/sessions/{session_id}/prompt` — API-only bearer-token authenticated gateway for ACP adapter clients. Requires `ACP_GATEWAY_ENABLED=true`, `RUN_API=true`, `LETTA_BASE_URL`, a provisioned `bear_agents(role='pair')`, and a bearer token with `acp:chat` scope. JSON body: `message`, optional `conversation_id`, optional `client` (`zed`, `opencode`, or adapter default). Den validates the token, membership-checks the bear by slug, resolves the pair role strictly (no talk/legacy fallback), and streams adapter-friendly SSE events (`agent_message_chunk`, `status`, `done`) from Letta API direct. Client-tool relay is intentionally not part of this first slice.
- `POST /acp/bears/{slug}/sessions/{session_id}/cancel` — marks no active runtime stream as cancelled unless Letta cancellation support is added; returns a diagnostic `cancelled: false` response for API-direct pair sessions.
- `POST /acp/bears/{slug}/sessions/{session_id}/close` — marks the ACP session binding closed and archives the resolved Letta conversation where possible.
- `GET /acp/bears/{slug}/conversations` — lists conversations for the Bear's pair role agent.
- `GET /acp/bears/{slug}/conversations/{conversation_id}/history` — loads conversation history for the Bear's pair role agent.
- `GET /acp/bears/{slug}/auth-check` — validates bearer token and membership for the Bear.
