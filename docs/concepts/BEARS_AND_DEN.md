# Bears and Den

A **Bear** is the durable assistant identity users interact with. **Den** is the control plane that makes Bears real: it provisions them, routes traffic to them, governs access, schedules work, and keeps their underlying agents reconciled.

## Summary

- A Bear is the product object: the assistant with memory, skills, conversations, tasks, and identity.
- Den is the system of record and control plane for Bears.
- Letta agents are implementation components underneath a Bear.
- Surfaces such as Slack, web chat, IDEs, task dispatch, and webhooks reach a Bear through Den-managed routing.

## What is a Bear?

A Bear is one coherent assistant from the user's perspective.

A Bear can:

- remember durable knowledge,
- talk with users,
- pair inside tools,
- learn skills through review,
- watch external events,
- and perform approved background work.

Internally, a Bear is made of multiple specialized agents. Conceptually, though, users should not have to think of a Bear as five separate bots. The Bear is the stable identity; the internal agent roles are how the system delivers that identity safely across contexts.

## What is Den?

Den is the control plane for Bears.

Den is responsible for:

- creating and provisioning Bears,
- tracking Bear membership and access,
- routing each surface to the correct internal agent role,
- enforcing policy around tasks, tools, skills, and memory,
- scheduling curate runs and background work,
- receiving inbound events,
- maintaining canonical configuration,
- and reconciling runtime state against that configuration.

Den does not replace the Bear's assistant identity. Users talk to Bears; Den manages Bears.

## What a Bear is not

A Bear is not:

- a single Letta agent,
- a single chat conversation,
- a Slack bot alone,
- an IDE session alone,
- a task runner alone,
- or Den itself.

The Bear is the durable assistant identity that spans those surfaces and capabilities.

## What Den is not

Den is not:

- the assistant persona,
- the LLM,
- Letta,
- a Letta Code harness,
- or the place where all agent reasoning happens.

Den owns control, policy, routing, scheduling, and reconciliation. The Bear's internal agents do the role-specific reasoning and work.

## Relationship to Letta

Letta provides the underlying agent runtime and persistence. A Bear maps to multiple Letta agents, each with a fixed internal role.

Den owns the Bear abstraction above Letta:

- Den knows which Letta agents belong to a Bear.
- Den knows each agent's role.
- Den provisions prompts, tools, skills, memory policy, and runtime configuration.
- Den repairs or reports drift when runtime state diverges from canonical configuration.

## Relationship to surfaces

Different surfaces reach different internal Bear agent roles:

| Surface | Typical role |
|---------|--------------|
| Slack, web chat, Discord | `talk` |
| IDEs and ACP clients | `pair` |
| Scheduled or approved background work | `work` |
| Webhooks, polling, queues, subscriptions | `watch` |
| Memory integration and review | `curate` |

The surface changes, but the user-facing identity remains the Bear.

## Product language

Prefer:

- “your Bear” for the assistant identity,
- “Den manages Bears” for the control plane,
- “Bear agent roles” for `talk`, `pair`, `curate`, `work`, and `watch`,
- “membership roles” or “access roles” for human permissions.

Avoid:

- “Den answered the user,” unless describing infrastructure logs,
- “a Bear is a Letta agent,”
- “the five roles are five separate assistants,”
- or “Bear roles” when the intended meaning is specifically internal agent roles.

## Related docs

- [Bear agent roles](BEAR_AGENT_ROLES.md)
- [Memory model](MEMORY_MODEL.md)
- [Tasks and autonomy](TASKS_AND_AUTONOMY.md)
- [Capabilities and skills](CAPABILITIES_AND_SKILLS.md)
- [Observations and subscriptions](OBSERVATIONS_AND_SUBSCRIPTIONS.md)
- [Identity and membership](IDENTITY_AND_MEMBERSHIP.md)
- [Multi-agent architecture ADR](../architecture/adr/multi-agent-architecture.md)
- [Den Bear spec](../../services/den/docs/bear-spec.md)
