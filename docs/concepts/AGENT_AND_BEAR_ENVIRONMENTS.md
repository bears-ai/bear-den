# Agent and Bear Environments

This document defines shared language for the environments BEARS creates around Bears, roles, channels, work surfaces, and the runtime projections that let a Bear operate safely in different situations.

## Summary

- A **Bear Operating Environment** is the durable environment owned by a Bear.
- A **role runtime** is the situated runtime environment Den projects for one Bear role in a specific channel, task, or session.
- An **Environment Projection** is the mapping from durable Bear state into a role runtime.
- A **Turn Context** is the serialized slice of the runtime environment presented to a model for one step.
- An **agent role** is the Bear's current operating mode and responsibility boundary, such as `talk`, `pair`, `curate`, `work`, or `watch`.
- A **channel** is the concrete touchpoint through which the Bear is interacting or executing.
- A **work surface** is the durable work context the Bear is acting on.

This language helps distinguish the Bear as a durable assistant identity from the runtime projections and internal role-specific environments that act on its behalf. The Bear should understand and present itself as the Bear, not as a separate Space or sub-agent.

## Bear Operating Environment

The **Bear Operating Environment** is the durable, bear-level environment that defines a Bear's identity, memory, policy, skills, approved sources, relationships, routines, integrations, and role configuration.

It includes things such as:

- Bear identity and profile;
- durable semantic memory;
- role-specific memory branches;
- approved reference sources;
- skills and skill proposals;
- tool entitlements;
- policy and approval rules;
- users, membership, and relationships;
- routines and automations;
- task and observation history;
- integrations and service bindings;
- artifacts and work-surface bindings.

A Bear Operating Environment is not the same thing as a prompt or context window. It is broader, durable, and managed by Den.

## Roles, channels, and work surfaces

A Bear operates through **roles**, in **channels**, and often against one or more **work surfaces**.

| Concept | Meaning | Examples |
|---------|---------|----------|
| Role | The Bear's current operating mode and responsibility boundary. | `talk`, `pair`, `curate`, `work`, `watch` |
| Channel | The concrete touchpoint through which the Bear is interacting or executing. | web chat, Slack thread, ACP session, webhook source, task run |
| Work surface | The durable work context the Bear is acting on. | repo, local checkout, service, deployment, Mission, project |

These are not separate assistant identities. They are the structured context Den uses to project the right tools, memory view, policy, and runtime constraints.

## Role runtime

A **role runtime** is the situated runtime environment Den projects for a Bear role during a task, session, or conversation.

It includes things such as:

- the current role, such as `talk`, `pair`, `curate`, `work`, or `watch`;
- the current channel;
- the current work-surface hints or resolved work-surface anchors when relevant;
- role instructions and runtime reminders;
- available tools;
- relevant memory projection;
- skill projection;
- session, task, or run context;
- current human or actor context;
- approval state;
- runtime constraints;
- observations and tool results.

Role runtimes do not receive the entire Bear Operating Environment. They receive a purpose-built projection appropriate to their job and trust boundary.

## Environment Projection

An **Environment Projection** is the process of mapping the Bear Operating Environment into a role runtime.

Examples:

- A `pair` runtime in ACP may include workspace tools, pair memory, editor session state, work-surface hints, relevant skills, current-human context, and client-mediated approvals.
- A `work` runtime may include task instructions, approved web sources, non-interactive execution tools, stricter egress policy, curated context, and the work surface being executed against.
- A `curate` runtime may include memory review tools, skill proposals, task intents, observation summaries, and audit history.
- A `watch` runtime may include subscription context, inbound payload interpretation tools, and write access to observations.

Environment projection is how one durable Bear can safely support multiple roles and channels without giving each runtime all context and all authority.

## Turn Context

A **Turn Context** is the concrete serialized slice of a role runtime that is presented to the model for one generation or tool-use step.

It may include:

- system/developer instructions;
- conversation messages;
- retrieved memory snippets;
- tool descriptors;
- policy hints;
- current task/session metadata;
- recent observations or tool results.

Turn Context is narrower than a role runtime, which is narrower than Bear Operating Environment.

In current BEARS implementation, the stable prompt is composed by Den from these high-level parts, in order:

1. **Den baseline** — shared Bear safety and control-plane guidance. It establishes that the agent is operating as the Bear through a constrained environment, must preserve role and policy boundaries, must not claim unavailable tools or authority, should ask before destructive or externally visible actions, and should not intentionally remember secrets or credentials.
2. **Role instructions** — role-specific instructions for `talk`, `pair`, `curate`, `work`, or `watch`. Older implementation docs may still refer to these as Space-specific instructions or role contracts.
3. **User steering** — operator/user-provided steering for how this Bear should behave.
4. **Bear context** — durable Bear-specific context such as identity, purpose, preferences, and scope.
5. **Runtime/thread context** — optional situational context for a particular chat, ACP session, task run, event, or thread.

Additional runtime-specific context may be added per turn. For example, a `pair` ACP turn may include a Den-injected system reminder describing available local client tools, Den server tools, workspace roots, authenticated human identity, memory boundaries, plan-mode state, visible workboard state, work-surface hints, and tool-loop behavior.

## Relationship between the concepts

```text
Bear Operating Environment
        ↓ projected into
Role runtime
        ↓ serialized into
Turn Context
```

Or, more explicitly:

```text
Bear environment = durable world/state
Role runtime = situated projection for a role/channel/work surface
Turn context = concrete model input/tool slice
```

A practical way to read this in BEARS is:

- The **Bear Operating Environment** contains the full durable configuration and state Den knows about the Bear.
- Den projects that into a **role runtime** by selecting a role, channel, memory view, tool surface, policy boundary, work-surface hints or anchors, and session/task metadata.
- The runtime serializes the immediately relevant slice as **Turn Context** through prompt sections, conversation messages, tool descriptors, reminders, and policy hints.

Not every component appears as natural-language prompt text. Some parts are carried as API fields, tool descriptors, runtime configuration, database state, memory branch selection, or policy checks enforced by Den, Codepool, Letta, the ACP adapter, or the MemFS sidecar.

## Design discipline

The practical discipline of shaping these environments is **Agent Environment Design**.

Agent Environment Design is the intentional design of the agent operating environment: prompts, tools, memory, skills, policies, permissions, observations, and feedback loops.

This is distinct from **agent-facing product design**, which means making an external product usable by agents through APIs, MCP servers, structured documentation, or machine-readable workflows.

In BEARS:

- Agent-facing product design makes external systems easier for agents to use.
- Agent Environment Design shapes the world BEARS agents inhabit.
- Bear Environment Design shapes the durable Bear Operating Environment from which role runtimes are projected.

## Optional philosophical language

In cognitive terms, the Bear's current runtime environment is its temporary **umwelt**: the meaningful world it can perceive and act within.

In engineering terms, prefer:

- Bear Operating Environment;
- role runtime;
- role, channel, and work surface;
- Environment Projection;
- Turn Context;
- Agent Environment Design.

Avoid introducing **Space** as a primary concept in new docs. Older implementation material may still contain it, but the preferred conceptual model is role/channel/work-surface.

## Examples

A Bear's Operating Environment may include long-term memory, approved documentation hosts, a skill manifest, routines, users, and policy.

When the Bear is invoked through ACP, Den projects that environment into a `pair` runtime. The runtime environment contains workspace-local tools, relevant memory, ACP session state, authenticated-human context, work-surface hints, and user approval affordances. The turn context includes the `pair` instructions plus per-turn reminders about callable client tools, Den server tools, workspace roots, plan-mode constraints, and memory/tool boundaries.

When the Bear is invoked through web chat, Den projects that environment into a `talk` runtime. Den sends trusted Bear, user, channel, tool, and runtime-plan metadata to Codepool. Codepool runs the Letta Code harness for that role. The model still sees the stable `talk` instructions, while runtime details such as tool registration, session handling, and memory setup are partly carried outside the natural-language prompt.

When the same Bear runs scheduled work, Den projects a different runtime environment into a `work` runtime containing task instructions, approved web sources, non-interactive validation tools, approved tool scope, curated context, relevant work-surface identity, and stricter egress policy.

The Bear is durable. The runtime is situated. The turn context is the immediate model input.

## Related docs

- [Bears and Den](BEARS_AND_DEN.md)
- [Bear agent roles](BEAR_AGENT_ROLES.md)
- [Capabilities and skills](CAPABILITIES_AND_SKILLS.md)
- [Memory model](MEMORY_MODEL.md)
- [Tasks and autonomy](TASKS_AND_AUTONOMY.md)
- [Tool naming and execution strategy ADR](../architecture/adr/tool-naming-and-execution-strategy.md)
