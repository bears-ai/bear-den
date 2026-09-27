# Bear memory and hats

**Owner:** Den memory, runtime-policy, and Bear management code owners.
**Scope:** Bear memory visibility, hats, session/Job binding, and curation.
**Current as of:** 2026-09-27; evidence: `services/den/crates/den-memory/src/schema.sql`, `services/den/crates/den-memory/src/scoped.rs`, `services/den/crates/den-service/src/bears/hats/memory_binding.rs`, `services/den/crates/den-service/src/prompt_memory_block_store.rs`, `services/den/crates/den-runtime/src/agent_loop/assembler.rs`, `services/den/src/core/tools/memory_read.rs`, and `services/den/src/core/tools/tests/prompt_memory_bound.rs` (focused tests and offline workspace library check). These are branch-code claims, not proof of a deployed release.
**Target:** [Hats and session memory boundaries plan](../roadmap/HATS_AND_SESSION_MEMORY_BOUNDARIES_PLAN.md) (user-facing hat setup, reviewed promotion, and migration remain open).
**Decisions:** [SQLite-first canonical Bear memory](../decisions/adr-0031-sqlite-first-canonical-store-for-bear-agent-memory-and-tasks.md), [Trust profiles and governance](../decisions/adr-0039-trust-profiles-and-governance.md). The target changes some of their memory contracts; this topic does not supersede those ADRs.

## Current behavior in this branch

- Per-Bear SQLite is the canonical memory store. Legacy `profile_local` notes and shared `core/` remain intact. Den Postgres holds Bear-owned hat definitions and optional conversation/Job hat bindings. A hat only restricts the Bear's existing surface grants. The Work-hat resolver denies access after eligibility or a surface grant is revoked.
- A *bound* conversation or eligible Work run gets a Den-verified source/hat scope. Bound Pair `memory_write_entry` writes source-local notes. Model-facing direct read, browse, and keyword search admit only own-source, bound-hat, and shared records, subject to access-bearing rules; a known logical path does not grant access. Bound turn-start context likewise excludes legacy profile highlights and rechecks derived recall hits against SQLite. New source/hat records are **not yet indexed** for semantic recall while member-facing admin search remains broad.
- Bound runtime prompt-block selection and model-facing prompt-block tools expose Bear-wide blocks and exact-client-session blocks, not profile-local or work-surface blocks. Bound prompt-block writes and patches are limited to that client session; Postgres refuses block-ID reassignment across Bears or scopes. `memory_status` and `session_info` report the narrower bound scope instead of profile counts/labels. Bound turn assembly discards opaque supplied runtime-context text and rebuilds its Den-owned supplement.
- **Partial cutover:** Unbound sessions still use profile-local memory. There is no ordinary hat-setup UI, reviewed hat-promotion or Work-enablement flow. Member-facing memory administration remains broad, and historical profile-local records have not been assigned source owners. Do not claim Bear-wide session isolation or completed hat curation. See the [memory model](../architecture/memory-model.md) and [state-machine inventory](../architecture/den-state-machine-inventory.md).

## Target to validate

One Bear can wear a named, user-configured **hat** for a responsibility and limited resources/actions. A hat is reusable across conversations and approved Work Jobs, but never grants credentials, tools, or authorization on its own. The intended ordinary memory read set is:

1. Its own uncurated notes, keyed by the canonical conversation, Work run, or stable intake unit—not a transient client connection. Transcript history remains separate from memory.
2. Reviewed knowledge for its bound hat.
3. Bear-wide curated `core/` knowledge.

No ordinary run reads another source's uncurated notes, even under the same hat or legacy profile. Curation has an auditable cross-source review path and may retain a note locally, promote reviewed material to a hat, or deliberately promote it to Bear-wide `core/`. Promoted content is data, never an instruction or an authorization grant.

**Simple hat promotion rule:** a hat's reviewed memory is available to all its permitted uses, including approved outbound Work if enabled. The curator must consider this when promoting; enabling Work on a populated hat requires review of existing curated memory before the permission broadens. Work still needs an authorized Job and bounded tools/egress. No extra per-record visibility tier is proposed.

Stance names may eventually be derived labels rather than authority inputs if typed source, hat, verified armature, Job, and governance policy preserves all denial invariants. Until tested, `chat`, `pair`, `curate`, `work`, and `watch` remain current implementation vocabulary; [the plan](../roadmap/HATS_AND_SESSION_MEMORY_BOUNDARIES_PLAN.md) requires an end-to-end slice before changing this axis.

## Questions before user-facing rollout

- Is a hat Bear-owned and shared with all Bear members, or can it be private to a person? An isolated test Bear can use an explicitly shared hat; do not promote a person's raw note to a shared audience silently.
- How will member-facing memory browse, search, record detail, and derived recall enforce session ownership without hiding legitimately shared hat/core knowledge?
- Which historical profile-local records have a verifiable source ID, and how will ambiguous records remain reviewable without automatic promotion?
- How does changing a hat's resources or Work eligibility invalidate active run authority and trigger review of already-curated memory?

## Read deeper

- [Memory architecture](../architecture/memory-model.md)
- [Work surfaces and conversations](../guides/work-surfaces-and-conversations.md)
- [State-machine inventory](../architecture/den-state-machine-inventory.md)
- [Active plan and acceptance gates](../roadmap/HATS_AND_SESSION_MEMORY_BOUNDARIES_PLAN.md)
