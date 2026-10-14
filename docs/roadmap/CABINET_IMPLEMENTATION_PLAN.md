# Cabinet implementation plan

**Status:** In progress — Phase 0 landed (contract doc + `den-cabinet` types/checks); Phase 1 implemented (Postgres storage + `den_service::cabinet` facade, `cabinet_search/read/history/create/update/source_link/lifecycle` tools with the per-Bear `bears.cabinet_enabled` gate, person-only item deletion, `/cabinet` wiki UI, [user guide](../guides/cabinet.md)); Phase 2 page hierarchy, inherited access, Bear-write review, web administration, Docket Mission annotations and private immutable document copies are implemented in the local branch. Phase 3 artifact linking/detach, retention, permission-rechecking downloads and bounded human uploads with configured S3 storage are implemented; derived recall remains open. Phase 4 remains open. Phase 5 is in progress: the branch implements Cabinet-only startup, distinct navigation/cross-host links, per-page discussion/history and an initial read-only MCP adapter; root Compose packaging, production image and isolated local-HTTPS acceptance are validated; actual public-host rollout and native-browser/live-provider acceptance remain open. Cabinet's domain and database remain in Den.
**Scope:** Shared, human-editable, policy-controlled knowledge for Den, with a perceptually distinct human and external-agent surface.
**Decisions (2026-08):** one Cabinet layer per Den; humans and authorized Bears edit directly (true wiki — every write publishes an immutable version, revision history is the safety net); review/approval is Phase 2 policy, not a Phase 1 gate; Phase 1 storage is Den Postgres behind the provider-neutral facade.
**Decision (2026-09) — one structural concept:** Cabinet has a **page tree** and nothing else. There are no collection or Mission containers: a Mission *is* a page (optionally `kind: mission`) carrying a goal description, whose child pages are the grouped material. Access policy and membership live on pages and inherit down the tree, narrowing only. A Docket Job may name a page's `cabinet_ref`; Cabinet stores no Job identity, so work management never leaks into the knowledge layer.
**Surface direction (2026-10-13, planned):** Cabinet should feel like a distinct service to people and external agent runtimes: its own hostname, Cabinet-focused navigation, and a dedicated container. This is a surface/deployment separation, not extraction from Den: reuse the Den workspace's Cabinet implementation, authorization, identity and canonical Postgres database. Bear administration remains Cabinet-aware, but live Cabinet page details and content manipulation belong to the Cabinet surface.
**Related architecture:** [Cabinet contract](../architecture/cabinet-contract.md), [Bear charter and Cabinet Missions](../architecture/bear-charter-and-cabinet-missions.md), [ADR-0004 — Artifacts, Garage, and Cabinet separation](../decisions/adr-0004-artifacts-garage.md)
**Topic home:** [Cabinet guide](../guides/cabinet.md) (current behavior and linked planned surface).

## Boundary

Cabinet is Den's shared knowledge layer: people and authorized Bears can create, read, organize, review, and update durable knowledge. Its one structural concept is a page tree; a Mission is a page whose subtree groups related material, not a separate container or storage system.

Cabinet is **not** the reading stack, a bookmark manager, a highlight store, or a vector database. The reading/research pipeline is a Cabinet client and source producer; its delivery belongs to [Cabinet research ingestion](CABINET_RESEARCH_INGESTION_PLAN.md). Artifact payload retention belongs to the [artifact refs plan](ARTIFACT_REFS_IMPLEMENTATION_PLAN.md), and derived retrieval belongs to the [derived recall plan](DERIVED_RECALL_INDEX_IMPLEMENTATION_PLAN.md).

Den owns the agent-facing facade, authorization, and policy. The backing provider is an implementation decision behind that boundary; no active plan assumes Outline, Karakeep, or another provider is Cabinet itself.

The planned Cabinet container is another narrow composition of this same Den implementation. Its human UI and MCP adapter call the shared Cabinet facade with authenticated, explicit actor scope; existing Den Bear tools continue to use that facade. Sharing the canonical Den Postgres database is intentional. This does not require a new Cabinet HTTP service between internal callers and the facade, a separately owned database, or an MCP-based internal application protocol. A dedicated container is the end goal, not merely a second hostname proxying the full Den web app.

## Goals

- Give humans a durable, editable shared-knowledge surface.
- Give authorized Bears a bounded Den-mediated API for finding, reading, creating, and updating Cabinet material.
- Give people a distinct Cabinet hostname and knowledge-focused UI, deployed in a dedicated container without extracting the shared Den domain implementation.
- Let other agent runtimes interact through an authenticated MCP adapter over the same operations, policy and provenance rules.
- Keep Bear administration Cabinet-aware without duplicating Cabinet detail views or content-editing workflows.
- Support pages, page hierarchy, source links, metadata, and artifact attachments without conflating them.
- Preserve provenance, review state, authorization, and immutable citations/snapshots where needed.

## Non-goals

- Replacing per-Bear canonical memory.
- Implementing research/reader synchronization or owning third-party reading data.
- Defining artifact blob storage or derived-vector indexing internals.
- Selecting a provider before the Cabinet contract and constraints are validated.
- Extracting Cabinet into an autonomous data/identity service, duplicating policy or state, or creating a second Cabinet per Den.
- Running a second full Den runtime or sharing canonical Bear SQLite files between processes to support the Cabinet surface.

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

**Partial delivery (2026-10-05):** page attachments use existing registry `artifact_links`, deterministic typed attachment refs and independent page/artifact authorization. Cabinet links and snapshot citations prevent deletion/expiration/GC; ordinary page links can be detached. Snapshot release was then operator-only; the code/test retirement follow-on below adds the creator-owned workflow. Downloads support JSON and configured Garage content (16 MiB limit, size/hash verification, no browser-visible storage URL). The 2026-10-06 follow-on adds bounded private-by-default human uploads, internal storage write/read-back integrity verification and atomic publication/linking after fresh page/Bear-membership authorization. Database-backed route tests use an S3 byte-store substitute and cover unavailable storage, private/explicit-sharing audiences, pending readability, transfer/integrity failures, archive and access revocation, cleanup and replay. A subsequent read-only inspection slice adds content-first attachment pages, selected metadata behind Details, bounded escaped text/JSON/Markdown/diffs, signature-checked raster-image/PDF byte responses, sandboxing and download/unavailable fallbacks. Six database-backed preview route tests cover privacy, MIME confusion, independent asset requests, transfer-time revocation and UTF-8 bounds; the full 130-test web suite passes. Registered-upload recovery is now implemented/tested in an isolated validation database: owner-only history/retry, canonical lease checks, a bounded post-grace reaper and durable physical-removal acknowledgement, with Cabinet page/snapshot retention preserved. All 136 web tests and offline production checking pass from an exact staged snapshot; migration default/constraint/index checks and populated rollback/reapply pass. The new migration has not been applied to the running Den and the recovery slice is not deployed. Live Garage verification, native browser rendering and general/bucket-wide orphan recovery remain open. Recall passage production is not implemented, so the combined Phase 3 exit is not met. See the [artifact plan](ARTIFACT_REFS_IMPLEMENTATION_PLAN.md#phase-4--cabinet-attachment-integration) and [guide](../guides/cabinet.md#attachments-and-private-copies).

**Code-only shared file consumption:** `cabinet_read` now lists safe current-page attachment citations and accepts an attachment selector for bounded UTF-8 text/JSON ranges. Independent same-Bear shared-file access, page policy, enablement, link and native source rechecks remain in force; private human uploads/foreign Bear files do not leak. Transfer integrity is verified before slicing and storage is injected/reuses the existing reader. No new provider tool, dependency, schema migration or server configuration/deployment is introduced. PDF/image extraction and the broader hat grant resolver remain open. The combined recall exit above remains unmet.

**Code/test retained-evidence lifecycle follow-on:** creator-authorized **Your saved copies** retirement and a fresh-inventory Bear deletion preview now resolve eligible simple-copy retention without granting private readers, purging required evidence, or erasing payloads as part of retirement. Original provenance stays immutable with a receipt while the Bear survives; full Bear deletion is a separately confirmed destructive action. Canonical completion proof/state, source rows, reverse foreign references and links are fenced, and retired copies cannot acquire new references or reopen audit. 40 focused retirement/migration tests, five Bear-deletion tests and all 326 web tests pass; strict workspace Clippy and a production/musl image build pass. Schema changes are isolated-validation only; populated receipts intentionally block downgrade before guards unwind. This does not implement derived recall, general byte/version cleanup or live storage/provider acceptance, and does not complete the combined Phase 3 exit. See [Docket lifecycle details](../topics/docket.md#saved-copy-retirement-and-deletion-resolution).

### Phase 4 — Provider selection, migration, and operations

- Select or implement a provider only after Phase 1 validates the facade contract.
- Document migration, backup/export, audit, retention, operational ownership, and failure behavior.
- Keep provider-specific adapters behind the Phase 0 contract.

**Exit:** provider changes do not change model tool semantics, authority checks, or Cabinet identities.

### Phase 5 — Distinct Cabinet surface and external MCP

**Status:** In progress, code/test delivery only. `den cabinet-serve` now provides the narrow composition, distinct UI and initial read-only MCP; discussion/history are implemented below. The root Compose service is approved and implemented, with production image and isolated local-HTTPS acceptance validated. Actual public-host/TLS rollout and native-browser/live-provider acceptance remain open. Den's unconfigured `/cabinet` remains the local fallback. The narrow entrypoint, not the existing `RUN_WEB`/`RUN_API`/`RUN_WORKERS` switches, selects Cabinet. This phase proceeds independently of recall/provider migration.

#### 5a — Narrow composition and dedicated deployment

- Add a typed Cabinet-only startup composition in the Den workspace, reusing Cabinet contract/facade/storage code rather than forking it. Whether packaged as a dedicated entrypoint or a mode of the existing image is an implementation choice; the runtime boundary must be explicit and testable.
- Deploy that composition in a dedicated Cabinet container with its own public hostname. Use the same canonical Den Postgres database and migration set, preserving page/version refs, memberships, policy, artifact links and transaction semantics. No data copy or provider migration is required for the surface split.
- Do not initialize Bear turns, BearWire, chat, Work dispatch, reflection, sandbox infrastructure or `MemoryStoreManager` in the Cabinet-only process; do not mount/open canonical Bear SQLite files there. Disabling workers in a second full Den process is not an acceptable substitute. Preserve the [memory write topology](../guides/infrastructure-and-ops.md#memory-write-topology).
- Keep migrations under the existing deploy-time migration owner; the Cabinet process verifies schema compatibility rather than independently evolving it. Coordinate Den/Cabinet release compatibility and budget both processes' Postgres connections.
- Assign one canonical owner for Cabinet upload recovery and other background work; keep the existing owner until explicitly moved. A new surface must not accidentally start another general Den worker set or duplicate cleanup scheduling/state. Reuse existing artifact/storage services and configuration for authorized byte access.
- Keep deployment compatible with the single root Compose file. Explicit approval was granted on 2026-10-13: `bears-cabinet` is implemented behind profile `cabinet`, shares `DEN_IMAGE`/database/origin, mounts no native volumes and uses a read-only root with ephemeral `/tmp`. Den waits for Cabinet health when the profile is active; the optional dependency preserves the unconfigured local/remote rollout paths. The source-build override reuses Den's image build.

#### 5b — Cabinet identity, navigation and route ownership

- Provide a distinct Cabinet shell and landing page. At minimum its primary navigation is Cabinet-focused: browse/search and page tree, reviews, your uploads and saved copies as authorized. Do not inherit Den's Bear/chat/Work/operator rail. An explicit return link to Den/Bear administration is appropriate; separate navigation does not grant separate authority.
- Make the Cabinet surface canonical for live page content/detail views, history, creation/editing, organization, page membership/policy, version review, lifecycle, source links and attachment upload/inspection/download. Reuse existing handlers/application operations beneath surface-specific routing and templates; do not keep two independently maintained Cabinet workflows.
- Include the supporting login/logout/account handoff, assets and attachment byte routes needed to complete those workflows on the Cabinet origin. Route mounting and the hostname root landing path are implementation choices, not new page identity schemes.
- Use one configured Cabinet public origin and a shared typed URL resolver for cross-surface links, including page/version/review/attachment links and saved-copy remediation. Bear management, Docket Mission links, review entry points and deletion blockers must link to the correct authorized Cabinet destination rather than hardcoding local `/cabinet` paths.
- Preserve existing Den Cabinet links through deliberate compatibility handling: safe GET links can redirect, while legacy mutating endpoints must be removed or explicitly retired rather than blindly redirecting POST requests across hosts. Handle missing/unavailable Cabinet origins honestly; never fall back to a second inline editor.
- Keep browser sessions and credentials scoped to the correct origin; host-only secure cookies are the default. Establish explicit authentication/return-link handling for Den ↔ Cabinet, including unrelated domains, without requiring broad cookie sharing. Shared Postgres identity/session storage alone is not browser SSO. Validate return destinations, CSRF/origin boundaries and the independent authorization of same-origin attachment byte requests.

#### Bear administration remains Cabinet-aware

| Surface | Retained responsibility | Cabinet handoff |
|---|---|---|
| Bear administration | Configure Bear-level Cabinet capability/enablement; show authorized access/review/retention summaries and actionable status | Open Cabinet for page details, content, history, page policy/membership and content operations |
| Docket/Job UI | Own Job→Mission page annotations, Job evidence and existing evidence/capture authorization | Open the referenced live page on Cabinet; any selector is an access-filtered reference picker, not another page browser/editor |
| Bear deletion/lifecycle | Own deletion preview, confirmation and blockers without disclosing private evidence | Link readable Cabinet-owned upload/saved-copy remediation to Cabinet; no access elevation or borrowed private-copy ownership |
| Cabinet UI | Own knowledge browsing, detail and manipulation workflows | Link back to the relevant Bear/Job when authorized; do not absorb Bear capability, Job state or runtime administration |

Bear-specific Domains and private memory remain under their existing Bear-owned boundaries, not Cabinet Missions. Summaries and links are projections over canonical state, not new editable copies of page policy or membership. Surface handoffs preserve existing page and artifact authorization, including private-copy ownership; no Den/Bear admin shortcut may create borrowed private-copy access.

#### 5c — Authenticated MCP for external runtimes

- Expose a network-accessible MCP adapter from the Cabinet-only composition over the same typed operations used by the UI and native Den tools. MCP is an external protocol edge, not the internal transport the UI or Bear tools must use.
- Resolve authenticated principals and delegated authority at the boundary into typed actor scope. External runtimes may act on a person's delegated authority or a supported explicitly provisioned agent identity; they do not become Bears by supplying a `bear_id`, and tool arguments cannot select arbitrary actor identity. Select and document the supported credential/delegation flow before enabling external writes.
- Use descriptor-owned operation names, schemas, audiences and permission metadata rather than maintaining independent MCP aliases/allowlists. Preserve existing native tool naming and admission rules; an MCP connection is not an ACP armature or a Den run.
- Define the initial read/write/history/source operation set explicitly. Apply the same current membership, Bear enablement where applicable, review requirements, person-only deletion, optimistic version conflicts and attachment privacy as the facade. Do not advertise every human workflow as an agent tool merely because the UI supports it.
- Audit principal, delegation/client provenance and mutations without inventing a native Bear run or weakening review/retention checks. Document token audience, revocation, transport/session protections and error semantics; enforce fresh operation authorization rather than treating MCP connection establishment as a durable permission grant.

#### 5d — Next-release page discussion and history

- Provide Page / Discussion / History navigation on every readable Cabinet page. Discussion is current-page state shared across revisions; history remains an immutable page-content projection, not a conversation transcript.
- First release: human-only, append-only plain-text discussion under current inherited page readership/write authority, with active-page posting, archived read-only behavior, server-owned author provenance, bounded messages and preserved invalid/conflicting drafts while the user still has read access. Bear/MCP posting, edit/delete, threaded replies and older-message pagination are separate follow-ons.
- Retain discussion and provenance through page tombstoning/account lifecycle without making tombstones readable or changing page revisions/pending-review publication. Fence and freshly authorize appends; prohibit update/delete/truncate and unsafe rollback in the database.
- History shows visible revisions, authors/timestamps, publication/review state and current marker; exact-version views and escaped side-by-side source comparison cover published revisions. Pending/rejected versions must retain existing author/reviewer restrictions.

**Local evidence (2026-10-13):** Cabinet-only startup/config/surface tests prove no native memory capability or SQLite access, shared-session/schema read-only admission, legacy read redirects/retired writers, independent browser/MCP auth and exclusive recovery ownership by configuration. The full `den-web` library suite passes (395 tests, 7 opt-in ignored); six opt-in MCP database/security tests additionally pass. A real-router test covers Cabinet password login, page/discussion/history, shared-facade visibility, legacy Den handoff and browser/MCP credential separation. All 23 discussion facade/retention/rollback tests, four startup tests, three origin/destination/recovery tests and eleven Cabinet contract tests pass. SQLx metadata was regenerated additively through `scripts/sqlx.sh prepare-all`; full workspace all-target strict Clippy, formatting, whitespace and memory write-topology checks pass. The MCP agent also exercised the existing `rmcp` SDK loopback probe without adding a dependency. These are local code/test results, not production or native-browser acceptance.

**Artifact/deployment follow-on (2026-10-13):** approved root Compose `cabinet` profile and the source-build override are implemented. Six resolved-Compose tests verify shared image/database/identity/storage, origin/port configuration, migration→Cabinet-health→Den ordering, default fallback and native-volume isolation. The production Alpine/musl image `bears-den:cabinet-surface-validation` built successfully after extending the build limit; full workspace all-target strict Clippy now passes. Three opt-in production-image tests use the same image's migration job on a fresh isolated database, then exercise trusted local HTTPS Secure-cookie login, page create/edit, discussion/history/published comparison, embedded assets, UID 10001/read-only/no-native-memory startup and browser/MCP credential separation. The disposable container is removed; the running Den stack and deployment secrets were not changed. Repeatable checks and rollout configuration are in the [deployment notes](../guides/den-deploy.md#cabinet-only-composition-next-release).

**Remaining release gates:** choose/configure the actual public hostname and TLS ingress; activate the Cabinet profile/origin consistently where locally hosted; validate native-browser and Den/Bear/Docket handoffs, live storage/revocation, and OAuth consent/client setup. Discussion's original migration is already applied to bundled development Den and isolated validation; the retention correction is applied to the isolated database only. MCP is initially read-only with manually configured existing Den OAuth credentials; write scopes, Cabinet-specific audience and durable delegated-client provenance remain follow-ons. No recall/provider milestone is completed by this slice.

#### Delivery checks / exit

- [ ] A dedicated Cabinet container serves its own hostname and distinct navigation while Den remains the single domain/schema owner; an edit is visible to native Den Cabinet tools and the reverse without synchronization or duplicate state.
- [ ] Cabinet-only startup has no Bear runtime/memory initialization or SQLite mount, no unintended Den routes and no duplicate migration/general-worker ownership. A two-process startup test proves Cabinet upload recovery runs only in its designated owner and remains available after old routes are retired; web composition must not start it implicitly in both processes. Postgres readiness, storage-unavailable behavior and coordinated release/rollback are documented and tested.
- [ ] Bear administration still configures Cabinet capability and shows authorized status, but live Cabinet details/manipulation use the new surface. Docket references/evidence remain Docket-owned; cross-host page, review, attachment and lifecycle-remediation links work without widening access.
- [ ] Browser tests on distinct origins cover authentication/return paths, Cabinet navigation, stale edits, reviews, uploads/previews/downloads, unavailable service behavior and legacy-link handling. Revocation and private-content non-disclosure hold across both surfaces.
- [ ] An external MCP client can authenticate and perform the release's explicitly supported Cabinet operations without a Den-native run. Initial read-only adapter security/SDK tests are implemented; live OAuth/ingress acceptance remains. Before adding writes, separately validate delegated provenance, scopes/audience, concurrent-edit conflicts and mutation audit.
- [ ] Every readable page has working Discussion and History views: current inherited audience, active-page human append, archived read-only state, retained author provenance, no content-revision side effects, protected rollback, exact revision views and published-only source comparison. Code/route tests are implemented; native-browser release acceptance remains.
- [ ] Guides describe the actually shipped hostname, login, navigation, MCP setup and operations; current `/cabinet` documentation is replaced only after delivery is verified. No surface-only milestone is counted as completion of recall or provider work.

## Documentation obligations

Every phase that changes model-visible Cabinet behavior must review and update `MODEL_EXPERIENCE.md`. Every user- or operator-visible phase must create or update the applicable documentation in `docs/guides`, including permissions, hierarchy and review behavior, and any source or attachment limitations.
