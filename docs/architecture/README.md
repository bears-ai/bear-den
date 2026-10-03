# Architecture

Conceptual models, stable contracts, and architecture overviews for Bear Den.

Bear Den is a **single Den-native system**:

- one in-process runtime loop owned by Den;
- direct inference through Bifrost;
- canonical Bear cognition in per-Bear SQLite;
- canonical jobs/tasks in Den Postgres through Docket;
- protocol edges such as ACP/BearWire, web chat, and future channels projecting the same core runtime.

There is no Letta, Letta Code/Codepool, or MemFS sidecar in the current architecture. Historical migration material may remain in the repo, but it is not part of the onboarding path for understanding the live system.

## Start here

Use the [topic map](../README.md) first: it distinguishes verified current behavior, intended changes, and decision rationale. For an architecture overview, read [overview](overview.md), then [Den runtime](den-runtime.md); follow their links to details as needed. This directory also contains drafts and historical design material—location under `architecture/` alone does not prove a claim is deployed.

## What this section should let you answer

Without reading the source, these docs should let you answer:

- what Den is and what a Bear is;
- how roles, channels, and armatures differ;
- where memory, tasks, approvals, and transcript state live;
- how a turn runs from user input to model output to tool execution;
- which parts of the system are protocol-neutral core and which are edge adapters;
- how reflection, planning, and autonomous work fit into the same architecture;
- and where the implementation lives in the Rust workspace.

## System at a glance

Bear Den consists of these architectural layers:

| Layer | Responsibility |
|------|----------------|
| Product model | Bears, roles, work surfaces, tasks, approvals, memory, and skills |
| Runtime core | Native turn loop, context assembly, tool orchestration, continuation, compaction, and event production |
| Persistence | Per-Bear SQLite for cognition; Den Postgres for conversations, approvals, identity, compiled configs, Docket, and schedulers |
| Tooling | Den-hosted tools, armature-local tools, external web/retrieval integrations, and sandbox execution |
| Edges | ACP/BearWire, web UI/chat, API surfaces, and future channel adapters |
| Inference | Bifrost as the unified model gateway |

## Reading paths by topic

### Runtime and execution

- [den runtime](den-runtime.md)
- [overview](overview.md)
- [den state machine inventory](den-state-machine-inventory.md)
- [den crate architecture](den-crate-architecture.md)
- [bear channel and ACP](bear-channel-and-acp.md)
- [context compilation scenarios](context-compilation-scenarios.md)
- [runtime error UX policy](runtime-error-ux-policy.md)

### Runtime state discipline

- [Den state machine inventory](den-state-machine-inventory.md) is the living reference for conversation/session/turn/run state axes, owners, transitions, and invariants.
- [workflow state overview](workflow-state-overview.md) remains the focused explanation of the canonical current-turn workflow state and derived `operational_focus`.

### Bear model and stances

- [bears and den](bears-and-den.md)
- [hats and execution context](bear-stances.md)
- [pair stance](pair-stance.md)
- [stance vocabulary](stance-vocabulary.md)

### Memory, reflection, and learning

- [memory model](memory-model.md)
- [reflection system](reflection-system.md)
- [reflection run taxonomy](reflection-run-taxonomy.md)
- [capabilities and skills](capabilities-and-skills.md)

### Tasks, planning, and autonomy

- [tasks and autonomy](tasks-and-autonomy.md)
- [planning](planning.md)
- [task schema](task-schema.md)
- [workflow state overview](workflow-state-overview.md)

### Identity, governance, and scope

- [identity and membership](identity-and-membership.md)
- [bear charter and cabinet missions](bear-charter-and-cabinet-missions.md)
- [bear environment tool contract](bear-environment-tool-contract.md)

## Contents

### Core concepts

- [bears and den](bears-and-den.md)
- [hats and execution context](bear-stances.md)
- [bear charter and cabinet missions](bear-charter-and-cabinet-missions.md)
- [identity and membership](identity-and-membership.md)
- [capabilities and skills](capabilities-and-skills.md)
- [planning](planning.md)

### Runtime and systems

- [den runtime](den-runtime.md)
- [den state machine inventory](den-state-machine-inventory.md)
- [overview](overview.md)
- [den crate architecture](den-crate-architecture.md)
- [den bear spec](den-bear-spec.md)
- [bear channel and ACP](bear-channel-and-acp.md)
- [context compilation scenarios](context-compilation-scenarios.md)
- [prompt fragment registry](prompt-fragment-registry.md)
- [den concepts overview](den-concepts-overview.md)
- [workflow state overview](workflow-state-overview.md)
- [bear environment tool contract](bear-environment-tool-contract.md)
- [pair stance](pair-stance.md)

### Memory, reflection, and work

- [memory model](memory-model.md)
- [observations and subscriptions](observations-and-subscriptions.md)
- [reflection system](reflection-system.md)
- [reflection run taxonomy](reflection-run-taxonomy.md)
- [tasks and autonomy](tasks-and-autonomy.md)
- [task schema](task-schema.md)

### Reference and terminology

- [stance vocabulary](stance-vocabulary.md)
- [interactive stances and role axes](interactive-stances-and-role-axes.md)
- [task schema](task-schema.md)
- [den prompt memory block contract](den-prompt-memory-block-contract.md)

### Historical material

These remain useful for archaeology and migration history, but they are not part of the current architecture path:

- [letta dependency matrix](letta-dependency-matrix.md)
- [den architecture](den-architecture.md)
