# Agent and Bear Environments

This document defines shared language for the environments BEARS creates around Bears, Bear Spaces, and the internal agent roles that back them.

## Summary

- A **Bear Operating Environment** is the durable environment owned by a Bear.
- A **Bear Space** is a Bear-facing projected operating environment, such as Conversation Space or Collaboration Space.
- An **Agent Runtime Environment** is the concrete runtime environment that backs a Space.
- An **Environment Projection** is the mapping from durable Bear state into a Space and its runtime environment.
- A **Turn Context** is the serialized slice of the runtime environment presented to a model for one step.
- An **agent role** is an internal implementation slot that currently backs a Space. Agent-role language should generally be hidden from the Bear and minimized in ordinary user-facing surfaces.

This language helps distinguish the Bear as a durable assistant identity from the projected Spaces and internal role agents that act on its behalf. The Bear should understand and present itself as the Bear, not as a role agent.

## Bear Operating Environment

The **Bear Operating Environment** is the durable, bear-level environment that defines a Bear's identity, memory, policy, skills, approved sources, relationships, routines, integrations, and role-agent configuration.

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
- artifacts and workspace/project bindings.

A Bear Operating Environment is not the same thing as a prompt or context window. It is broader, durable, and managed by Den.

## Bear Spaces

A **Bear Space** is a projected operating environment for a Bear. A Space determines the Bear's available tools, memory view, runtime family, policy boundary, channel or task shape, and interaction expectations.

Spaces are not separate assistant identities. When a Bear operates in a Space, it should still understand and present itself as the Bear. The Space is the environment the Bear is operating through, not who the Bear is.

Canonical Spaces:

| Internal agent role | Bear-facing Space | Purpose |
|---------------------|-------------------|---------|
| `talk` | **Conversation Space** | Direct synchronous conversation through chat-like surfaces. |
| `pair` | **Collaboration Space** | Working alongside the current human in a client, IDE, or workspace. |
| `curate` | **Curation Space** | Reviewing, organizing, and promoting durable memory, task intents, observations, results, and skill proposals. |
| `work` | **Execution Space** | Carrying out approved background or external work within a defined scope. |
| `watch` | **Observation Space** | Receiving inbound events and recording structured observations. |

Agent roles remain useful implementation vocabulary for code, schemas, routing, provisioning, diagnostics, and architecture docs. In prompts visible to the Bear, and in ordinary user-facing language, prefer Space language and avoid encouraging the Bear to identify as a role, sub-agent, backend component, or separate assistant.

## Agent Runtime Environment

An **Agent Runtime Environment** is the situated runtime environment backing a Bear Space during a task, session, or conversation.

It includes things such as:

- the current Space, such as Conversation Space or Collaboration Space;
- the internal agent role that backs that Space, such as `talk`, `pair`, `curate`, `work`, or `watch`;
- Space prompt and instructions;
- available tools;
- relevant memory projection;
- skill projection;
- session, task, or channel context;
- current human or actor context;
- approval state;
- runtime constraints;
- observations and tool results.

Space-backed role agents do not receive the entire Bear Operating Environment. They receive a purpose-built projection appropriate to their job and trust boundary.

## Environment Projection

An **Environment Projection** is the process of mapping the Bear Operating Environment into a Bear Space and the Agent Runtime Environment that backs it.

Examples:

- The Collaboration Space projection, currently backed by `pair`, may include ACP workspace tools, pair memory, editor session state, relevant skills, current-human context, and client-mediated approvals.
- The Execution Space projection, currently backed by `work`, may include task instructions, approved web sources, non-interactive execution tools, stricter egress policy, and curated context.
- The Curation Space projection, currently backed by `curate`, may include memory review tools, skill proposals, task intents, observation summaries, and audit history.
- The Observation Space projection, currently backed by `watch`, may include subscription context, inbound payload interpretation tools, and write access to observations.

Environment projection is how one durable Bear can safely support multiple Spaces without giving each runtime all context and all authority.

## Turn Context

A **Turn Context** is the concrete serialized slice of an Agent Runtime Environment that is presented to the model for one generation or tool-use step.

It may include:

- system/developer instructions;
- conversation messages;
- retrieved memory snippets;
- tool descriptors;
- policy hints;
- current task/session metadata;
- recent observations or tool results.

Turn Context is narrower than Agent Runtime Environment, which is narrower than Bear Operating Environment.

In current BEARS implementation, the stable prompt is composed by Den from these high-level parts, in order:

1. **Den baseline** — shared Bear safety and control-plane guidance. It establishes that the agent is operating as the Bear through a constrained environment, must preserve Space boundaries, must not claim unavailable tools or authority, should ask before destructive or externally visible actions, and should not intentionally remember secrets or credentials.
2. **Space instructions** — Space-specific instructions for Conversation, Collaboration, Curation, Execution, or Observation Space. Implementation docs may still call these role contracts.
3. **User steering** — operator/user-provided steering for how this Bear should behave.
4. **Bear context** — durable Bear-specific context such as identity, purpose, preferences, and scope.
5. **Runtime/thread context** — optional situational context for a particular chat, ACP session, task run, event, or thread.

Additional runtime-specific context may be added per turn. For example, a Collaboration Space ACP turn may include a Den-injected system reminder describing available local client tools, Den server tools, workspace roots, authenticated human identity, memory boundaries, plan-mode state, visible workboard state, and tool-loop behavior.

## Relationship between the concepts

```text
Bear Operating Environment
        ↓ projected into
Agent Runtime Environment
        ↓ serialized into
Turn Context
```

Or, more explicitly:

```text
Bear environment = durable world/state
Bear Space / runtime environment = situated projection
Turn context = concrete model input/tool slice
```

A practical way to read this in BEARS is:

- The **Bear Operating Environment** contains the full durable configuration and state Den knows about the Bear.
- Den projects that into a Space-specific **Agent Runtime Environment** by selecting a Space, internal agent role, runtime family, memory view, tool surface, policy boundary, and session/task/channel metadata.
- The runtime serializes the immediately relevant slice as **Turn Context** through prompt sections, conversation messages, tool descriptors, reminders, and policy hints.

Not every component appears as natural-language prompt text. Some parts are carried as API fields, tool descriptors, runtime configuration, database state, memory branch selection, or policy checks enforced by Den, Codepool, Letta, the ACP adapter, or the MemFS sidecar.

## Design discipline

The practical discipline of shaping these environments is **Agent Environment Design**.

Agent Environment Design is the intentional design of the agent operating environment: prompts, tools, memory, skills, policies, permissions, observations, and feedback loops.

This is distinct from **agent-facing product design**, which means making an external product usable by agents through APIs, MCP servers, structured documentation, or machine-readable workflows.

In BEARS:

- Agent-facing product design makes external systems easier for agents to use.
- Agent Environment Design shapes the world BEARS agents inhabit.
- Bear Environment Design shapes the durable Bear Operating Environment from which role environments are projected.

## Optional philosophical language

In cognitive terms, the Bear's current Space/runtime environment is its temporary **umwelt**: the meaningful world it can perceive and act within.

In engineering terms, prefer:

- Bear Operating Environment;
- Bear Space;
- Agent Runtime Environment;
- Environment Projection;
- Turn Context;
- Agent Environment Design.

## Examples

A Bear's Operating Environment may include long-term memory, approved documentation hosts, a skill manifest, routines, users, and policy.

When the Bear is invoked through ACP, Den projects that environment into Collaboration Space, currently backed by the `pair` agent role. The runtime environment contains workspace-local tools, relevant memory, ACP session state, authenticated-human context, and user approval affordances. The turn context includes the Collaboration Space instructions plus per-turn reminders about callable client tools, Den server tools, workspace roots, plan-mode constraints, and memory/tool boundaries.

When the Bear is invoked through web chat, Den projects that environment into Conversation Space, currently backed by the `talk` agent role. Den sends trusted Bear, user, channel, tool, and runtime-plan metadata to Codepool. Codepool runs the Letta Code harness for that Space's backing role agent. The model still sees the stable Conversation Space prompt, while runtime details such as tool registration, session handling, and memory setup are partly carried outside the natural-language prompt.

When the same Bear runs scheduled work, Den projects a different runtime environment into Execution Space, currently backed by `work`, containing task instructions, approved web sources, non-interactive validation tools, approved tool scope, curated context, and stricter egress policy.

The Bear is durable. The Space/runtime is situated. The turn context is the immediate model input.

## Related docs

- [Bears and Den](BEARS_AND_DEN.md)
- [Bear agent roles](BEAR_AGENT_ROLES.md)
- [Capabilities and skills](CAPABILITIES_AND_SKILLS.md)
- [Memory model](MEMORY_MODEL.md)
- [Tasks and autonomy](TASKS_AND_AUTONOMY.md)
- [Tool naming and execution strategy ADR](../architecture/adr/tool-naming-and-execution-strategy.md)
