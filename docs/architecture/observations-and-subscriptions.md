# Observations and Subscriptions

Subscriptions let a Bear receive external events. Observations are the structured records `watch` writes when those events arrive.

## Summary

- A subscription is a durable request to monitor an external source.
- Den owns subscription registration, validation, routing, and polling where needed.
- `watch` receives inbound events and writes observations.
- `curate` reviews observations before they become shared memory or tasks.
- Inbound monitoring does not directly imply outbound action.

## Subscriptions

A subscription tells Den that a Bear should monitor something.

Examples:

- a webhook from a deployment system,
- new issues in a repository,
- changes in an external service,
- queue messages,
- scheduled polling of a read-only endpoint.

Subscriptions are usually requested through `talk` or `pair` as task intents. After review, Den registers and maintains the subscription.

## Events

An event is a specific inbound payload or detected change from a subscription.

Events can come from:

- webhooks,
- polling jobs,
- message queues,
- third-party APIs,
- or other stream-like sources.

Den validates and routes events. Polling is a Den responsibility so `watch` does not need generic outbound network access.

## Observations

An observation is `watch`'s structured record of an event.

An observation should capture:

- where the event came from,
- when it happened,
- what changed,
- a summary or payload reference,
- salience or urgency,
- and any interpretation based on shared `core/` context.

Observations live in `watch/` until reviewed.

## Review by `curate`

`curate` reviews observations and decides what happens next.

Possible outcomes:

- promote durable knowledge to `core/`,
- create or recommend a derived task intent,
- connect the observation to existing work,
- or dismiss it as not durable or not actionable.

This review step prevents raw external payloads from directly controlling Bear memory or outbound action.

## Monitoring vs action

Monitoring is inbound. Action is outbound.

`watch` monitors and records. It does not post, mutate external systems, or dispatch work on its own. If an observation should cause action, that action flows through `curate`, Den policy, and the normal task path to `work`.

## Role responsibilities

| Role/System | Responsibility |
|-------------|----------------|
| `talk` | Capture user requests to monitor something. |
| `pair` | Capture client-context requests to monitor something. |
| Den | Register subscriptions, validate events, perform polling, route payloads. |
| `watch` | Interpret inbound events and write observations. |
| `curate` | Review observations and decide whether to promote, dismiss, or derive work. |
| `work` | Execute approved outbound tasks derived from observations. |

## Product language

Prefer:

- “The Bear watches this source through a subscription.”
- “`watch` records observations from inbound events.”
- “Observations are reviewed before they become memory or action.”
- “Monitoring and acting are separate flows.”

Avoid:

- “The webhook directly triggers external action.”
- “`watch` can post or mutate external systems.”
- “Every event becomes shared memory.”
- “Polling means `watch` has general internet access.”

## Related docs

- [Bear Den and Den](BEARS_AND_DEN.md)
- [Bear agent roles](BEAR_AGENT_ROLES.md)
- [Memory model](MEMORY_MODEL.md)
- [Tasks and autonomy](TASKS_AND_AUTONOMY.md)
- [Capabilities and skills](CAPABILITIES_AND_SKILLS.md)
- [Multi-agent architecture ADR](../architecture/adr/multi-agent-architecture.md)
- [Den Bear spec](../../services/den/docs/bear-spec.md)
