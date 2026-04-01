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

- `GET /` — marketing home when logged out; logged-in users see `dashboard_empty.html` (or redirect to email verify if needed)

## Admin (`src/web/admin/mod.rs`)

- `GET /admin/` — admin menu
- `GET|POST /admin/users/*` — user management
- `GET|POST /admin/bear-templates/*` — bear agent templates (create / list)
- `GET|POST /admin/bears/*` — bear registry (new bear from template)
- `GET|POST /admin/membership/*` — list and grant `user_bear` membership
- `GET|POST /admin/api/*` — JSON admin API (templates, bears, membership; operator session cookie)
- `GET|POST /admin/oauth_clients/*` — OAuth client CRUD, PKCE test
- `GET|POST /admin/oauth_tokens/*` — token admin

All `/admin/*` routes use `permission_required!(…, "admin")`.

## API service (separate router)

The standalone API (`RUN_API=true`) is built in `src/api/service.rs` — see `src/api/` and `src/api/oauth/README.md`, not this file.
