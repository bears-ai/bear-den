# ADR: Bear Workplaces for Planning and Work Activity

**Status:** Superseded and archived
**Superseded by:** `bear-work-surfaces.md` and the updated `bear-workplaces.md` path
**Original Date:** 2026-05-11
**Archived Date:** 2026-05-17
**Deciders:** Hans

> Historical note: this ADR introduced useful ideas about durable grouping for plans, tasks, artifacts, memory, and activity, but used the now-superseded product term **Workplace**. It is retained here for historical reference only.

## Original ADR text

# ADR: Bear Workplaces for Planning and Work Activity

**Status:** Accepted
**Date:** 2026-05-11
**Deciders:** Hans

## Context

BEARS now has several planning-related objects:

- live workboard plans in Den,
- ACP `pair` plan-mode artifacts and mode state,
- durable plan artifacts under role-local MemFS paths such as `pair/plans/`,
- task intents and future Docket work,
- work results and future Cabinet references.

The first implementation naturally grouped plans by role and session because `pair`, `talk`, and `work` interact through different runtimes. That grouping is useful for provenance and policy, but it is not the right primary product concept.

A Bear can work across many real settings:

- multiple Git repositories,
- local editor workspace roots,
- services in the BEARS stack,
- deployment environments,
- Cabinet Missions,
- future Docket projects,
- long-running household or organization responsibilities,
- research topics or operational areas.

A user asking “do you have plans?” usually means “does this Bear have plans for this work setting or in general?”, not “does this role have a live plan in this ACP session?” The production planning issue where `list_plans` found no live workboard plans while a durable `pair/plans/*.md` artifact existed demonstrates that the system needs a durable grouping concept that is broader than role ownership and less overloaded than “context”.

The term **context** is dangerous in BEARS because it already refers to model context windows, prompt context, ACP client context, Den situation briefings, memory context, and workspace context. We need a product-level noun for a Bear-level work setting.

## Decision

BEARS will use **Workplace** as the product and architecture term for a durable Bear-level work setting.

A **Workplace** is a durable, Bear-level work setting that groups plans, tasks, artifacts, memory, and activity around a coherent place or scope of work.

A Workplace is:

- Bear-level, not role-owned.
- Durable enough to survive conversations and runtime sessions.
- Able to map to external systems or local environments.
- Able to collect multiple plans, tasks, artifacts, and memory references.
- Discoverable from runtime signals such as Git remotes, workspace roots, Cabinet Mission ids, Docket project ids, service names, and deployment environments.
- Not an execution authorization boundary by itself.

Role remains important, but role is provenance, participation, or execution metadata — not the primary owner of a plan.

## Terminology

### Workplace

The durable Bear-level work setting.

Examples:

- `BEARS monorepo`
- `Home renovation`
- `Production Den deployment`
- `github.com/example/app`
- `Cabinet Mission: Renovation Budget`
- `Docket Project: Billing Automation`

### Workplace reference

A structured reference that identifies or locates a Workplace in an external or runtime system.

Examples:

| Reference kind | Example key / URI | Meaning |
|---|---|---|
| `git_repo` | `git@github.com:org/repo.git` | A repository associated with the Workplace. |
| `workspace_root` | `/Users/alice/dev/repo` | A local editor/workspace root that indicates the Workplace. |
| `branch` | `repo#feature/acp-plans` | A branch within a repository. |
| `cabinet_mission` | `mission:abc` | A Cabinet Mission related to the Workplace. |
| `docket_project` | `project:abc` | A Docket project related to the Workplace. |
| `service` | `bears-den` | A service or component. |
| `deployment` | `production` | A deployed environment. |
| `acp_session` | `acp-...` | A session where work occurred or originated. |
| `conversation` | `conv-...` | A Letta/Den conversation related to the Workplace. |

### Session Workplace

The Workplace inferred for an active session, if any. For ACP, this may be inferred from workspace roots, Git remotes, cwd, or explicit client/session metadata.

### Plan Workplace

The Workplace associated with a plan. A plan may be Bear-level without a specific Workplace, or it may be scoped to one or more Workplaces.

### Workspace

A technical/local environment such as an editor workspace root or filesystem checkout. A workspace root can be evidence for a Workplace, but it is not itself the product concept.

### Cabinet Mission and Docket Project

A Cabinet Mission or Docket Project can be a Workplace reference or, in some cases, map one-to-one to a Workplace. Not every Workplace is a Cabinet Mission or Docket Project.

## Model direction

The future normalized model should look like this conceptually:

```text
bear_workplaces
- id
- bear_id
- slug
- name
- kind
- summary
- status
- created_at
- updated_at

bear_workplace_refs
- workplace_id
- ref_kind
- ref_key
- uri
- label
- metadata

bear_plans
- id
- bear_id
- workplace_id nullable
- title
- status
- kind
- created_by_role
- current_phase
- created_at
- updated_at

bear_plan_artifacts
- plan_id
- kind
- path_or_uri
- metadata

bear_plan_participants
- plan_id
- role
- participation_kind

bear_plan_handoffs
- plan_id
- from_role
- to_role
- status
- approved_by_user_id nullable
- task_id nullable
```

This ADR does not require adding these tables immediately. It defines the target concept and vocabulary.

## Current implementation interpretation

Existing planning schema should be interpreted through this lens:

| Current field/object | Workplace-aware interpretation |
|---|---|
| `bear_work_plans.bear_id` | Plan belongs to the Bear. |
| `bear_work_plans.owner_role` | Current provenance / primary role, not durable product ownership. |
| `bear_work_plans.owner_agent_id` | Agent provenance. |
| `bear_work_plans.source_acp_session_id` | Workplace/session reference candidate. |
| `bear_work_plans.source_conversation_id` | Workplace/conversation reference candidate. |
| `bear_work_plans.workspace_context` | Early unnormalized Workplace reference metadata. |
| `acp_plan_mode_sessions.plan_artifact_path` | Plan artifact reference. |
| `pair/plans/*.md` | Durable role-local plan artifact that should be discoverable through Bear-level planning tools. |

Future migrations may rename or supplement `owner_role` with clearer provenance fields such as `created_by_role`, `last_updated_by_role`, `participant_roles`, and `intended_executor_role`.

## Planning implications

Planning is Bear-level and Workplace-aware.

`list_plans` should become a unified Bear planning view that can include:

- live workboard plans,
- active/submitted plan-mode artifacts,
- saved plan artifacts,
- handoff/task-intent state,
- future Docket tasks/projects,
- relevant Workplace references.

The default `list_plans` behavior should be user-friendly:

- prioritize plans for the current session Workplace when known,
- also surface Bear-level pending approvals/handoffs,
- avoid saying “no plans” unless it checked both current Workplace plans and broader Bear-level pending plans,
- distinguish “no active plans for this Workplace” from “no plans anywhere”.

## Role, authority, and delegation

Role is not the primary owner of a plan. Role is provenance and participation metadata.

A `pair` agent may create a plan that is intended for `work`, but `work` should not treat an arbitrary `pair` plan as executable. Cross-role visibility is not cross-role authority.

A safe delegation flow is:

```text
pair creates Bear-level / Workplace-scoped plan
  -> pair requests handoff to work
  -> user or curate approval promotes it to an approved task/Docket item
  -> Den dispatches approved work to work
  -> work updates execution status and writes results
  -> curate reviews durable outcomes
```

This permits rich cross-role planning without letting one role directly command another outside an approval/dispatch boundary.

## Workplace and work-surface discovery

Workplaces and work surfaces may be created explicitly, inferred from evidence, or confirmed by the user.

Initial inference signals may include:

- ACP `cwd`,
- ACP workspace roots,
- Git remotes discovered from a workspace root,
- repository URLs in user prompts or memory,
- service names in the BEARS stack,
- Cabinet Mission references,
- Docket project references,
- artifact paths.

Inference should be conservative. If a Workplace or work surface cannot be confidently identified, a plan can remain Bear-level with session/conversation references until a clearer scope is assigned.

The resolution state should be visible to the Bear, not only to Den. Agents should be able to communicate their current assumption, ask the user to verify it, or ask the user to choose between plausible candidates. User confirmation can raise confidence for the current thread and should be preserved as provenance when plans, memory, or artifacts are later associated with that work surface.

Recommended resolution states:

| State | Meaning |
|---|---|
| `unresolved` | No useful current work-surface candidate is known. |
| `candidate` | One likely work surface is suggested by session/workspace/conversation hints. |
| `ambiguous` | Multiple plausible work surfaces exist. |
| `resolved` | Evidence such as canonical anchors, workspace metadata, or durable references identifies the surface. |
| `confirmed` | The user explicitly confirmed the work surface for the thread. |
| `rejected` | A candidate was explicitly rejected. |

## Consequences

### Positive

- Avoids overloading “context”.
- Avoids treating role ownership as the primary planning model.
- Supports Bears that work across multiple Git repositories or projects.
- Gives `list_plans` a coherent path to become a unified Bear-level planning surface.
- Gives Docket, Cabinet, memory, artifacts, and work results a shared grouping concept without making any one of them the source of truth.
- Allows role-aware policy and provenance without making plans role-owned.

### Tradeoffs

- Adds a new product noun that must be explained carefully.
- Requires distinguishing `Workspace` from `Workplace` in docs and UI.
- May require schema evolution from existing `owner_role`/`workspace_context` fields.
- Workplace inference can be wrong if implemented too aggressively.

## Guidance for implementation

Use “Workplace” when referring to the durable Bear-level work setting.

Use “workspace” only for local/editor/filesystem concepts such as ACP workspace roots and checkouts.

Use “situation” for trusted Den interaction briefings.

Use “context” only where protocol/model terminology already requires it, such as ACP client context or prompt context.

For near-term planning work:

1. Enrich `list_plans` to include live workboard plans, plan-mode artifacts, and saved plan artifacts.
2. Include available Workplace reference metadata in returned plans, even if unnormalized.
3. Change user-facing language from “no plans” to more precise phrases like “no active plans for this Workplace” or “no Bear-level pending plans”.
4. Treat role fields as provenance and policy hints, not product ownership.

## Relationship to work surfaces

A Bear manifests through agents performing roles. An agent acts in a **Workplace** and may act on a **work surface**.

- **Workplace** remains the durable Bear-level work setting and operating scope described by this ADR.
- A **work surface** is the concrete repo, service, deployment, Mission, project, or other coherent scope of work an agent may currently be acting on.
- Canonical work-surface memory may live under shared paths such as `core/work_surfaces/...`.
- Some roles may also keep role-local working memory about a work surface while acting in their Workplace.

This phrasing is meant to avoid muddling where an agent is operating (`in` a Workplace) with what it is currently engaging (`on` a work surface).

## Related documents

- [Planning in BEARS](../../concepts/PLANNING.md)
- [Memory Model](../../concepts/MEMORY_MODEL.md)
- [Bear Charter and Cabinet Missions](../../concepts/BEAR_CHARTER_AND_CABINET_MISSIONS.md)
- [Task System Implementation Plan](../../planning/TASK_SYSTEM_IMPLEMENTATION_PLAN.md)
- [Role-Aware Tool Guidance Plan](../../planning/ROLE_AWARE_TOOL_GUIDANCE_PLAN.md)
