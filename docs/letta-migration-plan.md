# Letta Migration Plan

## Purpose

This document proposes a phased migration path for BEARS to move off the deprecated Letta API server while preserving current behavior, minimizing operational risk, and keeping migration reversible where possible.

It is based on the dependency inventory in [`./letta-dependency-matrix.md`](./letta-dependency-matrix.md).

## Executive summary

BEARS should not treat this migration as a simple vector-store replacement.

The current repository depends on Letta primarily for:

1. **runtime execution and conversation lifecycle**
2. **role-agent provisioning and configuration sync**
3. **Codepool-backed harness behavior for `talk` and `work`**
4. **conversation/admin read models and diagnostics**

Canonical Bear memory already appears to live primarily in MemFS/git role branches, which is an advantage. The recommended migration strategy is therefore:

1. **contain** Letta behind explicit internal abstractions
2. **move persistence ownership** for conversations/runs into Den
3. **replace API-direct roles first** with a Den-native runtime (`watch`, then `curate`, then `pair`)
4. **replace provisioning/registry semantics** with Den-owned runtime handles
5. **separately replace Letta Code / Codepool dependencies** for `talk` and `work`
6. **finally remove Letta-specific MemFS/git/indexing assumptions**

## Migration goals

### Primary goals

- Remove operational dependency on the Letta API server
- Preserve BEARS role semantics and current user-visible behavior
- Keep canonical memory in BEARS-owned systems
- Maintain ACP safety and approval semantics
- Preserve or improve observability and debuggability

### Secondary goals

- Make runtime behavior explicit and testable
- Reduce hidden state and cross-service coupling
- Enable Qdrant or another owned retrieval layer without entangling it with runtime state
- Allow per-role migration rather than all-at-once cutover

### Non-goals

These should **not** be treated as first-order migration goals for phase 1:

- replacing every Letta-adjacent concept with Qdrant
- redesigning all BEARS role semantics
- reworking MemFS branch policy from scratch
- fully unifying talk/pair/curate/work/watch runtimes at the start

## Core migration principles

### 1. Separate runtime from memory from retrieval

These concerns should be explicitly different in the target architecture:

- **runtime**: model loop, tools, approvals, streaming, cancellation
- **memory**: canonical role/core durable state in MemFS/git
- **retrieval**: embeddings/indexes over canonical sources

Letta currently spans parts of all three. The migration should split them cleanly.

### 2. Den should own the control plane

Den already appears to be the best place to own:

- Bear/role registry
- session and policy state
- approvals
- work planning and workflow state
- conversation metadata
- auditability

The migration should continue in that direction.

### 3. Migrate by runtime family, not by whole system

The repo already defines two runtime families:

- **API-direct**: `pair`, `curate`, `watch`
- **harness-backed**: `talk`, `work`

These families should migrate independently.

### 4. Prefer dual-write and compatibility periods over hard cutovers

Where feasible, migration should first add BEARS-owned data models and write to them alongside Letta-backed execution before switching reads and then switching execution.

## Current-state assessment

### API-direct roles

Roles:

- `pair`
- `curate`
- `watch`

Current behavior:

- Den uses Letta conversations and runs as execution substrate
- Den adds policy, tool mediation, and workflow control around that runtime
- ACP `pair` has especially strong Letta-specific recovery/cancellation logic

### Harness-backed roles

Roles:

- `talk`
- `work`

Current behavior:

- Den uses Codepool as the runtime boundary
- Codepool is currently configured as a Letta Code harness
- Letta remains the real runtime dependency behind the harness

### Memory

Current durable memory source of truth appears to be:

- MemFS/git role branches
- `core/` and role-local branches
- Den-managed memory tooling and governance

This is helpful because it means migration can focus first on runtime and state orchestration.

## Target architecture

## Den-owned control plane

Den should own:

- Bear registry and role runtime metadata
- conversation metadata and message/event store
- run records and cancellation state
- approval state machine
- policy gating
- workflow/plan state
- memory governance and promotion flows

## Runtime service(s)

A runtime implementation should own:

- model invocation
- streaming responses
- tool-call loop
- continuation after tool results
- approval pauses/resumes
- summarization/compaction

This may initially be:

- Den-native for API-direct roles
- Codepool-next or equivalent for harness roles

## MemFS manager

MemFS manager should own:

- canonical git-backed role/core repositories
- view registration / role-scoped branch access
- direct git operations previously routed through Letta

## Retrieval/index layer

Qdrant or another owned system should own:

- derived semantic indexes over canonical sources
- retrieval APIs for context assembly
- optional conversation-summary retrieval later

## Recommended internal abstractions

The first implementation step should be to define interfaces that isolate Letta.

### 1. Runtime registry

Responsibilities:

- create/update/delete runtime instances for a role
- persist runtime family and runtime handle
- expose diagnostics and status
- compare desired config with applied config

Suggested shape:

- `runtime_family`: `letta_api`, `letta_code`, `den_native`, `codepool_native`, etc.
- `runtime_handle`: opaque identifier, replacing role-specific reliance on `letta_agent_id`
- `config_hash`
- `status`
- `last_reconciled_at`

### 2. Conversation runtime

Responsibilities:

- start a conversation or run
- continue after tool execution
- stream events/tokens
- track pending approvals
- cancel safely
- compact/summarize

### 3. Conversation store

Responsibilities:

- persist thread metadata
- persist message envelopes/events/tool calls/approvals
- support title/archive/delete
- provide admin/UI read APIs

### 4. Tool registry

Responsibilities:

- store canonical BEARS tool descriptors
- resolve tool availability by role/runtime/policy
- avoid dependence on Letta-specific tool ids as the canonical source

### 5. Retrieval service

Responsibilities:

- index canonical sources
- answer retrieval requests independent of runtime engine

## Phased migration plan

## Phase 0 — preparation and containment

### Objective

Stop Letta from being an ambient assumption and make it a replaceable implementation detail.

### Work items

1. Introduce explicit traits/interfaces for:
   - runtime registry
   - conversation runtime
   - conversation store
   - tool registry

2. Refactor Letta integration behind those interfaces.

3. Replace direct Letta-centric naming in internal core logic where appropriate:
   - favor `runtime_handle` over `letta_agent_id` in generic paths
   - keep legacy fields as compatibility adapters until cutover

4. Add instrumentation around all Letta interactions:
   - endpoint called
   - role/runtime family
   - latency
   - failure mode
   - correlation ids / run ids

### Deliverables

- internal abstraction layer merged
- Letta becomes one provider implementation
- improved observability for migration planning

### Exit criteria

- major call sites no longer directly depend on `LettaClient` semantics outside adapter/provider layers
- replacement implementation could be added without touching UI or role logic everywhere

## Phase 1 — Den-owned conversation and run persistence

### Objective

Make Den the source of truth for conversation metadata and run tracking before changing execution.

### Work items

1. Add Den tables for:
   - conversations
   - conversation participants/context
   - messages/events
   - tool calls
   - approvals
   - runs
   - run cancellation state

2. Start dual-writing metadata during Letta-backed execution:
   - new conversations
   - titles
   - archived state
   - messages/events
   - run lifecycle markers

3. Repoint admin/UI reads where possible to Den-owned data instead of Letta list/read endpoints.

4. Preserve compatibility adapters for existing Letta-backed histories during transition.

### Deliverables

- Den-owned conversation read model
- Den-owned run ledger
- conversation/admin UI less coupled to Letta

### Exit criteria

- UI can render recent talk/pair conversation lists from Den
- Den can answer thread title/archive/delete state from its own store
- run and approval state is queryable without asking Letta first

### Rollback strategy

- continue reading Letta as fallback if Den projections are incomplete
- dual-write remains non-destructive

## Phase 2 — Den-native runtime for `watch`

### Objective

Replace the simplest API-direct Letta runtime first.

### Why `watch` first

- narrowest tool surface
- event-driven role
- lower user-facing concurrency complexity than `pair`
- lower external breadth than `curate`

### Work items

1. Implement a Den-native execution loop for `watch`:
   - prompt assembly
   - context retrieval from MemFS/core/watch inputs
   - model call
   - tool mediation
   - event/message persistence in Den

2. Preserve existing watch role contract and output schemas.

3. Add feature flag or per-role runtime-family switch.

4. Compare outputs and operational logs against Letta-backed baseline in staging.

### Deliverables

- `watch` can run without Letta
- Den-native streaming/event persistence path proven in production-like conditions

### Exit criteria

- `watch` role fully functional on Den-native runtime
- no Letta dependency for watch execution path

## Phase 3 — Den-native runtime for `curate`

### Objective

Migrate internal governance/reflection flow off Letta.

### Why second

- more complex than `watch`, but still internally controlled
- valuable proving ground for approvals/reviews/memory integration
- lower direct user interactivity risk than `pair`

### Work items

1. Implement Den-native curate loop with:
   - multi-branch read context assembly
   - narrow tool roster
   - review/approval/write flows
   - summarization and memory-promotion hooks

2. Replace Letta-dependent reflection/defragmentation logic with explicit Den-native equivalents or scheduled jobs.

3. Store curate runs and review events in Den.

### Deliverables

- `curate` running without Letta
- explicit review and reflection orchestration in Den

### Exit criteria

- curate cycle can execute end-to-end with Den-native runtime
- no Letta API dependency for curate execution path

## Phase 4 — Den-native runtime for `pair`

### Objective

Move ACP off the Letta execution substrate.

### Why last among API-direct roles

`pair` is the highest-risk API-direct role because ACP currently depends on Letta-specific handling for:

- pending approvals
- tool-return continuations
- run cancellation
- concurrent session hygiene
- poisoned conversation recovery

### Work items

1. Implement Den-native ACP conversation runtime:
   - stream-safe incremental responses
   - server-mediated tool call requests
   - approval state machine
   - continuation after tool execution
   - run ids / request ids / cancellation semantics

2. Preserve ACP policy semantics:
   - Ask / Plan / Write gating
   - pending permissions
   - pending plan approvals
   - active turn cleanup and cancellation safety

3. Build compatibility tests for known Letta-era failure modes.

4. Perform shadow or canary rollout for low-risk sessions first.

### Deliverables

- Den-native `pair` runtime
- removal of Letta-specific ACP runtime logic from core session paths

### Exit criteria

- ACP sessions can run without Letta
- session cancellation and tool continuation are reliable under concurrency

### Rollback strategy

- maintain per-session or per-bear runtime family fallback to Letta during rollout

## Phase 5 — Replace provisioning and role runtime registry

### Objective

Stop creating Letta agents as the canonical role runtime identity.

### Work items

1. Generalize `bear_agents` or equivalent runtime metadata so it can represent non-Letta runtimes.

2. Replace generic internal assumptions that a role runtime is a Letta agent.

3. Migrate provisioning/sync code to:
   - compute desired runtime config
   - apply it through a runtime provider
   - persist provider-neutral runtime metadata

4. Keep Letta adapters for any remaining roles until harness migration is complete.

### Deliverables

- provider-neutral runtime registry
- per-role runtime family control
- reduced reliance on `letta_agent_id`

### Exit criteria

- API-direct roles no longer require Letta agent provisioning
- Den can reconcile runtime config without Letta-specific patch/recompile semantics

## Phase 6 — Replace Codepool / Letta Code dependency for `talk` and `work`

### Objective

Remove the indirect Letta dependency that remains through Codepool.

### Two viable paths

#### Option A: evolve Codepool into a BEARS-native harness

Pros:

- preserves Den-facing interface
- smaller blast radius for Den web/chat routes
- keeps warm-runtime/session management in a dedicated service

Cons:

- still requires substantial runtime reimplementation inside Codepool

#### Option B: replace Codepool's Letta-specific role with a new runtime service

Pros:

- cleaner long-term architecture
- avoids preserving Letta Code-specific assumptions

Cons:

- larger integration change up front

### Recommended direction

Prefer **Option A as a bridge** if delivery speed matters, but design toward **Option B semantics** so Codepool does not become a permanent compatibility layer for Letta-era concepts.

### Work items

1. Inventory exactly what Codepool depends on Letta for:
   - conversation state
   - skills loading
   - harness startup lifecycle
   - channel behavior
   - MemFS local mirror assumptions

2. Implement replacement backend behavior under the existing Den-facing API.

3. Migrate `talk` first or `work` first depending on operational risk:
   - `talk` has more direct user experience impact
   - `work` has more policy/tooling risk

4. Remove Letta Code harness YAML generation once no longer needed.

### Deliverables

- `talk` and `work` running without Letta Code
- Codepool or successor runtime no longer requires `LETTA_BASE_URL`

### Exit criteria

- talk/work traffic no longer transitively depends on Letta

## Phase 7 — MemFS/git cleanup and retrieval replacement

### Objective

Remove the remaining Letta-shaped storage and indexing assumptions.

### Work items

1. Replace Letta `/v1/git/*` proxy assumptions with direct MemFS/git APIs.

2. Move any cache/index invalidation behavior to explicit jobs or hooks.

3. Replace Letta archives with Qdrant or another owned retrieval/index service.

4. Remove Letta-specific filesystem layout assumptions where practical.

### Deliverables

- direct MemFS/git ownership by BEARS services
- owned retrieval/index layer
- no Letta-shaped storage coupling required

### Exit criteria

- no runtime or indexing path requires Letta
- Letta-specific infra can be shut down

## Phase 8 — retirement and cleanup

### Objective

Fully remove Letta from the stack and codebase.

### Work items

1. Remove:
   - `bears-letta`
   - `bears-letta-postgres`
   - Letta env vars
   - Letta-specific health checks
   - Letta backup jobs
   - Letta-specific docs and comments

2. Delete dead adapter code and compatibility shims.

3. Backfill any remaining conversation/runtime data from Letta if needed.

4. Update docs, operational runbooks, and deployment templates.

### Exit criteria

- no production path depends on Letta
- infra and docs no longer mention Letta except in migration history

## Data model recommendations

## Conversation store

Recommended Den-owned entities:

- `conversation`
- `conversation_member` or role-context association
- `message`
- `message_event`
- `tool_call`
- `tool_result`
- `approval_request`
- `approval_response`
- `run`
- `run_cancellation`

This should preserve enough fidelity to support:

- ACP session continuity
- web chat history
- admin diagnostics
- replay/debugging
- migration fallback

## Runtime registry

Recommended fields or concepts:

- `bear_id`
- `role`
- `runtime_family`
- `runtime_handle`
- `provisioning_status`
- `config_hash`
- `runtime_policy_hash`
- `last_reconciled_at`
- `last_error`

This allows per-role runtime-family migration without forcing all roles onto one backend.

## Risks and mitigations

## Risk: ACP regression during `pair` migration

Mitigation:

- migrate `watch` and `curate` first
- dual-write conversation state early
- preserve fallback runtime family
- build explicit concurrency/cancellation tests

## Risk: Codepool migration drags on

Mitigation:

- treat API-direct migration and Codepool migration as separate workstreams
- allow temporary hybrid operation by role family

## Risk: UI/admin regressions from read-model transition

Mitigation:

- dual-write and verify Den projections before switching reads
- keep Letta-backed reads as temporary fallback

## Risk: hidden Letta side effects around MemFS/git or indexing

Mitigation:

- instrument current `/v1/git` and role-view flows
- make invalidation/index refresh explicit in BEARS services

## Risk: skill handling diverges across runtime families

Mitigation:

- define a provider-neutral skill projection model before replacing providers
- keep skill manifest as source of truth

## Milestones

A practical milestone sequence:

1. **M1** — abstraction layer introduced, Letta isolated
2. **M2** — Den-owned conversation/run store dual-write enabled
3. **M3** — `watch` on Den-native runtime
4. **M4** — `curate` on Den-native runtime
5. **M5** — `pair` on Den-native runtime
6. **M6** — provider-neutral runtime registry in place
7. **M7** — `talk`/`work` no longer depend on Letta Code
8. **M8** — MemFS/index cleanup complete
9. **M9** — Letta infra retired

## Recommended immediate next steps

1. Define the provider-neutral interfaces in code:
   - runtime registry
   - conversation runtime
   - conversation store

2. Design Den-owned schema for conversations/runs/tool calls/approvals.

3. Document current ACP runtime invariants from `api/acp.rs` so `pair` parity targets are explicit.

4. Inventory Codepool's Letta-specific contracts in similar detail to the Den dependency matrix.

5. Build `watch` as the first Den-native runtime implementation.

## Decision summary

The recommended path is:

- **not** a big-bang replacement
- **not** a Qdrant-first migration
- **yes** to Den-owned runtime and persistence abstractions
- **yes** to dual-write transition state
- **yes** to API-direct roles first
- **yes** to a separate harness migration for `talk` and `work`

This path matches the current repo structure and minimizes the risk of destabilizing ACP and chat flows while progressively removing Letta from the architecture.
