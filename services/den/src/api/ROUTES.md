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

- `POST /acp/bears/{slug}/sessions/{session_id}/prompt` — API-only bearer-token authenticated gateway for ACP adapter clients. Requires `ACP_GATEWAY_ENABLED=true`, `RUN_API=true`, `CODEPOOL_BASE_URL`, and a bearer token with `acp:chat` scope. JSON body: `message`, optional `conversation_id`, optional `client` (`zed`, `opencode`, or adapter default). Den validates the token, membership-checks the bear by slug, injects trusted `bear_channel` context with `channel.family = coding_workspace`, `channel.protocol = agent_client_protocol`, and streams adapter-friendly SSE events (`agent_message_chunk`, `status`, `done`). Client-tool relay is intentionally not part of this first slice.
