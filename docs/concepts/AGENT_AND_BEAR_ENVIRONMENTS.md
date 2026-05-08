# Agent and Bear Environments

This document defines shared language for the environments BEARS creates around Bears and their role agents.

## Summary

- A **Bear Operating Environment** is the durable environment owned by a Bear.
- An **Agent Runtime Environment** is a role-specific projection of that Bear environment into a concrete runtime.
- An **Environment Projection** is the mapping from durable Bear state into a role-specific agent environment.
- A **Turn Context** is the serialized slice of the runtime environment presented to a model for one step.

This language helps distinguish the Bear as a durable assistant identity from the temporary role agents that act on its behalf.

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

## Agent Runtime Environment

An **Agent Runtime Environment** is the role-specific, situated environment presented to an individual role agent during a task, session, or conversation.

It includes things such as:

- the agent role, such as `talk`, `pair`, `curate`, `work`, or `watch`;
- role prompt and instructions;
- available tools;
- relevant memory projection;
- skill projection;
- session, task, or channel context;
- approval state;
- runtime constraints;
- observations and tool results.

Role agents do not receive the entire Bear Operating Environment. They receive a purpose-built projection appropriate to their job and trust boundary.

## Environment Projection

An **Environment Projection** is the process of mapping the Bear Operating Environment into an Agent Runtime Environment.

Examples:

- The `pair` projection may include ACP workspace tools, pair memory, editor session state, relevant skills, and client-mediated approvals.
- The `work` projection may include task instructions, approved web sources, non-interactive execution tools, stricter egress policy, and curated context.
- The `curate` projection may include memory review tools, skill proposals, task intents, observation summaries, and audit history.
- The `watch` projection may include subscription context, inbound payload interpretation tools, and write access to observations.

Environment projection is how one durable Bear can safely support multiple role agents without giving each role all context and all authority.

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
Role agent environment = situated projection
Turn context = concrete model input/tool slice
```

## Design discipline

The practical discipline of shaping these environments is **Agent Environment Design**.

Agent Environment Design is the intentional design of the agent operating environment: prompts, tools, memory, skills, policies, permissions, observations, and feedback loops.

This is distinct from **agent-facing product design**, which means making an external product usable by agents through APIs, MCP servers, structured documentation, or machine-readable workflows.

In BEARS:

- Agent-facing product design makes external systems easier for agents to use.
- Agent Environment Design shapes the world BEARS agents inhabit.
- Bear Environment Design shapes the durable Bear Operating Environment from which role environments are projected.

## Optional philosophical language

In cognitive terms, the role agent's runtime environment is its temporary **umwelt**: the meaningful world it can perceive and act within.

In engineering terms, prefer:

- Bear Operating Environment;
- Agent Runtime Environment;
- Environment Projection;
- Turn Context;
- Agent Environment Design.

## Example

A Bear's Operating Environment may include long-term memory, approved documentation hosts, a skill manifest, routines, users, and policy.

When the Bear is invoked through ACP as `pair`, Den projects that environment into a Pair Agent Runtime Environment containing workspace-local tools, relevant memory, ACP session state, and user approval affordances.

When the same Bear runs scheduled work as `work`, Den projects a different runtime environment containing task instructions, approved web sources, non-interactive validation tools, and stricter egress policy.

The Bear is durable. The role agent runtime is situated. The turn context is the immediate model input.

## Related docs

- [Bears and Den](BEARS_AND_DEN.md)
- [Bear agent roles](BEAR_AGENT_ROLES.md)
- [Capabilities and skills](CAPABILITIES_AND_SKILLS.md)
- [Memory model](MEMORY_MODEL.md)
- [Tasks and autonomy](TASKS_AND_AUTONOMY.md)
- [Tool naming and execution strategy ADR](../architecture/adr/tool-naming-and-execution-strategy.md)
