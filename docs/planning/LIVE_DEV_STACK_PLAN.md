# Live dev stack plan

## Summary

The default smoke stack should be GitOps-like: build immutable images from the current Git workspace, run those images without source/config bind mounts, seed explicitly, then smoke-test the resulting deployment shape. That mode answers: "would the artifact we just built run?"

We still want a separate live-development stack for fast iteration. That stack may bind-mount source trees, use file watchers, hot reload services, and allow host-local config overrides. It must be clearly opt-in and must not be confused with the smoke/prod-like deployment path.

## Current state

- `scripts/smoke-stack.sh` builds local Bifrost, Den, and Codepool images, starts bundled databases, seeds the `smoke` profile, starts the stack, and runs `scripts/smoke.sh`.
- `docker-compose.yaml` is the shared Compose file for local and Coolify-style deployment.
- Bifrost config is already baked into the Bifrost image by `services/bifrost/Dockerfile`.
- Preflight config validation now also bakes `services/bifrost/config.json` into the preflight image and no longer bind-mounts it from the host.

## Direction 1: GitOps-like smoke stack

This is the default and should remain the behavior of `scripts/smoke-stack.sh`.

### Goals

- Build deployable images from Git-tracked inputs.
- Run the built images without source-tree bind mounts.
- Keep runtime configuration in explicit environment variables, secrets, named volumes, or image-baked defaults.
- Avoid Docker Desktop host file-sharing requirements for smoke tests.
- Keep smoke tests useful as a local approximation of production deployment shape.

### Non-goals

- Hot reload.
- Editing files inside running containers.
- Mounting arbitrary local source paths into application services.

### Guardrails

1. Base `docker-compose.yaml` should avoid host bind mounts for app/config files in application and preflight services.
2. If a runtime file is required, prefer one of:
   - baked image content from Git-tracked source;
   - environment variables/secrets rendered inside the container;
   - named volumes for mutable service data.
3. Bundled databases are acceptable in smoke mode because they model local infra dependencies, but they should be explicit via the `bundled` profile and named volumes.
4. `scripts/smoke-stack.sh` should not require Docker Desktop file sharing for `/workspace` or other source paths.
5. Production and smoke should share as much Compose structure as practical, with production overriding images/secrets rather than changing service wiring.

### Follow-up tasks

- Audit `docker-compose.yaml` for any remaining app-service source bind mounts outside the `workspace` dev service and backup services.
- Add a CI or local check that fails if `scripts/smoke-stack.sh` depends on source bind mounts for app services.
- Consider publishing/prebuilding a preflight image if production should never build it on deploy.
- Document required smoke environment variables in a single `.env.smoke.example` or docs section.

## Direction 2: opt-in live dev stack

Live dev should be a separate mode, not the default smoke path.

### Goals

- Fast local iteration on Den, Codepool, Bifrost config, and supporting services.
- Optional source bind mounts and hot reload/watch commands.
- Clearly communicate that live dev is host-dependent and not production-like.

### Proposed shape

Use one or both of:

1. `docker-compose.live-dev.yaml` override file.
2. A `live-dev` Compose profile.

Suggested command:

```/dev/null/live-dev-command.sh#L1-3
COMPOSE_PROFILES=bundled,live-dev \
  docker compose -f docker-compose.yaml -f docker-compose.live-dev.yaml \
  up -d
```

### Candidate live-dev overrides

- Den:
  - bind-mount `services/den`;
  - run a dev command or rebuild-on-change workflow;
  - expose logs and debug ports if needed.
- Codepool:
  - bind-mount `services/codepool/src` and package files;
  - run `npm run dev` or a build/watch wrapper.
- Bifrost:
  - optionally bind-mount `services/bifrost/config.json` for rapid model/provider config iteration;
  - keep this override out of smoke/prod-like compose.
- Preflight:
  - either skip config preflight in live-dev while iterating, or provide a live-dev preflight override that intentionally bind-mounts config.

### Safety conventions

- Name the override `live-dev` rather than `dev` if possible, because `dev` is easy to confuse with smoke/local production-like testing.
- Add comments near every bind mount explaining that it is live-dev only.
- Keep live-dev environment defaults separate from production/smoke defaults.
- Do not use live-dev compose files in Coolify or production deployment docs.

### Follow-up tasks

1. Create `docker-compose.live-dev.yaml` with minimal Den/Codepool/Bifrost overrides.
2. Add `scripts/live-dev-stack.sh` to start the override stack.
3. Add `scripts/live-dev-down.sh` or document the shutdown command.
4. Document live-dev prerequisites, especially Docker Desktop file sharing paths on macOS.
5. Add a README section comparing:
   - smoke/prod-like stack;
   - live-dev stack;
   - devcontainer startup behavior.
6. Add a quick validation command for live-dev that confirms mounted source changes are visible inside containers.

## Acceptance criteria

- `scripts/smoke-stack.sh` can start the stack without any source/config bind mount requirement beyond Docker socket access from the devcontainer.
- The smoke path remains reproducible from Git-tracked inputs plus explicit environment/secrets.
- A documented live-dev plan exists for intentionally host-dependent bind mounts and hot-reload workflows.
- Developers can tell which stack mode they are using and why.
