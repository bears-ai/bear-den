# Agent Guide

## Core Policies

SQL operations are performed through SQLx macros, for compile-time validation

Model-facing instructions and prompts are abstracted through the context compilation system. Armature should not be instruction, only providing tool results and error details. Words "baked in" to Den should be found in Markdown files in Den's `prompts/` directory.

We respect the Rust Way and use strong types. Only at the very edges should string comparisions ever drive logic.

Modules should not be large and monolithic. Use submodules liberally.

Substantial tests should be kept in distinct modules. Only the most trivial regression checks should live in normal code files.

## Stack

Three application services run via `docker-compose.yaml` (native runtime):

- `bears-bifrost` is the LLM gateway on port `8080`.
- `bears-den` is the Rust service on port `3000`.
- Optional `bears-postgres` (profile `bundled`) for local Den Postgres.

The workspace container has access to the Docker socket and can manage the stack.

Services are reachable by their compose service names over the internal Docker network, for example `http://bears-den:3000`. The root devcontainer startup script attaches the workspace container to `bears-stack_default` and exports dev defaults for `DATABASE_URL` and `LLM_API_URL`, so Den tests can resolve `bears-postgres` and `bears-bifrost` from inside the devcontainer.

## Scripts

Run smoke tests:

```bash
./scripts/smoke.sh
```

Restart a single service after code changes:

```bash
./scripts/restart.sh bears-den
```

Tail logs for a service:

```bash
./scripts/logs.sh bears-den
```

## Smoke Tests

`tests/smoke/test_stack.py` hits the running stack over HTTP.

Run with:

```bash
./scripts/smoke.sh
```

Build local Den/Codepool/Bifrost images, start/recreate the dev stack, seed, and run smoke tests:

```bash
./scripts/smoke-stack.sh
```

Run SQLx commands through `scripts/sqlx.sh`. It starts and verifies bundled Postgres without building Den, makes the Compose service reachable from this workspace, changes to the Den Cargo workspace, and supplies the matching `DATABASE_URL` to Cargo:

```bash
./scripts/sqlx.sh migrate run
./scripts/sqlx.sh prepare-all
./scripts/sqlx.sh migrate run
./scripts/sqlx.sh prepare --check --workspace -- --all-targets
```

The prepare command and verification script include all Cargo targets, so test-only SQLx macros are represented in the committed `services/den/.sqlx/` cache. Commit cache changes with the query or migration that requires them.

Create Den migrations with SQLx; never hand-write a numeric migration prefix. SQLx generates a unique timestamped version:

```bash
./scripts/sqlx.sh migrate add <description>
```

Use a lowercase underscore description (for example, `add_widget_status`). The pre-commit hook rejects a staged migration version reused for a different description.

## Dependency Hygiene

- Before adding a dependency, check whether an existing crate in the workspace already solves the problem. Prefer reusing existing dependencies over adding parallel libraries for the same concern.
- Keep new dependencies in the narrowest crate that needs them. Do not add a dependency to `den-core`, `den-service`, or the root `den` package unless the functionality is genuinely shared by that layer.
- Prefer focused modules over broad utility crates. If dependency-backed functionality starts growing in a central crate, consider a narrow leaf crate before broadening core/service layers.
- Use workspace dependency declarations for shared versions, but keep feature flags crate-local and surgical. Avoid enabling a broad feature superset just because another crate needs it.
- Keep dev/test dependencies in `[dev-dependencies]`; do not let test-only tooling affect production build graphs.
- When adding prompt, template, or config parsing support, first consider existing workspace dependencies such as `minijinja`, `serde`, and `serde_yml`. Do not introduce filesystem watching, embedding, or markdown-rendering crates unless the implementation truly needs them.
- After changing `Cargo.toml`, inspect the relevant `cargo tree` or focused `cargo check` output and state whether the change adds new packages, broadens features, or only exposes an already-present workspace dependency to another crate.

## Project Concepts

- A Bear's **charter** is a descriptive property of the Bear: its durable purpose/responsibility boundary. Do not model `charter_id`, `charters[]`, or a separate Charter entity unless explicitly requested.
- Bear-scoped records should use `bear_id`. Bear-specific knowledge areas are **Domains** under the Bear, not Cabinet Missions.
- A Cabinet **Mission** is a *page* in Cabinet, not a container entity: its child pages are the grouped material, and page policy/membership decide who may read or edit that subtree. Do not model a Mission id space, collection entity, or Cabinet→Job reference; name a Mission by its `cabinet_ref` ([Cabinet contract](docs/architecture/cabinet-contract.md)). Use `mission_ref` only where a record annotates "relates to a Cabinet Mission", never for Bear Domains.
- `core/` is canonical shared Bear memory. Role branches (`talk/`, `pair/`, `curate/`, `work/`, `watch/`) are role-local memory.
- Letta Archives are derived semantic retrieval indexes over canonical sources, not the source of truth. Do not introduce a Bear Den vector store while Letta Archives satisfy retrieval needs.

## Channels and Armatures

- A **channel** carries conversation between humans and Bears. Examples: Slack, WhatsApp, web chat, macOS app chat.
- An **armature** gives a Bear a trusted work-surface harness. Examples: ACP/Zed, future editor integrations, local CLI/TUI with workspace tools.
- BearWire remains armature-first. Channel adapters may reuse Den run services and may later use BearWire if they run out of process, but simple channels should not be forced to pretend to be ACP armatures.
- For channel planning, see `docs/roadmap/DEN_CHANNELS_IMPLEMENTATION_PLAN.md`.

## Tool Naming

- Model-facing provider names should be concise action names, not implementation-branded names. Prefer `session_info`, `memory_browse`, `memory_read`, `memory_search`, `memory_write_entry`, `web_fetch`, `web_search`, and `fs_edit_file`.
- Keep canonical internal names scoped and dotted, for example `den.session.info`, `den.memory.browse`, and `acp.fs.edit_file`.
- Tool names, provider aliases, permission classes, adapter/client methods, and UI labels should be descriptor-owned. Do not add scattered alias `match` arms or hardcoded allowlists when a descriptor resolver can be used.
- Provider names are not enough to decide execution location. Use descriptor metadata/resolvers to determine whether a tool is Den-hosted, armature-local, forwarded MCP, or future channel-local.
- Legacy aliases may be accepted at routing boundaries, but do not advertise legacy names such as `situation_get`, `memory_tree`, `fs_replace_text`, or `den_*` provider names to models.

## BearWire, ACP, and Tool Routing

- BearWire is the Den ↔ armature wire. ACP/Zed is an armature: it provides a trusted local work-surface harness with editor/workspace tools, permission UX, and session state.
- Channels such as Slack, WhatsApp, web chat, and macOS app chat are conversation surfaces, not armatures by default. They should not inherit ACP/local-workspace assumptions unless they explicitly expose a trusted work-surface tool boundary.
- Keep Pair/ACP tool surfaces stable across turns. Do not hide filesystem, git, terminal, MCP, or Den-hosted tools based on prompt heuristics; that causes models to learn false per-turn capabilities.
- Tool ownership must be descriptor-owned:
  - Den-hosted tools (`session_info`, `memory_*`, `web_fetch`, `web_search`, `list_task_lists`, `get_task_list_status`, `update_task_list`, `request_task_list_handoff`, etc.) execute inside Den.
  - Armature-local tools (`fs_*`, `git_*`, `terminal_run_command`, `process_run`, forwarded MCP tools) execute through the armature/client.
  - Do not route Den-hosted tools to `bear-armature` for local execution.
- If adding, renaming, or aliasing tools, update descriptors/resolvers first. Avoid scattered string `match` arms or hardcoded allowlists except at narrow routing boundaries.

## Single Source of Truth for State

- Model each piece of state once, with one canonical owner. Other layers may derive, project, or cache it, but must not keep independently writable copies that can drift; update the canonical state and regenerate derived views.

## Typed Boundaries and String Hygiene

- Avoid "stringy" protocol designs. Do not embed control data in transcript text with XML/Markdown/JSON blocks or sentinel strings; use typed BearWire/ACP events, message parts, or explicit fields instead.
- Prefer typed Rust values and enums over raw `String`/`&str` for distinct concepts such as model handles, provider model IDs, session IDs, conversation IDs, run IDs, tool call IDs, approval IDs, modes, statuses, permission classes, and error kinds.
- Keep SQL static and parameterized. Do not assemble SQL from runtime strings except through tightly controlled, whitelisted identifiers or query-builder APIs.
- Parse JSON/tool arguments/config/env once at the boundary into typed structs; avoid passing `serde_json::Value` or raw strings deep into business logic.
- Do not classify routing, permissions, ownership, or errors by substring/prefix matching rendered strings. Use descriptors, resolvers, structured error types, and explicit metadata.

## Memory and Reflection

- `pair` is API-direct and uses Den-hosted memory tools. `memory_write_entry` writes pair-local entries; `memory_request_review` asks Reflection/`curate` to review role-local memory.
- `pair` can learn things useful to `work`, but `work` must not read raw `pair/`. The intended path is `pair/` → pair reflection/review request → `curate` → `core`/archive/Cabinet/task context → `work`.
- Human identity for ACP `pair` comes from the ACP token. Use `session_info.human` as trusted identity; do not infer the human from chat text when it conflicts with Den identity.
- `curate` owns cross-role memory curation and `core/` cleanliness. Human UI should make its activity visible and overrideable, not require approval for routine inner-loop memory work.
- Write topology (ADR-0031 amendment): exactly one `MemoryStoreManager` per process. Production code receives clones of the instance built at server startup (threaded via `DenState`/runtime context) — never call `MemoryStoreManager::new` outside the sanctioned sites (startup, short-lived CLIs, tests). CI enforces this via `scripts/check-memory-write-topology.sh`.

## Conversation History and Transcript Projection

- Canonical conversation storage is the source of truth for user/assistant transcript replay.
- Avoid raw `message_type` / `role` filtering in runtime paths. Use shared projection helpers for model transcript vs user-visible history.
- Model replay and UI history are different projections:
  - model transcript may include rows hidden from user history when appropriate;
  - user-visible history should not include diagnostic-only rows.
- When fixing history bugs, verify both persistence and next-turn LLM request construction.

## Worktree safety (mandatory)

Mass file deletions have recurred when agent sessions repair a broken tree incorrectly.

**Never do this when files are missing on disk:**
- `git checkout <commit> -- <some/paths>` — partial restore leaves ~150+ tracked files deleted
- `git checkout HEAD -- <a/few/paths>` — same problem
- `git add -A` / `git commit` while `git status` shows many ` D` entries
- Bulk "remove deprecated Letta/native files" commits without an explicit user file list

**Always do this instead:**
- If `git status` shows more than a handful of ` D` (deleted) entries: run **`git checkout -- .`** or **`git restore .`** first — full restore only
- Stage only the files you intentionally changed; never `git add -A` after a mass-deletion glitch
- Run `./scripts/guard-worktree.sh` at session start if unsure

Repo guards (keep enabled):
- `.cursor/hooks.json` — blocks partial `git checkout` when deletions are already present; auto-restores on session start
- `scripts/git-hooks/pre-commit` — rejects commits deleting more than 10 files (install: `./scripts/install-git-hooks.sh`)

## Documentation ownership

- Start at [`docs/README.md`](docs/README.md) and locate the topic home before adding a document. Prefer updating the maintained topic, guide, or linked plan to creating another overview.
- Treat **current behavior** (verified against code/tests/deployment), **intended change** (linked active plan), and **decision rationale** (ADR) as different claims. Do not present a proposal or accepted ADR as deployed behavior; completing a plan requires checking its exit criteria.
- If a change affects a documented contract or capability, update the topic's current claim and affected plan/public description in the same change, or explicitly explain why no documentation change is needed. New topics/plans/ADRs must be linked from their indexes.
- Run `python3 scripts/check-docs.py --base HEAD` for documentation edits. CI checks new links and new-document structure; reviewers verify the truth of implementation claims.

## Notes

- Do not run `docker compose down`; restart individual services instead.
- Modify `docker-compose.yaml` only after explicit user approval.
- Environment variables are managed via `.env`; do not hardcode values.
- Keep deployment compatible with a single root `docker-compose.yaml`.
