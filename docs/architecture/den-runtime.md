# Den Runtime

**Status:** Canonical runtime architecture.

This document describes the Den runtime, including current working-tree WIP. The [maintained hats topic](../topics/bear-memory-hats.md) owns source-admission and retirement claims: the implementation session reports passing workspace all-target checks and 121 root tests, but the latest Docker image has not been rebuilt. Mandatory real-hat admission applies even with zero hats; inference/results/continuations/direct dispatch recheck live actor/source/hat and exact Work eligibility. Old unbound history is owner/admin read-only; no ordinary Legacy memory/network/Cargo fallback or fabricated default/import/promotion remains. Hat action/resource filtering is still partial, not a complete `HatGrantResolver`.

The file path retains the historical `den-native-runtime.md` name for link compatibility, but the preferred architecture term is **Den runtime** or **in-process Den runtime**, not "native runtime".

It rests on these decisions:

- [ADR-0031 — SQLite-first canonical store for Bear memory](../decisions/adr-0031-sqlite-first-canonical-store-for-bear-agent-memory-and-tasks.md)
- [ADR-0033 — Model tasks layer](../decisions/adr-0033-model-tasks-layer.md)
- [ADR-0034 — Jobs and tasks work-management (Docket)](../decisions/adr-0034-jobs-and-tasks-work-management.md)
- [ADR-0037 — Work sandbox, egress gateway, and upstream auth](../decisions/adr-0037-work-sandbox-egress-gateway-and-upstream-auth.md) — Phase 7 execution isolation and multi-identity GitHub policy
- [ADR-0043 — ACP is an edge adapter; the Den runtime is protocol-agnostic](../decisions/adr-0043-acp-as-edge-adapter-protocol-agnostic-core.md) — the runtime below owns turns/sessions/events under neutral names; ACP is one edge
- [ADR-0046 — File-backed prompt fragments and compiled runtime prompts](../decisions/adr-0046-file-backed-prompt-fragments-and-compiled-runtime-prompts.md) — repository-authored prompt fragments + runtime-authored compile-time-only prompt content compiled into hot-path prompt bases

## Why this architecture exists

The earlier plan converged on a clean trait seam (`RuntimeTurnBackend` / `RuntimeCancellationBackend` / `RuntimeConversationBackend`) whose only implementation was Letta. That seam is a faithful re-model of Letta's HTTP **process boundary** — it enshrines "Den control plane + Letta execution process," a split that only ever existed because we built on Letta, an external project.

The target removes that split entirely. There is **no Letta server, no Letta Code SDK, no Codepool harness process, and no git-backed MemFS memory sidecar**. Den runs an in-process agent loop for verified ordinary sources, with dedicated source-verified internal inference paths, talks directly to Bifrost for inference, and stores all Bear memory/cognition in per-Bear SQLite. The runtime is no longer pluggable "for optionality": the in-process Den loop *is* the runtime.

## Guiding principles

- **In-process runtime, verified sources.** Origin/governance select capability ceilings; canonical actor/source/hat and exact Work run own admission. The removed Rust aliases leave derived runtime-context labels only, with historical schema/audit encoding, not configurable stances.
- **One loop primitive, patterns as policy.** The step (assemble context -> stream model -> execute tools -> persist) is the primitive. Reasoning "patterns" are a thin, data-driven **strategy policy** over it, not forked runtimes. See [Loop strategies](#loop-strategies).
- **A turn is a Tokio task owned by Den**, not an HTTP call to another service. Cancellation is a `CancellationToken`, not external run-ids.
- **Den owns conversation identity, message/context state, approvals, and compaction.** Pending IDE sessions materialize durable `den-conv-*` only after hat admission; edges cannot invent canonical sources or synthetic terminal outcomes.
- **Tool exchanges must be replayable transcript state.** Tool requests/results that affect model behavior are first-class model-history artifacts: stable tool-call id, canonical tool name, typed arguments, matching result/error, and bounded output/summary. ACP/BearWire/web projections may render them differently, but no edge cache may be the only record of what tool ran or what happened.
- **Non-blocking structured updates are not tool dependencies.** Conversation titles, advisory in-flight task status, and similar surface/control-plane updates can be persisted and projected without creating client obligations or forcing model continuation. The descriptor registry owns whether a model-facing action is a blocking tool, a client obligation, a non-blocking structured update, or ephemeral progress.
- **Bifrost is the inference substrate** (OpenAI-compatible), called directly by Den.
- **The loop is protocol-agnostic; ACP is an edge adapter** ([ADR-0043](../decisions/adr-0043-acp-as-edge-adapter-protocol-agnostic-core.md)). The turn controller, tool-turn coordinator, session machinery, and semantic event stream are core organs of this loop — they carry neutral names and live in the runtime, not behind any wire protocol. ACP (like REST and the web UI) is a sibling adapter that projects the canonical BearWire semantic events ([ADR-0029](../decisions/adr-0029-den-structured-runtime-events.md)/[ADR-0030](../decisions/adr-0030-bearwire-resource-oriented-event-model.md)) to/from its wire format. The current `acp_*` naming inside `den-runtime` is historical drift that ADR-0043 corrects, not evidence those organs belong to ACP.
- **Bear memory and cognition is canonical in per-Bear SQLite** (ADR-0031). The git MemFS sidecar is removed; git is retained only for human-authored artifacts.
- **Tasks/jobs are not Bear memory.** They are Docket-canonical in Den Postgres (ADR-0034). The bear/Den boundary is drawn at memory, not tasks.
- **Loop control separates supervision from objective.** A run's **governance** determines continuation pressure (`interactive`, `grace`, `autonomous_continuation`, `observational`, `frozen`). A run's **focused Job**, when present, is the Docket Job kept centered for next-task selection. `work` normally requires a focused Job; `pair` only designates one explicitly through Bear conversation or client command. See [ADR-0050](../decisions/adr-0050-agent-loop-control-adaptive-budgets-and-runtime-checkpoints.md).
- **Runtime state axes are documented and tested.** Conversation/session/turn/run state is inventoried in [Den state machine inventory](den-state-machine-inventory.md). Runtime changes that add or change state semantics must update that inventory and leave a small executable invariant, table-driven test, replay assertion, or projection test for behavior-affecting changes.
- **Authoritative state changes are inspectable semantic facts.** Major user-relevant control transitions are reduced by their Den-owned aggregate and recorded as persistent, replayable BearWire diagnostic events in the canonical session stream. They are control-plane transcript artifacts, not fabricated model-role messages. Den controls authorization and recording; clients decide locally whether a debug view renders them. Operational logs may correlate with these events but are not a competing execution authority.

## Runtime architecture

```mermaid
flowchart TB
  subgraph DenProc [bears-den process]
    GW[ACP + Web HTTP gateways]
    ORCH[Turn orchestrator: tool-turn coordinator, cancel registry, turn phase machine]
    LOOP[In-process Den loop: step primitive + strategy policy]
    CTX[Context assembler: compiled prompt + key memory projection + prompt-memory + compaction]
    MEM[Memory layer: sqlx, single-writer, sequence allocator]
    TOOLS[Tool executor]
    LLM[Bifrost streaming client]
    PG[(Den Postgres: conversations, messages, approvals, registry, bear_compiled_configs, prompt_memory_blocks, Docket jobs/tasks, reflection queue)]
    GW --> ORCH --> LOOP
    LOOP --> CTX
    LOOP --> LLM
    LOOP --> TOOLS
    LOOP --> PG
    CTX --> PG
    CTX --> MEM
    TOOLS --> MEM
  end
  subgraph Sandbox [Den-managed code sandboxes]
    SBX[ephemeral workspace containers for work/coding turns]
  end
  subgraph BearState [per-Bear canonical cognition]
    SQLITE[(per-Bear SQLite: memory_records, memory_links, memory_promotions, proposals, observations, reflection-run outcomes)]
  end
  GIT[(git: human-authored artifacts - skills, prompts, policies, schemas)]
  TOOLS -->|server tools in-process| TOOLS
  TOOLS -->|client/adapter tools| GW
  TOOLS -->|shell/fs for work| SBX
  LLM --> BIF[bears-bifrost /v1] --> PROV[providers]
  MEM --> SQLITE
  TOOLS -.read-only.-> GIT
```

## Concurrency model

- **Turn = a spawned async task.** At most one active turn per (bear, stance, channel), enforced by the existing tool-turn coordinator.
- **Cancellation = `CancellationToken` / `watch`.** Letta `run_ids` and `POST /messages/cancel` are dropped.
- **Tool calls and updates:** server tools run in-process; client/adapter tools become obligations awaited on a `oneshot`; shell/fs tools for `work` run in a Den-managed sandbox. Non-blocking structured updates are persisted/projected through runtime events rather than awaited as model-visible tool results.
- **Approvals:** a turn pauses awaiting a Den-stored decision and resumes the same in-process task. This replaces Letta's `requires_approval` stop_reason plus deny/cleanup recovery.
- **Streaming:** the loop yields semantic events directly into the existing SSE mapper; the Letta-SSE byte reparse is deleted. Post-tool/approval continuation liveness is Den-owned: before the first resumed runtime event, watchdogs must include the LLM handshake allowance; after events begin, shorter inter-event idle watchdogs apply.

## Storage boundary: Bear cognition vs Den control plane

This is the most important conceptual line in the target, and it is not "content vs records." Per-Bear SQLite already holds *operational* records (the promotion/review audit trail and the change-tracking sequence). The real boundary is:

- **Bear cognition -> per-Bear SQLite** (canonical, via `sqlx`): source-local + hat/shared knowledge and restricted historical profile-local records, references, memory proposals, watch observations, promotion/curate decisions and audit, and reflection-run **outcome** records. This is the durable record of what the Bear knows and how it decided to know it.
- **Control-plane infrastructure the Bear plugs into -> Den Postgres**: conversations/transcript, hat bindings/grants, approvals, historical registry rows, **Docket** jobs/tasks (ADR-0034), and the reflection **scheduler/queue**.

The metaphor (from ADR-0034): a Bear *uses* Den's schedulers and trackers the way a person uses a project tracker. The tracker is infrastructure, not part of the Bear.

### The reflection-run split

A reflection run has two natures, so it is split across the boundary rather than forced wholly into one store:

- The **scheduler/queue** (trigger, claim, status, timing; the global multi-Bear index) stays control-plane in **Den Postgres**.
- The **canonical run record + its outcomes** (proposals considered, curate decisions) live in **per-Bear SQLite**, next to `memory_promotions`, as one self-contained cognition graph (run -> proposals -> promotions).

The Postgres queue row references the SQLite run id; once a run completes, Postgres retains only ephemeral scheduling state. The only cross-store link is a single id pointer on a transient queue row — never an audit-graph seam.

**Cross-store discipline:** control plane references cognition by id only. There is no content sync seam between Postgres and SQLite.

## Turn context assembly

Every admitted ordinary turn builds **Turn Context** from its verified canonical source/hat and execution origin. The assembler is Den-owned end-to-end; there is no provider-side prompt or memory injection.

### Layer 1 — Compiled system prompt (`bear_compiled_configs`)

Managed compilation emits `bound_base` (Bear-wide baseline/steering) and repository-owned platform `bound_*_mode` components only, independently of legacy role contracts/metadata. Ordinary turn assembly selects the current canonical hat via `bound_hat_identity`; there is no no-hat stance prompt fallback. Dedicated Curate briefings use direct source-verified tool-free inference with repository Markdown instructions, not ordinary stance compilation. No production inference selects a role prompt or role contract. Worker briefings consume direct verified `MemorySource`; live source admission precedes every production inference/continuation and direct tools recheck it. Generic system starts/tools/results/continuations are denied.

Compilation merges:

- published baseline **`system_blocks`** (Den-global versioned prose; historical `space_instruction.*` contracts remain inspection data),
- per-Bear **`bear_block_bindings`** (`inherit` vs `custom` overrides),
- Bear-local bound-base fields (`user_steering`, `bear_context`), parsed without coupling to legacy contracts/metadata.

Under [ADR-0046](../decisions/adr-0046-file-backed-prompt-fragments-and-compiled-runtime-prompts.md), this layer evolves into a **hybrid prompt source model**:

- **repository-authored fragments** live in Git as Markdown + YAML frontmatter and are loaded into a startup prompt registry,
- **runtime-authored prompt content** (for example Bear Admin-authored text) remains data-backed in Postgres,
- both source classes are normalized and compiled into `bear_compiled_configs`.

The architecture and rollout details live in [prompt-fragment-registry.md](prompt-fragment-registry.md) and the [Prompt Fragment Registry implementation plan](../roadmap/PROMPT_FRAGMENT_REGISTRY_IMPLEMENTATION_PLAN.md).

`compile_and_store_managed_config_for_bear` stores bound base/mode hashes and prompt-source version; missing/stale components are regenerated before an admitted turn. Initialization ensures memory/runtime plan/managed config but never creates/refreshes a profile registry. `/models` is Bear-wide and profile model/loop rows are not live overrides. Defaults/control/compaction use verified origin and canonical source. The hat-identity component reads the current hat record per turn, so changing its authored text does not require copying or editing stance contracts.

**Target invariant:** the native agent loop **must** read managed Bear-wide and mode components from `bear_compiled_configs` (plus the current canonical hat identity for bound turns). It must **not** recompose prompts via `compose_role_context(..., resolved: None)`, which bypasses managed-block resolution and diverges from Letta-era behavior.

Additional invariant from ADR-0046: the turn hot path must not parse frontmatter, read prompt files from disk, or render runtime-authored database templates. Runtime-authored prompt content is compile-time-only; turn-time templating is reserved for explicitly approved repository-owned fragments.

An unmanaged Bear's Bear-wide base does not bypass real-hat admission or enable a legacy per-stance prompt lane.

Recompile triggers match provisioning today: bear create/update, managed-block binding changes, and reconcile when `context_profile` is present.

### Layer 2 — Key memory projection (SQLite)

Letta previously injected **persona/human-style blocks** every turn from provider-owned agent state. Under ADR-0031 that durable knowledge lives in **per-Bear SQLite**, but the model still needs a **bounded, proactive subset** in Turn Context — not the whole memory bank, and not only what tools retrieve mid-turn.

**Key memory projection** is Den’s deliberate selection of SQLite `memory_records` (and linked anchor summaries) to append after the compiled system prompt in a dedicated `# Projected memory` section, subject to a character/record budget.

This is distinct from:

| Mechanism | Purpose |
|-----------|------|
| **Compiled system prompt** | Bear-wide bound base/platform mode plus current verified hat identity; dedicated internal inference has its own narrow source |
| **Prompt memory blocks** | Ordinary reads select Bear-wide/exact-session blocks; mutations are session-confined; historical profile/work-surface blocks are not ordinary context ([prompt-memory contract](den-prompt-memory-block-contract.md)) |
| **Key memory projection** | Read-only proactive slice of **canonical SQLite memory** (path anchors) |
| **Derived recall** | Vector search over chunked passages ([ADR-0038](../decisions/adr-0038-platform-embedding-standard-and-derived-recall-index.md)); bounded turn-start + hybrid `memory_search` |
| **`memory_search` / `memory_read` tools** | On-demand retrieval; `memory_search` becomes hybrid when Qdrant is configured |

#### v1 selection policy (locked)

Projection follows the **work-surface-first** precedence in [`memory-model.md`](memory-model.md), stays within canonical own-source/selected-hat/core grants, and remains small enough for every turn. Implementation lives in the context assembler (`core/agent_loop/`), reading through the memory store manager — not ad hoc in gateways.

**Tiers** (ordered; stop when the global character budget is exhausted):

1. **Shared identity anchors** — latest-head `scope_type=shared` records at Bear-global anchor paths, in order: `core/bear-overview.md`, `core/bear-glossary.md`, `core/shared-conventions.md`. Include only `visibility=normal` in v1.
2. **Active work-surface anchors** — latest-head shared records at canonical surface paths for the primary work surface (see work-surface gating below): `core/work_surfaces/<slug>/index.md`, `overview.md`, `glossary.md`, `architecture.md`, `decisions.md`, `conventions.md`.
3. **Own-source and selected-hat highlights** — current canonically authorized records; historical profile-local rows never become a fallback. Derived recall hits are independently rechecked and reconstructed from SQLite.
4. **Situation/session briefing** — optional short trusted briefing records when modeled (not transcript); at most one record in v1.

**Explicitly excluded from proactive projection** (tools or curate review only):

- raw unpromoted proposals and pending observations,
- full stance branches (`pair/` raw history for `work`, etc.),
- promotion/audit graphs and reflection-run machinery,
- Docket/task state (Postgres control plane),
- conversation transcript (separate message list),
- superseded record bodies and per-path history chains.

#### v1 budgets

Budgets are character-selection heuristics, not authority. The following v1 profile-named caps describe historical projection tuning, not configurable stances or admission for internal/zero-hat sources; verified-origin policy owns current defaults/control:

| Profile | Global char cap |
|---------|-----------------|
| `pair`, `chat`, `work` | 8 000 |
| `curate` | 6 000 |
| `watch` | 4 000 |

| Tier | Max records | Per-record cap | Tier soft cap |
|------|-------------|----------------|---------------|
| 1 Shared identity | 4 | 1 500 | 3 000 |
| 2 Work-surface anchors | 6 | 1 200 | 3 500 |
| 3 Role-local highlights | 4 | 800 | 2 000 |
| 4 Situation briefing | 1 | 1 000 | 1 000 |

Assembly stops when the global cap is reached. Emit a `key_memory_projection` diagnostic (included paths/ids, omitted-by-budget, omitted-because-no-surface) alongside prompt-memory diagnostics where practical.

**Char caps are selection heuristics, not the budget authority.** The caps above bound what the projector *picks* — cheap and deterministic, with no tokenizer in the memory hot path. Authoritative token accounting happens once, on the final assembled request, under [ADR-0047](../decisions/adr-0047-context-window-budget-and-token-estimation.md): key memory projection and derived recall are named components in the budget report's per-component attribution, and Den calibrates chars→tokens per model family from Bifrost-observed prompt usage (correction ratios held in the model registry, per ADR-0047 §7). Char caps should be re-tuned from those measured ratios rather than treated as token estimates themselves.

#### v1 supersede policy

**Latest head only** — no short history in proactive projection.

For each `logical_path`, or when `logical_path` is null each `(scope_type, scope_role, work_surface_ref, kind)` group, include at most one row: the current head (highest `sequence_no` among rows not superseded by a newer row). Chained history remains tool-mediated via `memory_read` / `memory_search`.

#### v1 work-surface gating

Primary slug selection uses the same session signals as tools today (`work_surface_candidate_slug`: `runtime_target`, then `conversation_selection`, then `workspace_roots`).

| `session_info.work_surface.status` | Tier 2 behavior |
|-----------------------------------|-----------------|
| `unresolved`, `ambiguous` | **Skip** tier 2 |
| `candidate` | Include tier 2 **only if** SQLite has at least one canonical anchor for the candidate slug (`core/work_surfaces/<slug>/index.md` or `overview.md`) |
| `resolved`, `confirmed` | Include tier 2 for that slug (full tier 2 quota) |

**Anchor-required for candidates:** a normalized workspace slug alone is not enough; tier 2 requires proof in canonical memory. This avoids projecting the wrong surface from weak session hints.

**Deferred (v1.1):** when conversation-persisted `primary_work_surface` lands ([work-surface resolution plan](../roadmap/WORK_SURFACE_RESOLUTION_IMPLEMENTATION_PLAN.md)), projection prefers **conversation binding → session slug → workspace roots**, with the same anchor-required rule for `candidate`.

#### v1 rendering

Keep compiled Bear-wide/mode components hash-stable. Append authorized projection after the current verified hat identity and compiled base/mode; no ordinary legacy prompt lane exists:

```text
<compiled Bear-wide base + verified hat identity + platform mode>

# Projected memory
## Shared anchors
…
## Work surface: <slug>    (omit section if tier 2 skipped)
…
## Authorized own-source and selected-hat highlights
…
## Situation                  (omit if empty)
…
```

Layer 3 supplements (prompt memory blocks, compaction, channel reminders) follow this block — see [prompt-memory contract](den-prompt-memory-block-contract.md).

#### v1 caching

Cache projection **per agent-loop turn** (one user prompt, multiple tool steps). Reuse across ReAct steps 1…N within the same `AgentLoopSession`.

**Cache key:** `(bear_id, stance, conversation_id, primary_surface_slug | None, sqlite_sequence_high_water, compiled_config_hash)` where `sqlite_sequence_high_water` is `MAX(sequence_no)` at build time.

**Invalidate when:** a new human message starts a turn, `sequence_high_water` advances, the primary surface slug changes, or the compiled config hash changes. Do not cache across conversations or Den restarts in v1.

### Layer 3 — Runtime supplements (per turn)

After compiled prompt + key memory projection, the assembler appends **turn-local** Den-owned supplements when applicable:

- **ACP / channel runtime context** — plan mode, workboard, trusted-session mode, tool-surface reminders (today’s `<system-reminder>` envelope for `pair`),
- **prompt memory blocks** — Bear-wide/exact-current-session selection, with historical profile/work-surface blocks excluded,
- **compaction envelope** — Den-owned transcript bounding artifacts.

These supplements remain distinct from the compiled prompt base even after file-backed prompt extraction. Repository-owned prompt fragments may contribute narrowly-scoped turn-time templated supplements (for example date or budget reminders), but runtime-authored prompt content remains pre-turn compiled.

Order target:

```text
system:  [compiled Bear-wide base + verified hat identity + mode, or legacy stance prompt]
       + [key memory projection — path anchors]
       + [derived recall — optional vector passages]
       + [runtime supplements: prompt-memory, compaction, channel reminders]
messages: [canonical transcript, including replayable tool calls/results] + [current user/tool step]
tools:    [merged Den + client descriptors]
```

See also [`agent-and-bear-environments.md`](agent-and-bear-environments.md) (Environment Projection → Turn Context).

For scenario-oriented examples of how these layers combine, see [Context Compilation Scenarios](context-compilation-scenarios.md).

## Memory model under SQLite

Per ADR-0031, canonical memory is append-only records, not a markdown file tree:

- `memory_records` (append-only; `scope_type` `role_local`|`shared`, `scope_role`, `kind`, `entity_ref`, `content_text`, `supersedes_memory_id`, `visibility`), `memory_links`, `memory_promotions`, plus a Bear-wide monotonic **sequence allocator** for replay/export/"what changed since X".
- Operational defaults: `PRAGMA journal_mode=WAL`, `synchronous=NORMAL`, `busy_timeout=5000`; a single logical write path (dedicated `SqlitePool`, `max_connections(1)`).
- **Logical-path projection.** The Bear-facing memory model (the `core/` and stance-branch anchor tree from [`memory-model.md`](memory-model.md), e.g. `core/work_surfaces/<slug>/architecture.md`) is preserved as a projection: a logical path maps to (`scope_type`, `scope_role`, `work_surface_ref`, `kind`). `memory_browse`/`memory_read` keep their stable-anchor UX over rows instead of files. The file tree is a view, not the store.

### Git's remaining place

Git is retained **only** for human-authored artifacts: skills documentation, prompts, policies, schema definitions/migrations, design artifacts, tests/fixtures, and optionally exported curated summaries. It is no longer canonical for any live machine-written Bear memory.

### Semantic retrieval (derived recall)

Letta Archives and Letta pgvector are removed with Letta. Semantic recall is a **Den-owned derived index** — not a second canonical memory store.

- **Platform embedding standard:** versioned contract shared by Bear memory and Cabinet ([ADR-0038](../decisions/adr-0038-platform-embedding-standard-and-derived-recall-index.md)); initial id `bears-embed-v1` (`text-embedding-3-small`, 1536d via Bifrost).
- **Vector store:** Qdrant collections named per embedding standard; passage metadata in Den Postgres; vectors are disposable (rebuild from SQLite / Cabinet sources).
- **Complements key memory projection:** anchors = fixed logical paths; recall = fuzzy / cross-corpus passages when policy allows.
- **Eligibility and cleanup:** only eligible current core/hat records are indexed; source/profile-local records are never indexed, even with zero hats. Stale legacy derived-point cleanup is retried before embedding; canonical old SQLite is preserved. Profile-string recall APIs are removed.
- **Implementation:** [Derived recall index plan](../roadmap/DERIVED_RECALL_INDEX_IMPLEMENTATION_PLAN.md).

## Loop strategies

Most "agent patterns" (Plan & Solve, Reflexion, Reflection, REWOO, STORM, LATS, LLM Compiler) are compositions over a tool-calling step, not separate runtimes. ReAct is the substrate the others build on. We expose the high-value ones as a small fixed set of composable knobs over the single loop:

- `plan?` — decompose before executing. Largely already realized by **Docket**: a job's task tree *is* the plan, and `work` executes beads one turn each (ADR-0034). Interactive planning is pair plan-mode. No in-loop planner is needed for job work.
- `reflect_on_fail?` — Reflexion-style retry. Realized by Docket `command` acceptance criteria (e.g. `cargo test` exits 0): on failure, write a reflection note to per-Bear SQLite memory and re-dispatch the run. The episodic failure memory is the SQLite store.
- `critique?` — an optional post-step critique/revise pass (Reflection) for quality-sensitive turns.
- `fanout_n` — multi-perspective/exploration (STORM, best-of-N, LATS-lite) as **Docket child task runs / spawned subagent turns**, reusing the turn=task concurrency model and its observability — not an in-loop tree/DAG engine.

**Selection is data-driven** via the [ADR-0033](../decisions/adr-0033-model-tasks-layer.md) model-tasks policy layer, which already maps task `difficulty`/`effort_hint` to model + effort. The same mapping emits a `strategy_profile` keyed on signals already stored: job category, `task.kind` (`execution | investigation | decision`), difficulty, and whether `command` criteria exist. "Code job -> reflect-on-fail; research job -> fan-out" is a policy row, not branching runtimes.

**Explicitly deferred** (high complexity, narrow gain, very high call cost): true LATS tree search and LLM Compiler DAG engines. We do **not** build a pluggable "agent-pattern" framework; that would re-introduce the speculative abstraction this migration exists to delete.

## Runtime initialization and remaining cutover

Native initialization opens the Bear memory store, ensures its runtime plan, and compiles managed bound configuration; it never creates/refreshes a profile registry. Admin stance detail/configuration/provisioning, per-profile model, and profile-registration routes are gone. Bear-wide `/models` and canonical conversation selection replace live profile model/loop overrides; historical rows/contracts remain inspection data.

Removal of the `TrustProfile` / `BearProfile` / `BearStance` aliases leaves five-source-kind derived runtime-context metadata (`RuntimeContextLabel`), retaining historical schema/audit encoding only. Every ordinary source requires a real hat and live canonical admission; labels cannot recreate authority. Pair Plan/shared scaffold model tools are retired.

### Current gap (implementation)

Hat grant/action/resource filtering remains partial for supported Den web, exact-root editor filesystem reads, and upgraded sandbox egress. Broader Den effects/catalogs and other bounded persistent client choices are next under [Gate 0A](../roadmap/HATS_AND_SESSION_MEMORY_BOUNDARIES_PLAN.md#gate-0a--hat-owned-tool-and-network-permissions-partial-cutover-exit-pending). Latest-image rebuild/smoke, real model curation, and real provider Job/revocation evidence remain outstanding; see [the topic](../topics/bear-memory-hats.md).

## What this supersedes

- The "Den -> Letta Code -> Letta" and Codepool `bear_channel` runtime paths.
- The historical split between different stance runtime families.
- MemFS/git as canonical for live Bear memory, and "Letta-native memory only / no Den memory store."
- The MemFS file-based task pipeline (`chat/tasks` -> `core/tasks` -> `work/results`) for human-initiated work, replaced by Docket (ADR-0034).
- Decisions written against the Letta-backed model (notably ADR-0005, ADR-0013, ADR-0014, and the MemFS/Letta-Archives assumptions in ADR-0021/0022) are superseded by ADR-0031/0033/0034 on the points above.

## Related documents

- Migration plan and phasing: [`../roadmap/DEN_RUNTIME_PLAN.md`](../roadmap/DEN_RUNTIME_PLAN.md)
- Derived recall (Qdrant + embeddings): [ADR-0038](../decisions/adr-0038-platform-embedding-standard-and-derived-recall-index.md), [`../roadmap/DERIVED_RECALL_INDEX_IMPLEMENTATION_PLAN.md`](../roadmap/DERIVED_RECALL_INDEX_IMPLEMENTATION_PLAN.md)
- Bear package format (portable export/import): [`../guides/bear-package.md`](../guides/bear-package.md)
- Memory model (Bear-facing): [`memory-model.md`](memory-model.md)
- Historical Letta dependency inventory: [`letta-dependency-matrix.md`](letta-dependency-matrix.md)
- Historical backfill/rollback planning: [`../roadmap/den-migration-backfill-and-rollback-plan.md`](../roadmap/den-migration-backfill-and-rollback-plan.md) (production Phase 8 backfill retired)
