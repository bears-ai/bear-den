# Repository overview

**This repository** is a Rust web starter: one binary, optional **web** (`RUN_WEB`), **API** (`RUN_API`), and **in-process workers** (`RUN_WORKERS`). The crate defaults to package name **`newapp`**; see [`rename-from-starter.md`](rename-from-starter.md) when rebranding. Service toggles and runtime layout: [`infrastructure-and-ops.md`](infrastructure-and-ops.md).

See **[`docs/README.md`](README.md)** for the documentation index.

## Where things live

| Area | Path | Role |
|------|------|------|
| Web UI | `src/web/` | Axum routes, MiniJinja templates under `src/web/templates/`, static assets |
| Standalone API | `src/api/` | API + OAuth provider; templates under `src/api/templates/` |
| Shared domain | `src/core/` | Users, email, shared DB access; migrations in `migrations/` |
| Config & entry | `src/config.rs`, `src/main.rs` | Service toggles, startup |

The default schema is a **single migration** ([`migrations/20250309000000_trestle.up.sql`](../migrations/20250309000000_trestle.up.sql)): `users`, `invites`, `email_configs`, `email_messages`, and OAuth provider tables. Rust code is trimmed to **web UI + auth**, **admin (users, OAuth clients)**, and the **standalone API** (OAuth + `v1.0` user/profile routes).
