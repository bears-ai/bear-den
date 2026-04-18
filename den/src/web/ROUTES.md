# Web module routes

Axum routes for the web server (`RUN_WEB=true`). Update this file when you add or remove routes.

## Top-level (`src/web/mod.rs`)

- `GET /health` — liveness (BEARS Phase 1 M0 canonical path)
- `GET /healthcheck` — liveness (legacy alias)
- `GET /health/ready` — readiness (DB ping)
- `GET /manifest.json` — Web App Manifest (`APP_DISPLAY_NAME`, `APP_SLUG`, icons)
- `GET /assets/*` — static assets (memory-serve)
- `GET /*` — fallback 404 (`src/web/public.rs`) for unmatched paths

## Authenticated user (`src/web/user/mod.rs`)

- `GET|POST /settings/*` — profile / email settings (login required)
- `GET|POST /account/*` — registration, account view, password
- `GET /login`, `POST /login/password`, `GET /logout`, `GET /su/{id}` — session (`src/web/user/session.rs`)

## Home (`src/web/home.rs`)

- `GET /` — marketing home when logged out; logged-in users see `dashboard.html` listing bears they may use (links to `/bear/{slug}` and `/bear/{slug}/details`), a link to create bears (`/bears/new`), or redirect to email verify if needed

## Member bear management (`src/web/bear_management.rs`)

- `GET|POST /bears/new` — create a bear; creator is granted `user_bear.role = admin` and Letta is provisioned like operator create (`src/web/bear_create_support.rs` shared form context)
- `GET /bear/{slug}/details` — bear home: boxed overview (name, slug, description), usage (members + active conversations, all **Web** for now), system prompt (Den copy + optional resync), configuration (model, agent type, tools), memory summary; optional query `letta_resync=ok|error` after resync attempts
- `POST /bear/{slug}/details/resync-letta` — push Den registry to Letta (`PATCH` agent + recompile); bear admins only; redirects back to details
- `GET /bear/{slug}/details/edit` — redirect to `/bear/{slug}/details/edit/overview`
- `GET|POST /bear/{slug}/details/edit/overview` — edit slug, name, description; delete bear form (POST still targets `/bear/{slug}/details/delete`)
- `GET|POST /bear/{slug}/details/edit/prompt` — edit system prompt only (bear admins)
- `GET|POST /bear/{slug}/details/edit/configuration` — edit model, Letta agent type, tool ids (bear admins)
- `GET /bear/{slug}/details/access` — manage members (add/remove); bear admins only
- `GET /bear/{slug}/details/conversations` — all conversations including archived (Letta); membership required
- `GET /bear/{slug}/details/memory` — memory block details from Letta
- `POST /bear/{slug}/details/delete` — delete bear row (bear admins only); form field `confirm_slug` must match the slug
- `POST /bear/{slug}/details/members/add` — add or update a user by username (`username`, `role` = `member` or `admin`) — bear admins only
- `POST /bear/{slug}/details/members/remove` — remove membership (`remove_user_id`) — bear admins only; cannot remove the last admin

## End-user chat (Phase 1 — same origin as web)

- `GET /bear/{slug}` — Deep Chat view for a single bear the user may access (membership-checked; `src/web/templates/bear_chat.html`, handler in `src/web/bear_chat.rs`).
- `GET /v1/bears` — JSON list of bears the signed-in user may use (membership-filtered; includes `is_bear_admin`; no Letta ids exposed) (`src/web/v1/mod.rs`).
- `GET /v1/chat/conversations` — query `bear_id` (required). Membership-checked; returns `{ "conversations": [ { "id", "title", "last_message_at" } ] }` for the bear’s Letta agent (`default` = main thread + `conv-…` rows), sorted by most recent activity, excluding conversations that look archived in Letta JSON.
- `GET /v1/chat/history` — query `bear_id` (required), optional `conversation_id` (`default` or `conv-…`; default when omitted), optional `before` (Letta message id cursor), optional `limit` (default 50, max 100). Membership-checked; proxies Letta `GET /v1/conversations/{id}/messages?order=desc` (with `agent_id` when `conversation_id=default`) for Deep Chat `loadHistory`.
- `POST /v1/chat/send` — JSON body `bear_id`, `message`, optional `conversation_id` (`default` or `conv-…`). Membership-checked; proxies Letta `POST /v1/conversations/{id}/messages` with streaming (SSE); the browser parses `data:` lines and shows `reasoning_message` (HTML “Thinking” strip), `assistant_message` text, and `error_message` payloads in Deep Chat (see `bear_chat.html`).

`/v1/*` uses `login_required!(…)` (same session as the rest of the web app).

## Admin (`src/web/admin/mod.rs`)

- `GET /admin/` — admin menu (includes Letta `/v1/health` status when `LETTA_BASE_URL` is set)
- `GET|POST /admin/users/*` — user management
- `GET|POST /admin/bears/*` — bear registry (create bear with prompt/model fields)
- `GET /admin/bears/unlinked-letta-agents` — Letta agents with no Den bear (`letta_agent_id`); link to new-bear-from-agent flow
- `GET /admin/bears/new?from_letta_agent={id}` — new bear form prefilled from Letta `GET /v1/agents/{id}` (hidden `attach_letta_agent_id` skips provisioning)
- `GET /admin/bears/{id}` — read-only bear detail (Den fields, membership count, Letta agent summary when configured)
- `GET|POST /admin/bears/{id}/edit` — edit bear row (slug, prompt, model, tools JSON)
- `POST /admin/bears/{id}/retry-letta` — create Letta agent when `letta_agent_id` is unset (responds with detail HTML including a status line)
- `GET|POST /admin/membership/*` — list and grant `user_bear` membership
- `GET /admin/health/letta` — JSON: Letta reachable + auth (`GET /v1/health` on Letta) (`src/web/admin/ops.rs`)
- `GET /admin/letta-code` — Letta Code harness deploy preview + checklist (operator HTML)
- `GET /admin/letta-code.yaml` — download `letta-code.yaml` (`text/yaml`; membership → agents)
- `GET|POST /admin/api/*` — JSON admin API (bears, membership; operator session cookie)
- `GET|POST /admin/oauth_clients/*` — OAuth client CRUD, PKCE test
- `GET|POST /admin/oauth_tokens/*` — token admin

All `/admin/*` routes use `permission_required!(…, "admin")`.

## API service (separate router)

The standalone API (`RUN_API=true`) is built in `src/api/service.rs` — see `src/api/` and `src/api/oauth/README.md`, not this file.
