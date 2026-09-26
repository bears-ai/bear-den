# AGENTS.md

How to orient in **Den**: this is the Rust service for Bear identity/session state, Den-hosted memory and work tools, BearWire, web/API surfaces, canonical conversation persistence, and the Den-native agent runtime.

Den is no longer just a generic Axum starter or a Letta/Codepool orchestration shim. The active Pair/ACP path is:

```text
ACP client / armature
        │
        ▼
bear-armature
        │ BearWire v1
        ▼
den-bearwire
        │
        ▼
Den-native Pair runtime
```

Key crates and areas:

- `crates/den-bearwire/` — BearWire RPC/SSE edge for armatures.
- `crates/den-runtime/` — native runtime, agent loop, conversation persistence, BearWire event projection, memory/runtime helpers.
- `crates/den-core/` — descriptor-owned Den tools, tool constants, dispatch, policy/context types.
- `src/core/tools/` — concrete Den tool context wiring for builtin Den-hosted tools.
- `migrations/` — Postgres schema.
- `src/lib.rs` / `src/main.rs` — binary composition and service startup.

For repository-wide project rules, Bear concepts, worktree safety, and stack commands, also read [`../../AGENTS.md`](../../AGENTS.md).

## BearWire / ACP runtime rules

- BearWire is the canonical Den ↔ armature wire. Prefer `den-bearwire` for new armature-facing behavior.
- Do not reintroduce adapter-SSE or legacy `/acp/**` hot-path behavior when BearWire can handle the flow.
- ACP/Zed is an armature, not a generic channel. It owns local filesystem/git/terminal/MCP execution and permission UI.
- Channels such as Slack, WhatsApp, web chat, and macOS app chat should be implemented as channel adapters, not as ACP armatures. See [`../../docs/roadmap/DEN_CHANNELS_IMPLEMENTATION_PLAN.md`](../../docs/roadmap/DEN_CHANNELS_IMPLEMENTATION_PLAN.md).

## Tool surfaces and routing

Den-native Pair sessions expose a stable mixed tool surface:

- Den-hosted tools:
  - `session_info`
  - `memory_write_entry`
  - `memory_status`
  - `memory_browse`
  - `memory_read`
  - `memory_search`
  - `memory_request_review`
  - `web_fetch`
  - `web_search`
  - `list_task_lists`
  - `get_task_list_status`
  - `update_task_list`
  - `request_task_list_handoff`
  - `set_conversation_title`
- Armature-local/client tools:
  - `fs_*`
  - `git_*`
  - `terminal_run_command`
  - `process_run`
  - forwarded MCP tools.

Rules:

- Do not use prompt heuristics to hide or reveal Pair tools turn-by-turn. This caused ACP sessions to lose filesystem capabilities after meta/capability questions.
- Route by descriptor ownership, not by ad hoc tool-name matches:
  - Den-hosted tools execute in Den through the Den tool dispatcher/invoker.
  - Armature-local and forwarded MCP tools are emitted to the armature/client.
- If the model sees a Den-hosted tool such as `list_task_lists`, Den must be able to execute it server-side. It must not reach `bear-armature` as an unsupported local tool request.
- Keep model-facing names descriptor-owned and concise; do not advertise legacy `den_*`, `situation_get`, `memory_tree`, `list_plans`, `get_plan_status`, `update_plan`, `request_work_handoff`, or implementation-branded names.

## Conversation history

- Canonical conversation persistence is the source of truth for transcript replay.
- Native runtime history loading should use shared transcript projection helpers, not raw `message_type` string checks.
- Keep model transcript replay and user-visible history as separate projections.
- For BearWire multi-turn fixes, test both:
  1. current turn persistence for future history;
  2. next-turn LLM request includes prior user and assistant messages exactly once.

## Verifying Rust changes (agents + dev containers)

**`cargo` is available** in typical dev containers and CI images that include the Rust toolchain. After editing this crate, run checks from the repository root with `--manifest-path services/den/Cargo.toml`, or from `services/den/` directly, for example:

- `cargo build` or `cargo check` — compile the library + binary; for host-side commands, prefix with `SQLX_OFFLINE=true` (details below).
- `cargo test` — unit tests; integration tests that need Postgres require `DATABASE_URL` and applied migrations (see [Den quickstart](../../docs/guides/den-quickstart.md)).
- `cargo clippy --all-targets` — Clippy is not suppressed at the crate root; review module-level legacy allowances in the code you touch. Fix warnings in code you touch and shrink those module-level allows over time.

Do not assume the environment is “simulated only”: prefer running focused `cargo` checks yourself to catch compile errors before handing work back.

Useful focused checks:

> **SQLx offline builds:** Normal focused Rust checks must use the checked-in SQLx query metadata rather than trying to resolve a development database host. Prefix host-side commands with `SQLX_OFFLINE=true`, for example `SQLX_OFFLINE=true cargo check --manifest-path services/den/Cargo.toml -p den-web`. The Docker smoke-stack build already enables this. New or modified static SQL must use `query!`, `query_as!`, or `query_scalar!`; treat the count of runtime-query call sites as a ratchet that must not grow. Genuinely dynamic SQL may use runtime APIs with an immediately preceding `// sqlx-dynamic: <reason>` comment. Do not replace SQLx compile-time macros with runtime queries to work around an unavailable database; refresh `.sqlx` with `../../scripts/sqlx.sh prepare-all` from an environment with the migrated database when queries or schema change. Never use a bare `cargo sqlx prepare --workspace -- --all-targets`: it does not expand member-crate test targets and deletes their existing entries, so it silently leaves `.sqlx` incomplete (see [SQLx patterns](../../docs/guides/sqlx-patterns.md)). CI runs `../../scripts/sqlx.sh migrate run` then `../../scripts/sqlx.sh prepare --check --workspace -- --all-targets`, but `--check` only warns about extra entries; the clippy step is what fails on missing ones.

```bash
SQLX_OFFLINE=true cargo test --manifest-path services/den/Cargo.toml -p den-bearwire bearwire_
cargo test --manifest-path services/den/Cargo.toml -p den-runtime pair_
cargo test --manifest-path services/den/Cargo.toml -p den-runtime den_tools_route_server_side_but_client_tools_do_not
cargo test --manifest-path services/den/Cargo.toml -p den-bearwire bearwire_
```

**Docker build:** For release/deploy-impacting changes, do not treat the change as complete until a `docker build` of [`Dockerfile`](Dockerfile) from `services/den/` succeeds. For narrow Rust/runtime changes, run the most specific cargo tests first and explicitly state if Docker was not run. Release images use `--features production`, Alpine/musl, and SQLx at build time in ways a local glibc `cargo check` does not fully replicate. When Docker is unavailable locally, say so explicitly (build-time env: [Den deployment](../../docs/guides/den-deploy.md), [`COOLIFY_DEPLOY.md`](COOLIFY_DEPLOY.md)).

## Documentation entry point

Start with the repository [topic map](../../docs/README.md) for verified current behavior, linked active plans, decisions, and developer guides. Do not use older roadmap status summaries as the source of truth for what is deployed. For this service, use [Den quickstart](../../docs/guides/den-quickstart.md) and the [architecture index](../../docs/architecture/README.md).

## Database migrations (SQLx)

- **Never edit** an existing file under `migrations/` that has already been applied anywhere: SQLx checksums the file content in `_sqlx_migrations`. **Add a new** `*_up.sql` for fixes or new columns (see [`migrations/README.md`](migrations/README.md)).
- New migrations should follow the reversible / expand-contract deployment policy documented in [`migrations/README.md`](migrations/README.md). In short: add a matching `.down.sql` for each new `.up.sql` unless explicitly justified, prefer backward-compatible expand steps first, and defer destructive contract changes until a later deploy.
- Den startup now rejects databases whose successful SQLx version is newer than the binary's embedded migrator. Keep deploy docs and migration reviews aligned with that guard; details live in [`migrations/README.md`](migrations/README.md) and [`COOLIFY_DEPLOY.md`](COOLIFY_DEPLOY.md).
- If checksum drift already happened, follow **Repairing checksum mismatch** in that README (`sqlx migrate info`, then align `checksum` with the canonical file).

## Working on features

- **BearWire / armatures** — `crates/den-bearwire/`, `crates/den-runtime/src/runtime/bearwire_projection/`, and `tools/bear-armature/` at the repo root.
- **Native runtime / agent loop** — `crates/den-runtime/src/agent_loop/`, `crates/den-runtime/src/native_runtime/`.
- **Den-hosted tools** — descriptors and dispatch in `crates/den-core/src/tools/`; concrete service wiring in `src/core/tools/`.
- **Conversation persistence/history** — `crates/den-runtime/src/conversation/`, `crates/den-runtime/src/native_runtime/turn.rs`, BearWire history/replay in `crates/den-bearwire/`.
- **HTTP web UI** — `src/web/`, templates under `src/web/templates/`. CSS: follow [frontend development](../../docs/guides/frontend-development.md): no authored `<style>` blocks or inline layout/theme in templates; standalone pages still use `/assets/css/style.css` and scoped rules in `src/web/assets/css/specifics.css`.
- **HTTP API / OAuth provider** — `src/api/`.
- **Config** — `src/config.rs`, plus env and ops notes in [Den deployment](../../docs/guides/den-deploy.md), [infrastructure and ops](../../docs/guides/infrastructure-and-ops.md), and [`.env.example`](.env.example).
- **Entrypoint / workers** — [`src/lib.rs`](src/lib.rs) (`run()`), thin [`src/main.rs`](src/main.rs).

## After substantial changes

- Update the affected [topic page](../../docs/README.md) when verified current behavior changes; link plans for intended work and ADRs for rationale. If documentation is unaffected, explain why in the PR.
- Document repeatable developer workflows in the relevant guide or service-local runbook rather than creating another summary.

## Patterns (read when touching that layer)

| Topic | Doc |
|--------|-----|
| Development principles | [Development principles](../../docs/guides/development-principles.md) |
| SQLx macros & `cargo sqlx prepare` | [SQLx patterns](../../docs/guides/sqlx-patterns.md) |
| `minijinja::context!` | [MiniJinja contexts](../../docs/guides/minijinja-context-patterns.md) |
| Axum routers, state, layers | [Axum in this repo](../../docs/guides/axum-in-this-repo.md) |
| Axum routes & extractors (`{id}` not `:id`) | [Axum handler patterns](../../docs/guides/axum-handler-patterns.md) |
| Services, deploy, ops | [Infrastructure and ops](../../docs/guides/infrastructure-and-ops.md) |
| Local quickstart (`cargo run`, dev quirks) | [Den quickstart](../../docs/guides/den-quickstart.md) |
| Deploy notes | [Den deployment](../../docs/guides/den-deploy.md) |
| Frontend / templates | [Frontend development](../../docs/guides/frontend-development.md) |
| MiniJinja template limits | [MiniJinja template limitations](../../docs/guides/minijinja-template-limitations.md) |

## Planning docs (BEARS)

Find the topic first and follow its linked plans or the [planning index](../../docs/roadmap/README.md). Do not duplicate roadmap Markdown under `services/den/plans/`.
