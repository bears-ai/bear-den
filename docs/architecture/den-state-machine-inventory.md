# Den state machine inventory

**Status:** Living architecture reference.

This document inventories the state dimensions that apply to a Den conversation, session, turn, run, and Docket task unit of work. It is intentionally a matrix of orthogonal axes, not a proposal for one giant enum.

Use this document when adding or changing runtime state, mode labels, continuation policy, approvals, current-task selection, model selection, or client projection. If a new state dimension can affect what the model may do, whether a turn may stop, what a client shows, or how a run resumes, update this inventory in the same change. The [session memory and hats contract](../topics/bear-memory-hats.md) is a partial branch cutover, not a fully deployed ownership model; [its plan](../roadmap/HATS_AND_SESSION_MEMORY_BOUNDARIES_PLAN.md) tracks the remaining axes and migration.

## Maintenance rule

State changes are architecture changes. A PR or plan that adds, removes, renames, or changes semantics for any state axis below should also update:

- this inventory;
- the owning architecture document or ADR;
- relevant roadmap/implementation plans;
- at least one executable invariant, table-driven test, replay assertion, or projection test when the state affects runtime behavior.

Prompt text and UI labels are never authority by themselves. Canonical state must remain typed, owned, replayable where it affects runtime behavior, and derivable into prompt/UI projections.

## State-machine shape

Den should keep a broad inventory of mostly orthogonal product axes:

```text
TurnStateInventory =
  BearIdentity
× UserMembership
× ConversationState
× SessionBinding
× HatBinding
× TrustProfile
× Governance
× CurrentTaskState
× WorkAssignmentState
× PermissionPolicy
× TurnRunLifecycle
× ObligationSet
× LoopControlState
× ModelSelection
× RecoveryState
```

That inventory is not the runtime authority surface. The implemented authority
surface is intentionally smaller:

```text
RuntimeAuthorityState =
  TurnAuthority
× ResolvedCurrentTask
× WorkAssignment
× DocketTaskState
× DeliveryState
× TurnRunState
× ObligationSet
× LoopControlState
```

The three authority owners that must stay singular are:

```text
MutationAuthority = TurnAuthority
FocusDrivenContinuation = ResolvedCurrentTask × WorkAssignment × DocketTaskState
ActiveObligations = TurnRunState × ObligationSet
TerminalOutcome = TurnRunState × ObligationSet × TurnStepState × BearWireTerminalEvent
```

`TurnAuthority` compiles the verified execution origin, governance, and Den-owned
session mode/plan policy for armature tool advertisement, routing, prompt authority
blocks, and client permission projection. Governance may narrow armature tool
availability but cannot grant an origin new authority. The selected hat, rendered
prompt, and client-supplied tool labels cannot expand this permission surface.
`ResolvedCurrentTask` is Pair's validated persisted session current task; `WorkAssignment` is Work's explicit Job assignment. Only
the applicable owner may supply task-driven continuation. `TurnRunState` owns
lifecycle and active wait reason. Terminal closure is committed through one
atomic finish operation that transitions the run, settles obligations and active
steps, and appends exactly one run terminal BearWire event before exposing the
terminal state.

The remaining axes feed compilation, execution, or projection. They are not peer
authority owners unless a typed implementation seam consumes them into one of
the authority owners above:

```text
EffectiveCapabilityProjection = VerifiedTurnExecutionOrigin × Governance
ArmaturePermissionEnvelope = EffectiveCapabilityProjection × DenOwnedSessionMode

RunSupervisionProjection = Governance × HumanPresence

CompletionProjection = ResolvedCurrentTask × WorkAssignment × DocketTaskState × Governance × BudgetState × ObligationSet

DerivedViews = ModelRequest × PromptContext × MemoryProjection × CompactionProjection × SurfaceProjection
```

`DerivedViews` are explicitly non-authoritative. They may cache or render canonical
state, but cannot expand mutation permission, manufacture task selection or
obligations, choose a terminal outcome, or override current-turn ordering.

Do not let a derived view become a peer source of truth. If two axes disagree,
resolve the conflict into canonical state before prompt rendering or client
projection.

Exit gate for the reduced authority model:

- mutation authority has one owner: `TurnAuthority`;
- task-driven continuation has one owner: Pair's validated `ResolvedCurrentTask` or Work's explicit `WorkAssignment`, together with applicable Docket task state;
- active waits have one owner: `TurnRunState` plus `ObligationSet`;
- projections, caches, labels, model choice, prompt text, and compaction state
  cannot expand authority or manufacture task selection/Work assignment.

## Ownership summary

| Axis | Primary owner | Scope/lifetime | Notes |
| --- | --- | --- | --- |
| Bear identity/profile binding | Den registry/control plane | durable Bear | Identifies the Bear; managed prompts compile the Bear-wide baseline and platform interaction modes. A bound turn's responsibility/identity comes from its selected Bear-owned hat, not a per-stance persona. |
| User membership/access | Den identity/RBAC | durable + request-scoped | Gates which conversations/surfaces a user can access. |
| Conversation | Den Postgres | durable user-visible chat container | Owns transcript, archive state, title, model selection, and Pair's persisted current-task reference. Authenticated human viewers use immutable `created_by_user_id` plus current Bear membership; Bear admins may inspect ownerless legacy rows. |
| Session/client binding | adapter/BearWire/web edge + Den session store | live client binding | Projects conversation/runtime state to a connected client; not the conversation. BearWire event reads check Bear and human owner. A Postgres ownership guard prevents new cross-user/Bear session-ID claims; historical collisions remain quarantined rather than silently reassigned. |
| Hat binding | Den Postgres conversation or Docket Job | durable conversation/Job | Nullable for historical/no-hat records; a configured Bear cannot start an unbound ordinary turn. Bound memory tools and turn assembly resolve this canonical binding for both memory and hat identity, never a hat named in chat; ineligible Work bindings fail closed. Bound prompt-memory selection, model-facing prompt-block tools, and diagnostics derive narrower access from it. Bear-admin UI can bind an empty inactive conversation or eligible draft Job. A Bear-owned IDE default applies on new ACP session creation, and an armature-local `/hat` can select another hat before the first turn; browser users select a hat before creating a conversation; Job creators select an eligible Work-enabled hat before the Job and its initial run are created. The Bear-level connection is identity, not a hat grant. |
| Trust profile | Bear profile registry | per turn/template | `chat`, `pair`, `curate`, `work`, `watch` remain compatibility labels for runtime binding, descriptor role filtering, memory-source routing, and no-hat prompts, not the identity or private-memory owner of a bound turn. The effective capability table, BearWire armature tool envelope, and native turn model-tool merger use verified `TurnExecutionOrigin`, deriving a compatibility profile for prompt/budget selection. Client-result continuations use the originating native session profile, recheck live Work eligibility, and verify a matching Bear runtime binding before executing an approved Den-hosted tool. Other runtime paths still take a profile and no stance has been fully retired. |
| Governance | runtime/workspace session | run-scoped mutable timeline | `interactive`, `grace`, `autonomous_continuation`, `observational`, `frozen`. |
| Pair current task | conversation + client-session binding | durable conversation/session selection, resolved per run | A validated persisted `client_sessions.current_task_id` is Pair's optional objective; it may reference a session-local or Docket task. |
| Pair task settlement | Docket task settlement | durable task outcome | A terminal user/model declaration appends the canonical outcome and task link. In Pair it is not gated on commit creation, publication, or artifact finalization. |
| Delivery | runtime delivery coordinator + work surface | task/job delivery attempt and evidence | `commit_policy` schedules delivery after Pair settlement (`none`, `per_task`, `per_job`). Delivery can be pending, committed/finalized, skipped, or failed; it is retryable and cannot reopen the settled Pair task. Explicit Work/release policy may gate a job delivery projection. |
| Work assignment | WorkRun + Docket | durable Work-run Job boundary, resolved per run | An explicit assigned Job is Work's execution boundary. Human job/task reads, checkout and task activation recheck owner/admin/`BearVisible` access; Work checkout of a shared Job needs its exact run-minted token. Optional in-run task progress remains constrained to that Job. |
| Workflow state | current-turn state compiler | current turn | Inputs compiled into `TurnAuthority`; derived operational context is advisory. |
| Permission policy | Den policy resolver + descriptors | current turn/tool call | Resolves Ask/Plan/Write, tool classes, approvals, and armature routes before `TurnAuthority`/routing consume them. |
| Turn/run lifecycle | runtime turn controller + atomic run finisher | run/turn | Accepted/running/waiting states use ordinary transitions. Completed/failed/cancelled state, obligation/step settlement, and the terminal BearWire event are one atomic finish operation. |
| Obligations | obligation coordinator | per tool/permission/human wait | Client/tool/approval waits; blocks only while open. |
| Loop control/budgets | agent loop controller | run/turn | Budgets, checkpoints, KO/failure signals, context pressure. |
| Recovery/error | runtime recovery logic | run/turn/session | Retry, resume, terminal outcome, late-result handling. |
| Derived views: model request, prompt/context, memory, compaction, UI/surface caches | projection/assembler owners | request/turn/session bounded | Non-authoritative views only; cannot create permissions, task selection, obligations, lifecycle, or completion. |
| Work surface/sandbox | Docket/work execution | job/run/workspace | Execution materialization; not a Pair task selection by itself. |

## Axis inventory

### Bear identity and involved users

Representative state:

- Bear id and slug;
- bound trust-profile/stance config;
- involved users and membership roles;
- source user/session ids;
- compiled prompt/config binding;
- per-Bear memory-store binding.

Invariants:

- Bear identity is durable and not changed by governance, task selection, or client reconnects.
- User access is resolved structurally, not by parsing rendered labels.
- Trust-profile changes apply to new turns/templates; they do not silently launder memory/tool scope mid-run.
- Bound memory visibility is derived from the canonical conversation/Work run and hat binding plus the per-Bear SQLite record scope. The binding does not create tools, approvals, or outbound authority. Unbound sessions still use the legacy profile-local memory path.

### Conversation

Representative state:

- canonical conversation id;
- pending/external client conversation id during materialization;
- title and title timestamp;
- archived vs active;
- transcript and message persistence;
- Pair's persisted session current-task reference, when selected;
- selected model state;
- compaction and prompt-memory state;
- review/reflection candidate state.

Invariants:

- A conversation is the durable user-visible chat container. A session is only a live binding to it. Human conversation access is checked against the canonical created-by human; ownerless historical conversations remain admin-only. Browser default chats have per-user canonical IDs and newly selected conversations materialize a durable owner-scoped ID before transcript persistence.
- Archived conversations should not accept ordinary new turns unless explicitly restored or unarchived.
- Title updates are non-blocking structured updates; they must not create model-visible obligations or force continuation.
- Conversation task selection must be changed through the canonical persisted current-task reference, not stale prompt text or session cache. A job task's visibility is independently rechecked for the authenticated human before checkout, selection, or focused execution; session attachment by itself is not authority.

### Session and client binding

Representative state:

- client session id;
- client kind/channel;
- conversation id and resolved conversation id;
- current client mode/projection;
- cwd and workspace roots;
- armature availability;
- advertised client tools and direct tool support;
- connected/disconnected state;
- active turn registration.

Common client modes:

- `ask`;
- `plan`;
- `write`.

A current task is a separate projection, not a permission preset or client mode.

Invariants:

- A stale session cache must never resurrect a cleared Pair current task or Work assignment.

### Trust profile

Values:

- `chat`;
- `pair`;
- `curate`;
- `work`;
- `watch`.

Owned semantics:

- memory read/write scope;
- default tool roster;
- approval/autonomy defaults;
- cross-role visibility;
- default prompt slice.

Invariants:

- Governance does not change trust profile.
- A `pair` run that continues autonomously does not become `work`.
- A `work` run under observation does not gain `pair` memory scope.

### Governance

Values:

- `interactive` — human present, live collaboration;
- `grace` — client recently disconnected, bounded continuation/cleanup;
- `autonomous_continuation` — human absent, runtime drives approved task-oriented or assigned Work execution;
- `observational` — human present read-only inspection;
- `frozen` — panic, handoff, or cancelled checkpoint awaiting disposition.

Expected transitions:

```text
interactive -> grace                    client disconnected
grace -> autonomous_continuation         grace timeout
autonomous_continuation -> observational inspection opened
observational -> autonomous_continuation inspection released
any active -> frozen                     panic, handoff, cancellation
frozen -> interactive                    explicit resume/new workspace session
```

Invariants:

- Governance changes continuation pressure, not memory or trust scope.
- `observational` cannot own mutation or continuation.
- `frozen` cannot continue the model/tool loop.
- Autonomous continuation without a resolved Pair current task is suspicious for interactive profiles; Work autonomous continuation requires its explicit Job assignment.

### Current task, Work assignment, Docket, and task orientation

The current architecture separates Pair's optional objective from Work's explicit execution boundary:

```text
Pair session -> validated persisted current task -> task-oriented behavior
Work run     -> assigned Docket Job              -> work-execution behavior
```

- `client_sessions.current_task_id` is the sole Pair selection authority. It may identify a session-local task or a Docket task reference. A valid explicit selection has precedence over any cached task-list or implicit-next-task projection; no cached task list or implicit next task may manufacture it.
- A Pair session has at most one session-connected root task. **Planning mode is derived solely from that root's `draft` status**; it is not an independent persisted flag. While the root is draft, every descendant is non-executable even if its own status is otherwise ready/pending, Pair must have no execution current task, and no Pair execution run is created. Selecting a draft task or a descendant of a draft ancestor for execution is rejected.
- An explicit execution current-task selection is Pair's start signal once its root is non-draft. It drives execution focus; a Pair execution run is created or resumed to record the attempt, budget slices, and checkpoint/resume history. The run does not grant execution authority. A non-draft session-connected current task must have that persisted run before execution begins. `RunPaused` requires its persisted run ID; absence of one is execution-initialization failure, not a valid runless pause. A real budget boundary pauses the run while preserving the selected task, so the controller resumes a subsequent slice rather than requiring the user to say "continue." UI navigation focus is client-local and must not mutate this selection.
- Starting Pair execution is one Den-owned command, not a client-orchestrated sequence of "select, create run, acquire controller, then invoke." The command resolves the persisted selection, creates or resumes its execution run, acquires a fenced Docket attempt, and schedules the first loop slice under one serialized transition. It returns success only when the resulting authoritative projection is executable and scheduled. A failure must leave a typed durable outcome; it must not leave a nominally running run with an inactive controller and no attempt.
- `/focus` is an edge spelling of that command. It both assigns Docket loop control to the selected executable Pair task and starts the loop; a second "please start" turn is neither required nor a supported part of the protocol. Later ordinary user steering may interrupt that execution control, but it does not clear the persisted current-task selection.
- Pair execution status is reduced by one authoritative Den-side aggregate from persisted selection, execution run, fenced attempt/lease, scheduler/controller ownership, and any open obligation. Components and clients must not independently infer `running` from only one of those records. Docket lifecycle, session activity, and runtime controller state are inputs to the aggregate, not peer authorities.

The aggregate exposes a small derived state such as `unfocused`, `starting`, `running`, `waiting`, `paused`, `blocked`, or `terminal`, plus a typed reason and the identities/fences from which it was reduced. Impossible combinations are invariant violations, not additional steady states. At minimum:

```text
PairExecution=running
  => executable persisted selection
  && active persisted execution run
  && current fenced attempt/lease
  && live or durably queued controller ownership

start command accepted
  => PairExecution in {running, waiting}
  || typed durable start failure
```

Reconciliation may repair an expired lease or settle an abandoned attempt, but it must use the same aggregate and transition service as the normal command. It must not mask a split state by rewriting only the UI projection.
- A Work run's explicit assigned Job is the sole Work execution boundary. Its optional `executing_task_id` is in-run progress within that Job, never a replacement assignment and never sourced from Pair.
- Pair task orientation is `freeform` without a resolved current task and task-oriented with one. A Docket-backed Pair task remains task-oriented; it does not acquire Work execution authority. Work derives execution orientation only from its assigned Job.

Task selection and creation are explicit mutations. Apparent user objective redirection is confirmation-first: Pair proposes the change and waits for confirmation rather than silently select, clear, replace, complete, or create a task. Selecting or creating a Pair session task updates the conversation title; clearing it does not erase the title.

Docket supplies durable task trees, job state, journals, recovery, and isolated background execution when explicitly requested. It is not required for ordinary Pair work. `dispatch_work` is isolated-sandbox execution, and dispatch/delegation does not change Pair's current task.

ADR-0056 execution records remain relevant to Work and durable Docket runs:

- `docket_cursors` are per-client browsing viewports (`job_id`, optional `task_id`). They never select, claim, resume, or complete work.
- `bear_task_run_state` is the sole execution-position authority. A partial unique index permits at most one `in_progress` task per run.
- `docket_conversation_bindings` owns the preferred durable transcript container for a task; `docket_conversation_binding_runs` preserves which conversation each run used.
- `docket_routing_decisions` records immutable, idempotent placement decisions. Durable turn attempts and result rollups reference those decisions/run-task identities.
- Symbolic execution profiles (`economy`, `balanced`, `advanced`) are selected deterministically from typed task difficulty. Normalized eligible failures may advance one tier at a time, with an explicit three-attempt ceiling; concrete provider/model selection remains outside task state.
- `docket_turn_attempts` records profile provenance, normalized terminal outcome, evidence, latency, and optional cost attribution. The supervisor—not the model—derives retry, escalation, or handoff.
- The canonical run-diagnostics projection combines authoritative run/task state, routing decisions, attempts, rollups, and attention into one semantic model rendered by the work-run UI. Browsing cursors are never consulted for execution position.
- Paused work runs remain active and non-terminal. Pause/resume transitions use compare-and-set semantics, and task mutation at an active execution boundary requires the run to be paused.
- Work dispatch provisions an isolated sandbox. It does not select or mutate an attached local workspace as part of Docket dispatch.
- BearWire permission obligations remain the sole command-approval authority for attached unattended work. Approval-required work is projected as a durable wait; only a current accepted outcome may clear it, and stale or duplicate outcomes cannot resume a run.
- Attached-session disconnect uses compare-and-set lifecycle transitions: an affected active run auto-pauses and records a bounded deadline; reconnect clears disconnect state but does not implicitly resume. An overdue disconnect terminalizes exactly once with normalized `armature_disconnect_timeout` evidence and a failure rollup.
- Recovery eligibility derives from that typed persisted timeout outcome. One authorized action creates at most one replacement active run from durable incomplete task state, preserves source evidence, and leaves the terminal source run immutable.
- The canonical diagnostics projection owns attachment, permission, disconnect, timeout, and recovery semantics for conversation and work-run UI projections; labels and UI text are not recovery authority.
- Job completion is criteria-gated: no actionable task is not evidence of completion. If required work remains and nothing is actionable, the derived job state is `blocked`, with one open attention record per run.

Representative state:

- Pair persisted current-task reference and resolved current task;
- Work assigned Job and optional in-run executing task;
- session task-list projection;
- task orientation (`freeform`, task-oriented, work-execution);
- Job and task status;
- ordered sibling/current-task ACP plan projection;
- handoff/sync state.

Job statuses:

- `draft`, `ready`, `running`, `blocked`, `completed`, `cancelled`, `archived`.

Task statuses:

- `pending`, `in_progress`, `done`, `blocked`, `cancelled`.

Continuation inputs should be treated as:

| Source | May affect continuation? | Notes |
| --- | --- | --- |
| Pair resolved current task | yes, for Pair task-oriented behavior | Must originate in a valid persisted explicit selection of a non-draft executable task. It is an objective, not authority: Docket must also authorize a matching fenced Pair execution attempt. Planning mode is derived from the sole session-connected root task being `draft`; it is not a separate continuation authority. |
| Work assigned Job | yes, for Work execution behavior | The WorkRun assignment bounds the task tree; each autonomous turn also requires a matching Docket-authorized Work execution attempt. |
| Docket execution attempt | yes, for autonomous Pair/Work continuation | The sole durable authorization for a concrete owner/task/fence. A checkpoint acknowledgement is evidence only and cannot itself authorize a further turn. |
| Legacy execution record | no | Removed in pre-release; it cannot select, authorize, or resume continuation. |
| Runtime/session cached task list or client/prompt projection | no | Display/context only; cannot manufacture selection, assignment, or continuation. |
| None | no | Completion is allowed when no obligations/budgets block. |

Critical invariant:

```text
Pair.ResolvedCurrentTask=None && Work.WorkAssignment=None
  => task state must not by itself force continuation
```

Do not let task orientation, legacy execution context, or stale activity projection force continuation. Only the applicable resolved current task or explicit Work assignment may do so.

### Docket routed-turn contract

Every autonomous dispatch, continuation, rollup, and routed user turn follows the
same durable claim-and-commit protocol. This is the execution contract for
ADR-0056 and is deliberately separate from cursor and UI state.

#### Typed records and ownership

| Record / state | Owner | Required contents | Rule |
| --- | --- | --- | --- |
| Routing decision | router transaction | intent, policy version, matched rule, normalized policy inputs, execution surface and source, profile provenance, stable turn idempotency key | Immutable and created or reused before invocation. |
| Claim / reservation | dispatcher transaction | expected job/run/task versions, owner, lease expiry, turn key, decision and binding references | The sole authority to invoke work for an eligible task position. |
| Turn attempt | attempt ledger | reservation identity, lifecycle, timestamps, observed boundary/cause/code, normalized outcome when known, evidence, last successful activity, failing boundary, criteria evidence, disposition, recovery action, synthetic provenance | Created before provider/model work; append-only after settlement except lifecycle timestamps and lease heartbeat. |
| Replay activity | canonical transcript stream | attempt and task correlation, sequence/idempotency key, model/provider/tool event payload | Incremental events share the canonical replay stream; no second raw-log format. |
| Task/run projection | Docket task/run reducer | execution state and derived job status | Derived only from settled attempt/disposition, task events, and explicit run control; it cannot infer a new failure cause. |
| Notification outbox | same finalization transaction | attention event, authorized recipient/resource scope, delivery dedupe key, delivery state and acknowledgement | Delivery workers present and retry it; they do not decide failure semantics. |

`AttemptLifecycle = reserved | executing | settled | abandoned` is not an
outcome. `ObservedBoundary` records where progress stopped; `NormalizedOutcome`
records a known classified result (or is absent); `SupervisorDisposition` selects
what happens next; and task/run state is the reduction of those records. A
provider disconnect, watchdog expiry, process loss, or continuation loss must
append synthetic provenance rather than fabricate model text or a provider
terminal result.

The pre-v1 Phase 0/1 tables are an incomplete predecessor, not a competing
contract: their `running | terminal` attempt state, unleased routing decision,
and untyped JSON evidence do not satisfy the contract above. Increment 1 must
replace or migrate those shapes without losing readable history.

| Current predecessor | v1 contract gap | Required migration direction |
| --- | --- | --- |
| `docket_routing_decisions` | No claim owner/lease, expected versions, policy-input snapshot, or surface/profile provenance source | Add a reservation keyed by the stable turn key; preserve decisions as immutable forensic records. |
| `docket_turn_attempts` | `running | terminal` conflates lifecycle and settlement; it lacks observed boundary, last successful activity, failing boundary, criteria evidence, recovery action, and synthetic provenance | Introduce typed lifecycle and distinct outcome/disposition/evidence fields; backfill legacy terminal rows as settled history. |
| `docket_attention` | No transactional outbox, recipient/resource authorization, delivery dedupe, retry, or acknowledgement | Keep attention as domain event and add an outbox owned by the finalization transaction. |
| direct work-run terminal transitions | May bypass a reserved attempt and shared failure projection | Route through claim and supervisor finalization; retain compatibility projections only. |

#### Claim, invocation, and finalization

1. The dispatcher selects an eligible serialized task and supplies expected
   job/run/task versions plus an owner and stable turn idempotency key.
2. In one transaction it compares those versions, claims the position, reserves
   or reuses the key, and creates or reuses the routing decision and scoped
   conversation binding. A loser performs no model or tool invocation.
3. The claimant records the `reserved` attempt, then moves it to `executing` and
   renews its lease while appending replay activity.
4. Finalization compares the live claim owner, lease, attempt identity, and
   expected versions. On success it atomically settles the attempt, appends the
   task event/rollup, updates task/run projection, and writes any attention
   outbox item. A late or stale result remains forensic history but cannot settle
   a replacement attempt.
5. A recovery worker may abandon only an expired claim with positive absence of
   liveness. Uncertain continuation loss is `stalled`/`await_recovery`, not a
   provider failure. Concurrent sweepers converge through the same compare-and-
   set finalization.

Legal lifecycle edges are:

```text
claim absent -> reserved -> executing -> settled
                         \-> abandoned
reserved/executing -> abandoned          expired lease + no liveness proof
abandoned -> reserved                    fresh router claim for a new attempt
settled/abandoned -/-> executing         terminal attempt is immutable
```

Only the claim transaction may create a reservation; only its current claimant
may append execution activity; only the supervisor finalizer may settle an
attempt and change task/run execution state; only the reducer derives job state;
and only the delivery worker changes notification delivery/acknowledgement. UI,
cursors, and model-authored fields may supply evidence or requests but own none
of these transitions.

#### Supervisor reduction and failure truth

The supervisor validates task completion against criteria. Model text, including
“cannot continue” or a requested stop, is evidence only. Its bounded
`SupervisorDisposition` is one of `complete`, `retry`, `escalate_profile`,
`handoff`, `pause`, `await_recovery`, or `terminal_failure`; retries return to
step 1 and always create a new attempt.

The job reducer must use this table rather than treating “no actionable task” as
success:

| Required task condition | Job projection |
| --- | --- |
| all required tasks criteria-complete | completed |
| one task executing or eligible pending | running / ready |
| blocked, handoff, paused, or awaiting recovery | blocked / waiting |
| stalled or exhausted terminal failure | failed |
| run explicitly stopped with incomplete required work | stopped / cancelled, never completed |

The same normalized outcome/evidence record is projected into conversation,
task activity, job workspace, notifications, and forensic diagnostics. Concise
views may summarize it but must retain or link the last successful activity,
failing boundary, cause/code, disposition, recovery action, and authorized
resources. Clients do not independently classify an error.

#### Notification outbox invariants

Finalization writes the durable attention event and outbox entry in the same
transaction as the failure rollup and task/run transition. The outbox owns a
stable per-channel dedupe key, retry state, recipient and referenced-resource
authorization snapshot, and acknowledgement state. Presentation may coalesce
entries but must retain the durable event link and enforce current resource
authorization before rendering any transcript, tool output, work surface, or
run-control reference.

### Current-turn workflow state

Canonical domains:

1. `workplan` — proposal and approval lane;
2. `activity` — live tactical progress lane;
3. `memory` — durable semantic capture lane;
4. `execution` — current-turn capability/effect lane.

Representative fields:

- `state_authority`;
- `state_version`;
- `permission_mode`;
- `tool_classes`;
- `workplan.state`;
- `workplan.approval_status`;
- `activity.status`;
- `activity.current_item`;
- `execution.execution_unlocked`;
- `memory.active_plan_write_allowed`.

Derived `operational_context` values may include:

- `clarify`;
- `plan`;
- `execute`;
- `await_approval`;
- `summarize`;
- `recover_context`;
- `handoff`.

Invariants:

- Current-turn workflow state contributes typed inputs to `TurnAuthority`; it is
  not a separate mutation authority after compilation.
- `operational_context` is advisory.
- If execution is locked, no derived context may imply mutation is allowed.
- Prior-turn state and prompt reminders must not override current-turn authority.

### Workplan and plan-mode gate

Representative states:

- absent/inactive;
- `active` / drafting;
- `submitted` / awaiting approval;
- `approved`;
- `rejected` / cancelled.

Derived policy shape:

| Client mode / plan state | Effective behavior |
| --- | --- |
| Ask, no plan | read-only |
| Plan, no plan | read-only |
| Write, no plan | write-capable subject to policy/descriptors |
| active/submitted plan | read-only until approved |
| approved plan | execution may unlock subject to policy/descriptors |
| rejected plan | read-only/cancelled |

Invariants:

- Workplan approval is not the same as individual tool approval.
- Submitted/drafting workplans do not unlock mutation.
- Rejected workplans cannot preserve write capability through stale mode state.

### Permissions, approvals, and tool routing

Representative state:

- permission mode: Ask/Plan/Write;
- allowed and denied tool classes;
- tool enablement: read-only vs all enabled classes;
- approval-required policy;
- active approval ids;
- approval decisions and scopes;
- tool execution route: Den server, adapter-local, unsupported.

Tool classes:

- `read_only`;
- `workspace_mutation`;
- `execution`;
- `browser`.

Invariants:

- Tool availability comes from resolved policy and descriptors, not prompt labels.
- Execution ownership is resolved once by the descriptor-owned `ToolExecutionOwner` resolver; projection, initial streams, continuations, recovery, and armatures consume that result rather than matching tool names independently.
- Adapter-local tools that require a client response must create obligations.
- Den-server tools must not wait for client tool results; Den-owned approval requests may wait only on a typed permission obligation.
- Unsupported tools fail or stop explicitly; they do not wait forever.

### Tool calls and obligations

Turn phases:

- `Created`;
- `Streaming`;
- `WaitingForObligations`;
- `ContinuingAfterTool`;
- `Cancelling`;
- `Terminal`.

Obligation kinds:

- `ToolResult`;
- `PermissionDecision`;
- `HumanInput`;
- `ResourceBinding`;
- `HandoffDecision`.

Obligation states:

- requested/waiting;
- claimed/running with a current renewable lease (armature-local `ToolResult` only);
- result received;
- continued;
- failed;
- timed out;
- expired with `outcome_unknown` after execution was claimed;
- cancelled;
- late ignored.

Lease transitions for armature-local tool execution:

```text
waiting --claim--> claimed/running
claimed/running --renew--> claimed/running
claimed/running --result--> result received
claimed/running --lease expiry--> failed/outcome_unknown
```

Invariants:

- Open obligations block turn completion/continuation decisions; settled obligations do not.
- Terminal turns cannot be reopened by late client/tool/permission results.
- Waiting states require a matching open obligation.
- Exactly one claimant may acquire execution authority for an obligation; local task registries are not authority.
- Claim, renewal, result, cancellation, and expiry match run, session, obligation, tool call, attempt token, responder, and open state; `turn_step_id` joins the fence once available.
- Den's database clock owns lease expiry. Renewal is idempotent and cannot revive a settled or expired obligation.
- Conditional transitions give result, cancellation, renewal, and expiry races one canonical winner.
- A stale or reconnecting armature without the current attempt token may inspect `run.state` but cannot renew, submit, or re-execute.
- A claimed command whose result is not confirmed expires as `outcome_unknown`; recovery never automatically retries it.

### Turn and run lifecycle

Representative states:

- `Accepted`;
- `Running`;
- `WaitingForClient`;
- `Continuing`;
- `Completed`;
- `Failed`;
- `Cancelled`.

Terminal outcomes/reasons include:

- ok/end-turn/stream-complete;
- failed/stream-error/tool-timeout/unsupported-tool;
- cancelled;
- orphaned-requires-approval;
- recovered;
- needs-new-session.

Invariants:

- At most one active turn per runtime ownership key.
- Terminal run states must not have open obligations or active turn steps.
- A terminal run state must have a matching durable `run.completed`, `run.failed`, or `run.cancelled` event committed in the same transaction.
- Ordinary `transition_run` calls are nonterminal-only; terminal fixtures and production outcomes use the atomic finisher.
- `WaitingForClient` requires at least one open client obligation.
- The specific blocking reason (`tool_result`, `permission_decision`, human/resource/handoff, or `multiple`) is derived from open obligations and is not duplicated in run state.

### Completion and continuation

`TurnCompletionDecision` is an ephemeral, pure policy result consumed within the
runtime stream. It is not persisted as run state, a completion reason, or a peer
lifecycle authority. Its variants are:

- complete;
- continue.

Representative complete reasons:

- no applicable current task or Work assignment;
- Pair current task complete finalization drain;
- Pair current task or Work-assigned work complete or terminally blocked;
- repeated terminal objection.

Representative continue reasons:

- Pair current task or Work-assigned work remains;
- runtime limit is not proof of task completion.

Invariants:

- With neither a resolved Pair current task nor a Work assignment, normal final answers may complete the turn when no obligations block.
- Incomplete actionable work may force continuation only through the applicable resolved Pair current task or explicit Work assignment.
- `TurnCompletionDecision` may select continuation or emit an internal turn-completed semantic event, but the only durable outcome is the atomic `finish_run_with_bearwire_event` transition and matching BearWire terminal event.
- Runtime limits, budget pressure, progress reports, tool failure, assistant text, and stream EOF are not run completion.
- ACP ends a turn only after a durable run terminal event; tool-level terminal updates are not run terminal events.
- Den-owned tool requests continue draining the runtime stream; armature-owned requests and typed approval waits form client-wait boundaries.
- Repeated task-gate rejection should stop with a human-review blocker instead of infinite continuation.

### Loop control, budgets, and checkpoints

Control levels:

- `light`;
- `standard`;
- `careful`;
- `strict`.

Budget dimensions:

- wall-clock;
- total tool calls;
- tool-class calls;
- context/token budget;
- consecutive failures;
- repeated same tool/signature;
- emergency hard steps;
- post-mutation verification window.

Tool budget classes:

- `read`, `search`, `fetch`, `execute`, `write`, `destructive`, `other`.

Invariants:

- Loop control may checkpoint, warn, stop, or nudge; it does not mutate task state by itself.
- Checkpoint prose cannot satisfy Docket/task-list gates.
- Budget exhaustion can stop a run, but it is not proof that a Pair current task or Work-assigned work is complete.

### Error and recovery

Representative state:

- consecutive tool failures;
- same-signature KO counts;
- stale approval recovery;
- interrupted run recovery attempts;
- retry final delivery;
- overflow/context recovery;
- orphaned approval detection;
- unsupported tool handling;
- late result ignored counts.

Invariants:

- Recovery may add context or retry delivery, but it must not mutate Pair task selection, Work assignment, task, or permission state except through explicit typed transitions.
- Needs-new-session and frozen states are explicit terminal/recovery outcomes, not hidden active waits.

### Model selection

Representative state:

- selection mode: auto or explicit;
- requested model;
- selected model;
- effective model;
- model options/capability profile;
- per-model loop-control default.

Invariants:

- Model selection affects provider request and model capability defaults.
- Model selection must not directly change task selection, Work assignment, governance, or permission mode.
- Mid-active-turn model changes apply at a clear turn boundary unless explicitly designed otherwise.

### Prompt, context, memory projection, and compaction

Representative state:

- compiled prompt/config hash;
- projected key memory;
- prompt-memory blocks;
- transcript window;
- compaction state and diagnostic;
- context budget report;
- runtime supplements and reminders.

Invariants:

- Prompt/context projection is not authority.
- Compaction must preserve unresolved obligations and resolved current-task/Work-assignment context accurately, but must not create either.
- Old prompt text cannot preserve task selection, Work assignment, permission, or governance after canonical state changes.

### Work surface, sandbox, and external execution

Representative state:

- work surface ref;
- work branch and commit policy;
- workspace/sandbox/session id;
- work run state;
- upstream auth context;
- egress policy;
- checkpoint/publish branch.

Work run states:

- `queued`, `claimed`, `provisioning`, `running`, `reporting`, `succeeded`, `blocked`, `failed`, `cancelled`, `timed_out`.

Invariants:

- Work surface state is not a Pair current task by itself.
- A run may have a workspace without changing the Pair conversation's current task.
- Outbound auth follows `RunAuthContext`, never client labels or trust-profile names alone.

## High-value validity matrices

### Trust profile × governance × task/execution authority

| Trust profile | Governance | Required task/execution authority | Notes |
| --- | --- | --- | --- |
| `chat` | `interactive` | none | ordinary chat |
| `pair` | `interactive` | optional resolved current task | normal Pair collaboration |
| `pair` | `autonomous_continuation` | resolved current task required for task-driven continuation | no silent profile flip to `work` |
| `work` | `autonomous_continuation` | explicit assigned Job required | normal Work execution |
| `watch` | `observational` | none | inspect/observe only |
| any | `frozen` | none active | no continuation/mutation |

Invalid or suspicious combinations should be rejected before prompt assembly or surfaced as explicit diagnostics.

### Continuation input × completion policy

| Input | May pass task state to completion policy? | May force continuation? |
| --- | --- | --- |
| Pair resolved current task | yes, for Pair | yes, within Pair task-oriented behavior |
| Work explicit assigned Job | yes, for Work | yes, within the assigned Job tree |
| legacy execution record | no | no |
| stale session cache / cached task list / client projection | no | no |
| none | no | no |

### Run state × obligations

| Run state | Required obligation condition |
| --- | --- |
| Running | no blocking awaited obligation |
| WaitingForClient | at least one open client obligation; blocking reason derived from obligation responder actions; a claimed/running tool obligation remains open while its lease is current |
| Completed/Failed/Cancelled | no open obligations or active steps; exactly one matching terminal BearWire event committed atomically; late results and renewals ignored |

## Test obligations

Add or update tests whenever behavior crosses axes. Keep these seam checks permanently mapped to the reduced authority model:

| Seam | Current executable check |
| --- | --- |
| submitted/drafting plan cannot unlock mutation | `den-core` `submitted_plan_keeps_write_tools_locked`; `turn_authority_is_single_derived_permission_surface` |
| prompt/client projection labels cannot expand authority | `den-core` `turn_authority_ignores_client_policy_projection_labels`; `turn_authority_has_no_prompt_or_compaction_authority_input` |
| model choice is not an authority source | `den-core` `turn_authority_has_no_model_choice_authority_input`; loop-control tests may still cover model-default budget/checkpoint behavior |
| stale cached task list/client projection cannot manufacture selection, assignment, or continuation | `den-runtime` current-task and Work-assignment resolver tests; completion-policy cache-clearing tests |
| Pair Docket-backed current task remains task-oriented | `den-runtime` `pair_does_not_treat_legacy_execution_as_work_assignment` |
| terminal transitions atomically own run state, obligation/step closure, and terminal event | `den-bearwire` completion/cancellation/expiry/failure persistence tests; `den-runtime` `terminal_turn_run_cannot_be_reopened_or_overwritten`; `client_obligation_coordinator_contract` late-result tests |
| Job-scoped Work Run owns one sandbox/workspace/session for its task tree | Job-dispatch contract tests plus provisioning root checks; task progress remains in `bear_task_run_state` |

Baseline scenarios to preserve:

1. Pair with no resolved current task allows final answers even with a stale cached task list;
2. changing client mode does not silently select, clear, or replace Pair's current task;
3. a valid Pair current task with incomplete actionable work may drive Pair task-oriented continuation;
4. a completed, terminally blocked, invalid, or cleared Pair task allows normal finalization;
5. `work` + autonomous continuation without an explicit assigned Job is invalid;
6. a Docket-backed Pair current task remains task-oriented and never becomes a Work assignment;
7. submitted/drafting plan keeps mutation tools locked;
8. terminal run with open obligation is invalid;
9. late tool/permission result after terminal run is ignored;
10. model selection changes do not mutate task selection, Work assignment, governance, or permissions;
11. prompt/compaction/client projection cannot create task selection, Work assignment, or obligations;
12. Den-owned tool requests do not stop initial or continuation stream consumption unless a typed approval obligation waits on the client;
13. terminal run state, settled obligations/steps, and one terminal BearWire event commit or roll back together.

Prefer pure derivation/validation tests first, then narrow persistence/replay tests at state seams.

## Related documents

- [Den runtime](den-runtime.md)
- [Workflow state overview](workflow-state-overview.md)
- [Interactive stances and role axes](interactive-stances-and-role-axes.md)
- [ACP runtime contract](acp-runtime-contract.md)
- [Non-blocking structured updates](non-blocking-structured-updates.md)
- [Tasks and autonomy](tasks-and-autonomy.md)
- [ADR-0027: Workflow-state ontology](../decisions/adr-0027-workflow-state-ontology.md)
- [ADR-0039: Trust profiles and governance states](../decisions/adr-0039-trust-profiles-and-governance.md)
- [ADR-0045: Session task lists as Docket checkouts and working projections](../decisions/adr-0045-session-task-lists-and-docket-checkout.md)
- [ADR-0048: Core turn/client-obligation coordinator](../decisions/adr-0048-core-turn-client-obligation-coordinator.md)
- [ADR-0050: Agent Loop Control, Adaptive Budgets, and Runtime Checkpoints](../decisions/adr-0050-agent-loop-control-adaptive-budgets-and-runtime-checkpoints.md)
