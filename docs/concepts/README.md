# BEARS concepts glossary

BEARS is a system for creating durable AI assistants called **Bears**. A Bear feels like one assistant to humans, while **Den** projects it into focused operating spaces with separate memory, tools, and safety boundaries.

## Core concepts

- **Bear** — A persistent assistant identity with a charter, memory, skills, policy, users, tools, conversations, tasks, and integrations.
- **Den** — The BEARS control plane: it provisions Bears, routes sessions, enforces policy, stores canonical state, schedules work, and reconciles runtimes.
- **Charter** — The Bear's durable purpose and responsibility boundary. It is a property of the Bear, not a separate entity.
- **Bear Operating Environment** — The full durable environment Den maintains for a Bear: identity, memory, skills, policy, tools, relationships, tasks, routines, and integrations.
- **Bear Space** — A focused projection of the Bear for a kind of activity: Conversation, Collaboration, Curation, Execution, or Observation Space.
- **Agent role** — The internal implementation slot backing a Space: `talk`, `pair`, `curate`, `work`, or `watch`.
- **Turn Context** — The immediate model input for one step: instructions, messages, tool descriptors, retrieved memory, policy hints, and runtime metadata.

## How a Bear works

- **Conversation Space / `talk`** — Chat-like conversation through web, Slack, Discord, and similar channels.
- **Collaboration Space / `pair`** — Side-by-side work with a human in an ACP client, IDE, or other workspace-like tool.
- **Curation Space / `curate`** — Reflection, memory review, task review, observation review, and skill governance.
- **Execution Space / `work`** — Approved background or external work within a Den-issued scope.
- **Observation Space / `watch`** — Inbound events, subscriptions, polling, and structured observations.

## Memory and knowledge

- **`core/` memory** — Curated shared Bear memory that every role may use as canonical orientation.
- **Role-local memory** — Memory under `talk/`, `pair/`, `curate/`, `work/`, or `watch/`; useful local memory may stay local forever.
- **Reflection** — The auditable review and learning system, primarily backed by `curate`, that decides what becomes shared memory, indexed recall, approved work, or durable skill.
- **Letta Archives** — Derived semantic retrieval indexes over canonical sources such as `core/`, role branches, Cabinet, Den DB records, or artifacts. They are not the source of truth.
- **Cabinet** — Shared human-editable knowledge organized around People, Missions, and Knowledge.
- **Cabinet Mission** — A shared work/knowledge container that can involve zero, one, or many Bears; it is not the same as a Bear's charter.
- **Domain** — A Bear-scoped area of durable knowledge or responsibility within the Bear's charter.

## Work and autonomy

- **Workplace** — A durable Bear-level work setting, such as a repo, service, deployment, Cabinet Mission, Docket project, or long-running responsibility, that groups plans, tasks, artifacts, memory, and activity.
- **Workboard** — Den's live, user-visible todo/progress state for current work.
- **Plan artifact** — A durable markdown plan used for approval or future reference; it is not itself a task or Cabinet Mission.
- **Task intent** — A proposed background/external work request captured by `talk` or `pair` before approval.
- **Approved task** — A reviewed, scoped task Den may dispatch to `work`.
- **Task run** — One execution attempt for an approved task, with scoped tools, logs, and results.
- **Observation** — A structured record of an inbound event produced by `watch` for review.

## Capabilities and identity

- **Capability** — Something a Bear can do because Den has provisioned the right tools, policy, skills, and runtime support.
- **Tool** — A concrete callable operation exposed to a role according to descriptors and policy.
- **Skill** — A durable reusable procedure or behavior, reviewed before becoming part of a Bear's configuration.
- **User** — A human identity known to Den.
- **Membership** — A user's relationship and access role for a Bear.
- **Trusted context** — Identity, membership, session, and runtime facts supplied by Den or the authenticated client rather than inferred from chat text.

Start with [Bears and Den](BEARS_AND_DEN.md), [Bear agent roles](BEAR_AGENT_ROLES.md), [Memory model](MEMORY_MODEL.md), and [Tasks and autonomy](TASKS_AND_AUTONOMY.md) for the shortest path through the system.
