# Capabilities and Skills

Capabilities describe what a Bear is allowed to do. Tools are the concrete actions available to agents. Skills are reusable procedures or knowledge packages that help a Bear use its tools and memory well.

## Summary

- A capability is a product-level permission or ability.
- A tool is an executable action exposed to one or more Bear agent roles.
- A skill is reusable know-how installed for selected roles.
- Den owns the canonical capability and skill configuration.
- Durable skill learning happens through proposal and review, not raw self-installation.

## Capabilities

A capability is something a Bear can do from the user's or administrator's perspective.

Examples:

- use GitHub,
- post to Slack,
- read a project repository,
- create background tasks,
- monitor a webhook,
- use company conventions while coding.

Capabilities should be described in product language. They may map to tools, skills, policies, credentials, or external integrations underneath.

## Tools

A tool is a concrete action an agent can call.

Examples:

- read or write allowed memory paths,
- write a task intent,
- approve a task intent,
- post to an integration,
- write an observation,
- relay a client-side ACP tool call.

Tools are role-scoped. A tool that is safe for `work` may be unsafe for `talk`; a tool that is appropriate for `curate` may be inappropriate for `watch`.

## Skills

A skill is reusable know-how.

Skills can encode:

- coding conventions,
- integration procedures,
- recurring workflows,
- reflection patterns,
- subscription parsing guidance,
- or team-specific practices.

Skills are not the same as memory. Memory stores what the Bear knows; skills shape how the Bear performs a repeatable activity.

## Role applicability

Not every role should receive every skill or tool.

| Role | Typical capability shape |
|------|--------------------------|
| `talk` | Conversation, task-intent capture, general user help. |
| `pair` | Client-mediated collaboration and coding/design assistance. |
| `curate` | Reflection, review, memory integration, skill approval. |
| `work` | Approved outbound execution with scoped integration tools. |
| `watch` | Inbound event interpretation and observation writing. |

Role applicability keeps the Bear useful without giving every internal agent every power.

## Skill proposals

Agents do not install durable skills directly.

The normal skill-learning flow is:

1. A role identifies a reusable procedure or convention.
2. The role submits a skill proposal through Den.
3. `curate` reviews the proposal.
4. `curate` chooses whether to approve it and which roles it applies to.
5. Den updates the Bear skill manifest.
6. Den provisions or reconciles affected roles.

This gives the Bear a way to learn without letting any one role mutate shared capability unchecked.

## What Den owns

Den owns the canonical records for:

- which capabilities a Bear has,
- which tools each role may use,
- which skills are installed,
- which roles a skill applies to,
- and whether runtime state matches the intended configuration.

Agents may request capability changes. Den enforces and installs them.

## Product language

Prefer:

- “This Bear has the GitHub capability.”
- “This skill applies to `talk` and `pair`.”
- “The Bear proposed a new skill for review.”
- “Den provisions tools and skills according to policy.”

Avoid:

- “Every agent can use every tool.”
- “Skills are just memories.”
- “The Bear installed a durable skill without review.”
- “A capability is only a tool id.”

## Related docs

- [Bears and Den](BEARS_AND_DEN.md)
- [Bear agent roles](BEAR_AGENT_ROLES.md)
- [Memory model](MEMORY_MODEL.md)
- [Tasks and autonomy](TASKS_AND_AUTONOMY.md)
- [Observations and subscriptions](OBSERVATIONS_AND_SUBSCRIPTIONS.md)
- [Multi-agent architecture ADR](../architecture/adr/multi-agent-architecture.md)
- [Den Bear spec](../../services/den/docs/bear-spec.md)
