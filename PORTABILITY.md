# Bear Portability Between Den Servers

How a Bear moves between Den servers — for cloning, disaster recovery, or migrating to a new host — without dragging host-specific state along. This consolidates the portability model; the canonical reference is [docs/guides/bear-package.md](docs/guides/bear-package.md).

## Why Bears are portable by construction

The architecture draws one line through all state ([ARCHITECTURE.md](ARCHITECTURE.md), [docs/architecture/den-runtime.md](docs/architecture/den-runtime.md#storage-boundary-bear-cognition-vs-den-control-plane)):

- **Bear cognition** — everything the Bear knows and how it came to know it — lives in a single per-Bear SQLite file (`memory.sqlite`).
- **Den control plane** — conversations, approvals, tasks, membership, schedulers — lives in the host's Postgres.

A portable Bear is the cognition side of that line plus its configuration. Because the boundary is structural (cross-store references are by id only, with no content-sync seam), export never has to untangle Bear knowledge from host infrastructure.

## What moves and what stays

| Concern | Moves with the Bear? | Where it lives |
|---------|----------------------|----------------|
| Stance-local and shared memory, promotions, proposals, observations, reflection outcomes | **Yes** | `memory.sqlite` |
| Identity, charter, prompts, stance profiles, capability config | **Yes** | `manifest.yaml` (+ `artifacts/`) |
| Work-surface knowledge (anchors and overviews under `core/`) | **Yes** | `memory.sqlite` |
| Approved skills and exported policies | **Yes** | `manifest.yaml` references + `artifacts/` |
| Durable prompt-memory blocks, web source policy, watch subscription config | Operator tier only | manifest config (no secrets) |
| Conversations, transcripts, tool audit | **No** | Den Postgres (host forensics) |
| Docket jobs/tasks/runs, live workboard plans | **No** | Den Postgres |
| Users and membership | **No** | Den Postgres |
| In-flight approvals, ACP session state | **No** | Den Postgres / channel adapters |
| Host runtime binding ids (`den-native:{id}:chat`, …) | **No** | Re-provisioned on import |
| Derived recall vectors (Qdrant) | **No** | Rebuilt from `memory.sqlite` on import |
| Secrets (OAuth, API keys, webhook signing) | **Never** | Host secret store; re-attached by the operator |

Keeping conversations and secrets out of the package avoids transporting the source host's chat history or credentials. It does not make a cognition snapshot public or safely shareable: full private backups and selectively shared exports have different content/authorization policies.

Because transcripts stay behind, continuity across a move rides on curated memory, not chat logs. The target export workflow first **flushes pending curation** (not yet implemented by the browser bundle) — the async harvest lane ([ADR-0041](docs/decisions/adr-0041-archival-recall-and-async-curation.md)) drains so recent salient context is promoted into `memory.sqlite` before the snapshot — so an imported Bear still knows you because it remembers, not because its transcript traveled. See [pre-export curation flush](docs/guides/bear-package.md#continuity-across-a-move-pre-export-curation-flush).

### User identity continuity across hosts

**Phase 6A reconnection code/storage/HTTP tests verified (2026-10-21); live acceptance and broader gates pending, not deployed:** the Bear's person identity is portable; a Den account's authentication authority is host-local. Import retains person IDs and memory relations while quarantining all source authentication authority. A human-only **Backup & move → Imported identities** flow at `/bear/{slug}/portability/identities` lets a current verified Bear Admin explicitly reconnect imported Alice to a current verified destination Bear member without merging Bob into Alice. Export permission determines what cognition may travel; it neither identifies every included person as the importer nor grants ordinary recall of all their context.

Export records source-qualified identity labels/account/host, source Bear, and exporter association in SQLite `user_identity_origins` without allocating a person. Labels and historical imported receipts help human review but never grant destination authority. Reconnection works without the source Den online, copied credentials, or transcripts. It has a separate explicit review/confirmation with actor/session/Bear-bound expiring nonces and a server-held typed snapshot; Postgres User/email/Bear locks protect current eligibility through the atomic SQLite mapping/entity/handle/generation/receipt commit, including source-metadata snapshot comparison inside that SQLite transaction. Generic identity mutation remains prohibited, and no model API is exposed.

Exactly one person is active per Bear/account. With separate explicit acknowledgement, a fresh or populated destination identity can be **parked with all records, relations, candidates, and provenance preserved**, not merged or automatically jointly recalled. It remains reconnectable later through another review. Mistakes can be corrected by reviewed revocation followed by a separate reconnection; authority generations reject stale contexts even when the same tuple is restored. Memory records, their lifecycle, IDs, and audience targets are never rewritten. The management receipt-history view prioritizes local decisions for recovery after a lost confirmation response; optional history failures do not block mapping, and imported history is never live authority.

The change adds only SQLite `user_identity_generations`, `user_identity_origins`, and `user_identity_receipts` alongside existing identity classification: no Postgres migration or dependency change, and the browser bundle was **version 3 at that checkpoint**. Current exports use **v6** (readers **1–6**), requiring the erasure ledger, erased-body handling and retained-source guards. V4 introduced permanent-publication protection; v5 added typed direct-owner request provenance and revision-aware consent. Older readers must reject v6; upgrade destination Den and never lower the manifest version or strip protection/audit state. See the [guide's current browser bundle and reconnection workflow](docs/guides/bear-package.md#reconnect-imported-identities) and [active Phase 6A milestone](docs/roadmap/USER_ASSOCIATED_BEAR_MEMORY_PLAN.md#phase-6a--portable-identity-reconnection) for verified code/storage/HTTP evidence and pending mixed-workspace SQLx verification. The earlier v5 owner-workflow production/musl image passed, but **no v6 erasure production image has been built**; image/deployment, native-browser, live-provider and actual two-host live acceptance remain pending. No serving-Postgres migration is included.

This is not the completed personal-memory feature. Owner-only request/approval/review/correction/retraction/opt-out is committed as `2220cfea9`, including the passing historical v5 direct-request round trip. The [explicit-erasure follow-on](docs/roadmap/USER_ASSOCIATED_BEAR_MEMORY_PLAN.md#explicit-preferred-name-erasure-checkpoint) is implemented/tested: About you erases **all** owned preferred-name candidate/fact body versions, leaves `CandidateState::Erased`/`preferred_name = None`, capture off and the retained current head `Retracted`, while preserving immutable heads, classification and audit IDs. Original SQLite source bodies remain but are hidden from generic **and scoped** memory read/search/restaging via exact record/ledger guards, not name-string deletion.

The content-free ledger compares reviewed head/latest candidate/capture revision and the exact retained owner binding; original receipt replay returns old affected IDs without erasing future names. Import rebases only its owning `bear_id`, preserving recorded-host Bear/actor qualifiers and IDs; imported pending history is **not** target-local cleanup authority. Host-local cleanup scrubs dependency-marked canonical assistant/tool messages and summary payloads and deletes marked BearWire events, keeping raw original human user rows. It uses the same Postgres authority connection, commits before fresh membership recheck/SQLite acknowledgement, and retries durable pending work on owner inspection/action. This is not a distributed transaction or deletion of original human history.

Existing reconnection, bundle-I/O and v1–v5 compatibility checks passed; the **new v6 erasure-ledger real HTTP export/import/reconnect test also passed** against an isolated base-plus-owned-changes snapshot. The [checkpoint](docs/roadmap/USER_ASSOCIATED_BEAR_MEMORY_PLAN.md#explicit-preferred-name-erasure-checkpoint) records observed tests and SQLx evidence. Local scrub does not solve source/context release gates: full runtime history/in-flight integration remains unfinished/disabled, along with model-tool intake, audience-preserving derivation, complete history/scoped export and ordinary personal recall. Phase 6A's ordinary personal-recall exit remains open. Preferred-name scope, ordinary configured inference/non-secret trust and provider/secret policies are unchanged; automatic forgetting remains future work. Hats maintain context/core cleanliness, not privacy between hats. The full private admin snapshot is distinct from unimplemented scoped sharing and pre-export curation flushing.

## The package format

```text
bear-package/
  manifest.yaml    # identity, versions, models, stance profiles, capability config
  memory.sqlite    # canonical memory + curation audit trail
  artifacts/       # optional: skill trees, exported policies, source prompt context
```

- **`manifest.yaml`** (YAML, the canonical interchange format) carries Bear identity (name, slug, charter), version fields for validation, per-stance model ids and profiles, provisioning metadata, and pointers into `artifacts/`. There is no separate lockfile — all compatibility metadata is inline.
- **`memory.sqlite`** is the authoritative cognition store: `memory_records`, `memory_links`, `memory_promotions`, `memory_proposals`, `memory_observations`, `reflection_run_outcomes`, and the `bear_sequence` allocator required for ordering and replay.
- **`artifacts/`** holds human-authored, git-shaped content: approved skill documentation, exported policies, and optionally the source `context_profile` so the target Den can recompile prompts. Live machine-written memory never goes here.

### Export tiers

| Tier | Contents | Use |
|------|----------|-----|
| **Cognition export** | manifest + `memory.sqlite` + `artifacts/` | Clone knowledge, back up memory, share approved skills |
| **Operator snapshot** | Cognition export + durable prompt-memory blocks, web source policies, watch subscription config (no secrets) | Move a Bear to a new Den with the same operational posture |
| **Forensics** | Conversations, tool audit, transcripts | **Not a package tier** — Den Postgres backup/retention tooling only |

## Importing on the target Den

The import sequence, in order:

1. **Validate versions.** `manifest_version` (envelope), `memory_schema_version` (SQLite table generation — reject or migrate on mismatch), `den_min_version` (minimum Den build), `provisioning_version` (stance-profile compilation generation — triggers recompile when bumped).
2. **Assign identity.** Default: allocate a **new `bear_id`** and rewrite it across all SQLite tables in one transaction. Preserving the source UUID is reserved for trusted clones or disaster restore to an empty Den (fail fast on collision). Slugs are the human-stable handle; on collision, suffix or prompt (`atlas` → `atlas-2`), never silently overwrite.
3. **Register and re-provision.** Create the Bear row in target Postgres from the manifest, then re-provision stance profiles. Host runtime bindings are always dropped and recreated — they are host-local, not portable.
4. **Remap models.** Model ids in the manifest are hints from the source host. The target Den remaps each stance's model to what its Bifrost exposes; missing models surface a clear warning and fall back to defaults or an operator mapping — never a silently unusable profile.
5. **Recompile prompts.** When source and target Den versions differ, prefer `prompt_policy.mode: source_recompile` — ship the source `context_profile` and let the target compile it. Shipping compiled prompt output is a convenience only valid when both hosts share the same `provisioning_version` and compiler.
6. **Rebuild derived state.** Semantic recall indexes are rebuilt from canonical SQLite; vectors are disposable and never imported as truth.
7. **Re-link entities** (general target in [ADR-0042](docs/decisions/adr-0042-memory-entity-relationships-and-bear-entity-layer.md)): general automatic strong-handle relinking remains pending; current browser import quarantines external bindings and fails closed. Authenticated-user identities may be reconnected only through the separate human review above, never by matching source IDs or handles.
8. **Operator re-attaches** memberships, connections, and secrets on the target host.

## Rules of thumb

1. **Package = cognition + config**, not Den's project tracker or chat history.
2. `memory.sqlite` is the canonical memory graph; `manifest.yaml` is the portable control spec.
3. **No secrets in the package**, ever.
4. Default import assigns a **new `bear_id`** and **remaps models** to the target host.
5. **Ship source prompt context** when Den versions may differ; recompile beats stale compiled blobs.
6. Forensics are a host operator concern, not a Bear package tier.

## Canonical sources

- [docs/guides/bear-package.md](docs/guides/bear-package.md) — package format, manifest fields, import mechanics
- [ADR-0031](docs/decisions/adr-0031-sqlite-first-canonical-store-for-bear-agent-memory-and-tasks.md) — SQLite-first canonical store
- [ADR-0038](docs/decisions/adr-0038-platform-embedding-standard-and-derived-recall-index.md) — derived recall is rebuildable
- [ADR-0042](docs/decisions/adr-0042-memory-entity-relationships-and-bear-entity-layer.md) — entity layer and re-linking on import
- [docs/architecture/den-runtime.md](docs/architecture/den-runtime.md) — the storage boundary that makes this possible
