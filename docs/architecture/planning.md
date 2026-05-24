# Planning in Bear Den

Planning in Bear Den means a user-visible mini-project plan for an active body of work. Plans are Bear-level records with role provenance, and they usually attach to a **work surface**: the durable work context the Bear is acting on, such as a repo, local checkout, service, deployment, Cabinet Mission, Docket project, or long-running responsibility.

Planning is distinct from continuation, which keeps an agent going in an existing task loop, and from a Cabinet Mission, which is a larger shared knowledge/work container. Bear Den aligns with Letta Code's two planning layers while adapting them to Den's multi-role Bear architecture.

## Two layers

### 1. Live progress tracking

Live progress tracking is the lightweight todo/status list an agent updates while it works.

Letta Code has this as `TodoWrite` for Claude-style toolsets and `UpdatePlan` for Codex-style toolsets. In Letta Code this is mostly UI/state rendering; `UpdatePlan` is a no-op tool implementation whose arguments drive the visible plan, with a simple list of `{ step, status }` and statuses such as `pending`, `in_progress`, and `completed`.

Bear Den's Den equivalent is the **workboard**:

- table: `bear_work_plans`
- audit stream: `bear_work_plan_events`
- Den tools:
  - `den.work_plan.update`
  - `den.work_plan.get_status`
  - `den.work_plan.list`
  - `den.work_plan.request_handoff`

A workboard plan is intentionally small and operational. It records what the Bear is trying to do now, the current item, blockers, the role that created or last updated it, whether the plan should be private to that role, visible to the same user, visible to the Bear, or ready for handoff, and the current work-surface attachment when one is known.

### 2. ACP pair planning mode

Letta Code has `EnterPlanMode` and `ExitPlanMode`. Bear Den keeps the familiar **Ask**, **Plan**, and **Write** mode names for ACP `pair`, but treats them as workflow/UI modes rather than a separate durable mutation gate.

In Bear Den ACP:

1. User or agent requests Plan mode when substantial implementation planning would help.
2. ACP/Den records planning state for the current session.
3. Ask and Plan modes expose read/search/inspect tools.
4. Write mode enables mutation/execution/browser tools; concrete effects still require Den policy, adapter safety checks, and ACP client approval.
5. Pair may write a durable markdown plan artifact under `pair/plans/`.
6. `exit_plan_mode` submits or updates that artifact; it does not create a global mutation gate or require an ACP approval modal.
7. `record_plan_approval` records explicit authenticated-human approval when useful for workflow/audit and switches the ACP UI to Write.

## Planning objects and how they relate

These are related but different objects:

| Object | Owner | Purpose | Durability |
|--------|-------|---------|------------|
| Work surface | Den canonical model and anchors | Durable work context that planning, tasks, artifacts, memory, and activity can attach to | Durable continuity object, not an authorization boundary |
| Workboard plan | Den DB | Current visible todo/progress state | Durable enough for resume/status, but not a project archive |
| Plan artifact | MemFS or approved local plan file | Proposed implementation plan for user approval before mutation or future reference | Durable markdown artifact |
| Task intent | MemFS | Request for reviewed background/autonomous work | Durable input to `curate` review |
| Approved task | MemFS | Curate-approved executable work for `work` | Durable task definition |
| Work result | MemFS/Garage | Result/log/report from `work` | Durable output, curatable into `core/` |

A workboard plan can attach to a **work_surface_id** when Den can resolve or infer one from available anchors. Those anchors may include a local checkout path, workspace root, normalized Git remote, service name, deployment id, Cabinet Mission reference, Docket project reference, or artifact path. A workboard plan can link to a task intent when `den.work_plan.request_handoff` is used. A pre-implementation plan artifact may later be summarized into a workboard plan, task intent, or role-local memory, but it is not itself a Cabinet Mission.

Role is provenance and policy metadata for a plan, not the primary product owner. Cross-role visibility is not cross-role authority: a `pair` plan intended for `work` still needs handoff, review, and Den dispatch before `work` may execute it.

## Work-surface continuity

Planning should follow the same work-surface continuity model used elsewhere in Bear Den.

A common flow is:

1. `pair` encounters a local checkout or workspace root,
2. starts a workboard plan or writes a plan artifact against that current work,
3. Den records the observed anchor and may resolve a provisional work surface,
4. Den canonicalizes the work surface when a stronger durable anchor is available, such as a normalized Git remote,
5. and later `work` re-materializes that same work surface in a different runtime to execute an approved task.

Key principle: plans, handoffs, task intents, approved tasks, runs, and related memory should attach to the durable **work_surface_id**, not only to a machine-local checkout path.

## Pair role behavior

`pair` should use planning like a collaborative coding agent:

- Use `den.work_plan.update` for non-trivial mini-projects, multi-step edits, debugging loops, and user-visible progress.
- Keep at most one item `in_progress`.
- Mark items `completed`, `blocked`, or `cancelled` as reality changes.
- Use `den.work_plan.get_status` to recover the current ACP session's plan after interruption.
- Include available work-surface signals and anchors such as repo, local checkout, workspace root, normalized Git remote, service, deployment, Cabinet Mission, Docket project, or artifact path when relevant.
- Use `den.work_plan.request_handoff` when the plan becomes broader background work that needs `curate` review and `work` execution.
- Do not treat every response as requiring a plan; very small single-step answers can proceed without one.

## Work role behavior

`work` is Letta Code-backed and should continue to use Letta Code's native planning affordances where available. Den should still expose the workboard tools to `work` so approved tasks can surface status to Den, operators, and other Bear roles.

`work` must execute only approved task definitions, not channel-originated workboard plans directly.

When `work` executes against a repo-oriented task, it may materialize a fresh checkout in a different runtime. That checkout should be treated as another observed anchor on the same work surface rather than as unrelated planning state.

## Memory interaction

Planning state is not shared Bear memory by default.

Use this ladder:

1. Keep tactical progress in the Den workboard.
2. Write a plan artifact for pre-implementation approval when mutation should be gated.
3. Write role-local memory only when the plan or its rationale will matter beyond the current mini-project.
4. Request curation when lessons, decisions, or results should become shared `core/` memory.
5. Use task intents and approved tasks for autonomous/background work.

A simple Den memory log of plans underway/completed can be derived from `bear_work_plan_events` and selected summaries rather than copying every plan into `core`.

## Current implementation state

Implemented:

- Den DB-backed workboard schema and event table.
- Work plan validation with at most one `in_progress` item.
- Den workboard tools and role policy.
- ACP prompt injection of current session workboard context.
- ACP pair exposure of Den workboard tools.
- ACP pair plan-mode DB schema and audit events.
- Den plan-mode tools: `den.plan_mode.enter`, `den.plan_mode.status`, `den.plan_mode.exit`, and `den.plan_mode.cancel`.
- ACP plan-mode prompt reminders.
- Pair-local `plan` memory entries under `pair/plans/` for markdown plan artifacts.
- ACP `ask` and `plan` modes expose read/search/inspect tools; `write` mode enables mutation/execution/browser tools, which still require concrete ACP client approval and Den/adapter policy checks.
- `den.plan_mode.exit` submits or updates a markdown plan artifact; it no longer creates a durable mutation gate or requires an ACP permission request.
- `record_plan_approval` records explicit approval from the authenticated human when useful for workflow/audit, but planning approval is not a global prerequisite for all mutation.
- ACP native `plan` updates projected from Den workboard items.
- ACP native mode/config updates using `Ask`, `Plan`, and `Write` modes.
- ACP `session/new` / `session/resume` mode state with both modern `configOptions` and legacy `modes` compatibility.

Planned:

- Operator and chat UI for active/completed plans.
- A unified Bear-level `list_plans` view that includes live workboard plans, active/submitted planning artifacts, saved plan artifacts, handoffs/task intents, and available work-surface references.
- Work-surface resolution and canonicalization from ACP workspace roots, local checkout paths, Git remotes, Cabinet Mission references, Docket project references, service names, deployments, and artifact paths.
- Handoff implementation from workboard items to durable task intents.
- Reflection/curate review of completed plan summaries and durable lessons.

## Related docs

- [Bears and Den](BEARS_AND_DEN.md)
- [Bear agent roles](BEAR_AGENT_ROLES.md)
- [Memory model](MEMORY_MODEL.md)
- [Tasks and autonomy](TASKS_AND_AUTONOMY.md)
- [Bear Workplaces ADR](../architecture/adr/bear-workplaces.md)
