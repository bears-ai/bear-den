# Tasks and Autonomy

Tasks are how a Bear turns a user's request or an external observation into reviewed background work. Autonomy flows through intent, review, policy, execution, and result promotion.

## Summary

- `talk` and `pair` capture requests for external or background work as task intents or handoff requests.
- `curate` reviews task intents before they become approved tasks.
- Den enforces policy, generates durable artifact paths, stores task state, schedules runs, and dispatches work.
- `work` executes approved tasks within a scoped run context.
- Future Docket functionality may own the richer task/project lifecycle, while Den remains the Bear control plane.
- Results are reviewed before durable learnings return to shared memory.

## Why tasks exist

Some requests should not run immediately inside a conversation.

Examples:

- “Check this status every morning.”
- “Post a summary to Slack after standup.”
- “Watch for new issues and draft a response.”
- “Run this report every Friday.”

These requests imply background work, external effects, recurrence, or delayed execution. They become tasks instead of direct chat side effects.

## Task intent

A task intent is a proposed task.

It captures what the user or system wants, but it is not yet approved for execution. `talk` and `pair` write task intents, or request handoff from a workboard plan into a task intent, when a synchronous interaction produces a request for background or external work.

A task intent should describe:

- the requested outcome,
- the source of the request,
- relevant scope,
- schedule or trigger,
- likely tools or integrations,
- work-surface or artifact references when relevant,
- and any risk or approval context.

For schema-owned durable artifacts such as task intents, agents provide semantic fields; Den chooses the path.

## Approved task

An approved task is a task intent that has passed review.

`curate` reviews task intents and decides whether to approve, reject, or refine them. Den performs the controlled state transition and stores the approved task in the canonical task area.

Approval does not mean unlimited authority. Approved tasks still run under policy, scope, allowed tools, and audit requirements.

## Task run

A task run is one execution of an approved task.

A recurring task may have many runs. A one-off task may have one run. Each run should have:

- a task id,
- a run id,
- a role and channel context,
- a work-surface reference,
- an execution context,
- allowed tools and scope,
- logs or notes,
- and a result.

High-risk runs may require additional human approval before execution.

For checkout-oriented work, the run may materialize a fresh checkout in a new runtime. That new checkout should be recorded as an observed anchor on the same work surface rather than treated as unrelated work.

## Role responsibilities

| Role/System | Responsibility |
|-------------|----------------|
| `talk` | Capture chat-originated task intents. |
| `pair` | Capture client/tool-originated task intents. |
| `curate` | Review intents, approve or reject work, and later review results. |
| Den | Enforce policy, generate durable artifact paths, schedule tasks, dispatch runs, audit transitions. |
| `work` | Execute approved runs within the Den-issued scope. |
| `watch` | Produce observations that may lead to derived task intents. |

## What autonomy is not

Autonomy is not a chat agent secretly doing arbitrary work.

A Bear's autonomous work should be:

- requested or event-derived,
- reviewed,
- scoped,
- auditable,
- policy-bound,
- and executed by the `work` role rather than by every role.

## Results

`work` writes results for each run. `curate` reviews those results and promotes durable learnings or summaries into `core/` when appropriate.

This lets users later ask what happened without giving conversational roles raw access to every execution detail.

## Work-surface continuity across roles

A common first use case is:

1. a Bear in `pair` works in a local checkout,
2. creates a long-term plan or task intent,
3. and later a Bear in `work` checks out the same repo in a different runtime and executes against it.

To support that flow, plans, task intents, approved tasks, and runs should attach to the durable **work surface id**. Local checkouts and runtime checkouts should be stored as observed anchors for that work surface. When a more portable anchor such as a normalized Git remote is available, Den should use it to canonicalize the work surface early so later runs can re-materialize the same ongoing work in a new context.

## Product language

Prefer:

- “The Bear can perform approved background work.”
- “Requests for external action become task intents or reviewed handoffs.”
- “`curate` reviews work before Den dispatches it.”
- “`work` executes within an approved scope.”

Avoid:

- “The chat agent can do anything later.”
- “Autonomy bypasses review.”
- “A subscription directly triggers outbound action.”
- “Approval is the same as unlimited access.”

## Related docs

- [Bear Den and Den](BEARS_AND_DEN.md)
- [Bear agent roles](BEAR_AGENT_ROLES.md)
- [Memory model](MEMORY_MODEL.md)
- [Observations and subscriptions](OBSERVATIONS_AND_SUBSCRIPTIONS.md)
- [Capabilities and skills](CAPABILITIES_AND_SKILLS.md)
- [Planning in Bear Den](PLANNING.md)
- [Schema-first path strategy ADR](../architecture/adr/schema-first-path-strategy.md)
- [Bear Workplaces ADR](../architecture/adr/bear-workplaces.md)
- [Multi-agent architecture ADR](../architecture/adr/multi-agent-architecture.md)
- [Den Bear spec](../../services/den/docs/bear-spec.md)
