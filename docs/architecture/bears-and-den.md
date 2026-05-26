# Bears and Den

A **Bear** is the durable assistant identity users interact with. **Den** is the control plane that makes Bears real: it provisions them, routes traffic to them, governs access, schedules work, and keeps their underlying role runtimes reconciled.

For the canonical role model and current role names, see [bear roles](bear-roles.md). This document focuses on the Bear/Den relationship rather than re-explaining the full role model.

## Summary

- A Bear is the product object: the assistant with memory, skills, conversations, tasks, and identity.
- Den is the system of record and control plane for Bears.
- Provider-managed agents such as Letta agents are implementation components that may sit underneath a Bear role during migration, but they are not the primary product concept.
- Surfaces such as Slack, web chat, IDEs, task dispatch, and webhooks reach a Bear through Den-managed routing.

## What is a Bear?

A Bear is one coherent assistant from the user's perspective.

A Bear can:

- remember durable knowledge,
- chat with users,
- pair inside tools,
- learn skills through review,
- watch external events,
- and perform approved background work.

Internally, a Bear executes through multiple specialized roles. During the Letta-backed era, some of those role runtimes are implemented as separate provider-managed agents, but users should not have to think of a Bear as five separate bots. The Bear is the stable identity; the internal roles are how the system delivers that identity safely across contexts.

## What is Den?

Den is the control plane for Bears.

Den is responsible for:

- creating and provisioning Bears,
- tracking Bear membership and access,
- routing each surface to the correct internal role,
- enforcing policy around tasks, tools, skills, and memory,
- scheduling review runs and background work,
- receiving inbound events,
- maintaining canonical configuration,
- and reconciling runtime state against that configuration.

Den does not replace the Bear's assistant identity. Users chat to Bears; Den manages Bears.

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

Den owns control, policy, routing, scheduling, and reconciliation. The Bear's internal roles and their runtime instances do the role-specific reasoning and work.

## Relationship to Letta

Letta currently provides some underlying runtime implementations and persistence during the migration era. In the Letta-backed architecture, some Bear roles map to Letta-managed agents or Letta Code harnesses.

Den owns the Bear abstraction above Letta:

- Den knows which provider-managed runtimes belong to a Bear role.
- Den knows each role's policy, memory scope, and runtime family.
- Den provisions prompts, tools, skills, memory policy, and runtime configuration.
- Den repairs or reports drift when runtime state diverges from canonical configuration.

## Relationship to surfaces

Different surfaces reach different internal Bear roles:

| Surface | Typical role |
|---------|--------------|
| Slack, web chat, Discord | `chat` |
| IDEs and ACP clients | `pair` |
| Scheduled or approved background work | `work` |
| Webhooks, polling, queues, subscriptions | `watch` |
| Memory integration and review | `review` |

The surface changes, but the user-facing identity remains the Bear.

## Product language

Prefer:

- “your Bear” for the assistant identity,
- “Den manages Bears” for the control plane,
- “Bear roles” for `chat`, `pair`, `review`, `work`, and `watch`,
- “membership roles” or “access roles” for human permissions.

Avoid:

- “Den answered the user,” unless describing infrastructure logs,
- “a Bear is a Letta agent,”
- “the five roles are five separate assistants,”
- or “Bear roles” when the intended meaning is specifically human membership or access roles.

## Related docs

- [Bear roles](bear-roles.md)
- [Memory model](memory-model.md)
- [Tasks and autonomy](TASKS_AND_AUTONOMY.md)
- [Capabilities and skills](capabilities-and-skills.md)
- [Observations and subscriptions](observations-and-subscriptions.md)
- [Identity and membership](IDENTITY_AND_MEMBERSHIP.md)
- [Multi-role runtime architecture ADR](../architecture/adr/multi-role-runtime-architecture.md)
- [Den Bear spec](../../services/den/docs/bear-spec.md)
