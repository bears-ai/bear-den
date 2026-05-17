# Role Environment Prompt-Construction Spec

## Objective
Refactor BEARS role-environment construction from prose-first concatenation into schema-first prompt assembly with explicit layer separation, compact operational summaries, structured policies, initiative configuration, context accounting, memory policy, and environment feedback support.

This spec defines a shared prompt-construction architecture for BEARS role environments. Role-specific environments such as `talk`, `pair`, `curate`, `work`, and `watch` should all be composed from the same layered model, with role contracts and runtime capabilities providing specialization. `pair` is the first concrete worked example, not the exclusive target.

---

## Scope across roles

This specification applies to all Bear roles.

It is especially relevant to direct Letta/API role harnesses such as `pair`, `curate`, and `watch`, where prompt assembly, runtime state, tool capability boundaries, memory boundaries, and workflow ontology must remain legible both to the agent and to operators.

The architecture should be shared across roles, while role-specific behavior should be expressed through structured role contracts, capability policies, workflow policies, and runtime context rather than ad hoc prompt forks.

## Relationship to context composition

This spec is the conceptual and architectural document for role-environment prompt construction. The concrete implementation-facing schema contract lives in `ROLE_ENVIRONMENT_PROMPT_COMPILER_SCHEMA.md`.

This spec is the runtime/prompt-rendering specialization of the broader BEARS context-composition direction documented in `archives/CONTEXT_COMPOSITION_PLAN.md`.

That broader plan defines the conceptual layered model:
- Den baseline
- role contracts
- user steering
- Bear context
- runtime/thread context

This spec narrows the focus to the agent-facing role environment that is constructed at runtime for a specific Bear role.

The intended mapping is:
- **CoreInvariantContext** corresponds to stable Den/baseline invariants.
- **RoleContext** corresponds to the role-contract layer.
- **InitiativeProfile** may be influenced by user steering and role defaults.
- **ContextInventory** and **OperationalState** correspond to runtime/thread context and already-available contextual inputs.
- **CapabilityPolicy**, **WorkflowPolicy**, and **MemoryPolicy** make role/runtime policy explicit rather than leaving it diffused across prose.

This means the context-composition model remains the conceptual authoring architecture, while this spec defines how those layers should be rendered into a structured, ontology-aware role environment for model consumption.

Different roles will emphasize different policy areas:

- `talk` emphasizes conversational steering, user interaction, and handoff clarity.
- `pair` emphasizes collaborative client-mediated tool use, live activity/progress tracking, and mutation/execution boundaries.
- `curate` emphasizes semantic integration, review authority, memory governance, and durable knowledge boundaries.
- `work` emphasizes approved execution, task authorization, and result/reporting flows.
- `watch` emphasizes observation, event interpretation, and boundaries between observation, activity, memory, and execution.

---

## 1. Design Goals

1. Distinguish clearly between:
   - stable invariants
   - role-specific behavior
   - operational policy
   - turn-local state
   - available context
   - style/initiative preferences
2. Reduce repeated prose reminders.
3. Surface a compact authoritative operational summary at the top of each turn.
4. Prevent the agent from ignoring context already available via instructions, memory, open files, or discoverable workspace guidance.
5. Make durable-memory expectations explicit.
6. Provide a structured path for reporting environment friction.
7. Keep rendering deterministic and low-redundancy.
8. Make planning representation explicit so live activity/progress tracking defaults to planning tools rather than artifact submission.
9. Preserve a shared environment architecture across roles while keeping role-specific contracts and capabilities explicit.

---

## 2. Prompt Assembly Model

Prompt construction should shift from freeform concatenated templates to:

1. **Build structured environment objects** from runtime state and configuration.
2. **Normalize and derive fields** so the model does not need to infer obvious consequences.
3. **Render stable prompt sections** in a fixed order.
4. **Deduplicate semantically overlapping rules** before final emission.

### Rendering order
1. Core invariants
2. Role definition
3. Operational summary
4. Permissions and tools
5. Workflow and stopping rules
6. Initiative profile
7. Context inventory
8. Memory policy
9. Environment feedback policy
10. Optional explanatory prose / human-readable elaboration

This order should be stable across turns unless a deliberate schema version change occurs.

This layered composition is consistent with the broader role-aware context composition direction in BEARS: stable baseline and role contracts should remain distinct from steering and runtime context, and runtime/thread context should not be flattened into the same conceptual layer as stable prompt content.

---

## 3. Internal Data Model

The prompt-construction system should assemble the following internal objects before rendering:

### 3.1 CoreInvariantContext
Fields:
- `safety_rules`
- `global_behavior_contract`
- `hard_boundaries`
- `instruction_precedence_notes`

Purpose:
- Holds stable system-level constraints that do not vary by Bear role or session state.

### 3.2 RoleContext
Fields:
- `bear_name`
- `agent_role`
- `role_mission`
- `memory_scope`
- `allowed_memory_paths`
- `role_contract_version` (optional)

Purpose:
- Encodes stable role identity and collaboration expectations.
- Serves as the structured representation of the role-contract layer in the broader context-composition model.

### 3.3 OperationalState
Fields:
- `permission_mode`
- `tool_classes`
- `execution_unlocked`
- `workspace_roots`
- `session_id`
- `plan_state`
- `plan_approval_status`
- `activity_status`
- `activity_plan_id`
- `activity_current_item`
- `can_read_workspace`
- `can_edit_workspace`
- `can_use_web`
- `can_write_memory`
- `can_submit_plan`

Purpose:
- Provides authoritative current-turn operational facts.
- Should reflect concrete turn state rather than summary projections of broader workflow policy.

### 3.4 CapabilityPolicy
Fields:
- `allowed_tool_families`
- `allowed_tool_ids`
- `disallowed_actions`
- `identity_trust_rules`

Purpose:
- Canonical source for what tools/actions are allowed this turn and how to interpret trusted identity.
- Uses canonical BEARS tool identity (`ToolId`) plus compact grouping (`ToolFamily`) rather than provider/runtime-specific tool-name strings.
- Should vary by role and runtime surface without requiring the rest of the prompt architecture to fork.

### 3.5 WorkflowPolicy
Fields:
- `memory_vs_plan_rules`
- `plan_representation_rules`
- `stop_conditions`

Purpose:
- Encodes activity/workplan representation rules, memory boundaries, and stopping conditions.
- Role-specific policy emphasis may differ, but the environment should always preserve the ontology distinctions among workplan, activity, memory, and execution.
- Continuation-after-tool-results behavior should be treated as a compiler/runtime invariant unless real variability emerges.

### 3.6 InitiativeProfile
Fields:
- `initiative_level`
- `response_style`
- `risk_posture`

Purpose:
- Makes agent initiative and answer style explicitly configurable.
- Different roles may reasonably have different defaults.

### 3.7 ContextInventory
Fields:
- `instructions_present`
- `memory_available`
- `workspace_access`
- `open_files_count`
- `open_file_paths` (optional, bounded)
- `guidance_files_known` (e.g. AGENTS.md / README.md presence if host already knows)
- `work_surface_known`
- `work_surface_kind`
- `work_surface_label`
- `work_surface_refs`
- `surface_guidance_sources`
- `surface_memory_sources`
- `surface_cabinet_sources`
- `work_surface_resolution_available`
- `work_surface_resolution_hint`
- `preloaded_summaries_present`

Purpose:
- Helps the model account for context already available before asking for more.
- Corresponds to the runtime/thread context layer in the broader role-aware composition model.
- Discovery behavior should be treated as a compiler/runtime rule rather than a per-turn schema field.

### 3.8 MemoryPolicy
Fields:
- `durable_memory_good_candidates`
- `durable_memory_bad_candidates`
- `promotion_guidance`
- `confidence_expectations`
- `review_recommendations`

Purpose:
- Clarifies what should and should not become durable memory.
- Especially important for roles like `curate`, but should be explicit across all roles.

### 3.9 EnvironmentFeedbackPolicy
Fields:
- `may_report_environment_friction`
- `reportable_categories`
- `preferred_feedback_format`

Purpose:
- Enables the agent to surface instruction conflicts, tooling gaps, and workflow friction.

---

## 4. Required Rendering Sections

### 4.1 [ROLE]
A short stable identity block.

Example shape:
```text
[ROLE]
bear=Builder Bear
agent_role=pair
memory_scope=pair_local
```

Requirements:
- Must remain short.
- Must not duplicate operational state.
- Should encode the role-contract layer, not ad hoc runtime state.

### 4.2 [OPERATIONAL SUMMARY]
A compact top-of-turn summary block rendered from `OperationalState`.

Example shape:
```text
[OPERATIONAL SUMMARY]
permission_mode=Plan
tool_classes=WorkspaceRead
can_read_workspace=true
can_edit_workspace=false
execution_unlocked=false
workspace_roots=/workspace
plan_state=Drafting
plan_approval_status=Drafting
activity_status=Inactive
```

Requirements:
- Must contain authoritative current-turn state.
- Must include derived booleans where helpful.
- Must avoid long explanatory prose.
- Must represent current-turn runtime/thread context rather than stable role contract content.
- Any compact operational-summary projection of plan-representation behavior should be derived from workflow policy rather than stored as an independent source-of-truth field.

### 4.3 [PERMISSIONS AND TOOLS]
Structured permissions/tooling section.

Content:
- allowed tool families
- disallowed action classes
- trust semantics for identity/session
- special tool-routing rules

Requirements:
- One normalized statement per rule category.
- No repeated prose examples unless necessary.
- Must allow role-specific differences without changing the surrounding prompt architecture.

### 4.4 [WORKFLOW RULES]
Structured workflow section covering:
- activity/workplan representation rules
- when to use `update_plan`
- when to use `enter_plan_mode` / `exit_plan_mode`
- when not to use memory tools
- stop conditions

Requirements:
- Should replace repeated freeform warnings elsewhere.
- Must distinguish active plan state from formal artifact submission.
- Should be adaptable by role while preserving ontology boundaries.
- Continuation-after-tool-results behavior should generally be treated as a compiler/runtime invariant rather than repeated as a per-turn field.

#### Planning Representation Policy
- Use `update_plan` for active task planning, decomposition, and progress tracking.
- Prefer `update_plan` for live activity/progress tracking when the user asks for a plan unless they explicitly ask for a formal workplan document or approval artifact.
- Use `enter_plan_mode` / `exit_plan_mode` when a durable markdown implementation plan artifact is specifically needed for approval, review, or audit.
- Do not substitute memory entries or stored artifacts for active plan state.

Note: this policy will be most directly relevant to interactive roles such as `pair` and `talk`, but the architecture should preserve the same distinction across other roles so that `curate`, `work`, and `watch` do not collapse activity, workplan, memory, and execution into one undifferentiated layer.

### 4.5 [INITIATIVE PROFILE]
Rendered only if configured.

Example shape:
```text
[INITIATIVE PROFILE]
initiative_level=High
response_style=Balanced
risk_posture=Conservative
```

Requirements:
- Values must come from configuration, not prose heuristics.
- Should be configurable per role.

### 4.6 [CONTEXT INVENTORY]
Structured accounting of already-available context.

Example shape:
```text
[CONTEXT INVENTORY]
instructions_present=true
memory_available=true
workspace_access=true
open_files_count=0
guidance_files_known=unknown
work_surface_known=false
work_surface_resolution_available=true
work_surface_resolution_hint=prefer service/deployment/monitor target resolution before broad search when the task appears surface-bound
```

Requirements:
- Should bias the model away from redundant orientation requests.
- Should map cleanly to runtime/thread context rather than stable steering or role-contract content.
- Discovery behavior should be described normatively by the compiler/runtime rather than stored as a per-turn `discovery_policy` field.

### 4.7 [MEMORY POLICY]
Explicit durable-memory guidance.

Content:
- examples of stable facts worth storing
- examples of transient facts not worth storing
- reminder that active plans belong in planning systems, not durable memory

Requirements:
- Structured bullets or key-value lines preferred over prose paragraphs.
- Must remain role-aware while preserving common architecture.

### 4.8 [ENVIRONMENT FEEDBACK POLICY]
Defines how the agent may surface environment problems.

Example shape:
```text
[ENVIRONMENT FEEDBACK POLICY]
may_report_environment_friction=true
reportable_categories=instruction_conflict,prompt_redundancy,tool_gap,workflow_confusion,memory_policy_gap,plan_representation_confusion
preferred_feedback_format=structured_then_brief_explanation
```

Requirements:
- Must make environment-improvement reporting explicitly allowed.

---

## 5. Deduplication Rules

Before rendering final prompt text, run a deduplication pass over policy statements.

### Deduplicate these categories:
- stop conditions
- editing disallowance
- statements about authoritative current-turn runtime state
- plan-vs-memory usage rules
- plan representation rules
- identity trust rules

### Strategy:
1. Normalize rules to standard rule categories.
2. Keep one authoritative wording per category.
3. Optionally allow one brief “critical reminder” restatement only for highest-risk rules.

### Do not deduplicate away:
- stable role identity
- turn summary block
- explicit hard safety boundaries
- role-contract distinctions

---

## 6. Derived-State Requirements

Prompt assembly must compute and inject derived fields rather than forcing the model to infer them from raw state.

Minimum required derived fields:
- `can_read_workspace`
- `can_edit_workspace`
- `can_write_memory`
- `can_submit_plan`
- `environment_focus_supported` (e.g. tools/workflow/instructions are fair game)

Rationale:
- Reduces inference burden.
- Makes behavior more consistent.
- Summary projections of plan-representation behavior should be derived at render time from workflow policy rather than stored as separate source-of-truth state fields.

---

## 7. Context Accounting Rules

The environment should explicitly teach the model to account for already-present context before asking for more.

### Required rule
Include a concise rule equivalent to:
- Before asking for orientation or documentation, first account for context already available through instructions, memory, open files, and trusted workspace discovery.

### Preferred host-side enhancement
If the runtime already knows any of the following, inject them directly:
- open file list
- presence of AGENTS.md
- presence of README.md
- prior summarized session context
- loaded memory summaries

This reduces unnecessary workspace scans and redundant requests.

---

## 8. Memory Policy Requirements

The environment must distinguish durable memory from active work state.

### Required durable-memory guidance
Good candidates:
- stable user preferences
- recurring workflow expectations
- long-lived project conventions
- frequently reused commands
- durable collaboration norms

Poor candidates:
- temporary debugging trails
- active task progress
- speculative observations
- one-off review findings unless promoted deliberately

### Required plan/memory rule
Active plans, task lists, and implementation progress belong in planning/workflow systems, not durable memory entries.

---

## 9. Environment Feedback Support

The environment should explicitly allow the agent to report friction in the environment itself.

### Reportable categories
- instruction conflict
- prompt redundancy
- tool gap
- workflow confusion
- context-visibility gap
- memory-policy ambiguity
- plan representation confusion

### Preferred feedback schema
If a structured channel exists, shape feedback around:
- `category`
- `severity`
- `symptom`
- `likely_cause`
- `proposed_fix`
- `requires_code_change`

If no structured sink exists yet, at minimum make such reporting permissible in conversational output.

---

## 10. Rendering Guidelines

1. Prefer stable headings and short structured lines.
2. Keep summaries compact; move explanations below structured source-of-truth facts.
3. Minimize synonymous restatements.
4. Keep examples short and policy-relevant.
5. Avoid mixing stable rules with turn-specific facts in the same section.
6. Ensure turn-state sections explicitly override earlier assumptions where needed.
7. Separate active plan state, plan artifacts, and durable memory in both prompt structure and wording.
8. Preserve the layer boundaries implied by the broader role-aware context composition model: baseline and role contracts should remain distinct from steering and runtime/thread context.

---

## 11. Backward-Compatibility Strategy

To avoid abrupt behavior changes:

### Phase A
- Introduce `[OPERATIONAL SUMMARY]` and `[CONTEXT INVENTORY]` alongside existing reminders.
- Keep existing prose, but mark new structured blocks as authoritative.
- Add structured plan-representation guidance while preserving older plan-mode phrasing.

### Phase B
- Convert repeated reminder prose into standardized structured sections.
- Deduplicate overlapping language.
- Make `update_plan` the explicit default for live activity/progress tracking.

### Phase C
- Move fully to schema-first prompt construction.
- Retain only minimal explanatory prose where evidence shows it helps.
- Reserve artifact submission guidance for explicit approval/review use cases.
- Generalize the architecture consistently across `talk`, `pair`, `curate`, `work`, and `watch`.

---

## 12. Acceptance Criteria

A compliant implementation should make it more likely that the agent:
1. distinguishes stable rules from turn-local state;
2. does not ask for context already present;
3. behaves consistently under read-only vs editable turns;
4. uses memory more selectively and durably;
5. reports environment/tooling friction clearly;
6. answers environment questions as environment questions rather than repo questions;
7. uses `update_plan` by default for active work planning rather than prematurely submitting plan artifacts;
8. preserves the same environment architecture across roles while allowing role-specific contracts and runtime policies.

---

## 13. Minimal Example Render

```text
[ROLE]
bear=Builder Bear
agent_role=pair
memory_scope=pair_local

[OPERATIONAL SUMMARY]
permission_mode=Plan
tool_classes=WorkspaceRead
can_read_workspace=true
can_edit_workspace=false
execution_unlocked=false
workspace_roots=/workspace
plan_state=Drafting
plan_approval_status=Drafting
activity_status=Inactive

[PERMISSIONS AND TOOLS]
allowed_tool_families=WorkspaceRead,Memory,Planning,Identity,WebFetch
allowed_tool_ids=FsReadTextFile,SessionInfo,MemoryRead,UpdatePlan,WebFetch
identity_trust_rules=SessionInfoIsTrustedIdentitySource
disallowed_actions=WorkspaceMutation

[WORKFLOW RULES]
stop_conditions=UserJudgment,RequiredApproval,MissingInformation,UnrecoverableError
plan_representation_rules=UseUpdatePlanForLiveActivityProgressTracking,FormalWorkplanArtifactsRequireExplicitNeed
memory_vs_plan_rules=DoNotUseDurableMemoryForActivePlansOrEphemeralProgress

[INITIATIVE PROFILE]
initiative_level=High
response_style=Balanced
risk_posture=Conservative

[CONTEXT INVENTORY]
instructions_present=true
memory_available=true
workspace_access=true
open_files_count=0
work_surface_known=false
work_surface_resolution_available=true
work_surface_resolution_hint=prefer service/deployment/monitor target resolution before broad search when the task appears surface-bound

[MEMORY POLICY]
durable_memory_good_candidates=stable_preferences,recurring_commands,long_lived_conventions
durable_memory_bad_candidates=active_task_state,transient_debugging,speculative_notes

[ENVIRONMENT FEEDBACK POLICY]
may_report_environment_friction=true
reportable_categories=InstructionConflict,PromptRedundancy,ToolGap,WorkflowConfusion,PlanRepresentationConfusion
```

---

## 14. Non-Goals

This spec does not yet define:
- the concrete host-language types
- the exact rendering library/template mechanism
- telemetry collection details
- UI presentation changes outside prompt construction

Those belong in the implementation plan.
