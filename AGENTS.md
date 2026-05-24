# Agent Guide

## Stack

Three application services run via `docker-compose.yaml`:

- `bears-memfs-manager` is the Python service on port `8285`.
- `bears-den` is the Rust service on port `3000`.
- `bears-codepool` is the TypeScript service on port `3030`.

The workspace container has access to the Docker socket and can manage the stack.

Services are reachable by their compose service names over the internal Docker network, for example `http://bears-den:3000`. The root devcontainer startup script attaches the workspace container to `bears-stack_default` and exports dev defaults for `DATABASE_URL` and `LETTA_PG_URI`, so Den tests can resolve `bears-postgres` and `bears-letta-postgres` from inside the devcontainer.

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

## Project Concepts

- A Bear's **charter** is a descriptive property of the Bear: its durable purpose/responsibility boundary. Do not model `charter_id`, `charters[]`, or a separate Charter entity unless explicitly requested.
- Bear-scoped records should use `bear_id`. Bear-specific knowledge areas are **Domains** under the Bear, not Cabinet Missions.
- Cabinet **Missions** are shared work/knowledge containers with an n:n relationship to Bears. Use `mission_ref` only for Cabinet Missions.
- `core/` is canonical shared Bear memory. Role branches (`talk/`, `pair/`, `curate/`, `work/`, `watch/`) are role-local memory.
- Letta Archives are derived semantic retrieval indexes over canonical sources, not the source of truth. Do not introduce a Bear Den vector store while Letta Archives satisfy retrieval needs.

## Tool Naming

- Model-facing provider names should be concise action names, not implementation-branded names. Prefer `session_info`, `memory_browse`, `memory_read`, `memory_search`, `memory_write_entry`, `web_fetch`, `web_search`, and `fs_edit_file`.
- Keep canonical internal names scoped and dotted, for example `den.session.info`, `den.memory.browse`, and `acp.fs.edit_file`.
- Tool names, provider aliases, permission classes, adapter/client methods, and UI labels should be descriptor-owned. Do not add scattered alias `match` arms or hardcoded allowlists when a descriptor resolver can be used.
- Legacy aliases may be accepted at routing boundaries, but do not advertise legacy names such as `situation_get`, `memory_tree`, `fs_replace_text`, or `den_*` provider names to models.

## Memory and Reflection

- `pair` is API-direct and uses Den-hosted memory tools. `memory_write_entry` writes pair-local entries; `memory_request_review` asks Reflection/`curate` to review role-local memory.
- `pair` can learn things useful to `work`, but `work` must not read raw `pair/`. The intended path is `pair/` → pair reflection/review request → `curate` → `core`/archive/Cabinet/task context → `work`.
- Human identity for ACP `pair` comes from the ACP token. Use `session_info.human` as trusted identity; do not infer the human from chat text when it conflicts with Den identity.
- `curate` owns cross-role memory governance and `core/` cleanliness. Human UI should make its activity visible and overrideable, not require approval for routine inner-loop memory work.

## Notes

- Do not run `docker compose down`; restart individual services instead.
- Modify `docker-compose.yaml` only after explicit user approval.
- Environment variables are managed via `.env`; do not hardcode values.
- Keep deployment compatible with a single root `docker-compose.yaml`.
