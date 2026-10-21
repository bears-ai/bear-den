# Bear package format (portability)

How to **export**, **transport**, and **import** a Bear's **cognition and configuration** without dragging Den control-plane state along. A portable package is what you move when cloning a Bear to another host, backing up durable knowledge, or sharing an approved skill/policy bundle — not a full Den snapshot.

**Related:** [ADR-0031 — SQLite-first canonical store](../decisions/adr-0031-sqlite-first-canonical-store-for-bear-agent-memory-and-tasks.md), [`den-runtime.md`](../architecture/den-runtime.md) (storage boundary), [`memory-model.md`](../architecture/memory-model.md), [`bear-memory.md`](bear-memory.md), [`work-surfaces-and-conversations.md`](work-surfaces-and-conversations.md)

## Current browser bundle (version 3; branch implementation)

The implemented `.bear` ZIP contains `bear.yaml` and `memory.sqlite`, not the fuller target layout below. Version 3 adds named model configurations (model handle and optional reasoning effort), the Bear default configuration reference and optional hat configuration references. Import validates every model/effort against the destination catalog before Bear creation, remints configuration IDs and remaps the references. Invalid choices are rejected rather than silently substituted; blank historical raw defaults mean inheritance. Versions 1 and 2 remain readable. Version 2 introduced hat identity/purpose/directory summaries and IDE preference, secretless resource/web grant **intent**, and approved instruction-only procedure versions with integrity hashes and applicability. Import mints new hat IDs and remaps canonical hat-memory/proposal scope columns, requires audience acknowledgement, restores Work and automatic sharing **off**, and never restores tools, credentials or live surface grants. Procedures are staged disabled for review; inherited entity bindings/handles are quarantined for fresh resolution. An immutable, admin-only reconnection receipt is shown on Backup & move. Invalid setup is compensated by removing the newly created Bear.

**Review-before-import (code/test only, not deployed):** uploading a bundle stages a private copy and creates no Bear. The authenticated review page shows identity, prompts, hats, procedure versions, destination-model compatibility and knowledge audience before a separate consent/confirmation step. It does not inflate or display the full SQLite snapshot; review imported memory through the new Bear's web Memory/hat screens before granting members or enabling Work/sharing. The copy expires after 15 minutes, belongs to the uploader's exact signed-in session and is atomically consumed once. Cancellation, expiry, replay and hash mismatch cannot create a Bear; compatibility is rechecked on confirmation. Startup/periodic cleanup is bounded and protects active uploads/reads/claims. Creation failures report rollback versus an incomplete/uncertain Bear, preserve any surviving Bear's memory and require a fresh review rather than automatic retry.

**Authenticated-user identity foundation (2026-10-21, code/test only; not deployed):** before any initialization or migration writes, import inspects SQLite read-only and rejects triggers, foreign keys (including cascades), virtual/shadow tables, and views other than the canonical `memory_links`. Custom executable schema is not supported. Imported `authenticated_user_entities` rows retain a quarantined user-identity classification; their persons are demoted, canonical refs cleared, and handles detached atomically. Source-host numeric user IDs never grant destination access, even when an importing account has the same ID. A fresh authenticated destination binding alone does not regain imported personal audiences; the dedicated human review below must explicitly reconnect the selected portable person.

**Portable identity reconnection (2026-10-21, code/storage/HTTP tests verified; live acceptance and broader gates pending, not deployed):** [Phase 6A of the memory plan](../roadmap/USER_ASSOCIATED_BEAR_MEMORY_PLAN.md#phase-6a--portable-identity-reconnection) now provides a dedicated human-only workflow to select imported Alice and connect her unchanged portable person ID to a verified destination Bear member. Imported Bob stays distinct and unmapped until separately authorized. Export records source-qualified username/display labels, account ID, host, source Bear, and exporter association in SQLite `user_identity_origins` before snapshotting; export never allocates a person. Source metadata aids selection but grants no access. Reconnection requires neither an online source Den nor source credentials or transcripts. It is not a model API; generic entity/handle reassignment, merge, and split remain prohibited.

Conversations, Docket work, Cabinet pages, memberships, host credentials, raw secrets and derived indexes remain outside the bundle. Canonical SQLite includes private/source-local memory: this is a private backup, not a shared-knowledge export. Explicit export authority determines which cognition may travel, independently of ordinary per-user recall. A full authorized backup may include Alice and Bob without identifying both as its importer; selectively shared exports and their content policy/filtering are not implemented. Export snapshots current memory; **there is no pre-export curation flush yet**. The rest of this guide describes the fuller portability target, not deployed support for every tier or artifact type.

### Reconnect imported identities

The branch's management flow is **Backup & move → Imported identities**, `/bear/{slug}/portability/identities`:

1. Sign in as a **current verified Bear Admin**. Select the portable person using the source-qualified labels and provenance, then explicitly choose a **current verified member of this Bear** as the destination account. An ordinary member, matching source account ID/name, bundle possession, or historical receipt is not reconnection authority.
2. Prepare and read the separate review, including its audience consequences. Preparing/viewing does not change a mapping or reveal memory content. The review uses actor/session/Bear-bound nonces, expires after 15 minutes, and keeps the typed expected identity/binding snapshot server-side rather than accepting browser-supplied authority or versions.
3. If the destination already has a person, separately acknowledge **parking** it. This works for both fresh auto-created and populated identities and preserves **all records, relations, candidate bodies, and provenance**; Pending personal candidates become Quarantined, not deleted. Exactly one person remains active per Bear/account. Parking is not a merge or automatic joint recall; the parked person's audience closes until it is explicitly reconnected through another review.
4. Confirm the exact mapping. The server rechecks current actor/target eligibility, holding Postgres User/email/Bear locks through the atomic SQLite binding/entity/handle/generation/receipt commit. Selected/displaced source origins and display labels are compared with the server-held snapshot inside that SQLite transaction. Memory records, their lifecycle, IDs, and audience targets are never rewritten. If the review is stale, expired, or already committed, start a fresh review rather than retrying blindly.
5. If the response was lost, refresh the management receipt-history view and current mappings before another decision. Local receipts take recovery priority over source history; imported receipt history is informational, never a destination grant. Optional history-load failures do not block the mapping UI. To correct a mapping, review and confirm its revocation first, then prepare a separate reconnection. Revocation advances the authority generation, so an old context and capture consent stay invalid even if the same Bear/account/person tuple is restored. Capture after reconnection requires fresh explicit consent; reconnection does not reactivate historical candidates.

The reconnection slice adds SQLite `user_identity_generations`, `user_identity_origins`, and `user_identity_receipts` alongside existing identity classification: **no Postgres migration or dependency change, and no bundle version bump from 3**. Reconnection code/storage/HTTP tests are verified; see the [maintained topic's checkpoint](../topics/bear-memory-hats.md#portable-identity-reconnection-phase-6a) for that milestone's validation. Live acceptance and broader gates remain pending: no Docker image build, restart/deployment, live browser/provider/model test, actual two-host live migration acceptance, or serving-Postgres migration is claimed. The server-only candidate follow-on below is not end-user intake or promotion. Audience-preserving derivation, curation output protection, replay-aware removal, and ordinary personal recall/UI remain blocked; reconnection alone does not satisfy the plan's end-to-end exit.

### Personal candidates in private backups (server-only foundation)

The [Phase 3 storage/provenance foundation](../roadmap/USER_ASSOCIATED_BEAR_MEMORY_PLAN.md#phase-3--personal-fact-candidate-surface) adds only SQLite `personal_memory_candidates` and `personal_memory_settings` plus indexes; there are no Cargo/dependency changes or Postgres migrations, and the browser bundle remains **v3**. A full authorized private snapshot carries these protected bodies and settings, not a safely shareable export. Bodies never enter generic proposals/observations/records or their unfiltered inspection/curation queues. No model tool, personal-body UI or runtime/HTTP route is registered; publication and ordinary personal recall remain disabled.

Import rebases only the owning `bear_id` in these two tables. Portable person/candidate IDs and source Bear/user/generation/conversation/note provenance remain unchanged and informational, not destination grants. Pending becomes Quarantined; Withdrawn/Quarantined history is not resurrected. In the authority-quarantine transaction, imported capture settings reset **disabled**. Local parking also quarantines Pending. Even after authorized reconnection to the same tuple, a fresh current binding and fresh explicit epoch-bound capture consent are required. The server-only setting has no human UI yet; opting out atomically withdraws Pending and Quarantined candidates, and reenabling does not restore them.

Evidence: [owner-key import rebasing](../../services/den/crates/den-web/src/bear/settings.rs), [import/consent quarantine](../../services/den/crates/den-memory/src/personal_candidates/settings.rs), and [memory import/reconnection regressions](../../services/den/crates/den-memory/src/personal_candidates/tests/reconnection.rs). The [real web-bundle regression](../../services/den/crates/den-web/src/bear/settings/identity_reconnection/tests/personal_candidates.rs) passes, verifying preserved provenance, disabled imported capture, and quarantined candidates with owner-only access after reconnection. See the [topic's candidate checkpoint](../topics/bear-memory-hats.md#server-only-personal-candidates-phase-3-foundation) for storage/service/web, lint, and SQLx verification; this is code/test evidence, not deployment or live-provider acceptance. `Durable` means candidate intention, not completed legal retention/deletion policy; publication and deletion/replay policy remain future work.

## What a portable Bear is (and is not)

| Concern | In the package? | Where it lives at runtime |
|---------|-----------------|---------------------------|
| Role-local and shared memory, promotions, proposals, observations | Yes | per-Bear **`memory.sqlite`** |
| Identity, prompts, role profiles, capability config | Yes | **`manifest.yaml`** (+ optional **`artifacts/`**) |
| Work-surface knowledge (anchors, overviews in `core/`) | Yes | **`memory.sqlite`** (logical-path projection) |
| Approved skills and exported policies | Yes | **`manifest.yaml`** references + **`artifacts/`** content |
| Durable prompt-memory blocks | Yes (operator tier) | SQLite and/or manifest policy fields |
| Bear-scoped web source policy, watch subscriptions | Yes (operator tier) | manifest config (not secrets) |
| Conversations, tool audit, transcripts | **No** | Den Postgres |
| Docket jobs/tasks/runs, live workboard plans | **No** | Den Postgres |
| Membership (`user_bear`), users | **No** | Den Postgres |
| Approvals in flight, ACP session state | **No** | Den Postgres / channel adapters |
| Host-local runtime binding ids (`den-native:{id}:chat`, …) | **No** | re-provision on import |
| Qdrant / derived recall vectors | **No** | rebuild from `memory.sqlite` on import ([ADR-0038](../decisions/adr-0038-platform-embedding-standard-and-derived-recall-index.md)) |
| Secrets (OAuth, API keys, webhook signing) | **No** | host secret store |

The boundary matches [den-runtime storage](../architecture/den-runtime.md#storage-boundary-bear-cognition-vs-den-control-plane): **Bear cognition → per-Bear SQLite**; **control plane → Den Postgres**. A package is the portable half of that split plus human-authored artifacts.

### Continuity across a move: pre-export curation flush

Transcripts stay Postgres-only (table above), so a package carries what the Bear *remembers*, not the log of how a conversation went. That is the intended model — a Bear is its curated memory, not its transcript — but it creates one timing hazard: salient context from a recent session that the async curation/harvest lane ([ADR-0041 — Archival recall and async curation](../decisions/adr-0041-archival-recall-and-async-curation.md)) has not yet promoted into `memory.sqlite` would be stranded in the non-portable transcript, and a freshly imported Bear could feel amnesiac about the last hour.

The target export workflow must **flush pending curation first** (not implemented by the current browser bundle): drain (or bound) the harvest/curation backlog so recent salient context is promoted into canonical memory before the SQLite snapshot is taken. After import the Bear still knows you because that knowledge is now memory — not because any transcript traveled. Never resolve this by adding transcripts to the package; the fix is always to curate before export, never to widen what the package carries.

## Package layout

```
bear-package/
  manifest.yaml          # identity, slug, models, role profiles, provisioning_version, schema versions
  memory.sqlite          # canonical memory + curation audit trail (per-Bear SQLite per ADR-0031)
  artifacts/             # optional: skill trees, exported policies (git-shaped)
```

There is **no** `manifest.lock` file. Version and compatibility fields live **directly in `manifest.yaml`** so importers can validate the package without a sidecar.

### `manifest.yaml` (primary manifest)

YAML is the canonical interchange format (not JSON). It carries:

- Bear **identity** (name, slug, charter text, optional source `bear_id` for provenance)
- **Version fields** for import validation (see [Schema and compatibility](#schema-and-compatibility))
- **Role runtime stances** (model ids, tool roster references, memory scope, approval policy keys)
- **Provisioning** metadata (`provisioning_version`, capability config)
- Pointers to **`artifacts/`** (skills, policies, optional source `context_profile` for recompile)

### `memory.sqlite`

Single SQLite file per Bear, named **`memory.sqlite`** (not `cognition.sqlite`). It is the authoritative store for append-only memory and curation audit per [ADR-0031](../decisions/adr-0031-sqlite-first-canonical-store-for-bear-agent-memory-and-tasks.md).

Tables included in a standard cognition export (see the [canonical schema](../../services/den/crates/den-memory/src/schema.sql)):

| Table | Purpose |
|-------|---------|
| `memory_records` | Append-only notes, summaries, decisions; role-local and shared |
| `entities`, `entity_handles` | Bear-local descriptive identities and handles |
| `authenticated_user_entities` | Local principal bindings and permanent imported/quarantined user-identity markers; never destination authentication authority on import |
| `user_identity_generations` | Current local authority generations; invalidate stale contexts across revocation/reconnection |
| `user_identity_origins` | Source-qualified export labels, account/host/Bear provenance, and exporter association; informational on import |
| `user_identity_receipts` | Append-only reconnection/revocation history; imported receipts are not destination grants |
| `personal_memory_candidates` | Isolated server-only preferred-name candidates; immutable source provenance retained, Pending quarantined on import |
| `personal_memory_settings` | Epoch-bound capture setting; source consent resets disabled on import |
| `memory_relations`, `memory_access_rules` | Descriptive relations and access-bearing rules |
| `memory_links` | Read view over descriptive relations and access-bearing rules |
| `memory_promotions` | Review/curate promotion audit trail |
| `memory_proposals` | Pending and resolved promotion proposals |
| `memory_observations` | Watch-lane observations pending or reviewed |
| `reflection_run_outcomes` | Canonical reflection run records and outcomes |
| `bear_sequence` | Monotonic sequence allocator (required for ordering/replay) |

> **Target relinking ([ADR-0042](../decisions/adr-0042-memory-entity-relationships-and-bear-entity-layer.md)):** keep opaque `entity_id`s stable and re-link ordinary entity references against destination registries via strong handles; current browser import quarantines rather than implementing that general resolver. Authenticated-user identities require explicit authorized reconnection, never automatic reactivation from a canonical ref, matching numeric ID, or handle. The dedicated human reconnection workflow above has verified code/storage/HTTP tests; live acceptance remains pending, and general automatic relinking, scoped sharing, and the plan's ordinary personal-memory feature remain unimplemented.

Operational SQLite settings at rest: `journal_mode=WAL`, `synchronous=NORMAL`, `busy_timeout=5000` (same as runtime defaults).

### `artifacts/` (optional)

Git-shaped trees for **human-authored** content retained outside SQLite:

- Approved **skill** documentation and trees
- Exported **policies** and prompt templates
- Optional **source** `context_profile` (or equivalent) when you prefer **recompile on import** over shipping compiled prompt output

Live machine-written memory does **not** belong in `artifacts/`; it belongs in `memory.sqlite`.

## Sample `manifest.yaml`

Skeleton showing key fields. Exact keys may evolve with Den releases; importers should tolerate unknown fields and enforce required version checks.

```yaml
# Bear package manifest — YAML primary interchange format
manifest_version: "1"              # format of this file (package envelope)
memory_schema_version: "2026060901" # must match importer's expected SQLite schema generation
den_min_version: "0.42.0"          # minimum Den build that can import this package

package:
  kind: bear_cognition             # bear_cognition | operator_snapshot (see export tiers)
  exported_at: "2026-06-09T12:00:00Z"
  exported_from:
    den_instance: "prod-den.example"   # informational only
    source_bear_id: "550e8400-e29b-41d4-a716-446655440000"  # provenance; import may assign new id

identity:
  name: "Atlas"
  slug: "atlas"                    # collision handling on import — see Import rules
  charter: |
    Platform reliability Bear for BEARS monorepo and production Den.

provisioning_version: 3            # Den role-profile provisioning generation

models:
  default: "claude-sonnet-4-20250514"
  by_role:
    chat: "claude-sonnet-4-20250514"
    pair: "claude-sonnet-4-20250514"
    work: "claude-opus-4-20250514"
    curate: "claude-sonnet-4-20250514"
    watch: "claude-haiku-3-20250307"

role_profiles:
  chat:
    memory_scope: [core, chat]
    sandbox: false
  pair:
    memory_scope: [core, pair]
    sandbox: true
  work:
    memory_scope: [core, work]
    sandbox: true
  curate:
    memory_scope: [core, curate, shared]
    sandbox: false
  watch:
    memory_scope: [core, watch]
    sandbox: false

# Prefer shipping source context for recompile when Den versions differ
prompt_policy:
  mode: source_recompile           # source_recompile | compiled
  context_profile: artifacts/prompts/context_profile.yaml
  # compiled_output: artifacts/prompts/compiled/   # only when mode: compiled

artifacts:
  skills:
    - path: artifacts/skills/incident-response
      manifest_ref: skills/incident-response
  policies:
    - path: artifacts/policies/web-sources.yaml

# Operator snapshot tier only (omit for cognition-only export)
web_source_policy: artifacts/policies/web-sources.yaml
watch_subscriptions:
  - id: github-releases-bears
    kind: github_releases
    config: { owner: bears-stack, repo: bears }

capabilities:
  tools: default                   # or explicit allowlist keys resolved by target Den
  approvals: standard
```

## Export tiers

| Tier | Contents | Typical use |
|------|----------|-------------|
| **Cognition export** | `manifest.yaml` + `memory.sqlite` + skills/`artifacts/` | Clone Bear knowledge, disaster recovery of memory, share approved skills |
| **Operator snapshot** | Cognition export **plus** durable prompt-memory blocks, web source policies, watch subscription **config** (no secrets) | Move a Bear to a new Den with the same operational posture |
| **Forensics** | Conversations, tool audit, full transcript | **Den Postgres only** — **not** part of the Bear package |

Forensics belong in Den backup/retention tooling, not in portable Bear packages. Omitting transcripts and credentials does not make the current full private admin snapshot safely shareable; scoped sharing remains unimplemented.

## Import rules

### Bear identity

| Strategy | When to use | Behavior |
|----------|-------------|----------|
| **New `bear_id` (recommended)** | Default import onto a host that already has Bears | Allocate fresh UUID; rewrite `bear_id` columns in `memory.sqlite`; update manifest identity; re-provision role bindings |
| **Preserve source UUID** | Trusted clone, disaster restore to empty Den | Keep `bear_id`; fail fast if that id already exists |

Slug is the human-stable handle. On collision with an existing Bear slug, importers should **suffix or prompt** (`atlas`, `atlas-2`) rather than silently overwrite.

### Model remap

Model ids in `manifest.yaml` are **hints from the source Den**. The target Den must **remap** each role's model to what Bifrost exposes on that host. Missing models should surface a clear import warning and fall back to Den defaults or operator-provided mapping — never fail silently with an unusable profile.

### Schema and compatibility

Importers validate three layers before applying SQLite and provisioning:

| Field | Meaning |
|-------|---------|
| `manifest_version` | Envelope format for this YAML file |
| `memory_schema_version` | Expected `memory.sqlite` table generation; reject or migrate if mismatch |
| `den_min_version` | Minimum Den semver that understands this package's profiles and artifact layout |
| `provisioning_version` | Role-profile compilation generation; triggers recompile when bumped |

There is **no** `manifest.lock`. All compatibility metadata is inline in `manifest.yaml`.

### Prompt policy: source + recompile vs compiled

When **source and target Den versions differ**, prefer:

1. **`prompt_policy.mode: source_recompile`** — ship `context_profile` (or equivalent) under `artifacts/`; target Den recompiles managed prompt policy on import.
2. **`compiled`** — only when source and target share the same `provisioning_version` and compiler; otherwise treat as best-effort and recompile anyway if validation fails.

Compiled output is a convenience; **source + recompile** is the portable default.

### Host bindings and secrets

Historical runtime bindings/stance settings are not live authority on import. Native initialization never creates/refreshes a profile registry or fabricates a default hat. Ordinary conversation/Job/run admission requires a real Bear-owned hat even on zero-hat Bears; imported records do not acquire owners or get automatically promoted into one. Do **not** copy membership, users, OAuth tokens, API keys, or webhook signing secrets — operators re-attach those on the target host. See [current WIP and rollout limits](../topics/bear-memory-hats.md).

### SQLite import mechanics

1. Validate `memory_schema_version` against importer support.
2. If using new `bear_id`, rebase owning `bear_id` columns. In the personal-candidate/settings tables, do not rewrite source/origin Bear IDs or historical source/audit IDs; those are provenance, not ownership or grants.
3. Place file at the target Den's per-Bear SQLite path (lifecycle defined by Den deployment; see ADR-0031 follow-up).
4. Register Bear row in Den Postgres from `manifest.yaml` (identity + slug + charter).
5. Initialize memory/runtime-plan/managed bound configuration and remap Bear-wide models; do not provision stance bindings or use historical profile model/loop rows as live overrides.
6. Rebuild derived indexes (semantic retrieval, if enabled) from canonical SQLite — never treat derivatives as source of truth.
7. General re-linking of entity `canonical_ref`s against destination registries via strong handles remains the target ([ADR-0042](../decisions/adr-0042-memory-entity-relationships-and-bear-entity-layer.md) §12). Current browser import quarantines external bindings. For authenticated-user identities, only the separate explicit human reconnection workflow above may establish destination authority; matching refs/handles/IDs never reactivate source grants.

## What stays on Den (reference)

For operators restoring **forensics** or **live operations**, use Den Postgres backups separately:

- `conversations`, messages, compaction state
- Tool audit and transcript tables
- Docket jobs, tasks, runs
- Reflection **scheduler/queue** rows (outcomes remain in SQLite)
- Approvals, ACP session state, role-runtime registry

See [reflection-run split](../architecture/den-runtime.md#the-reflection-run-split): queue in Postgres, outcomes in SQLite (outcomes ship in the package; queue does not).

## Rules of thumb

1. **Package = cognition + config**, not Den's project tracker or chat history.
2. **`memory.sqlite`** is the canonical memory graph; **`manifest.yaml`** is the portable control spec.
3. **No secrets** in the package; **no `manifest.lock`** — version fields live in the manifest.
4. **Default import** assigns a **new `bear_id`** and remaps **models** to the target Den.
5. **Ship source prompt context** when Den versions may differ; recompile beats stale compiled blobs.
6. **Forensics** are a Den operator concern, not a Bear package tier.

## Where implementation will live

Export/import APIs and CLI are not fixed in this guide. Expected touchpoints:

- Memory store: `services/den/src/core/memory/store/` (`schema.sql`, migrations)
- Bear provisioning and role profiles: Den core provisioning modules
- Artifact layout: aligned with git-retained human-authored trees per [ADR-0031](../decisions/adr-0031-sqlite-first-canonical-store-for-bear-agent-memory-and-tasks.md)

Track implementation status in the Den native runtime migration plan when export/import milestones are scheduled.
