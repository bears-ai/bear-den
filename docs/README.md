# BEARS documentation

Human-oriented docs for the **Basic Environment for Agents Runtime Server (BEARS)** stack: Coolify deploy configs, planning, and architecture.

**Start here:** [README.md](../README.md) at the repository root (short overview). **Full doc map:** this page. **Agent/repo conventions:** [AGENTS.md](../AGENTS.md).

## Layout

| Path | Contents |
|------|----------|
| [../services/codepool/](../services/codepool/) | **Codepool** — Letta Code SDK harness (warm pool, Den streaming, optional channel listeners) |
| [planning/](planning/) | Roadmap, phased delivery, Phase 1 Den bootstrap, [locked Phase 1 decisions](planning/PHASE1_DECISIONS.md) |
| [deployment/](deployment/) | Coolify deployment order and steps |
| [architecture/](architecture/) | Stack notes, Den + multi-user Letta; [`bear_channel` + ACP](architecture/BEAR_CHANNEL_AND_ACP.md); [MemFS and memory UI](architecture/MEMFS_AND_MEMORY_UI.md); [Letta vs bear UI coverage](architecture/LETTA_BEAR_UI_EXPOSURE.md); [Den meta tools](architecture/DEN_ARCHITECTURE.md#den-meta-tools-bears-control-plane-tools) (Den-defined control-plane tools and APIs; **Letta Code** brokers execution; not ad hoc scripts in Letta) |
| [dynamic-skills-subagents-adr.md](dynamic-skills-subagents-adr.md) | Dynamic skills (catalog + bear-authored), **reflection** subagents, predefined subagents in **bear** configuration; inspirational expert sketch (`skill-curator`, hooks) — Proposed |
| [routines-automation-adr.md](routines-automation-adr.md) | **Routines** (Phase 1): Den-managed schedules, bear-assigned; file outputs → Garage — Proposed |
| [artifacts-garage-adr.md](artifacts-garage-adr.md) | **Artifacts** in Garage (not Letta); human + agent provenance; GC; **Cabinet** separate bucket — Proposed |

Service-specific runbooks stay next to their configs: `services/*/COOLIFY_DEPLOY.md` and related `.md` under each service tree.

**Den (Rust)** lives in **`services/den/`**. **Codepool** (Node / Letta Code SDK harness) lives in **`services/codepool/`**. The Python MemFS Manager service lives in **`services/api/`**. Supporting service assets live alongside them under `services/`.

## Cloning and automation

This repo is a **light monorepo**: documentation, `services/*` deploy artifacts, and (once present) the full Den codebase share one Git history.

- **Coolify and similar tools** usually clone the whole repository. That is fine for small and medium-sized trees. If you only need a subset (for example `services/bifrost`), **shallow clone** keeps disk and network cost down:

  ```bash
  git clone --depth 1 <repo-url>
  ```

- **Sparse checkout** (optional) materializes only chosen paths after clone; use it when machines must not see `services/den/` or large paths at all:

  ```bash
  git clone --filter=blob:none <repo-url> bears-depoy
  cd bears-depoy
  git sparse-checkout init --cone
  git sparse-checkout set services/bifrost docs README.md AGENTS.md
  ```

  Adjust the path list to match what that host needs. Den builds should include `services/den/` (and typically `docs/` for context). On older Git versions, run `git sparse-checkout init` before `set` if `set` does not enable sparse checkout automatically.

## Assistant-oriented material

Tooling notes for coding agents live at the repository root in **[AGENTS.md](../AGENTS.md)**. The [`.kilocode/memory_bank/`](../.kilocode/memory_bank/) directory is aligned with the same **bear** / Den vocabulary but is not end-user documentation.
