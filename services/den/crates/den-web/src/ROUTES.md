# Web module routes

Axum routes for the web server (`RUN_WEB=true`). Update this file when you add or remove routes.

See also `src/web/WEB_UI_FIXTURES.md` for the feature-gated browser/UI fixture workflow used for
real-page smoke testing in development.

## Top-level (`src/web/mod.rs`)

- `GET /health` — liveness (BEARS Phase 1 M0 canonical path)
- `GET /version` — JSON build identity (`service`, `version` from Cargo.toml, `built_at_utc`, `git_sha` from `GIT_SHA` Docker build-arg or `unknown`)
- `GET /healthcheck` — liveness (legacy alias)
- `GET /health/ready` — readiness (DB ping)
- `GET /metrics` — Prometheus text exposition (in-memory counters for chat send outcomes; scrape on the internal network; no auth — protect with firewall / reverse proxy as for other metrics endpoints)
- `GET /status` — **BEARS stack** status page: aggregate health probes plus **deployed vs GHCR** when `GITHUB_PACKAGES_TOKEN` + `GHCR_PACKAGES_OWNER` are set
- `GET /status.json` — combined JSON (`health`, `den_version`, optional `ghcr_*`) — **503** if any health check is `fail`
- `GET /design` — CSS fixture page for text, forms, and two-column layout
- `GET /design/chat` — static chat UI fixture for iterating on chat styling
- `GET /manifest.json` — Web App Manifest (`APP_DISPLAY_NAME`, `APP_SLUG`, icons)
- `GET /assets/*` — static assets (memory-serve)
- `GET /*` — fallback 404 (`src/web/public.rs`) for unmatched paths

## Authenticated user (`src/web/user/mod.rs`)

- `GET|POST /settings/*` — profile / email settings (login required)
- `GET|POST /account/*` — registration, account view, password
- `GET /login`, `POST /login/password`, `GET /logout`, `GET /su/{id}` — session (`src/web/user/session.rs`)

## Home (`src/web/home.rs`)

- `GET /` — marketing home when logged out; logged-in verified users with bears see `dashboard.html`; verified users with no bears redirect to `/onboarding/first-bear`; unverified users redirect to email verify

## Onboarding (`src/web/onboarding.rs`)

- `GET|POST /onboarding/first-bear` — first Bear setup flow for verified users with no Bear memberships; creates a stance-aware `context_profile`, provisions native stance bindings, and redirects to chat

## Bear management (`src/web/bear/settings.rs`, `src/web/bear/manage.rs`)

Bear management at `/bear/{slug}/…` is membership-gated. Raw inspection (Bear-wide activity, context, reflection evidence, stance internals, advanced diagnostics) requires Bear admin; ordinary members see shared settings and curated memory. Navigation does not confer access.

- `GET /bear/{slug}/overview` — member-safe overview; Bear admins also see health, recent conversations, raw memory/recall stats, and weekly activity.
- `GET /bear/{slug}/identity` — identity & charter summary; links to edit forms and per-stance models
- `GET|POST /bear/{slug}/hats`, `GET|POST /bear/{slug}/hats/{hat_id}` — Bear-admin hat list/create and purpose/identity edit with an escaped preview of the hat identity component used in bound turns and, on managed Bears, an admin-only reference panel for effective older chat/pair/Work instructions (including customizations, never copied automatically); Work-enabled hats require explicit audience acknowledgement when edited. `POST /bear/{slug}/hats/{hat_id}/ide-default` selects the one Bear-owned IDE default without changing existing bindings. `POST /hats/{hat_id}/surfaces` replaces its permitted subset of Bear-assigned surfaces; `POST /hats/{hat_id}/work` enables Work for an empty hat with a surface only after an identity-audience acknowledgement and fingerprint recheck (or disables it); `GET|POST /bear/{slug}/hats/{hat_id}/work-review` presents historical hat memory in 100-record pages (up to a 500-record snapshot) and records a Bear-admin review of both the full memory snapshot and hat identity on the last page before enabling a populated hat. Changes to either between pages require restarting the review; hats above the bound fail closed. `POST /hats/{hat_id}/conversations` creates and binds a new admin-owned chat before its first turn. `GET|POST /bear/{slug}/hats/{hat_id}/review` is an admin-only, explicit source-local → hat-memory review: a canonical source, edited safe content, rationale, and a Work-audience acknowledgement (when enabled) are required; the source remains private. `GET|POST /bear/{slug}/hats/{hat_id}/core-review` reviews a current hat entry into newly authored Bear-wide core text, with an all-member/future-Work acknowledgement and optimistic core-head recheck; it cannot publish raw notes or another hat's entries. `GET|POST /bear/{slug}/hats/{hat_id}/legacy-review` gives Bear admins a paginated inventory of unattributed profile-local notes and permits explicitly reauthored hat knowledge (including from non-UUID imported IDs) after unknown-owner/member and conditional Work acknowledgements; it does not assign the old record an owner.
- `GET /bear/{slug}/skills` — owned procedures (honest placeholder until Skills land)
- `GET /bear/{slug}/tools` — tool matrix: one row per unique tool with origin (built-in / armature-local; MCP when it lands), stance availability as columns
- `GET /bear/{slug}/connections` — editor (armature) code token; provider connections when they land
- `GET /bear/{slug}/resources` — the web as a resource under policy (sources/approvals/fetches; POST actions as before), internal resources noted
- `GET /bear/{slug}/activity` and `/conversations` — Bear-admin conversation/processing inspection; `GET /bear/{slug}/conversations/{conversation_id}` — Bear-admin raw transcript/compaction/checkpoint detail; `POST /bear/{slug}/conversations/{conversation_id}/hat` binds only an empty inactive conversation once. Legacy compaction events keyed only by external ID are not rendered because they cannot be attributed to a Bear.
- `GET /bear/{slug}/people` — membership; bear admins grant/revoke via POST actions
- `GET /bear/{slug}/portability` — bundle export (`GET /bear/{slug}/export.bear`), import (`POST /bears/import`), what-moves/what-stays
- `GET /bear/{slug}/context` — Bear-admin prompt/context inspection, including compiled stance prompts, standing-note previews, and the latest Bear-wide conversation budget.
- `GET /bear/{slug}/reflections` — Bear-admin inspection of Bear-wide reflection events and processing evidence.
- Internals (kept reachable): `GET /bear/{slug}/stances/{stance}` (Bear-admin stance detail; model POSTs also admin-only), `GET|POST /bear/{slug}/models`, `GET /bear/{slug}/advanced` (Bear-admin diagnostics and provisioning).
- Retired paths redirect: `/access` → `/people`, `/policy` → `/resources`, `/stances` (list) → `/advanced`, `/persona` → `/context`; `/conversations` remains as an alias of the activity stream

## Bear memory & entities (`src/bear/memory.rs`)

- `GET /bear/{slug}/memory` — ordinary members see only shared/hat-curated recent entries and their count; Bear admins see broad inspection stats, review queue, reflection, and derived-recall diagnostics.
- `GET /bear/{slug}/memory/recent` — curated current shared and Bear-owned hat entries for members; all records for Bear admins.
- `GET /bear/{slug}/memory/search?q=&mode=` — members use curated canonical keyword search by default; semantic mode, when Qdrant/embeddings are configured, filters Bear-owned hat/core candidates and reconstructs every current result from authorized SQLite records, never Qdrant text. Unavailable or stale recall falls back to keyword. Bear admins retain broad keyword and configured semantic inspection.
- `GET|POST /bear/{slug}/memory/browse` — curated paths for members; all paths for Bear admins. POST deletes/requests review (Bear admins only).
- `GET /bear/{slug}/memory/records/{memory_id}` — direct ID lookup and history enforce curated eligibility for members, including canonical scope and access-bearing restrictions; Bear admins may inspect all versions, entity links, and recall status.
- `GET /bear/{slug}/memory/proposals/{proposal_id}` and `GET /bear/{slug}/memory/reflection/{run_id}[/evidence]` — raw review/evidence reads for Bear admins only; proposal resolution POST is admin-only.
- `GET /bear/{slug}/entities?type=` and `GET /bear/{slug}/entities/{entity_id}` — Bear-admin inspection only until entity provenance is scoped.
- `POST /bear/{slug}/memory/import-legacy` and `POST /bear/{slug}/memory/review-queue/clear` — Bear-admin import and review actions.

## Member bear management (`src/web/bear_management.rs`)

- `GET|POST /bears/new` — create a bear; creator is granted `user_bear.role = admin`
- `GET /bear/{slug}/details` — permanent redirect to `/bear/{slug}/overview`
- `GET /bear/{slug}/details/{*rest}` — permanent redirects to canonical `/bear/{slug}/…` paths (legacy `roles/` → `stances/`)
- `GET /bear/{slug}/edit` — redirect to `/bear/{slug}/edit/overview`
- `GET|POST /bear/{slug}/edit/overview` — edit slug, name, description; delete bear form
- `GET|POST /bear/{slug}/edit/prompt` — edit system prompt (bear admins)
- `GET|POST /bear/{slug}/edit/configuration` — edit default model only via Bifrost catalog (bear admins)
- `GET|POST /bear/{slug}/code-token` — ACP code token for pair profile
- `GET /bear/{slug}/memory/browse/runtime-blocks` — permanent redirect to `/bear/{slug}/advanced` (deprecated)
- `GET /bear/{slug}/memory/browse/proposals/{id}` — permanent redirect to `/bear/{slug}/memory/proposals/{id}`
- `POST /bear/{slug}/delete` — delete bear row (bear admins only)
- `POST /bear/{slug}/members/add`, `POST /bear/{slug}/members/remove` — legacy membership actions

## End-user chat (Phase 1 — same origin as web)

- `GET /bear/{slug}` — Deep Chat view for a single bear the user may access (membership-checked; `src/web/templates/bear_chat.html`, handler in `src/web/bear_chat.rs`). Registered with trailing-slash redirect (`/bear/{slug}/` → `/bear/{slug}`) so links like `/bear/{slug}/?conversation_id=…` from the details UI resolve.
- `GET /v1/bears` — JSON list of bears the signed-in user may use (membership-filtered; includes `is_bear_admin`) (`src/web/v1/mod.rs`).
- `GET /v1/chat/conversations` — query `bear_id` (required). Lists only canonically owned conversations for ordinary members, filtered before LIMIT, with each binding's `hat_id`, an owner-only `own_notes_available` UI hint, and this Bear's selectable hats. Bear admins may inspect all, including NULL-owner legacy rows. Browser `default` resolves to the user's own durable conversation, not another member's history. The synthetic writable default is listed only for Bears without hats; historical unbound threads remain listed/readable under existing ownership rules but are marked read-only when hats exist. `POST /v1/chat/conversations` takes an exact Bear-owned `hat_id` and creates/binds a new owned conversation before its first turn; it cannot select another Bear's hat.
- `PATCH /v1/chat/conversations/{conversation_id}` — JSON body `bear_id` plus optional `title`, `archived`, or `deleted`; owner/admin authorization required before mutation.
- `GET /v1/chat/history` — query `bear_id`, optional `conversation_id`, `before`, `limit` (default 50, max 100). Canonical owner/admin authorization and user-visible transcript projection; diagnostic-only/model-only tool records are excluded.
- `GET /v1/chat/notes` — required `bear_id` and `conversation_id`. Only the canonical conversation creator with current Bear membership and a hat-bound conversation can read up to 50 current, access-visible source-local notes. Bear-admin transcript inspection does not grant this private-notes read; no hat/core, other source, or legacy profile records are included. The chat view renders note content as plain text, never HTML.
- `GET /v1/chat/artifacts` — query `bear_id`, optional `conversation_id`. Canonical owner/admin authorization before access-filtered artifact citations; no storage locations, hashes, or provenance.
- `GET /v1/chat/current-task` — query `bear_id` (required), optional `conversation_id`. Ensures the authenticated browser’s Pair client session for that conversation and returns session-anchored tasks plus its selected current task.
- `POST /v1/chat/current-task` — JSON body `bear_id`, `conversation_id`, and `title`. Creates a minimal session-owned Pair task for the server-derived browser session; the browser then requests confirmation before selecting it.
- `POST /v1/chat/current-task/selection-request`, `/select`, `/clear` — JSON body `bear_id`, `conversation_id`, and (for preview/select) `task_id`. Membership-checked browser adapters to the canonical Pair current-task confirmation/select/clear operations; browser session ownership is server-derived from the authenticated user, bear, and conversation.
- `POST /v1/chat/send` — JSON body `bear_id`, `message`, optional `conversation_id`. Canonical owner/admin authorization; a configured Bear refuses unbound conversations before persisting a user turn or starting runtime, including `default` and `new-*`. For Bears without hats, a first `new-*` message materializes an owner-scoped durable conversation and announces its ID before runtime/persistence. Runs the Den-native chat loop through Bifrost. Each request gets a UUID **`X-Request-Id`** on the response (SSE success or JSON error). Failures return **`application/json`** `{ "error": "…", "request_id": "…" }` (not HTML). The browser parses `data:` lines and shows `reasoning_message`, `assistant_message`, and `error_message` payloads in Deep Chat (see `bear_chat.html`).

`/v1/*` uses `login_required!(…)` (same session as the rest of the web app).

## Admin (`src/web/admin/mod.rs`)

- `GET /admin/` — admin menu
- `GET|POST /admin/users/*` — user management
- `GET|POST /admin/bears/*` — bear registry (create bear with prompt/model fields and native stance provisioning defaults)
- `GET /admin/bears/{id}` — redirects to member-facing `/bear/{slug}/…` profile/settings pages
- `GET|POST /admin/bears/{id}/edit` — legacy redirects to member-facing edit pages
- `GET|POST /admin/membership/*` — list and grant `user_bear` membership
- `GET|POST /admin/api/*` — JSON admin API (bears, membership; operator session cookie)
- `GET|POST /admin/oauth_clients/*` — OAuth client CRUD, PKCE test
- `GET|POST /admin/models*` — Den model selector catalog CRUD (`model_selection_options`)
- `GET /admin/loop-control/` — transcript-free 30-day aggregate of runtime loop-control decisions, for later production tuning
- `GET /admin/runs/` — recent failed turn runs and arbitrary `run_id` lookup; `GET /admin/runs/{run_id}` — run lifecycle detail with persisted run-scoped BearWire events
- `GET|POST /admin/oauth_tokens/*` — token admin

### Sandbox images (`src/admin/sandbox_images.rs`)

- `GET /admin/sandbox` — Den-managed image catalog (editable even when the provider is down), engine image store + disk usage, recent operations, pull form, shipped-variant build buttons
- `POST /admin/sandbox/pull` — background registry pull (redirects to the operation page)
- `POST /admin/sandbox/build` — background build of a shipped variant (base/rust/node/godot; needs `SANDBOX_BUILD_CONTEXT_DIR` on the provider)
- `POST /admin/sandbox/images/remove` — synchronous engine-store removal
- `GET /admin/sandbox/operations/{id}` — operation state + log tail (auto-refreshes while running; ops don't survive provider restarts)
- `POST /admin/sandbox/catalog` · `/{id}/update` · `/{id}/delete` · `/{id}/default` — catalog CRUD; each pushes the managed config to the provider best-effort

All `/admin/*` routes use `permission_required!(…, "admin")`.

## Cabinet (`src/cabinet/mod.rs`)

Shared-knowledge wiki over the `den_service::cabinet` facade (Cabinet Phase 1; contract in `docs/architecture/cabinet-contract.md`).

- `GET /cabinet` — list/search items (`q`, `lifecycle=archived`)
- `GET /cabinet/new` / `POST /cabinet/new` — create an item (first published revision)
- `GET /cabinet/{cabinet_ref}` — rendered item (`?version=` for an older immutable revision)
- `GET /cabinet/{cabinet_ref}/edit` / `POST /cabinet/{cabinet_ref}/edit` — publish a new revision; a stale base re-renders the form with the conflict and the editor's draft
- `GET /cabinet/{cabinet_ref}/history` — revision list
- `POST /cabinet/{cabinet_ref}/archive` · `/restore` — lifecycle transitions
- `POST /cabinet/{cabinet_ref}/delete` — tombstone the item; **people only** (the facade refuses Bears), revisions retained so existing citations keep resolving
- `POST /cabinet/{cabinet_ref}/sources` — link provenance (kind, locator, role)
- `POST /cabinet/{cabinet_ref}/sources/{source_ref}/remove` — unlink provenance

All `/cabinet/*` routes use `login_required!(…)`; every Den user may read and edit (Phase 1 open-wiki default).

## Docket (`src/work/mod.rs`)

- `GET /bear/{bear_slug}/jobs` — jobs + active/past Docket runs overview (auto-refreshes while runs are active)
- `GET /bear/{bear_slug}/jobs/new` — job creation form (goal, sandbox root, commit policy, work branch, tasks)
- `POST /bear/{bear_slug}/jobs/new` — create the Docket job (tasks assigned to the work stance; created_by_role `ui`)
- `POST /bear/{bear_slug}/jobs/new` — if this Bear has hats, the creator selects a Work-enabled hat whose grant covers the chosen surface; Docket binds it in the same transaction as Job creation and its initial run. Bears with no hats retain the legacy unbound form. `GET /bear/{bear_slug}/jobs/{job_id}` — job detail: editable goal/surface/commit policy/branch, task tree with statuses, job dispatch, duplication, run history with publish outcomes. `POST /bear/{bear_slug}/jobs/{job_id}/hat` is Bear-admin-only and binds an eligible draft Job once to a Work-enabled hat covering all its surfaces.
- `POST /bear/{bear_slug}/jobs/{job_id}/edit` — update job-level settings; task-tree editing remains separate/deferred
- `POST /bear/{bear_slug}/jobs/{job_id}/duplicate` — copy job intent/settings/criteria/task hierarchy into a fresh ready job; run state and publish branch are reset
- `POST /bear/{bear_slug}/jobs/{job_id}/cancel` — cancel the active Bear-owned Docket lifecycle run and release any stale Pair execution claim; this works even if its original Pair session is defunct and is distinct from sandbox work-run cancellation
- `POST /bear/{bear_slug}/jobs/{job_id}/complete` — after all tasks finish, accept remaining criteria as a human decision and close the job/current run
- `POST /bear/{bear_slug}/jobs/{job_id}/extend` — add a fresh work-assigned task with concrete criteria to the current run and return the job to ready
- `POST /bear/{bear_slug}/jobs/{job_id}/tasks/{task_id}/retry` — retry a blocked current-run task after the operator supplies an audit reason
- `GET /bear/{bear_slug}/jobs/runs/{run_id}` — run detail: state, sandbox type/strength, image, work surface, published branch/commit, changed files + diff, headless conversation link, sandbox/armature output, usage, cleanup status
- `POST /bear/{bear_slug}/jobs/{job_id}/dispatch` — explicitly dispatch the job. For a ready/running job, enqueue one background work run for all runnable work-assigned tasks (optional form fields: root, image, git_ref). For a job whose current Docket run was blocked by a terminal work failure, preserve that run and its evidence, create a new current Docket run, carry forward completed work, reset interrupted work to pending, and enqueue a new work run. Automatic dispatch does not retry blocked jobs. If unpublished changes from the failed attempt cannot be recovered, require confirmation before starting clean.
- `POST /bear/{bear_slug}/jobs/runs/{run_id}/cancel` — request cancellation (dispatch worker performs teardown)
- `POST /bear/{bear_slug}/jobs/runs/{run_id}/retry` — retry the work run as a new work attempt within its Docket lifecycle; unlike job-level dispatch after a blocked Docket run, this does not replace `bear_jobs.current_run_id`

### Work surfaces (`src/work/surfaces.rs`)

- `GET /work/surfaces` — surfaces the user manages (admins: all) + surfaces available to their bears
- `GET /work/surfaces/new` / `POST /work/surfaces/new` — create a managed Git surface (creator becomes owner; optional encrypted credential); job-scoped query/form fields can assign the Bear, attach the surface, and return to the originating job
- `GET /work/surfaces/{surface_id}` — manage page (managers/owners/site admins only; deny-as-404): settings, write-only credential, managers, assigned bears, provider readiness, delete
- `POST /work/surfaces/{surface_id}/update` · `/credential` · `/credential/clear` · `/github-app` · `/github-app/clear` · `/managers/grant` · `/managers/revoke` · `/bears/assign` · `/bears/unassign` · `/delete`
- `POST /work/surfaces/{surface_id}/sync` — test and prepare: push managed config, verify credential/upstream/default ref, and clone/fetch the provider's pristine mirror without launching a work run

Mutations push the managed config (surfaces + image catalog) to the sandbox provider best-effort; the dispatch worker reconciles every 5 minutes. New surfaces are prepared immediately and failed preparation remains visible/retryable from the surface page.

All `/work/*` routes use `login_required!(…)`; runs/jobs are scoped to bears the user is a member of, and surface management to the surface's managers (or site admins).

## API service (separate router)

The standalone API (`RUN_API=true`) is built in `src/api/service.rs` — see `src/api/` and `src/api/oauth/README.md`, not this file.
