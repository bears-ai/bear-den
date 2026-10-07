# BearWire compatibility

Den and Armature may be released from separate repositories. Work sandboxes therefore negotiate protocol behavior instead of comparing Git revisions or requiring the newest image.

Armature sends a `CompatibilityManifest` with `work.checkout`. Den validates the protocol generation and required capabilities before binding the session or mutating run/task state. An incompatible image fails during provisioning; it must never begin a task and fail later in the protocol.

## Current local-branch startup boundary (2026-10-06)

This section describes code and regression coverage on the local branch, not deployed/shipped compatibility or live-provider validation. See the maintained [BearWire and ACP topic](../../../../docs/topics/bearwire-acp.md) for final test evidence and the successful production/musl build of the separate local image `bears-den-armature-validation:local`. That image was not deployed and no service was restarted; no real-model/provider smoke was run.

There are two distinct negotiations:

| Exchange | Who checks it | Purpose |
|---|---|---|
| Den BearWire `initialize` → `capabilities` | Armature | Does Den support the startup semantics the client needs? |
| Armature `CompatibilityManifest` in `work.checkout` | Den | Does the sandbox armature support Den's required Work protocol behavior? |

Den currently advertises `session_access: true` and `expected_work_source: true` in the first exchange. They are not substitutes for the checkout manifest, and a BearWire v1 version number or successful token check alone proves neither semantic boundary.

### Ordinary ACP session access

Den's session projection includes typed `access: { state, may_select_hat }`, where `state` is `awaiting_hat`, `executable`, or `read_only`. It distinguishes an owner-bound pending session from executable canonical history and authorized read-only inspection. The armature requires this projection when creating/restoring sessions and before forwarding a prompt after `session.open`; missing, malformed, mismatched-client, or inconsistent projections fail closed.

Unknown explicitly requested history must fail rather than become a fresh session. Reconnect and direct `run.start` cannot replace a read-only session's transcript with a different owned or provisional source; stored and resolved bindings are checked independently. Load/resume preserves an ongoing initial prompt/selection reservation, rejects busy restore, and fences new interactions while restoring. Generation checks reject stale fetched history/state before projection or cache/eligibility replacement.

Ordinary token preflight does not require the additive `initialize.capabilities` flags. Thus an old Den can pass token preflight yet fail lifecycle decoding because it does not return access. Upgrade Den to the matching projection; do not infer execution authority from an ID prefix or fall back to legacy `/acp/**`.

### Atomic source publication is a separate boundary

Pending-to-canonical source creation and resolved-binding publication share the client-session row lock and transaction, including the owner/hat binding. Direct initial starts without a row use a separate short transaction-scoped publication lock, independent of focused-task locks. A competing command adopts the winning canonical source or fails; metadata compare-and-set guards preserve the latest resolved source instead of overwriting it through a stale pending alias.

This makes **source publication** atomic. It does not hold an inference lease or strengthen the exact Work preflight/instantaneous-revocation guarantees below.

### Headless exact Work source

Before `work.checkout`, headless armature requires BearWire v1 and the boolean `initialize.capabilities.expected_work_source: true`. An absent, false, or malformed flag stops startup before checkout/session/run mutation: an older server might otherwise silently ignore an unknown request field.

After checkout, the armature requires:

- `ok: true` and `gate.status: allowed` with `gate.binding.kind: work_run`;
- the exact requested Work-run UUID in both `work_run_id` and `gate.binding.work_run_id`;
- a non-nil `execution_attempt_id`, positive integer `execution_attempt_fence_epoch`, and non-empty prompt.

It carries these values as a typed, top-level `expected_work_source` object on **both** `session.open` and `run.start`:

| Field | Wire type | Source |
|---|---|---|
| `work_run_id` | UUID string | Exact checked-out run |
| `execution_attempt_id` | UUID string | Checkout's execution attempt |
| `fence_epoch` | Integer | Checkout's `execution_attempt_fence_epoch` |

This is an expected identity, never a grant or transcript instruction. Den validates it against the current live session/run, actor, Job/hat/surface authorization, running attempt, and execution gate/fence. A missing/replaced association, revoked authority, cancelled run, released/replaced attempt, or stale fence detected at a recheck must fail rather than fall back to an ordinary IDE default. Recovery retains the original supplied expectation; it does not mint a new fence.

Transcript authority is checked separately: an existing Work transcript must be owned by the actor, active, and not marked archived. A valid Job/hat/run or Bear-admin read access does not authorize execution against another owner's, null-owner, or archived transcript. Exact-source startup refuses unproven reuse of an already-active turn, including a Pair turn or a turn from an older Work source/attempt/fence; the current session association cannot prove the original admission.

**The guarantee is startup preflight with rechecks, not an atomic inference lease.** These checks do not hold attempt/hat authority across inference or guarantee instantaneous attempt/hat revocation. A change after a successful check may only be observed at a later boundary; fail-closed detection is not a continuous revocation guarantee.

The change adds no migrations or dependencies, but its shared typed protocol still requires compatible Den/armature versions. **Upgrade Den and armature together** for exact-source startup. New armature refuses headless startup against Den without the advertised capability; an older armature cannot be assumed to enforce the new expectation. Keep incompatible Work startup disabled until the pair is compatible. Do not bypass negotiation, synthesize attempt/fence values, or retry as ordinary Pair work to make a failed checkout run.

## Changing the protocol

- Capability wire names are permanent. Never reuse a name or change its meaning.
- Additive behavior gets a new capability.
- Breaking semantic changes get a new capability or protocol generation.
- Den should require a capability only when it cannot operate safely without it. Supporting an optional feature is not a reason to invalidate old images.
- Every advertised capability needs a runnable conformance check covering the actual exchange, not only manifest serialization.

Adding a required capability is a compatibility boundary: deployed sandbox images may be older, so release a compatible Armature image before Den relies on it. Unrelated changes to either repository do not require sandbox rebuilds.

Build versions and Git revisions are useful diagnostic evidence, but are not compatibility gates.
