# Plan: Authoritative focused execution control and diagnostic transition tracing

**Status:** Focused-execution lifecycle stabilization through P11 completed 2026-09-11; live Den + Armature ACP validation completed 2026-09-12

**Scope:** Focused Docket execution start, authoritative status projection, BearWire diagnostics, and client debug views

`pair` is a trust-profile/capability shorthand, not an execution identity. Execution state and authority are named for sessions, tasks, runs, hosts, and attempts.

### Completed live release validation

- A live `gpt-5.5` Pair-stance write turn created and selected a session task, invoked `focus_current_task`, continued focused execution on the same BearWire run, completed an Armature-local file tool, settled the task, and produced exactly one terminal ACP prompt response.
- The persistent focused transition stream was contiguous (`focus_acquired` → client-wait continuation → terminal completion), all tool calls retained one run ID, the client obligation reached `continued`, and final `run.state` reported no open obligations.
- Validation exposed and repaired three adjacent single-source-of-truth failures: smoke seeding did not provision required Bear-scoped Bifrost credentials, native materialization discarded explicit conversation model selection by looking up the internal `den-conv-*` ID, and session-task settlement rejected the selected task it was designed to settle.
- The stale Letta/MemFS/Codepool smoke matrix and redundant direct-inference check were retired. Three broad checks now cover native stack readiness, BearWire authentication, and the full live Armature ACP focus lifecycle.
- Rollout tooling now shares one response-counting Armature test client, keeps fake BearWire coverage aligned with durable event pages and tool leases, and inspects canonical `turn_*` lifecycle tables without dumping transcript or tool-result bodies.
- The former three-failure BearWire baseline is eliminated: blocked Docket settlement now atomically blocks the job run, preserves task selection, terminalizes the exact BearWire host when no successor exists, and returns chat control; command-expiry and replay tests now model current process ownership and asynchronous persistence truthfully. The broad BearWire suite passes 124/124.
- The former seven-failure Armature baseline is also eliminated. Invalid bare-hunk fixtures now use standard unified ranges, created-file patches preserve the conventional final newline unless explicitly negated, and duplicate argument-reconstructed card-title assertions were retired in favor of descriptor-owned display coverage. The broad Armature suite passes 322/322 with two intentionally ignored legacy web-fetch tests.

### Completed stabilization phase P11

- Docket bounded-loop types and methods now use focused-slice terminology; execution continuation no longer names the Pair trust profile.
- The internal execution gate variant is `Session`; its serialized `"pair_session"` spelling remains pinned for deployed compatibility.
- BearWire responses expose canonical `session_execution` while retaining generated read-only `pair_binding`, and Armature prefers the canonical field with legacy fallback.
- Session-attempt helpers, diagnostics, error text, and focused runtime tests use session/focused terminology. Legacy SQL table names remain unchanged to avoid a no-value migration.

### Completed stabilization phase P10

- Armature owns one bounded local focused-transition tracker per client session; tracking never affects Den state, obligations, prompt liveness, or terminal delivery.
- Existing `/debug off|on|verbose` controls visibility locally: off tracks silently, while on/verbose render concise transitions as ACP thought updates and return a copyable bounded JSON bundle.
- Transition replay is idempotent, local history is capped and cleared on session close, and version gaps trigger a rate-limited refresh from `session.execution.diagnostics` rather than client-side guessing.
- Unknown/legacy clients continue ignoring optional diagnostics. Existing command/event tests plus one broad tracker test cover visibility, gaps, refresh replacement, and bundle rendering.

### Completed stabilization phase P9

- Orphan detection, stale session/foreign authority cleanup, typed reconciliation reasons, source-run failure, and recovery handoff projection now live under the focused-execution application service instead of the session transport module.
- Historical settled/released attempts are excluded when no task is selected. A selected task whose focused authority ended while its ordinary host run remains live now projects `Terminal`, not a false active-run invariant violation.
- The authenticated read-only `session.execution.diagnostics` RPC returns the canonical snapshot plus bounded typed transition history, truncation/gap indicators, snapshot consistency, and reason counts.
- Existing start/reuse, settlement, stale authority, orphan recovery, and reducer tests cover the consolidated behavior; no standalone reconciliation test matrix was added.

### Completed stabilization phase P8

- One protocol-owned `diagnostic.state_transition` envelope records focused-execution phase changes with typed reasons, correlation/causation IDs, task/run/attempt refs, obligation counts, and session-scoped monotonic state versions.
- Nonterminal and terminal run mutations append focused transitions in the same SQL transaction when a Docket execution attempt is bound to the run.
- Task settlement and exceptional attempt-only reconciliation project the corresponding terminal transition before successor control starts.
- Legacy `docket.execution.*` event production and model-context injection were removed. Diagnostics remain outside model history; Armature treats the new event as optional, run-scoped metadata while tolerating legacy replay events.
- Existing broad focus, continuation, settlement, recovery, protocol, and adapter tests now assert transition ordering and compatibility; the redundant prose-rendering unit test was retired.

### Completed stabilization phase P7

- Execution authorization and routing now derive from `EffectivePolicy` capabilities rather than treating `pair` as an execution identity.
- Explicit capabilities cover job creation, work dispatch, work-surface management, session-task ownership, focused execution, conversation replay, and work-surface use.
- Governance removes mutation capabilities outside interactive operation, while armature availability independently controls local-tool access.
- Direct `pair` checks remain only at trust-profile, prompt, memory-path, review, UI-label, and provenance boundaries where the profile itself is semantically relevant.

### Completed stabilization phases P5/P6

- Shared lifecycle DTOs now decode run state, terminal outcomes, launch responses, obligation envelopes, and wrapped/direct event compatibility in `bearwire-protocol`.
- Armature reconciliation consumes the shared terminal model rather than maintaining a duplicate run-state enum and raw decoder.
- One `PromptDriver` owns the parsed run, response guard, cancellation receiver, polling loop, delivery deadline, reconciliation, tool-card cleanup, and obligation servicing for each followed prompt.
- Focused phases, controller disposition, invariant violations, launch state, typed obligations, recovery handoffs, and the full binding/run/attempt/fence snapshot envelope are protocol-owned.
- Run-state RPC responses are decoded once into typed lifecycle plus a raw extension payload used only for extensible tool arguments and diagnostics.
- The duplicate Armature terminal decoder, repeated launch parsing, free polling/deadline state machines, and obsolete unleased tool-future waiter were removed.

### Completed stabilization phase P4

- Removed the Pair-specific `docket_pair_launches` mini-scheduler; generic run, attempt/fence, and controller state now own startup.
- Fresh starts return `accepted` + `authorized` + `claimed`. They transition to `running` and emit `run.started` only after native session construction succeeds.
- Controller claim and native start are distinct typed `diagnostic.state_transition` reasons.
- Startup failure releases authorized execution attempts instead of leaving nominally running authority.
- Technical restart recovery uses an exclusive lease and launches a normal claimed successor; it never reports a source run as running without a native session/controller.
- Docket task changes use the same explicit superseding handoff rather than requiring the successor task to already own the predecessor run.

### Completed stabilization phase P3

- Terminal writes are limited to typed `complete_run`, `fail_run`, and `cancel_run` methods; callers cannot choose a contradictory event type.
- Run-level `blocked` was migrated to retryable or non-retryable `failed`; Docket task and work-run `blocked` states remain distinct.
- Controller registrations and native execution sessions are run-scoped, and terminal paths evict the exact native run.
- `run.cancel` requires the target `run_id`; stale cancellation cannot affect a successor run.
- Client-wait persistence locks and validates the durable run, so late tool events cannot reopen terminal runs or create new obligations.
- Orphan focus recovery consumes `FocusedExecutionSnapshot` and emits a typed `run.failed` followed by `run.recovered`, never a failed run carrying `run.recovering` as its terminal event.

## Problem

A `/focus` attempt can currently leave independently plausible but contradictory state: the Pair task is selected, a Docket run appears `running`, session activity is inactive, no controller owns execution, and no work starts. A later ordinary user turn is a poor recovery mechanism because steering is allowed to interrupt Docket control.

The failure is architectural, not merely a missing retry. Selection, run lifecycle, attempt authorization, controller acquisition, and UI activity are independently written or inferred. No single transition result proves that focus both acquired control and scheduled the first slice, and no replayable semantic record explains where acquisition stopped.

## Decisions

- `/focus` and a model-facing `focus_current_task` tool invoke one Den-owned start command. Success means control was acquired and the first slice is durably queued or running; callers do not orchestrate intermediate writes.
- Task selection remains assignment only. The model may self-focus only when an executable current task is already selected (or after an explicitly authorized selection); focus never silently selects, creates, replaces, or settles a task.
- One authoritative focused-execution aggregate reduces persisted selection, execution run, fenced attempt/lease, controller/scheduler ownership, and obligations into a typed derived state.
- Existing records remain normalized inputs where useful; do not add a duplicate mutable `pair_execution_state` table merely for convenience.
- Major aggregate transitions produce persistent BearWire `diagnostic.state_transition` events in the canonical session replay stream.
- Diagnostic events are control-plane transcript artifacts, not assistant/user/model messages. They are normally excluded from model history.
- Den always records and sends authorized subscribers these events. Whether to display them is client-local debug-view state, not Den session state.
- Logs and metrics remain operational telemetry. They may link to transition correlation IDs but are not the user-reviewable source of truth.

## Target invariants

```text
FocusedExecution=running
  => executable persisted current task
  && active persisted execution run
  && current fenced Docket attempt
  && live or durably queued controller ownership

start accepted
  => authoritative state in {running, waiting}
  && first slice queued/running or a typed open obligation

start not acquired
  => no success response
  && typed durable rejection/failure transition
  && no orphan nominally-running projection
```

Every aggregate transition has a monotonic state version and one correlation/idempotency key. A durable transition event is written transactionally with authoritative persisted changes, preferably through an outbox consumed by BearWire projection. Process-local controller registration that cannot share the database transaction must be represented by a durable queued/lease record before success is returned.

## Implementation sequence

### 1. Reproduce and instrument the failure

- Add a focused integration scenario: selected executable session task, `/focus`, no later user turn.
- Assert that the current implementation can produce or previously produced the contradictory selection/run/activity/controller combination.
- Trace `session.current_task.start`, Docket attempt acquisition, Pair controller registration, first-slice scheduling, and failure cleanup with one correlation ID.
- Classify each exit with a typed reason; do not rely on status text.

**Done when:** the regression fails before the repair and identifies the exact boundary where acquisition can stop.

### 2. Introduce the authoritative aggregate — completed 2026-09-11

- `FocusedExecutionSnapshot` is produced by one SQLx projection and one side-effect-free reducer.
- It covers `Unfocused`, `Selected`, `Starting`, `Running`, `WaitingForClient`, `Continuing`, `Recovering`, `Terminal`, and typed `Inconsistent` states.
- Selection/task, binding, run, attempt/fence, host, controller disposition, and obligation summary are projected once.
- Focus/start results, Docket-facing status, BearWire `session.state`, and Armature diagnostics consume the same snapshot instead of independently deriving `active` or `running`.
- The old `SessionTaskStartResult`, `FocusedExecutionSnapshot::is_live`, native-session activity inference, and `active_docket_execution` projection were removed.

**Validated by:** one table-driven reducer test covering every phase and invariant, plus the existing broad SQLx focus/start test asserting identical snapshots across start replay and `session.state`.

### 3. Make start one serialized command

- Route `/focus`, BearWire `session.current_task.start`, and the model-facing `focus_current_task` tool to one application service.
- Extend or inject the model-tool invocation boundary so it can call the Den-owned focused-execution service. Do not work around this by copying a database-only start sequence into a workflow tool.
- Expose `focus_current_task` only when effective policy grants `ExecuteFocusedTask`. It takes no task ID: it starts the session's already-selected task, returns the authoritative snapshot/correlation ID, and follows the same authorization, idempotency, and typed-failure behavior as `/focus`.
- Under transaction/CAS and an idempotency key: resolve the selected executable task, create/resume the execution run, acquire a fenced Docket attempt/lease, persist durable controller queue ownership, and append the transition outbox record.
- Return success only after the postcondition reducer reports `running` or a legitimate `waiting` state.
- On rejection or failure, atomically settle/release partial acquisition and append a typed rejected/failed transition.
- Preserve task selection when later user steering interrupts control.

**Done when:** retries are idempotent, concurrent starts produce one owner, and crash-at-boundary tests cannot leave a successful but ownerless start.

### 4. Add semantic transition tracing — completed 2026-09-11

- `FocusedExecutionTransition` and its reason enum are owned by `bearwire-protocol` and projected as persistent `diagnostic.state_transition` events.
- Major run phases, client wait/clear, steering interruption, task settlement, reconciliation, and terminal outcomes use one typed transition stream.
- Session event locking assigns contiguous aggregate versions and ordered `from`/`to` states; the BearWire event envelope supplies durable sequence and timestamp metadata.
- Run-state and terminal mutations append transitions transactionally. Docket task settlement emits its transition immediately after the leaf transaction and before successor execution begins.
- Transition payloads contain bounded typed refs and counts only—no prompts, credentials, tool arguments, or output—and are excluded from model runtime context.

**Validated by:** the existing broad same-run focus, autonomous focus/settlement, bounded continuation, and orphan-recovery tests, plus shared protocol decoding and Armature optional-event compatibility tests.

### 5. Build client debug projection — completed 2026-09-11

- Armature observes typed transitions into a bounded process-local/session-local tracker regardless of visibility mode.
- `/debug off|on|verbose` changes only that Armature process: off is silent; on/verbose render transitions as ACP thoughts and expose a copyable JSON diagnostic bundle.
- Rendered transitions include version, from/to phase, typed reason, correlation ID, and task/run/attempt refs without entering the ordinary assistant transcript.
- Duplicate/old transition versions are ignored. Gaps are marked and trigger a rate-limited authoritative `session.execution.diagnostics` refresh.
- Session close deletes the local timeline, and clients without this support continue ignoring the optional event.

**Validated by:** the broad local tracker replacement test, debug toggle/bundle test, and existing optional-event compatibility test.

### 6. Reconcile and operate — completed 2026-09-11

- Start-time reconciliation uses the focused aggregate service and typed reasons for orphaned controllers, stale session authority, and stale foreign authority.
- Recovery fails the exact orphan source run, preserves task selection, releases only stale attempts whose host is terminal/missing, and records the replacement handoff.
- `session.execution.diagnostics` exposes the authoritative snapshot, bounded transition history, reason counts, version gaps, truncation, and snapshot/transition agreement without exposing transcript or tool payloads.
- Current invariant state remains part of the typed snapshot; persisted transition reason counts provide bounded operator counters linked to event correlation IDs.

**Validated by:** the existing broad start/reuse, task-settlement, stale-session, stale-foreign, and orphan-controller recovery tests.

#### Recovery runbook

1. Query `session.execution.diagnostics` for the authenticated Bear/session.
2. If the snapshot reports missing controller/authority or the latest transition disagrees with an active snapshot, retry the normal focus command; do not edit run/attempt rows directly.
3. The command fails the exact orphan host run, preserves the selected task, releases stale authority, and launches a claimed successor. A live foreign owner is rejected rather than stolen.
4. If `version_gap` or `history_truncated` is set, treat the canonical snapshot as current and use event sequence/correlation IDs for the bounded historical investigation.

**Done when:** the known split state is detected, explainable from replay, and repaired without clearing task assignment or inventing execution.

## Smallest runnable checks

1. Table-driven reducer test covering every aggregate state and contradictory combinations.
2. Integration test that `/focus` alone starts and schedules the selected task.
3. Tool-parity test: a session with `ExecuteFocusedTask` receives `focus_current_task`; invoking it and `/focus` reaches the same application service and postcondition, while policies without the capability do not receive it.
4. Tool-safety test: `focus_current_task` without a selected executable task returns a typed failure and performs no selection or run mutation.
5. Crash/failure injection between each acquisition boundary; no accepted start becomes ownerless.
6. Concurrency test: two starts yield one fenced owner and one idempotent/conflict result.
7. Steering test: a user turn interrupts control but preserves selection.
8. BearWire golden trace and replay test: snapshot plus transitions reconstructs current state.
9. Client projection test: debug off hides diagnostics; debug on renders them; neither changes server state.

Run the smallest crate-local tests identified while tracing before broader workspace checks.

## Rollout

1. Ship the aggregate read-only and compare it against existing projections; emit metrics for disagreement.
2. Switch status/read paths to the aggregate.
3. Enable transition outbox and BearWire events; clients initially ignore them.
4. Switch `/focus` to the atomic start service.
5. Enable client debug rendering.
6. Remove superseded independent status derivations after disagreement remains zero through a deployment window.

## Non-goals

- Recording every database write or heartbeat in transcript history.
- Making debug visibility a persisted session mode.
- Feeding raw diagnostics into ordinary model context.
- Replacing operational logs, traces, or metrics with BearWire events.
- Creating a second execution lifecycle owned by BearWire or the client.
