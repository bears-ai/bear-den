# Trestle

This is a starter repository for Rust web applications. It is meant to be useful as boilerplate for coding agents and seeks to provide strong patterns to guide them toward a maintainable, efficient result.

As such, it is opinionated:

- URLs for both APIs and page requests are routed to straightforward [Axum](https://github.com/tokio-rs/axum) handlers.
- HTML responses are generated with [MiniJinja](https://docs.rs/minijinja/) templates.
- Data is stored in Postgresql (bring your own), managed with [SQLx](https://github.com/launchbadge/sqlx) (including migrations).
- An OAuth provider and very minimal user management is included as example code. (With emails sent via [Mailgun](https://www.mailgun.com/))
- Simple in-process worker management is stubbed in.
- Deployment is simple with Docker,

You don't need to be familiar with any of this to get started, but would benefit from having enough technical understanding to make sense of what they are.

Note that the name "newapp" is used in a few places. You or our agent should see [`docs/rename-from-starter.md`](docs/rename-from-starter.md) for details, and can use **`./scripts/verify-rename.sh --strict`** to check.

---

## Key Documentation

| File | |
|------|-|
| `README.md` (this file) | Scope, conventions, how services are toggled |
| `AGENTS.md` | Where agents start; links into `docs/` |
| [`docs/development-principles.md`](docs/development-principles.md) | Development principles (dependencies, frontend minimalism, etc.); fill in as your team agrees |
| `docs/` | Axum, SQLx, MiniJinja, deploy, auth/OAuth provider notes |
| [`.env.example`](.env.example) | Sample **runtime** env for a hello-world deploy |
| [`deploy/docker-build.env.example`](deploy/docker-build.env.example) | Sample **`DATABASE_URL` for `docker build`** (SQLx) |

---

## Quickstart

Use the devcontainer or local `.env` (see [`.env.example`](.env.example)) with `DATABASE_URL`, apply migrations (`sqlx migrate run` or equivalent), set `RUN_WEB=true` (and optionally `RUN_API`, `RUN_WORKERS`), then `cargo run`.

**Development-only link prefix:** Without `--features production`, [`src/config.rs`](src/config.rs) sets `URL_PREFIX` to `https://redirectmeto.com/http://localhost:3000/`. Generated absolute links (email verification, telemetry) therefore go through the third-party redirect service [redirectmeto.com](https://redirectmeto.com) before hitting your local app. Edit `URL_PREFIX` in that file if you prefer plain `http://localhost:…`, a tunnel URL, or another approach.

**Templates:** in **development**, MiniJinja loads files from `TEMPLATES_DIR` (default `src/web/templates`). In **`--features production`** / release Docker builds, templates are **embedded** at compile time—plan on **rebuilding** the binary when HTML changes in production.

**Fresh database:** the only schema file is `migrations/20250309000000_trestle.up.sql`. For `SQLX_OFFLINE` / CI builds, run `cargo sqlx prepare` against a DB with that migration applied and commit `.sqlx/`.

**Mail:** `MAILGUN_API_KEY` and `MAILGUN_DOMAIN` default to empty; set them (or swap the mail implementation) before relying on outbound email.

**Shutdown:** **Ctrl+C** is honored on all platforms; **SIGTERM** triggers graceful shutdown on **Unix** only.

---

## License

This project is licensed under the [MIT license](LICENSE.md).
