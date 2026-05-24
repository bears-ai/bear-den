# Tasks

## Devcontainer Startup

The devcontainer runs `/workspace/scripts/devcontainer-start.sh` on startup. It builds local Bifrost, Den, and Codepool images, starts bundled Postgres services, applies the Den `smoke` seed profile, then attempts to start the rest of the BEARS stack with local source images.

Startup is intentionally non-fatal: if compose startup or seeding fails, the devcontainer still opens. Check:

- `.devcontainer/logs/startup.status` for `ok`, `local_image_build_failed`, `postgres_start_failed`, `postgres_unready`, `seed_failed`, `stack_failed_after_seed`, or `seed_and_stack_failed`
- `.devcontainer/logs/startup.log` for full command output

Rerun the seed manually:

```bash
./scripts/seed-dev.sh smoke
```

Run smoke tests after the stack is started and seeded:

```bash
./scripts/smoke.sh
```

Build local source images, start/recreate the stack, seed, and run smoke tests. This path should remain GitOps-like: built images plus explicit env/secrets, without app/config source bind mounts at runtime.

```bash
./scripts/smoke-stack.sh
```

Live bind-mount/hot-reload development is intentionally a separate future mode; see `docs/roadmap/LIVE_DEV_STACK_PLAN.md`.


