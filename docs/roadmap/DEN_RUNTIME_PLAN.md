# Den Runtime — Implementation Plan

**Status:** Active. This is the canonical migration plan for removing Letta and running a single Den-native, in-process agent runtime.

**Architecture source of truth:** [`../architecture/den-runtime.md`](../architecture/den-runtime.md).

**Runtime state inventory:** [`../architecture/den-state-machine-inventory.md`](../architecture/den-state-machine-inventory.md). Runtime work that changes conversation/session/turn/run/focus/obligation state must keep the inventory current and add or update a small executable invariant/replay/projection check for behavior-affecting changes.

**Prompt source extraction plan:** [`PROMPT_FRAGMENT_REGISTRY_IMPLEMENTATION_PLAN.md`](PROMPT_FRAGMENT_REGISTRY_IMPLEMENTATION_PLAN.md) (ADR-0046).

This plan supersedes the Letta-backed runtime direction in older roadmap docs (Phase 1 Letta/Codepool stack, ACP-as-Letta-conversation, MemFS-canonical memory, harness-backed vs API-direct split). See [the architecture doc](../architecture/den-runtime.md#what-this-supersedes) for the full supersession list.

## Goal

Replace Letta entirely with a single Den-native, in-process agent runtime: a streaming tool-calling loop that talks directly to Bifrost, stores all Bear memory/cognition in per-Bear SQLite ([ADR-0031](../decisions/adr-0031-sqlite-first-canonical-store-for-bear-agent-memory-and-tasks.md), retiring the git MemFS sidecar), reuses Den's existing orchestration/persistence, unifies all roles under capability stances, and deletes the control-plane/execution-process split (Letta server, Letta Code SDK, Codepool) along with its concurrency artifacts.

## Why the previous plan was wrong

The previous direction converged on a trait seam (`RuntimeTurnBackend` / `RuntimeCancellationBackend` / `RuntimeConversationBackend`) whose only implementation was Letta — a faithful re-model of Letta's HTTP process boundary. Finishing it would enshrine "Den control plane + Letta execution process," exactly the vestigial split we want gone. The replacement is smaller than it looks because Den already owns most of a runtime (canonical transcript, tool execution, tool-turn coordinator, in-process cancellation registry, turn phase machine, SSE orchestration, prompt assembly). Letta's remaining job is narrow: call the LLM, drive the tool-step loop, hold run/conversation state, and emit approval/stop semantics — all of which Den can own.

## Storage boundary (cognition vs control plane)

- **Per-Bear SQLite** (cognition, canonical): memory records/links, references, memory proposals, watch observations, promotion/curate decisions + audit, and reflection-run **outcome** records.
- **Den Postgres** (control plane): conversations/transcript, approvals, role-runtime registry, Docket jobs/tasks ([ADR-0034](../decisions/adr-0034-jobs-and-tasks-work-management.md)), and the reflection **scheduler/queue**.
- **Reflection-run split:** the scheduler/queue stays in Postgres; the canonical run record + outcomes live in SQLite next to `memory_promotions`. The queue references the SQLite run id; the only cross-store link is a transient id pointer.

See the [storage boundary section](../architecture/den-runtime.md#storage-boundary-bear-cognition-vs-den-control-plane) for detail.

## Loop strategies (composable policies, not a pattern zoo)

One step primitive; reasoning patterns are a thin, data-driven strategy policy (`plan?`/`reflect_on_fail?`/`critique?`/`fanout_n`) selected by the [ADR-0033](../decisions/adr-0033-model-tasks-layer.md) model-tasks policy layer. Plan&Solve is realized by Docket, Reflexion by Docket acceptance criteria + SQLite reflection, fan-out by Docket child runs / subagents. LATS tree search and LLM Compiler DAG engines are deferred. See [Loop strategies](../architecture/den-runtime.md#loop-strategies).

## Phases

### Phase 1 — Native Bifrost inference client

- New `core/llm/` (OpenAI-compatible): streaming `chat/completions` with tool-calling, targeting `LLM_API_URL` (default `http://bears-bifrost:8080/v1`).
- Add `llm_api_url` + default model handling to `config.rs` (currently `LLM_API_URL` is read only for `/status` warnings).
- Stream parser emits the existing semantic-event contract directly (drop the Letta JSON intermediate).
- Tests with recorded Bifrost SSE (text deltas, tool-call deltas, finish reasons).

### Phase 2 — Per-Bear SQLite canonical memory (ADR-0031)

Foundational; parallelizable with Phase 1. Replaces the git MemFS sidecar so the loop in Phase 3 is built on SQLite, not MemFS.

- New `core/memory/store/` backed by `sqlx` SQLite. Schema per ADR-0031: `memory_records`, `memory_links`, `memory_promotions`, plus a Bear-wide monotonic sequence allocator.
- Operational defaults: WAL, `synchronous=NORMAL`, `busy_timeout=5000`; single logical write path (`SqlitePool`, `max_connections(1)`); define per-Bear DB file lifecycle/placement.
- **Logical-path projection**: map logical paths to (`scope_type`, stance scope — currently the compatibility column `scope_profile`, `work_surface_ref`, `kind`) so `memory_browse`/`memory_read` keep the stable-anchor UX over rows.
- Migrate Den-hosted memory tools off MemFS to the SQLite store (`memory_write`, `memory_read`, `memory_review`, `memfs` retire/replace), routed through the memory manager to SQLite instead of the `/v1/git` MemFS API.
- Move all Bear cognition into per-Bear SQLite: `bear_memory_proposals`, `bear_observations`, and curate/promotion decisions + audit migrate from Den Postgres; `core/` promotion writes `memory_records`/`memory_promotions`.
- **Reflection-run boundary (split):** scheduler/queue stays in Den Postgres; the run record + outcomes move to SQLite next to `memory_promotions`; the queue references the SQLite run id.
- Transition note retired: no production Letta-runtime Bears require MemFS migration. Commit the `.sqlx` offline cache so CI compile-time query checks pass.

### Phase 3 — Native agent loop (ReAct stepper)

- New `core/agent_loop/`: prompt -> Bifrost stream -> assistant text + tool calls -> execute tools -> append results -> loop until `stop`/`max_steps`/cancel.
- Factor the loop so the step primitive is reusable and the run is parameterized by a `strategy_profile`; v1 ships the plain ReAct profile (all knobs off) plus the seam to read a profile. The policy selector and optional passes land in Phase 7.
- Context assembler builds model input from **`bear_compiled_configs` system prompt** + **key memory projection** (SQLite; [v1 policy locked](../architecture/den-runtime.md#v1-selection-policy-locked)) + Den transcript + prompt-memory blocks + compaction — fully Den-owned. See [Turn context assembly](../architecture/den-runtime.md#turn-context-assembly).
- Prompt source extraction is a parallel workstream, not a turn-hot-path redesign: repository-authored prompt fragments and runtime-authored compile-time prompt content both compile into `bear_compiled_configs`; the loop must continue to consume compiled prompt output only. See [ADR-0046](../decisions/adr-0046-file-backed-prompt-fragments-and-compiled-runtime-prompts.md) and the [Prompt Fragment Registry plan](PROMPT_FRAGMENT_REGISTRY_IMPLEMENTATION_PLAN.md).
- Persist each step to canonical transcript; Den-native approvals store (new table) + pause/resume integrated with the tool-turn coordinator.

### Phase 4 — Wire native loop under existing ACP orchestration for `pair` — Closed

- `AcpRuntimeSseStream` now consumes the Den-native `pair` loop while keeping ACP edge orchestration intact: tool-turn coordinator, turn controller, adapter SSE mapping, cancellation, and canonical persistence.
- Pair prompt/continuation dispatch goes through the native runtime entry points (`start_native_acp_turn_event_stream`, `continue_native_acp_turn_event_stream`).
- Golden ACP trace coverage validates OpenAI/Bifrost-style SSE → semantic events → Bearwire projection → adapter SSE (`bearwire_projection/golden_traces_tests.rs`).

### Phase 5 — Collapse the seam and shed Letta-isms — Closed

- Native `den-conv-*` conversations are persisted in Postgres and canonical visible history is loaded from Den storage.
- Prompt resolution skips re-materialization when a session already has a durable `den-conv-*` id.
- `RuntimeTurnBackend` / `LettaRuntimeTurnBackend` / `DenRuntimeAcpTurnRunner` are gone; ACP turn dispatch is an edge wrapper over native runtime functions.
- Dead `LettaAcpConversationRuntime` and stale Letta runner cleanup paths are removed. Stale runtime cleanup is now in-process (`run_ids` empty) and no longer issues external agent-wide cancels under native runtime.
- Native continuation, stream diagnostics, and pair turn comments use runtime/native terminology instead of Letta-facing labels.
- `start_native_turn_event_stream` takes the Den-verified execution origin for BearWire turns; `continue_native_profile_turn_event_stream` remains a compatibility continuation entry point over the same Den loop.
- Native curate briefing is wired: rule-based `memory_curate_executor` runs first; when briefing items remain, `run_native_curate_briefing_collect_assistant_text` runs an internal Curate LLM turn and projects assistant text into the memory_curate conversation (`NATIVE_CURATE_LLM_BRIEFING=0` disables).
- Letta/Codepool/MemFS runtime removal is complete in production. Remaining references are documentation/schema/UI naming cleanup, not runtime migration work.

### Phase 6 — Den-native stance registry (replace provisioning) — Closed

- Each operating stance is a Den-owned runtime stance: compiled system prompt, model choice, descriptor-owned tool roster, memory scope, and `den-native:{bear_id}:{stance}` binding id.
- Native reconciliation (`reconcile_bear_native` / `provision_missing_bear_profiles_native`) refreshes all five stance bindings from compiled prompts + `config_hash` without Letta HTTP or external agent creation.
- The Den-native stance registry is wired into pair binding resolution; stance config hashes use native runtime-family labels and omit Letta tool rosters.
- Active admin create/edit/import/API paths ignore legacy Letta agent/tool fields and write native-compatible empty legacy values.
- Operator UI/routes and provisioning APIs advertise **stance** vocabulary for the five operating stances; membership **roles** (`user_bear.role`) remain unchanged.
- Deprecated `bear_profile_bindings.letta_agent_id` and residual Letta/MemFS naming are cleanup debt only, not migration blockers.

### Phase 7 — Native coding harness (replace Codepool + Letta Code) for `work`

> **Status (2026-07): implemented**, with a different shape than sketched
> below (the bullets are the original design, kept for the record).
> Canonical internals: [work-sandbox internals](../guides/work-sandbox-internals.md);
> remaining items: [`SANDBOX_IMPROVEMENTS_ROADMAP.md`](SANDBOX_IMPROVEMENTS_ROADMAP.md).
> Design → reality:
>
> - `bears-sandbox-runner` → the **RUN_SANDBOX provider** (`den-sandbox`
>   crate) shipped as `bears-sandbox-provider` + `bears-sandbox-engine`
>   (dind) in the **default** compose stack
>   ([`SANDBOX_COMPOSE_SERVICE_PLAN.md`](SANDBOX_COMPOSE_SERVICE_PLAN.md)).
> - Runner-RPC coding tools → a headless **`bear-armature`** inside each
>   sandbox drives the native loop over BearWire; egress is restricted to
>   the Den callback by default.
> - Durable workspace records → durable **run** rows (`bear_work_runs`);
>   sandboxes are ephemeral and successful runs **publish to upstream
>   `den/job-*` branches** instead of archiving workspaces.
> - Den-managed remote origins → **managed work surfaces**
>   (`/work/surfaces`: user-owned, bear-assigned, encrypted credentials)
>   plus the admin-managed image catalog (`/admin/sandbox`).
> - Still open from this sketch: a work canvas beyond the `/work` pages,
>   first-class workflow actions, strategy policy, and PR/forge handoff.

**Design:** [ADR-0037](../decisions/adr-0037-work-sandbox-egress-gateway-and-upstream-auth.md).

- Add **`bears-sandbox-runner`** compose service: **`docker_workspace`** backend only (v1); paired **workspace + egress gateway**; **git/gh command bridge** at gateway (defense-in-depth; credential boundary is primary); Den owns policy, runner owns materialized workspaces; **telemetry** to gate warm pool and second backend choice.
- **Durable workspaces from v1:** introduce Den-owned workspace records for each `work` run/work surface, separate from individual model turns. Track workspace id, Docket run id, origin/work surface, base ref, branch, sandbox backend, materialized path/ref, status, archive state, and recovery/cleanup state. A workspace may eventually host multiple sessions/turn purposes (investigate, implement, review, recap), even if v1 runs one active `work` loop.
- **Visible branch/worktree lifecycle:** make branch/workspace identity product-visible rather than hiding it inside the runner: base ref, working branch, dirty state, changed files, commits, PR association, last command/check, and review-ready/completed status.
- **Work canvas / run dashboard:** ship a minimal status surface for active and archived workspaces: queued/planning/running/waiting-for-input-or-approval/review-ready/completed/failed, last activity, diff summary, command/check status, PR link, and Docket task linkage.
- **Workflow actions, not only free-form shell:** add first-class `work` actions with prompt/model/tool defaults and Docket artifacts: investigate, implement, run checks, summarize diff, draft PR, address review comments, resolve conflicts, and handoff/recap summary.
- **Output shaping as a tool contract:** fs/shell tools store full raw logs as artifacts but send bounded summaries/excerpts to the model (`summary`, `exit_code`, stdout/stderr excerpts, `artifact_ref`, `truncated`) to control context and cost.
- **Archive, recovery, and recap lifecycle:** every nontrivial workspace can produce a durable recap artifact; completed workspaces can be archived/restored; orphaned/dirty/stale sandboxes have explicit recovery and cleanup paths.
- **Pluggable `SandboxBackend`:** second isolation technology (Incus, VM, …) is a **candidate**, not committed — chosen post-v1 from telemetry.
- **Arbitrary repos:** Den-managed remote origins + configurable **clone depth**; opportunistic toolchain detection — **no required `mise.toml`** or repo scaffold.
- **`chat` has no sandbox** — delegate to a Docket **`work`** run with phase SSE; **`pair` stays client-armature** (hosted pair → Phase 7.1).
- Den **bear-level origins** UI/API; **multiple bear service identities** per `(provider, org_scope)`; **Connections** with `owner ∈ {user, bear}`; **`RunAuthContext`** with interactive (requester PR) vs autonomous (bear draft PR) paths.
- Reuse the native loop; add coding tools (fs, shell) via runner RPC; route `work` memory through per-Bear SQLite, retiring the Letta Code git memory path.
- Tasks for `work` come from **Docket** (ADR-0034): Den dispatches to the native `work` loop in the Bear's scoped memory context (execution invariant). **Auto-sweeps from task sources** (periodic issue/security/Linear intake with filters and run limits) are useful but deferred until after the v1 workspace lifecycle is stable.
- Land minimal **strategy policy** (`reflect_on_fail?`, `fanout_n`; `plan?` deferred).
- **Linked projects / linked work surfaces:** read-only cross-repo context is explicitly deferred. V1 writes only to the active workspace and reads only configured task/work-surface context plus Bear memory.
- Replace Codepool transport with native loop SSE; no warm pool in v1.

### Phase 8 — Retired

- No production Bears use the old Letta runtime, so one-time Letta conversation/MemFS backfill is no longer required.
- Keep `den import-legacy-memory` as optional historical/operator tooling for ad hoc archived bundles, not an active release gate.
- Transitional Letta/MemFS naming cleanup is no longer tracked as a Phase 8 milestone; handle it as ordinary cleanup when touching affected schema/UI surfaces.

## Risks and sequencing notes

- **Tool-calling fidelity:** provider/Bifrost tool-call streaming differs from Letta's framing; Phase 1 parser + Phase 4 golden traces de-risk this.
- **Context/compaction parity:** Den must reproduce in-context management Letta did implicitly (Phase 3 context assembler), including **`bear_compiled_configs`**, **key memory projection**, and **derived recall** ([ADR-0038](../decisions/adr-0038-platform-embedding-standard-and-derived-recall-index.md)) — see [Turn context assembly](../architecture/den-runtime.md#turn-context-assembly).
- **Recall index (parallel track):** [Derived recall index plan](DERIVED_RECALL_INDEX_IMPLEMENTATION_PLAN.md) — Qdrant + `bears-embed-v1`; not blocking Phase 4 ACP wiring but required for Letta archival parity at scale.
- **Memory model shift (Phase 2):** moving from a markdown file tree to append-only SQLite records is the biggest conceptual change. The logical-path projection must preserve the stable-anchor UX prompts depend on; no production MemFS backfill is required.
- **Cross-store discipline:** Den Postgres (control plane) and per-Bear SQLite (cognition) must not grow a sync seam; control plane references cognition by id only.
- **Harness (Phase 7):** landed as the RUN_SANDBOX system without blocking `pair` (Phases 1–5), as intended. Container isolation, run lifecycle, and publish shipped; status/canvas UX beyond `/work`, output shaping, and workflow actions remain follow-ups ([`SANDBOX_IMPROVEMENTS_ROADMAP.md`](SANDBOX_IMPROVEMENTS_ROADMAP.md)).
- Letta runtime and live MemFS are no longer supported runtime paths.

## Non-goals

- Derived recall is a **Qdrant derived index**, not canonical memory ([ADR-0038](../decisions/adr-0038-platform-embedding-standard-and-derived-recall-index.md)); SQLite remains canonical record storage.
- No pluggable multi-runtime abstraction retained "for optionality" — the native loop is the runtime.
- No bear-local task store: tasks/jobs stay Docket-canonical in Den Postgres (ADR-0034); SQLite stores memory/cognition only.
- No "agent-pattern" framework or pattern zoo: reasoning patterns are a small fixed set of composable strategy knobs (ADR-0033). LATS/LLM Compiler deferred.
