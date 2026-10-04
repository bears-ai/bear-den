# ADR-0039 — Trust profiles and governance states

**Status:** Accepted (2026-06-12)
**Deciders:** Hans
**Related:**
- [ADR-0036 — Bear profile registry and binding vocabulary](adr-0036-bear-profile-registry.md)
- [ADR-0037 — Work sandbox, egress gateway, and upstream auth](adr-0037-work-sandbox-egress-gateway-and-upstream-auth.md)
- [ADR-0035 — Den-native in-process agent runtime](adr-0035-den-native-in-process-agent-runtime.md)
- [ADR-0034 — Jobs and tasks (Docket)](adr-0034-jobs-and-tasks-work-management.md)
- [ADR-0026 — Work handoff and human escalation](adr-0026-work-handoff-and-human-escalation.md)
- [ADR-0006 — Bear work surfaces](adr-0006-bear-work-surfaces.md)
- [ADR-0050 — Agent Loop Control, Adaptive Budgets, and Runtime Checkpoints](adr-0050-agent-loop-control-adaptive-budgets-and-runtime-checkpoints.md)
- [`interactive-stances-and-role-axes.md`](../architecture/interactive-stances-and-role-axes.md)
- [`work-surfaces-and-conversations.md`](../guides/work-surfaces-and-conversations.md)
- [`den-state-machine-inventory.md`](../architecture/den-state-machine-inventory.md)

> **State-inventory maintenance.** This ADR owns the trust-profile and governance axes in the Den state machine inventory. Changes to trust profiles, governance values, governance transitions, or effective-policy derivation must update that inventory and include behavior-level tests or replay assertions for affected transitions.

## Hat authorization amendment (accepted 2026-10-01; partial execution cutover)

The Bear-owned hat is the canonical owner of positive tool/action and outbound-network grants for conversations and eligible Job runs wearing it. Typed, Den-verified execution origin and governance determine which effect surfaces can be used; human membership, the current connected armature, credential actor, work-surface bounds, Job/run assignment, and Den safety policy intersect the hat grant. A remembered interactive approval must be stored under the hat plus its action/target scope; `Just this time` is an exact open obligation, not a durable Bear-wide grant. Internal Curate and Watch activity receives separate Den-owned system authority rather than inheriting a hat. The [active hats plan](../roadmap/HATS_AND_SESSION_MEMORY_BOUNDARIES_PLAN.md#gate-0a--hat-owned-tool-and-network-permissions-partial-cutover-exit-pending) defines the phased fail-closed cutover. For current working-tree implementation and its still-partial effect coverage, see the implementation note below and the [maintained topic](../topics/bear-memory-hats.md). This amendment is decision rationale, not proof of complete resolver enforcement or shipment.

## Context

Bear Den models capability-specific runtimes as **profiles** (`chat`, `pair`, `curate`, `work`, `watch`) over one Den-native agent loop ([ADR-0035](adr-0035-den-native-in-process-agent-runtime.md), [ADR-0036](adr-0036-bear-profile-registry.md)). A profile bundles a trust boundary (which memory branches are readable/writable, approval/autonomy defaults, cross-role visibility) with a default compiled prompt and tool roster.

Designing the **handoff seams** between interactive and autonomous work exposed that profiles were doing two jobs at once:

1. a **durable trust contract** — non-negotiable boundaries such as "`work` must not read raw `pair/`" and "`curate` is the sole routine writer to `core/`"; and
2. a **runtime execution switch** — "this sandbox is a `pair` session" vs "this sandbox is a `work` run".

Three scenarios make the conflation untenable:

- **Remote `pair` loses its client.** The user expects the session to keep running in the cloud until they return. Naively this is "flip `pair` → `work`", but that would silently change memory scope and approval semantics mid-session.
- **Long-running `work` the user wants to interrogate.** Naively this is "attach a `pair` turn", but that collides with turn ownership and the `work`/`pair` memory wall.
- **Panic / checkpoint.** The user wants to stop an autonomous loop, snapshot its worktree, and resume interactively — without pretending the run was `pair` all along.

In every case the **work surface** is stable, the **sandbox/workspace** can persist, but **who is supervising the run right now** changes. Treating "human in the loop?" as a profile flip breaks the very invariants profiles exist to protect.

ADR-0037 already introduced a coarse `run_mode ∈ { interactive, autonomous }` on `RunAuthContext`. That is the precursor to this decision; it needs to generalize into an explicit, durable runtime dial that is **orthogonal** to the trust contract.

## Decision

Split the two jobs into two named, orthogonal concepts.

### 1. Trust profile (`Profile` in code)

A **trust profile** is the slow-changing, durable trust-and-memory contract for a class of work. It owns:

- memory read/write scopes (which branches: `pair/`, `work/`, `core/`, …);
- approval / autonomy class defaults;
- cross-role visibility rules (lethal-trifecta split);
- the default compiled prompt slice and default tool roster.

Values are unchanged: `chat`, `pair`, `curate`, `work`, `watch`. This is exactly what [ADR-0036](adr-0036-bear-profile-registry.md) calls a **profile**; this ADR renames the *concept* to **trust profile** in documentation and keeps the **code shorthand `Profile`** (`BearProfile`, `bear_profile_bindings.profile`, `profile_slug`, …). No schema or enum change is required by this ADR.

A trust profile is **not** the identity of a long-running sandbox or run. It is applied **per turn** as a template.

### 2. Governance (`Governance` in code)

A **governance** is the fast-changing supervision contract on a **run / workspace session**: *how is this execution being supervised right now?* It is run-scoped and mutable over the life of one run.

Initial modes:

| Mode | Human | Intent |
|------|-------|--------|
| `interactive` | Present (ACP/web) | Live collaboration; approvals via client/channel |
| `grace` | Recently disconnected | Finish in-flight turn; no new client-only tools; await return |
| `autonomous_continuation` | Absent, grace expired | Continue under executor-leaning effective policy; durable handoffs on block |
| `observational` | Present, read-only | Inspect/interrogate a run without owning its turns |
| `frozen` | Panic / handoff | Turn cancelled; worktree checkpointed; awaiting disposition |

Governance is documented as **governance**; the **code shorthand is `Governance`**. It generalizes ADR-0037 `run_mode`: `interactive`/`autonomous` become projections of governance onto `RunAuthContext` (e.g. `autonomous_continuation` ⇒ `run_mode = autonomous`).

### 3. Effective policy is a product, not a label

The runtime computes, per turn:

```text
EffectivePolicy = TrustProfile × Governance × Armature × RunAuthContext
```

- **TrustProfile** — durable boundaries and defaults (this ADR / ADR-0036).
- **Governance** — supervision dial (this ADR).
- **Armature** — where actuators live: ACP client tools, Den sandbox, or none (`interactive-stances-and-role-axes.md`).
- **RunAuthContext** — git/auth/operation actor selection ([ADR-0037](adr-0037-work-sandbox-egress-gateway-and-upstream-auth.md)).

The model never *infers* this cross product. Den enforces the tool roster, memory write target, and approval class for the effective policy, and exposes the components through `session_info`.

**Implementation note (2026-10-03, working-tree WIP; latest image not rebuilt):** Verified origin/governance and canonical source now own capability ceilings, defaults/control, and compaction; profile model/loop rows are not live overrides. Every ordinary conversation/Job/run requires a real hat even with zero hats, and inference/results/continuations/direct dispatch recheck live actor/source/hat and exact Work eligibility. Old unbound history is owner/admin read-only; pending IDE `den-conv-*` materializes only after admission. No fabricated default, automatic import/promotion, or ordinary Legacy memory/network/Cargo lane remains. Admin stance detail/configuration/provisioning, per-profile model/profile-registration routes, and Pair Plan/shared scaffold model tools are gone; initialization never creates/refreshes a profile registry; managed compilation emits bound base/modes independently of legacy contracts. No production inference selects a role prompt or role contract; source admission precedes every inference/continuation and direct tools recheck it. Curate briefing uses dedicated source-verified direct tool-free inference with Markdown instructions, and worker briefings consume direct verified `MemorySource`; generic system sessions/results/continuations are denied. The removed Rust aliases leave derived runtime-context metadata (`RuntimeContextLabel`), five source-kind labels with historical schema/audit encoding, not configurable stance authority. This does **not** complete `HatGrantResolver`: Den action/resource filtering remains partial for web, supported filesystem reads, and sandbox egress; broader Den effects/catalogs and other bounded persistent client choices remain next. The implementation session reports passing workspace all-target checks and 121 root tests, not latest-image shipment validation. The decision/rationale below is preserved history; consult [the current reference](../architecture/bear-stances.md) and [active plan](../roadmap/HATS_AND_SESSION_MEMORY_BOUNDARIES_PLAN.md).

### 4. Lifetimes are separated

| Object | Lifetime | Owner |
|--------|----------|-------|
| **Work surface** | Bear-durable | Bear ([ADR-0006](adr-0006-bear-work-surfaces.md)) |
| **Workspace session** | Run-scoped materialization (one clone + egress gateway pair) | Docket run ([ADR-0037](adr-0037-work-sandbox-egress-gateway-and-upstream-auth.md)) |
| **Governance** | Per-run, mutable timeline | Docket run |
| **Trust profile** | Applied per turn as a template | `bear_profile_bindings` ([ADR-0036](adr-0036-bear-profile-registry.md)) |

The **same** workspace session can span multiple governance states. A trust profile change is a change of template for *new* work, not a re-pointing of the sandbox.

### 5. Invariants that must not bend

Governance changes supervision; it must never be used to launder trust:

- `work`-class effective policy **must not read raw `pair/` or `chat/`**, even under `observational` inspection by a human.
- Outbound git/auth always follows `RunAuthContext`, never "whatever mode feels convenient".
- Approvals are authoritative in **Den**, not "maybe the disconnected client will answer" ([ADR-0026](adr-0026-work-handoff-and-human-escalation.md)).
- `curate` and `watch` remain distinct trust profiles; they are **not** governance states of an interactive session.

### 6. Transitions are durable and model-visible

Each transition emits a durable, model-visible event and updates run state:

```text
governance.changed {
  from, to,
  reason,                       // client_disconnected | grace_timeout | panic | explicit_handoff | human_returned | inspect_open | inspect_release
  run_id, work_surface_ref, workspace_session_id,
  trust_profile,                // effective profile for subsequent turns
  branch, checkpoint_ref,       // when a snapshot was taken
  human_expectation             // user-facing one-liner
}
```

`session_info` exposes `trust_profile`, `governance`, `armature`, `work_surface`, and `workspace_session_id`, plus `linked_runs` so an interactive session re-entry can reference background runs by id, branch, and PR rather than implying local edits.

### 7. Timing and intervention (policy hooks, not hard-coded)

- **Grace window** — heartbeat loss enters `grace`; a bounded timeout transitions to `autonomous_continuation`. Durations are policy, not constants in this ADR.
- **Turn-boundary steering** — `observational` mode may read and ask at any time; steering (new instruction/task) applies at the next turn boundary, except panic.
- **Panic** — cancels the active turn (`CancellationToken`), commit-and-pushes the worktree to a checkpoint branch, sets `frozen`, and optionally materializes a new workspace session for interactive continuation from that branch (same work surface, new manifestation).

## Naming

| Documentation term | Code shorthand | Notes |
|--------------------|----------------|-------|
| **Trust profile** | `Profile` | `chat`/`pair`/`curate`/`work`/`watch`; existing `BearProfile`, `profile`, `profile_slug` symbols unchanged |
| **Governance** | `Governance` | `interactive`/`grace`/`autonomous_continuation`/`observational`/`frozen` |
| Profile (bare) | — | Legacy alias for **trust profile**; avoid in new runtime prose |
| `run_mode` (ADR-0037) | — | Coarse `interactive`/`autonomous`; derived from governance |

Do not reuse `Governance` for human-identity trust, channel selection, or work-surface resolution state.

**"Governance" is reserved for runtime context/run supervision** (this ADR). The `curate` role's review/promotion of durable memory is **memory curation**, not "memory governance"; the code module is `core/memory/curation.rs` (`uses_sqlite_curation`) and the skill-proposal side effect is `ToolSideEffectKind::SkillReview`. The memory-curation roadmap lives at [`MEMORY_CURATION_PLAN.md`](../roadmap/MEMORY_CURATION_PLAN.md). Remaining "governance" uses are intentional: runtime governance (this ADR), RBAC/cost governance, and BearWire control-plane diagnostics.

## Relationship to continuation supervision and acceptance criteria

The "keep the loop on-task" machinery from [ADR-0023 (Task focus supervisor)](adr-0023-task-focus-supervisor.md) and the acceptance criteria from [ADR-0034 (Jobs and tasks)](adr-0034-jobs-and-tasks-work-management.md) are **not** a separate, orthogonal supervision system. They split cleanly along this ADR's axes:

| Question | Owner |
|----------|-------|
| What counts as finished / on-task? | **Acceptance criteria** (`bear_job_criteria`, ADR-0034) — durable definition of done; `command` criteria are hard completion gates. |
| How hard to drive, when to yield, who is watching? | **Governance** (this ADR) — `interactive` yields/asks, `autonomous_continuation` drives to completion, `grace` transitions, `frozen` stops, `observational` never nudges. |
| Is *this* candidate yield premature right now? | **Task focus** — an ephemeral projection of `(governance × focused Job × acceptance-criteria state × run/task status)`, evaluated as a phase of the native loop, **not** a fourth state machine. |

Consequently, continuation bias is **governance-driven, not trust-profile-driven**. ADR-0023's "`work` drives harder than `pair`" is re-expressed as: the trust profile *defaults* a run's governance (a `work` run typically starts more autonomous, a `pair`/`chat` run interactive), but a `pair` run in `autonomous_continuation` is driven just as hard. Focus nudges are governance-aware and reference acceptance criteria as the success contract.

A **focused Job** is the Docket Job designated as the durable objective for a run. `work` normally requires one; `pair` normally has none, but may designate one explicitly through Bear conversation or a client command. While a focused Job is active, Den asks the model to address the next logical incomplete, unblocked task for that Job until the Job completes, blocks, is cancelled, focus is cleared, or loop-control checkpoints/budgets stop the run. This is intentionally not generalized into `focus_target` yet; the only supported durable focus object is a Docket Job.

The concrete budget/checkpoint machinery for this relationship is specified by [ADR-0050 (Agent Loop Control, Adaptive Budgets, and Runtime Checkpoints)](adr-0050-agent-loop-control-adaptive-budgets-and-runtime-checkpoints.md) and sequenced in [`AGENT_LOOP_CONTROL_IMPLEMENTATION_PLAN.md`](../roadmap/AGENT_LOOP_CONTROL_IMPLEMENTATION_PLAN.md): tool-call and wall-clock budgets, repeated-tool/ko detection, failure thresholds, runtime checkpoints, and task-list/Docket reconciliation are loop-control policy, while task/job state remains Docket-owned.

## Consequences

- **`pair` ↔ `work` flipping largely disappears** as a runtime mechanism. Offline continuation, interrogation, and panic/resume are governance transitions on a stable run + workspace session.
- **Profiles keep their meaning** as durable trust contracts and prompt/tool defaults, and as product language (`bear-stances.md`). They stop being sandbox-lifetime identifiers.
- **Schema impact is additive.** Governance is a new run-scoped field plus a transition log; trust profile vocabulary and `bear_profile_bindings` are unchanged. ADR-0037 `run_mode` becomes a derived projection.
- **`curate` and `watch` are unaffected**; they are not interactive runs and do not carry governance states beyond a fixed autonomous supervision.
- **UX can stay continuous** ("your session is still running") while ops truth stays honest (`governance = autonomous_continuation`, executor-leaning effective policy on the same `workspace_session_id`).
- **Follow-up (not decided here):** the concrete `WorkspaceSession` + governance-timeline schema, transition guards, and whether `chat`/`pair` eventually collapse into one `interactive` trust profile distinguished only by armature.
