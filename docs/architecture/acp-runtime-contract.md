# ACP Runtime Contract

## Purpose

This document defines the Phase 0 contract for Den's ACP runtime boundary.

The goal is **not** to replace Letta immediately. The goal is to:

1. define a Den-native contract for ACP turn execution and conversation lifecycle
2. pin current runtime behavior with contract tests
3. move ACP orchestration to depend on the contract instead of direct Letta semantics
4. allow the current Letta-backed path and a future Den-native runtime to satisfy the same behavioral contract

This document complements:

- [`../guides/letta-migration-plan.md`](../guides/letta-migration-plan.md)
- [`./letta-dependency-matrix.md`](./letta-dependency-matrix.md)

## Why this contract exists

The largest remaining Letta dependency in Den is not terminology visibility. It is that ACP still relies directly on Letta for:

- runtime conversation creation and continuation
- turn execution and streaming
- tool continuation and approval handling
- stale approval recovery
- cancellation and active-turn hygiene
- conversation history behavior

As long as ACP is written in terms of Letta's runtime semantics, Den still depends on Letta as an execution substrate.

The ACP runtime contract is therefore the first major containment seam. It should make ACP depend on Den-owned runtime behavior while allowing Letta to remain a temporary compatibility adapter behind the seam.

## Scope

This contract is for the ACP-facing role runtime used by the Bear `pair` role and any future ACP-capable role runtime with the same interaction model.

It covers:

- runtime conversation lifecycle for ACP sessions
- turn submission and streaming
- tool-call continuation and approval settlement
- cancellation and preflight hygiene
- conversation history access needed by ACP
- runtime health checks required for ACP startup/operation

It does **not** define the full Bear runtime architecture.

## Non-goals

This contract does not attempt to:

- model Letta's full HTTP API
- define Bear provisioning/reconciliation contracts
- replace Letta Code / Codepool harness behavior
- redesign all role semantics
- force migration of legacy persistence field names in phase 0
- standardize every runtime behavior for non-ACP roles before ACP containment is complete

## Design principles

### 1. Behavior-first, not provider-first

The contract should reflect what ACP needs a runtime to do, not what Letta currently exposes.

Bad examples:

- `create_agent`
- `patch_agent`
- `recompile_agent`
- `post_messages`

Preferred examples:

- `ensure_session_conversation`
- `start_turn`
- `continue_turn`
- `cancel_active_turn`
- `load_history`

### 2. Opaque runtime identities

ACP should not need to understand backend-specific identity formats.

In particular, ACP should not encode rules such as:

- Letta conversation ids start with `conv-`
- a specific backend requires `agent_id`
- a specific backend uses run ids with special cleanup behavior

Runtime identifiers should be modeled as opaque compatibility handles at the contract boundary.

### 3. Den owns event and error semantics

ACP should observe Den-owned event types and Den-owned error categories.

Backend-specific behaviors should be normalized inside adapters so that:

- ACP callers do not parse Letta-specific errors
- future Den-native implementations can satisfy the same behavior
- contract tests can validate stable runtime semantics

### 4. Contract tests are part of the design

This contract is incomplete without tests.

The Letta-backed implementation should pass a shared contract test suite first. The future Den-native implementation should be required to pass the same suite before cutover.

## Proposed runtime boundary

The ACP runtime boundary should be split into a small number of focused contracts.

### A. Role runtime binding resolution

Purpose:

- resolve the runtime binding for a Bear role without exposing ACP to provider-specific registry details

Examples of what this may return:

- the currently configured compatibility binding handle for the `pair` role
- enough binding metadata for the runtime adapter to route turns correctly

This belongs near the role profile / runtime registry boundary, not inside ACP orchestration.

### B. ACP conversation lifecycle

Purpose:

- map ACP sessions to runtime conversations
- create new runtime conversations when needed
- recover and continue existing runtime conversations
- load ordered conversation history for ACP UI flows

ACP should see:

- session selection / durable mapping decisions
- Den-owned conversation references
- normalized history records

ACP should not see:

- Letta-native conversation validation rules
- direct provider id parsing

### C. ACP turn runner

Purpose:

- submit a user turn to the role runtime
- stream runtime events in a Den-owned shape
- expose tool continuations and completion/failure states

This is the highest-value seam because it contains the live execution path.

### D. ACP turn continuation and recovery

Purpose:

- continue blocked turns after tool execution
- settle approvals or denials
- run backend-specific stale-state recovery behind the contract

This is especially important because Letta currently requires specialized stale approval handling. ACP should depend on the behavioral outcome, not the Letta recovery mechanism.

### E. ACP runtime hygiene and cancellation

Purpose:

- cancel or clean up active runtime work for a session/conversation
- run preflight cleanup before starting a new turn
- preserve safe multi-session behavior

## Proposed Den-owned types

The exact Rust shapes can evolve, but the contract should include Den-owned types with the following conceptual roles.

### Runtime identity types

- `RoleRuntimeBinding`
  - opaque runtime binding handle for a Bear role
  - optional compatibility metadata for adapters and diagnostics

- `RuntimeConversationRef`
  - opaque handle to the active runtime conversation for an ACP session
  - ACP should treat this as an identifier, not as a provider-specific string format

- `RuntimeTurnRef`
  - opaque handle for an active turn/run
  - may map to a provider run id, but ACP should not depend on that shape

### Request types

- `EnsureConversationRequest`
  - bear id
  - role
  - ACP session id
  - requested session selection if any
  - runtime binding

- `StartTurnRequest`
  - runtime conversation ref
  - runtime binding
  - human message
  - tool descriptors / tool policy
  - runtime context payload already assembled by ACP

- `ContinueTurnRequest`
  - runtime conversation ref
  - runtime turn ref if applicable
  - tool result or approval decision payload

- `CancelTurnRequest`
  - session/conversation/turn identity sufficient to cancel safely
  - cancellation reason/source

### Response and event types

- `EnsureConversationResult`
  - resolved conversation ref
  - whether a new runtime conversation was created
  - normalized history/archive targets if ACP still needs these distinctions

- `RuntimeStreamEvent`
  - assistant text delta / chunk
  - assistant message completed
  - tool call requested
  - tool call settled
  - turn waiting on approval
  - progress / heartbeat if needed
  - conversation resolved
  - turn completed
  - turn failed
  - turn cancelled

- `RuntimeHistoryRecord`
  - Den-normalized transcript/history entry
  - enough to drive ACP history APIs without exposing Letta documents directly

### Error categories

The contract should normalize errors into Den-owned categories such as:

- `Unavailable`
- `Misconfigured`
- `InvalidIdentity`
- `PermissionDenied`
- `ConflictPendingApproval`
- `Cancelled`
- `Timeout`
- `BackendProtocol`
- `Internal`

The error type can still carry adapter diagnostics, but ACP should branch on Den-owned categories.

## Streaming contract requirements

The streaming boundary should guarantee the following behaviors regardless of backend.

### Ordering

- events for a single turn are emitted in order
- terminal events are emitted at most once
- a tool request is emitted before continuation for that tool can be accepted

### Completion

Each started turn must end in exactly one logical terminal outcome:

- completed
- failed
- cancelled
- blocked awaiting external continuation

### Text visibility

Assistant text should be emitted in a consistent shape whether the backend natively streams tokens or emits coarser chunks.

### Tool-call visibility

Tool calls should surface in a Den-owned event shape that includes:

- stable tool call identifier
- tool name
- arguments/payload visible to ACP policy and logging layers
- whether approval is required before continuation

## Conversation identity rules

ACP currently carries several notions of conversation selection and resolved conversation state. The contract should keep the useful distinction while removing Letta-specific assumptions.

### Contract requirements

- ACP may supply a requested session selection token
- the runtime layer resolves that to a durable runtime conversation reference when needed
- ACP stores a Den-owned mapping between ACP session and resolved runtime conversation reference
- the runtime conversation reference is opaque outside the runtime adapter

### Migration note

Existing persistence may continue storing legacy Letta-flavored ids during the transition. That is acceptable for phase 0 as long as the ACP orchestration layer no longer depends on Letta-specific id semantics.

## Tool continuation and approval contract

ACP runtime behavior must support blocked turns that require external work before completion.

The contract should support:

- runtime emits a tool call or approval-needed event
- ACP executes or mediates the tool call
- ACP submits continuation payload back to the runtime
- runtime resumes the same logical turn/conversation

### Required behavioral guarantees

- tool continuation targets the correct conversation and turn scope
- approval denials and tool failures are distinguishable from successful tool returns
- repeated continuation attempts behave deterministically
- stale blocked state recovery happens inside the runtime adapter or runtime implementation, not in ACP orchestration logic

## Cancellation and hygiene contract

ACP needs a stable safety story for cancellation and turn hygiene.

The contract should support:

- preflight cleanup before a new turn when required
- targeted cancellation of an active turn without collateral damage to unrelated sessions
- safe handling when the backend has already completed or forgotten the turn
- idempotent cleanup where practical

This is especially important because the current Letta-backed flow already contains precise logic to avoid unsafe agent-wide cancellation.

## Mapping current Letta behavior into the contract

The Letta-backed adapter should implement the contract by mapping existing Letta behavior into Den-owned concepts.

### Current Letta-backed behavior that should move behind the adapter

- `create_conversation_for_agent`
- Letta-specific conversation id validation
- message streaming / pair turn posting
- pending approval inspection and denial recovery
- run cancellation using Letta-native ids
- Letta-specific conflict and stale-approval recovery heuristics
- history fetch and transcript normalization

### Important rule

The adapter may retain Letta-specific logic internally, but ACP orchestration should stop calling or reasoning about these behaviors directly.

## Recommended Rust contract shape

The code does not need one giant trait. A split contract is likely safer.

Recommended boundaries:

- `RoleProfileRegistry`
  - resolve runtime binding for a role

- `AcpConversationRuntime`
  - ensure session conversation
  - load history
  - archive/compact only if ACP still requires these operations at this layer

- `AcpTurnRunner`
  - start turn
  - continue turn
  - cancel turn
  - preflight hygiene

- `RuntimeHealthCheck`
  - reused for startup and operational health validation

## Contract test matrix

The contract test suite should become the executable specification.

### 1. New conversation creation

- no prior runtime conversation exists
- runtime creates one
- returned conversation reference is stored and reused
- follow-up turns continue the same logical conversation

### 2. Existing conversation continuation

- previously resolved conversation is reused
- no duplicate conversation is created
- history remains attached to the correct conversation

### 3. Streaming completion behavior

- assistant output streams in a stable Den-owned event shape
- tool request events appear before continuation is expected
- exactly one terminal outcome is produced

### 4. Tool continuation success path

- runtime blocks for tool execution
- ACP provides tool result
- runtime resumes and completes the turn

### 5. Tool continuation denial/error path

- ACP submits a denial or failed tool result
- runtime reflects the failure/denial deterministically
- subsequent session state remains usable

### 6. Stale blocked-state recovery

- a conversation contains unresolved prior approval/tool state
- preflight or continuation recovery clears or normalizes the blocked state
- a subsequent turn can proceed cleanly

### 7. Cancellation semantics

- active turn is cancelled
- terminal state is surfaced correctly
- future turns are not poisoned by the cancelled state

### 8. History semantics

- ordered transcript is returned in Den-normalized form
- invalid or missing conversation references fail with normalized errors

### 9. Error normalization

- backend-specific errors are translated into Den-owned categories
- ACP callers do not branch on Letta text fragments or status codes

### 10. Health and misconfiguration

- missing runtime backend is reported as a normalized health/configuration failure
- ACP startup gating depends on runtime capability, not direct Letta assumptions

## Phased implementation plan

### Phase 0A: Define contract and tests

Deliverables:

- this design note
- Rust trait/type skeletons in core runtime contracts
- contract test scaffolding for the Letta-backed adapter

### Phase 0B: Move ACP orchestration behind the contract

Deliverables:

- `api/acp.rs` depends on ACP runtime contracts rather than direct Letta semantics
- Letta-specific recovery and continuation logic moves into adapter code
- startup validation reasons about ACP runtime capability instead of directly coupling ACP to Letta config

### Phase 0C: Stabilize Letta-backed adapter under contract tests

Deliverables:

- Letta-backed implementation passes the contract suite
- known behavioral edge cases are documented and pinned

### Phase 1: Build Den-native replacement against the same contract

Deliverables:

- Den-native ACP runtime implementation
- the same contract tests pass for the new implementation
- Letta cutover becomes an implementation swap rather than a new orchestration rewrite

## Phase 0 status update

As of the current Phase 0 checkpoint, the first live seam is in place and shipping in the active ACP path.

### Implemented now

The codebase now includes a Den-owned ACP turn-runner contract and a Letta-backed adapter:

- `services/den/src/core/runtime_contracts.rs`
  - `AcpTurnRunner`
  - `StartTurnRequest`
  - `StartTurnResult`
  - `CancelTurnRequest`
  - `CancelTurnResult`
- `services/den/src/core/acp_turn_runner.rs`
  - `LettaAcpTurnRunner`
  - `start_acp_turn_with_retries`
  - `acp_cleanup_stale_runtime_state`
  - targeted Letta cleanup helpers
- `services/den/src/core/runtime_provider.rs`
  - runtime-facing re-exports for ACP callers
- `services/den/src/api/acp.rs`
  - updated to depend on the seam for turn start / retry / cleanup behavior rather than directly embedding all Letta-specific logic inline

There is also implementability coverage in:

- `services/den/src/core/runtime_provider_tests.rs`

using a no-op `AcpTurnRunner` to prove the seam is mockable and not hard-wired to Letta types.

### What this seam currently owns

Today the seam meaningfully owns:

- ACP turn start
- explicit Den-owned turn continuation request/result types
- approval-decision and tool-result continuation semantics
- Letta-backed continuation payload translation behind the adapter
- stale-approval retry behavior used during turn start
- targeted stale runtime cleanup before retry / recovery
- contract-level request/result types for turn start, continuation, and cancellation-oriented cleanup paths
- Den-owned continuation stream handles and stream error types

### What still remains adapter-specific or outside the seam

The migration is still incomplete. The following concerns remain partially Letta-shaped or still coordinated outside a fuller runtime boundary:

- runtime conversation lifecycle and conversation resolution
- history loading / transcript normalization as a first-class contract surface
- continuation stream parsing still remains transitional: the runtime seam now exposes a Den-owned event stream type, but continuation events are currently carried as `RuntimeStreamEvent::RawBytes` rather than fully semantic runtime events
- broader cancellation semantics for active runtime work beyond the currently extracted targeted cleanup path
- normalized Den-owned runtime error categories used consistently across all ACP runtime operations
- runtime health/capability reporting for a future non-Letta implementation
- ACP still owns the final bridge from continuation stream events into ACP wire events, which should eventually be driven by structured runtime events rather than raw backend SSE payloads

### Transitional entrypoints

The following helpers are intentionally transitional and should be treated as compatibility wrappers, not the final contract shape:

- `start_acp_turn_with_retries`
- `acp_cleanup_stale_runtime_state`

They exist to:

- preserve the current ACP behavior while the seam is being extracted incrementally
- keep Letta-specific stale approval and cancel behavior inside adapter-oriented code
- avoid a flag-day rewrite of `api/acp.rs`

The long-term direction is to either:

- absorb these behaviors into a fuller `AcpTurnRunner` trait surface, or
- move them into a slightly broader ACP runtime orchestration/provider layer that remains Den-owned and backend-agnostic

## Forward plan after this checkpoint

The next work should focus on expanding the seam from "turn start + stale recovery" into a backend-complete ACP runtime boundary.

### Next milestone 1: Structured continuation event stream

Advance the Den-owned continuation surface from transport wrapping into semantic runtime streaming.

Target outcomes:

- preserve `ContinueTurnRequest` / `ContinueTurnResult` as the continuation contract
- replace continuation `RuntimeStreamEvent::RawBytes` payloads with structured runtime events where practical
- move continuation SSE parsing / normalization behind the runtime adapter seam
- make ACP consume Den-owned continuation events rather than Letta-shaped SSE frame bytes
- keep stale blocked-state recovery part of continuation behavior rather than ad hoc ACP orchestration logic

### Next milestone 2: Conversation lifecycle contract

Extract ACP conversation lifecycle behind a Den-owned boundary.

Target outcomes:

- explicit `ensure_session_conversation` contract
- opaque `RuntimeConversationRef` used consistently by ACP
- backend-specific conversation-id validation removed from ACP orchestration logic
- Den-normalized history loading for ACP UI APIs

### Next milestone 3: Cancellation and hygiene contract

Generalize the current cleanup path into a fuller runtime hygiene surface.

Target outcomes:

- explicit cancel / cleanup semantics for active turns
- idempotent targeted cleanup where possible
- no ACP branching on Letta-specific cancel endpoints or stale-approval heuristics
- safe session-scoped cancellation semantics preserved across backends

### Next milestone 4: Error normalization

Introduce stable Den-owned ACP runtime error categories and make ACP branch only on those.

Target outcomes:

- adapters translate backend-specific HTTP / protocol / conflict errors into Den-owned categories
- ACP stops matching on Letta-flavored error strings
- contract tests verify behavior using normalized categories rather than backend text fragments

### Next milestone 5: Den-native implementation path

Once the contract surface is broad enough, add a second implementation path that is not Letta-backed.

Target outcomes:

- runtime-provider selection can resolve either Letta-backed or Den-native ACP runtime implementations
- shared contract tests run against both implementations
- Letta becomes a compatibility adapter rather than an architectural requirement

## Recommended planning sequence

A practical near-term sequence is:

1. add explicit turn continuation types and adapter methods
2. extract conversation ensure/history behavior into a sibling runtime contract
3. normalize runtime error categories used by ACP
4. add a Den-native skeleton implementation behind provider selection
5. expand contract tests so behavior is specified backend-independently before deeper cutover

## Immediate next step

The most valuable immediate next step is:

1. keep the explicit continuation contract (`ContinueTurnRequest` / `ContinueTurnResult`) as the stable Den-owned surface
2. replace continuation `RuntimeStreamEvent::RawBytes` bridging with structured runtime stream events
3. move continuation SSE parsing / normalization fully behind the adapter seam
4. pin those behaviors with backend-agnostic contract tests

That is the next highest-leverage extraction because continuation, approval settlement, and stale blocked-state recovery are still where backend-specific runtime semantics leak most strongly into ACP orchestration.
