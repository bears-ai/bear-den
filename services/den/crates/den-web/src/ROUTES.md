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

## Shared management (`src/management_hub.rs`)

- `GET /connections` — verified-user Den-wide connection destination: repository credential/GitHub App **status** for repositories this actor manages (site admins: all), plus only their Bear editor-access links. Secret values are never rendered. Owner-scoped reusable Git HTTPS/SSH/GitHub App records are now managed here; attaching clears local credential copies. Other provider adapters are not implemented. `POST /connections/create`, `/{id}/revoke`, `/{id}/repositories` and `/repositories/{id}/detach` use typed forms and canonical ownership/manager checks; secrets are never rendered and changes request provider reconciliation.
- `GET /reviews[?bear={slug}]` — verified-user, Bear-admin-only review summaries projected from the canonical Postgres/SQLite memory proposal counts. Only the actor's Bear memberships are considered; inaccessible Bear filters return 404, unavailable counts are not displayed as zero. Decisions remain on their owning memory/hat/Job pages; page-authorized pending Cabinet versions are also listed, with Skills review links. A combined Job-approval inbox is not implemented.

## Onboarding (`src/web/onboarding.rs`)

- `GET|POST /onboarding/first-bear` — first Bear setup for verified users with no memberships; purpose/model first, optional steering/context, then redirects to hat setup. Creation does not execute the optional first-task suggestion.

## Bear management (`src/web/bear/settings.rs`, `src/web/bear/manage.rs`)

Current contract below is working-tree WIP, not latest rebuilt-image evidence; see [the maintained topic](../../../../../docs/topics/bear-memory-hats.md). Every ordinary conversation, Job, and run requires a real hat, even with zero hats. Old settings/rows/records are historical inspection data, not live overrides or automatically imported/promoted hats. Pending IDE sessions create durable `den-conv-*` only after admission. Inference, continuations, results, and direct dispatch recheck live canonical actor/source/hat and exact Work eligibility. Den action/resource grants remain partial for web, supported filesystem reads, and sandbox egress; broader Den effects and other persistent client choices remain next.

Bear management at `/bear/{slug}/…` is membership-gated. Raw inspection (Bear-wide activity, context, reflection evidence, historical settings, advanced diagnostics) requires Bear admin; ordinary members see shared settings and curated memory. Navigation does not confer access.

- `GET /bear/{slug}/overview` — ownership-led, member-safe overview with direct Chat/Memory/reach/Jobs actions, shared hat directory summaries and recent Jobs filtered by Docket's viewer-scoped listing before LIMIT. Admins also see review counts, recent conversations and optional raw memory/recall/activity diagnostics.
- `GET /bear/{slug}/identity` — **Purpose**: identity/description, model and hat-directory summaries. Admins have existing name/prompt/model edit links and escaped steering/context inspection; members do not see authored steering. Configurable stance detail is retired.
- `GET|POST /bear/{slug}/hats`, `GET|POST /bear/{slug}/hats/{hat_id}` — Bear-admin hat list/create and purpose/identity edit with an escaped preview of the hat identity component used in bound turns and, on managed Bears, an admin-only reference panel for effective older chat/pair/Work instructions (including customizations, never copied automatically); Work-enabled hats require explicit audience acknowledgement when edited. `POST /bear/{slug}/hats/{hat_id}/ide-default` selects the one Bear-owned IDE default without changing existing bindings. `POST /hats/{hat_id}/surfaces` replaces its permitted subset of Bear-assigned surfaces; `POST /hats/{hat_id}/work` enables Work for an empty hat with a surface only after an identity-audience acknowledgement and fingerprint recheck (or disables it); `GET|POST /bear/{slug}/hats/{hat_id}/work-review` presents all historical hat memory in 100-record pages and records a Bear-admin review of both the full memory snapshot and hat identity on the last page before enabling a populated hat. Changes to either between pages require restarting the review; incomplete snapshots fail closed. `POST /bear/{slug}/hats/{hat_id}/access` lets Bear admins add/revoke Den-owned `web_fetch` and `web_search` tool grants and exact HTTPS hosts, with future-Job audience confirmation on additions; creating a hat revokes historical active Bear-wide web approvals, which the resources page no longer offers once hats exist; the hat detail page lists current grants and explains that native Pair fetch skips ACP when the current hat has both grants, Brave search discloses queries through a server-configured key, and only newly provisioned eligible Job sandboxes receive intersected host ceilings. `POST /hats/{hat_id}/auto-curate` controls automatic sharing of Curate-rewritten conversation notes with every authorized wearer, including eligible Job runs, with explicit admin acknowledgement of the provider and audience. `POST /hats/{hat_id}/conversations` creates and binds a new admin-owned chat before its first turn. `GET|POST /bear/{slug}/hats/{hat_id}/review` is an admin-only, explicit source-local → hat-memory review: a canonical source, edited safe content, rationale, and a Work-audience acknowledgement (when enabled) are required; the source remains private. `GET|POST /bear/{slug}/hats/{hat_id}/core-review` reviews a current hat entry into newly authored Bear-wide core text, with an all-member/future-Work acknowledgement and optimistic core-head recheck; it cannot publish raw notes or another hat's entries. `GET|POST /bear/{slug}/hats/{hat_id}/legacy-review` gives Bear admins a paginated inventory of unattributed profile-local notes and permits explicitly reauthored hat knowledge (including from non-UUID imported IDs) after unknown-owner/member and conditional Work acknowledgements; it does not assign the old record an owner.
- `GET|POST /bear/{slug}/skills` — instruction-only catalog/attached procedures and human draft creation; members see attached approved versions plus attached disabled status; disabled content stays excluded from model requests. Current permitted uses are visible and prefilled; drafts/unattached private procedures remain owner/admin-scoped. `POST /skills/{id}` takes a typed operation (`approve`, `attach`, `detach`, `disable`), exact hash/applicability and explicit publication/Work acknowledgements. Approved bytes are immutable; edits mean new versions. Effective use is compiled into subsequent bound Chat/Editor/Work prompts.
- `GET /bear/{slug}/tools` — static tool catalog by channel, connected editor, eligible Job run, curation, and observation context; actual tool grants depend on the run, approvals and connected client. Forwarded MCP tools are session-local, not Bear-wide catalog rows
- `GET /bear/{slug}/connections` — Bear context linking to the shared `/connections` destination and reach view, with admin-only editor-token management links; reusable repository account catalogs are available; other adapters remain unsupported
- `GET /bear/{slug}/resources` — **What it can use**: assigned repositories, shared Cabinet and tool/hat reach links plus web-source policy. Raw fetch and editor plan-session details are loaded/rendered only for Bear admins; existing POST policy actions are unchanged.
- `GET /bear/{slug}/activity` and `/conversations` — Bear-admin conversation/processing inspection; `GET /bear/{slug}/conversations/{conversation_id}` — Bear-admin raw transcript/compaction/checkpoint detail; `POST /bear/{slug}/conversations/{conversation_id}/hat` binds only an empty inactive conversation once. Legacy compaction events keyed only by external ID are not rendered because they cannot be attributed to a Bear.
- `GET /bear/{slug}/people` — membership; bear admins grant/revoke via POST actions
- `GET /bear/{slug}/portability` — **Backup & move**: admin-only export link (`GET /bear/{slug}/export.bear`), review-before-import, exact what-moves/what-stays preview. `POST /bears/import` stages one bounded private bundle and redirects to authenticated `GET /bears/import/{nonce}` without creating a Bear. That page previews identity/prompts/hats/procedures, knowledge audiences and destination-model compatibility—not full SQLite memory content. `POST /bears/import/{nonce}/confirm` requires fresh acknowledgement after review; `POST /bears/import/{nonce}/cancel` retires the staged copy. Nonces are owner/session-bound, expire after 15 minutes, are tamper-checked and atomically consumed once. Startup/periodic cleanup is bounded and respects active guards; processing is semaphore-bounded. Failed creation reports rollback/retained/uncertain state and never removes a surviving Bear's memory. Version 3 includes named model configurations and Bear/hat references alongside all canonical memory scopes (including private notes), hat identity/default, grant intent and reviewed instruction-only procedures. Destination models/effort are validated before Bear creation; configuration IDs and references are reminted/remapped. Import requires audience acknowledgement, remints hat IDs and scopes, keeps Work/sharing/grants off, stages procedures disabled, quarantines entity bindings and records a reconnection receipt. Versions 1 and 2 remain readable. Transcripts, Jobs, Cabinet, memberships and secrets stay behind; there is no curation flush. Bears dashboard also offers verified-user bundle import.
- `GET /bear/{slug}/context` — Bear-admin prompt/context inspection, including bound Bear-wide base/platform modes, historical instruction inspection, standing-note previews, and the latest Bear-wide conversation budget; legacy contracts are not live bound compilation inputs or selected by any production inference.
- `GET /bear/{slug}/reflections` — Bear-admin inspection of Bear-wide reflection events and processing evidence.
- `GET|POST /bear/{slug}/models` — named Bear model configurations and effective default display; POST retains separate budget, loop-control and Bifrost key settings (writes Bear-admin-only). `POST /bear/{slug}/models/configurations`, `/configurations/{id}`, `/configurations/{id}/delete` create/update/delete same-Bear configurations; referenced configurations cannot be deleted. `POST /bear/{slug}/models/default` selects a configuration or explicit deployment inheritance. `POST /bear/{slug}/hats/{hat_id}/model` selects a same-Bear primary override or Bear inheritance. Explicit effort requires known catalog support; revoked selections stay inspectable/repairable. Historical profile model/loop rows are not live overrides. `POST /bear/{slug}/models/provision-bifrost-key` provisions the Bear key, not stances. `GET /bear/{slug}/advanced` is admin diagnostics. Stance detail/configuration/provisioning, admin per-profile model, and profile-registration routes are gone; the Bear-wide `/models` route remains. Initialization never creates/refreshes a profile registry, and named Rust type aliases are removed.
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
- `GET|POST /bear/{slug}/code-token` — Bear-level ACP armature identity token; not a hat or stance grant
- `GET /bear/{slug}/memory/browse/runtime-blocks` — permanent redirect to `/bear/{slug}/advanced` (deprecated)
- `GET /bear/{slug}/memory/browse/proposals/{id}` — permanent redirect to `/bear/{slug}/memory/proposals/{id}`
- `POST /bear/{slug}/delete` — delete bear row (bear admins only)
- `POST /bear/{slug}/members/add`, `POST /bear/{slug}/members/remove` — legacy membership actions

## End-user chat (Phase 1 — same origin as web)

- `GET /bear/{slug}` — Deep Chat view for a single bear the user may access (membership-checked; `src/web/templates/bear_chat.html`, handler in `src/web/bear_chat.rs`). Registered with trailing-slash redirect (`/bear/{slug}/` → `/bear/{slug}`) so links like `/bear/{slug}/?conversation_id=…` from the details UI resolve.
- `GET /v1/bears` — JSON list of bears the signed-in user may use (membership-filtered; includes `is_bear_admin`) (`src/web/v1/mod.rs`).
- `GET /v1/chat/conversations` — query `bear_id` (required). Lists only canonically owned conversations for ordinary members, filtered before LIMIT, with each binding's `hat_id`, an owner-only `own_notes_available` UI hint, and this Bear's selectable hats. Bear admins may inspect all, including NULL-owner legacy rows. Browser `default` resolves to the user's own durable conversation, not another member's history. No synthetic writable unbound default is offered, even with zero hats; historical unbound threads remain owner/admin-readable and read-only. `POST /v1/chat/conversations` takes an exact Bear-owned `hat_id` and creates/binds a new owned conversation before its first turn; it cannot select another Bear's hat.
- `PATCH /v1/chat/conversations/{conversation_id}` — JSON body `bear_id` plus optional `title`, `archived`, or `deleted`; owner/admin authorization required before mutation.
- `GET|PATCH /v1/chat/model` — inspect or change this owned conversation's model pin. GET resolves current pin → hat configuration → Bear configuration → deployment and reports configuration/source/effort. Invalid selections return options plus `error` and a null effective model so the person can repair/clear the pin; authorization failures remain errors. PATCH validates explicit pins against the canonical selectable catalog; `auto` restores inheritance even if the inherited model is unavailable. Automatic cached rows are never pins; a pin replaces configured reasoning with model-default effort.
- `GET /v1/chat/history` — query `bear_id`, optional `conversation_id`, `before`, `limit` (default 50, max 100). Canonical owner/admin authorization and user-visible transcript projection; diagnostic-only/model-only tool records are excluded.
- `GET /v1/chat/notes` — required `bear_id` and `conversation_id`. Only the canonical conversation creator with current Bear membership and a hat-bound conversation can read up to 50 current, access-visible source-local notes. Bear-admin transcript inspection does not grant this private-notes read; no hat/core, other source, or legacy profile records are included. The chat view renders note content as plain text, never HTML.
- `GET /v1/chat/artifacts` — query `bear_id`, optional `conversation_id`. Canonical owner/admin authorization before access-filtered artifact citations; no storage locations, hashes, or provenance.
- `GET /v1/chat/current-task` — query `bear_id` (required), optional `conversation_id`. Resolves the canonical owned, active hat-bound browser source and returns authorized task choices with readable titles/status plus current selection. Pending/unbound or another actor's inspection history does not become execution authority. Chat uses a searchable picker and canonical preview/confirmation, not UUID entry; stale conversation responses cannot mutate the new selection.
- `POST /v1/chat/current-task` — JSON body `bear_id`, `conversation_id`, and `title`. Creates a minimal session-owned Pair task for the server-derived browser session; the browser then requests confirmation before selecting it.
- `POST /v1/chat/current-task/selection-request`, `/select`, `/clear` — JSON body `bear_id`, `conversation_id`, and (for preview/select) `task_id`. Membership-checked browser adapters to the canonical Pair current-task confirmation/select/clear operations; browser session ownership is server-derived from the authenticated user, bear, and conversation.
- `POST /v1/chat/send` — JSON body `bear_id`, `message`, optional `conversation_id`. Live canonical owner/member/source/real-hat admission is required before user-turn persistence or runtime, including `default` and `new-*`, even with zero hats. Admin history inspection does not grant execution as another owner; no legacy unbound materialization fallback exists. Runs the Den-native chat loop through Bifrost. Each request gets a UUID **`X-Request-Id`** on the response (SSE success or JSON error). Failures return **`application/json`** `{ "error": "…", "request_id": "…" }` (not HTML). The browser parses `data:` lines and shows `reasoning_message`, `assistant_message`, and `error_message` payloads in Deep Chat (see `bear_chat.html`).

`/v1/*` uses `login_required!(…)` (same session as the rest of the web app).

## Admin (`src/web/admin/mod.rs`)

- `GET /admin/` — operator home in the shared document/style/theme shell, with a single authorized operator navigation rail; child title/head blocks remain functional.
- `GET|POST /admin/users/*` — user management
- `GET|POST /admin/bears/*` — Bear registry (create with Bear-wide prompt/model fields; initialize memory/runtime plan/managed bound config without stance registry provisioning or an autogenerated hat)
- `GET /admin/bears/{id}` — redirects to member-facing `/bear/{slug}/…` profile/settings pages
- `GET|POST /admin/bears/{id}/edit` — legacy redirects to member-facing edit pages
- `GET|POST /admin/membership/*` — list and grant `user_bear` membership
- `GET|POST /admin/api/*` — JSON admin API (bears, membership; operator session cookie)
- `GET|POST /admin/oauth_clients/*` — OAuth client CRUD, PKCE test
- `GET|POST /admin/models*` — Den model selector catalog CRUD (`model_selection_options`)
- `GET /admin/loop-control/` — transcript-free 30-day aggregate of runtime loop-control decisions, for later production tuning
- `GET /admin/runs/` — recent failed turn runs and arbitrary `run_id` lookup; `GET /admin/runs/{run_id}` — run lifecycle detail with persisted run-scoped BearWire events
- `GET|POST /admin/oauth_tokens/*` — token admin with typed scope/expiry/state projections and contextual form errors. Client/token issuance feedback is expiring, issuing-operator/session-bound and server-side, never in redirect query strings; credential responses use no-store/no-referrer.

### Sandbox images (`src/admin/sandbox_images.rs`)

- `GET /admin/sandbox` — Den-managed image catalog (editable even when the provider is down), engine image store + disk usage, recent operations, pull form, shipped-variant build buttons
- `POST /admin/sandbox/pull` — background registry pull (redirects to the operation page)
- `POST /admin/sandbox/build` — background build of a shipped variant (base/rust/node/godot; needs `SANDBOX_BUILD_CONTEXT_DIR` on the provider)
- `POST /admin/sandbox/images/remove` — synchronous engine-store removal
- `GET /admin/sandbox/operations/{id}` — operation state + log tail (auto-refreshes while running; ops don't survive provider restarts)
- `POST /admin/sandbox/catalog` · `/{id}/update` · `/{id}/delete` · `/{id}/default` — catalog CRUD; each pushes the managed config to the provider best-effort

All `/admin/*` routes use `permission_required!(…, "admin")`.

## Cabinet (`src/cabinet/mod.rs`)

Shared-knowledge wiki over the `den_service::cabinet` facade (hierarchy/policy/review plus artifact attachments and bounded human uploads; bounded attachment inspection/previews; recall remains pending; contract in `docs/architecture/cabinet-contract.md`).

- `GET /cabinet` — list/search items (`q`, `lifecycle=archived`)
- `GET /cabinet/uploads` — latest 64 owner-only historical upload records under current Bear membership; readable source-page links only, retired/retained/recovery states and no-store headers; does not grant retired content access
- `POST /cabinet/uploads/{artifact_ref}/cleanup` — owner-only manual retry after lease/grace expiry, with fresh membership/retention checks; canonical-key DELETE then durable acknowledgement; cannot force cleanup of live/retained records
- `GET /cabinet/new` / `POST /cabinet/new` — create an item (first published revision)
- `GET /cabinet/{cabinet_ref}` — rendered item (`?version=` for an older immutable revision)
- `GET /cabinet/{cabinet_ref}/edit` / `POST /cabinet/{cabinet_ref}/edit` — publish a new revision; a stale base re-renders the form with the conflict and the editor's draft
- `GET /cabinet/{cabinet_ref}/history` — revision list
- `POST /cabinet/{cabinet_ref}/archive` · `/restore` — lifecycle transitions
- `POST /cabinet/{cabinet_ref}/delete` — tombstone the item; **people only** (the facade refuses Bears), revisions retained so existing citations keep resolving
- `POST /cabinet/{cabinet_ref}/sources` — link provenance (kind, locator, role)
- `POST /cabinet/{cabinet_ref}/sources/{source_ref}/remove` — unlink provenance
- `POST /cabinet/{cabinet_ref}/attachments` — link an existing finalized `artifact_ref` and typed role; page write plus independent artifact content access required
- `POST /cabinet/{cabinet_ref}/attachments/upload` — server-rendered multipart upload (`file`, `bear_id`, typed `role`, optional `share_with_bear=true` acknowledgement), one non-empty file up to 16 MiB; configured storage, active-page write and selected Bear membership required. Private by default; internal signed PUT/read-back verification precedes atomic finalization/linking with fresh page/membership checks. Failed transfers attempt safe pending-row/blob cleanup; no signed URL or storage key reaches the browser.
- `POST /cabinet/{cabinet_ref}/attachments/{attachment_ref}/remove` — detach the exact page/link; page write required
- `GET /cabinet/{cabinet_ref}/attachments/{attachment_ref}` — content-first human inspection after page/link/artifact authorization; selected metadata behind Details, escaped text/pretty JSON with a 256 KiB UTF-8-safe visible cap, raster/PDF embeds or download/unavailable fallback; no raw provenance/metadata or storage fields
- `GET /cabinet/{cabinet_ref}/attachments/{attachment_ref}/preview` — independently authorized, signature-checked raster/PDF bytes only; full bounded size/hash verification and post-transfer recheck, inline/no-store/nosniff/no-referrer and sandbox CSP; HTML/script/SVG/text are refused as inline documents
- `GET /cabinet/{cabinet_ref}/attachments/{attachment_ref}/content` — permission-rechecking download; JSON or configured Garage content (16 MiB, size/hash checked), no-store/nosniff/no-referrer/attachment headers, no storage URL exposed

All `/cabinet/*` routes use `login_required!(…)`; page/ancestor membership and policy are enforced by the shared facade. `GET /cabinet` without a query lists accessible roots; each page shows accessible children. `GET /cabinet/new?parent={cabinet_ref}` creates a child under current destination authority. `POST /{cabinet_ref}/policy`, `/organize`, `/review` apply named membership/policy, bounded move/order and human pending-version decisions. `GET /cabinet/{cabinet_ref}/move` previews readable current → proposed inherited audience and rejects forbidden/cyclic destinations before confirmation; POST still revalidates the move. Policy administration is separate from review authority; audience changes require acknowledgement. Archive cascades only with subtree authority, restore is one-page, and deletion refuses non-deleted children.

## Docket (`src/work/mod.rs`)

- `GET /bear/{bear_slug}/jobs` — **Jobs** in the shared ownership-led Bear shell, as are Job creation/detail and run detail; existing execution controls and authorization are preserved (auto-refreshes while runs are active)
- `GET /bear/{bear_slug}/jobs/new` — job definition form (goal, eligible hat/repository, supported output policy, branch, tasks); browser repository-change dispatch currently supports per-task publication only. Unsupported historical choices stay visible as unavailable and require explicit replacement; no alternative runtime publication behavior is implied.
- `POST /bear/{bear_slug}/jobs/new` — create the Docket job (tasks assigned to the eligible Job hat; creation does not start execution)
- `POST /bear/{bear_slug}/jobs/new` — the creator selects a Work-enabled hat whose current grants cover the Job's assigned repositories; Docket binds it atomically. No-hat Bears cannot create executable Jobs through a legacy fallback. `GET /bear/{bear_slug}/jobs/{job_id}` — job detail: editable goal/surface/commit policy/branch, task tree with statuses, job dispatch, duplication, run history with publish outcomes. `POST /bear/{bear_slug}/jobs/{job_id}/hat` is Bear-admin-only and binds an eligible draft Job once to a Work-enabled hat covering all its surfaces.
- `POST /bear/{bear_slug}/jobs/{job_id}/edit` — update job-level settings; task-tree editing remains separate/deferred
- `POST /bear/{bear_slug}/jobs/{job_id}/mission` — owner/admin link or clear an accessible Cabinet page using the current annotation `revision`; Docket owns the reference and stale writes fail
- `POST /bear/{bear_slug}/jobs/{job_id}/mission/snapshot` — owner/admin capture the exact published `version` with annotation `revision`; private retained JSON artifact plus Job source evidence in one transaction, no Job-status or Work-audience change
- `GET /bear/{bear_slug}/jobs/{job_id}/evidence/{artifact_ref}/content` — download Job-linked JSON document evidence after Job and artifact authorization; saved copies are listed on Job detail only when readable; no-store/nosniff/attachment headers
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

- `GET /work/surfaces` — **Repositories**: Git resources the user manages (admins: all) + resources available to their Bears; URL and manager/owner boundaries are unchanged
- `GET /work/surfaces/new` / `POST /work/surfaces/new` — create a managed Git surface (creator becomes owner; optional encrypted credential); job-scoped query/form fields can assign the Bear, attach the surface, and return to the originating job
- `GET /work/surfaces/{surface_id}` — manage page (managers/owners/site admins only; deny-as-404): settings, write-only credential, managers, assigned bears, provider readiness, delete
- `POST /work/surfaces/{surface_id}/update` · `/credential` · `/credential/clear` · `/github-app` · `/github-app/clear` · `/managers/grant` · `/managers/revoke` · `/bears/assign` · `/bears/unassign` · `/delete`
- `POST /work/surfaces/{surface_id}/sync` — test and prepare: push managed config, verify credential/upstream/default ref, and clone/fetch the provider's pristine mirror without launching a work run

Mutations push the managed config (surfaces + image catalog) to the sandbox provider best-effort; the dispatch worker reconciles every 5 minutes. New surfaces are prepared immediately and failed preparation remains visible/retryable from the surface page.

All `/work/*` routes use `login_required!(…)`; runs/jobs are scoped to bears the user is a member of, and surface management to the surface's managers (or site admins).

## API service (separate router)

The standalone API (`RUN_API=true`) is built in `src/api/service.rs` — see `src/api/` and `src/api/oauth/README.md`, not this file.
