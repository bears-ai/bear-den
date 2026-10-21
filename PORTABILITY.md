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

**Required target, not implemented:** the Bear's person identity is portable; a Den account's authentication authority is host-local. Import must retain person IDs and memory relations while disabling source grants. An authorized restore workflow must then let Alice explicitly connect imported Alice to her verified destination account without merging Bob into Alice or requiring the source Den to remain online. Export permission determines what cognition may travel; it neither identifies every included person as the importer nor grants ordinary recall of all their context.

The current foundation preserves and quarantines imported identities but has no reconnection operation. The [memory plan's required Phase 6A](docs/roadmap/USER_ASSOCIATED_BEAR_MEMORY_PLAN.md#phase-6a--portable-identity-reconnection) adds identity selection/provenance, destination conflict handling, atomic audited reconnection, and Alice/Bob round-trip tests before personal-memory enablement.

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
7. **Re-link entities** (pending [ADR-0042](docs/decisions/adr-0042-memory-entity-relationships-and-bear-entity-layer.md)): entity references re-resolve against the destination's registries via strong handles; entities that fail to re-resolve are demoted to `provisional`, and access-bearing rules stay inert until re-resolved — fail-closed.
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
