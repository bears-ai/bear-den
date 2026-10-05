# Cabinet implementation plan

**Status:** In progress — Phase 0 landed (contract doc + `den-cabinet` types/checks); Phase 1 implemented (Postgres storage + `den_service::cabinet` facade, `cabinet_search/read/history/create/update/source_link/lifecycle` tools with the per-Bear `bears.cabinet_enabled` gate, person-only item deletion, `/cabinet` wiki UI, [user guide](../guides/cabinet.md)); Phase 2 page hierarchy, inherited access, Bear-write review, web administration, Docket Mission annotations and private immutable document copies are implemented in the local branch. Phase 3 finalized-artifact linking/detach, retention and permission-rechecking downloads are implemented; new-file uploads and derived recall remain open. Phase 4 remains open.
**Scope:** Shared, human-editable, policy-controlled knowledge for Den.
**Decisions (2026-08):** one Cabinet layer per Den; humans and authorized Bears edit directly (true wiki — every write publishes an immutable version, revision history is the safety net); review/approval is Phase 2 policy, not a Phase 1 gate; Phase 1 storage is Den Postgres behind the provider-neutral facade.
**Decision (2026-09) — one structural concept:** Cabinet has a **page tree** and nothing else. There are no collection or Mission containers: a Mission *is* a page (optionally `kind: mission`) carrying a goal description, whose child pages are the grouped material. Access policy and membership live on pages and inherit down the tree, narrowing only. A Docket Job may name a page's `cabinet_ref`; Cabinet stores no Job identity, so work management never leaks into the knowledge layer.
**Related architecture:** [Cabinet contract](../architecture/cabinet-contract.md), [Bear charter and Cabinet Missions](../architecture/bear-charter-and-cabinet-missions.md), [ADR-0004 — Artifacts, Garage, and Cabinet separation](../decisions/adr-0004-artifacts-garage.md)

## Boundary

Cabinet is Den's shared knowledge layer: people and authorized Bears can create, read, organize, review, and update durable knowledge. Its one structural concept is a page tree; a Mission is a page whose subtree groups related material, not a separate container or storage system.

Cabinet is **not** the reading stack, a bookmark manager, a highlight store, or a vector database. The reading/research pipeline is a Cabinet client and source producer; its delivery belongs to [Cabinet research ingestion](CABINET_RESEARCH_INGESTION_PLAN.md). Artifact payload retention belongs to the [artifact refs plan](ARTIFACT_REFS_IMPLEMENTATION_PLAN.md), and derived retrieval belongs to the [derived recall plan](DERIVED_RECALL_INDEX_IMPLEMENTATION_PLAN.md).

Den owns the agent-facing facade, authorization, and policy. The backing provider is an implementation decision behind that boundary; no active plan assumes Outline, Karakeep, or another provider is Cabinet itself.

## Goals

- Give humans a durable, editable shared-knowledge surface.
- Give authorized Bears a bounded Den-mediated API for finding, reading, creating, and updating Cabinet material.
- Support pages, page hierarchy, source links, metadata, and artifact attachments without conflating them.
- Preserve provenance, review state, authorization, and immutable citations/snapshots where needed.

## Non-goals

- Replacing per-Bear canonical memory.
- Implementing research/reader synchronization or owning third-party reading data.
- Defining artifact blob storage or derived-vector indexing internals.
- Selecting a provider before the Cabinet contract and constraints are validated.

## Phases

### Phase 0 — Contract and boundary

- Define strongly typed identities and minimum records for Cabinet item (page), page hierarchy/policy, source link, attachment link, review state (reserved for Phase 2 policy), and immutable item version/snapshot.
- Define the distinction between a Cabinet catalog/item, artifact ref, external source, and derived recall passage.
- Specify Den operations: search, read, history, direct create/update (each write publishes an immutable version), archive/restore, delete, and source link/unlink, plus contract-reserved organize (reparent/reorder), review/approve, and attachment link/unlink operations; all accept an explicit Bear/user actor scope rather than special strings.
- Define authorization inputs and outcomes: user membership, Bear membership, page membership and policy inherited over the ancestor chain, and read/write/review authority.

**Exit:** a provider-neutral API/schema contract and an assertion-style contract check for required IDs, scope, provenance, and authority fields.

### Phase 1 — Den facade and minimal shared knowledge

- Implement provider-neutral Cabinet storage behind Den's policy facade (Den Postgres first).
- Add authorized search/read and direct create/update flows for a minimal document/item type: humans and authorized Bears both edit directly, every write appends an immutable version, and stale-base updates fail with a structured conflict rather than merging.
- Record author, source/provenance, revision identity, and review state (`none` under Phase 1 direct-edit; `pending` versions are rejected until Phase 2).
- Keep direct backing-store access out of agent tools.

**Exit:** a human and an authorized Bear can share one item through Den, while an unauthorized Bear cannot read or alter it.

### Phase 2 — Page hierarchy, access policy, and review

- Add the page tree: `parent_item_ref` (cycle-rejecting, depth-capped), `position` for explicit sibling order, and a provider-maintained `path` so policy resolution stays a single indexed lookup.
- Add `cabinet_organize` (reparent/reorder), requiring write authority on the page **and** on the destination parent, since a move changes the subtree's effective policy.
- Add per-page access policy and membership with ancestor inheritance that narrows only: replace Phase 1's blanket capability check with the contract's full decision chain (actor → Den membership → page membership → effective policy → authority).
- Cascade archive to descendants; refuse deletion while a page has non-deleted children, so subtree removal stays deliberate and leaf-first.
- Add optional review/approval policy for Bear-authored changes where configured; direct edit remains the default.
- Add human-visible hierarchy/provenance views (breadcrumbs, child lists, revision provenance) and stable snapshot citations via `cabinet_document_snapshot` artifacts recording `(cabinet_ref, version_ref)`.
- Let a Docket Job optionally name a Mission page's `cabinet_ref`. The reference points Docket → Cabinet only.

**Exit:** a Mission page can govern its own subtree without broadening access to unrelated Cabinet pages, and a Job can name the page whose subtree documents it.

**Local implementation evidence (2026-10-05):** `den-docket::missions` owns a Job's optional page annotation with optimistic revision; `den-service::cabinet::snapshots` captures an exact published version into a private `cabinet_document_snapshot` artifact linked as Job source evidence. Job detail lists readable saved copies and serves scoped downloads, without changing Job status or Work audience. `den-web/src/work/tests/knowledge.rs` checks stale annotations, hidden page refs/titles, cross-Job downloads, immutable version/hash/content and retention after page deletion.

### Phase 3 — Attachments and recall integration

- Integrate Cabinet attachments through artifact refs; Cabinet retains page/ACL policy while artifacts retain payload identity and lifecycle.
- Produce access-filterable passages for the derived recall index; recall remains derived and rebuildable.
- Do not begin this phase until the artifact-ref and recall contracts are available.

**Exit:** an authorized Cabinet item can cite an immutable attachment/version and contribute filtered recall passages without becoming a blob store.

**Partial delivery (2026-10-05):** page attachments use existing registry `artifact_links`, deterministic typed attachment refs and independent page/artifact authorization. Cabinet links and snapshot citations prevent deletion/expiration/GC; ordinary page links can be detached, while snapshot retention release remains operator-only. Downloads support JSON and configured Garage content (16 MiB limit, size/hash verification, no browser-visible storage URL). New-file upload and recall passage production are not implemented; the combined Phase 3 exit is not met. See the [artifact plan](ARTIFACT_REFS_IMPLEMENTATION_PLAN.md#phase-4--cabinet-attachment-integration) and [guide](../guides/cabinet.md#attachments-and-private-copies).

### Phase 4 — Provider selection, migration, and operations

- Select or implement a provider only after Phase 1 validates the facade contract.
- Document migration, backup/export, audit, retention, operational ownership, and failure behavior.
- Keep provider-specific adapters behind the Phase 0 contract.

**Exit:** provider changes do not change model tool semantics, authority checks, or Cabinet identities.

## Documentation obligations

Every phase that changes model-visible Cabinet behavior must review and update `MODEL_EXPERIENCE.md`. Every user- or operator-visible phase must create or update the applicable documentation in `docs/guides`, including permissions, hierarchy and review behavior, and any source or attachment limitations.
