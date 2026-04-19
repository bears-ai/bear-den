# Quick start (local development)

## Run the app

1. Copy [`.env.example`](../.env.example) to `.env` (or set env another way) and set **`DATABASE_URL`** to a PostgreSQL database that exists on your machine or network (empty database is fine).
2. Enable at least one service, for example **`RUN_WEB=true`** (and optionally `RUN_API`, `RUN_WORKERS`).
3. With **`RUN_WEB=true`**, set **`CODEPOOL_BASE_URL`** to your [Codepool](../../codepool/README.md) service (for example `http://localhost:3030`). Den will not start the web server without it.
4. Run:

   ```bash
   cargo run
   ```

   The app applies SQLx migrations from [`migrations/`](../migrations/) automatically. A migration seeds a **bootstrap operator** on empty databases: username **`admin`**, password **`Never deploy with default passwords.`** (see [`migrations/README.md`](../migrations/README.md) § *Default operator account*). Replace that password before any real deployment.

   When you add new migration files, use `sqlx migrate add` / `sqlx migrate run` from the `den/` directory as described in [sqlx-patterns.md](sqlx-patterns.md).

   **Static assets (`src/web/assets/`):** In a **debug** `cargo run`, `memory-serve` registers routes when the binary is **compiled** and reads file bytes from **disk** at request time using those recorded paths. If you add or change files under `src/web/assets/` (for example Deep Chat under `assets/deep-chat/`), run a **fresh build** and **restart** the `den` process; a long-lived or stale process can otherwise return **404** for `/assets/...` even though the files exist in the tree. Release builds embed assets in the binary instead.

You can use the devcontainer in this repo instead of a manual local Postgres setup if that matches your workflow.

## Development-only link prefix

Without `--features production`, [`src/config.rs`](../src/config.rs) sets `URL_PREFIX` to `https://redirectmeto.com/http://localhost:3000/`. Generated absolute links (email verification, telemetry) therefore go through the third-party redirect service [redirectmeto.com](https://redirectmeto.com) before hitting your local app. Edit `URL_PREFIX` in that file if you prefer plain `http://localhost:…`, a tunnel URL, or another approach.

## Templates

In **development**, MiniJinja loads files from **`TEMPLATES_DIR`** (default `src/web/templates`). In **`--features production`** / release Docker builds, templates are **embedded** at compile time—plan on **rebuilding** the binary when HTML changes in production.

## API and JWT secret

If you enable **`RUN_API=true`**, set **`JWT_SECRET`** to a long random value (OAuth access tokens are HS256-signed). Release and Docker images are built with **`--features production`**, which also requires **`JWT_SECRET`** at runtime.

## Fresh database and SQLx offline

The schema is applied automatically on startup from `migrations/`. For **`SQLX_OFFLINE`** / CI builds, run `cargo sqlx prepare` against a database that has run those migrations at least once and commit [`.sqlx/`](../.sqlx/). See [sqlx-patterns.md](sqlx-patterns.md).

**Strict migrations:** By default, SQLx does not ignore migration files missing from the repo. If integration tests or a disposable database fail with a migration history mismatch, fix the database or set **`SQLX_MIGRATE_IGNORE_MISSING=true`** only as a documented recovery step—not for routine production deploys.

## Mail

`MAILGUN_API_KEY` and `MAILGUN_DOMAIN` default to empty; set them (or swap the mail implementation) before relying on outbound email.

## Shutdown

**Ctrl+C** is honored on all platforms; **SIGTERM** triggers graceful shutdown on **Unix** only.
