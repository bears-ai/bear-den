# Implementation Plan: Den Context Compaction

This plan implements the context-compaction direction described in [ADR-0032: Den Context Compaction Architecture](../decisions/adr-0032-den-context-compaction-architecture.md) and supports the Letta migration plan in [letta-migration-plan.md](../guides/letta-migration-plan.md).

The goal is to replace Letta-era long-context handling with a Den-owned compaction system that preserves continuation quality, keeps transcript ownership explicit, and separates derived compaction artifacts from durable memory.

## Scope

This plan covers:

- canonical transcript ownership and derived compaction artifacts,
- runtime semantic grouping for compaction,
- prompt-assembly integration for compacted state,
- trigger and safety-floor policy,
- role-sensitive compaction behavior,
- operator visibility and auditability,
- and evaluation of post-compaction continuation quality.

This plan does **not** include full archival retrieval replacement, editable prompt-memory replacement, or all migration/backfill mechanics beyond what compaction directly depends on.

## Success criteria

The implementation is successful when:

- Den owns transcript history and compaction artifacts independently of Letta runtime state.
- Long-lived sessions can continue after compaction without losing active constraints, plan state, artifacts, or unresolved tool/approval spans.
- Prompt assembly uses explicit compacted-state inputs rather than silently mutating transcript semantics.
- Operators can inspect when compaction happened, what policy triggered it, and what summary artifact was produced.
- Compaction quality is evaluated with continuation probes rather than token-count reductions alone.

## Guiding constraints

- Canonical transcript state remains distinct from derived compacted state.
- Compaction must not cross active semantic floors such as unresolved tool spans or approval spans.
- Durable memory promotion remains separate from compaction.
- `pair`/ACP invariants are the strictest baseline and should drive the initial implementation safety model.
- The first release should favor correctness and auditability over aggressive token reduction.

---

## Phase 0 — Specification and invariants freeze

**Goal:** lock the compaction contract before implementation spreads across runtime codepaths.

### Tasks

1. Define the canonical compaction terms in one implementation-facing note:
   - semantic group
   - active working set
   - canonical transcript
   - derived compaction artifact
   - safety floor
   - trigger vs target
   - compaction boundary
2. Enumerate role-invariant protected spans:
   - unresolved tool spans
   - unresolved approval spans
   - active workflow/plan state
   - current constraints and decisions
   - current artifact references required for continuation
3. Define which runtime surfaces initially participate:
   - ACP / `pair`
   - web conversation / `chat`
   - future shared role runtime surfaces
4. Define what compaction may produce in v1:
   - iterative summary artifact
   - collapsed tool bundle summaries
   - recency-window retention
   - truncation only as final backstop

### Acceptance

- A short implementation-facing contract exists and is referenced by runtime work.
- Protected semantic floors are explicit and testable.
- The first participating runtime surfaces are named.

---

## Phase 1 — Canonical transcript and derived-state schema

**Goal:** give Den a clear persistence model for transcript ownership and compaction artifacts.

### Tasks

1. Add or extend Den-owned storage to distinguish:
   - canonical transcript/event rows,
   - semantic-group metadata,
   - compaction artifact rows,
   - compaction runs/events/telemetry.
2. Ensure compaction artifacts are attributable and versioned with:
   - session/conversation id,
   - compaction policy version,
   - trigger reason,
   - source span/group range,
   - artifact type,
   - created-at timestamp.
3. Define whether compaction artifacts are rebuildable and what inputs are required.
4. Keep durable memory storage out of this schema except for explicit references.

### Acceptance

- Schema separates transcript from compaction artifacts.
- A compaction artifact can be traced to source transcript ranges and policy version.
- No table conflates compaction summary state with durable memory.

---

## Phase 2 — Semantic grouping model

**Goal:** compact grouped runtime units rather than raw messages.

### Tasks

1. Define semantic group types such as:
   - user turn
   - assistant reply
   - tool interaction bundle
   - approval interaction bundle
   - plan/workflow update
   - artifact/reference update
   - prior compaction summary artifact
2. Implement grouping logic in transcript ingestion/runtime write paths.
3. Define group boundaries for ACP and web conversation flows.
4. Add tests for group formation around tool-heavy and approval-heavy turns.

### Acceptance

- Runtime history can be queried as semantic groups.
- Tool and approval spans stay intact as groups.
- Grouping behavior is covered by tests for ACP and conversation flows.

---

## Phase 3 — Compaction policy engine and safety floors

**Goal:** add a deterministic compaction controller that respects runtime invariants.

### Tasks

1. Implement trigger classes:
   - token-pressure trigger
   - turn-count/group-count trigger
   - model-window safety-margin trigger
   - explicit/manual trigger
2. Implement target selection that excludes protected spans.
3. Implement safety floors preventing compaction from crossing:
   - unresolved tool spans
   - unresolved approval spans
   - current working plan/workflow state
   - current artifact/constraint anchors
4. Define fallback order:
   - tool-result collapse
   - structured summary merge
   - recency-window enforcement
   - truncation as final backstop
5. Add policy tests for floor enforcement.

### Acceptance

- Compaction never crosses an active protected span.
- Policy chooses older eligible groups before recent active groups.
- Fallback order is explicit and tested.

---

## Phase 4 — Iterative summary artifact generation

**Goal:** maintain anchored summary state incrementally rather than rewriting history from scratch.

### Tasks

1. Define a v1 summary artifact shape, including fields for:
   - active user goals
   - important constraints
   - decisions made
   - artifact references
   - plan/workflow state references
   - unresolved follow-ups
   - compacted span provenance
2. Implement summary merge/update logic from newly compacted spans.
3. Preserve provenance linking summary content to compacted source groups.
4. Add tests for iterative updates over multiple compaction cycles.

### Acceptance

- Summary artifacts can be updated incrementally.
- New compaction cycles extend/merge prior summary state rather than rewriting from scratch.
- Summary artifacts retain provenance to source groups.

---

## Phase 5 — Prompt assembly integration

**Goal:** make compacted state an explicit input to runtime prompt construction.

### Tasks

1. Update prompt assembly to inject:
   - system/developer instructions,
   - active workflow/workplan state,
   - uncompacted recent groups,
   - derived compaction artifacts as explicit context objects.
2. Ensure prompt assembly does not present summary artifacts as raw transcript.
3. Define ordering/precedence rules between recent transcript and compacted summary state.
4. Add tests that resumed sessions assemble context correctly after compaction.

### Acceptance

- Runtime prompt assembly explicitly consumes compacted state.
- Compacted summaries are distinguishable from raw transcript during assembly.
- Resumed sessions continue with preserved plan/constraint/artifact context.

---

## Phase 6 — Role-sensitive compaction policies

**Goal:** share the architecture while allowing role-specific floors and strategy differences.

### Tasks

1. Define default policy profiles for:
   - `pair`
   - `chat`
   - `work`
   - `watch`
   - `review`
2. Start by implementing `pair` and `chat` profiles.
3. For `pair`, preserve:
   - unresolved tool/approval spans,
   - immediate workplan state,
   - current coding constraints,
   - active artifact references.
4. For `chat`, allow more aggressive compaction of older conversational material while preserving current commitments and intent.
5. Add tests proving role-policy differences are applied intentionally.

### Acceptance

- `pair` and `chat` have distinct compaction policies.
- `pair` remains the strict safety baseline.
- Future roles have identified policy hooks even if not fully implemented.

---

## Phase 7 — Operator visibility and diagnostics

**Goal:** make compaction observable and auditable.

### Tasks

1. Add compaction events/telemetry recording:
   - trigger type
   - policy version
   - source groups compacted
   - artifact id produced
   - token estimates before/after
2. Extend admin/read-model surfaces to show:
   - whether a session has been compacted
   - latest compaction artifact
   - count/history of compaction events
3. Provide drill-down visibility from compaction artifact to source span provenance.
4. Add diagnostic tests for compaction observability.

### Acceptance

- Operators can inspect compaction history for a session.
- Compaction events are attributable to trigger and policy version.
- Artifact provenance is visible enough for debugging.

---

## Phase 8 — Continuation-quality evaluation harness

**Goal:** evaluate compaction by continuation quality, not only token savings.

### Tasks

1. Build a probe suite covering:
   - recall of active constraints
   - recall of decisions made
   - artifact retention
   - workflow/workplan continuity
   - next-step continuity
   - unresolved tool/approval awareness
2. Create representative transcripts for:
   - ACP tool-heavy sessions
   - long chat sessions
   - plan-heavy sessions
3. Compare pre-compaction and post-compaction continuation behavior.
4. Set baseline acceptance thresholds for safe rollout.

### Acceptance

- Compaction quality is measured with continuation probes.
- `pair`/ACP scenarios are included in the baseline suite.
- Release gating can reference continuation-quality results.

---

## Phase 9 — Initial rollout and migration support

**Goal:** introduce Den-owned compaction safely on selected surfaces.

### Tasks

1. Roll out first on a bounded surface, preferably internal/test `pair` and/or `chat` sessions.
2. Run compaction artifacts in shadow/observe mode before making them authoritative in prompt assembly where feasible.
3. Compare continuation outcomes, operator visibility, and failure modes.
4. Document rollback behavior:
   - disable compaction policy,
   - ignore derived artifacts,
   - rebuild artifacts from transcript if needed.

### Acceptance

- At least one runtime surface uses Den-owned compaction successfully.
- Rollback is documented and practical.
- Compaction can be disabled without transcript loss.

---

## Open questions to resolve during implementation

- What exact summary schema best balances structure with model flexibility?
- Should compaction artifacts be regenerated opportunistically when policy versions change?
- How should transcript replay APIs expose compacted vs uncompacted history to clients?
- Which role beyond `pair` and `chat` should be the next rollout target?
- What is the right operator-facing representation for compacted state in admin views?

## Recommended implementation order

1. Phase 0 — specification and invariants
2. Phase 1 — transcript/artifact schema
3. Phase 2 — semantic grouping
4. Phase 3 — compaction policy engine
5. Phase 4 — iterative summary artifacts
6. Phase 5 — prompt assembly integration
7. Phase 7 — observability
8. Phase 8 — evaluation harness
9. Phase 6 — broaden role-sensitive policy from `pair`/`chat`
10. Phase 9 — rollout

This ordering favors correctness of transcript ownership, safety floors, and prompt-assembly semantics before broad rollout.

---

## Current implementation status

This section tracks the current status of the Den-owned compaction migration slice.

### Completed in this slice

- **Phase 0 — specification and invariants**
  - implementation-facing contract documented in:
    - `docs/architecture/den-context-compaction-contract.md`
  - protected floors and compaction terms are explicitly documented.

- **Phase 1 — transcript/artifact schema direction**
  - logical schema direction documented in:
    - `docs/architecture/den-context-compaction-schema.md`
  - transcript/artifact/event separation is now explicit at the design level.

- **Phase 2 — semantic grouping (scaffold level)**
  - initial semantic-group types and grouping logic added in:
    - `services/den/src/core/runtime_conversations.rs`
    - `services/den/src/core/runtime_compaction.rs`
  - coverage exists for user, assistant, tool, approval, workflow, artifact, and system grouping behavior.

- **Phase 3 — compaction policy engine (initial implementation)**
  - initial trigger-aware policy, protected-tail handling, and protected-span skipping implemented in:
    - `services/den/src/core/runtime_compaction.rs`
  - policy behavior is covered by unit tests.

- **Phase 4 — iterative summary artifacts (initial implementation)**
  - v1 summary shape and merge/update behavior implemented in:
    - `services/den/src/core/runtime_conversations.rs`
    - `services/den/src/core/runtime_compaction.rs`

- **Phase 5 — prompt assembly integration (initial foothold)**
  - explicit runtime context-envelope primitives implemented in:
    - `services/den/src/core/runtime_compaction.rs`
  - ACP prompt assembly now includes an initial Den-owned compaction-layer reminder in:
    - `services/den/src/api/acp/prompt_context.rs`

- **Phase 7 — observability (initial event model)**
  - compaction event/status types and helper builders implemented in:
    - `services/den/src/core/runtime_compaction_observability.rs`
  - observability note documented in:
    - `docs/architecture/den-context-compaction-observability.md`

- **Phase 8 — continuation-quality evaluation harness (deterministic scaffold)**
  - deterministic pair/chat continuation probes implemented in:
    - `services/den/src/core/runtime_compaction_eval_tests.rs`
  - guide-level explanation documented in:
    - `docs/guides/context-compaction-guide.md`

- **Validation**
  - `cargo test --lib --manifest-path /workspace/services/den/Cargo.toml` is passing after these changes.

### Partially complete / still in progress

- **Phase 2** is only partially integrated into real runtime data flows.
  - semantic grouping currently exists as shared primitives and tests,
  - but it is not yet built from live persisted transcript history end-to-end.

- **Phase 5** has an initial runtime-path integration,
  - but prompt assembly is not yet fed by real transcript-backed compacted artifacts.

- **Phase 7** has event shapes,
  - but these are not yet wired into durable event recording or operator/admin read models.

### Remaining work before this migration slice is complete

1. **Connect compaction to real transcript history**
   - build `RuntimeSemanticGroup` values from real stored session/conversation events,
   - not just synthetic fixtures and helper inputs.

2. **Run compaction decisions against live runtime state**
   - evaluate triggers against actual sessions,
   - select eligible historical ranges,
   - emit applied/skipped events from real execution paths.

3. **Persist and retrieve real compaction artifacts**
   - move from schema direction and in-memory shapes to stored artifacts with provenance.

4. **Feed prompt assembly from real compacted state**
   - replace the current placeholder/default compaction envelope in ACP prompt assembly with transcript-backed compacted context.

5. **Expose compaction state in operator surfaces**
   - session-level compaction history,
   - artifact visibility,
   - trigger/policy provenance.

### Practical migration summary

Current status can be summarized as:

- **Cleanup and test-alignment phase:** complete
- **Compaction architecture/contract/schema phase:** complete
- **Compaction primitives and regression scaffold:** complete
- **First runtime prompt-path foothold:** complete
- **End-to-end transcript-backed Den-owned compaction:** not yet complete

This means compaction is no longer a missing design blocker in the Letta migration, but it is still not the final end-to-end runtime implementation.
