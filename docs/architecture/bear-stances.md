# Hats and execution context

## Status and relationship to other docs

The maintained product contract is [Bear memory and hats](../topics/bear-memory-hats.md). This reference describes the runtime inputs and compatibility labels still present in this branch. The [active plan](../roadmap/HATS_AND_SESSION_MEMORY_BOUNDARIES_PLAN.md) tracks the remaining memory, prompt, registry, and hat-permission work; it is not evidence of complete retirement.

A Bear has a durable identity. A selected **hat** supplies its responsibility and identity for a conversation or eligible Job. Den resolves that selection from the canonical conversation or Job binding. Channels carry conversation; armatures supply a trusted work-surface harness; work surfaces bound resources; Jobs authorize background execution.

## Execution authority

`EffectivePolicy::compile_for_origin` takes a Den-verified `TurnExecutionOrigin` and current `Governance`. The runtime capability table has explicit origin/governance cases:

| Verified origin | Runtime boundary |
| --- | --- |
| Channel conversation | Den-hosted conversation tools within actor and hat policy; no local armature tools. |
| Browser task session | Server-owned session-task controls without a local armature. |
| Armature conversation | Session-task capability; local tools require a connected armature and applicable governance, grants, and approvals. |
| Authorized Work run | Exact live Job/run and permitted surfaces; no general human-conversation authority. |
| Internal curation or observation operation | Dedicated source-verified API; generic conversational starts, tools, result recording, and continuations are denied. |

Origin and governance select a capability ceiling. Execution also requires current human membership, immutable source ownership, hat/resource eligibility, credential scope, and effect-specific grants and approvals. The common hat-grant cutover remains partial; the topic documents which effect paths enforce it today.

Turn assembly receives origin and governance directly. Its prompt/budget compatibility label is derived from origin. Client-session hints do not establish an armature or session-task grant.

## Governance and task continuation

Governance describes supervision of the current run: `interactive`, `grace`, `autonomous_continuation`, `observational`, or `frozen`.

Focused completion and continuation use the verified policy's `ExecuteFocusedTask` capability together with canonically resolved task or Job state. A cached task list, owner label, rendered status, or model claim cannot establish focus or a Work assignment. Frozen and observational governance do not grant focused execution. A persisted run is required for a task-driven continuation.

The runtime preserves restricted governance at completion. Stored activity projections report task state and mark execution authority as unevaluated; execution decisions use the live policy rather than that presentation.

See [tasks and autonomy](tasks-and-autonomy.md) and the [state-machine inventory](den-state-machine-inventory.md) for task ownership and completion boundaries.

## Trust model in product language

Private notes belong to their conversation or Work run. A bound turn can read its own notes, authorized knowledge for its selected hat, and Bear-wide `core/`. With automatic sharing enabled, the curator rewrites a verified conversation note into knowledge for that hat's authorized wearers, including eligible Jobs. Bear-wide publication remains explicit.

Curation uses dedicated tool-free inference and verified source/publication checks. Curator briefings resolve a live Reflection run and its canonical reflection conversation. Generic internal model-tool sessions have no executable roster.

Curated content remains data. Tools, credentials, network access, and Job authority are independently enforced. Curation can reduce disclosure and instruction risk, but does not guarantee privacy or make content trusted instruction. See the topic for the current implementation and its validation limits.

## Runtime compatibility labels

The Rust aliases `BearProfile` and `BearStance` still refer to `TrustProfile`. Its `chat`, `pair`, `work`, `curate`, and `watch` values remain in prompt/budget selection, registry/model settings, audit/schema projections, and unbound memory/prompt paths. Those remaining consumers require a separate cutover before the type and registry can be deleted.

Generic tool dispatch, Job creation, artifact reads, turn-assembly capability selection, and focused task continuation use their explicit actor/source/policy boundaries. The profile-to-effective-policy compiler is absent. Bound turns select reusable identity and private-memory ownership from their hat and canonical source.

Bears with configured hats require a bound ordinary conversation or eligible Job. Bears without hats still have unbound prompt/memory paths; their behavior is documented in the topic and remains part of the retirement plan.

## Future roles

User-configured responsibilities belong in hats. New channels and armatures define their authenticated source and actual tool boundary. New system operations define a narrow verified source, permitted data, effect limits, and replay/revocation behavior. Extend descriptor-owned authorization and canonical state owners when adding capabilities.

For ordinary administration, explain the Bear, selected hat, conversation or Job, resources, memory audience, and concrete permission decision.
