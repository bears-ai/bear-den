# Bear Memory Forgetting Plan

**Status:** Draft — skeleton requested on 2026-10-23; no implementation or retention schedule approved.

**Topic:** [Bear memory and hats](../topics/bear-memory-hats.md)

**Related:** [User-associated memory](USER_ASSOCIATED_BEAR_MEMORY_PLAN.md#approved-v1-policy-amendment-2026-10-23), [memory surfaces](MEMORY_SURFACES_IMPROVEMENT_PLAN.md#5-forgetting-and-correction), [canonical memory decision](../decisions/adr-0031-sqlite-first-canonical-store-for-bear-agent-memory-and-tasks.md).

## Goal

Design forgetting as a broader Bear-memory capability: deliberately stop reusing, age, archive or erase selected information while preserving identity, audience boundaries, useful memory and truthful user expectations. This is not simply a timer on personal-name candidates.

This skeleton records future design work, not deployed behavior, a deletion guarantee, permission for autonomous deletion, or a commitment to every potential feature below.

## Boundary with the current personal-memory plan

| Operation | Owning work |
| --- | --- |
| Explicit owner-requested deletion of personal candidate/fact bodies and protected derived copies, with content-free audit evidence | **Remains required by the active user-associated memory plan**; not deferred here. |
| Retraction/opt-out and correction/replay invalidation for personal memory | Active user-associated memory plan; retraction alone is not erasure. |
| Original human conversation history | Separate from deleting a personal fact: preserved under its own lifecycle, but cannot silently reintroduce the removed fact into model replay. Broader history-deletion design belongs here. |
| Automatic expiry, inactivity/relevance-based forgetting, general memory aging and configurable retention schedules | This future plan; **no durations or automatic jobs are chosen**. |
| Old backups, downloaded bundles and provider-held copies | Separate retention/restore/external-policy work. Local deletion cannot recall copies outside Den's control. |

Explicit personal erasure must be implemented and tested without waiting for a general automatic-forgetting engine. Conversely, this draft does not authorize silently expiring existing records or modifying current retention defaults.

## Potential scope to investigate

- User-selected forgetting of a fact, topic, conversation or other bounded source, with confirmation where selection is ambiguous.
- Distinct typed operations for stop-reusing, correction/supersession, archive, logical erasure and storage reclamation; avoid a single misleading “forget” action.
- Optional retention classes or time-based policies for temporary, inactive, rejected, superseded and durable material.
- Optional relevance/usage-based review suggestions, with human override before any autonomous forgetting policy is enabled.
- Canonical memory plus dependent transcript replay, summaries, reflection/harvest outputs, caches, retrieval indexes, artifacts and delayed/in-flight work.
- Backup/export/restore handling that does not silently resurrect information deleted after an older snapshot was taken.

These are possibilities, not approved v1 personal-memory features or a settled implementation scope.

## Invariants

- Keep one canonical owner for state; reuse the existing memory/lifecycle/dependency systems rather than create a parallel memory store or deletion authority.
- Authorization remains Bear- and audience-bound. Bear Admin access is not an ordinary override for another person's personal bodies.
- Hats organize context and clean-core maintenance, not confidentiality. Crossing hats is not itself a reason to forget or a privacy leak.
- Resolve targets through typed identifiers and dependency/provenance relationships, not name-substring matching or model-supplied identity authority.
- Distinguish retained human evidence from model-reusable context. Old summaries, tool results, sources and imports must not bypass removal.
- Keep content out of deletion receipts/audit records; retain only the minimum references needed for recovery, protection and non-resurrection.
- Do not claim forensic disk erasure, backup recall or provider deletion from a row update, tombstone, cache TTL or successful local cleanup alone.

## Proposed workstreams

1. **Inventory and contract:** map canonical/derived stores and existing lifecycle behavior; define operations, authorization, removal boundaries and what “complete” means for each surface.
2. **Policy design:** choose which categories may age or expire, if any; review schedules, user/Bear controls, audit requirements and exceptions before implementing timers. No numerical defaults are implied here.
3. **Dependency-safe execution:** plan invalidation/deletion ordering, in-flight effects, idempotent receipts, retryable cleanup, crash recovery and content-free observability using existing runtime/storage ownership.
4. **Human controls:** scoped preview, ambiguous-target selection, consequence disclosure and overrides. Specify separately which archive operations can be undone and which erasures cannot.
5. **Portability and external copies:** establish restore reconciliation, source-offline migration, backup expiry and provider/gateway limitations without rewriting portable person identity or widening audiences.
6. **Validation and staged rollout:** dry-run/suggest-only operation first where useful; test isolation, correction, repeated requests, dependency cleanup, crashes and old-snapshot restoration before destructive automation.

## Open decisions

- Which operations and memory surfaces form the first deliverable beyond explicit personal-memory deletion?
- Which policy choices belong to the human owner, Bear administrators or the Den operator?
- What retention schedules, review thresholds and audit lifetimes are appropriate? None are selected yet.
- How should mixed-source derived artifacts be handled without exposing or discarding another person's material?
- What evidence is required before reporting logical erasure, derived cleanup or physical reclamation complete?
- What anti-resurrection metadata and disclosures are needed when restoring an independently held old backup?

## Exit criteria for turning this skeleton into an active plan

- [ ] Agree scope and distinguish mandatory explicit deletion from optional automatic forgetting.
- [ ] Document a store/dependency inventory and current-versus-target behavior.
- [ ] Approve authorization, retention and recovery/irreversibility semantics.
- [ ] Define tested milestones and deletion/cleanup acceptance criteria, including external limitations.
- [ ] Link any resulting implementation decision and rollout/incident procedures from the maintained memory topic.

No schema, worker, API, tool, dependency, TTL or deployment change is introduced by this draft. Tests should reuse production APIs and shared fixtures rather than add per-case test-only SQL.
