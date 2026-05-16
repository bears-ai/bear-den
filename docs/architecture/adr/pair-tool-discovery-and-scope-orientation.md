# ADR: Pair Tool Discovery and Scope Orientation

## Status

Accepted for near-term implementation.

## Context

BEARS `pair` conversations contain many layers of context:

- Den baseline behavior and safety invariants,
- Bear identity and charter,
- role/Workplace contract,
- work-surface grounding,
- thread/conversation state,
- turn-local runtime state,
- execution/tool state.

Historically, ACP pair turns appended Den-generated runtime/tool/workflow guidance to the human user message before sending it to Letta. This made the agent aware of tools and runtime state, but it polluted Letta user-message history with scaffolding such as `<system-reminder>` blocks and workflow/tool instructions.

The Letta message-boundary refactor establishes that persisted Letta `role=user` content must contain only human-authored input. This removes an accidental tool-discovery mechanism. We therefore need an explicit, structured policy for tool discovery and scope orientation.

The memory model also requires scope awareness. A Bear may have multiple Workplaces and multiple work surfaces. A `pair` agent acting in one repository or project should not blindly answer from all Bear memory when the user asks local questions such as “what do you know about this project?”

## Decision

We will separate tool discovery and scope orientation from user-message content.

1. **Tool and runtime awareness will be provided through structured affordances**
   - `client_tools` / future `client_skills`, descriptors, tool availability, tool returns, and runtime-state tools.

2. **`session_info` is the canonical orientation tool**
   - It should be advertised whenever tools are enabled for `pair`.
   - It should answer where the agent is operating: Bear, role/Workplace, channel, session/thread, workspace roots, current policy, work-surface hints, and activity/workplan state.

3. **Tool advertisement will be level-based**
   - Hidden Den mechanisms are not model-facing.
   - Orientation tools are broadly available.
   - Contextual tools are advertised when relevant and safe.
   - Mutating/approval-sensitive tools are advertised only when policy permits the model to request them.
   - Recovery/admin tools remain rare or human/operator-facing.

4. **Agents are expected to self-orient when scope matters**
   - For ambiguous local questions, current-project questions, “continue” requests, and memory/plan questions, the agent should use `session_info`, scoped memory tools, and workspace inspection before assuming scope.
   - Work-surface resolution state should be visible to the agent. The agent may state assumptions, ask the user to verify a candidate, or ask the user to choose between plausible work surfaces when ambiguity affects memory or action.
   - For self-contained/simple user requests, discovery is not mandatory.

5. **Tool descriptors must carry scope and side-effect semantics**
   - Descriptors should state what scope the tool operates in, whether it is read-only/mutating/approval-sensitive, when to call it, and what to do if scope is unclear.

6. **Workplace agents must remain minimally personified**
   - We will not compensate for removed prompt suffixes by adding broad persona prose.
   - Stable role prompts should define mission, boundaries, and discovery expectations, not rich personality.

7. **Memory and artifact provenance must remain visible**
   - Memory/artifact/tool outputs should carry enough provenance to distinguish Bear-global, Workplace-local, work-surface-local, thread-local, and turn-local information.
   - User-confirmed work-surface resolution should be preserved as provenance when later memory, plans, or artifacts are associated with that work surface.

## Consequences

### Positive

- New user messages remain clean and human-authored.
- Tool awareness can evolve without polluting conversation history.
- Scope orientation becomes explicit and reusable across ACP, future web `pair`, and other channels.
- Memory retrieval can become work-surface-first rather than Bear-global by default.
- Agentic skills can later plug into the same discovery model.

### Negative / tradeoffs

- Agents may initially be less tool-aware until descriptors and `session_info` improve.
- Some behavior previously enforced by prompt suffixes must move to descriptors, tools, or stable managed prompts.
- More care is needed to design concise descriptors without recreating prompt bloat.

## Alternatives considered

### Continue injecting runtime context into user messages

Rejected. This caused persistent history pollution and compounded stale context problems.

### Use `override_system` for per-turn runtime context

Rejected as a default approach. Letta documents `override_system` as replacement semantics for the compiled/persisted system prompt. It may be useful in exceptional cases, but only when Den constructs the complete replacement system prompt.

### Add rich pair persona/system prose

Rejected. Workplace agents should remain minimally personified and capability/policy oriented.

### Pure self-discovery with no advertised orientation tools

Rejected. Agents need a reliable, discoverable entry point for current scope, role, policy, and workspace state.

## Implementation plan

See `docs/planning/PAIR_TOOL_DISCOVERY_AND_SCOPE_POLICY.md`.

Near-term slices:

1. Audit ACP/Den tool descriptors against the descriptor standards.
2. Ensure `session_info` is always advertised when tools are enabled and is clearly described as the orientation tool.
3. Expand `session_info` output with Workplace/work-surface resolution state, candidate confidence, user-confirmation needs, and runtime policy.
4. Improve filesystem, memory, and workplan descriptors with scope and discovery guidance.
5. Add consistency tests across descriptors, `session_info`, and memory boundary validation.
6. Keep user-message content clean; do not reintroduce prompt suffix injection.

## Related documents

- `docs/planning/PAIR_TOOL_DISCOVERY_AND_SCOPE_POLICY.md`
- `docs/planning/PAIR_LETTA_MESSAGE_BOUNDARY_PLAN.md`
- `docs/planning/PAIR_ENVIRONMENT_PROMPT_CONSTRUCTION_SPEC.md`
- `docs/planning/archives/CONTEXT_COMPOSITION_PLAN.md`
- `docs/concepts/MEMORY_MODEL.md`
- `docs/architecture/adr/workflow-state-ontology.md`
- `docs/architecture/adr/bear-workplaces.md`
- `docs/architecture/adr/semantic-bear-memory.md`
