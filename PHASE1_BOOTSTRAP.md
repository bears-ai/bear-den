# Phase 1 implementation plan (Den) — Trestle bootstrap

**Trestle** is only a **short-lived bootstrap codename** for the first milestone: bare-bones **Axum + PostgreSQL + self-building Docker**. It is **not** a service directory in this repo and does not persist after you have a working skeleton. The **lasting** binary, crate, and deploy artifact are **Den** (see [PLAN.md](PLAN.md), [DEN_ARCHITECTURE.md](DEN_ARCHITECTURE.md)).

Put the Rust project at **`services/den/`** (suggested) with package/binary name **`den`**, Coolify service e.g. **`bears-den`**.

**Phase 1 success** (from PLAN): web users chat **Open WebUI → Den → Letta**; bear registry + **users↔bears** many-to-many; LettaBot stays **direct to Letta** for chat but Den **owns** bear provisioning and **LettaBot config output**; optional **read-only** LiteLLM observability; **no Cabinet**.

---

## 0. Trestle bootstrap (M0 only)

Use whatever **one-off** scaffold you prefer (`cargo new`, an internal template, or a throwaway repo named “trestle”). **Do not** add `services/trestle/` here. When the skeleton has health + config + Dockerfile + migrations wiring, **merge into `services/den/`** and drop the Trestle name from paths and binary.

**M0 exit:** `services/den/` exists with Axum `GET /health`, env-based config, tracing, and a multi-stage `Dockerfile`.

---

## 1. Scope

### In scope (Phase 1)

| Area | Deliverable |
|------|-------------|
| Runtime | Axum HTTP server, structured logging, graceful shutdown |
| Data | PostgreSQL schema + migrations; Den is system of record for users, bears, membership |
| Auth (web-first) | Minimal login suitable for homelab: e.g. session cookie after email+password **or** long-lived API token for Open WebUI server-side calls |
| Bears | CRUD (admin), `letta_agent_id` linkage, provision via Letta REST API |
| Membership | Many-to-many `user_bear`; enforce on every chat |
| Chat | `POST /chat/send` (and/or OpenAI-compatible shim later) → validate → Letta messages API with **SSE streaming** back to client |
| Den web UI (optional) | Static **Loquix** chat page served by Den (`GET /` or `/app`); uses **same** chat + discovery endpoints as Open WebUI — see [Loquix](https://github.com/loquix-dev/loquix) |
| Discovery | `GET /agents` or `GET /bears` → bears the current user may use |
| LettaBot | `GET /internal/lettabot.yaml` (admin-authenticated) **or** write to volume path on change — generated from DB |
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

This plan lives at repo root: **`PHASE1_BOOTSTRAP.md`**. The Rust tree:

```
services/den/
├── Cargo.toml
├── Dockerfile
├── .dockerignore
├── README.md               # runbook: env vars, ports, Open WebUI notes
├── migrations/             # SQL (sqlx or refinery)
│   └── 001_initial.sql
├── static/                 # optional: Loquix chat UI (index.html, bundled assets) or vite output
└── src/
    ├── main.rs
    ├── config.rs           # figment/env: DATABASE_URL, LETTA_*, SESSION_*, BIND_ADDR, STATIC_ROOT?
    ├── error.rs            # unified ApiError → HTTP
    ├── state.rs            # AppState: pool, letta client, config
    ├── db/
    ├── auth/
    ├── handlers/           # health, auth, bears, chat, admin
    ├── letta/              # reqwest client, SSE forward
    ├── observability/
    └── middleware/
```

**Recommendation:** one binary crate **`den`** under `services/den/` (no workspace split until needed).

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

---

## 4. Database schema (v1)

### Tables

**`users`**

- `id` UUID PK
- `email` TEXT UNIQUE NOT NULL (or `username` if no email)
- `password_hash` TEXT NOT NULL (nullable only if token-only users)
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
| GET | `/`, `/assets/*` | Optional: **Loquix** single-page chat (static files); route only paths that do not shadow API |
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

### Admin (protect with `ADMIN_API_KEY` or separate admin role)

| Method | Path | Purpose |
|--------|------|---------|
| POST | `/admin/bears` | Create bear: optionally `provision=true` → create Letta agent then insert row |
| PATCH | `/admin/bears/:id` | Update metadata; optional re-sync Letta |
| POST | `/admin/bears/:id/provision` | Idempotent: create or update Letta agent from Den template |
| POST | `/admin/users/:id/bears` | Grant membership |
| DELETE | `/admin/users/:id/bears/:bear_id` | Revoke |
| GET | `/admin/lettabot.yaml` | Render yaml from DB + template |

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

**Deliverable:** Document the chosen path in `services/den/README.md` + example env for Open WebUI.

### Den native UI: Loquix (optional parallel)

**Goal:** Offer a **first-party** chat UI on Den as an **alternative to Open WebUI**, reusing the **same** `POST /v1/chat/send` streaming handler and `GET /v1/bears` (no second gateway to Letta).

**Pieces:**

1. **Static assets** — `services/den/static/` (or a small `web/` package built with Vite/esbuild): HTML shell with Loquix imports (`@loquix/core/tokens/variables.css`, `define-chat-container`, `define-message-list`, `define-chat-composer`, etc.); see [Loquix README](https://github.com/loquix-dev/loquix) and streaming recipe.
2. **Axum** — `ServeDir` for `/` + assets; mount **below** API routes or use a dedicated prefix (`/app`) if path conflicts are awkward.
3. **Browser → Den** — `fetch` + `ReadableStream` (or `EventSource` if you standardize on SSE) for assistant tokens; map Den’s stream chunks to Loquix message content per your negotiated format.
4. **Auth** — Prefer **same-origin** session cookies so Loquix page and API share Den origin without CORS preflight for cookies; Bearer tokens also work with explicit `Authorization`.

**Milestone:** Ship after **M5** (chat proxy works with `curl`); can land with or shortly after **M6** (Open WebUI). Document “Loquix-only homelab” vs “Open WebUI + Den” in `services/den/README.md`.

---

## 8. LettaBot YAML generation

- **Template:** Stable structure matching [DEN_ARCHITECTURE.md](DEN_ARCHITECTURE.md) sample (`agents[].agentId`, `channels.slack.allowedUsers`, …).
- **Data source:** `bears` + `user_bear` + optional `user_external_ids` table (Phase 1: **manual** Slack user ids in admin API or env map).
- **Delivery:**
  - **Pull:** `GET /admin/lettabot.yaml`
  - **Push (optional):** Write to shared volume if Den has mount (Coolify volume)
- **Reload:** Document that LettaBot must restart or SIGHUP if no hot reload — operational note in `services/den/README.md`.

---

## 9. LiteLLM observability (optional Milestone 1.5)

- Env: `LITELLM_BASE_URL`, `LITELLM_MASTER_KEY` (read-only usage).
- Endpoints to call: `/health/liveliness`, `/metrics` (Prometheus), or admin spend API per your LiteLLM version.
- **No** forwarding of chat completions through Den.
- **Attribution:** Document gap: correlating LiteLLM logs to `user_id` may need Letta extra headers — out of Den unless you add Letta config change.

---

## 10. Docker (self-building image)

**Pattern:** multi-stage `Dockerfile` in **`services/den/`**.

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
| `ADMIN_API_KEY` | yes (prod) | Protect admin routes |
| `RUST_LOG` | no | `den=info,tower_http=info` |
| `LITELLM_BASE_URL` | no | Observability only |
| `LITELLM_MASTER_KEY` | no | If required by LiteLLM admin |

---

## 12. Security checklist (Phase 1)

- [ ] Never expose `LETTA_AUTH` or `ADMIN_API_KEY` to browsers
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
| Manual | `curl` scripts in `services/den/scripts/` for login → list bears → stream chat |

---

## 14. Milestones (suggested order)

| # | Milestone | Exit criteria |
|---|-----------|----------------|
| M0 | **Trestle bootstrap → Den** | Throwaway scaffold merged into `services/den/`; Axum `GET /health`, config, tracing, Dockerfile |
| M1 | Postgres | Migrations applied; no business logic |
| M2 | Auth | Register/login disabled or gated; session or API token works |
| M3 | Bears + membership | Admin CRUD + user sees only member bears |
| M4 | Letta provision | `POST /admin/bears` creates Letta agent + row |
| M5 | Chat proxy | Streaming `POST /v1/chat/send` end-to-end with curl |
| M6 | Open WebUI | Documented integration path; demo user chatting via Den |
| M6b | Loquix UI (optional) | Den serves static Loquix page; demo user chats in browser via same streaming API |
| M7 | LettaBot yaml | `GET /admin/lettabot.yaml` matches real bot config needs |
| M8 | Polish | Rate limits, readiness probe, Coolify deploy |

**LiteLLM observability:** M8 or parallel track.

---

## 15. Acceptance criteria (Phase 1 complete)

- [ ] Open WebUI **and/or** Den-hosted **Loquix** page sends chat **through Den** to Letta with streaming responses (same API contract)
- [ ] At least two users and two bears with **many-to-many** membership verified (user A: bears 1+2; user B: bear 2 only)
- [ ] Non-member cannot invoke bear (403)
- [ ] New bear can be provisioned in Letta from Den admin API
- [ ] LettaBot yaml can be generated from current DB state; LettaBot still talks **direct** to Letta
- [ ] Deployed via **single Dockerfile** build on Coolify (or CI → registry)
- [ ] No Cabinet calls required

---

## 16. Documentation updates after code exists

- [ ] [DEPLOYMENT.md](DEPLOYMENT.md) — add Step for Den + Postgres
- [ ] [DEN_ARCHITECTURE.md](DEN_ARCHITECTURE.md) — `services/den/README.md` for ports/env
- [ ] [PLAN.md](PLAN.md) — Phase 1 already links this bootstrap plan

---

## 17. Open decisions (resolve before M4–M6)

1. **Auth mechanism for Open WebUI:** cookie from browser vs server-side API token per workspace
2. **Bear id in JSON:** `agent_id` vs `bear_id` vs both with alias
3. **Letta agent create payload:** single template vs per-bear type (personal vs shared)
4. **Conversation id:** Letta thread/conversation API — map Open WebUI `chat_id` to Letta conversation if required by API version

Record decisions in **`services/den/DECISIONS.md`** as you lock them.
