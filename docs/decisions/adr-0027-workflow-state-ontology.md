# Workflow State Ontology — Architecture Decision Record

## Status: Approved

## Date: 2026-05-12
## Updated: 2026-05-13

---

## Context

BEARS currently exposes several adjacent but distinct operational concepts to agents and humans:

- ACP permission and tool-availability state (`Ask` / `Plan` / `Write`, tool classes, current turn capabilities);
- live workboard state (`den.work_plan.*`);
- ACP plan-mode state (`enter_plan_mode`, `exit_plan_mode`, `record_plan_approval`);
- semantic memory state (`den.memory.*`);
- execution state (workspace mutation, process execution, browser tools, approvals, and results).

Each area is individually reasonable, but together they create an affordance problem:

- the same interaction may mention plan, artifact, summary, status, and memory in adjacent turns;
- several tool families create durable records with paths, ids, and approval metadata;
- per-turn capability changes are described in prose alongside workflow reminders;
- model-facing and human-facing labels do not always make the ontology boundary obvious.

This produces Norman-door-like failures: the locally plausible action can still be the wrong category of action. In particular, the environment currently relies on the agent to maintain a precise internal distinction between:

- planning vs work tracking;
- plan artifacts vs semantic memory;
- current-turn tool availability vs prior-turn state;
- execution-unlocked vs still-in-planning workflow state.

This is an environment design problem, not just a prompt-discipline problem.

Constraints:

- Provider-facing tool names must remain provider-safe and cannot use dotted names.
- Human-visible wording should stay concise and natural; over-encoding every distinction into user-facing names is undesirable.
- The solution should improve both machine affordance and operator understanding.

---

## Decision

BEARS will adopt a **single ontology-aware workflow state model** and expose it consistently across reminders, tool schemas, tool outputs, planning documents, and UI.

The model has four top-level domains:

1. **workplan** — planning and approval-gate state for a proposed course of action;
2. **activity** — live tactical status for active work items and current progress;
3. **memory** — durable semantic role-local or curated knowledge capture;
4. **execution** — current-turn capability state and side-effect-bearing tool availability.

These names are normative for current Den and ACP implementation surfaces. Earlier discussion and draft text may refer to `workflow` and `workboard`; those terms are superseded by `workplan` and `activity` respectively.

These domains must be treated as distinct categories in system design, not just explained as conventions in prompts.

The turn-state model is also the intended replacement for older overlapping state-machine patterns in ACP and Den. Session `mode`, plan-mode gate state, and tool-enablement state may continue to exist as implementation details or compatibility fields, but they must resolve into one canonical per-turn state shape. No Den surface should require agents or operators to mentally merge multiple partially overlapping state machines in order to know what is allowed right now.

---

## Domain model

### 1. Workplan

Workplan is the proposal/approval lane.

Examples:

- entering plan mode;
- submitting an implementation plan;
- recording human approval;
- revising or cancelling a proposed plan.

Properties:

- reviewable;
- approval-aware;
- not semantic memory;
- not the live status list;
- may unlock execution when approved.

### 2. Activity

Activity is the live tactical progress lane.

Examples:

- current task list;
- current item in progress;
- blocked/completed status;
- handoff requests.

Properties:

- short-horizon and operational;
- visible/projection-based;
- not the approval artifact;
- not semantic memory.

### 3. Memory

Memory is durable semantic capture.

Examples:

- note;
- log;
- decision;
- reflection;
- scratch;
- summary.

Properties:

- role-local or curated;
- durable knowledge-oriented record;
- not an active plan;
- not a task list;
- not a run result;
- not a direct `core/` write.

### 4. Execution

Execution is the current-turn capability and effect lane.

Examples:

- workspace mutation available or unavailable;
- process execution enabled or disabled;
- browser tools allowed;
- approval state for effectful tool calls.

Properties:

- authoritative per turn;
- may change after planning approval;
- must supersede prior-turn assumptions.

---

## Consequences for design

### A. One authoritative turn-state summary

Each turn should expose a concise authoritative state block that names the active state in workplan, activity, memory, and execution terms.

Minimum desired fields:

- `permission_mode`
- `tool_classes`
- `workplan.state` (`inactive`, `drafting`, `submitted_waiting_approval`, `approved`, `cancelled`)
- `workplan.approval_status`
- `activity.plan_id` / summary when present
- `activity.status`
- `execution.execution_unlocked`
- `memory.active_plan_write_allowed` (expected `false`)
- `state_version` or equivalent current-turn freshness indicator

This state block should be treated as the canonical per-turn truth. Prior-turn assumptions must not override it.

Derived or legacy state representations must be downstream of this summary, not peers to it. If session mode, approval gate, or tool-enablement records disagree, the system must resolve that disagreement into the authoritative turn-state output rather than exposing the conflict as agent work.

### B. Distinct schemas and return payloads by ontology domain

Tool outputs must reinforce category separation.

#### Workplan outputs

Should look like workplan state, for example:

- workplan id
- approval state
- submitted/approved/rejected timestamps
- execution unlocked or not

They should not primarily present as memory-like file results.

#### Activity outputs

Should look like plan/status projections:

- current item
- statuses
- blockers
- visibility
- handoff status

#### Memory outputs

Should look like semantic memory capture:

- memory entry id
- memory kind
- memory scope
- review eligibility
- durable path or source reference where needed

#### Execution outputs

Should expose current capability state and diagnostics.

### C. Ontology-aware validation

The environment should prefer structural prevention over instruction-only guidance.

Memory writes should reject plan-like content classes.

Examples of disallowed content in `memory_write_entry`:

- active implementation plans;
- task lists intended as current work tracking;
- task intents;
- run results;
- observations;
- direct `core/` updates;
- Cabinet writes.

Planning tools should reject clearly non-workflow semantic-memory content where practical.

### D. Plan artifacts should not present as memory

Workplan artifacts should be represented in a workplan-native namespace and response shape.

Preferred direction:

- use workplan-native ids and state as the primary surfaced representation;
- if a durable path is needed, avoid role-memory-looking locations and labels;
- do not make plan artifacts look like ordinary semantic memory entries;
- do not describe or surface workplan artifacts as MemFS semantic-memory documents, even if implementation details temporarily reuse MemFS-backed storage.

### E. Provider-safe names remain required, but purpose should be surfaced separately

Because dotted provider-facing names are not allowed, BEARS must continue using provider-safe names such as `session_info` or `memory_write_entry`.

To preserve ontology clarity without belabored user-facing names:

- keep provider-safe names compact;
- expose a stronger ontology field in descriptors and UI, such as `domain: workplan | activity | memory | execution`;
- use human-facing titles that emphasize purpose rather than implementation ownership.

This means Option C from the earlier discussion is preferred in a lightweight form: the environment should carry a structured content/domain class even if the human-facing tool name stays concise.

### F. Current-turn state must explicitly supersede prior-turn state

The environment should state this invariant directly.

Suggested reminder language:

> Current-turn capabilities are authoritative. Ignore prior-turn permission assumptions when they conflict with the tool list and state summary shown for this turn.

### G. Replace overlapping mode/gate state machines with turn-state authority

The environment should not rely on agents or clients to reconcile multiple overlapping state machines such as:

- ACP session `current_mode`;
- plan-mode gate state;
- tool-enablement state;
- approval-unlocked execution state.

These may persist internally for compatibility, storage, audit, or migration reasons, but they must feed a single canonical turn-state model rather than remaining co-equal sources of truth.

Requirements:

- one canonical resolution path should derive authoritative turn-state from any legacy/internal records;
- API responses should surface `workflow_state` / turn-state as primary and treat older mode/gate fields as compatibility metadata only;
- new product and operator surfaces should consume the canonical turn-state shape rather than reconstructing policy from `current_mode` plus plan-mode state;
- tests should assert that incompatible combinations cannot leak as visible ambiguity.

---

## Rejected alternatives

### 1. Rely on prompt wording alone

Rejected because the problem is structural. The current environment already contains many instructions, yet adjacent durable concepts still bleed together.

### 2. Encode everything in long human-facing tool names

Rejected because it is cumbersome for humans and unnecessary for providers. The stronger distinction should live in structured ontology fields and response shapes, not only in verbose tool names.

### 3. Defer the unified workflow-state model until after current feature work

Rejected. The ontology problem is already affecting planning, memory, and execution behavior. The model should be a near-term organizing priority rather than a distant cleanup.

---

## Implementation direction

### Near-term requirements

1. Add the workflow-state ontology to planning and architecture docs.
2. Update planning docs so the unified workflow-state model becomes an immediate priority.
3. Add a canonical turn-state summary shape for ACP reminders and related Den surfaces.
4. Add ontology/domain fields to relevant tool descriptors.
5. Adjust plan-mode and activity responses to look workplan-native rather than memory-like.
6. Add misuse rejection for memory tools receiving workplan/activity/task-like content.
7. Align all remaining Den surfaces, operator views, reminders, and audit outputs to the same `workplan` / `activity` / `memory` / `execution` ontology.
8. Replace legacy overlapping mode/gate reasoning with one canonical turn-state resolution path, keeping old fields only as derived or compatibility data where necessary.

### Follow-on requirements

1. Align UI chrome and reminder text with the same four-domain ontology.
2. Align operator views and audit logs around workplan/activity/memory/execution categories.
3. Extend tests to assert ontology separation in tool availability, outputs, and rejection behavior.
4. Remove or rename any remaining implementation surfaces that make workplan artifacts look like role-memory documents.
5. Demote legacy overlapping mode/gate fields so they are no longer treated as co-equal visible state machines next to canonical turn-state.

---

## Acceptance criteria

This decision is successfully implemented when:

- ACP and related Den surfaces expose one authoritative per-turn workflow-state summary.
- Planning, activity, memory, and execution are explicitly represented as separate domains in reminders and/or descriptor metadata.
- Workplan artifacts no longer present as ordinary semantic memory entries or as MemFS semantic-memory documents.
- `memory_write_entry` rejects active-plan/task-intent/run-result-like content.
- Current-turn tool availability is clearly marked as authoritative over prior-turn state.
- Planning documents treat the unified workflow-state model as an active near-term priority.
- Den-wide operator, audit, and API surfaces use the same ontology consistently.
- No Den surface requires clients, agents, or operators to reconcile overlapping `current_mode`, plan-mode, and tool-enablement state machines to determine what is allowed in the current turn.

---

## References

- [ADR 0028: Environment Affordance and Resource Boundaries](./adr-0028-environment-affordance-and-resource-boundaries.md)
- [Task System Implementation Plan](../../planning/TASK_SYSTEM_IMPLEMENTATION_PLAN.md)
- [Memory Tools Implementation Plan](../../planning/MEMORY_TOOLS_IMPLEMENTATION_PLAN.md)
- [Provider-Safe Tool Naming — Architecture Decision Record](provider-safe-tool-naming.md)
- [Semantic Bear Memory — Architecture Decision Record](semantic-bear-memory.md)
- [Schema-first Den-generated path strategy — Architecture Decision Record](schema-first-path-strategy.md)
