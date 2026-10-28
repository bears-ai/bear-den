# API service routes

Axum routes for the API server (`RUN_API=true`). The API listener defaults to `API_PORT=3001`; the web listener defaults to `PORT=3000`. Use the deployed API origin/path prefix rather than assuming the web port serves these routes.

## Top-level

- `GET /health` — API liveness.
- `GET /version` — JSON build identity.
- `GET /healthcheck` — legacy liveness alias.
- `GET /health/ready` — database readiness check.
- `GET /api-docs/openapi.json` — OpenAPI document; not a claim that every native first-slice route is described there.

## OAuth

- `GET|POST /oauth/authorize` — browser authorization and authenticated-session consent; native public clients require S256 PKCE.
- `POST /oauth/token` — authorization-code exchange.
- `GET /oauth/userinfo` — bearer-authenticated profile (`profile:read`; email additionally needs `profile:email`).

See the [OAuth module guide](../../den-oauth/src/oauth/README.md) for general background; some older examples describe confidential clients. The native app contract below and [app setup](../../../../../apps/apple/Bears/BUILDING.md#den-and-oauth-setup) describe the current public-client slice.

## v1.0 API

- `GET /v1.0/me` — bearer-token authenticated profile endpoint.
- `GET|POST /v1.0/oauth/*` — token management endpoints.

### Native Apple client first slice

**Source/test checkpoint: 2026-10-28; not a deployment or release claim.** Implemented under `v1/native/`, merged into the `/v1.0` router. These are human OAuth routes, not BearWire/Armature-token routes. Every operation requires an active bearer token and its scope; visibility, membership and execution checks remain in Den.

| Route | Scope | Current contract |
| --- | --- | --- |
| `GET /v1.0/bears` | `data:read` | Current member Bears: `bear_id`, `slug`, `name`, `description`, `is_bear_admin`. |
| `GET /v1.0/chat/conversations?bear_id=<uuid>` | `data:read` | Up to 100 visible conversations plus shared hats. Conversations include external `id`, `title`, optional `hat_id`, `can_send`, `last_message_at`, `session_uri`, `chat_uri`. Not a fully paginated catalog. |
| `POST /v1.0/chat/conversations` | `data:write` | JSON `bear_id`, `hat_id`, `request_id` UUIDs; explicit existing Bear hat and non-nil hat/request required. Owner/Bear/request-scoped idempotency: same request and hat returns the same conversation, changed hat conflicts. Returns `id`, `title`, `hat_id`, `session_uri`, `chat_uri`. |
| `GET /v1.0/chat/history?bear_id=<uuid>&conversation_id=<external-id>` | `data:read` | Canonical user-visible text projection; diagnostic-only rows excluded. Optional positive `before` sequence and `limit` 1–100 (default 50). Returns chronological `messages`, `has_more`, `next_before` (string cursor or null). |
| `GET /v1.0/agent-host` | `data:read` at upgrade; `data:write` for send/cancel | OAuth-authenticated **WebSocket Upgrade**, AHP 1.0 JSON-RPC text subset. Preserve `Authorization` and WebSocket upgrade through the proxy. |

AHP negotiates `1.0.0` against upstream tag `spec/v1.0.0`. Supported: `initialize`, `subscribe`, `unsubscribe`, snapshot-only `reconnect`; root/session/chat snapshots; `dispatchAction` with text `chat/turnStarted` or same-connection `chat/turnCancelled`; assistant parts/deltas/terminal outcomes through `action` notifications. `ahp-session:/<canonical-conversation-uuid>` and `ahp-chat:/<canonical-conversation-uuid>` are derived protocol resources, not new source entities. An action receipt is not acceptance; inspect the origin/client sequence and rejection. Persisted turn IDs cannot be replayed as new sends. Disconnect requests cancellation of that connection's turn; canonical partial-text/terminal finalization can outlive the socket if it exceeds the bounded wait. Reconnect never restarts inference or replays sends. Sequence numbers are not durable replay cursors.

This is **not a complete AHP host**: `createSession`, `createChat`, `listSessions`, `fetchTurns` and general AHP auth methods are unavailable. Use JSON APIs/OAuth instead. No permission/question replies, subagents, refs/resources, attachments, local tools, Wasmer/sandbox or live multi-client fan-out. Scope/owner/source checks distinguish inspection from execution, including read-only admin/archived/Work history. Input text is bounded to 32 KiB and input frames to 64 KiB. Snapshots are bounded to 10,000 interaction rows / 1 MiB transcript text; use paged JSON history for larger sources. No native Docket or Bear-admin mutation API is added by this slice.

Register the native client manually per Den: active, `public=true`, `trusted=false`, exact callback `bears://oauth/callback`, scopes `profile:read data:read data:write`. No client secret or public-client refresh credential; current tokens last about one hour and the app reauthorizes. App sign out is local cancellation/Keychain removal, not server revocation. Browser authorization needs the web login session: configure shared-origin routing or trusted sibling-host cookie sharing/session backing; merely exposing port 3000 is insufficient. See the [active app plan](../../../../../docs/roadmap/MACOS_BEARS_CLIENT_APP_PLAN.md) for rationale, evidence and remaining acceptance.

## BearWire

BearWire is mounted by the binary composition root under `/bearwire`; it is not part of `den-api` itself. Its current Bear-scoped Armature authentication and trusted local-work contract are separate from the native human OAuth channel.
