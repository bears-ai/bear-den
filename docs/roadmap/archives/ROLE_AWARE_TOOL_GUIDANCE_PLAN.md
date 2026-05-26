# Role-Aware Tool Guidance Plan

For the canonical role model and current role names, see [bear roles](../../architecture/bear-roles.md).
This plan centralizes BEARS agent tool guidance so prompts, tool descriptors, tool results, ACP projection, and operator UI all use one Den-owned source of truth.

## Problem

Tool guidance is currently scattered across several layers:

- ACP `pair` prompt reminders in `services/den/src/api/acp.rs`.
- Built-in Den tool descriptors in `services/den/src/core/den_tools.rs`.
- Situational tool-result guidance in individual Den tool handlers.
- Session information guidance in `den.session.info` / provider `session_info`.
- ACP adapter behavior and labels in `tools/bears-acp-adapter/src/main.rs`.
- Operator-facing Bear detail UI shows role health and tool counts, but not a clear role-specific view of available tools and guidance.

This creates drift risk. For example, a tool name or behavior can change in Den descriptors while prompt guidance or UI copy remains stale.

## Goals

1. Put model-facing and operator-facing tool guidance in one Den-owned module.
2. Make guidance explicitly role-aware.
3. Introduce a general `ToolArea` taxonomy for tools, derived primarily from canonical tool names.
4. Support a small generic lifecycle trigger taxonomy plus tool-area/context metadata. Planning, Docket, Cabinet, workspace lifecycle, memory, skills, observations, and web tools should reuse the same generic triggers rather than adding domain-specific trigger variants.
5. Expose guidance in Bear detail UI so users/operators can see what each role is expected to do with available tools.
6. Preserve clean provider-visible tool names such as `update_plan`, `enter_plan_mode`, `session_info`, and `memory_write_entry`.
7. Design so future per-bear customization can be layered in without changing the core shape.

## Non-goals for the first implementation

- Per-bear custom guidance editing UI.
- Per-user guidance overrides.
- LLM-generated guidance.
- Moving every prompt string in the system at once.
- Replacing durable policy enforcement; guidance explains behavior, policy still enforces it.

## Proposed architecture

Add two related Den concepts:

- `services/den/src/core/tool_taxonomy.rs` — canonical tool naming and `ToolArea` classification.
- `services/den/src/core/tool_guidance.rs` — role-aware guidance records layered on top of tool descriptors and areas.

The taxonomy layer owns the broad functional area for each tool. The guidance layer owns structured guidance records keyed by role, canonical tool name or tool area, generic trigger, and optional context fields.

### Tool taxonomy

A **Tool Area** is a coarse functional category for agent-facing tools. It describes what part of BEARS or the user environment the tool operates on, independent of the specific role using it.

Tool areas should be mapped from canonical Den tool names, not provider-visible aliases. Provider names are optimized for LLM/tool UX; canonical names carry system structure.

Canonical naming convention:

```/dev/null/canonical-tool-names.txt#L1-8
den.<namespace>.<verb>
den.<namespace>.<object>.<verb>
```

The `<namespace>` maps to `ToolArea`. Multiple namespaces may map to one area. For example, `den.work_plan.*` and `den.plan_mode.*` both map to `Planning`.

Suggested taxonomy:

```/dev/null/tool_taxonomy.rs#L1-45
pub enum ToolArea {
    General,
    Session,
    Identity,
    Policy,
    Capabilities,
    Channel,
    Web,
    Memory,
    Planning,
    Tasks,
    Docket,
    Cabinet,
    Workspace,
    Filesystem,
    Terminal,
    Git,
    Browser,
    Skills,
    Observations,
    Runs,
    Core,
}

pub fn tool_area_for_canonical_name(name: &str) -> ToolArea {
    match canonical_namespace(name).as_deref() {
        Some("session") => ToolArea::Session,
        Some("policy") => ToolArea::Policy,
        Some("web") => ToolArea::Web,
        Some("memory") => ToolArea::Memory,
        Some("work_plan" | "plan_mode") => ToolArea::Planning,
        Some("task") => ToolArea::Tasks,
        Some("docket") => ToolArea::Docket,
        Some("cabinet") => ToolArea::Cabinet,
        Some("workspace") => ToolArea::Workspace,
        Some("skill") => ToolArea::Skills,
        Some("observation") => ToolArea::Observations,
        Some("run") => ToolArea::Runs,
        Some("core") => ToolArea::Core,
        Some("user" | "bear") => ToolArea::Identity,
        Some("capabilities") => ToolArea::Capabilities,
        Some("channel") => ToolArea::Channel,
        _ => ToolArea::General,
    }
}
```

`DenToolDescriptor` should expose `area: ToolArea`, derived by default from the canonical name. If a tool does not map cleanly, it should either be renamed or explicitly classified, with renaming preferred when practical.

### Guidance core types

Suggested shape:

```/dev/null/tool_guidance.rs#L1-70
pub enum ToolGuidanceTrigger {
    ToolAvailable,
    PromptContext,
    BeforeCall,
    AfterSuccess,
    AfterFailure,
    StateChanged,
    NeedsUserApproval,
    HandoffCreated,
    Completion,
    Blocked,
}

pub enum ToolGuidanceAudience {
    Agent,
    Operator,
    Both,
}

pub struct ToolGuidance {
    pub role: BearAgentRole,
    pub canonical_tool_name: &'static str,
    pub provider_name: &'static str,
    pub trigger: ToolGuidanceTrigger,
    pub area: Option<ToolArea>,
    pub subject_kind: &'static str,
    pub action: Option<&'static str>,
    pub outcome: Option<&'static str>,
    pub audience: ToolGuidanceAudience,
    pub summary: &'static str,
    pub guidance: &'static str,
    pub recommended_followups: &'static [&'static str],
    pub anti_patterns: &'static [&'static str],
}
```

The first implementation can use static built-in guidance only. Later, the same query functions can layer database rows on top.

### Query API

The module should expose functions like:

```/dev/null/tool_guidance.rs#L72-120
pub fn guidance_for_role(role: BearAgentRole) -> Vec<ToolGuidance>;

pub fn guidance_for_tool(
    role: BearAgentRole,
    canonical_tool_name: &str,
) -> Vec<ToolGuidance>;

pub struct GuidanceQuery<'a> {
    pub bear_id: Option<Uuid>,
    pub role: BearAgentRole,
    pub canonical_tool_name: &'a str,
    pub trigger: ToolGuidanceTrigger,
    pub area: Option<ToolArea>,
    pub subject_kind: Option<&'a str>,
    pub action: Option<&'a str>,
    pub outcome: Option<&'a str>,
}

pub fn guidance_for_event(query: GuidanceQuery<'_>) -> Vec<ToolGuidance>;

pub fn prompt_guidance_for_role(role: BearAgentRole) -> String;

pub fn operator_tool_guidance_for_role(role: BearAgentRole) -> Vec<OperatorToolGuidanceView>;
```

`prompt_guidance_for_role` should return compact model-facing instructions. UI functions should return richer structured rows.

## Canonical naming and ToolArea mapping

Tool area should normally be derivable from the canonical name. If a canonical namespace does not map cleanly, prefer renaming the canonical tool namespace before adding special cases.

Current/refined mapping examples:

| Canonical tool | Namespace | ToolArea |
|---|---:|---|
| `den.session.info` | `session` | `Session` |
| `den.web.fetch` | `web` | `Web` |
| `den.memory.write_entry` | `memory` | `Memory` |
| `den.work_plan.update` | `work_plan` | `Planning` |
| `den.plan_mode.enter` | `plan_mode` | `Planning` |
| `den.task.write_intent` | `task` | `Tasks` now, possible Docket migration later |
| `den.skill.propose` | `skill` | `Skills` |
| `den.observation.write` | `observation` | `Observations` |
| `den.run.write_result` | `run` | `Runs` |
| `den.core.write_result_summary` | `core` | `Core` |
| `den.user.get_current` | `user` | `Identity` |
| `den.bear.get_self` | `bear` | `Identity` |
| `den.capabilities.list_self` | `capabilities` | `Capabilities` |
| `den.channel.get_context` | `channel` | `Channel` or `Session` |

Future canonical naming should keep namespace-to-area mapping obvious:

```/dev/null/future-tool-names.txt#L1-18
den.docket.intent.write
den.docket.task.list
den.docket.run.status

den.cabinet.search
den.cabinet.document.read
den.cabinet.document.propose_update

den.workspace.open
den.workspace.status
den.workspace.cleanup
```

Provider-visible names may remain ergonomic and do not need to encode area.

## Role-specific guidance model

Guidance must be role-aware from the start.

### Pair

`pair` guidance should cover:

- ACP local tools: discover/read/edit/verify workflow.
- Server tools:
  - `session_info`,
  - memory tools,
  - planning/workboard tools,
  - plan-mode tools,
  - web fetch/search.
- Plan completion guidance:
  - when `update_plan` completes a non-trivial plan, consider one concise role-local memory entry only if durable knowledge was produced.
  - do not write memory for routine/trivial task completion.
- Plan mode guidance:
  - `enter_plan_mode` when substantial implementation planning would help,
  - `exit_plan_mode` submits or updates a markdown plan artifact,
  - `record_plan_approval` records explicit authenticated-human approval when useful,
  - ask/plan modes expose read/search/inspect tools; write mode enables mutation/execution/browser tools, still subject to concrete ACP client approval and Den/adapter policy.

### Work

`work` guidance should cover:

- Update workboard status while executing approved work.
- Write task/run results through run-result tooling, not ordinary memory, for task outputs.
- Do not execute channel-originated plans directly; only approved tasks.

### Chat

`chat` guidance should cover:

- Use workboard only for user-visible multi-step conversational work.
- Use handoff/task-intent pathways for autonomous/background work.
- Do not pretend to run background work directly.

### Review

`review` guidance should cover:

- Review and promote, not raw-produce.
- Treat role-local plans as candidate evidence, not shared truth.
- Promote durable summaries/decisions to `core/` only after review.

### Watch

`watch` guidance should remain minimal until a concrete observation-status use case exists.

## Situational guidance design

Tool handlers should not hand-code guidance strings directly. Instead, they should ask the guidance layer with a generic trigger plus domain/context metadata.

Example: `update_plan` completion result for `pair`:

```/dev/null/update_plan_result.json#L1-18
{
  "plan": { "status": "completed" },
  "guidance": [
    {
      "trigger": "completion",
      "area": "planning",
      "subject_kind": "work_plan",
      "action": "update",
      "outcome": "completed",
      "summary": "Consider durable memory only for non-trivial completed plans.",
      "message": "If this completed plan produced durable knowledge useful to future pair sessions, write one concise pair-local memory entry with memory_write_entry. Skip routine or trivial completion. Use lifecycle.scope=core-candidate only when it may matter across roles.",
      "recommended_followups": ["memory_write_entry"],
      "anti_patterns": ["Do not write raw logs", "Do not duplicate the full plan"]
    }
  ]
}
```

This guidance should appear only when triggered by a relevant status transition.

## UI exposure in Bear detail

Add a Bear detail section such as **Role tools and guidance**.

For each role, show:

- provider-visible tool name,
- canonical Den tool name,
- label,
- scope/kind,
- availability/role policy,
- short guidance summary,
- optional trigger-specific guidance details.

Initial implementation can be read-only and static. It should use the same guidance module as prompts/tool results.

Recommended UI grouping:

1. Role tabs/sections: `chat`, `pair`, `review`, `work`, `watch`.
2. Tool rows sorted by provider name.
3. Expandable guidance details.
4. Highlight tools available to the selected role.
5. Show role-specific guidance even when the tool is hidden from other roles.

## Future per-bear customization

Do not implement this now, but design for it.

Possible future table:

```/dev/null/schema.sql#L1-24
CREATE TABLE bear_tool_guidance_overrides (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    bear_id UUID NOT NULL REFERENCES bears(id) ON DELETE CASCADE,
    role TEXT NOT NULL,
    canonical_tool_name TEXT NOT NULL,
    trigger TEXT NOT NULL,
    area TEXT NOT NULL DEFAULT 'general',
    subject_kind TEXT NOT NULL DEFAULT '',
    action TEXT NULL,
    outcome TEXT NULL,
    audience TEXT NOT NULL DEFAULT 'both',
    guidance TEXT NOT NULL,
    enabled BOOLEAN NOT NULL DEFAULT TRUE,
    created_by_user_id INTEGER NULL REFERENCES users(id) ON DELETE SET NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
```

Overlay order later:

1. Built-in global guidance.
2. Optional deployment/site guidance.
3. Bear-specific overrides, either for a specific canonical tool or an entire `ToolArea`.
4. Maybe session-specific hints.

The first implementation should keep function signatures ready for `bear_id: Option<Uuid>` even if it ignores it initially.

## Implementation phases

### Phase 1 — Tool taxonomy and descriptor area

Acceptance:

- Add `core/tool_taxonomy.rs` with `ToolArea` and namespace mapping.
- Add `area: ToolArea` to `DenToolDescriptor`.
- Derive area from canonical tool names by default.
- Add tests showing every built-in Den descriptor has the expected `ToolArea`.
- Rename any canonical tool namespaces that do not map cleanly, if practical.

### Phase 2 — Static guidance module

Acceptance:

- Add `core/tool_guidance.rs`.
- Define generic triggers, audiences, and view structs.
- Guidance records reference canonical tools and/or `ToolArea`; they do not define their own domain taxonomy.
- Add static built-in records for `pair` planning/memory/session/web tools.
- Add tests for provider names, role filtering, area filtering, and prompt guidance snippets.

### Phase 3 — Prompt guidance migration

Acceptance:

- Replace the large server-tool guidance string in ACP `pair` prompt construction with `tool_guidance::prompt_guidance_for_role(BearAgentRole::Pair)`.
- Keep ACP local workspace-tool guidance nearby or add a separate local-tool guidance section in the same module.
- No duplicated planning/memory/server tool guidance remains in `acp.rs` except composition calls.

### Phase 4 — Tool result guidance

Acceptance:

- `update_plan` uses the guidance layer with generic triggers for completion, blocked, and handoff-created result guidance, with `area=Planning` and `subject_kind=work_plan`.
- `enter_plan_mode`, `exit_plan_mode`, and `cancel_plan_mode` use guidance records with generic state/approval triggers and `area=Planning`, `subject_kind=plan_mode`.
- Tool result guidance is structured JSON, not free-floating strings.

### Phase 5 — Bear detail UI

Acceptance:

- Bear detail page shows role-aware tool guidance per role.
- UI groups tools by `ToolArea`.
- UI uses the same `tool_guidance` module, not hand-coded template copy.
- Operators can see clean provider-visible names such as `update_plan`, `session_info`, and `memory_write_entry`.

### Phase 6 — Codepool / Letta Code path alignment

Acceptance:

- Codepool-facing Den tool descriptors include or can query guidance summaries.
- `chat` and `work` prompts/context use the same guidance source where Den controls their tool profile.
- Any Letta Code-native planning guidance remains conceptually aligned with Den guidance.

### Phase 7 — Future customization design hook

Acceptance:

- Add no customization UI yet.
- Query functions accept enough context to later overlay bear-specific guidance.
- Document planned overlay model.

## Testing plan

- Unit tests for `ToolArea` mapping from canonical tool names.
- Unit tests for guidance lookup by role/tool/area/trigger/subject/action/outcome.
- Unit tests ensuring every built-in guidance row references a known Den tool or a known `ToolArea`, and a valid role.
- ACP prompt test confirming provider-visible tool names are current and no stale `den_work_plan_*` or `den_plan_mode_*` strings appear.
- Bear detail rendering test confirming guidance rows are present.
- Tool result tests confirming completion guidance appears only on relevant status transitions.

## Risks and mitigations

| Risk | Mitigation |
|------|------------|
| Guidance becomes too verbose in prompts | Keep prompt guidance compact; expose detailed guidance in UI and tool result only when triggered. |
| Guidance drifts from policy | Keep canonical tool names in guidance records and validate against Den descriptors in tests. |
| Domain-specific trigger lists proliferate | Keep triggers generic and put product-specific meaning in `ToolArea`, `subject_kind`, `action`, and `outcome`. |
| ToolArea drifts from canonical naming | Derive area from canonical namespaces and test every built-in descriptor. Prefer renaming awkward namespaces over one-off special cases. |
| Role-specific copy proliferates | Centralize all role/tool guidance records and add tests for duplicate/conflicting records. |
| Future per-bear customization complicates lookup | Start with a layered lookup interface even if only built-ins are returned today. |

## First concrete slice

Start with `pair` and planning:

1. Add `tool_guidance.rs` with static records for:
   - `session_info`,
   - `memory_write_entry`,
   - `update_plan`,
   - `get_plan_status`,
   - `list_plans`,
   - `request_work_handoff`,
   - `enter_plan_mode`,
   - `exit_plan_mode`,
   - `cancel_plan_mode`,
   - `web_fetch`,
   - `web_search`.
2. Replace ACP pair server-tool prompt guidance with the centralized prompt guidance string.
3. Add `update_plan` completion guidance through the centralized result-guidance API.
4. Add Bear detail UI table for role tool guidance.
