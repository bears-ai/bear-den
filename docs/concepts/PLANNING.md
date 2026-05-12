# Planning in BEARS

Planning in BEARS means a user-visible mini-project plan for an active body of work. Plans are Bear-level records with role provenance, and they may be associated with a **Workplace**: a durable Bear-level work setting such as a repo, service, deployment, Cabinet Mission, Docket project, or long-running responsibility.

Planning is distinct from continuation, which keeps an agent going in an existing task loop, and from a Cabinet Mission, which is a larger shared knowledge/work container. BEARS aligns with Letta Code's two planning layers while adapting them to Den's multi-role Bear architecture.

## Two layers

### 1. Live progress tracking

Live progress tracking is the lightweight todo/status list an agent updates while it works.

Letta Code has this as `TodoWrite` for Claude-style toolsets and `UpdatePlan` for Codex-style toolsets. In Letta Code this is mostly UI/state rendering; `UpdatePlan` is a no-op tool implementation whose arguments drive the visible plan, with a simple list of `{ step, status }` and statuses such as `pending`, `in_progress`, and `completed`.

BEARS' Den equivalent is the **workboard**:

- table: `bear_work_plans`
- audit stream: `bear_work_plan_events`
- Den tools:
  - `den.work_plan.update`
  - `den.work_plan.get_status`
  - `den.work_plan.list`
  - `den.work_plan.request_handoff`

A workboard plan is intentionally small and operational. It records what the Bear is trying to do now, the current item, blockers, the role that created or last updated it, and whether the plan should be private to that role, visible to the same user, visible to the Bear, or ready for handoff.

### 2. Pre-implementation planning gate

Letta Code also has a separate gate: `EnterPlanMode` and `ExitPlanMode`.

In Letta Code, entering plan mode requires user approval, switches permissions to a `plan` mode, allows read/search/inspection, denies code edits and non-read-only shell commands, allows writing only to `~/.letta/plans/*.md`, and then exits by presenting the generated markdown plan for user approval. Approval restores the prior permission mode and allows implementation.

BEARS' ACP `pair` plan-mode gate is implemented as a Den-managed session gate. It is conceptually aligned with Letta Code-backed `work` sessions while remaining explicit about ACP client-tool boundaries. In ACP UI, BEARS uses the mode names **Ask**, **Plan**, and **Write**; `Write` means implementation is allowed after approval, while normal client permissions still apply.

The BEARS version is:

1. User or agent requests plan mode.
2. ACP/Den records a planning gate for the current session.
3. Pair may inspect/read/search and use Den read-only tools.
4. Mutating client tools, broad side effects, and non-read-only shell operations are denied by ACP tool policy while the gate is active.
5. Pair may write a durable markdown plan artifact in an approved plan location.
6. Pair exits plan mode by presenting that artifact to the user.
7. User approval closes the gate and restores normal ACP tool permissions for implementation.

## Workplace vs workboard vs plan artifact vs tasks

These are related but different objects:

| Object | Owner | Purpose | Durability |
|--------|-------|---------|------------|
| Workplace | Den DB, future normalized model | Durable Bear-level work setting that groups plans, tasks, artifacts, memory, and activity | Durable product grouping, not an authorization boundary |
| Workboard plan | Den DB | Current visible todo/progress state | Durable enough for resume/status, but not a project archive |
| Plan artifact | MemFS or approved local plan file | Proposed implementation plan for user approval before mutation or future reference | Durable markdown artifact |
| Task intent | MemFS | Request for reviewed background/autonomous work | Durable input to `curate` review |
| Approved task | MemFS | Curate-approved executable work for `work` | Durable task definition |
| Work result | MemFS/Garage | Result/log/report from `work` | Durable output, curatable into `core/` |

A workboard plan can be scoped to a Workplace when Den can infer or record one. A workboard plan can link to a task intent when `den.work_plan.request_handoff` is used. A pre-implementation plan artifact may later be summarized into a workboard plan, task intent, or role-local memory, but it is not itself a Cabinet Mission.

Role is provenance and policy metadata for a plan, not the primary product owner. Cross-role visibility is not cross-role authority: a `pair` plan intended for `work` still needs handoff, review, and Den dispatch before `work` may execute it.

## Pair role behavior

`pair` should use planning like a collaborative coding agent:

- Use `den.work_plan.update` for non-trivial mini-projects, multi-step edits, debugging loops, and user-visible progress.
- Keep at most one item `in_progress`.
- Mark items `completed`, `blocked`, or `cancelled` as reality changes.
- Use `den.work_plan.get_status` to recover the current ACP session's plan after interruption.
- Include available Workplace signals such as repo, workspace root, service, deployment, Cabinet Mission, Docket project, or artifact path when relevant.
- Use `den.work_plan.request_handoff` when the plan becomes broader background work that needs `curate` review and `work` execution.
- Do not treat every response as requiring a plan; very small single-step answers can proceed without one.

## Work role behavior

`work` is Letta Code-backed and should continue to use Letta Code's native planning affordances where available. Den should still expose the workboard tools to `work` so approved tasks can surface status to Den, operators, and other Bear roles.

`work` must execute only approved task definitions, not channel-originated workboard plans directly.

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
- ACP plan-mode tool policy that blocks mutating client tools and mutating Den tools while the gate is active.
- Pair-local `plan` memory entries under `pair/plans/` for markdown plan artifacts.
- ACP permission request emission when `den.plan_mode.exit` submits a plan artifact.
- ACP permission decision handling for plan-mode approve/reject.
- ACP adapter rendering of plan-mode approval options as approve/reject instead of URL-style allow/reject labels.
- ACP native `plan` updates projected from Den workboard items.
- ACP native mode/config updates using `Ask`, `Plan`, and `Write` modes.
- ACP `session/new` / `session/resume` mode state with both modern `configOptions` and legacy `modes` compatibility.

Planned:

- Operator and chat UI for active/completed plans.
- A unified Bear-level `list_plans` view that includes live workboard plans, active/submitted plan-mode gates, saved plan artifacts, handoffs/task intents, and available Workplace references.
- Normalized Workplace records and conservative Workplace inference from ACP workspace roots, Git remotes, Cabinet Mission references, Docket project references, service names, deployments, and artifact paths.
- Handoff implementation from workboard items to durable task intents.
- Reflection/curate review of completed plan summaries and durable lessons.

## Related docs

- [Bears and Den](BEARS_AND_DEN.md)
- [Bear agent roles](BEAR_AGENT_ROLES.md)
- [Memory model](MEMORY_MODEL.md)
- [Tasks and autonomy](TASKS_AND_AUTONOMY.md)
- [Bear Workplaces ADR](../architecture/adr/bear-workplaces.md)
