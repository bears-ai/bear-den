# ADR-0042 — Memory–Entity Relationships and the Bear Entity Layer

**Status:** Proposed (2026-06-14)
**Deciders:** Hans
**Related:**
- [ADR-0031 — SQLite-first canonical store](adr-0031-sqlite-first-canonical-store-for-bear-agent-memory-and-tasks.md)
- [ADR-0041 — Archival recall and asynchronous curation](adr-0041-archival-recall-and-async-curation.md)
- [ADR-0038 — Platform embedding standard and derived recall index](adr-0038-platform-embedding-standard-and-derived-recall-index.md)
- [ADR-0006 — Bear work surfaces](adr-0006-bear-work-surfaces.md)
- [ADR-0040 — Connections and work-surface presentation](adr-0040-connections-and-work-surface-presentation.md)
- [ADR-0039 — Trust profiles and governance](adr-0039-trust-profiles-and-governance.md)
- [ADR-0015 — Multi-user memory](adr-0015-multi-user-memory.md) (Letta-era; to be reframed for native by this ADR)
- [ADR-0008 — Cabinet reading pipeline](adr-0008-cabinet-reading-pipeline.md)
- [Memory model](../architecture/memory-model.md)

## Context

Retiring Letta cost the native runtime a capability it has not replaced: **delineating memory about specific entities**. In the Letta era, MemFS templates and ADR-0015's isolated `human` / `person:{name}` blocks let a Bear keep per-user memory. The native SQLite model ([ADR-0031](adr-0031-sqlite-first-canonical-store-for-bear-agent-memory-and-tasks.md)) delineates memory by **trust profile** (`scope_type`/`scope_profile`) and by **work surface** (`work_surface_ref` + anchor projection), but has no first-class way to relate memory to a person, contact, event, organization, or other entity. The `entity_ref` column exists but is never populated; `memory_links` (`dst_ref_type`/`dst_ref`/`link_type`) is the only generic hook and is unused.

Two observations frame the decision:

- **"Person" is the wrong unit to hard-code.** We will also want contacts, events, missions, domains, organizations, artifacts, places. Hard-coding `person` repeats the Letta mistake at a different layer.
- **Relating memory to an entity is not one relationship, and "entity" is not one storage tier.** A note can be *about* an entity, *for* an entity's eyes only, *from* an entity, *involving* several entities, or *applicable when* an entity is in play. And the same real-world person ("Ryan") shows up as a doc mention, a Slack user, and a Cabinet author — identities that must be **resolved into one entity and stay correctable** (the Slack Ryan may turn out to be someone else).

This sits cleanly in the [four-axis model](../architecture/interactive-stances-and-role-axes.md): memory scope partitions by **trust stance** (Trust axis) and **work surface** (Resource axis). Entity-relatedness is **none of those axes** — it is a cross-cutting *relation on a record*, not a profile scope ([ADR-0039](adr-0039-trust-profiles-and-governance.md)). It must therefore be modeled as typed relations, not as another `scope_*` partition.

The design must be **flexible for new relationship and entity types without becoming RDF**: no arbitrary predicates, no triple store, no inference.

## Decision

### 1. Memory references entities; it does not define them

Bear memory holds **statements about** and **typed relations to** entities. The canonical object for an entity lives in its owning **registry**, not in bear memory:

- People / Missions / Knowledge → **Cabinet** ([ADR-0008](adr-0008-cabinet-reading-pipeline.md))
- Users / Connections / Work surfaces / Docket projects → **Den control-plane** ([ADR-0040](adr-0040-connections-and-work-surface-presentation.md), [ADR-0034](adr-0034-jobs-and-tasks-work-management.md))
- Events → calendar / external systems

**Entity↔entity edges** ("Ryan authored Doc X", "Work surface W is reached through Connection C") stay in those registries. Bear memory stores only **record→entity** relations and its **own reference + resolution** of each entity. A Bear may hold a **provisional** entity until it is canonicalized (mirroring the work-surface lifecycle).

### 2. Two layers

1. **Bear entity layer (bear-local, portable):** `entities` + `entity_handles` — the Bear's awareness and resolution of every entity it knows, with a `canonical_ref` into the owning registry when resolved. This is the portable unit (travels in the bear package).
2. **Memory–entity relation layer:** relations from a `memory_record` to a bear-local `entity_id`, typed and classified (§3).

### 3. The relation is not singular: descriptive vs access-bearing (settled — "fork 1")

Relation **kinds** split into two **classes**, because they have different enforcement semantics even though they share a shape:

- **Descriptive** — filter/boost only, never gate visibility: `subject` (aboutness), `source` (provenance/origin), `participant` (multi-party involvement), `applies_when` (context/trigger, e.g. an event or work surface).
- **Access-bearing** — gate visibility/recall, enforced: `audience` (only surface when acting with/for this entity — the native heir to Letta's isolated `human` block), `confined_to` (hard scope; e.g. client-A memory must not leak into client-B's work surface).

The same entity may be referenced under either class (e.g. a work surface via `applies_when` to *boost* work-surface-first recall, vs `confined_to` to *gate* leakage). Work-surface grounding precedence stays **descriptive**; only true confinement is access-bearing.

### 4. Open, governed, descriptor-owned vocabularies — not RDF (settled)

Entity types, relation types, and presentation labels are all **descriptor-owned** (consistent with [ADR-0040](adr-0040-connections-and-work-surface-presentation.md) and `AGENTS.md`: no scattered `match` arms or hardcoded allowlists). Adding a relation or entity type is a **descriptor edit, not a schema migration**. Writes validate against the registry.

Anti-RDF guardrails (invariants):

- **Fixed edge domain/arity:** every relation is exactly `memory_record → entity`. No entity↔entity edges, no record↔record predicates, no reification.
- **Objects are resolved entity ids, never literals.**
- **Relations are a curated descriptor registry, not open IRIs.**
- **No inference:** relations are stored facts; no transitive closure, ontology, or inverse-derivation.
- **Retrieval-time traversal is allowed; stored graph structure is not.** Recall may walk the **bipartite** record↔entity links at query time — depth-capped, read-only — to expand candidates via shared entities ([ADR-0041](adr-0041-archival-recall-and-async-curation.md) §6, [DERIVED_RECALL Phase 3.5](../roadmap/DERIVED_RECALL_INDEX_IMPLEMENTATION_PLAN.md)). This persists nothing, derives no new edges, and never creates entity↔entity links — it is query expansion, not inference or a knowledge graph.
- **Class is a closed, immutable 2-value enum** (`descriptive | access_bearing`) owned by the relation descriptor.
- **Flexibility comes from bounded qualifiers, not type proliferation** (property-graph-lite): an edge may carry a small, descriptor-allowed set of typed qualifiers (e.g. `confidence`, `is_primary`, `source_handle`).

A relation descriptor declares: canonical dotted id + model-facing alias, `class`, `cardinality`, `applies_to_entities`, `allowed_qualifiers`, `recall_effect` (`boost` | `gate` | `none`), and `anchor_projecting`.

### 5. One unified entity model; `work_surface` and `connection` are entity types (settled — "fork A")

There is **one** entity-resolution model and machinery. **Work surface** and **Connection** are entity *types* within it, not parallel tables:

- The work-surface resolution lifecycle **is** the entity-resolution lifecycle (§6); the work-surface anchors (observed checkout vs canonical git remote) **are** the handle model (§6). Keeping them parallel would duplicate the hard part (resolution).
- Canonical homes stay **per-type** via `canonical_ref`: externally-backed work surfaces resolve through **Connections** (Den control-plane, owner-scoped, [ADR-0040](adr-0040-connections-and-work-surface-presentation.md)); internally-backed ones resolve to Cabinet/Docket. No work-surface type is "bear-local canonical." `connection` is its own entity type; the work_surface↔connection edge is registry-owned (§1).
- Migration is **additive**: the existing work-surface resolution code becomes the **first per-type resolver**; nothing about today's work-surface behavior must break (consistent with ADR-0040's "no code/schema churn"). `work_surface_ref` may remain as a denormalized convenience during transition.

### 6. One resolution + trust lifecycle for all entity types (settled — "fork 3")

All entity types share one lifecycle, because the epistemic problem is type-agnostic: the Bear sees identifying signals over time, decides "new entity or known one?" under uncertainty, and must stay correctable.

- **Resolution states:** `observed → provisional → resolved → confirmed`, plus `rejected` and `merged`/`superseded` (reuse the [work-surface states](../architecture/memory-model.md#canonical-work-surface-anchors)).
- **Trust** is "who asserted the identity" (session/ACP token, normalized git remote, calendar id = strong; chat-text inference = weak), not the entity type.
- **Multiple entry points, one lifecycle:** authoritative-by-construction entities (a Den user from the token; a real Cabinet Mission id) enter directly at `confirmed`/`resolved` with `trust = asserted`. This is a fast path, not a second lifecycle.
- **Handles/aliases** are first-class: an entity has many `entity_handles` (`slack:U123`, `cabinet:author:ryan`, `mention:"Ryan"`, `email:…`) that attach/detach without destroying the entity.
- **Merge and split** are first-class, audited operations (correcting a bad merge = split + re-home the offending handle), exactly like work-surface merge/reconcile.
- **Per-type resolvers** plug type-specific canonicalization and handle-strength policy into the shared states.

### 7. Relation storage: two physical tables, descriptor-routed (settled — "fork C")

Because access-bearing relations are a **security boundary**, the safe path is structural rather than a remembered predicate. Two physical tables:

- `memory_relations` — descriptive record→entity relations; broad write access; boost/filter only.
- `memory_access_rules` — access-bearing record→entity relations; **the only table the recall gate consults**; append-only (so it *is* the visibility audit log); tighter write authority (e.g. `curate`/Den only); may enforce stricter constraints (e.g. gate targets must be `resolved`/`confirmed` + trusted — gating on a mergeable provisional identity is itself a leak risk).
- `memory_links` (read **view**) — `memory_relations ∪ memory_access_rules` with a `class` column, for cross-cutting reads (projection, "everything about entity E").

Properties:

- **The descriptor still owns the class and routes the write** to the correct table; writers never choose, so misfiling is impossible.
- This eliminates the *misclassification* failure mode entirely (a descriptive query cannot return access-bearing rows). The *forgot-the-gate* failure mode is closed in code by making an `AccessContext` (the `memory_access_rules` query) a **required input** to the recall assembler, so omission is a compile error rather than a silent open-fail.
- **`entity_ref` is retired** (the table identity encodes class; the relation layer carries aboutness). The "primary subject" used for path anchoring becomes a `subject` relation flagged `is_primary`, or the existing `logical_path`.
- Relations are **append-only with supersession/retraction** (`state ∈ {active, retracted, superseded}` + `supersedes_link_id`), consistent with `memory_records`. Retracting "this note is about Ryan" is a relation op; re-identifying Ryan is an entity-layer merge/split — clean separation.

### 8. Anchors generalize beyond work surface (settled — "fork 5")

Path anchors were never work-surface-specific; work surface was just the first entity type to earn them. **Resolved + salient** entities get projected anchors (`core/people/<id>/…`, `core/missions/<id>/…` alongside `core/work_surfaces/<slug>/…`); **transient / low-salience mentions stay query-derived views** over the relation layer. The promotion threshold is the same kind of "enough trusted signal + salience" decision used for work surfaces.

**V1 anchor projection policy.** Entity anchors are explicit canonical memory records, not synthesized from arbitrary relation hits:

- Entity must be `resolved` or `confirmed`.
- Entity type must be descriptor `anchor_eligible`.
- Salience threshold: at least one `subject`-linked memory record with `salience = high|critical`, or a `confirmed` entity with at least two `normal` `subject`-linked records.
- Only relations whose descriptor has `anchor_projecting = true` count toward anchor promotion; in the v1 descriptor set this means `subject`.
- Projection reads generated anchor paths only, such as `core/people/<anchor>/profile.md` or `core/missions/<anchor>/overview.md`. If no anchor file exists, the entity remains query-derived via `memory_links` and recall/search.
- Derived fallback from `memory_links` into prompt projection is deferred until curation/consolidation policy is mature.

### 9. Portability

The **bear entity layer travels in the bear package** so relations never dangle on export ([bear package](../guides/bear-package.md)): the package carries `entities` + `entity_handles` (bear-local ids, types, display names, handles, resolution state, trust, provenance, `canonical_ref`). On **import**, `canonical_ref`s are re-linked against the destination registries; unresolved ones stay provisional. We export the Bear's **references and resolution**, not the registries' knowledge — **extracting Cabinet's own knowledge about an entity is explicitly deferred**. When a Bear is exported/imported, Den must communicate the set of entities the Bear is aware of.

### 10. Relationship to ADR-0015 and ADR-0041

- This ADR is the **native replacement** for [ADR-0015](adr-0015-multi-user-memory.md)'s per-user memory: "the human you are talking to" is a `person` entity at `confirmed`/asserted trust (identity from the session/ACP token), and per-user isolation is an `audience` access-bearing relation — not a Letta isolated block.
- Recall scoring ([ADR-0041](adr-0041-archival-recall-and-async-curation.md), [ADR-0038](adr-0038-platform-embedding-standard-and-derived-recall-index.md)) gains **entity filters**: descriptive relations boost; access-bearing relations gate; the Qdrant passage payload should carry resolved `entity_id`s for filtered recall.

### 11. Entity identity: ids, handles, and strong/weak resolution (settled — "fork D")

**Entity ids.** Opaque, stable, bear-local (ULID-style `TEXT`). Type is a column, **never encoded in the id** — types can change (a provisional `person` may be promoted to a Den user; a `contact` is just a `person` with address-book provenance). The `entity_id` is the **portable identity**; `canonical_ref` re-links on import; merge/split never reuse ids.

**Merge and split.**

- **Merge:** choose a survivor; the loser gets `state = merged` and `superseded_by_entity_id = survivor`. Reads follow the forward pointer to the live entity; a background pass repoints relations.
- **Split:** create a new entity, move the offending handle(s) to it, and re-home relations using the `source_handle` qualifier — relations whose `source_handle` belongs to the moved handle follow it; provenance-less relations go to `curate` review.
- Merge/split are consequential identity operations: **`curate` (and human via UI) only.**

**Entity-type vocabulary (v1, descriptor-owned).** `contact`, `place`, and others are added by registering a descriptor — no migration.

| Entity type | Owning registry (`canonical_ref`) | Default trust | Anchor-eligible |
|---|---|---|---|
| `person` | Cabinet People / Den user (when known) | inferred (asserted from token) | yes |
| `org` | Cabinet | inferred | yes |
| `event` | calendar / external | inferred (asserted from event id) | optional |
| `mission` | Cabinet | asserted | yes |
| `domain` | bear-local | asserted | yes |
| `work_surface` | Den control-plane / Cabinet / Docket | asserted | yes |
| `connection` | Den control-plane (owner-scoped) | asserted | no |
| `artifact` | external (URL/registry) | inferred | optional |

`contact` is **not** a type — it is a `person` with address-book provenance (a handle / `canonical_ref`).

**Handle vocabulary + strength.** Strength is declared on the **handle-type descriptor**, optionally overridable per entity type (e.g. a `checkout` path is unique but still *weak* because it is machine-local).

| Handle type | Example | Strength |
|---|---|---|
| `den_user` | `den_user:42` | strong |
| `session_human` | (from ACP token) | strong (asserted) |
| `git_remote` | `git_remote:github.com/acme/app` | strong |
| `cabinet_ref` | `cabinet_ref:people/ryan` | strong |
| `connection_ref` | `connection_ref:gh-acme` | strong |
| `calendar_event_id` | `calendar_event_id:…` | strong |
| `slack_user` | `slack_user:T01:U07` | strong |
| `email` | `email:ryan@acme.com` | strong |
| `url` | `url:https://github.com/acme/app/pull/12` | strong |
| `checkout` / `workspace_root` | `checkout:/home/h/app` | weak |
| `mention` | `mention:"Ryan"` | weak |
| `alias` | `alias:"Ry"` | weak |

**Resolution algorithm.**

1. The per-type resolver normalizes incoming signals into handles.
2. **Exact match on a strong handle ⇒ resolve to that entity** (attach new handles; raise resolution/trust per the source). A new entity created from a strong handle starts at `resolved` (or `confirmed` if session-asserted).
3. **Only weak handles ⇒** search for candidates (matching weak handles, name similarity *within type*); a confident candidate yields `candidate`/`ambiguous` for confirmation; otherwise create a new `provisional`/`observed` entity.
4. **Never auto-merge across *different* strong identities on similarity.** Cross-handle unification (Ryan's `slack_user` + `email` + `cabinet_ref` are one person) requires an **authoritative registry mapping** (Den identity map, Cabinet People) or an explicit merge.
5. **Conflicting strong matches** (a new signal claims two existing strong-handle entities are one) → `curate`/human, never automatic.

**Trust gate (ties to §7).** Access-bearing relations (`memory_access_rules`) may target **only `resolved`/`confirmed`** entities. Provisional/weak entities can hold descriptive relations but cannot gate visibility — a mis-resolved provisional must never leak memory.

**Who does what.**

- Deterministic strong-handle resolution: any ingestion path.
- Provisional-entity creation, handle attach, and descriptive relations: `chat`/`pair`/`work`/`watch`.
- Weak-candidate promotion, merge/split, and **all** access-bearing relations: `curate` (+ human via UI), consistent with `curate` owning cross-profile governance.
- Session/ACP-token identity ⇒ a `person` at `confirmed` + asserted trust; **never inferred from chat text** (per `AGENTS.md`).

#### Authenticated-principal binding amendment (2026-10-21)

The [user-associated memory plan](../roadmap/USER_ASSOCIATED_BEAR_MEMORY_PLAN.md) implements a narrow authentication foundation over the existing `person` entity type. This amendment does not mark the broader ADR or the full personal-memory feature complete; current implementation and validation live in the [memory topic](../topics/bear-memory-hats.md#authenticated-user-identity-foundation).

- `authenticated_user_entities` is the canonical Bear/principal-to-entity authority, separate from the portable descriptive identity/reference. Its `active` or `quarantined` state is not a second entity-resolution lifecycle: it answers whether a local authenticated principal currently authorizes that exact entity.
- Only server-side resolution from the verified typed Den user ID creates an active binding and its confirmed/asserted person atomically. Matching names, canonical refs, or historical strong handles never establish that authority.
- Bound and quarantined user entities/handles are not subject to generic merge/split/reassignment. Ordinary descriptive-person reconciliation cannot widen a principal's audience through a merge forward pointer.
- Import never treats source-host numeric user IDs as destination authority. It rejects triggers, foreign keys, virtual tables, and noncanonical views before any store initialization writes, retains quarantined identity markers, and demotes/clears their external bindings atomically. In the current foundation, ordinary destination resolution creates a fresh entity and cannot recover imported audiences. The required reconnection milestone below addresses that portability gap without automatically restoring source-host authority.
- User-audience recall requires the exact current verified Bear/user/entity binding. Raw entity IDs, missing or invalid targets, and retired access-rule histories are not grants. Audience restrictions apply independently of hat selection: hats organize context and maintain a clean core, not privacy between hats.

Protected publication, derived information flow, export/operator authority, replay-aware removal, user controls, and the remaining product policy are still delivery gates in the plan; no personal-fact tool or recall projection is enabled by this foundation.

#### Authorized portability reconnection clarification (2026-10-21)

**Required target, not implemented:** the Bear remembers a portable person; a Den account authenticates that person on a host. Preserve person `entity_id`s and memory relation targets across an authorized move. Quarantine revokes source authentication authority, but must not make the person permanently unusable: a dedicated, human-authorized restore operation establishes a new destination binding to the original person. Generic merge/split/reassignment guards remain; reconnecting Alice is not permission to merge her with Bob.

Ordinary recall, private backup/migration, and identity reconnection have separate authorization boundaries. An authorized full backup may carry distinct personal audiences within its export grant; carrying them does not map all of them to the importer. The destination must check restore/reconnection rights, explicitly confirm the selected person/account mapping, resolve existing destination-binding/context conflicts without data loss, and commit the binding and audit receipt atomically. Source labels/IDs are review provenance, not grants. The source Den need not remain online, and no source credentials or transcripts are required.

The [memory plan's Phase 6A](../roadmap/USER_ASSOCIATED_BEAR_MEMORY_PLAN.md#phase-6a--portable-identity-reconnection) makes reconnection, Alice/Bob round-trip isolation, repeat moves, conflict handling, and revocation tests pre-enablement requirements. The foundation's current permanently quarantined rows and one-active-person-per-account constraint must be evolved through that narrow workflow, not bypassed by ordinary entity tools.

### 12. Portability: package placement and import re-linking (settled — "fork E")

The entity layer needs **no new package tier**: `entities`, `entity_handles`, `memory_relations`, and `memory_access_rules` live in per-Bear SQLite (bear cognition), so they ship automatically in the existing **cognition export** (`memory.sqlite`) alongside `memory_records` ([bear package](../guides/bear-package.md)). `memory_links` is a view, recreated by schema. This bumps `memory_schema_version`.

- **`bear_id` rewrite already covers the new tables** (the existing import rule rewrites `bear_id` across all SQLite tables in one transaction). **`entity_id` is bear-local and opaque — never rewritten**, so every relation stays valid after import; nothing dangles.
- **`canonical_ref` re-linking (new import step).** `canonical_ref` points at the *source* Den/Cabinet/calendar registries and is meaningless on another host. On import: keep `entity_id` stable; re-resolve each entity's `canonical_ref` against the **destination** registries via its **strong handles** (`git_remote` → destination work surface/connection; `cabinet_ref`/`email`/`slack_user` → destination Cabinet/Den). Entities that re-resolve become `resolved`/`confirmed`; those that do not are **demoted to `provisional` with `canonical_ref` cleared** — never keep a stale cross-host canonical pointer. This mirrors the existing "rebuild derived indexes / re-provision bindings / remap models" pattern.
- **Access-bearing rules travel** (SQLite rows), so visibility gating survives import. After re-linking, any `memory_access_rules` whose target entity did not reach `resolved` is **inert until re-resolved** (§11 trust gate) — fail-closed, not fail-open.
- **Operator visibility on export.** Authoritative entity data is in `memory.sqlite`; the manifest may additionally carry an informational **entity summary** (counts by type, resolved vs provisional) so operators can see "what entities this Bear knows" without opening the DB. (Optional, informational.)

Deferred (unchanged): extracting the registries' *own* knowledge about an entity (Cabinet People content, etc.). The package carries the Bear's references and resolution, not Cabinet's graph.

## Schema deltas (sketch)

Canonical bear SQLite (`den-memory`). Final names/columns settle with D/E.

```sql
-- Bear entity layer (portable)
CREATE TABLE entities (
    entity_id      TEXT PRIMARY KEY,         -- bear-local stable id
    bear_id        TEXT NOT NULL,
    sequence_no    INTEGER NOT NULL,
    type           TEXT NOT NULL,            -- descriptor-owned vocabulary
    display_name   TEXT NULL,
    resolution     TEXT NOT NULL DEFAULT 'observed', -- observed|provisional|resolved|confirmed|rejected|merged
    trust          TEXT NOT NULL DEFAULT 'inferred', -- inferred|asserted
    canonical_ref  TEXT NULL,                -- cabinet:/den:/calendar: ref when resolved
    superseded_by_entity_id TEXT NULL,       -- forward pointer: dead (merged|superseded) -> live survivor
    metadata_json  TEXT NOT NULL DEFAULT '{}',
    created_at     TEXT NOT NULL
);

CREATE TABLE entity_handles (
    handle_id    TEXT PRIMARY KEY,
    bear_id      TEXT NOT NULL,
    entity_id    TEXT NOT NULL,
    handle_type  TEXT NOT NULL,              -- slack|email|cabinet_author|mention|git_remote|checkout|...
    handle_value TEXT NOT NULL,
    source       TEXT NULL,
    trust        TEXT NOT NULL DEFAULT 'inferred',
    state        TEXT NOT NULL DEFAULT 'active', -- active|detached
    created_at   TEXT NOT NULL
);

-- Relation layer: two descriptor-routed tables, same shape
CREATE TABLE memory_relations (         -- descriptive only
    link_id          TEXT PRIMARY KEY,
    bear_id          TEXT NOT NULL,
    sequence_no      INTEGER NOT NULL,
    src_memory_id    TEXT NOT NULL,
    entity_id        TEXT NOT NULL,
    relation         TEXT NOT NULL,         -- descriptor id; class=descriptive
    qualifiers_json  TEXT NOT NULL DEFAULT '{}',
    author_profile   TEXT NOT NULL,
    author_agent_id  TEXT NULL,
    confidence       TEXT NULL,
    state            TEXT NOT NULL DEFAULT 'active', -- active|retracted|superseded
    supersedes_link_id TEXT NULL,
    created_at       TEXT NOT NULL
);

CREATE TABLE memory_access_rules (      -- access-bearing only; audit surface
    -- identical columns to memory_relations; relation.class=access_bearing
    -- the ONLY table the recall gate consults
    link_id          TEXT PRIMARY KEY,
    bear_id          TEXT NOT NULL,
    sequence_no      INTEGER NOT NULL,
    src_memory_id    TEXT NOT NULL,
    entity_id        TEXT NOT NULL,         -- constraint: target entity resolution >= resolved
    relation         TEXT NOT NULL,
    qualifiers_json  TEXT NOT NULL DEFAULT '{}',
    author_profile   TEXT NOT NULL,
    author_agent_id  TEXT NULL,
    confidence       TEXT NULL,
    state            TEXT NOT NULL DEFAULT 'active',
    supersedes_link_id TEXT NULL,
    created_at       TEXT NOT NULL
);

-- Convenience read view for cross-cutting reads
CREATE VIEW memory_links AS
    SELECT *, 'descriptive'   AS class FROM memory_relations
    UNION ALL
    SELECT *, 'access_bearing' AS class FROM memory_access_rules;
```

- **Retire** `memory_records.entity_ref` (vestigial) and the legacy `memory_links` base table (`dst_ref_type`/`dst_ref`/`link_type`), migrating any future use to the two-table model.

## Consequences

- Per-user / per-entity memory delineation returns, generalized: persons, contacts, events, missions, orgs, work surfaces, and connections share one model, one lifecycle, one toolset, one anchor mechanism.
- Visibility gating is structural and auditable; misclassification of access-bearing relations is unrepresentable.
- New relationship and entity types are descriptor edits, not migrations; the design stays bounded (no RDF, no entity↔entity edges in memory).
- Work surfaces are unified into the entity model without a big-bang rewrite; the existing resolver is the first per-type resolver.
- Bear portability requires exporting the entity reference layer; Den must surface "entities this Bear knows" on export/import.
- New surface area to build: entity resolver(s), handle ingestion, merge/split tooling and audit, and entity-aware recall/anchor projection.

## Open questions

- **Naming** — `memory_access_rules` vs `memory_visibility` vs `memory_access_scopes`; view materialization (likely plain view).
- **Work-surface canonical home** — confirm externally-backed surfaces resolve through Connections only, and whether `work_surface_ref` is retired once `entities` lands.
- **Salience threshold** for promoting an entity from query-derived view to projected anchor.
- **ADR-0015 disposition** — supersede vs amend, once this lands.
