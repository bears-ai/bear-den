# Role Environment Prompt-Construction Implementation Plan

## Objective
Implement the shared role-environment prompt-construction architecture described in [`ROLE_ENVIRONMENT_PROMPT_CONSTRUCTION_SPEC.md`](ROLE_ENVIRONMENT_PROMPT_CONSTRUCTION_SPEC.md), moving from prose-first prompt assembly toward schema-first, ontology-aware, role-general environment construction across `talk`, `pair`, `curate`, `work`, and `watch`.

This plan focuses on practical rollout sequencing, integration points, acceptance criteria, and the first concrete runtime slices.

---

## Summary

The implementation should proceed in bounded phases:

1. align terminology and planning docs around the role-general architecture;
2. define the concrete runtime/compiler data model;
3. ship the first structured rendering slice for `pair`;
4. generalize the architecture to `curate` and `watch` as additional direct Letta/API role harnesses;
5. separate transcript, runtime annotations, and compiled prompt artifacts;
6. complete validation, observability, and operator-facing diagnostics.

The implementation goal is not merely to reword prompts. It is to make the environment itself more legible and structurally harder to misuse.

---

## Desired outcomes

A successful rollout should produce the following outcomes:

- prompt construction uses explicit typed/structured intermediate state rather than ad hoc prose concatenation;
- the environment consistently distinguishes `workplan`, `activity`, `memory`, and `execution`;
- active progress representation defaults to planning tools rather than formal artifact submission;
- repeated reminders are consolidated into canonical sections;
- runtime/thread context is visibly separate from stable role-contract content;
- role-specific specialization happens through data/configuration and role contracts, not prompt forks;
- transcript rendering no longer leaks hidden runtime annotations into the visible user-authored chat surface.

---

## Workstreams

### Workstream 1: Terminology and doc alignment

Goal:
- make the role-general architecture explicit across active planning docs.

Tasks:
- keep `ROLE_ENVIRONMENT_PROMPT_CONSTRUCTION_SPEC.md` as the canonical spec;
- link this implementation plan from nearby planning hubs if/when those hubs are updated;
- align terminology with the current ontology:
  - `workplan`
  - `activity`
  - `memory`
  - `execution`
- align wording with the broader context-composition model from `archives/CONTEXT_COMPOSITION_PLAN.md`.

Acceptance criteria:
- active docs no longer present the architecture as `pair`-only;
- role-environment language is consistent with current ontology docs;
- context composition and runtime prompt construction are described as complementary, not competing, concepts.

---

### Workstream 2: Prompt compiler data model

Goal:
- define the concrete runtime structures used to construct a role environment.

Tasks:
- define host-language types or equivalent structured payloads for:
  - `CoreInvariantContext`
  - `RoleContext`
  - `OperationalState`
  - `CapabilityPolicy`
  - `WorkflowPolicy`
  - `InitiativeProfile`
  - `ContextInventory`
  - `MemoryPolicy`
  - `EnvironmentFeedbackPolicy`
- define field provenance for each structure:
  - Den session/runtime state
  - ACP session state
  - role contract/config material
  - memory availability and policy
  - workspace/client metadata
  - user steering or Bear profile inputs where applicable
- define derived fields explicitly rather than inferring them during render.

Implementation notes:
- preserve deterministic render ordering;
- keep the compiler flexible enough for role-specific policy variation without changing the outer schema;
- avoid embedding large prose fragments as primary state.

Acceptance criteria:
- all required structures and fields are defined in implementation-facing form;
- each field has an identified source of truth;
- derived fields are computed before rendering.

---

### Workstream 3: First runtime slice for `pair`

Goal:
- deliver the first meaningful structured prompt-construction improvement in the currently most active interactive role.

Scope:
- `pair` only for this slice, but using the shared architecture.

Tasks:
- render a canonical `[OPERATIONAL SUMMARY]` block;
- render canonical `[WORKFLOW RULES]` and `[PERMISSIONS AND TOOLS]` sections;
- consolidate repeated reminder text into those canonical sections;
- inject planning-representation policy using ontology-aligned language;
- inject context-accounting guidance explicitly;
- ensure derived fields such as `activity_representation_default` and `formal_workplan_artifact_requires_explicit_need` are present.

Acceptance criteria:
- `pair` sees a compact authoritative operational summary each turn;
- plan/activity/memory/execution distinctions are clearer in the prompt;
- reminder duplication is measurably reduced;
- active work representation defaults to `update_plan` rather than accidental artifact submission.

---

### Workstream 4: Additional direct Letta/API role harness rollout

Goal:
- prove the architecture is truly shared by applying it beyond `pair`.

Priority order:
1. `curate`
2. `watch`
3. `talk`
4. `work` as appropriate for its runtime path

#### 4.1 `curate`

Emphasis:
- memory governance
- semantic integration
- review authority
- distinction between durable memory, observations, results, and active workflow state

Tasks:
- instantiate role-specific `MemoryPolicy` and `WorkflowPolicy`;
- ensure prompt construction does not blur review work with active activity tracking;
- preserve common architecture while shifting policy emphasis.

#### 4.2 `watch`

Emphasis:
- observation boundaries
- event/runtime context
- avoiding conflation among observation state, activity state, memory state, and execution capability

Tasks:
- instantiate role-specific `CapabilityPolicy`, `WorkflowPolicy`, and `ContextInventory`;
- ensure observation-oriented current-turn state is represented cleanly.

Acceptance criteria:
- `curate` and `watch` can use the same outer prompt schema;
- specialization happens through structured role data and policy, not forked prompt architecture.

---

### Workstream 5: Transcript, runtime annotation, and compiled prompt separation

Goal:
- structurally separate visible transcript content from model-facing prompt compilation artifacts.

Tasks:
- distinguish three artifacts explicitly:
  1. user-authored message text
  2. turn runtime annotations / hidden operational state
  3. compiled model prompt
- ensure prompt augmentation does not leak hidden reminders into the visible chat transcript;
- define fallback/safe rendering behavior when hidden state exists but should not be displayed as authored user content;
- make this separation visible in debugging and operator tooling where possible.

Why this matters:
- it addresses the earlier duplicated-reminder/user-message leakage problem directly;
- it reduces confusion about what the user actually said versus what the runtime injected.

Acceptance criteria:
- hidden runtime annotations are no longer rendered as user-authored chat content;
- debugging surfaces can still inspect runtime annotations and compiled prompt state;
- transcript semantics and model-input semantics are no longer conflated.

---

### Workstream 6: Validation, testing, and observability

Goal:
- make the new environment architecture testable and inspectable.

Tasks:
- add render tests for canonical section ordering;
- add tests for derived-field correctness;
- add tests preventing ontology collapse across `workplan`, `activity`, `memory`, and `execution`;
- add tests that memory guidance rejects active-plan-style representations;
- add tests preventing duplicate high-level reminders where canonical sections exist;
- add diagnostics or operator-visible debug views for the structured environment payload.

Acceptance criteria:
- schema-first role-environment construction is covered by automated tests;
- prompt changes can be evaluated in a structured way rather than only by anecdotal behavior;
- operators can inspect the assembled structured environment during debugging.

---

## Phase plan

### Phase A: Planning/doc integration

Deliverables:
- canonical role-general spec
- implementation plan
- stronger statement of relationship to context composition

Status:
- in progress / partially complete

### Phase B: Compiler schema and field provenance

Deliverables:
- implementation-facing structures
- field provenance map
- derived-field computation rules

### Phase C: `pair` runtime slice

Deliverables:
- canonical structured summary
- canonical workflow/tool sections
- initial deduplication pass

### Phase D: `curate` and `watch` rollout

Deliverables:
- role-specialized policy instances using the same architecture
- cross-role confidence that the schema is genuinely shared

### Phase E: Transcript/runtime separation

Deliverables:
- clean separation among transcript, hidden annotations, and compiled prompt
- debugging visibility for each layer

### Phase F: Validation and hardening

Deliverables:
- tests
- diagnostics
- operator inspection paths

---

## Concrete near-term next slice

The best immediate engineering slice after this planning work is:

1. define the concrete structured payloads for the role-environment compiler;
2. identify the current `pair` prompt-builder inputs and map them into those payloads;
3. ship a minimal structured rendering with:
   - `[ROLE]`
   - `[OPERATIONAL SUMMARY]`
   - `[PERMISSIONS AND TOOLS]`
   - `[WORKFLOW RULES]`
4. remove or compress the most obviously duplicated reminder prose.

This slice is small enough to implement incrementally but meaningful enough to validate the architecture.

---

## Risks and mitigations

### Risk 1: Rewording without structural change
Mitigation:
- require typed/structured intermediate state before considering the work complete.

### Risk 2: Pair-specific assumptions leak back into the shared design
Mitigation:
- explicitly test the architecture against `curate` and `watch` before declaring it stable.

### Risk 3: Hidden runtime state still leaks into the transcript
Mitigation:
- treat transcript/runtime/prompt separation as a first-class workstream, not cleanup.

### Risk 4: The ontology remains conceptually clear but operationally blurred
Mitigation:
- add validation and render tests that encode the distinctions directly.

### Risk 5: Prompt size grows even as structure improves
Mitigation:
- enforce deduplication and compact section rendering as implementation requirements.

---

## Acceptance criteria for the overall plan

This implementation plan should be considered successful when:

1. at least one active role (`pair`) is using schema-first structured environment rendering;
2. at least two additional roles (`curate` and `watch`) can adopt the same outer architecture with role-specific policy instances;
3. current-turn runtime state is rendered in a compact authoritative section;
4. active progress/activity representation is clearly separated from workplan artifacts and durable memory;
5. visible transcript content is separated from hidden runtime annotations and compiled prompt artifacts;
6. the new environment architecture is testable, inspectable, and easier to evolve safely.
