# Canonical Docket execution attempts

## Status

Implementation contract for the continuation-authority revision. This is the
required foundation for direct, fence-validated control decisions at Pair and
Work safe boundaries; it does not require a scheduler-observation outbox.

## Boundary

Docket owns the **outer** continuation loop for every autonomous task attempt:

```text
eligible task -> authorize attempt -> receive durable outcome
              -> continue | advance | retry | pause | stop | await user
```

Pair and Work own their **inner** model/tool loops. They choose prompts and
local tool actions, enforce a Docket stop at safe boundaries, and report facts.
They never choose another Docket task or decide task-tree continuation.

A Pair `current_task_id` is an objective, not execution authority. A Work run's
assigned Job bounds task selection, but is not by itself authority to execute a
task. An autonomous start or resume needs a valid Docket execution attempt.
Ordinary untasked Pair conversation remains Den/runtime-driven and creates no
attempt.

## Focused-execution ownership

`docket_execution_attempts` remains the only durable authority for focused
execution. Do **not** add a focus-session table, infer authority from a host run,
or require an in-memory controller to prove that authority exists.

Four related records have deliberately separate meanings:

1. **Task selection** associates a client session or Work assignment with an
   objective. It survives interruption and grants no execution authority.
2. **Execution attempt** is Docket's exclusive, fenced authority to act on one
   task tree. Its live state is the sole answer to “may autonomous work
   continue?”
3. **Host run** is transport/transcript/checkpoint evidence produced by a host.
   It may be replaced across reconnects or bounded continuations without
   replacing the execution attempt.
4. **Controller** is an ephemeral worker for a host run. Losing it is a host
   failure to reconcile, not a second durable execution state.

Consequently:

- focus acquisition is a Docket operation, serialized at the execution
  attempt's exclusive owner/task scope;
- repeated acquisition for the same live binding is idempotent and returns the
  existing attempt and fence;
- a conflicting live binding is rejected or explicitly superseded by Docket,
  never repaired by inserting another attempt and catching a unique-index
  error;
- only the current attempt ID and fence may report a boundary or settle work;
- bounded host slices preserve the attempt and fence unless Docket explicitly
  pauses, releases, or supersedes it;
- an absent controller never authorizes work, but neither does it invalidate or
  settle the attempt implicitly;
- stale or terminal host runs are cleanup evidence and cannot block acquisition
  once Docket has released or superseded their binding;
- settling task A and selecting/authorizing successor B is one Docket reduction,
  not a host decision based on whether a final response was emitted.

## Migration boundary

This migration completed in place; no parallel authority abstraction was
introduced. The former `owner_kind = pair | work`, `pair_session_id`,
`pair_run_id`, and `work_run_id` representation conflated the durable owner
binding with a particular host run. The compatible stages were:

1. Add a host-neutral binding identity and kind to execution attempts. The
   binding identifies the stable execution owner (for example a client session
   or Work assignment); host-run correlation is separate, replaceable evidence.
2. Backfill binding identity from the former `pair_session_id` and `work_run_id`
   columns. Keep those columns temporarily as compatibility projections while
   session and Work callers move to shared Docket acquisition.
3. Move live-owner uniqueness and lookup to the host-neutral binding. Keep
   one-live-attempt-per-task fencing. Acquisition must be transactional and
   idempotent by caller-supplied authorization/acquisition key.
4. Route session `/focus`, model-initiated focus, Work dispatch, settlement,
   cancellation, and recovery through the shared service path. Host adapters
   attach or replace their run correlation after acquisition; they do not grant
   authority. Cancellation terminalizes available host evidence and releases
   the stable binding even when that host run or controller is already absent.
   UI/tool projections read the binding and host fields; legacy stance-specific
   columns were compatibility storage, not presentation contracts.
5. Remove stance-specific authority columns, indexes, queries, and reconciliation
   after no caller treats them as canonical. Preserve terminal attempt and
   host-run evidence.

The implementation may initially represent binding kind as the existing Pair
and Work variants to avoid a broad rename migration, but Docket service APIs and
new schema concepts must use host-neutral terms (`binding`, `host`, `host_run`,
`attempt`, and `controller`). `Pair` belongs only in the Pair adapter and legacy
compatibility mapping.

## Migration acceptance checks

- Concurrent equivalent acquisitions return one attempt/fence; conflicting
  acquisitions cannot create two live authorities.
- A controller loss followed by reacquisition does not require weakening
  settlement fencing and does not leave a unique-index dead end.
- Replacing a bounded host run preserves the authoritative attempt/fence.
- A stale host run cannot settle after its attempt is released or superseded.
- Task selection remains visible when execution is paused, released, or absent.
- A non-Pair host can acquire, present, continue, and release focus without a
  Pair run ID or Pair lifecycle branch.

## Record

`docket_execution_attempts` is the one durable authority record. It is distinct
from task status, Pair runs, Work runs, routing reservations, and turn-attempt
telemetry.

```rust
enum ExecutionAttemptOwner {
    Pair { session_id: ClientSessionId, pair_run_id: PairRunId },
    Work { work_run_id: WorkRunId },
}

enum ExecutionAttemptState {
    Authorized,
    Running,
    Paused,
    AwaitingUser,
    Stopping,
    Settled,
    Released,
}
```

Each record has an immutable ID, `task_id`, owner kind/correlation, monotonic
`fence_epoch`, state, authorization key, creation/update timestamps, and
state-specific timestamps (`started_at`, `paused_at`, `settled_at`,
`released_at`). The owner correlation is typed columns/foreign keys, not JSON.
A fence is opaque to the runtime except for equality: it must accompany every
start, resume, control acknowledgement, and outcome report.

`Authorized`, `Running`, `Paused`, `AwaitingUser`, and `Stopping` are live.
`Settled` and `Released` are terminal. Only one live attempt may exist for an
exclusive task/owner scope. The precise partial unique indexes are determined
by whether a task supports concurrent owners; the initial contract is one live
attempt per task and one per Pair session or Work run.

## Transitions

| Transition | Actor / atomic boundary | Idempotency and fencing |
| --- | --- | --- |
| authorize | Docket transaction: verify task eligibility and owner binding; allocate next fence; write attempt | authorization key returns the same live attempt; conflicting owner/task is rejected |
| start | runtime request accepted by Docket transaction, then runtime begins local loop | requires `Authorized` + exact fence; replay returns the same state |
| pause | runtime reports a bounded checkpoint; Docket transaction stores pause fact | requires exact live attempt/fence; duplicate report is a no-op |
| await user | Pair reports precise question; Docket stores question reference and pauses attempt | Pair only; exact fence; duplicate question key is a no-op |
| resume | Docket verifies explicit authenticated user response/resume for `AwaitingUser`, or continuation policy for `Paused`, then moves to `Authorized` or creates successor with new fence | stale fence is rejected; no implicit resume on reconnect |
| stop | Docket changes a live attempt to `Stopping`; runtime observes it at a safe boundary and reports release/outcome | stop is idempotent; it prevents new start/resume, not retroactive cancellation of a tool call |
| settle | runtime proposes typed terminal outcome; Docket validates task version/fence and atomically records outcome, task reduction, and attempt settlement | outcome idempotency key returns the accepted result; stale/duplicate conflicting reports are rejected |
| release | Docket reconciler releases an abandoned or superseded live attempt with synthetic provenance | compare-and-set on live state and fence avoids releasing a successor |

`Running` is set only by the successful Docket start transition, before inner
loop work. A selected task with no live attempt is never projected as executing.
A runtime may send progress/heartbeat facts while running, but they do not
renew authority or select further work.

## Outcomes and continuation

Runtime reports typed facts, not scheduling decisions: `completed`, `no_change`,
`blocked`, `failed`, `cancelled`, `checkpoint`, `awaiting_user`, and local stop
acknowledgement. Docket validates and reduces task/run state, then decides the
next continuation. `awaiting_user` is Pair-only, needs a precise question, and
is ineligible until authenticated user input or explicit resume is recorded.
Work has no user-input outcome; retry/timeout/escalation derives from Job/task
policy.

Commit/artifact delivery is post-settlement evidence. A delivery retry cannot
reopen an accepted attempt outcome.

## Failure and recovery

Transactions that grant authority also persist the attempt; transactions that
accept a terminal outcome also persist the reduction. Cross-process delivery is
at-least-once and every receiver validates attempt ID plus fence.

On restart/reconciliation, Docket must identify and resolve: ownerless live
attempts, live attempts whose Pair/Work owner no longer exists, selected tasks
presented as executing without a live attempt, expired Work ownership, and
reports bearing stale fences. It appends synthetic recovery provenance and
releases or stops the old attempt; it never fabricates model output. Lease
expiry means no new work and cannot roll back an already-issued external call.

## Legacy mapping

- `docket_execution_sessions` is migration input/correlation only; it is not a
  continuation authority after attempt creation.
- Pair runs and Work runs are owner-local execution/checkpoint history. They
  reference the attempt and fence; they do not authorize themselves.
- Docket routing reservations and turn-attempt telemetry remain inner-loop
  dispatch evidence and must reference the canonical attempt where task work is
  autonomous.
- Scheduler-observation tables were removed pre-release with legacy execution
  sessions. Direct Docket boundary decisions must instead carry attempt ID,
  owner correlation, and fence; do not reintroduce an observation outbox.
- Work checkpoint escalation is defined in [Docket Work checkpoint control](docket-work-checkpoint-control.md). A checkpoint artifact is evidence tied to the canonical Work attempt/fence through the existing checkout session-to-runtime-run boundary; checkpoint acknowledgement never authorizes dispatch.

## Required persistence tests

Database-backed tests cover duplicate authorization, double start, stale fence,
duplicate terminal report, Pair await-user/resume authorization, owner loss,
and Work retry/advance. Runtime-gate tests cover Pair start/resume and Work
dispatch rejection without a valid authorized attempt.
