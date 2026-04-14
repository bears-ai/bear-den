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

- `GET /` — marketing home when logged out; logged-in users see `dashboard.html` listing bears they may use (links to `/bear/{slug}`), or redirect to email verify if needed

## End-user chat (Phase 1 — same origin as web)

- `GET /app` — Loquix-based chat shell (login required); static HTML + CDN `@loquix/core`, calls `/v1/*` with session cookies (`src/web/loquix.rs`, `src/web/static/loquix_app.html`).
- `GET /bear/{slug}` — Loquix chat for a single bear the user may access (membership-checked; `src/web/templates/bear_chat.html`); same `/v1/chat/send` streaming as `/app`.
- `GET /v1/bears` — JSON list of bears the signed-in user may use (membership-filtered; no Letta ids exposed) (`src/web/v1/mod.rs`).
- `POST /v1/chat/send` — membership check, then proxies Letta `POST /v1/agents/{id}/messages` with `streaming: true`; response is `text/event-stream` for the browser fetch stream.

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
- `GET /admin/lettabot` — LettaBot YAML preview + deploy checklist (operator HTML)
- `GET /admin/lettabot.yaml` — download `lettabot.yaml` (`text/yaml`; membership → agents)
- `GET|POST /admin/api/*` — JSON admin API (bears, membership; operator session cookie)
- `GET|POST /admin/oauth_clients/*` — OAuth client CRUD, PKCE test
- `GET|POST /admin/oauth_tokens/*` — token admin

All `/admin/*` routes use `permission_required!(…, "admin")`.

## API service (separate router)

The standalone API (`RUN_API=true`) is built in `src/api/service.rs` — see `src/api/` and `src/api/oauth/README.md`, not this file.
