# Bear memory and hats

**Owner:** Den memory, runtime-policy, and Bear management code owners.
**Scope:** Bear memory visibility, proposed hats, session/Job binding, and curation.
**Current as of:** 2026-09-27; evidence: `services/den/crates/den-memory/src/schema.sql`, `services/den/crates/den-memory/src/tools.rs`, `services/den/crates/den-service/src/recall/query.rs`, `services/den/crates/den-core/src/effective_policy.rs`, `services/den/migrations/20260927061319_add_bear_hats.up.sql`, and `services/den/crates/den-service/src/bears/hats/tests.rs`.
**Target:** [Hats and session memory boundaries plan](../roadmap/HATS_AND_SESSION_MEMORY_BOUNDARIES_PLAN.md) (schema and internal CRUD started; memory/session behavior remains future work).
**Decisions:** [SQLite-first canonical Bear memory](../decisions/adr-0031-sqlite-first-canonical-store-for-bear-agent-memory-and-tasks.md), [Trust profiles and governance](../decisions/adr-0039-trust-profiles-and-governance.md). The target below would revise some of their trust/memory contracts; do not treat this page as a superseding ADR.

## Current behavior

Canonical memory lives in per-Bear SQLite. `memory_records` distinguishes `profile_local` from `shared` (`core/`), with an optional work-surface reference. Keyword memory search allows shared records or records under the caller's profile; semantic recall uses a corresponding profile filter. Stance/profile is still an authority and memory input, not merely provenance. A client session is distinct from the durable conversation to which it is bound. A new `bear_hats` table, resource restrictions, and nullable conversation/Job hat references are present in the schema; a typed internal registry can create/list hats and add Bear-assigned surfaces. No user-facing routes, active hat binding, Work enablement, or hat-scoped memory are wired into turns yet. There are no hats in the current memory/recall paths. See the [memory model](../architecture/memory-model.md) and [state-machine inventory](../architecture/den-state-machine-inventory.md) for the current architecture; neither the plan nor this target section is a claim that session isolation has shipped.

## Target to validate

One Bear can have named, user-configured **hats** for a responsibility and permitted resources/actions. A hat is reusable across conversations and Work Jobs, but cannot grant tools, credentials, or resources that the existing caller, armature, and Den policy do not permit. The same hat may be used interactively and in approved background work.

The proposed memory visibility rule for an ordinary model run is:

1. Its own uncurated notes, keyed by a stable source unit: canonical conversation for interactive turns, Work run for background execution, or a stable intake unit for inbound observations. A reconnect is not a new memory scope. Raw transcript history remains a separate canonical artifact, not a memory bucket.
2. Curated knowledge for the session/run's bound hat, if authorized for that hat's uses.
3. Bear-wide curated `core/` knowledge.

Ordinary runs cannot read other sessions' uncurated notes, even if they share a hat or today's profile. There is no ordinary cross-session raw-memory search or proactive cross-session raw-memory injection. Memory curation has a privileged, auditable review path to source notes and may retain them locally, promote a reviewed result to a hat, or deliberately promote to Bear-wide `core/`. A promoted record retains source and decision provenance. Content from memory is evidence/data, never an instruction or an authorization grant.

**Simple hat promotion rule:** material promoted to a hat is available to all permitted uses of that hat, including approved outbound Work runs if the hat is enabled for them. The curator must consider this before promoting. Enabling Work for an existing hat requires review of already-promoted hat memory before effective permissions broaden; a prompt warning alone cannot revoke previously disclosed information. No additional per-record visibility tier is proposed. Work still needs a separately authorized Job and bounded tools/egress; curation is not authorization for outbound action.

Stance names may eventually disappear as an authority axis if typed policy from source, hat, verified armature, Job assignment, and governance preserves all denial invariants. Until tested, `chat`, `pair`, `curate`, `work`, and `watch` continue to describe current implementation. The [plan](../roadmap/HATS_AND_SESSION_MEMORY_BOUNDARIES_PLAN.md) requires a narrow end-to-end slice before changing this axis.

## Questions that block a production rollout, not an isolated prototype

- Is a hat Bear-owned and shared with all Bear members, or can it be private to a person? For the prototype, use one explicitly shared Bear-owned hat and test that raw notes from different users remain isolated. Do not silently promote one person's raw notes to a shared audience.
- What exact durable source id should a Work run or inbound intake use, and what happens if the originating conversation/Job is archived, deleted, or reconnected?
- How are existing profile-local notes with no verifiable conversation/run source quarantined and offered for explicit review, rather than silently promoted into a default hat?
- What change to a hat's allowed resources or Work eligibility invalidates a running authorization snapshot or requires a memory re-review? Expansion must not silently grant an existing run new access.

## Read deeper

- [Memory architecture](../architecture/memory-model.md)
- [Work surfaces and conversations](../guides/work-surfaces-and-conversations.md)
- [State-machine inventory](../architecture/den-state-machine-inventory.md)
- [Active plan and acceptance gates](../roadmap/HATS_AND_SESSION_MEMORY_BOUNDARIES_PLAN.md)
