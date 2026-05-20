# BEARS documentation

Human-oriented docs for the **Basic Environment for Agents Runtime Server (BEARS)** stack: Coolify deploy configs, planning, and architecture.

**Start here:** [README.md](../README.md) at the repository root (short overview). **Full doc map:** this page. **Agent/repo conventions:** [AGENTS.md](../AGENTS.md).

## Layout

| Path | Contents |
|------|----------|
| [../services/codepool/](../services/codepool/) | **Codepool** — Letta Code SDK harness (warm pool, Den streaming, optional channel listeners) |
[concepts/](concepts/) | Durable cross-functional concepts and vocabulary: [Bears and Den](concepts/BEARS_AND_DEN.md), [Bear agent roles](concepts/BEAR_AGENT_ROLES.md), [Bear Charter and Cabinet Missions](concepts/BEAR_CHARTER_AND_CABINET_MISSIONS.md), [agent and bear environments](concepts/AGENT_AND_BEAR_ENVIRONMENTS.md), [memory model](concepts/MEMORY_MODEL.md), [planning](concepts/PLANNING.md), [reflection system](concepts/REFLECTION_SYSTEM.md), [reflection run taxonomy](concepts/REFLECTION_RUN_TAXONOMY.md), [tasks and autonomy](concepts/TASKS_AND_AUTONOMY.md), [capabilities and skills](concepts/CAPABILITIES_AND_SKILLS.md), [observations and subscriptions](concepts/OBSERVATIONS_AND_SUBSCRIPTIONS.md), [identity and membership](concepts/IDENTITY_AND_MEMBERSHIP.md) including conversation/thread/session guidance |
| [context/](context/) | Short cross-cutting context summaries for operators and implementers, including [semantic memory](context/SEMANTIC_MEMORY.md). |
| [planning/](planning/) | Roadmap and implementation sequencing: [PLAN](planning/PLAN.md), [Phase 1 bootstrap](planning/PHASE1_BOOTSTRAP.md), [locked Phase 1 decisions](planning/PHASE1_DECISIONS.md), [**Multi-agent bear** rollout + **doc/UI** for bear ↔ Letta roles](planning/MULTI_AGENT_IMPLEMENTATION_PLAN.md) (includes Phase 8.5: retire implicit 1:1 copy), [`bear_channel` Phase 7+](planning/BEAR_CHANNEL_PLANS.md), [memory tools](planning/MEMORY_TOOLS_IMPLEMENTATION_PLAN.md). For memory automation and Reflection, use [memory automation roadmap](planning/MEMORY_AUTOMATION_ROADMAP.md) as the canonical implementation tracker; [reflection system](planning/REFLECTION_SYSTEM_PLAN.md) is shared infrastructure design, [curate memory governance](planning/CURATE_MEMORY_GOVERNANCE_PLAN.md) is memory-governance design, and [pair reflection and work memory](planning/PAIR_REFLECTION_AND_WORK_MEMORY_PLAN.md) is pair→curate→work boundary design. ACP discovery notes live alongside these. |
| [deployment/](deployment/) | Coolify deployment order and steps |
| [architecture/](architecture/) | System architecture and contracts: stack notes, Den + multi-user Letta, [`bear_channel` + ACP](architecture/BEAR_CHANNEL_AND_ACP.md), MemFS and memory UI, Letta vs bear UI coverage, Den meta tools |
| [architecture/adr/](architecture/adr/) | Architecture Decision Records (ADRs) for cross-cutting product/architecture choices: artifacts/Garage, Cabinet ingestion, dynamic skills/subagents, multi-user memory, semantic bear memory, reflection, routines. Note: [Bear work surfaces for planning and work activity](architecture/adr/bear-workplaces.md) defines the durable **work surface** model for plans, tasks, artifacts, memory, and work activity. |

Service-specific runbooks stay next to their configs: `services/*/COOLIFY_DEPLOY.md` and related `.md` under each service tree.

**Den (Rust)** lives in **`services/den/`**. **Codepool** (Node / Letta Code SDK harness) lives in **`services/codepool/`**. The Python MemFS Manager service lives in **`services/memfs-manager/`**. Supporting service assets live alongside them under `services/`.

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
