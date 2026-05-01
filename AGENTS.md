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

## Notes

- Do not run `docker compose down`; restart individual services instead.
- Modify `docker-compose.yaml` only after explicit user approval.
- Environment variables are managed via `.env`; do not hardcode values.
- Keep deployment compatible with a single root `docker-compose.yaml`.
