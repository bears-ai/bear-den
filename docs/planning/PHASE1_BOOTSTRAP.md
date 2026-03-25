# Phase 1 implementation plan (Den) — Trestle bootstrap

**Trestle** is only a **short-lived bootstrap codename** for the first milestone: bare-bones **Axum + PostgreSQL + self-building Docker**. It is **not** a service directory in this repo and does not persist after you have a working skeleton. The **lasting** binary, crate, and deploy artifact are **Den** (see [PLAN.md](PLAN.md), [DEN_ARCHITECTURE.md](../architecture/DEN_ARCHITECTURE.md)).

Put the Rust project at repo-root **`den/`** with package/binary name **`den`**, Coolify service e.g. **`bears-den`**.

**Phase 1 success** (from PLAN): **operator console** usable for full provisioning; web users chat **Open WebUI → Den → Letta** (and/or Loquix); bear registry + **users↔bears** many-to-many; LettaBot stays **direct to Letta** for chat but Den **owns** bear provisioning and **LettaBot config output**; optional **read-only** LiteLLM observability; **no Cabinet**.

**Delivery priority:** Reach the **first user-testable moment** as early as possible: an **operator provisioning UI** (browser) for **authentication**, **user** lifecycle, **agent/bear** lifecycle (Letta create/sync), **membership**, and **LettaBot** setup (preview/download generated `lettabot.yaml`, copy-paste instructions). End-user **chat** (Open WebUI / Loquix) follows once provisioning flows are usable without `curl`.

---

## 0. Trestle bootstrap (M0 only)

Use whatever **one-off** scaffold you prefer (`cargo new`, an internal template, or a throwaway repo named “trestle”). **Do not** add `services/trestle/` here. When the skeleton has health + config + Dockerfile + migrations wiring, **merge into `den/`** and drop the Trestle name from paths and binary.

**M0 exit:** `den/` exists with Axum `GET /health`, env-based config, tracing, and a multi-stage `Dockerfile`.

---

## 1. Scope

### In scope (Phase 1)

| Area | Deliverable |
|------|-------------|
| Runtime | Axum HTTP server, structured logging, graceful shutdown |
| Data | PostgreSQL schema + migrations; Den is system of record for users, bears, membership |
| **Operator console (priority)** | **Browser UI** served by Den for: operator login; **create/edit users** (and end-user login/register as policy allows); **create/provision bears** (Letta agent create/update); **grant/revoke membership**; **Letta health** indicator; **LettaBot** panel (rendered YAML, download, short deploy checklist). Same actions backed by JSON admin API; **no `curl` required** for happy-path setup. |
| Auth (web-first) | Session cookie after email+password **or** long-lived API token for Open WebUI server-side calls; **operators** use a distinct **admin/operator** session or role (e.g. `users.is_admin`, bootstrap admin email) — **do not** expose `ADMIN_API_KEY` to browser JavaScript |
| Bears | CRUD (admin API + operator UI), `letta_agent_id` linkage, provision via Letta REST API |
| Membership | Many-to-many `user_bear`; enforce on every chat; managed in operator UI |
| Chat | `POST /chat/send` (and/or OpenAI-compatible shim later) → validate → Letta messages API with **SSE streaming** back to client |
| Den web UI: Loquix (defer after console) | Static **Loquix** chat page (`GET /chat` or `/app/chat`); uses **same** chat + discovery endpoints as Open WebUI — see [Loquix](https://github.com/loquix-dev/loquix) |
| Discovery | `GET /agents` or `GET /bears` → bears the current user may use |
| LettaBot | `GET /admin/lettabot.yaml` (operator session **or** server-side key); operator UI shows preview; optional write to volume path on change |
| Policy | RBAC-lite: membership check + optional per-bear `can_use` + basic rate limit |
| LiteLLM | Optional: fetch metrics/admin spend **read-only**; no proxying completions |
| Deploy | **Self-building Docker image** (multi-stage: build Rust in container, runtime image with binary + `ca-certificates`) |

### Explicitly out of scope (Phase 1)

- LettaBot → Den → Letta **for chat traffic**
- Cabinet / Outline
- Slack/WhatsApp **identity mapping** in DB (optional stub only)
- OAuth / SSO (defer unless trivial to add; document extension point)
- High availability, multi-region, zero-downtime migrations (document later)

---

## 2. Repository layout (suggested)

This plan lives in **`docs/planning/PHASE1_BOOTSTRAP.md`**. The Rust tree:

```
den/
├── Cargo.toml
├── Dockerfile
├── .dockerignore
├── README.md               # runbook: env vars, ports, Open WebUI notes
├── migrations/             # SQL (sqlx or refinery)
│   └── 001_initial.sql
├── static/                 # operator console (priority) + later Loquix chat assets
└── src/
    ├── main.rs
    ├── config.rs           # figment/env: DATABASE_URL, LETTA_*, SESSION_*, BIND_ADDR, STATIC_ROOT?
    ├── error.rs            # unified ApiError → HTTP
    ├── state.rs            # AppState: pool, letta client, config
    ├── db/
    ├── auth/
    ├── handlers/           # health, auth, bears, chat, admin, operator_ui (or templates)
    ├── letta/              # reqwest client, SSE forward
    ├── observability/
    └── middleware/
```

**Recommendation:** one binary crate **`den`** under `den/` (no workspace split until needed).

---

## 3. Technology choices

| Concern | Choice | Notes |
|---------|--------|--------|
| Web | **Axum** 0.7+ | Matches PLAN “Den in Rust” |
| Async runtime | **tokio** | Standard |
| DB driver | **sqlx** with `runtime-tokio-rustls`, `postgres`, `migrate` | Compile-time checked queries optional (`offline` mode in CI) |
| HTTP client | **reqwest** | Letta + LiteLLM admin calls |
| SSE / stream | **axum** `body::Body`, `futures::Stream`, or `async-stream` | Proxy Letta stream without buffering full reply |
| Passwords | **argon2** | `password-hash` crate |
| Sessions | **signed cookie** (e.g. `tower-cookies` + **Key** from `SESSION_SECRET`) **or** opaque token in DB | Pick one for v1; document the other as Phase 1.1 |
| Config | **figment** + `serde` | 12-factor env |
| Errors | **`thiserror`** + HTTP mapping | |
| Logging | **tracing** + **tracing-subscriber** | JSON in production |
| IDs | **uuid** v7 or v4 for `user_id`, `bear_id` | Store as UUID in Postgres |

**Alternates:** Diesel instead of sqlx; rate limit via `tower_governor`.

**Operator UI stack:** Prefer **small and shippable**: e.g. **Askama/Tera** HTML + forms or **htmx** against JSON admin routes; or a **Vite** SPA under `static/` if you want richer tables early. Mounted at **`/`** or **`/console`**; keep API JSON separate for Open WebUI integration later.

---

## 4. Database schema (v1)

### Tables

**`users`**

- `id` UUID PK
- `email` TEXT UNIQUE NOT NULL (or `username` if no email)
- `password_hash` TEXT NOT NULL (nullable only if token-only users)
- `is_admin` BOOLEAN NOT NULL DEFAULT false — operator console access (bootstrap first admin via migration or `BOOTSTRAP_ADMIN_EMAIL`)
- `webui_account_id` TEXT NULL UNIQUE — map Open WebUI stable id when available
- `created_at`, `updated_at` TIMESTAMPTZ

**`sessions`** (if using DB-backed opaque tokens)

- `id` UUID PK
- `user_id` UUID FK → users ON DELETE CASCADE
- `expires_at` TIMESTAMPTZ
- `created_at`

**`bears`**

- `id` UUID PK — Den’s **bear_id** (expose as `agent_id` in JSON if you want API parity with PLAN)
- `slug` TEXT UNIQUE — human-stable handle (`household`, `personal-hans`)
- `name`, `description` TEXT
- `letta_agent_id` TEXT NOT NULL — Letta’s agent id string
- `default_model` TEXT NULL — informational; Letta is source of truth for actual model
- `tools_enabled` JSONB NULL — optional mirror for future Cabinet
- `created_at`, `updated_at`

**`user_bear`** (many-to-many)

- `user_id` UUID FK
- `bear_id` UUID FK
- `role` TEXT NULL — e.g. `member`, `owner`
- PK `(user_id, bear_id)`

**`audit_chat`** (optional but useful)

- `id` BIGSERIAL
- `user_id`, `bear_id`, `created_at`, `bytes_out` INT NULL

### Migrations

- Use **sqlx migrate** or **refinery**; run migrations on startup (`migrate!` in dev) **or** separate init container in production (document both).

### Indexes

- `user_bear(bear_id)`, `users(webui_account_id)`, `bears(slug)`

---

## 5. HTTP API (Phase 1)

### Public / user-facing

| Method | Path | Purpose |
|--------|------|---------|
| GET | `/health` | Liveness (no DB) or `/ready` with DB ping |
| GET | `/`, `/console`, `/assets/*` | **Operator console** (priority): static +/or templated pages for provisioning; **Loquix** chat can live under `/app` or `/chat` so it does not block console at `/` |
| POST | `/auth/register` | Optional; may disable in prod and use admin-created users |
| POST | `/auth/login` | Returns session cookie or `{ token }` |
| POST | `/auth/logout` | Invalidate session |
| GET | `/v1/bears` or `/agents` | List bears for **authenticated** user (membership filter) |
| POST | `/v1/chat/send` | Body: `{ message, agent_id?, conversation_id?, stream? }` — **agent_id** = bear id |

**Chat contract** (align with [PLAN.md](PLAN.md) §2.1):

- Resolve `Authorization: Bearer <token>` or session cookie → `user_id`
- Resolve `agent_id` → `bear_id` → `letta_agent_id`; **403** if not member
- Apply rate limit
- `POST` Letta `.../agents/{letta_agent_id}/messages` (exact path per your Letta version — **verify against running image**)
- Stream SSE (or NDJSON) back matching what Open WebUI adapter expects

### Admin / operator API (protect with **operator session** in browser; `ADMIN_API_KEY` for automation only)

| Method | Path | Purpose |
|--------|------|---------|
| POST | `/admin/bears` | Create bear: optionally `provision=true` → create Letta agent then insert row |
| PATCH | `/admin/bears/:id` | Update metadata; optional re-sync Letta |
| POST | `/admin/bears/:id/provision` | Idempotent: create or update Letta agent from Den template |
| POST | `/admin/users` | Create user (operator); optional `set_password` / invite flow |
| PATCH | `/admin/users/:id` | Update user flags (e.g. `is_admin`), reset password if implemented |
| POST | `/admin/users/:id/bears` | Grant membership |
| DELETE | `/admin/users/:id/bears/:bear_id` | Revoke |
| GET | `/admin/lettabot.yaml` | Render yaml from DB + template (`text/yaml`; operator UI embeds or downloads) |
| GET | `/admin/health/letta` | Optional: Letta reachable + auth OK — surface in console |

### Internal / optional

| GET | `/internal/litellm/health` | Proxy read to LiteLLM `/health` or metrics (if enabled) |

**OpenAPI:** Generate with `utoipa` in a follow-up milestone if useful for Open WebUI custom backends.

---

## 6. Letta integration

1. **Config:** `LETTA_BASE_URL` (e.g. `http://bears-letta:8283`), `LETTA_AUTH` (Bearer `LETTA_SERVER_PASS` or as per your Letta version).
2. **Provision:**
   - `POST /v1/agents` with JSON body (name, model, system prompt — store template in Den or env).
   - On success, persist `letta_agent_id` on `bears` row.
3. **Chat:** Use per-agent messages endpoint; enable streaming.
4. **Version drift:** Document that Letta OpenAPI may differ by image tag; pin Letta version in deploy docs and add integration tests against that tag.
5. **Failure modes:** If Letta 5xx, return 502 with correlation id; do not leak Letta stack traces.

---

## 7. Open WebUI integration (Phase 1 target)

**Options** (pick one for first ship):

1. **Custom “API base”** — If Open WebUI can point at a compatible OpenAI-style `/v1/chat/completions`, implement a **thin shim** on Den that maps to Letta (harder if Letta is not OpenAI-shaped).
2. **Pipe / function** — Fork or extend [open-webui-tools](https://github.com/Haervwe/open-webui-tools) pattern: function calls Den `POST /v1/chat/send` with user’s mapped token; bear picker = model list from `GET /v1/bears`.
3. **Middleware proxy** — Less ideal; Open WebUI still thinks it talks to “Letta” but network path goes through Den (only if URL rewrite is trivial).

**User mapping:** On first login from Open WebUI, pass `webui_user_id` header or claim; Den upserts `users.webui_account_id`.

**Deliverable:** Document the chosen path in `den/README.md` + example env for Open WebUI.

### Den native UI: operator console (before Loquix)

**Goal:** An operator with only a browser completes **auth setup**, **users**, **bears + Letta provision**, **membership**, and **LettaBot yaml** handoff without reading API docs.

**Suggested screens (iterate thin):**

1. **Sign in** — operator vs normal user (or single login + role gate on `/console/*`).
2. **Users** — list, create, optional password set; link to membership.
3. **Bears** — list, create, **Provision to Letta** / re-sync, show `letta_agent_id` and errors inline.
4. **Membership** — assign bears ↔ users (checkbox grid or paired selects).
5. **LettaBot** — live YAML preview, download button, bullet list: where to paste, restart bot, Letta `baseUrl` hint.

### Den native UI: Loquix (after chat API is stable)

**Goal:** **End-user** chat alternative to Open WebUI — same `POST /v1/chat/send` and `GET /v1/bears`.

**Pieces:** Serve under **`/app` or `/chat`** so **`/` stays the operator console** unless you prefer a landing page with two links.

1. **Static assets** — Loquix shell in `den/static/chat/` (or separate package).
2. **Axum** — `ServeDir` below API routes; **CORS** only if needed.
3. **Browser → Den** — streaming per [Loquix](https://github.com/loquix-dev/loquix) recipe.
4. **Auth** — end-user session (not operator) for chat testing.

**Milestone:** Ship after **M6** (chat proxy + Open WebUI path proven) or in parallel once M5 is done if you want Loquix-first end users.

---

## 8. LettaBot YAML generation

- **Template:** Stable structure matching [DEN_ARCHITECTURE.md](../architecture/DEN_ARCHITECTURE.md) sample (`agents[].agentId`, `channels.slack.allowedUsers`, …).
- **Data source:** `bears` + `user_bear` + optional `user_external_ids` table (Phase 1: **manual** Slack user ids in admin API or env map).
- **Delivery:**
  - **Pull:** `GET /admin/lettabot.yaml` (also surfaced in **operator console**)
  - **Push (optional):** Write to shared volume if Den has mount (Coolify volume)
- **Reload:** Operator UI should link to this doc: LettaBot restart or SIGHUP if no hot reload — `den/README.md`.

---

## 9. LiteLLM observability (optional Milestone 1.5)

- Env: `LITELLM_BASE_URL`, `LITELLM_MASTER_KEY` (read-only usage).
- Endpoints to call: `/health/liveliness`, `/metrics` (Prometheus), or admin spend API per your LiteLLM version.
- **No** forwarding of chat completions through Den.
- **Attribution:** Document gap: correlating LiteLLM logs to `user_id` may need Letta extra headers — out of Den unless you add Letta config change.

---

## 10. Docker (self-building image)

**Pattern:** multi-stage `Dockerfile` in **`den/`**.

1. **Builder:** `rust:1.xx-bookworm`, install deps, `cargo build --release`.
   - Use **cargo-chef** or **cache mount** (`--mount=type=cache`) to speed rebuilds.
2. **Runtime:** `debian:bookworm-slim`, install `ca-certificates`, copy binary from builder, non-root user.
3. **Entrypoint:** `/usr/local/bin/den` or run migrations then exec (choose one strategy and document).
4. **Coolify:** Service e.g. **`bears-den`**, internal port e.g. `8080`, env from secrets, link to Postgres + network to `bears-letta`.

**`.dockerignore`:** `target/`, `.git/`, etc.

---

## 11. Configuration (environment)

| Variable | Required | Purpose |
|----------|----------|---------|
| `DATABASE_URL` | yes | Postgres connection string |
| `BIND_ADDR` | no | Default `0.0.0.0:8080` |
| `SESSION_SECRET` | yes | HMAC/signing for cookies |
| `LETTA_BASE_URL` | yes | Letta root URL |
| `LETTA_AUTH` | yes | Bearer token for Letta |
| `ADMIN_API_KEY` | yes (prod) | Machine/automation access to admin JSON API; **browser uses operator session** |
| `BOOTSTRAP_ADMIN_EMAIL` | no | First-run: promote this user to `is_admin` on registration (homelab) |
| `RUST_LOG` | no | `den=info,tower_http=info` |
| `LITELLM_BASE_URL` | no | Observability only |
| `LITELLM_MASTER_KEY` | no | If required by LiteLLM admin |

---

## 12. Security checklist (Phase 1)

- [ ] Never expose `LETTA_AUTH` or `ADMIN_API_KEY` to browsers; operator console uses **cookie session** + `is_admin` (or equivalent)
- [ ] Argon2 cost params documented for homelab vs prod
- [ ] Rate limit on `/v1/chat/send` and `/auth/login`
- [ ] CORS restricted to trusted web origins if credentialed cookies cross-origin; **Loquix on same host as Den** avoids this for the native UI
- [ ] SQL injection: only parameterized queries (sqlx)
- [ ] Dependencies: `cargo audit` in CI

---

## 13. Testing strategy

| Layer | What |
|-------|------|
| Unit | Password verify, membership guard, yaml render |
| Integration | Postgres + Den with `testcontainers` or docker-compose test job |
| Letta | Optional `wiremock` or recorded HTTP for CI; nightly job against real Letta |
| Manual | `curl` optional; **primary:** operator walks console → test user signs in → sees bears |

---

## 14. Milestones (suggested order)

**First user-testable moment:** end of **M5** — operator completes full setup in the **console** (users, bears, Letta provision, membership, LettaBot YAML); a **test user** can sign in and **list** bears; chat may still be `curl`/adapter-only until M6.

| # | Milestone | Exit criteria |
|---|-----------|----------------|
| M0 | **Trestle bootstrap → Den** | Throwaway scaffold merged into `den/`; Axum `GET /health`, config, tracing, Dockerfile |
| M1 | Postgres | Migrations applied (`users.is_admin`, …); no business logic |
| M2 | Auth | Register/login gated; session or API token; **operator login** with `is_admin` (or bootstrap admin) |
| M3 | Admin API: users, bears, membership | JSON CRUD + user sees only member bears on `GET /v1/bears`; non-member 403 on chat (stub ok until M5) |
| M4 | Letta provision | `POST /admin/bears` (+ provision) creates Letta agent + row; errors returned to client |
| **M4b** | **Operator console v1** | **Browser UI** covers: users, bears + provision trigger, membership, LettaBot YAML view/download, Letta health — **no curl for setup** |
| M5 | Chat proxy | Streaming `POST /v1/chat/send` end-to-end; test user chats via console “try it” **or** curl |
| M6 | Open WebUI | Documented integration path; demo user chatting via Den |
| M6b | Loquix UI (optional) | Den serves chat under `/app` or `/chat`; demo user chats in browser |
| M7 | LettaBot yaml polish | Generated yaml matches real bot configs; copy-paste tested from console |
| M8 | Polish | Rate limits, readiness probe, Coolify deploy |

**LiteLLM observability:** M8 or parallel track.

**Note:** **M4b** can overlap **M3–M4** (build UI against stub endpoints first) but must land **before** M6; goal is **shortest path to “someone can try the system.”**

---

## 15. Acceptance criteria (Phase 1 complete)

- [ ] **Operator console:** create users, provision bears to Letta, manage membership, view/download LettaBot yaml — all in browser
- [ ] Open WebUI **and/or** Den-hosted **Loquix** page sends chat **through Den** to Letta with streaming responses (same API contract)
- [ ] At least two users and two bears with **many-to-many** membership verified (user A: bears 1+2; user B: bear 2 only)
- [ ] Non-member cannot invoke bear (403)
- [ ] New bear can be provisioned in Letta from **console** (admin API underneath)
- [ ] LettaBot yaml can be generated from current DB state; LettaBot still talks **direct** to Letta
- [ ] Deployed via **single Dockerfile** build on Coolify (or CI → registry)
- [ ] No Cabinet calls required

---

## 16. Documentation updates after code exists

- [ ] [DEPLOYMENT.md](../deployment/DEPLOYMENT.md) — add Step for Den + Postgres
- [ ] [DEN_ARCHITECTURE.md](../architecture/DEN_ARCHITECTURE.md) — `den/README.md` for ports/env
- [ ] [PLAN.md](PLAN.md) — Phase 1 already links this bootstrap plan

---

## 17. Open decisions (resolve before M4–M6)

1. **Operator UI stack:** server-rendered (Askama/Tera + htmx) vs SPA in `static/`
2. **Auth mechanism for Open WebUI:** cookie from browser vs server-side API token per workspace
3. **Bear id in JSON:** `agent_id` vs `bear_id` vs both with alias
4. **Letta agent create payload:** single template vs per-bear type (personal vs shared)
5. **Conversation id:** Letta thread/conversation API — map Open WebUI `chat_id` to Letta conversation if required by API version

Record decisions in **`den/DECISIONS.md`** as you lock them.
