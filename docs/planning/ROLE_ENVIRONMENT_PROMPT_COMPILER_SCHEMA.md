# Role Environment Prompt Compiler Schema

## Purpose
This document defines the concrete schema for the first implementation of BEARS role-environment prompt construction.

It exists to make implementation unambiguous without turning prompt construction into a heavyweight framework. The goal is to provide the minimum structure needed to:
- build prompts from explicit runtime/context objects rather than prose piles;
- preserve the ontology distinctions among `workplan`, `activity`, `memory`, and `execution`;
- keep the architecture shared across roles while allowing role-specific policy differences;
- support a first real implementation slice for `pair` without collapsing back into pair-only design.

This is an implementation-facing contract, not a user-facing product model and not a final external API specification.

---

## Scope

### In scope
- the root prompt-construction context object;
- the required sub-objects for the first implementation slice;
- field-level provenance and derivation expectations;
- the first render contract for structured prompt sections;
- deduplication and validation rules;
- one concrete `pair` example and one non-`pair` example.

### Out of scope
- a generalized prompt DSL;
- user-facing prompt editing UX;
- a final cross-service API schema;
- telemetry/storage design beyond what the compiler needs;
- full role-specific policy catalogs for every role.

---

## Design principles

1. **Schema-first, not prose-first.** The compiler should render from structured state, not concatenate overlapping reminder paragraphs.
2. **Shared outer shape, role-specific values.** Roles should mostly share the same prompt-construction schema. Specialization should happen through role contracts and policy values, not prompt forks.
3. **Current-turn state is authoritative.** Runtime capability state for the current turn must override prior assumptions.
4. **Ontology boundaries stay explicit.** `workplan`, `activity`, `memory`, and `execution` should not collapse into one undifferentiated status blob.
5. **Keep slice 1 small.** The first real implementation should focus on a few required objects and a small number of rendered sections.

---

## Implementation slices

### Slice 1: required now
These objects are required for the first real implementation:
- `RoleContext`
- `OperationalState`
- `CapabilityPolicy`
- `WorkflowPolicy`

These sections are required in the first render:
- `[ROLE]`
- `[OPERATIONAL SUMMARY]`
- `[PERMISSIONS AND TOOLS]`
- `[WORKFLOW RULES]`

### Slice 1.5: strongly preferred if cheap
These are useful if they are easy to wire up during the first slice:
- `ContextInventory`
- `MemoryPolicy`

These sections are preferred if available:
- `[CONTEXT INVENTORY]`
- `[MEMORY POLICY]`

### Future extension
These should not block the first slice:
- `InitiativeProfile`
- `EnvironmentFeedbackPolicy`
- richer role-specific policy variants
- more detailed context provenance/debug views

---

## Root object

The compiler should consume a single root object:

```text
RoleEnvironmentPromptContext
- schema_version: string
- role: RoleContext
- operational_state: OperationalState
- capability_policy: CapabilityPolicy
- workflow_policy: WorkflowPolicy
- context_inventory?: ContextInventory
- memory_policy?: MemoryPolicy
- initiative_profile?: InitiativeProfile
- environment_feedback_policy?: EnvironmentFeedbackPolicy
- render_policy?: RenderPolicy
```

### Notes
- `schema_version` is required so the renderer and tests can evolve deliberately.
- The first implementation may omit optional objects entirely rather than including placeholder empty values.
- The outer shape should stay stable even if some optional objects are not yet populated.

---

## Required object definitions

## 1. `RoleContext`

### Purpose
Represents stable role identity and role-contract-level context for the current role environment.

### Schema
```text
RoleContext
- bear_name: string
- agent_role: "talk" | "pair" | "curate" | "work" | "watch"
- role_mission?: string
- memory_scope?: string
- allowed_memory_paths?: string[]
- role_contract_version?: string
```

### Required in slice 1
- `bear_name`
- `agent_role`

### Optional in slice 1
- all other fields

### Source of truth
- Bear/role runtime configuration
- role harness configuration
- role contract material if already explicitly available

### Notes
- This object should describe stable role identity, not current-turn tool state.
- If role-contract content is verbose, do not inline it all here; keep this object concise and structured.

---

## 2. `OperationalState`

### Purpose
Represents current-turn authoritative runtime state.

### Schema
```text
OperationalState
- permission_mode: PermissionMode
- tool_classes: ToolFamily[]
- execution_unlocked: boolean
- workspace_roots?: string[]
- session_id?: string
- plan_state?: PlanState
- plan_approval_status?: PlanApprovalStatus
- activity_status?: ActivityStatus
- activity_plan_id?: string | null
- activity_current_item?: string | null
- can_read_workspace: boolean
- can_edit_workspace: boolean
- can_use_web?: boolean
- can_write_memory?: boolean
- can_submit_plan?: boolean
```

### Required in slice 1
- `permission_mode`
- `tool_classes`
- `execution_unlocked`
- `can_read_workspace`
- `can_edit_workspace`

### Strongly preferred in slice 1
- `workspace_roots`
- `plan_state`
- `plan_approval_status`
- `activity_status`

### Source of truth
- ACP or channel turn capability state
- Den workplan/activity state where available
- runtime session binding and workspace metadata

### Derived fields
The compiler should derive at least these fields before rendering:
- `can_read_workspace`
- `can_edit_workspace`

Optional derived fields:
- `can_use_web`
- `can_write_memory`
- `can_submit_plan`

### Notes
- This object is where current-turn authority is reflected through concrete state fields such as permission mode, execution state, plan approval state, and edit/read capability. The renderer should not infer that authority from prose.
- If a field is unavailable in a given runtime, omit it rather than inventing a guessed value unless the host can derive it confidently.

---

## 3. `CapabilityPolicy`

### Purpose
Represents what the role is allowed to do this turn and how tool/identity rules should be interpreted.

### Schema
```text
CapabilityPolicy
- allowed_tool_families?: ToolFamily[]
- allowed_tool_ids?: ToolId[]
- disallowed_actions?: DisallowedAction[]
- identity_trust_rules?: IdentityTrustRule[]
```

### Required in slice 1
At least one of the following must be present:
- `allowed_tool_families`
- `allowed_tool_ids`

Strongly preferred:
- `identity_trust_rules`

### Source of truth
- current-turn tool availability
- Den role policy
- channel/runtime policy
- session identity trust semantics

### Notes
- Tool identifiers should be canonical BEARS `ToolId` values rather than provider/runtime-specific names.
- Tool families provide compact `ToolFamily` summaries; tool ids provide concrete capability identity.
- In Rust implementation, `ToolFamily`, `ToolId`, `DisallowedAction`, and `IdentityTrustRule` should be explicit enums.
- Runtime binding/source details such as local vs server-hosted tools may still matter operationally, but they should not be the primary identity model in this schema.
- Keep rules normalized and compact.
- Prefer a few normalized statements over many similar prose fragments.
- This object should carry operational policy, not generic role identity.

---

## 4. `WorkflowPolicy`

### Purpose
Represents activity/workplan representation rules and related stopping rules that the role should follow this turn or in this environment.

### Schema
```text
WorkflowPolicy
- memory_vs_plan_rules?: MemoryVsPlanRule[]
- plan_representation_rules?: PlanRepresentationRule[]
- stop_conditions?: StopCondition[]
```

### Required in slice 1
At minimum:
- `stop_conditions`

Strongly preferred:
- `plan_representation_rules`
- `memory_vs_plan_rules`

### Source of truth
- Den workplan/activity ontology and policy
- role policy
- current runtime/tool behavior expectations

### Notes
- This is where active activity representation vs formal workplan artifact guidance belongs.
- The object name is a pragmatic umbrella, but the fields inside it should stay aligned to the more specific ontology terms where possible.
- Approval-sensitive behavior should primarily come from current-turn state such as `plan_approval_status`, `plan_state`, `execution_unlocked`, and `stop_conditions`, rather than a separate approval-rules bucket.
- Continuation after tool results should be treated as a compiler/runtime invariant rather than a per-turn schema field unless a real need for variability emerges.
- This object should encode normalized activity/workplan behavior rules rather than duplicating every legacy reminder.

---

## Canonical enum-backed vocabulary

The following schema types are intended as explicit enums in Rust implementation.

### `BearRole`
```text
BearRole = Talk | Pair | Curate | Work | Watch
```

### `PermissionMode`
```text
PermissionMode = Ask | Plan | Write | Observe
```

### `ToolFamily`
```text
ToolFamily =
- WorkspaceRead
- WorkspaceMutation
- Execution
- Browser
- Memory
- Planning
- Identity
- WebFetch
- Git
```

### `ToolId`
Canonical BEARS tool identifiers should be modeled as an enum in Rust.

Illustrative values:
```text
ToolId =
- FsReadTextFile
- FsListDirectory
- FsSearchFiles
- FsEditFile
- FsCreateTextFile
- ProcessRun
- ChromeOpen
- ChromeSnapshot
- SessionInfo
- MemoryRead
- MemoryWriteEntry
- UpdatePlan
- EnterPlanMode
- ExitPlanMode
- RecordPlanApproval
- WebFetch
- GitStatus
```

The exact enum members should track the canonical BEARS tool surface, not provider/runtime-specific names.

### `DisallowedAction`
```text
DisallowedAction =
- WorkspaceMutation
- AutonomousOutboundExecution
```

### `IdentityTrustRule`
```text
IdentityTrustRule =
- SessionInfoIsTrustedIdentitySource
```

### `PlanState`
```text
PlanState =
- Drafting
- SubmittedWaitingApproval
- Approved
- Cancelled
```

### `PlanApprovalStatus`
```text
PlanApprovalStatus =
- Drafting
- SubmittedWaitingApproval
- ApprovedExecutionUnlocked
- Cancelled
```

### `ActivityStatus`
```text
ActivityStatus =
- Inactive
- Active
- Blocked
- Completed
```

### `MemoryVsPlanRule`
```text
MemoryVsPlanRule =
- DoNotUseDurableMemoryForActivePlansOrEphemeralProgress
```

### `PlanRepresentationRule`
```text
PlanRepresentationRule =
- UseUpdatePlanForLiveActivityProgressTracking
- FormalWorkplanArtifactsRequireExplicitNeed
```

### `StopCondition`
```text
StopCondition =
- UserJudgment
- RequiredApproval
- MissingInformation
- UnrecoverableError
```

### `InitiativeLevel`
```text
InitiativeLevel = Low | Medium | High
```

### `ResponseStyle`
```text
ResponseStyle = Concise | Balanced | Detailed
```

### `RiskPosture`
```text
RiskPosture = Conservative | Balanced | Aggressive
```

### `ReportableCategory`
```text
ReportableCategory =
- InstructionConflict
- PromptRedundancy
- ToolGap
- WorkflowConfusion
- PlanRepresentationConfusion
```

Fields that remain strings or string lists in this document are either identifiers/paths/labels or still-evolving descriptive hints, not intended closed vocabularies.

---

## Optional object definitions

## 5. `ContextInventory`

### Purpose
Helps the model account for already-available context before asking for more orientation, including whether the current task appears anchored to a known work surface and which nearby sources should be preferred.

### Schema
```text
ContextInventory
- instructions_present?: boolean
- memory_available?: boolean
- workspace_access?: boolean
- open_files_count?: number
- open_file_paths?: string[]
- guidance_files_known?: string[] | "unknown"
- work_surface_known?: boolean
- work_surface_kind?: string
- work_surface_label?: string
- work_surface_refs?: string[]
- surface_guidance_sources?: string[]
- surface_memory_sources?: string[]
- surface_cabinet_sources?: string[]
- work_surface_resolution_available?: boolean
- work_surface_resolution_hint?: string
- preloaded_summaries_present?: boolean
```

### Use in slice 1
Optional but recommended if the runtime already knows any of these values cheaply.

### Source of truth
- current runtime context
- host/editor integration
- memory/tool availability state
- work-surface inference/resolution signals where available

### Notes
- Keep this object as the single place where the compiler describes available and surface-anchored context.
- Do not split out a separate top-level work-surface object unless the compiler later needs substantially richer work-surface behavior.
- Discovery behavior is a compiler/runtime rule, not a per-turn schema field.
- The compiler should first account for already-available context before requesting more orientation.
- If a work surface is known, discovery should prefer surface-anchored sources before broad search.
- If a work surface is not known and the task appears surface-bound, discovery should prefer available work-surface resolution signals/processes before broad untargeted search.

---

## 6. `MemoryPolicy`

### Purpose
Makes durable-memory expectations explicit.

### Schema
```text
MemoryPolicy
- durable_memory_good_candidates?: string[]
- durable_memory_bad_candidates?: string[]
- promotion_guidance?: string[]
- confidence_expectations?: string[]
- review_recommendations?: string[]
```

### Use in slice 1
Optional but useful for preventing plan/memory confusion.

### Source of truth
- role memory policy
- workplan/activity/memory ontology
- current memory-tool behavior expectations

---

## 7. `InitiativeProfile`

### Purpose
Represents configured initiative/answer-style defaults.

### Schema
```text
InitiativeProfile
- initiative_level?: InitiativeLevel
- response_style?: ResponseStyle
- risk_posture?: RiskPosture
```

### Use in slice 1
Do not block the first implementation on this object.

---

## 8. `EnvironmentFeedbackPolicy`

### Purpose
Makes environment-friction reporting explicitly allowed.

### Schema
```text
EnvironmentFeedbackPolicy
- may_report_environment_friction?: boolean
- reportable_categories?: ReportableCategory[]
- preferred_feedback_format?: string
```

### Use in slice 1
Optional. Useful, but not required to begin structured rendering.

---

## 9. `RenderPolicy`

### Purpose
Holds renderer-specific formatting rules.

### Schema
```text
RenderPolicy
- section_order?: string[]
- omit_empty_sections?: boolean
```

### Use in slice 1
Optional. If omitted, the default render contract in this document applies.

---

## Field provenance table

This table covers the fields most important for slice 1.

| Field | Source | Required in slice 1 | Derived | Notes |
|---|---|---:|---:|---|
| `role.bear_name` | Bear/runtime config | yes | no | stable role environment identity |
| `role.agent_role` | role harness config | yes | no | stable role identity |
| `operational_state.permission_mode` | current-turn capability state | yes | no | authoritative for this turn |
| `operational_state.tool_classes` | current-turn capability state | yes | no | canonical turn capability summary |
| `operational_state.execution_unlocked` | current-turn capability state | yes | no | execution lane summary |
| `operational_state.workspace_roots` | session/workspace metadata | preferred | no | do not guess |
| `operational_state.can_read_workspace` | derived from turn capabilities | yes | yes | do not infer during render |
| `operational_state.can_edit_workspace` | derived from turn capabilities | yes | yes | do not infer during render |
| `capability_policy.allowed_tool_families` | current-turn tool availability + canonical tool-family mapping | one-of | no | compact capability summary |
| `capability_policy.allowed_tool_ids` | current-turn tool availability + canonical BEARS tool id mapping | one-of | no | concrete tool identity |
| `capability_policy.identity_trust_rules` | Den/session trust policy | preferred | no | should remain canonical |
| `workflow_policy.stop_conditions` | activity/workplan/runtime policy | yes | no | compact canonical list |
| `workflow_policy.plan_representation_rules` | activity/workplan/runtime policy | preferred | no | separates activity from workplan artifact |
| `workflow_policy.memory_vs_plan_rules` | activity/workplan/runtime policy | preferred | no | prevents memory misuse for active work |

---

## Render contract

The compiler should render sections in the following default order:

1. `[ROLE]`
2. `[OPERATIONAL SUMMARY]`
3. `[PERMISSIONS AND TOOLS]`
4. `[WORKFLOW RULES]`
5. `[CONTEXT INVENTORY]` if present
6. `[MEMORY POLICY]` if present
7. `[INITIATIVE PROFILE]` if present
8. `[ENVIRONMENT FEEDBACK POLICY]` if present

### General rendering rules
- Section order is fixed.
- Omit fully empty optional sections.
- Use compact key-value lines where possible.
- Render booleans as `true` / `false`.
- Render lists in stable order.
- Omit null fields unless omission would hide important current-turn state.

### Section mapping

#### `[ROLE]`
Source:
- `RoleContext`

Required keys for slice 1:
- `bear`
- `agent_role`

Optional keys:
- `memory_scope`

#### `[OPERATIONAL SUMMARY]`
Source:
- `OperationalState`

Required keys for slice 1:
- `permission_mode`
- `tool_classes`
- `can_read_workspace`
- `can_edit_workspace`
- `execution_unlocked`

Strongly preferred keys:
- `workspace_roots`
- `plan_state`
- `plan_approval_status`
- `activity_status`

#### `[PERMISSIONS AND TOOLS]`
Source:
- `CapabilityPolicy`

Preferred keys:
- `allowed_tool_families`
- `allowed_tool_ids`
- `identity_trust_rules`
- `disallowed_actions`

#### `[WORKFLOW RULES]`
Source:
- `WorkflowPolicy`

Required content for slice 1:
- stop conditions

Strongly preferred content:
- active activity/progress representation rules
- memory-vs-plan rules

---

## Deduplication rules

The compiler should deduplicate standard rule categories before rendering.

### Categories that should deduplicate
- current-turn authority
- stop conditions
- identity trust rules
- editing/mutation disallowance
- plan representation rules
- memory-vs-plan rules

### Strategy
1. normalize candidate rules by category;
2. prefer the structured source-of-truth field over legacy prose wording;
3. render only one normalized statement per category in the final structured sections;
4. allow additional explanatory prose only if it adds information that the source-of-truth field does not contain.

### Anti-goal
Do not keep duplicate reminders just because they are historically familiar.

---

## Validation rules

The implementation should enforce the following invariants.

### Required invariants for slice 1
1. `[OPERATIONAL SUMMARY]` must appear at most once.
2. stop conditions must be represented at most once canonically.
3. activity/workplan/memory/execution must not be collapsed into one unlabeled status line if the data is available separately.
4. the renderer must not infer `can_edit_workspace` or `can_read_workspace` from prose; these must come from structured state.

### Strongly preferred invariants
5. active-plan-style content must not be emitted as a recommended durable memory candidate.
6. hidden runtime annotations must not be treated as visible user-authored transcript content in downstream display surfaces.
7. continuation after tool results should be implemented as a compiler/runtime invariant unless a real need for variability emerges.
8. any operational-summary projection of plan-representation behavior should be derived from `workflow_policy.plan_representation_rules`, not stored as a separate source-of-truth field.

---

## Minimal slice-1 render example: `pair`

### Example input
```text
RoleEnvironmentPromptContext
- schema_version: "1"
- role:
  - bear_name: "Builder Bear"
  - agent_role: "pair"
  - memory_scope: "pair_local"
- operational_state:
  - permission_mode: "Write"
  - tool_classes: ["WorkspaceRead", "WorkspaceMutation", "Execution", "Browser"]
  - execution_unlocked: true
  - workspace_roots: ["/workspace"]
  - plan_state: "Approved"
  - plan_approval_status: "ApprovedExecutionUnlocked"
  - activity_status: "Inactive"
  - can_read_workspace: true
  - can_edit_workspace: true
- capability_policy:
  - allowed_tool_families: ["WorkspaceRead", "WorkspaceMutation", "Execution", "Browser", "Identity", "Memory", "Planning", "WebFetch"]
  - allowed_tool_ids: ["FsReadTextFile", "FsEditFile", "ProcessRun", "ChromeOpen", "SessionInfo", "MemoryRead", "MemoryWriteEntry", "UpdatePlan", "WebFetch"]
  - identity_trust_rules: ["SessionInfoIsTrustedIdentitySource"]
- workflow_policy:
  - stop_conditions: ["UserJudgment", "RequiredApproval", "MissingInformation", "UnrecoverableError"]
  - plan_representation_rules: ["UseUpdatePlanForLiveActivityProgressTracking", "FormalWorkplanArtifactsRequireExplicitNeed"]
  - memory_vs_plan_rules: ["DoNotUseDurableMemoryForActivePlansOrEphemeralProgress"]
- context_inventory:
  - instructions_present: true
  - memory_available: true
  - workspace_access: true
  - work_surface_known: true
  - work_surface_kind: "repo"
  - work_surface_label: "/workspace"
  - surface_guidance_sources: ["AGENTS.md", "README.md"]
  - surface_memory_sources: ["pair surface notes"]
  - work_surface_resolution_available: true
```

### Example render
```text
[ROLE]
bear=Builder Bear
agent_role=pair
memory_scope=pair_local

[OPERATIONAL SUMMARY]
permission_mode=Write
tool_classes=WorkspaceRead,WorkspaceMutation,Execution,Browser
can_read_workspace=true
can_edit_workspace=true
execution_unlocked=true
workspace_roots=/workspace
plan_state=Approved
plan_approval_status=ApprovedExecutionUnlocked
activity_status=Inactive

[PERMISSIONS AND TOOLS]
allowed_tool_families=WorkspaceRead,WorkspaceMutation,Execution,Browser,Identity,Memory,Planning,WebFetch
allowed_tool_ids=FsReadTextFile,FsEditFile,ProcessRun,ChromeOpen,SessionInfo,MemoryRead,MemoryWriteEntry,UpdatePlan,WebFetch
identity_trust_rules=SessionInfoIsTrustedIdentitySource

[WORKFLOW RULES]
stop_conditions=UserJudgment,RequiredApproval,MissingInformation,UnrecoverableError
plan_representation_rules=UseUpdatePlanForLiveActivityProgressTracking,FormalWorkplanArtifactsRequireExplicitNeed
memory_vs_plan_rules=DoNotUseDurableMemoryForActivePlansOrEphemeralProgress

[CONTEXT INVENTORY]
instructions_present=true
memory_available=true
workspace_access=true
work_surface_known=true
work_surface_kind=repo
work_surface_label=/workspace
surface_guidance_sources=AGENTS.md,README.md
surface_memory_sources=pair surface notes
```

---

## Minimal non-`pair` example: `watch`

This example exists to prove the outer shape is role-general.

### Example input
```text
RoleEnvironmentPromptContext
- schema_version: "1"
- role:
  - bear_name: "Builder Bear"
  - agent_role: "watch"
- operational_state:
  - permission_mode: "Observe"
  - tool_classes: ["WorkspaceRead"]
  - execution_unlocked: false
  - activity_status: "Inactive"
  - can_read_workspace: true
  - can_edit_workspace: false
- capability_policy:
  - allowed_tool_families: ["WorkspaceRead", "Memory"]
  - allowed_tool_ids: ["FsReadTextFile", "MemoryRead"]
  - disallowed_actions: ["WorkspaceMutation", "AutonomousOutboundExecution"]
- workflow_policy:
  - stop_conditions: ["MissingInformation", "UnrecoverableError"]
- context_inventory:
  - work_surface_known: false
  - work_surface_resolution_available: true
  - work_surface_resolution_hint: "prefer service/deployment/monitor target resolution before broad search when the observation task appears surface-bound"
```

### Example render
```text
[ROLE]
bear=Builder Bear
agent_role=watch

[OPERATIONAL SUMMARY]
permission_mode=Observe
tool_classes=WorkspaceRead
can_read_workspace=true
can_edit_workspace=false
execution_unlocked=false
activity_status=Inactive

[PERMISSIONS AND TOOLS]
allowed_tool_families=WorkspaceRead,Memory
allowed_tool_ids=FsReadTextFile,MemoryRead
disallowed_actions=WorkspaceMutation,AutonomousOutboundExecution

[WORKFLOW RULES]
stop_conditions=MissingInformation,UnrecoverableError

[CONTEXT INVENTORY]
work_surface_known=false
work_surface_resolution_available=true
work_surface_resolution_hint=prefer service/deployment/monitor target resolution before broad search when the observation task appears surface-bound
```

---

## Why this schema is intentionally small

This document is trying to avoid architectural theater.

It does **not** define:
- a full prompt-authoring system;
- a universal policy language;
- a final serialization format for all services;
- a requirement that every optional object be populated immediately.

Instead it defines only what implementation needs right now:
- a stable root object;
- a few required sub-objects;
- clear field provenance;
- a compact render contract;
- validation rules that prevent the specific failure modes already seen in practice.

That is the minimum useful contract for moving from prompt prose accumulation to a real prompt compiler.
