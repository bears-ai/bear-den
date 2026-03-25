# Quick start (local development)

## Run the app

1. Copy [`.env.example`](../.env.example) to `.env` (or set env another way) and set **`DATABASE_URL`** to a PostgreSQL database that exists on your machine or network.
2. Apply migrations from [`migrations/`](../migrations/), for example:

   ```bash
   sqlx migrate run
   ```

   (Use the same `DATABASE_URL` the app will use.)

3. Enable at least one service, for example **`RUN_WEB=true`** (and optionally `RUN_API`, `RUN_WORKERS`).
4. Run:

   ```bash
   cargo run
   ```

You can use the devcontainer in this repo instead of a manual local Postgres setup if that matches your workflow.

## Development-only link prefix

Without `--features production`, [`src/config.rs`](../src/config.rs) sets `URL_PREFIX` to `https://redirectmeto.com/http://localhost:3000/`. Generated absolute links (email verification, telemetry) therefore go through the third-party redirect service [redirectmeto.com](https://redirectmeto.com) before hitting your local app. Edit `URL_PREFIX` in that file if you prefer plain `http://localhost:…`, a tunnel URL, or another approach.

## Templates

In **development**, MiniJinja loads files from **`TEMPLATES_DIR`** (default `src/web/templates`). In **`--features production`** / release Docker builds, templates are **embedded** at compile time—plan on **rebuilding** the binary when HTML changes in production.

## Fresh database and SQLx offline

The schema is applied via migrations under `migrations/`. For **`SQLX_OFFLINE`** / CI builds, run `cargo sqlx prepare` against a database with migrations applied and commit [`.sqlx/`](../.sqlx/). See [sqlx-patterns.md](sqlx-patterns.md).

## Mail

`MAILGUN_API_KEY` and `MAILGUN_DOMAIN` default to empty; set them (or swap the mail implementation) before relying on outbound email.

## Shutdown

**Ctrl+C** is honored on all platforms; **SIGTERM** triggers graceful shutdown on **Unix** only.
