# Phase 1 implementation plan (Den) — Trestle bootstrap

**Trestle** is only a **short-lived bootstrap codename** for the first milestone: bare-bones **Axum + PostgreSQL + self-building Docker**. It is **not** a service directory in this repo and does not persist after you have a working skeleton. The **lasting** binary, crate, and deploy artifact are **Den** (see [PLAN.md](PLAN.md), [DEN_ARCHITECTURE.md](../architecture/DEN_ARCHITECTURE.md)).

Put the Rust project at repo-root **`den/`** with package/binary name **`den`**, Coolify service e.g. **`bears-den`**.

**Phase 1 success** (from PLAN): **operator console** usable for full provisioning; web users chat via **Den's chat UI → Den → LettaBot → Letta** as the **primary** end-user path; **Open WebUI → Den → LettaBot → Letta** is an **optional** addition for teams that want it; bear registry + **users↔bears** many-to-many; **LettaBot** is the **required** agent runtime (channels and web); **Den** owns bear provisioning on Letta, **LettaBot config**, **skills catalog + per-bear attachments**, and **MCP catalog + per-bear MCP attachments** (materialized for LettaBot; same patterns as skills; **Coolify** runs MCP server processes); optional **read-only** Bifrost observability; **no Cabinet**.

**Delivery priority:** Reach the **first user-testable moment** as early as possible: an **operator provisioning UI** (browser) for **authentication**, **user** lifecycle, **agent/bear** lifecycle (Letta create/sync), **membership**, **LettaBot** setup (preview/download generated `lettabot.yaml`, copy-paste instructions), **skills** and **MCP servers** (each: catalog + attach per bear; develop **side-by-side**; materialization may land in **M7** or dedicated milestones if chat stability comes first). End-user **chat** (same-origin on Den) follows once the **chat API** (M5) is stable; **Open WebUI** integration can ship after when needed.

**Locked product decisions** (operator UI, streaming, API IDs, provisioning, threading, deferred Open WebUI auth): [PHASE1_DECISIONS.md](PHASE1_DECISIONS.md).

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
| **Operator console (priority)** | **Browser UI** served by Den for: operator login; **create/edit users** (and end-user login/register as policy allows); **create/provision bears** (Letta agent create/update); **grant/revoke membership**; **skills** (org catalog, attach/detach per bear, optional paste-from-URL); **MCP servers** (local catalog, optional official-registry import, attach/detach per bear—same UX patterns as skills); **Letta health** indicator; **LettaBot** panel (rendered YAML, download, short deploy checklist). Same actions backed by JSON admin API; **no `curl` required** for happy-path setup. |
| Auth (web-first) | Session cookie after email+password **or** long-lived API token for automation / optional Open WebUI server-side calls; **operators** use a distinct **admin/operator** session or role (e.g. `users.is_admin`, bootstrap admin email) — **do not** expose `ADMIN_API_KEY` to browser JavaScript |
| Bears | CRUD (admin API + operator UI), `letta_agent_id` linkage, provision via Letta REST API |
| Membership | Many-to-many `user_bear`; enforce on every chat; managed in operator UI |
| Chat | `POST /v1/chat/send` (and/or OpenAI-compatible shim later for Open WebUI) → validate → **LettaBot** bridge → Letta with **SSE streaming** back to client |
| Den chat UI (first-party, priority after M5) | Deep Chat page (`GET /bear/{slug}`); **primary** end-user path — same chat + discovery endpoints as any other HTTP client; same-origin with Den |
| Open WebUI (optional) | Pipe, custom backend, or OpenAI-style shim pointing at Den — **same** membership + streaming contract as Den chat UI; ship when a deployment needs it |
| Discovery | `GET /agents` or `GET /bears` → bears the current user may use |
| LettaBot | `GET /admin/lettabot.yaml` (operator session **or** server-side key); operator UI shows preview; optional write to volume path on change; **skills** materialized to paths/volumes LettaBot reads; **MCP** connection metadata / Letta tool wiring materialized per [DEN_ARCHITECTURE.md](../architecture/DEN_ARCHITECTURE.md) § Den-managed skills and § Den-managed MCP servers |
| Policy | RBAC-lite: membership check + optional per-bear `can_use` + basic rate limit |
| Bifrost | Optional: fetch metrics/health **read-only**; no proxying completions |
| **User onboarding** | On new account creation, auto-provision a **Personal Bear** (slug: `personal-{name-slug}`) from a configurable default template set in the admin UI; immediately redirect the new user into chat with that bear using a standard onboarding prompt that invites the bear to learn about them |
| **Memory dashboard** | User-facing page showing the current user's `human` block (per bear) and any `person:{name}` blocks across all their bears — read-only in Phase 1 |
| **Org policy block** | Admin UI panel to view and edit the `org_policy` Letta block applied to all bears; seed content from **`den/defaults/org_policy.md`** (in-repo) when no policy has been set and the first bear is provisioned |
| Deploy | **Self-building Docker image** (multi-stage: build Rust in container, runtime image with binary + `ca-certificates`) |

### Explicitly out of scope (Phase 1)

- **Optional:** LettaBot → Den → LettaBot **channel-only proxy** for audit (not required for Phase 1); see [PLAN.md](PLAN.md) § Canonical paths vs optional channel proxy
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
├── README.md               # runbook: env vars, ports, chat UI + optional Open WebUI notes
├── migrations/             # SQL (sqlx or refinery)
│   └── 001_initial.sql
├── static/                 # operator console (priority) + chat assets
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
| HTTP client | **reqwest** | Letta + optional Bifrost observability calls |
| SSE / stream | **axum** `body::Body`, `futures::Stream`, or `async-stream` | Proxy **LettaBot → Den → browser** stream without buffering full reply |
| Passwords | **argon2** | `password-hash` crate |
| Sessions | **signed cookie** (e.g. `tower-cookies` + **Key** from `SESSION_SECRET`) **or** opaque token in DB | Pick one for v1; document the other as Phase 1.1 |
| Config | **figment** + `serde` | 12-factor env |
| Errors | **`thiserror`** + HTTP mapping | |
| Logging | **tracing** + **tracing-subscriber** | JSON in production |
| IDs | **uuid** v7 or v4 for `user_id`, `bear_id` | Store as UUID in Postgres |

**Alternates:** Diesel instead of sqlx; rate limit via `tower_governor`.

**Operator UI stack:** Prefer **small and shippable**: e.g. **Askama/Tera** HTML + forms or **htmx** against JSON admin routes; or a **Vite** SPA under `static/` if you want richer tables early. Mounted at **`/`** or **`/console`**; keep API JSON stable for the chat UI and **optional Open WebUI** adapters.

---

## 4. Database schema (v1)

### Tables

**`users`**

- `id` UUID PK
- `email` TEXT UNIQUE NOT NULL (or `username` if no email)
- `display_name` TEXT NULL — human-readable name; used in Personal Bear slug and onboarding prompt
- `password_hash` TEXT NOT NULL (nullable only if token-only users)
- `is_admin` BOOLEAN NOT NULL DEFAULT false — operator console access (bootstrap first admin via migration or `BOOTSTRAP_ADMIN_EMAIL`)
- `webui_account_id` TEXT NULL UNIQUE — map Open WebUI stable id when available
- `created_at`, `updated_at` TIMESTAMPTZ
- *(Phase 2)* `is_provisional` BOOLEAN NOT NULL DEFAULT false — no login; created automatically when LettaBot encounters an unknown user

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

**`bear_conversations`** (**optional cache only**, not source of truth)

- Use only for local UX hints (for example, "resume last thread" in the Den chat UI).
- Do **not** require this table for correctness of chat routing.
- LettaBot + Letta remain canonical for conversation lifecycle and per-channel thread separation.

**`user_bear_blocks`** (tracks per-user Letta blocks for group-mode bears)

- `user_id` UUID FK → users ON DELETE CASCADE
- `bear_id` UUID FK → bears ON DELETE CASCADE
- `letta_block_id` TEXT NOT NULL — Letta block id
- `block_label` TEXT NOT NULL — e.g. `person:alice`
- `created_at` TIMESTAMPTZ
- PK `(user_id, bear_id, block_label)`

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
| GET | `/`, `/console`, `/assets/*` | **Operator console** (priority): static +/or templated pages for provisioning; chat UI lives at **`/bear/{slug}`** (same origin) |
| POST | `/auth/register` | Optional; may disable in prod and use admin-created users |
| POST | `/auth/login` | Returns session cookie or `{ token }` |
| POST | `/auth/logout` | Invalidate session |
| GET | `/v1/bears` or `/agents` | List bears for **authenticated** user (membership filter) |
| POST | `/v1/chat/send` | Body: `{ message, agent_id?, conversation_id?, stream?, channel?, channel_thread_id? }` — **agent_id** = bear id |
| GET | `/v1/me/memory` | Return current user's `human` block content (per bear) and any `person:{name}` blocks across all member bears — for the memory dashboard |

**Chat contract** (align with [PLAN.md](PLAN.md) §2.1):

- Resolve `Authorization: Bearer <token>` or session cookie → `user_id`
- Resolve `agent_id` → `bear_id` → `letta_agent_id`; **403** if not member
- Apply rate limit
- Call **LettaBot** for the bear’s configured bot row (HTTP/gRPC/socket per your deployment — **verify against LettaBot version**); pass through `conversation_id` (if supplied) plus `channel` and `channel_thread_id` so LettaBot resolves/creates the right Letta conversation.
- Keep conversation ownership in LettaBot/Letta; Den should not implement its own canonical thread-mapping logic in Phase 1.
- Stream SSE (or NDJSON) back using a **single documented contract** — first-party chat UI is the reference client; Open WebUI adapters must conform or translate

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
| GET | `/admin/org-policy` | View current `org_policy` block content (from Letta or seeded default) |
| PUT | `/admin/org-policy` | Write `org_policy` block content; Den updates the Letta block on all provisioned bears |
| GET | `/admin/onboarding` | View/edit default Personal Bear template and onboarding prompt used for new users |
| GET/PUT | `/admin/skills`, `/admin/bears/:id/skills` | **Planned:** org skill catalog + per-bear attach list; PUT triggers materialization for LettaBot (exact routes TBD) |

### Internal / optional

| GET | `/internal/bifrost/health` | Proxy read to Bifrost `GET /health` or metrics (if enabled) |

**OpenAPI:** Generate with `utoipa` in a follow-up milestone if useful; optional Open WebUI adapters may consume it.

---

## 6. Letta and LettaBot integration

1. **Config (Letta):** `LETTA_BASE_URL` (e.g. `http://bears-letta:8283`), `LETTA_AUTH` (Bearer `LETTA_SERVER_PASS` or as per your Letta version).
2. **Provision (Den → Letta):**
   - `POST /v1/agents` with JSON body (name, model, system prompt — store template in Den or env).
   - On success, persist `letta_agent_id` on `bears` row.
3. **Chat (Den → LettaBot → Letta):** Den does **not** use Letta’s browser-facing messages API directly for `POST /v1/chat/send`; it calls **LettaBot**, which owns the conversation loop and uses Letta for persistence and models. Den forwards conversation context (`conversation_id`, `channel`, `channel_thread_id`) and LettaBot resolves/creates Letta conversations.
4. **LettaBot config:** Render `lettabot.yaml` from DB; mount or sync **skill directories** per [DEN_ARCHITECTURE.md](../architecture/DEN_ARCHITECTURE.md) § Den-managed skills.
5. **Version drift:** Letta OpenAPI and LettaBot APIs may differ by image/tag; pin versions in deploy docs and add integration tests.
6. **Failure modes:** If LettaBot or Letta returns 5xx, return 502 with correlation id; do not leak upstream stack traces.

---

## 7. End-user chat: Den chat UI (primary) and Open WebUI (optional)

### Den native UI — **first-party chat (Phase 1 target)**

**Goal:** **End-user** chat shipped on Den — **same** `POST /v1/chat/send` and `GET /v1/bears` as every other client; traffic path **Den → LettaBot → Letta**; **no dependency** on Open WebUI for BEARS operators who only need Den + LettaBot + Letta.

**Pieces:** Primary chat route **`/bear/{slug}`** (same origin as Den); **`/`** is operator console unless you add a landing page.

1. **Static assets** — Deep Chat bundle in `den/src/web/assets/deep-chat/`.
2. **Axum** — `memory-serve` for assets, MiniJinja template for the chat page; **same-origin** with Den avoids CORS for cookie sessions.
3. **Browser → Den** — SSE streaming via `POST /v1/chat/send`; **this contract is the reference** shape.
4. **Auth** — end-user session (not operator) for chat.

**Milestone:** Ship in **M6** immediately after **M5** (chat proxy) — **before** optional Open WebUI work.

### Den native UI: operator console

**Goal:** An operator with only a browser completes **auth setup**, **users**, **bears + Letta provision**, **membership**, **skills per bear**, and **LettaBot yaml** handoff without reading API docs.

**Suggested screens (iterate thin):**

1. **Sign in** — operator vs normal user (or single login + role gate on `/console/*`).
2. **Users** — list, create, optional password set; link to membership.
3. **Bears** — list, create, **Provision to Letta** / re-sync, show `letta_agent_id` and errors inline.
4. **Membership** — assign bears ↔ users (checkbox grid or paired selects).
5. **LettaBot** — live YAML preview, download button, bullet list: where to paste, restart bot, Letta `baseUrl` hint.
6. **Skills** — catalog (add from URL or upload), attach to bear, enable/disable; show materialization status when Den syncs trees for LettaBot.

### Open WebUI integration (optional, **M6b**)

**When:** After **M6** proves the Den chat + streaming contract in the browser.

**Options** (pick one per deployment):

1. **Custom “API base”** — If Open WebUI can point at a compatible OpenAI-style `/v1/chat/completions`, implement a **thin shim** on Den that maps to **LettaBot → Letta** (same as native chat path).
2. **Pipe / function** — Fork or extend [open-webui-tools](https://github.com/Haervwe/open-webui-tools) pattern: function calls Den `POST /v1/chat/send` with user’s mapped token; bear picker = model list from `GET /v1/bears`.
3. **Middleware proxy** — Less ideal; Open WebUI still thinks it talks to “Letta” but network path goes through Den (only if URL rewrite is trivial).

**User mapping:** On first login from Open WebUI, pass `webui_user_id` header or claim; Den upserts `users.webui_account_id`.

**Deliverable:** Document the chosen path in `den/README.md` + example env for Open WebUI.

---

## 8. LettaBot YAML generation

- **Template:** Stable structure matching [DEN_ARCHITECTURE.md](../architecture/DEN_ARCHITECTURE.md) sample (`agents[].agentId`, `channels.slack.allowedUsers`, …).
- **Data source:** `bears` + `user_bear` + optional `user_external_ids` table (Phase 1: **manual** Slack user ids in admin API or env map).
- **Delivery:**
  - **Pull:** `GET /admin/lettabot.yaml` (also surfaced in **operator console**)
  - **Push (optional):** Write to shared volume if Den has mount (Coolify volume)
- **Reload:** Operator UI should link to this doc: LettaBot restart or SIGHUP if no hot reload — `den/README.md`.

---

## 9. Bifrost observability (optional Milestone 1.5)

- Env: `BIFROST_BASE_URL` (e.g. `http://bears-bifrost:8080`). Management auth, if any, depends on your Bifrost config (file-only GitOps setups often expose `GET /health` without extra headers).
- Endpoints to call: `GET /health`, Prometheus scrape target if enabled, or log export per [Bifrost observability](https://docs.getbifrost.ai/features/observability/default).
- **No** forwarding of chat completions through Den.
- **Attribution:** Document gap: correlating gateway logs to `user_id` may need Letta extra headers — out of Den unless you add Letta config change.

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
| `BIFROST_BASE_URL` | no | Observability only (e.g. `http://bears-bifrost:8080`) |

---

## 12. Security checklist (Phase 1)

- [ ] Never expose `LETTA_AUTH` or `ADMIN_API_KEY` to browsers; operator console uses **cookie session** + `is_admin` (or equivalent)
- [ ] Argon2 cost params documented for homelab vs prod
- [ ] Rate limit on `/v1/chat/send` and `/auth/login`
- [ ] CORS restricted to trusted web origins if credentialed cookies cross-origin; **chat UI on same host as Den** avoids this for the native UI
- [ ] SQL injection: only parameterized queries (sqlx)
- [ ] Dependencies: `cargo audit` in CI

---

## 13. Testing strategy

| Layer | What |
|-------|------|
| Unit | Password verify, membership guard, yaml render |
| Integration | Postgres + Den with `testcontainers` or docker-compose test job |
| Letta | Optional `wiremock` or recorded HTTP for CI; nightly job against real Letta |
| Manual | `curl` optional; **primary:** operator walks console → test user opens **Den chat** → chats with member bear |

---

## 14. Milestones (suggested order)

**First user-testable moment:** end of **M5** — operator completes full setup in the **console** (users, bears, Letta provision, membership, LettaBot YAML); a **test user** can sign in and **list** bears; streaming chat works via **`curl` or API client** if the chat UI is not merged yet.

**First in-browser end-user chat:** end of **M6** — Den chat UI: a test user chats in the browser through **Den → LettaBot → Letta** (same session/membership rules).

| # | Milestone | Exit criteria |
|---|-----------|----------------|
| M0 | **Trestle bootstrap → Den** | Throwaway scaffold merged into `den/`; Axum `GET /health`, config, tracing, Dockerfile |
| M1 | Postgres | Migrations applied (`users.is_admin`, …); no business logic |
| M2 | Auth | Register/login gated; session or API token; **operator login** with `is_admin` (or bootstrap admin) |
| M3 | Admin API: users, bears, membership | JSON CRUD + user sees only member bears on `GET /v1/bears`; non-member 403 on chat (stub ok until M5) |
| M4 | Letta provision | `POST /admin/bears` (+ provision) creates Letta agent + row; errors returned to client |
| **M4b** | **Operator console v1** | **Browser UI** covers: users, bears + provision trigger, membership, LettaBot YAML view/download, Letta health — **no curl for setup** |
| **M4c** | **Onboarding + org policy** | Admin configures `org_policy` block (seeded from `den/defaults/org_policy.md`) and Personal Bear default template; new user account creation auto-provisions their Personal Bear |
| M5 | Chat proxy | Streaming `POST /v1/chat/send` end-to-end; conversation/thread context forwarded to LettaBot; validated with **curl**, integration test, or console “try it” |
| **M6** | **Den chat UI (first-party)** | Den serves chat at `/bear/{slug}`; demo user chats in browser — **reference client** for streaming contract |
| **M6b** | **Open WebUI (optional)** | Documented integration path + example env; demo user chatting via Den **when a deployment chooses Open WebUI** |
| M7 | LettaBot yaml + skills polish | Generated yaml matches real bot configs; **skills** catalog + per-bear materialization tested; copy-paste / volume deploy tested from console |
| M8 | Polish | Rate limits, readiness probe, Coolify deploy |

**Bifrost observability:** M8 or parallel track.

**Note:** **M4b** can overlap **M3–M4** (build UI against stub endpoints first). **M4c** can overlap **M4b** (org policy UI is a small panel; onboarding wiring needs M4 provision). **M6 (chat UI)** can overlap late **M5** (UI shell early; wire streaming when API is ready). **M6b (Open WebUI)** is **not** on the critical path for “someone can try the system” in-browser.

---

## 15. Acceptance criteria (Phase 1 complete)

- [ ] **Operator console:** create users, provision bears to Letta, manage membership, **manage skills per bear**, view/download LettaBot yaml — all in browser
- [ ] Den-hosted **chat UI** sends chat **Den → LettaBot → Letta** with streaming responses (**primary**); **Open WebUI optional** — if used, same API contract via adapter/shim
- [ ] Conversation behavior proven for one shared bear across at least two channels/threads: same bear identity, distinct Letta threads per channel/thread policy, with LettaBot as canonical mapper
- [ ] At least two users and two bears with **many-to-many** membership verified (user A: bears 1+2; user B: bear 2 only)
- [ ] Non-member cannot invoke bear (403)
- [ ] New bear can be provisioned in Letta from **console** (admin API underneath)
- [ ] LettaBot yaml can be generated from current DB state; **LettaBot → Letta** for persistence; **Den-managed skills** visible to LettaBot for at least one demo bear
- [ ] Deployed via **single Dockerfile** build on Coolify (or CI → registry)
- [ ] No Cabinet calls required
- [ ] New user registration auto-provisions their Personal Bear in Letta and redirects them into chat with the onboarding prompt
- [ ] `GET /v1/me/memory` returns the current user's `human` block content across their bears
- [ ] Admin can view and edit the `org_policy` block via the console; `den/defaults/org_policy.md` is applied on first bear creation when no policy exists

---

## 16. Documentation updates after code exists

- [ ] [DEPLOYMENT.md](../deployment/DEPLOYMENT.md) — add Step for Den + Postgres
- [ ] [DEN_ARCHITECTURE.md](../architecture/DEN_ARCHITECTURE.md) — `den/README.md` for ports/env
- [ ] [PLAN.md](PLAN.md) — Phase 1 already links this bootstrap plan

---

## 17. Open decisions (resolve before M4–M6)

**Resolved for this repo** — see [PHASE1_DECISIONS.md](PHASE1_DECISIONS.md). The list below is the original prompt; update that file if choices change.

1. **Operator UI stack:** server-rendered (Askama/Tera + htmx) vs SPA in `static/`
2. **Streaming payload for `POST /v1/chat/send`:** SSE vs NDJSON — **lock for Den chat UI first**; Open WebUI adapters translate if needed
3. **Auth mechanism for optional Open WebUI:** cookie from browser vs server-side API token per workspace
4. **Bear id in JSON:** `agent_id` vs `bear_id` vs both with alias
5. **Letta agent create payload:** single template vs per-bear type (personal vs shared)
6. **Conversation id:** Letta thread/conversation API — map client `chat_id` to Letta conversation if required by API version

Record new or changed decisions in **[PHASE1_DECISIONS.md](PHASE1_DECISIONS.md)** (monorepo `docs/planning/`).

The planning file **`den/DECISIONS.md`** is **not** used; keep a single source under `docs/planning/`.

---

## 18. Phase 2 pointer (after bootstrap)

Phase 1 delivers skills, **MCP catalog**, chat, and operator flows above. **Phase 2** (see [PLAN.md](PLAN.md) § Phase 2) adds **Cabinet** as Letta-facing tools and **provisional users** (`is_provisional` in schema notes). MCP architecture: [DEN_ARCHITECTURE.md](../architecture/DEN_ARCHITECTURE.md) § Den-managed MCP servers.
