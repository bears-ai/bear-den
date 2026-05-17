# ADR: Bear work surfaces for planning and work activity

**Status:** Accepted
**Date:** 2026-05-17
**Supersedes:** Prior `Workplace` terminology previously used at this path; archived in `bear-workplaces.archived-2026-05-17.md`
**Deciders:** Hans

## Context

BEARS has several planning-related objects:

- live workboard plans in Den,
- ACP `pair` plan-mode artifacts and mode state,
- durable plan artifacts under role-local MemFS paths such as `pair/plans/`,
- task intents and future Docket work,
- work results and future Cabinet references.

The first implementation naturally grouped plans by role and session because `pair`, `talk`, and `work` interact through different runtimes. That grouping is useful for provenance and policy, but it is not the right primary product concept.

A Bear can work across many durable settings:

- multiple Git repositories,
- local editor workspaces and checkouts,
- services in the BEARS stack,
- deployment environments,
- Cabinet Missions,
- future Docket projects,
- long-running household or organization responsibilities,
- research topics or operational areas.

A user asking “do you have plans?” usually means “does this Bear have plans for this repo, service, mission, project, or ongoing responsibility — or in general?”, not “does this role have a live plan in this ACP session?” The production planning issue where `list_plans` found no live workboard plans while a durable `pair/plans/*.md` artifact existed demonstrates that the system needs a durable grouping concept that is broader than role ownership and less overloaded than “context”.

The term **context** is dangerous in BEARS because it already refers to model context windows, prompt context, ACP client context, Den situation briefings, memory context, and workspace context. The term **Space** is also no longer preferred as a primary concept. We need a durable architectural noun for the coherent scope of work that plans, tasks, artifacts, memory, and activity attach to.

## Decision

BEARS will use **work surface** as the primary product and architecture term for a durable work grouping.

A **work surface** is the durable work context a Bear is acting on: a repo, local checkout, service, deployment, Mission, project, or other coherent scope of work.

A work surface is:

- Bear-level, not role-owned.
- Durable enough to survive conversations and runtime sessions.
- Able to map to external systems or local environments.
- Able to collect multiple plans, tasks, artifacts, memory references, and activity.
- Discoverable from runtime signals such as Git remotes, workspace roots, checkout paths, Cabinet Mission ids, Docket project ids, service names, deployment environments, and artifact paths.
- Not an execution authorization boundary by itself.

Role remains important, but role is provenance, participation, and execution metadata — not the primary owner of a plan.

## Terminology

### Work surface

The durable work context the Bear is acting on.

Examples:

- `BEARS monorepo`
- `Home renovation`
- `Production Den deployment`
- `github.com/example/app`
- `Cabinet Mission: Renovation Budget`
- `Docket Project: Billing Automation`

### Work-surface anchor

A structured anchor that identifies, locates, or helps resolve a work surface in an external or runtime system.

Examples:

| Anchor kind | Example key / URI | Meaning |
|---|---|---|
| `git_repo` | `git@github.com:org/repo.git` | Canonical repository identity for the work surface. |
| `workspace_root` | `/Users/alice/dev/repo` | Local editor/workspace root observed while acting on the work surface. |
| `checkout_path` | `/tmp/worktrees/repo` | Local checkout materialization of the work surface. |
| `branch` | `repo#feature/acp-plans` | Branch-specific evidence or binding. |
| `cabinet_mission` | `mission:abc` | Cabinet Mission related to the work surface. |
| `docket_project` | `project:abc` | Docket project related to the work surface. |
| `service` | `bears-den` | Service/component identity. |
| `deployment` | `production` | Deployed environment identity. |
| `acp_session` | `acp-...` | Session where work occurred or originated. |
| `conversation` | `conv-...` | Conversation related to the work surface. |
| `artifact_path` | `pair/plans/plan_x.md` | Artifact associated with the work surface. |

### Session work surface

The work surface inferred for an active session, if any. For ACP, this may be inferred from workspace roots, checkout paths, Git remotes, cwd, or explicit client/session metadata.

### Plan work-surface attachment

The work surface associated with a plan. A plan may be Bear-level without a specific work surface, or it may be attached to one or more work surfaces.

### Workspace / checkout

A technical local environment such as an editor workspace root or filesystem checkout. A workspace root or checkout can be an observed anchor for a work surface, but it is not by itself the durable product concept.

### Cabinet Mission and Docket Project

A Cabinet Mission or Docket Project can be an anchor for a work surface or, in some cases, map one-to-one with one. Not every work surface is a Cabinet Mission or Docket Project.

## Model direction

The future normalized model should look like this conceptually:

```text
bear_work_surfaces
- id
- bear_id
- slug
- name
- kind
- summary
- status
- created_at
- updated_at

bear_work_surface_anchors
- work_surface_id
- anchor_kind
- anchor_key
- uri
- label
- metadata
- canonical boolean

bear_plans
- id
- bear_id
- work_surface_id nullable
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

| Current field/object | Work-surface-aware interpretation |
|---|---|
| `bear_work_plans.bear_id` | Plan belongs to the Bear. |
| `bear_work_plans.owner_role` | Current provenance / primary role, not durable product ownership. |
| `bear_work_plans.owner_agent_id` | Agent provenance. |
| `bear_work_plans.source_acp_session_id` | Session anchor candidate for a work surface. |
| `bear_work_plans.source_conversation_id` | Conversation anchor candidate for a work surface. |
| `bear_work_plans.workspace_context` | Early unnormalized work-surface anchor metadata. |
| `acp_plan_mode_sessions.plan_artifact_path` | Plan artifact anchor/reference. |
| `pair/plans/*.md` | Durable role-local plan artifact that should be discoverable through Bear-level planning tools. |

Future migrations may rename or supplement `owner_role` with clearer provenance fields such as `created_by_role`, `last_updated_by_role`, `participant_roles`, and `intended_executor_role`.

## Planning implications

Planning is Bear-level and work-surface-aware.

`list_plans` should become a unified Bear planning view that can include:

- live workboard plans,
- active/submitted plan-mode artifacts,
- saved plan artifacts,
- handoff/task-intent state,
- future Docket tasks/projects,
- relevant work-surface anchors.

The default `list_plans` behavior should be user-friendly:

- prioritize plans for the current session work surface when known,
- also surface Bear-level pending approvals/handoffs,
- avoid saying “no plans” unless it checked both current work-surface plans and broader Bear-level pending plans,
- distinguish “no active plans for this work surface” from “no plans anywhere”.

## Role, authority, and delegation

Role is not the primary owner of a plan. Role is provenance and participation metadata.

A `pair` agent may create a plan that is intended for `work`, but `work` should not treat an arbitrary `pair` plan as executable. Cross-role visibility is not cross-role authority.

A safe delegation flow is:

```text
pair creates Bear-level / work-surface-attached plan
  -> pair requests handoff to work
  -> user or curate approval promotes it to an approved task/Docket item
  -> Den dispatches approved work to work
  -> work updates execution status and writes results
  -> curate reviews durable outcomes
```

This permits rich cross-role planning without letting one role directly command another outside an approval/dispatch boundary.

## Work-surface discovery and resolution

Work surfaces may be created explicitly, inferred from evidence, canonicalized from stronger anchors, re-materialized in new runtimes, or confirmed by the user.

Initial inference signals may include:

- ACP `cwd`,
- ACP workspace roots,
- local checkout paths,
- Git remotes discovered from a workspace root,
- repository URLs in user prompts or memory,
- service names in the BEARS stack,
- Cabinet Mission references,
- Docket project references,
- artifact paths.

Inference should be conservative. If a work surface cannot be confidently identified, a plan can remain Bear-level with session/conversation anchors until a clearer scope is assigned.

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

## Checkout-originated continuity

For repo-oriented work, a local checkout or workspace root may be the first observed anchor for a work surface, but it should not become the only durable identity.

A common lifecycle is:

1. **Observed** — `pair` encounters a checkout or workspace root.
2. **Provisional** — Den creates or resolves a provisional work surface from available evidence.
3. **Canonicalized** — Den links the provisional surface to a stronger canonical anchor such as a normalized Git remote.
4. **Bound** — The current session, conversation, or run records observed anchors for that work surface.
5. **Re-materialized** — Another role, such as `work`, attaches a different checkout or runtime binding to the same work surface.
6. **Merged or refined** — Later evidence reconciles duplicate provisional surfaces while preserving provenance.

Key principle: plans, tasks, memory, and workboard state should attach to the durable **work_surface_id**, not only to a machine-local path.

## Consequences

### Positive

- Avoids overloading “context”.
- Avoids treating role ownership as the primary planning model.
- Aligns planning, memory, and task continuity around the same work-surface concept.
- Supports Bears that work across multiple Git repositories, services, deployments, Missions, or projects.
- Gives `list_plans` a coherent path to become a unified Bear-level planning surface.
- Gives Docket, Cabinet, memory, artifacts, and work results a shared grouping concept without making any one of them the source of truth.
- Allows role-aware policy and provenance without making plans role-owned.

### Tradeoffs

- Adds a durable grouping concept that must be explained carefully.
- Requires distinguishing workspace/checkout evidence from durable work-surface identity.
- May require schema evolution from existing `owner_role`/`workspace_context` fields.
- Work-surface inference can be wrong if implemented too aggressively.

## Guidance for implementation

Use “work surface” for the durable work context plans, tasks, artifacts, memory, and activity attach to.

Use “workspace” and “checkout” only for local/editor/filesystem concepts such as ACP workspace roots and local materializations.

Use “situation” for trusted Den interaction briefings.

Use “context” only where protocol/model terminology already requires it, such as ACP client context or prompt context.

For near-term planning work:

1. Enrich `list_plans` to include live workboard plans, plan-mode artifacts, and saved plan artifacts.
2. Include available work-surface anchors in returned plans, even if unnormalized.
3. Change user-facing language from “no plans” to more precise phrases like “no active plans for this work surface” or “no Bear-level pending plans”.
4. Treat role fields as provenance and policy hints, not product ownership.

## Related documents

- [Planning in BEARS](../../concepts/PLANNING.md)
- [Memory Model](../../concepts/MEMORY_MODEL.md)
- [Bear Charter and Cabinet Missions](../../concepts/BEAR_CHARTER_AND_CABINET_MISSIONS.md)
- [Task System Implementation Plan](../../planning/TASK_SYSTEM_IMPLEMENTATION_PLAN.md)
- [Role-Aware Tool Guidance Plan](../../planning/ROLE_AWARE_TOOL_GUIDANCE_PLAN.md)
