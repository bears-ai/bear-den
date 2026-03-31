# BEARS documentation

Human-oriented docs for the **Basic Environment for Agents Runtime Server (BEARS)** stack: Coolify deploy configs, planning, and architecture.

**Start here:** [README.md](../README.md) at the repository root (overview and doc map).

## Layout

| Path | Contents |
|------|----------|
| [planning/](planning/) | Roadmap, phased delivery, Phase 1 Den bootstrap, [locked Phase 1 decisions](planning/PHASE1_DECISIONS.md) |
| [deployment/](deployment/) | Coolify deployment order and steps |
| [architecture/](architecture/) | Stack notes, Den + multi-user Letta |

Service-specific runbooks stay next to their configs: `services/*/COOLIFY_DEPLOY.md` and related `.md` under each service tree.

**Den (Rust)** — application code lives at repo root in **`den/`** (not under `services/`). `services/` holds images, compose-oriented assets, and integration docs for Letta, LiteLLM, Open WebUI, etc.

## Cloning and automation

This repo is a **light monorepo**: documentation, `services/*` deploy artifacts, and (once present) the full Den codebase share one Git history.

- **Coolify and similar tools** usually clone the whole repository. That is fine for small and medium-sized trees. If you only need a subset (for example `services/litellm`), **shallow clone** keeps disk and network cost down:

  ```bash
  git clone --depth 1 <repo-url>
  ```

- **Sparse checkout** (optional) materializes only chosen paths after clone; use it when machines must not see `den/` or large paths at all:

  ```bash
  git clone --filter=blob:none <repo-url> bears-depoy
  cd bears-depoy
  git sparse-checkout init --cone
  git sparse-checkout set services/litellm docs README.md AGENTS.md
  ```

  Adjust the path list to match what that host needs. Den builds should include `den/` (and typically `docs/` for context). On older Git versions, run `git sparse-checkout init` before `set` if `set` does not enable sparse checkout automatically.

## Assistant-oriented material

Tooling notes for coding agents live at the repository root in **[AGENTS.md](../AGENTS.md)**. The [`.kilocode/memory_bank/`](../.kilocode/memory_bank/) directory is aligned with the same **bear** / Den vocabulary but is not end-user documentation.
