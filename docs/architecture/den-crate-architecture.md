# Den Crate Architecture

**Status:** Current architecture, updated 2026-06 after the crate split and protocol-agnostic cleanup.

Den is one deployable Rust binary (`den`) made from a Cargo workspace. Crates exist to keep build/test iteration fast, enforce dependency direction, and keep protocol-specific edge concerns out of the runtime core. They are not process boundaries.

## Current Graph

```text
den binary
  - startup, migrations, DI/router composition
  - concrete builtin Den tool executors (`src/core/tools`)
  - internal Den-tool invoke endpoint (`src/internal_tools.rs`)

edge crates
  den-web       server-rendered UI + /v1 web chat edge
  den-api       JSON/REST + OAuth app composition
  den-bearwire  BearWire armature RPC/SSE edge

service/runtime crates
  den-runtime   native agent loop, turn execution, context assembly, runtime strategy
  den-service   shared service state and data services (`DenState`, sessions, tool turns,
                turn cancellation, bear registry/domain services, conversation persistence,
                prompt-memory store, pair reflection, Bifrost metadata, recall)
  den-protocol  stable runtime DTOs/contracts shared by runtime and edges

leaf/domain crates
  den-core      shared types/errors/config plus model-facing Den tool surface
  den-llm       Bifrost/LLM client and model registry helpers
  den-memory    per-Bear SQLite memory store and memory helpers
  den-docket    Docket jobs/tasks/work-plan orchestration
  den-http      HTTP/identity/error foundation and armature-token helpers
  den-oauth     OAuth2 server and bearer-auth helpers
```

## Dependency Rules

- Leaves do not depend on edges.
- `den-core` stays foundational: shared vocabulary, errors, config, and Den tool descriptors live there; concrete execution does not.
- `den-runtime` owns the native loop, but not HTTP routing, web UI, OAuth, or binary-local builtin tool wiring.
- `den-service` owns shared concrete service state below the edges, not runtime execution logic.
- Edge crates are peers. They depend downward on service/runtime/protocol crates; they do not depend on each other.
- The `den` binary is the composition root. It is allowed to depend on everything and wire concrete implementations together.

## Protocol Boundary

The runtime is protocol-agnostic. Den uses neutral terms for core concepts:

- `client_session_id`, `client_session`, `client_turn` for session/turn identity
- `armature` / `armature_tools` for trusted local work-surface tools
- `channel` for conversation-only surfaces
- `adapter` for protocol bridge code

ACP-specific code and naming were removed from Den runtime/core/service/API code. The legacy `den-acp` crate was deleted after BearWire became the armature edge and `/acp` routes were retired. See [clients-channels-armatures.md](../guides/clients-channels-armatures.md) for terminology.

## Tool Surface And Execution

The model-facing Den tool **surface** lives in `den-core::tools`: descriptors, argument shapes, dispatch, context types, display metadata, work-surface helpers, and capability traits.

The concrete Den tool **executors** live in the `den` binary under `src/core/tools`. This split is intentional:

- `den-runtime` must see the static tool surface to build prompts and route tool events.
- Concrete executors call runtime/service/memory/reflection/Docket code and therefore sit above those crates.
- A single `den-tools` crate would either be misleading or create dependency cycles.

`den-runtime` exposes `RuntimeToolInvoker`; the binary injects the concrete invoker at startup. `/internal/den-tools/invoke` also lives in the binary (`src/internal_tools.rs`) because it needs concrete builtin tool execution and should not pull those dependencies into `den-api`, `den-http`, `den-service`, or `den-runtime`. This route is unavailable when `DEN_INTERNAL_TOKEN` is unset/blank, and otherwise requires that service-level secret. The secret authenticates an internal caller; it does not by itself prove a caller-supplied conversation, hat, user, or run context. Remaining direct-tool origin verification is tracked in the [hat access plan](../roadmap/HATS_AND_SESSION_MEMORY_BOUNDARIES_PLAN.md#gate-0a--hat-owned-tool-and-network-permissions-partial-cutover-exit-pending).

### Den-Side Privileged Tools

A tool that needs authority unavailable to an Armature sandbox must execute on the Den side. The sandbox may request the operation through the normal tool protocol, but it must not receive the underlying capability or an arbitrary-command escape hatch.

- Put its typed request/response contract and descriptor in `den-core::tools`; those shared types are data, not privileged services.
- Pass the per-invocation context from `den-core::tools` so the executor can bind authorization, scope, and audit records to the Bear, conversation/session, and work surface.
- Implement and wire the executor in `den/src/core/tools`, using explicit dependencies or narrow capability traits. Do not add a shared capability-bearing `ToolContext` service bag.
- Treat all arguments, including sandbox-supplied paths and identifiers, as untrusted. Validate them against the invocation's authorized workspace/sandbox scope before acting.
- Expose only explicit, typed operations; prefer allowlists and bounded output, and never turn this tool into arbitrary Den-side command execution.
- Return structured outcomes and errors, and retain only the non-secret audit metadata needed to correlate the action with its invocation.
- Include a focused check that out-of-scope or unauthorized input is rejected.

## Durable Lessons From The Split

- **Triage before splitting.** Co-locating modules inside the original crate made later `git mv` extraction mechanical and reviewable.
- **Traits before crates.** Extracting a crate behind unstable public signatures increases rebuild churn. Consumer-owned inversion traits (`ToolContext`, `RuntimeToolInvoker`) kept lower crates clean.
- **Stable leaves matter.** `den-core`, `den-protocol`, `den-llm`, `den-memory`, and `den-docket` are useful only while their public surfaces stay narrow and stable.
- **Edges should be isolated.** High-churn HTTP/templates/API code belongs at the top of the graph so UI/API edits do not rebuild the runtime core.
- **Do not force everything into a crate.** Workflow/Docket tool execution remains binary-side where its DTOs depend on `den-docket`; moving those shapes into `den-core` would create a cycle.
- **Avoid compatibility shims that outlive the migration.** Re-export shims are useful during staged moves, but should be removed once callers import owning crates directly.
- **Build timings should drive refactors.** The most valuable graph reshapes were those that shortened the critical path, such as removing web/API/adapter edge dependencies and keeping web independent of runtime execution internals.
- **Cargo feature unification changes the calculus.** A `den-core`/`den-db` split was evaluated and deferred: full-workspace builds compile `sqlx` once regardless, and the remaining `den-core` SQLx coupling was too small to justify another crate.
- **Strict linting should follow a clean baseline.** The workspace now gates default clippy plus curated pedantic/nursery lints; broad stylistic lints are explicitly allow-listed.

## Verification Pattern

For crate-boundary changes, prefer focused checks first:

```bash
cargo check -p den-core -p den-service -p den-runtime -p den --lib
cargo test -p den-core --lib
cargo test -p den-service --lib
```

For release/deploy-impacting Den changes, also run the Docker build or state explicitly why it was not run.
