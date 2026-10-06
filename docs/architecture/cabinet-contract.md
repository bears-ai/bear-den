# Cabinet Contract

**Status:** Adopted; branch implementation now enforces page hierarchy, inherited membership, Bear-write policy and human version review through the facade. Legacy Phase 0 structs still retain rejected collection/Mission fields at compatibility boundaries; page-only helpers own the new state. Docket Mission annotations, snapshot citations and Phase 3 attachments remain pending. See the [implementation plan](../roadmap/CABINET_IMPLEMENTATION_PLAN.md).
**Related:** [Bear charter and Cabinet Missions](bear-charter-and-cabinet-missions.md), [ADR-0004 — Artifacts, Garage, and Cabinet separation](../decisions/adr-0004-artifacts-garage.md), [ADR-0008 — Research ingestion uses Cabinet](../decisions/adr-0008-cabinet-reading-pipeline.md), [Identity and membership](identity-and-membership.md)

This document is the provider-neutral contract for Cabinet: the typed identities, minimum records, Den facade operations, and authorization inputs/outcomes that every Cabinet implementation and client must honor. The backing provider (Den Postgres first; anything later) is an implementation detail behind this contract and must not leak into it.

## Summary

- Cabinet is Den's **single** shared knowledge layer: one Cabinet per Den deployment. It has exactly one structural concept — a **page tree**. Items nest under items; there are no separate collection or Mission containers.
- Humans and authorized Bears **edit directly** (true wiki). Every write produces an immutable version; revision history is the safety net. Optional page/ancestor policy can gate Bear writes on human review; direct publication remains the default.
- Den owns the facade, authorization, and policy. Agent tools and human UI both go through the facade; nothing reads or writes the backing store directly.
- Every operation takes an **explicit actor scope** (user or Bear). The Phase 1 Bear provenance still records a compatibility stance; native tool admission and write eligibility use verified origin and governance rather than that label. No ambient identity.
- Cabinet items are knowledge records. Artifact refs hold content payloads (ADR-0004). External sources stay external. Derived recall passages are rebuildable projections. These four never merge.

## Identities

All Cabinet refs are Den-minted, opaque, and stable for the entity lifetime. Following the artifact-ref convention, a ref is a fixed prefix plus a 32-character lowercase hex suffix. Models and clients never invent refs.

| Entity | Ref prefix | Example |
|--------|-----------|---------|
| Cabinet item | `cabinet_item_` | `cabinet_item_01f3…` |
| Item version | `cabinet_version_` | `cabinet_version_9ab0…` |
| Source link | `cabinet_source_` | `cabinet_source_be55…` |
| Attachment link | `cabinet_attachment_` | `cabinet_attachment_0c9f…` |
| Review record | `cabinet_review_` | `cabinet_review_d21a…` |

Notes:

- The protocol field name for an item ref is `cabinet_ref` (matches ADR-0004 and the existing `cabinet_ref` entity-handle type in `den-memory`).
- There is no Mission or collection ref. A **Mission is an item** — a page, optionally marked `kind: mission`, whose subtree is the grouping. Anything that needs to name a Mission names its `cabinet_ref`; this is what `den-memory`'s `mission` entity type (handle `cabinet_ref`, Cabinet-owned) already assumes.
- No ref is any other ref's prefix; parsing is unambiguous.
- A ref is not an object key, URL, filesystem path, title, or slug. Slugs/titles may exist for humans but are mutable display data, never identity.

## Records

Minimum required fields. Providers may store more; clients may rely only on what is here.

### Cabinet item

The durable knowledge object — a wiki document or typed knowledge record.

| Field | Requirement |
|-------|-------------|
| `cabinet_ref` | required, immutable |
| `kind` | required; `document` is the only Phase 1 kind. The enum is open (`mission`, `glossary_entry`, `decision`, `reference`, …) and is a **human-facing label only**: no operation branches on it, and nothing requires `kind: mission` to treat a page as a Mission |
| `title` | required, mutable display data |
| `current_version` | required after first write; points at the latest published `cabinet_version_` |
| `parent_item_ref` | optional (Phase 2); the parent page. Cycles are rejected; depth is capped |
| `position` | optional (Phase 2); explicit sort order among siblings |
| `path` | optional (Phase 2); provider-maintained materialized ancestor path. Derived — never client-supplied, never identity |
| `policy` | optional (Phase 2); see Authorization. Absent means "inherit from the nearest ancestor that sets one" |
| `user_members` / `bear_members` | optional (Phase 2); when set, access to this page and its subtree requires membership |
| `created_by` | required actor provenance (see Actor scope) |
| `created_at` | required |
| `lifecycle` | required: `active`, `archived`, `deleted` (tombstone) |

A page is the only container Cabinet has. A Mission is a page with a goal
description that may carry members and policy, and whose child pages (roadmap
plans, references, notes) are the grouped material. A Docket Job may name that
page's `cabinet_ref`; Cabinet stores no reference back to Jobs, so work
management never becomes a Cabinet concept.

### Item version

An immutable snapshot of item content. Versions are the citation unit and the revision history.

| Field | Requirement |
|-------|-------------|
| `version_ref` | required, immutable |
| `cabinet_ref` | required; the owning item |
| `revision` | required; per-item monotonically increasing integer starting at 1 |
| `content` | required; Phase 1 content is Markdown text |
| `content_sha256` | required; hash of the canonical content bytes |
| `authored_by` | required actor provenance |
| `authored_at` | required |
| `base_version` | required except on revision 1; the version the author edited from (concurrency evidence, not a merge mechanism) |
| `review` | required review state (see Review state) |

A finalized version is never mutated or deleted while any citation to it may exist. Item deletion tombstones the item; versions remain readable to actors authorized on the item at tombstone time, for citation integrity. Hard purge is an operator/compliance action outside this contract's model-facing operations.

### Page policy

Optional per-item access policy (Phase 2). A page without a policy inherits
the nearest ancestor's; a root page without one falls back to the open-wiki
default. Inheritance **narrows only** — a descendant may add restrictions and
can never grant access its ancestors withhold.

| Field | Requirement |
|-------|-------------|
| `bears_may_write` | required; when false, Bear actors get read only on this subtree |
| `review_required` | required; when true, Bear-authored versions land `pending` instead of publishing (Phase 2) |
| `allowed_kinds` | optional; restricts the `kind` of pages creatable in this subtree. `None` allows all |

Membership lives on the item (`user_members` / `bear_members`) rather than in
the policy record: when either set is non-empty on a page, access to that page
and its whole subtree requires membership. This is how a Mission governs its
material without widening access to unrelated Cabinet pages.

The effective policy for a page is resolved by walking ancestors. Providers
should maintain the derived `path` so resolution is a single indexed lookup
rather than a per-check recursive query; `path` is an implementation aid, never
identity and never client-supplied.

### Source link

Provenance from a Cabinet item to material outside Cabinet. This is how research ingestion (ADR-0008) and manual citation attach origin without Cabinet owning external data.

| Field | Requirement |
|-------|-------------|
| `source_ref` | required, immutable |
| `cabinet_ref` | required; the citing item |
| `source_kind` | required: `url`, `offline`, `artifact`, `conversation`, `external_record` |
| `locator` | required; normalized URL, synthetic scheme (`book://isbn/…`, `offline://…`), `artifact_…` ref, conversation ID, or provider-record identity — matching `source_kind` |
| `role` | required: `origin`, `citation`, `related` |
| `created_by`, `created_at` | required provenance |

A source link is provenance, not content. Cabinet never fetches, caches, or owns the bytes behind a `url` or `external_record` locator.

### Attachment link

Binding from a Cabinet item to a Den artifact ref (ADR-0004 §9). The local implementation uses the registry's existing `artifact_links` rows with `target_kind = cabinet_item`; `CabinetAttachmentRef` deterministically wraps the link ID, not another independently writable attachment store.

| Field | Requirement |
|-------|-------------|
| `attachment_ref` | required, immutable |
| `cabinet_ref` | required |
| `artifact_ref` | required; a finalized Den artifact ref |
| `role` | required: `source_pdf`, `generated_report`, `image`, `data`, `other` (open enum) |
| `created_by`, `created_at` | required provenance |

Cabinet owns item/ACL policy for the link. The artifact registry owns payload identity, lifecycle, and read authorization of the bytes. Linking never copies content and never exempts the reader from artifact read policy. Human artifact reads require current membership in the artifact's Bear plus its visibility policy; Bear reads require the same Bear and Bear-visible content. Unreadable attachments are omitted, including metadata/counts. Linking requires finalized readable content; detach requires page write authority and the exact page/link identity.

Registry links of kind `cabinet_item` or `cabinet_snapshot` retain payloads. Database triggers refuse artifact deletion or transition to deleted/expired while retained, and GC candidates exclude them. Page tombstones do not release retention; ordinary attachments can be detached, while snapshot retention release is operator-only. This can block Bear deletion via cascades as well. No separate writable retention status duplicates link state.

Docket owns optional Job→page annotations. Capturing an exact published page version creates a private `cabinet_document_snapshot` artifact with page/version refs, captured title, content and hash, plus an immutable snapshot citation link; the web capture transaction also attaches Job source evidence. A captured private copy remains readable to its authorized creator after source-page access changes or deletion. It neither promotes audience nor changes Job state. Job download authorization checks Job visibility, its registry link and artifact access independently. Model-facing attachment/capture tools remain pending.

Human file uploads use a two-phase web/facade workflow. An authenticated human with active-page write access selects a Bear with current membership; uploads are `same_user` unless explicitly acknowledged as Bear-visible. `den-service::cabinet::uploads` mints a non-deserializable admission receipt for a pending artifact with exact server-computed size/hash and page/user provenance. The web storage boundary performs bounded internal PUT/read-back verification without holding database locks. Publication rechecks active-page write access under the Cabinet fence and locks current Bear membership, then finalizes and inserts the existing registry link atomically. Pending, failed or corrupted uploads never become readable attachments. Cleanup changes only a still-pending row before deleting bytes, preventing destruction after an ambiguous successful commit. No blob key or signed URL is browser-facing. Uploads are capped at 16 MiB and preserve a sanitized filename; retained uploads ignore their 24-hour ephemeral deadline. Registered Cabinet-upload recovery is implemented as described below; general agent upload tools, bucket-wide orphan recovery and live Garage verification are not included.

Human attachment inspection is a read projection over the same page/link/registry authorization. It shows selected metadata, not raw provenance/metadata, storage keys or signed URLs. MIME declarations are parsed once into a conservative preview enum: escaped UTF-8 text/JSON (256 KiB visible cap), signature-checked PNG/JPEG/GIF/WebP/PDF, or download-only. HTML, JavaScript and SVG are never active inline documents; Markdown remains escaped source. Binary preview responses reauthorize independently, verify the same bounded full bytes as downloads, and apply no-store/nosniff/no-referrer plus `default-src 'none'; sandbox; frame-ancestors 'self'`. PDF frames are additionally sandboxed and always offer a download fallback. Access revoked between inspection and a byte request, or during transfer, prevents the bytes being returned. This adds no state owner, persistence, model tool or approval grant; live Garage and native browser rendering remain unverified.

Registered Cabinet-upload recovery is registry-owned. `expires_at` is the canonical pending write lease and later ephemeral deadline; signing revalidates the pending row, clamps the PUT lifetime to that lease, and publication locks/rechecks it. A bounded worker waits a 20-minute grace, excludes every `cabinet_item`/`cabinet_snapshot` retainer, and retires due records under `FOR UPDATE SKIP LOCKED` before issuing external DELETE. Only server-derived canonical keys receive a non-deserializable cleanup ticket; stored-key mismatches are refused. `content_removed_at` separately records acknowledged physical-key removal, constrained to terminal lifecycle states, without duplicating logical lifecycle or retention. Failed I/O/unacknowledged deletion remains retryable; acknowledgement is idempotent and audit rows remain. No network I/O holds the Cabinet fence or a database transaction. The owner-only upload-history projection requires current Bear membership, hides unreadable source refs/titles, and offers safe retry without granting access to retired content. This slice was validated in an isolated database and is not a deployed or bucket-wide GC claim.

### Review state

Phase 1 is direct-edit: every version publishes immediately with `review: none`. The state exists so Phase 2 review policy has a place to land without a schema break, and so the existing deferred `cabinet_update` curate action has a target.

| State | Meaning |
|-------|---------|
| `none` | published directly; no review required by policy at write time |
| `pending` | version exists but is not `current_version`; awaiting review (Phase 2) |
| `approved` | reviewed and published (Phase 2) |
| `rejected` | reviewed and not published; retained in history (Phase 2) |

A `cabinet_review_` record (reviewer actor, decision, rationale, timestamps) accompanies any transition out of `pending`. Phase 1 implementations must reject attempts to create `pending` versions rather than silently publishing them.

## Actor scope

Every facade operation takes an explicit `ActorScope`:

- exactly one of `user_id` (human) or `bear_id` plus a compatibility `stance` provenance label (Bear), and
- optional call provenance: `conversation_id`, `run_id`, `task_id` when the write originates from a run.

There are no service-identity or wildcard actors on the model-facing facade. Ingestion services (ADR-0008) act as the Bear or user they are configured to publish for.

Actor provenance recorded on items, versions, and links preserves this scope verbatim: `{ actor_kind: user|bear, user_id?, bear_id?, stance?, conversation_id?, run_id? }`. Native Den tool dispatch derives that label from verified origin; the label is not a write grant. The Phase 1 facade still checks the Bear-wide Cabinet enablement and contract rules, not live hat-specific Cabinet action grants.

## Operations

The Den facade exposes these operations. Signatures are conceptual; transport (tool descriptor, HTTP route) is an implementation concern, but names, inputs, outputs, and authority requirements are contract.

| Operation | Phase | Authority | Behavior |
|-----------|-------|-----------|----------|
| `cabinet_search(scope, query, filters?) → [item summary]` | 1 | read | Metadata/text search over items the actor may read. Filters: `kind`, `lifecycle`, and (Phase 2) `under` — restrict to a page's subtree. Results carry `cabinet_ref`, `current_version`, title, kind, parent, and updated-at. Unreadable items are absent, not redacted. |
| `cabinet_read(scope, cabinet_ref, version_ref?) → item + version` | 1 | read | Returns the item record and the requested version (default `current_version`), including full content, provenance, source links, and (Phase 3) attachment links. |
| `cabinet_history(scope, cabinet_ref) → [version summary]` | 1 | read | Revision list: `version_ref`, revision, author provenance, timestamp, review state, content hash. |
| `cabinet_create_item(scope, kind, title, content, parent_item_ref?, source_links?) → item + version 1` | 1 | write | Creates the item and its first published version atomically. `parent_item_ref` is Phase 2; reparenting is `cabinet_organize`. |
| `cabinet_update_item(scope, cabinet_ref, content, base_version, title?) → new version` | 1 | write | Appends a new immutable version and advances `current_version`. If `base_version` ≠ `current_version` at commit, fail with a structured conflict carrying the current version ref — the caller re-reads and reconciles; the facade never merges. |
| `cabinet_archive_item(scope, cabinet_ref)` / `cabinet_restore_item` | 1 | write | Lifecycle transitions `active ↔ archived`. Reversible; every revision stays readable. From Phase 2 archiving **cascades to descendants**, and restoring restores only the named page (its children stay archived until restored explicitly). |
| `cabinet_delete_item(scope, cabinet_ref)` | 1 | write, **person actors only** | Tombstones the item (`deleted`): it leaves search, read, and history. Versions are retained per the version-immutability rule so existing citations keep their meaning. A Bear actor is refused regardless of stance — a Bear's most destructive available act is a reversible archive. From Phase 2 deletion is **refused while the page has non-deleted children**: subtree removal must be done deliberately, leaf-first, not as one cascading act. Hard purge stays an operator/compliance action outside this contract. |
| `cabinet_link_source(scope, cabinet_ref, source_kind, locator, role)` / `cabinet_unlink_source` | 1 | write | Manage source links. Adding or removing provenance never publishes a revision and never alters versions. |
| `cabinet_organize(scope, cabinet_ref, parent_item_ref?, position?)` | 2 | write on the page **and** on the destination parent | Reparent and/or reorder a page. Rejects cycles (a page may not become its own descendant) and depth beyond the cap. Moving a subtree moves its descendants with it, and their effective policy changes to the destination's — so the mover must hold write authority at the destination, not only at the source. A `parent_item_ref` of `null` promotes the page to a root. Contract-reserved; not exposed in Phase 1. |
| `cabinet_review(scope, cabinet_ref, version_ref, decision, rationale) → review record` | 2 | review | Approve/reject a `pending` version; approval advances `current_version`. Contract-reserved. |
| `cabinet_link_attachment(scope, cabinet_ref, artifact_ref, role)` / `cabinet_unlink_attachment` | 3 | write + artifact read | Manage attachment links. Requires the actor to hold artifact read authority at link time. Contract-reserved until artifact-ref content transfer lands. |

Error taxonomy (structured, stable): `NotFound` (also returned for unauthorized reads — deny must not confirm existence), `NotAuthorized` (writes only, where existence is already readable), `Conflict` (stale `base_version`), `ValidationError`, `PolicyError` (operation valid but disallowed by the effective page policy — including a cycle, depth-cap, or delete-with-children refusal).

## Authorization

### Inputs

An authorization decision consumes, in order:

1. **Actor identity** — the explicit `ActorScope`.
2. **Den membership** — the user's or Bear's standing in this Den ([identity-and-membership.md](identity-and-membership.md)). Non-members get nothing.
3. **Page membership** — resolved from the page and its ancestors. When any ancestor (or the page itself) sets `user_members`/`bear_members`, the actor must be a member for any access to that subtree. Membership narrows access; it never widens access to pages outside the subtree.
4. **Effective page policy** — the nearest ancestor policy, which may further restrict: read-only for Bears, write requires review (Phase 2), specific kinds disallowed. Descendant policy may add restrictions, never remove them.
5. **Requested authority** — `read`, `write`, or `review`.

Steps 3 and 4 resolve over the ancestor chain, so a page's access is never
looser than any page above it. Phase 1 implements steps 1–2 only, as a blanket
capability check.

### Outcomes

The decision is `allow` or `deny` with a structured, logged reason. Rules:

- **Default for pages with no policy anywhere above them**: readable and writable by every Den member — the open-wiki default. Deployments wanting a stricter default set a policy on their root pages rather than relying on a special case.
- **Pages under membership**: read and write require membership (user or Bear) on the governing page. This is the plan's Phase 2 exit condition — a Mission governs its own material without broadening access to unrelated Cabinet pages.
- **Bears are members, not superusers**: a Bear's access derives from the same ancestor chain a person's does. Verified execution origin, governance, and future page policy may narrow an action; a stored stance label never widens it.
- **Search and read denial is silent**: filtered from search, `NotFound` on read. A page whose parent is unreadable is itself unreadable, and its existence is not disclosed through the tree.
- Every mutating decision (allow or deny) is auditable: actor scope, operation, target refs, resolved policy inputs, outcome.

## Distinctions (what Cabinet is not)

| Handle | Owner | Nature |
|--------|-------|--------|
| `cabinet_item_` / `cabinet_version_` | Cabinet | curated knowledge record and its immutable revision |
| `artifact_…` | Artifact registry (ADR-0004) | content payload/blob/external snapshot with provenance |
| Source locator (URL, `book://…`) | The external world | origin provenance; never Cabinet-owned bytes |
| Derived recall passage | Recall index (ADR-0038) | rebuildable, ACL-filtered projection; never canonical |

Consequences:

- Citing a Cabinet item from elsewhere in Den (Docket evidence, conversations) uses an artifact of kind `cabinet_document_snapshot` recording `(cabinet_ref, version_ref)` — per ADR-0004 §4.
- Recall passages derived from Cabinet content carry `(cabinet_ref, version_ref)` provenance and are filtered by Cabinet read authority at query time. Deleting or archiving an item must propagate to derived passages (Phase 3).
- Bear memory tools never write Cabinet, and Cabinet operations never write Bear memory. The existing `memory_write_entry` rejection of `cabinet_write` content stands.

## Invariants

1. Den mints all Cabinet refs; models, clients, and providers do not.
2. Every operation carries an explicit actor scope; there is no ambient or default actor.
3. Every item version is immutable, hash-stamped, and citable forever; content changes are new versions.
4. `current_version` only moves forward through `cabinet_update_item` or (Phase 2) an approved review.
5. Every record carries actor provenance sufficient to answer who wrote this, as whom, from where.
6. Authorization is evaluated on every operation against current membership and policy — never cached across actors, never bypassed by provider access.
7. A page's effective access is never looser than any of its ancestors': membership and policy narrow down the tree and never widen. Unauthorized existence is not disclosed, through the tree or otherwise.
8. The page tree is acyclic and depth-capped; `path` is derived and never accepted from a client.
9. Cabinet stores no artifact bytes and no external-source bytes.
10. Cabinet holds no work-management state: a Docket Job may name a page's `cabinet_ref`, but Cabinet stores no Job, task, or run identity.
11. Provider changes must not change refs, operation semantics, authority outcomes, or provenance fields.

## Contract checks

Phase 0 exits with an assertion-style check suite (Rust tests colocated with the contract types) that enforces, independent of any provider:

- ref mint/parse round-trips for every prefix, and rejection of malformed or cross-kind refs;
- every operation input type fails construction without an actor scope;
- item, version, source-link, and attachment-link records fail validation when identity, provenance, scope, or authority fields are missing;
- version construction rejects a missing `base_version` after revision 1 and any mutation of a finalized version;
- Phase 1 review-state handling rejects `pending` creation;
- (Phase 2) effective-policy resolution over an ancestor chain narrows monotonically — a descendant policy can never produce a broader outcome than its ancestors — and reparenting rejects cycles and depth-cap violations.

## Phase applicability

| Contract element | Phase 0 (types + checks) | Phase 1 (facade) | Later |
|------------------|--------------------------|------------------|-------|
| Item, version, source link records | defined | implemented | — |
| Search/read/create/update/history/archive/delete/source ops | defined | implemented | — |
| Page tree (`parent_item_ref`, `position`, `path`) | defined | rejected | implemented (2) |
| Page policy + membership, ancestor resolution | defined | blanket capability check only | implemented (2) |
| Organize (reparent/reorder), review ops + review states beyond `none` | defined | rejected | implemented (2) |
| Attachment links | defined | rejected | links/detach/download and bounded human uploads implemented locally (3); recall pending |
| Recall passage handoff | distinction defined | — | planned (3), not implemented |

## Documentation obligations

Implementing Phase 1 against this contract requires updating `MODEL_EXPERIENCE.md` (new model-visible tools and their guidance) and creating a `docs/guides` entry covering permissions, direct-edit behavior, revision history, and source-link limitations, per the implementation plan's standing obligation.
