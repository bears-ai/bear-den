# Agent Guide

## Stack

Three application services run via `docker-compose.yaml`:

- `bears-memfs-manager` is the Python service on port `8285`.
- `bears-den` is the Rust service on port `3000`.
- `bears-pool` is the TypeScript service on port `3030`.

The workspace container has access to the Docker socket and can manage the stack.

Services are reachable by their compose service names over the internal Docker network, for example `http://bears-den:3000`.

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

## Notes

- Do not run `docker compose down`; restart individual services instead.
- Do not modify `docker-compose.yaml` without confirming with the user.
- Environment variables are managed via `.env`; do not hardcode values.
- Keep deployment compatible with a single root `docker-compose.yaml`.
