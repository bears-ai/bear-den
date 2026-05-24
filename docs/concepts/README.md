# BEARS concepts glossary

BEARS is a system for creating durable AI assistants called **Bears**. A Bear feels like one assistant to humans, while **Den** operates it through different roles, channels, and work surfaces with separate memory, tools, and safety boundaries.

## Core concepts

- **Bear** — A persistent assistant identity with a charter, memory, skills, policy, users, tools, conversations, tasks, and integrations.
- **Den** — The BEARS control plane: it provisions Bears, routes sessions, enforces policy, stores canonical state, schedules work, and reconciles runtimes.
- **Charter** — The Bear's durable purpose and responsibility boundary. It is a property of the Bear, not a separate entity.
- **Bear Operating Environment** — The full durable environment Den maintains for a Bear: identity, memory, skills, policy, tools, relationships, tasks, routines, and integrations.
- **Role** — The Bear's current operating mode and responsibility boundary, such as `talk`, `pair`, `curate`, `work`, or `watch`.
- **Channel** — The concrete touchpoint through which the Bear is currently interacting or executing, such as a Slack thread, ACP session, webhook source, or task run.
- **Work surface** — The durable work context a Bear's activity is anchored to, such as a repo, local checkout, service, deployment, Mission, project, or long-running responsibility.
- **Surface anchor** — A concrete identifier or runtime binding for a work surface, such as a Git remote, checkout path, service name, deployment id, or Mission id.
- **Turn Context** — The immediate model input for one step: instructions, messages, tool descriptors, retrieved memory, policy hints, and runtime metadata.

## How a Bear works

- **`talk` role** — Chat-like conversation through web, Slack, Discord, and similar channels.
- **`pair` role** — Side-by-side work with a human in an ACP client, IDE, or other tool.
- **`curate` role** — Reflection, memory review, task review, observation review, and skill governance.
- **`work` role** — Approved background or external work within a Den-issued scope.
- **`watch` role** — Inbound events, subscriptions, polling, and structured observations.

## Memory and knowledge

- **`core/` memory** — Curated shared Bear memory that every role may use as canonical orientation.
- **Role-local memory** — Memory under `talk/`, `pair/`, `curate/`, `work/`, or `watch/`; useful local memory may stay local forever.
- **Reflection** — The auditable review and learning system, primarily backed by `curate`, that decides what becomes shared memory, indexed recall, approved work, or durable skill.
- **Letta Archives** — Derived semantic retrieval indexes over canonical sources such as `core/`, role branches, Cabinet, Den DB records, or artifacts. They are not the source of truth.
- **Cabinet** — Shared human-editable knowledge organized around People, Missions, and Knowledge.
- **Cabinet Mission** — A shared work/knowledge container that can involve zero, one, or many Bears; it is not the same as a Bear's charter.
- **Domain** — A Bear-scoped area of durable knowledge or responsibility within the Bear's charter.

## Work and autonomy

- **Workboard** — Den's live, user-visible todo/progress state for current work.
- **Plan artifact** — A durable markdown plan used for approval or future reference; it is not itself a task or Cabinet Mission.
- **Task intent** — A proposed background/external work request captured by `talk` or `pair` before approval.
- **Approved task** — A reviewed, scoped task Den may dispatch to `work`.
- **Task run** — One execution attempt for an approved task, with scoped tools, logs, and results.
- **Observation** — A structured record of an inbound event produced by `watch` for review.

## Checkout-linked work surfaces

A local checkout or workspace root may be the first durable anchor a Bear sees for ongoing work. BEARS should treat that checkout as a valid work-surface origin, while still linking it to a more portable work-surface identity from the beginning when possible.

Recommended lifecycle:

1. **Observed** — A role, usually `pair`, encounters a local checkout or workspace root in a channel.
2. **Provisional** — If no better durable identity is known yet, Den resolves or creates a provisional work surface from the observed checkout.
3. **Canonicalized** — If durable anchors are available, such as a normalized Git remote, Den links the checkout-originated surface to a canonical repo work surface.
4. **Bound** — The current channel, session, or run records its observed anchors for that work surface.
5. **Re-materialized** — Another role, such as `work`, may attach a different checkout or runtime binding to the same work surface.
6. **Merged or refined** — If later evidence shows that two provisional surfaces are the same durable work surface, Den merges or reconciles them while preserving provenance.

Key principle: plans, tasks, memory, and workboard state should attach to the **work surface id**, not only to a machine-local checkout path.

## Capabilities and identity

- **Capability** — Something a Bear can do because Den has provisioned the right tools, policy, skills, and runtime support.
- **Tool** — A concrete callable operation exposed to a role according to descriptors and policy.
- **Skill** — A durable reusable procedure or behavior, reviewed before becoming part of a Bear's configuration.
- **User** — A human identity known to Den.
- **Membership** — A user's relationship and access role for a Bear.
- **Trusted context** — Identity, membership, session, and runtime facts supplied by Den or the authenticated client rather than inferred from chat text.

Start with [Bears and Den](BEARS_AND_DEN.md), [Bear roles](BEAR_AGENT_ROLES.md), [Memory model](MEMORY_MODEL.md), and [Tasks and autonomy](TASKS_AND_AUTONOMY.md) for the shortest path through the system.
