# Pair Tool Discovery and Scope Policy

## Status

Initial implementation slice complete. `session_info` is the canonical orientation descriptor, its output includes policy/activity state for ACP pair turns, and memory/workplan/ACP local tool descriptors now include scope and orientation guidance. User testing confirms the agent uses tools naturally without prompt suffix injection.

## Purpose

`pair` agents need enough awareness to use tools and memory well without reintroducing Den-generated runtime text into persisted Letta user messages.

This policy complements:

- `docs/planning/PAIR_LETTA_MESSAGE_BOUNDARY_PLAN.md`
- `docs/planning/PAIR_ENVIRONMENT_PROMPT_CONSTRUCTION_SPEC.md`
- `docs/planning/archives/CONTEXT_COMPOSITION_PLAN.md`
- `docs/concepts/MEMORY_MODEL.md`

The central problem is that Bear conversations contain layered context: platform invariants, Bear identity, role/Workplace contract, work-surface grounding, thread state, turn-local runtime state, and execution/tool state. Tool discovery must respect those layers.

## Guiding principles

1. **Do not use user messages as a context transport**
   - Human-authored text is the only content that belongs in persisted `role=user` messages.

2. **Advertise orientation, not prose dumps**
   - Agents should reliably know how to orient themselves without receiving all runtime facts as appended instructions.

3. **Prefer structured affordances**
   - Tool descriptors, tool availability, runtime-state tools, and tool returns are preferred over per-turn prompt prose.

4. **Scope before recall**
   - When a request depends on local understanding, the agent should identify current Workplace and work surface before broad memory search.

5. **Discovery state should be visible**
   - Tool availability, MCP server summaries, mode/policy state, and work-surface confidence should be inspectable through `session_info`, `/status`, or equivalent ACP status UX. The user should not need to infer scope/tool state from failures.

6. **Minimal workplace persona**
   - Stable role prompts should define mission and boundaries, not rich personality. Workplace agents should remain capability/policy oriented.

7. **Discovery should be purposeful**
   - Self-discovery is good when scope or capability is ambiguous. It should not be mandatory for simple direct user requests.

## Context layers and placements

| Layer | Examples | Preferred placement | Not allowed |
|---|---|---|---|
| Den baseline | safety, privacy, attribution, memory governance | Den-managed baseline/system prompt | user message |
| Bear identity | charter, durable purpose, broad boundaries | Bear managed context / system blocks | per-turn user suffix |
| Role / Workplace contract | pair Collaboration Space, work Execution Space | role managed prompt block | user message |
| Work surface | repo, service, deployment, Mission, project | memory anchors, `session_info`, work-surface tools | unscoped memory assumptions |
| Thread scope | current conversation intent and recent decisions | conversation history and plans | runtime scaffolding as user text |
| Turn runtime state | mode, policy, workspace roots, channel, available tools | tool descriptors, `session_info`, UI events, Den enforcement | user message |
| Execution state | tool calls, tool results, pending approvals | tool events/results/ledger | durable memory by default |

## Scope model

Use the scope hierarchy from `MEMORY_MODEL.md`:

```text
Bear
  └── Workplace / role space
        └── Work surface
              └── Thread / conversation
                    └── Turn
```

A tool, memory entry, artifact, plan, or observation should carry enough provenance to distinguish:

- Bear-global knowledge,
- role/Workplace-local knowledge,
- work-surface-local knowledge,
- thread-specific state,
- turn-local execution detail.

Work-surface identification is a resolution process. The Bear should know whether the current work surface is `unresolved`, `candidate`, `ambiguous`, `resolved`, `confirmed`, or `rejected`, and it should be able to communicate that status to the user. When ambiguity affects memory or action, the Bear may ask the user to verify an assumption or choose among candidates. User confirmation can raise confidence for the current thread and should become provenance for later memory writes.

## Tool advertisement levels

### Level 0: Hidden Den/runtime mechanisms

Not model-facing. Examples:

- internal ledgers,
- active-turn locks,
- ACP compatibility metadata,
- recovery counters,
- raw authorization checks.

These are enforced by Den and surfaced only as diagnostics when needed.

### Level 1: Always-advertised orientation tools

Advertise when tools are enabled. These are the agent's safe entry points for self-location.

Expected examples:

- `session_info`
- current activity/workplan status tool
- scoped memory browse/search/read tools
- read-only workspace discovery tools when a workspace exists

Descriptor obligations:

- State that these tools answer “where am I / what scope am I in / what is authorized?”
- Describe Bear, Workplace, work surface, thread, and turn distinctions.
- Indicate whether the tool is read-only.

### Level 2: Contextual capability tools

Advertise only when relevant and safe in the current role/channel.

Examples:

- filesystem read/search tools when workspace roots exist,
- web fetch/search when configured,
- memory write tools when memory writes are policy-appropriate,
- planning/workplan tools when plan/activity state is active or useful.

Descriptor obligations:

- State operating scope.
- State mutation/read-only status.
- Point to orientation tools when scope is ambiguous.

### Level 3: Mutating or approval-sensitive tools

Advertise only when the current policy permits the model to request them.

Examples:

- file edit/write tools,
- command execution,
- plan approval recording,
- handoff/task creation.

Descriptor obligations:

- Explain what permission or human approval means.
- Explain that Den enforces policy even if the model asks.
- Provide expected failure semantics.

### Level 4: Rare recovery/admin tools

Advertise sparingly or keep human/operator-only.

Examples:

- compaction/recovery,
- runtime health diagnostics,
- admin provisioning,
- direct state repair.

## Discovery expectations

### The agent should self-orient when user intent depends on scope

Examples:

- “what do you know about this project?”
- “how does this work here?”
- “continue where we left off”
- “fix it” without naming the artifact,
- “use the plan” when no plan is visible in the current thread,
- “what did we decide about this architecture?”

Expected behavior:

1. Check current conversation and visible user intent.
2. Use `session_info` for trusted channel/workspace/work-surface hints when needed.
3. Use current work-surface memory anchors before broad Bear memory.
4. Use workspace inspection for artifact-specific questions.
5. Avoid answering from Bear-global memory when the question is local to a work surface.

### The agent may answer directly when scope is self-contained

Examples:

- pasted text transformations,
- simple explanations,
- general knowledge questions,
- explicit file/path questions where the required tool is obvious,
- direct user instructions that include all needed context.

Discovery should not become a ritual.

## Descriptor and tool-call UX standards

Every model-facing tool descriptor should answer:

1. What does this tool do?
2. What scope does it operate in?
3. Is it read-only, mutating, or approval-sensitive?
4. When should the agent call it?
5. What should the agent call first if scope is unclear?
6. What do denial/error results mean?
7. Does it create durable memory, active work state, transient observations, or external effects?

Descriptors should prefer compact, structured wording. Avoid repeating the whole role prompt in every tool.

Every user-visible tool must also provide descriptor-owned display metadata. Do not add scattered adapter/client `match` arms, hardcoded allowlists, or one-off UI labels when a descriptor resolver can own the messaging. The same logical tool should render consistently whether Den executes it directly or the ACP adapter executes it locally.

Tool-call UX principles:

1. Human-readable first, protocol second. Visible titles should say what is happening, not expose raw transport names such as `tool_call`, `tool_call_update`, or `tool_request`.
2. Tool activity should be stateful: requested, awaiting approval, running, succeeded, failed, denied, cancelled, or timed out.
3. Show intent and target, not full raw arguments. Display the operation, safe target, and concise summary.
4. Keep display metadata descriptor-owned. Labels, verbs, categories, approval language, and redaction policy belong with tool descriptors or shared display resolvers.
5. Separate model-facing provider names from human-facing labels. Provider names stay concise and stable; UI labels should be polished and descriptive.
6. Make scope visible. Show workspace-relative paths, memory scope, URL host, git repo, browser page, or plan/task id when useful.
7. Respect sensitivity and size. Redact secret-like or large arguments/results, and bound stdout/stderr, file content, patches, web content, and memory bodies.
8. Approval UX should explain risk concretely, e.g. the command, cwd, file path, URL host, or memory scope being approved.
9. Errors should be actionable and status-specific. Distinguish path-not-found, permission-denied, timeout, unsupported tool, command failure, and result-post failure.
10. Keep execution-location details diagnostic. Den vs adapter-local vs ACP client is useful metadata, but should not dominate the user-visible title.
11. Degrade gracefully when a client only supports simple `tool_call` / `tool_call_update` cards.

For ACP/Den tools, descriptors and Den-to-adapter `tool_request` events should include display metadata equivalent to:

- `label`: concise human label.
- `category`: `filesystem`, `git`, `terminal`, `browser`, `web`, `memory`, `plan`, `skills`, `tasks`, `orientation`, etc.
- `progress_verb`: running-state verb phrase.
- `complete_verb`: success-state verb phrase.
- `target_arg_keys`: safe argument keys that identify the visible target.
- `sensitive_arg_keys`: argument keys to redact in UI summaries.
- `approval_summary`: concrete approval/risk copy.
- event-level `display.title`, `display.subtitle`, `display.arguments_summary`, and `display.target` when arguments are known.

Adapter implementors should prefer `event.display.title` for ACP `ToolCall` titles, use `event.display.subtitle` and `event.display.approval_summary` for content/permission prompts, preserve `event.display.arguments_summary` as metadata or expandable detail, and fall back to legacy `title`/`tool_name` only when `display` is absent.

## `session_info` expectations

`session_info` should become the canonical orientation tool for `pair` and later other channels.

It should return, at minimum:

- authenticated human identity and membership role,
- Bear id/slug/name,
- role and Workplace,
- channel kind (`acp`, future web pair, etc.),
- session/thread id,
- resolved Letta conversation id when available,
- current workspace roots,
- current permission/mutation policy,
- available tool classes,
- current work-surface resolution state and candidates,
- whether user confirmation is needed,
- current activity/workplan summary,
- memory write policy and scope.

It should not return secrets or raw tokens.

For work-surface resolution, `session_info` and related orientation tools should expose:

- `status`: `unresolved`, `candidate`, `ambiguous`, `resolved`, `confirmed`, or `rejected`,
- `confidence`: `none`, `low`, `medium`, `high`, or `confirmed`,
- `reference_candidates`,
- `needs_user_confirmation`,
- guidance for when to state assumptions, inspect further, or ask the user to choose.

## Memory and artifact scope policy

When reading or writing memory:

- Prefer current work-surface canonical anchors for local understanding.
- Use role-local memory for tactical/local findings unless durable cross-role value is clear.
- Do not treat all Bear memory as equally relevant to the current thread.
- Memory writes must identify intended scope: Bear-global, role/Workplace-local, work-surface-local, or thread/task-local.
- Active plans, run results, transient command output, and operational observations are not semantic memory by default.

## Relationship to agentic skills

Future skills should plug into the same discovery model.

A skill should declare:

- scope (`bear`, `workplace`, `work_surface`, `thread`, `turn`),
- role/channel applicability,
- required tools,
- mutating/external effects,
- memory/artifact side effects,
- discovery prerequisites.

Agents should discover skills through structured skill descriptors, not prompt suffixes.

## Immediate implementation slices

1. [x] Audit current ACP/Den tool descriptors against descriptor standards.
2. [x] Ensure `session_info` descriptor is always advertised when tools are enabled and clearly marked as the orientation tool.
3. [x] Update `session_info` output to include current Workplace/work-surface hints and policy state.
4. [x] Improve filesystem/memory/workplan descriptors with scope and discovery guidance.
5. [x] Add tests that descriptor metadata, `session_info`, and memory tool boundaries agree.
6. [x] Avoid putting any of this guidance back into `messages[].content`.

## Follow-up slices

1. [x] Generalize descriptor guidance into a shared taxonomy/helper for Den tools, ACP local tools, future channels, and agentic skills.
2. Introduce a shared `PairTurnRequest` boundary so future `pair` channels cannot append runtime context to Letta user messages.
3. Improve work-surface resolution beyond candidate hints using repo metadata, memory anchors, explicit user references, and user confirmation state.
4. Stabilize smoke-stack regression coverage for the clean user-message boundary.
