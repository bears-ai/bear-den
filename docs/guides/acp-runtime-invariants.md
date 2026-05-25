# ACP Runtime Invariants for Letta Migration

## Purpose

This note captures the current ACP / `pair` runtime invariants that the Letta migration must preserve, especially as Den moves toward provider-neutral runtime abstractions and optional-worker deployment patterns.

It is intentionally focused on operational/runtime behavior rather than broader product architecture.

## Core invariants

### 1. Active-turn exclusivity is mandatory

For a given ACP session, only one active turn may own the runtime at a time.

Implications:
- a new prompt must not race with an already-running turn
- tool continuations and permission responses must settle against the correct active turn
- cancellation and cleanup must be scoped to the active turn rather than the entire Bear or role

### 2. Request identity and run identity must remain explicit

The runtime currently tracks request ids and run ids so Den can:
- correlate stream events
- route tool results back into the correct continuation
- cancel the correct run without broad collateral damage
- expose runtime diagnostics for operators and recovery logic

Any replacement runner must preserve explicit request/run identity handling.

### 3. Tool continuation is a first-class runtime behavior

ACP is not a simple prompt/response protocol. The runtime must support:
- emitting tool requests
- pausing while tool execution occurs
- accepting tool results back from the client or Den
- continuing the same turn correctly after tool settlement

This continuation path is a core runtime invariant, not an optional feature.

### 4. Approval pause/resume semantics must remain authoritative

Approval-gated actions must:
- pause at the correct point
- remain pending until an explicit decision is recorded
- resume or deny only through the authoritative policy path
- avoid duplicate fulfillment or bypass on retry/resume

### 5. Stale approval recovery must be safe and explicit

The current ACP path includes stale-approval recovery behavior for interrupted or poisoned runs.

Migration must preserve:
- ability to detect unresolved stale approval state
- ability to deny/close stale approvals safely where policy allows
- clear distinction between stale-request cleanup and a true user/policy denial

### 6. Cancellation must be precise

Cancellation must stop the correct in-flight run or turn without causing broader agent-wide or session-wide unintended side effects.

This is especially important under concurrency and retry conditions.

### 7. Diagnostics are part of the runtime contract

The ACP path currently exposes runtime/session diagnostics including active-turn state and recovery context.

A replacement runtime must preserve enough structured diagnostics to support:
- debugging
- operator visibility
- incident response
- mixed-mode migration comparison

### 8. Ask / Plan / Write gating remains a control-plane invariant

Although runtime providers may differ, the authoritative gating behavior belongs to Den policy.

Migration must preserve that:
- Den/ACP policy state is authoritative
- runtime continuations respect current session mode and approval state
- tool execution scope does not widen because of provider compatibility shortcuts

## Optional-worker / service-toggle implications

Den's one-binary, selectively enabled capability model is compatible with this runtime, but only if ownership remains explicit.

### Control-plane responsibilities that should remain centralized

These should remain Den-authoritative regardless of worker topology:
- trusted human identity
- session policy mode
- approval recording
- turn ownership / active-turn coordination
- run cancellation authority
- interaction/run persistence authority once migrated

### Worker-split concerns to design for early

If pieces of the runtime move into optional workers or projectors later, the system will need clear handling for:
- singleton versus multi-replica ownership of shared-state writers
- idempotent event processing
- avoiding duplicate tool fulfillment
- avoiding duplicate approval fulfillment
- deterministic correlation between persisted runs and streamed events

### Safe migration interpretation

Optional workers should control **where capabilities run**, not redefine the Bear or role model.

Good pattern:
- Den binary with ACP API enabled
- Den binary with Letta compatibility runtime enabled
- Den binary with native runtime worker enabled later
- Den binary with projector/backfill workers enabled only where needed

Bad pattern:
- implicit role semantics depending on deployment toggles alone

## Migration parity checklist

Later phases should explicitly verify parity for:
- active-turn exclusivity
- request/run identity continuity
- tool continuation correctness
- approval pause/resume correctness
- stale approval recovery correctness
- cancellation correctness
- runtime diagnostic completeness
- Den-authoritative Ask / Plan / Write policy enforcement
