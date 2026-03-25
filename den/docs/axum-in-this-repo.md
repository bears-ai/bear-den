# Axum in this repository

Short map for Rust developers who are new to [Axum](https://docs.rs/axum). For extractor and route syntax details, see [`axum-handler-patterns.md`](axum-handler-patterns.md).

## Entry and processes

- **[`src/lib.rs`](../src/lib.rs)** — `run()` loads [`Config`](../src/config.rs), connects SQLx, builds the web and/or API [`Router`](https://docs.rs/axum/latest/axum/struct.Router.html), and serves with [`axum::serve`](https://docs.rs/axum/latest/axum/fn.serve.html) on separate [`TcpListener`](https://docs.rs/tokio/latest/tokio/net/struct.TcpListener.html)s when `RUN_WEB` / `RUN_API` are enabled.
- **[`src/main.rs`](../src/main.rs)** — Thin wrapper: `newapp::run().await` (rename the crate in `Cargo.toml` when you fork).

## Web UI

- **[`src/web/mod.rs`](../src/web/mod.rs)** — Defines [`AppState`](../src/web/mod.rs) (pool, MiniJinja env, embedded static asset router, [`Arc`](https://doc.rust-lang.org/std/sync/struct.Arc.html) config).
- Router assembly:
  - Subrouters from [`admin`](../src/web/admin/mod.rs), [`user`](../src/web/user/mod.rs), [`home`](../src/web/home.rs), [`public`](../src/web/public.rs).
  - [`SessionManagerLayer`](https://docs.rs/tower-sessions/latest/tower_sessions/struct.SessionManagerLayer.html) + [`AuthManagerLayer`](https://docs.rs/axum-login/latest/axum_login/struct.AuthManagerLayerBuilder.html) wrap authenticated areas.
  - **Layer order:** routes merged first, then `auth_layer`, then `TraceLayer`. Routes registered **after** `layer(auth_layer)` (e.g. `/healthcheck`, `/health/ready`) are **not** behind axum-login’s permission gate—intentional for probes.
- **State:** `.with_state(state)` installs [`AppState`](../src/web/mod.rs) for handlers that use [`State`](https://docs.rs/axum/latest/axum/extract/struct.State.html).

## Standalone API (OAuth provider + versioned JSON API)

- **[`src/api/service.rs`](../src/api/service.rs)** — Builds the API `Router`:
  - Routes like `/v1.0/...` and OpenAPI/docs get [`ApiState`](../src/api/service.rs) via `.with_state(api_state)` **before** outer [`Layer`](https://docs.rs/tower/latest/tower/trait.Layer.html)s (`CorsLayer`, `TraceLayer`).
  - `/oauth/...` is a nested router with its own state and auth/session layers (see comments in that file: order matters for type inference and middleware wrapping).
- Health: **liveness** `GET /healthcheck`, **readiness** `GET /health/ready` (DB ping).

## Config

- Loaded **once** in `run()` as `Arc<Config>` and passed into web/API builders. Handlers that need URLs or mail settings use `state.config` on the web side.

## Further reading

- [`axum-handler-patterns.md`](axum-handler-patterns.md) — forms, validation, redirects, `{id}` path params.
- [Axum “Application output” tutorial](https://docs.rs/axum/latest/axum/) — general concepts.
