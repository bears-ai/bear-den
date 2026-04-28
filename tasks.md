# Tasks

## Devcontainer Startup

The devcontainer runs `/workspace/scripts/devcontainer-start.sh` on startup. It starts bundled Postgres, applies the Den `smoke` seed profile, then attempts to start the rest of the BEARS stack.

Startup is intentionally non-fatal: if compose startup or seeding fails, the devcontainer still opens. Check:

- `.devcontainer/logs/startup.status` for `ok`, `postgres_start_failed`, `postgres_unready`, `seed_failed`, `stack_failed_after_seed`, or `seed_and_stack_failed`
- `.devcontainer/logs/startup.log` for full command output

Rerun the seed manually:

```bash
./scripts/seed-dev.sh smoke
```

Run smoke tests after the stack is started and seeded:

```bash
./scripts/smoke.sh
```
