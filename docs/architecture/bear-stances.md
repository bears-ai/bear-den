# Hats and execution context

## Status and relationship to other docs

The maintained product contract is [Bear memory and hats](../topics/bear-memory-hats.md). This reference describes current working-tree WIP, not a rebuilt/shipped image. The [active plan](../roadmap/HATS_AND_SESSION_MEMORY_BOUNDARIES_PLAN.md) tracks the partial hat-permission resolver, ownership audits, and rollout evidence.

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

Turn assembly receives origin and governance directly. Defaults, model/control policy, and compaction use the verified origin and canonical source; derived labels and historical profile model/loop rows cannot override them. Client-session hints do not establish an armature or session-task grant.

## Governance and task continuation

Governance describes supervision of the current run: `interactive`, `grace`, `autonomous_continuation`, `observational`, or `frozen`.

Focused completion and continuation use the verified policy's `ExecuteFocusedTask` capability together with canonically resolved task or Job state. A cached task list, owner label, rendered status, or model claim cannot establish focus or a Work assignment. Frozen and observational governance do not grant focused execution. A persisted run is required for a task-driven continuation.

The runtime preserves restricted governance at completion. Stored activity projections report task state and mark execution authority as unevaluated; execution decisions use the live policy rather than that presentation.

See [tasks and autonomy](tasks-and-autonomy.md) and the [state-machine inventory](den-state-machine-inventory.md) for task ownership and completion boundaries.

## Trust model in product language

Private notes belong to their conversation or Work run. A bound turn can read its own notes, authorized knowledge for its selected hat, and Bear-wide `core/`. With automatic sharing enabled, the curator rewrites a verified conversation note into knowledge for that hat's authorized wearers, including eligible Jobs. Bear-wide publication remains explicit.

Curation uses dedicated tool-free inference and verified source/publication checks. Curator briefings directly resolve and recheck a live Reflection run and its canonical reflection conversation before inference and delivery, with instructions from repository Markdown and no tools/checkpoint tools. Worker briefings consume direct verified `MemorySource`, not role prompts/contracts. Source admission precedes every production inference/continuation; direct tools recheck it. Generic internal starts, tools, results, and continuations are denied.

Recall indexes only eligible current core/hat records, never source/profile-local records even with zero hats. Stale legacy derived-point cleanup is retried before embedding; canonical historical SQLite is preserved, and profile-string recall APIs are removed.

Curated content remains data. Tools, credentials, network access, and Job authority are independently enforced. Curation can reduce disclosure and instruction risk, but does not guarantee privacy or make content trusted instruction. See the topic for the current implementation and its validation limits.

## Derived runtime-context metadata

`TrustProfile`, `BearProfile`, and `BearStance` and their named aliases are removed; `RuntimeContextLabel` is derived runtime-context metadata. Five source-kind labels describe channel conversation, armature conversation, Job run, curation operation, and observation operation. Historical `chat/pair/work/curate/watch` encodings remain schema/audit projections, not configurable stances. Browser-task origin is distinct even though its metadata uses the historical `pair` encoding; labels cannot reconstruct origin.

Admin stance detail/configuration/provisioning, per-profile model, and profile-registration routes are gone. `/models` is Bear-wide; native initialization never creates/refreshes a profile registry. Historical model/loop rows are not read as live overrides. Managed compilation emits bound Bear-wide base and platform modes only, independently of legacy contracts/metadata; canonical hat identity is selected at turn time. No production inference selects a role prompt or role contract. Old Pair Plan and shared scaffold model tools are retired.

Every ordinary conversation, Job, and run needs a real Bear-owned hat, **even with zero hats**. Inference, result recording, continuations, and direct dispatch recheck the live actor/source/hat and exact Work run. Pending IDE sessions create durable `den-conv-*` only after admission. Old unbound history remains owner/admin read-only, NULL-owner history admin-only. No hat, import, ownership, or promotion is fabricated; no ordinary Legacy memory/network/Cargo fallback remains.

This source-admission cutover is not a complete `HatGrantResolver`. Den action/resource filtering is still partial for web, supported editor filesystem reads, and sandbox egress. Broader Den effects/catalogs and other bounded client `Always for [hat]` choices are next under Gate 0A; focused WIP tests are not latest-image shipment evidence.

## Future roles

User-configured responsibilities belong in hats. New channels and armatures define their authenticated source and actual tool boundary. New system operations define a narrow verified source, permitted data, effect limits, and replay/revocation behavior. Extend descriptor-owned authorization and canonical state owners when adding capabilities.

For ordinary administration, explain the Bear, selected hat, conversation or Job, resources, memory audience, and concrete permission decision.
