# Memory Model

Bear memory is the durable knowledge a Bear can use across surfaces and time. Raw interactions enter role-specific memory first; durable shared knowledge is promoted into `core/` by `curate`.

## Summary

- A Bear has shared memory and role-specific memory.
- `core/` is the shared, curated memory every role can use.
- `talk/`, `pair/`, `curate/`, `work/`, and `watch/` are role-specific memory areas.
- Raw inputs should not automatically become shared truth.
- `curate` is responsible for deciding what becomes durable shared memory.

## What is Bear memory?

Bear memory is the information a Bear keeps so it can remain useful beyond one conversation or task.

Memory may include:

- durable preferences,
- project or team facts,
- recurring patterns,
- summaries of completed work,
- decisions and rationales,
- and reviewed observations from external systems.

Memory is not just chat history. Chat history, task logs, observations, and durable knowledge are different things with different trust levels.

## Shared memory: `core/`

`core/` is the Bear's shared memory.

It should contain durable knowledge that is useful across roles and surfaces. For example:

- stable user preferences,
- project conventions,
- team norms,
- approved task summaries,
- durable facts learned from reviewed work or observations.

`core/` should be curated, not treated as a dumping ground. The goal is to keep shared memory useful, compact, and trustworthy.

## Role-specific memory

Each internal Bear agent role has its own memory area:

| Area | Purpose |
|------|---------|
| `talk/` | Notes and task intents from chat-like conversations. |
| `pair/` | Notes and task intents from client-side collaboration. |
| `curate/` | Reflection notes, review state, and integration work. |
| `work/` | Task execution notes, logs, and results. |
| `watch/` | Structured observations from inbound events. |
| `core/` | Shared durable memory curated for the whole Bear. |

Role-specific memory lets each role keep useful local context without exposing every raw input to every other role.

## How memory becomes shared

The normal memory flow is:

1. `talk`, `pair`, `work`, or `watch` writes role-specific notes, results, intents, or observations.
2. `curate` reviews those branches on its cycle.
3. `curate` decides what is durable, useful, and safe to share.
4. `curate` promotes the distilled knowledge into `core/`.
5. Other roles can use the updated `core/` on future turns or runs.

This keeps shared memory deliberate rather than accidental.

## What should not be remembered

Bear memory should not store:

- secrets,
- raw credentials,
- access tokens,
- private keys,
- unnecessary personal data,
- unreviewed webhook payloads as shared truth,
- or temporary details that will not matter later.

Secrets belong in secret-management systems, not in Bear memory.

## Product language

Prefer:

- “The Bear remembers durable knowledge through curated memory.”
- “`core/` is shared memory; role branches hold local context.”
- “Raw interactions are reviewed before they become shared memory.”
- “`curate` decides what the Bear should carry forward.”

Avoid:

- “Everything the user says becomes memory.”
- “All roles see all history.”
- “Memory is just conversation history.”
- “Shared memory is automatically updated by every agent.”

## Related docs

- [Bears and Den](BEARS_AND_DEN.md)
- [Bear agent roles](BEAR_AGENT_ROLES.md)
- [Tasks and autonomy](TASKS_AND_AUTONOMY.md)
- [Observations and subscriptions](OBSERVATIONS_AND_SUBSCRIPTIONS.md)
- [Multi-agent architecture ADR](../architecture/adr/multi-agent-architecture.md)
- [Den Bear spec](../../services/den/docs/bear-spec.md)
