# Bear package format (portability)

How to **export**, **transport**, and **import** a Bear's **cognition and configuration** without dragging Den control-plane state along. A portable package is what you move when cloning a Bear to another host, backing up durable knowledge, or sharing an approved skill/policy bundle — not a full Den snapshot.

**Related:** [ADR-0031 — SQLite-first canonical store](../decisions/adr-0031-sqlite-first-canonical-store-for-bear-agent-memory-and-tasks.md), [`den-runtime.md`](../architecture/den-runtime.md) (storage boundary), [`memory-model.md`](../architecture/memory-model.md), [`bear-memory.md`](bear-memory.md), [`work-surfaces-and-conversations.md`](work-surfaces-and-conversations.md)

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

Export therefore **flushes pending curation first**: drain (or bound) the harvest/curation backlog so recent salient context is promoted into canonical memory before the SQLite snapshot is taken. After import the Bear still knows you because that knowledge is now memory — not because any transcript traveled. Never resolve this by adding transcripts to the package; the fix is always to curate before export, never to widen what the package carries.

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

Tables included in a standard cognition export (see `services/den/src/core/memory/store/schema.sql`):

| Table | Purpose |
|-------|---------|
| `memory_records` | Append-only notes, summaries, decisions; role-local and shared |
| `memory_links` | Links from memory to artifacts, URLs, work surfaces, other memory |
| `memory_promotions` | Review/curate promotion audit trail |
| `memory_proposals` | Pending and resolved promotion proposals |
| `memory_observations` | Watch-lane observations pending or reviewed |
| `reflection_run_outcomes` | Canonical reflection run records and outcomes |
| `bear_sequence` | Monotonic sequence allocator (required for ordering/replay) |

> **Pending [ADR-0042](../decisions/adr-0042-memory-entity-relationships-and-bear-entity-layer.md):** the Bear entity layer — `entities`, `entity_handles`, `memory_relations`, `memory_access_rules` — ships in this same cognition export, and `memory_links` becomes a read view over the two relation tables. Import keeps each opaque `entity_id` stable and **re-links `canonical_ref`** against the destination registries via strong handles (entities that fail to re-resolve are demoted to `provisional`; access-bearing rules stay inert until re-resolved — fail-closed). Adds to `memory_schema_version`.

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

Forensics belong in Den backup/retention tooling, not in portable Bear packages, so packages stay shareable without leaking host conversation history or credentials.

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
2. If using new `bear_id`, rewrite `bear_id` in all SQLite tables in one transaction.
3. Place file at the target Den's per-Bear SQLite path (lifecycle defined by Den deployment; see ADR-0031 follow-up).
4. Register Bear row in Den Postgres from `manifest.yaml` (identity + slug + charter).
5. Initialize memory/runtime-plan/managed bound configuration and remap Bear-wide models; do not provision stance bindings or use historical profile model/loop rows as live overrides.
6. Rebuild derived indexes (semantic retrieval, if enabled) from canonical SQLite — never treat derivatives as source of truth.
7. Re-link entity `canonical_ref`s against destination registries via strong handles ([ADR-0042](../decisions/adr-0042-memory-entity-relationships-and-bear-entity-layer.md) §12); demote entities that do not re-resolve to `provisional`.

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
