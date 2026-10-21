# User-Associated Bear Memory Plan

**Status:** Active — identity/read-gate and Phase 3 server-only candidate storage/provenance foundations delivered; Phase 6A reconnection code/storage/HTTP tests verified; end-user capture, publication/recall and broader acceptance gates remain open

**Topic:** [Bear memory and hats](../topics/bear-memory-hats.md)

**Parent:** [ADR-0042 — Memory–Entity Relationships and the Bear Entity Layer](../decisions/adr-0042-memory-entity-relationships-and-bear-entity-layer.md)  
**Goal:** Let a Bear remember durable, user-specific context across that person's conversations, hats, and authorized moves between Den hosts, without confusing distinct people or exposing their context through ordinary interactions with other users.

## Implementation baseline and delivery gates

**2026-10-21, code/test only; not deployed:** the generic entity/handle tables, descriptor registry, descriptive/access-bearing relation tables, and `AccessContext` already existed. This plan reuses that infrastructure rather than introducing a parallel entity model.

The first implementation slice adds an atomic authenticated-user resolver over the existing `person` type, a canonical `authenticated_user_entities` authority table, principal bindings protected from generic mutation, and trusted `session_info.entities.human` wiring. Names, historical handles, and imported references never establish authentication authority. The read gate requires an exact current verified binding for a user audience; unknown rules, invalid targets, withdrawn-last-rule histories, and quarantined identities fail closed. Bundle import rejects executable/noncanonical schema (including triggers and foreign-key cascades) before initialization and retains quarantined user-identity markers so old audiences cannot become generic grants. See the [ADR-0042 amendment](../decisions/adr-0042-memory-entity-relationships-and-bear-entity-layer.md#authenticated-principal-binding-amendment-2026-10-21).

**Phase 6A implementation:** import still quarantines all source authentication authority, but a dedicated human-only management workflow can now explicitly reconnect a selected portable person to a current verified destination Bear member. Export records source-qualified labels/account/host, source Bear, and exporter association in SQLite without allocating a person. The workflow preserves generic immutability guards, supports no-data-loss parking of fresh or populated destination identities, and records atomic mapping/revocation receipts. See [Phase 6A](#phase-6a--portable-identity-reconnection) for the verified code/storage/HTTP boundary and remaining acceptance gates. This does not yet deliver ordinary cross-conversation personal recall.

**Phase 3 foundation delivered:** `den-memory::personal_candidates` and `den-service::personal_memory` provide isolated, server-only staging of explicit durable preferred-name requests, with capture disabled by default. This is storage/provenance groundwork, not completion of Phase 3's end-user/model-request exit criterion. See [Phase 3](#phase-3--personal-fact-candidate-surface) for the authority, consent, source and validation boundaries.

This is **not** an enabled personal-memory feature or broad policy approval. No personal-memory model/tool, HTTP/UI or runtime route is registered; no personal publication, promotion, user-context projection or ordinary personal recall is enabled. Ordinary recall callers still use their existing empty user-audience contexts. The separate admin identity-mapping UI exposes no memory content. Phase 0's remaining product policy and the following implementation gates must pass before end-user capture or personal publication is enabled:

- **Atomic protected publication:** record, mandatory audience, provenance, curation decision, and correction/supersession commit together; withdrawal never turns retained content unrestricted.
- **Audience-preserving derivation:** personal context rewritten into ordinary notes or summaries must not reach unrestricted hat/core publication without separately authorized audience widening. Crossing hats itself is not leakage.
- **Protected secondary outputs and export authority:** ordinary inspection, curation briefings/transcripts, diagnostics, and worker artifacts must honor personal audiences. Separately authorized private backup/migration may carry the Bear cognition allowed by the export grant, including distinct personal audiences; do not categorically reject the full SQLite snapshot or mistake it for a selectively shareable export. Its disclosure, contents, and permission checks must be explicit.
- **Usable portability:** Phase 6A's reconnection code preserves portable person IDs and memory relations and keeps Bob distinct when Alice is mapped, without a live source Den. Reconnection code/storage/HTTP tests are verified; live acceptance remains pending, and the end-to-end ordinary personal-recall acceptance criterion still depends on the later candidate/curation/recall phases. Quarantine means no access until explicit authorized reconnection, not permanent loss of usable memory.
- **Removal and replay semantics:** define historical evidence retention separately from active model reuse; cover persisted tool results, summaries, resumed sessions, harvest, delayed retries, and in-flight corrections/removal.
- **User control and rollout:** owner-bound backend get/list/withdraw, epoch-bound capture settings and pending deduplication now exist, but human inspection/correction/removal/opt-out UX and rollout remain gated. Do not expose a model tool or personal-body UI ahead of protected publication and the generic inspection/curation audit.

These are release gates, not claims that the remaining privacy paths have been implemented or reviewed end to end.

## Problem

Today, an ordinary `memory_write_entry` records a source-local observation: it is useful within the canonical conversation or work-run source, but does not provide the intended cross-conversation personal context. Conversely, an unrestricted Bear/core or hat record is shared knowledge and is not a safe destination for facts about an individual user.

The desired behavior for a statement such as “please remember that I am Hans” is:

1. the runtime binds the fact to the **authenticated Den user**, not to a name guessed from text;
2. it becomes a reviewable candidate associated with that user's resolved Bear entity;
3. curation may promote a minimal, durable fact to Bear-wide storage;
4. the promoted fact is retrievable across hats and conversations **only** through an authorized authenticated binding to that person;
5. after an authorized move, the person can explicitly reconnect their imported identity to their destination Den account and regain the same eligible memories; and
6. the user can inspect, correct, or delete it.

This is the `audience` access-bearing relation from ADR-0042, not a new global “user memory” scope and not a promotion of private material into unrestricted `core/` memory.

## Scope and invariants

### In scope

- Bear-local portable person entities, with separate host-local authenticated-user bindings.
- Audience-gated personal-context records that survive conversation/hat changes and authorized export/import with explicit identity reconnection.
- A narrowly scoped model-facing candidate/write operation for explicit personal-memory requests and durable preferences.
- Curation, recall, prompt projection, user controls, migration, and security testing.

### Out of scope

- Automatically inferring that two different Den accounts belong to the same person. Explicit authorized portability reconnection is in scope.
- Trusting a model-provided user ID, entity ID, or textual name as identity authority.
- Automatically preserving arbitrary biographical or sensitive statements.
- Replacing source-local memory, ordinary hat memory, or unrestricted Bear/core memory.
- Introducing privacy isolation between hats.

### Hat organization versus Bear-level privacy

Hats separate working context and organize curation so hat-specific material does not accumulate indiscriminately in core memory. **Hat separation is a memory-maintenance boundary, not a privacy boundary.** Moving information between hats of the same Bear is not, by itself, leakage.

Privacy is enforced at the Bear level: records remain bound to their Bear, and personal context within that Bear is restricted to its authenticated-user audience. Changing hats must neither grant nor revoke access to an otherwise eligible personal fact. Publishing an audience-bound fact as unrestricted Bear/core knowledge is a privacy violation because it widens the user audience, not because it crosses a hat boundary.

Source provenance, selected-hat retrieval, and explicit core-promotion rules still serve context quality, traceability, and a clean core. They must not be described or tested as confidentiality guarantees between hats.

### Non-negotiable invariants

- The authenticated principal is the only authority for the current Den account. A token authenticating as User A is User A for this purpose; the memory layer cannot distinguish token holders beyond that principal. An authorized binding connects that host-local account to a portable person; a source-host account ID is not that person's portable identity.
- A record associated with Alice must never be projected, searched, semantically recalled, or exposed in ordinary diagnostics to Bob merely because both use the same Bear or hat. Explicit backup/restore authority is a separate permission, not an ordinary recall grant.
- An authorized move preserves person `entity_id`s and the records/relations referring to them. Source authentication grants are revoked on import; authorized reconnection may establish a new destination binding to the same person without merging distinct people or republishing their facts.
- No matching name, username, numeric ID, portable reference, model claim, or bare knowledge of an entity ID authorizes reconnection. The authenticated restore actor, selected mapping, destination account, and current import/binding state must be checked explicitly.
- The relation that enforces privacy is structural and fail-closed. It must not depend on a recall caller remembering to add an optional filter.
- Promotion writes a concise, newly authored fact with provenance rather than copying raw conversation text into Bear-wide storage.
- The model may request a candidate classification, but server code resolves and attaches the authenticated user entity.
- Personal context remains correctable, retractable, deletable, and auditable.

## Target model

A preferred-name memory promoted for the active authenticated user should have the following shape:

```text
Bear-wide memory record
  content: "The authenticated user prefers to be called Hans."
  subject: person entity <portable-entity-id>
  audience: person entity <portable-entity-id>  [access-bearing]
  provenance: source-local observation + curation decision
```

The Bear-wide record permits cross-hat recall. The `audience` rule makes it visible only through an `AccessContext` authorized for that same resolved person in the current Bear. `subject` is descriptive; `audience` is the enforcement boundary. A host-local `den:user:<id>` reference is derived binding metadata, not the portable entity ID.

### Portability and authority contract

**The Bear remembers a portable person; a Den account authenticates that person on a particular host.** For example:

```text
Source Den:      account 42 -> person Alice (entity A) <- Alice's memory audiences
Imported Bear:                person Alice (entity A), no active destination binding
Destination Den: account 7  -> person Alice (entity A) <- the same memory audiences

Imported Bob remains person B, not account 7, and is not mapped automatically.
```

The target Bear may receive a new `bear_id`; person IDs remain stable inside the authorized imported cognition. Existing cross-Bear authorization checks still apply. Three authorities must remain distinct:

- **Ordinary recall:** a current authenticated binding grants only the person's eligible context, regardless of hat.
- **Private backup/migration:** explicit export authority determines which cognition may travel. A full authorized backup may contain Alice and Bob as distinct people; a scoped/shared export must omit material outside its grant. Neither mode converts personal records into unrestricted core knowledge.
- **Identity reconnection:** explicit restore/reconnection authority permits a selected portable person to be attached to a verified destination account. Export/import entitlement does not identify every person as the importer or automatically grant an ordinary session access to every included audience.

Reconnection must support offline migration and disaster recovery: require destination authorization and explicit mapping, not a live source Den, copied credentials, or model inference. Preserve source-instance/account provenance and useful identity labels for review, but treat imported metadata as information to select and audit a mapping, not as destination authentication authority. The trusted destination service records the authorized decision.

## Phase 0 — Approve contract and threat model

Before user-facing capture or publication, amend/accept ADR-0042 or add an implementation decision that fixes the following policy. The server-only preferred-name foundation is not approval of broader categories, sensitivity rules, publication timing, deletion/replay policy, or manual-curation/owner-review details. `Durable` describes candidate intention, not a completed legal retention or deletion policy.

1. Define the supported personal-context categories for v1: preferred name, pronouns, explicit communication/accessibility preferences, and explicitly requested durable preferences.
2. Define sensitivity policy. Explicit requests may create a candidate; medical, financial, precise-location, credentials, third-party, relationship, or similarly sensitive facts require stronger policy and must not be automatically promoted.
3. Define retention, correction, deletion, export/inspection, opt-out, and review/audit requirements. Specify separate permissions and disclosure for ordinary recall/inspection, full private backup versus scoped export, and explicit restore/reconnection; ordinary Bear membership or an opaque entity reference is not sufficient.
4. Specify that no cross-account entity merge is automatic, even if display names match.
5. Establish the minimal source-retention rule: curated output must contain only the fact needed for future assistance, while private source evidence follows existing lifecycle policy.
6. Identify every retrieval surface: initial turn projection, ordinary browse/search, semantic/index-assisted recall, curator views, diagnostics, exports, and API/MCP surfaces.
7. Specify identity-reconnection authorization, source provenance, conflict/revocation policy, and audit requirements, including offline restores and an already-used destination account. Do not treat every change of host-local principal as a prohibited merge of people.

**Exit criteria:** reviewed threat model covers cross-Bear and cross-user leakage, stale identity mappings, authorized export/restore and mistaken identity reconnection, index bypasses, curator authority, deletion propagation, and token/account semantics. Cross-hat movement within the same Bear is not a privacy threat in itself; core cleanliness is a separate curation concern.

## Phase 1 — Authenticated-user entity resolution

**Identity foundation implemented; broader entry-point integration remains part of the recall rollout.**

1. Reuse existing canonical `entities`, `entity_handles`, descriptors, and lifecycle. Add only the Bear-local authenticated-principal binding with typed `BearId`/`UserId` and a privately constructed `AuthenticatedUserEntity`.
2. Keep the existing descriptor-owned `person` type, as specified by ADR-0042; do not add a parallel `user` type. The derived canonical reference is `den:user:<authenticated-user-id>` (Den currently uses numeric user IDs). Display name, username, canonical reference, and even a historical `den_user` handle are not authority.
3. Implement an idempotent server-side resolver:
   ```text
   resolve_authenticated_user_entity(bear_id, authenticated_user_id)
   ```
   The normal resolver uses the current active binding and creates a fresh confirmed/asserted person, derived handle, and binding only when no binding exists. The canonical binding table—not a portable reference—is local authentication authority. Generic entity/handle mutation, merge, and split remain prohibited for active or quarantined user identities. The dedicated Phase 6A reconnection operation establishes a new destination binding to an existing imported person; normal resolution then reuses it rather than allocating another person.
4. Ensure equivalent authenticated sessions for the same Bear and Den account resolve to the same person across supported clients. Different local principals remain distinct by default; authorized import/reconnection preserves the selected portable person across a change of Bear/host/account IDs.
5. Do not infer or backfill person identity from historical transcript text. Any migration of existing data must be explicitly selected and auditable.

**Exit criteria:** given a verified session principal, all supported entry points can produce the same typed current-user entity, without accepting model/client-supplied entity identity.

## Phase 2 — Access-bearing audience rules and fail-closed recall

**Read-gate hardening implemented; complete recall/repository integration remains pending.**

1. Reuse existing descriptor-routed descriptive relations (`memory_relations`), access-bearing rules (`memory_access_rules`), and the cross-cutting read view. Preserve append-only lifecycle/supersession.
2. Reuse the existing descriptive `subject` and access-bearing `audience` descriptors. Restrict access-rule writes to curation/authorized service paths. Verified-user contexts must match the current Bear/principal/entity binding and authority generation; raw entity IDs cannot authorize user audiences. Revocation invalidates old contexts even if the same Bear/principal/entity tuple is later restored. Missing/unresolved/quarantined targets, unknown rules, and a history with no remaining active rules deny access.
3. Require a typed `AccessContext` as input to every recall assembler and repository query that can return memory. It must be constructed from the verified principal and resolved Bear entity.
4. Query audience rules as an enforced gate, not a ranking hint. If access context or a valid current-user entity is absent, do not return audience-gated records.
5. Apply equivalent gating before ordinary snippets, embeddings, graph expansion, caches, derived indexes, error messages, scoped exports, and diagnostics are returned. Full private backup/migration uses its explicit export authorization boundary, not an accidental bypass or the current user's recall filter. Derived systems may index protected content only if they preserve record identity and are filtered before any text/snippet reaches a caller.
6. Keep existing source-local, selected-hat, and unrestricted core semantics unchanged; audience-gated Bear records are an additional eligible class only after the access gate succeeds. Hat selection organizes recall, not personal-memory authorization; do not add hat-specific privacy gates.

**Exit criteria:** omission of access context is impossible or rejected by API shape, and integration tests demonstrate no User B path can retrieve User A's audience-gated record.

## Phase 3 — Personal-fact candidate surface

**2026-10-21, storage/provenance foundation delivered; server-only, not deployed or end-user enabled.** The [memory boundary](../../services/den/crates/den-memory/src/personal_candidates.rs) and [service boundary](../../services/den/crates/den-service/src/personal_memory.rs) stage candidates, not published facts. No model tool, personal-body UI, HTTP or runtime route is registered. Phase 4 is untouched.

- **Narrow typed scope:** only `PreferredName` + `ExplicitUserRequest` + `Durable`. Names are trimmed, validated to **1–128 UTF-8 bytes**, and reject control characters even before trimming. `PreferredName` has redacted `Debug`; neither it nor `PersonalCandidate` implements serde serialization/deserialization. Broader categories, inferred statements and temporary retention are not supported.
- **Separate protected storage:** bodies live only in `personal_memory_candidates`, with consent in `personal_memory_settings`. They never enter generic proposals, observations or records. Generic inspection/review queues are not personal-audience filtered and are unsafe destinations for these bodies; table isolation is not a claim that those queues are now hardened.
- **Current authority and consent:** every get/list/withdraw and capture/staging operation checks the exact current Bear/person/user binding and authority generation. Capture defaults to false and requires an explicit trusted server-only settings operation; there is no human settings UI yet. Consent is generation/epoch-bound. Opt-out atomically withdraws both Pending and Quarantined candidates for that person; reenabling does not resurrect them.
- **Postgres source fence:** the service requires current membership and an owned active canonical conversation joined to a real hat of the same Bear. User/Bear/membership/conversation/hat locks stay held through SQLite writes in a cancellation-safe spawned task. These are authority locks, not a distributed write transaction; cancellation can lose the response without cancelling the write. Hat-sharing opt-in is not required: hats organize context, not privacy.
- **Exact SQLite evidence:** staging rechecks the exact current visible, active, non-invalidated, non-superseded note UUID and canonical conversation scope columns, not a path or metadata identity claim. **Any access-rule history** rejects the source, including detached, withdrawn or superseded rules. Staging does not read/copy the source body or reclassify the note.
- **Idempotency and provenance:** only Pending candidates deduplicate, by Bear/person/conversation + exact source-note UUID + exact trimmed value (preferred-name type). Different users, notes, conversations, Bears or values remain separate. Immutable source Bear/user/generation/conversation/note provenance is informational, never an authorization grant.
- **Moves and local identity changes:** import rebases only the owning `bear_id` in the two new tables, retaining portable person/candidate IDs, origin Bear and source/audit ID metadata. Pending becomes Quarantined and source capture settings reset disabled with authority quarantine. Local identity parking also quarantines Pending candidates. Revocation/reconnection invalidates old bindings and consent even when the same Bear/user/person tuple returns. Capture after reconnection requires fresh explicit consent; old candidates never reactivate automatically.

Evidence: [types/value checks](../../services/den/crates/den-memory/src/personal_candidates/types.rs), [capture settings](../../services/den/crates/den-memory/src/personal_candidates/settings.rs), [SQLite source admission](../../services/den/crates/den-memory/src/personal_candidates/stage.rs), [Postgres authority locks](../../services/den/crates/den-service/src/personal_memory/authority.rs), [memory regression modules](../../services/den/crates/den-memory/src/personal_candidates/tests.rs), and [service owner/source and lock/cancellation regressions](../../services/den/crates/den-service/src/personal_memory/tests.rs).

**Verified code/test checkpoint:** **40 new memory tests** pass, with **202 `den-memory` tests** passing in full; **11 service canonical-owner/source, lock and cancellation tests** and **22 identity-reconnection web tests** pass against isolated Postgres. The redundant SQLite import simulation has been consolidated into the real bundle test, and shared parameterized fixtures/snapshots limit test-only query shapes. The web tests include the [real candidate bundle regression](../../services/den/crates/den-web/src/bear/settings/identity_reconnection/tests/personal_candidates.rs), covering provenance preservation, consent reset, candidate quarantine, and owner-only inspection after reconnection. Strict workspace all-target Clippy/topology, formatting, SQLite preparation, and mixed SQLite/Postgres all-target `prepare --check` pass; SQLx retains the existing potentially-unused-query warning. Documentation and whitespace checks pass. This slice adds only two additive SQLite tables and indexes: no Cargo/dependency change, Postgres migration or browser-bundle version change (**v3**). No Docker image build, service restart/deployment, or live-model/browser acceptance is included.

**Remaining, gated work:** design the eventual purpose-built current-user candidate request without broadening ordinary `memory_write_entry` into publication. Next are atomic protected publication and an audit of generic inspection/curation and their secondary outputs **before enablement**. Publication/category/sensitivity/timing, deletion/replay and manual-curation/owner-review policy remain undecided; `Durable` is candidate intention, not legal retention/deletion-policy completion. Do not stage a model tool or personal-body UI early. Correction/supersession, audience-preserving derivation, protected curation and ordinary personal recall remain future work.

**Exit criteria (not met):** “please remember that I am Hans” can create a reviewable, current-user-bound candidate through an authorized end-user/model request without any model-provided account or entity identifier. Server-only staging does not meet this criterion.

## Phase 4 — Curation and lifecycle

Extend Curate's policy and implementation.

1. Verify candidate provenance belongs to the same authenticated user entity and validate category, intent, trust, and sensitivity.
2. Deduplicate active personal facts; prefer a stable fact key per category/entity where appropriate.
3. On approval, create a new minimal Bear-wide record with both `subject(current-user)` and `audience(current-user)` relations. Never promote it as unrestricted `core/` knowledge.
4. Record an auditable decision and immutable provenance chain. Mark replaced facts and relations superseded rather than silently overwriting them.
5. Support reject, defer, retract, correct, expire, delete, and user-requested removal flows. Deletion/retraction must remove future projection and recall and propagate to derived indexes/caches.
6. Decide whether low-risk explicit requests can be auto-approved by policy or still pass periodic/manual curation; preserve an audit record in either case.

**Exit criteria:** an approved preferred-name candidate produces a cross-hat record gated to that user; a correction replaces it; a retraction prevents future recall.

## Phase 5 — Recall and model context

Update both initial context construction and on-demand recall. For a verified authenticated conversation, retrieval should consider, in addition to existing scopes:

1. current source-local records;
2. selected-hat curated records;
3. unrestricted Bear/core records; and
4. active Bear-wide records audience-gated to the current user entity, independent of hat.

Projection must be compact and legible, for example:

```text
## User context
- The authenticated user prefers to be called Hans.
```

Do not include source transcript excerpts, account identifiers, raw provenance, or another user's personal facts. Provenance should be available only through authorized diagnostic/curation views.

Update managed model-facing instructions to say:

- session identity is authoritative and must not be inferred from text;
- durable, explicit user preferences and “remember this” requests use the personal-fact candidate surface;
- sensitive facts require conservative handling;
- ordinary source-local notes are not equivalent to cross-conversation personal context; and
- the model must not retrieve, write, or guess another user's personal context.

**Exit criteria:** a fresh conversation under a different hat for the same user receives the approved context; another user on the same Bear does not.

## Phase 6 — User and operator controls

Provide a user-facing “What this Bear remembers about you” surface with:

- active, pending, superseded, rejected, and retracted facts;
- source/status explanation at an appropriate privacy level;
- edit/correct/remove and opt-out controls;
- visibility wording that accurately states the fact is available across that user's conversations with this Bear but not to other ordinary users;
- scoped inspection/export and separately authorized private backup/migration with accurate contents/visibility disclosure; and
- an explicit imported-identity reconnection surface as specified in Phase 6A.

Provide tightly controlled curation/administrator workflows with decision audit records. Bear-admin status must not silently become an ordinary recall/inspection override. Explicit backup, restore, and reconnection grants may authorize their stated operations and contents; show that disclosure and audit those decisions. Do not conflate an authorized private cognition backup with a shared-library download. Any other exceptional operator access must also be explicit, policy-governed, and audited.

**Exit criteria:** a user can manage their own personal context without viewing another user's records, and curation decisions are reviewable.

## Phase 6A — Portable identity reconnection

**Reconnection code/storage/HTTP tests verified (2026-10-21); live acceptance and broader gates pending, not deployed.** Quarantine remains the pre-reconnection safety state, not a terminal portability policy. The narrow human-authorized restore boundary lives in `den-memory::identity_reconnection` and the web **Backup & move → Imported identities** flow at `/bear/{slug}/portability/identities`. It is not a model API and does not relax generic entity/handle mutation, merge, or split guards.

1. **Export provenance and selection — implemented:** record source-qualified username/display labels, account ID, host, source Bear, and exporter association for existing active identities in `user_identity_origins` before snapshotting. Export does not allocate a person for the exporter. Stable person IDs, records, relations, memory lifecycle, and provenance travel within the private export grant. Labels and source account IDs help distinguish Alice from Bob; they are never destination grants. No transcripts, source credentials, or live source-host dependencies are needed.
2. **Import safely — implemented:** preserve portable person IDs and memory relations while quarantining all source authentication authority. Keep imported user identities classified as user identities, never generic raw-ID audiences. Unmapped people remain inaccessible to ordinary recall. Existing removal/supersession states must not be resurrected by reconnection.
3. **Review an explicit mapping — implemented:** require a current verified Bear Admin and a current verified destination Bear member. Show the selected person, source provenance, destination account, and audience consequences before a separate confirmation. Actor/session/Bear-bound nonces expire after 15 minutes; the typed expected identity/binding snapshot stays server-side, never in browser-supplied versions or authority fields. Preparation/viewing makes no identity change. A normal member, model, forged user ID, matching username, or possession of a bundle/UUID alone cannot authorize the operation. Full export authority does not implicitly map all people to the importer.
4. **Preserve identity, replace attachment — implemented:** establish new host-local authentication authority for the same portable `entity_id`, updating only necessary current entity-resolution/binding/handle metadata. Memory records, their lifecycle, IDs, and audience targets are never rewritten; facts are not copied into core and Alice is not merged with Bob. Retain source provenance and an immutable receipt of the new mapping decision.
5. **Handle an existing destination identity — implemented:** retain exactly one active person per Bear/account. A separate replacement acknowledgement can park either a fresh auto-created identity or a populated destination identity, preserving **all** records, relations, candidate bodies, and provenance. Pending personal candidates become Quarantined; their bodies are not deleted. This is attachment replacement, not an entity merge or automatic joint recall: the parked person's audience closes, and that person remains reconnectable later through a separate review. No context is silently discarded or moved between people.
6. **Commit atomically and revalidate — implemented:** recheck actor/target eligibility at confirmation and compare the server-held source-metadata snapshot, including selected/displaced origins and `display_name`, inside the SQLite commit transaction. Postgres User/email/Bear locks remain held through the SQLite commit; the mapping, entity/handle metadata, displaced-binding disposition, authority generations, and receipt commit in one SQLite transaction. This is not a distributed two-database write transaction. Durable review IDs reject duplicate confirmations and serialize competing claims without partial activation. An already connected identity must first be revoked in a separately reviewed operation, not silently stolen.
7. **Rebuild current authority — implemented at the binding/read gate:** subsequent resolution uses the approved destination binding and current generation. Revocation parks the person and advances its authority generation; old contexts fail even if the same Bear/account/person tuple is later restored. Correction is revocation followed by a new explicit reconnection review, never unrestricted access. Ordinary personal recall/projection wiring remains a later-phase gate.
8. **Preserve continued portability and recover receipts — implemented:** later exports retain portable people, origins, and receipt history; import still quarantines every source binding. The management receipt-history view supports recovery after a lost confirmation response, prioritizing local receipts over future-dated source history. Malformed imported history is handled defensively, and optional history-load failures do not block the mapping UI. Historical imported labels, account IDs, and receipts never grant destination authority or substitute for a new authorized decision. Reconnection needs neither an online source Den nor its credentials or transcripts.

**Storage and validation checkpoint:** this slice adds only the SQLite `user_identity_generations`, `user_identity_origins`, and `user_identity_receipts` tables alongside the existing `authenticated_user_entities` classification/authority table. Browser bundle format remains **version 3**; there is no Postgres migration or dependency change. Evidence: [memory reconnection implementation](../../services/den/crates/den-memory/src/identity_reconnection.rs), [memory regression modules](../../services/den/crates/den-memory/src/identity_reconnection/tests.rs), [web workflow](../../services/den/crates/den-web/src/bear/settings/identity_reconnection.rs), and [web regression modules](../../services/den/crates/den-web/src/bear/settings/identity_reconnection/tests.rs).

**Prior Phase 6A checkpoint (before the candidate follow-on):** reported local validation passed: **162 `den-memory` tests** and **21 focused `identity_reconnection` `den-web` tests**, the latter against auto-created, migrated SQLx Postgres test databases. The exported-bundle → new Bear → HTTP explicit-replacement round trip preserves the same person, audience, and records, with Alice recall allowed and Bob denied at the storage read gate; this is not ordinary personal-recall enablement. Coverage includes generation exhaustion, malformed receipt history (including invalid UTF-8 in every imported receipt field), local recovery priority despite 100 future-dated source receipts, stale selected/displaced origins and `display_name`, cancellation, and concurrency. The whole-workspace strict all-target Clippy plus memory/DenState topology gate passed via `CARGO_BUILD_JOBS=1 ./scripts/lint.sh`; `cargo fmt --all --check`, documentation, and whitespace checks also passed.

At that prior checkpoint, SQLite `prepare-memory` and mixed-workspace all-target `prepare --check` passed; the latter retains the existing potentially-unused-query warning. Query metadata was generated against the matching SQLite/Postgres schemas and preserved additively after an initial full-preparation timeout. No Docker image build, application restart/deployment, live browser/provider/model test, actual two-host live migration acceptance, or serving-Postgres migration is included.

**Still blocked:** end-user/model candidate intake, personal publication/promotion, audience-preserving derivation, protected curation outputs, replay-aware removal, and ordinary personal recall/UI. The [Phase 3 server-only storage/provenance foundation](#phase-3--personal-fact-candidate-surface) does not enable these paths. The existing full private admin snapshot is not scoped sharing, and pre-export curation flushing is still unimplemented. Phase 6A code delivery does not complete this plan.

**Exit criteria (not yet fully met):** Alice can export cognition she is authorized to transport, import it with the source offline, explicitly connect imported Alice to a differently numbered destination account, and recall the same eligible records across hats. The ordinary personal-recall part remains blocked by later phases even though the reconnection code is implemented. Imported Bob remains a distinct, unmapped person until separately authorized. Both an unused auto-created destination identity and one with existing context have a tested, no-data-loss conflict-resolution path. Repeated export/import, failed or revoked reconnection, and concurrent/replayed confirmations preserve identity and audience integrity.

## Phase 7 — Migration, observability, and rollout

1. Complete Phase 6A's authorized export/reconnection acceptance and the remaining policy/protection gates before exposing end-user capture, promotions or prompt projection. Server-only candidate storage is not rollout.
2. Add metrics for candidate volume, approval/rejection, retrieval hits, denied access-gated retrieval, stale entity resolution, correction/deletion latency, and index cleanup.
3. Audit all existing memory APIs and compatibility adapters for missing `AccessContext` or unfiltered derived text.
4. Gate the feature behind a rollout flag; begin with explicit preferred-name requests only.
5. Run multi-user staging tests and privacy review before enabling automated approval or broader categories.
6. Establish incident response: disable projection, invalidate caches/index entries, audit affected access paths, and notify/escalate per policy if an audience-bound record leaks.

**Exit criteria:** production rollout can be reversed without destroying records, and monitoring demonstrates gated records are never returned without a matching principal.

## Test matrix

### Identity and isolation

- The same authenticated Den user receives their approved personal context in a fresh conversation and under a different hat; changing hats does not change the fact's audience authorization.
- A record from one Bear cannot be retrieved through another Bear, even for the same authenticated user, outside an explicitly authorized import flow.
- A different authenticated user sharing the Bear/hat receives none of it through prompt projection, browse, exact search, semantic search, graph expansion, cache, API/MCP, diagnostics, or error snippets.
- A session/token authenticating as the same Den user does receive that user's context; this is expected credential semantics.
- Same display name, different Den users remain distinct entities.
- Missing/revoked/invalid identity fails closed.

### Candidate and curation

- An explicit preferred-name request creates a current-user-bound candidate.
- The model cannot target a different user by supplying an ID, name, relation, or malformed provenance.
- Curate creates an audience-gated Bear record rather than unrestricted core memory.
- Correction supersedes prior active value; deletion/retraction removes projection and recall.
- Sensitive categories obey confirmation/review policy.

### Portability and authorized reconnection

- Alice and Bob export as distinct portable people; authorized import preserves their person IDs, records, relation targets, lifecycle, and provenance even when the Bear receives a new ID.
- Before reconnection, neither matching numeric IDs/usernames nor a fresh destination binding grants either imported audience.
- With explicit restore/reconnection authority, Alice selects imported Alice and binds her verified destination account; her eligible memories become retrievable without changing their IDs or audience targets. Bob's memories do not appear.
- Bob may later connect to a separately verified eligible account without merging into Alice, including when destination numeric IDs collide with different source accounts.
- An ordinary member, forged mapping, unauthorized exporter/restorer, or model-supplied identity cannot authorize reconnection. Source labels and package metadata are not authentication proof.
- Full private backup preserves all cognition within its export grant; the still-unimplemented scoped/shared export must omit excluded material. Neither grants ordinary recall of every included person's context.
- Reconnection works with the source Den offline and without source-host credentials or transcript migration.
- An already-created destination identity, fresh or populated with personal facts/candidates/provenance, can be explicitly parked with all material and references preserved; no silent identity merge, automatic joint recall, or audience widening occurs. Reconnecting the parked person later requires another review.
- Concurrent claims, revoked permissions, stale versions, replayed confirmations, transaction failure, and revoked/corrected mappings fail safely without partial activation or stale-context grants. Restoring the same tuple does not revive an old-generation context; receipt history supports recovery without trusting imported historical claims.
- Deleted/retracted/superseded facts do not become active merely because identity is reconnected; a second move retains identity continuity and binding audit history.

### Regression and storage

- Existing source-local, hat-scoped, and unrestricted core records retain current behavior. Hat-specific material is not automatically promoted into core; this is a curation-quality regression check, not a cross-hat privacy test.
- All recall assemblers require `AccessContext`.
- Indexes and cached projections cannot bypass access rules.
- Imported/package data retains entity/relation lifecycle and does not become visible without a matched principal.

## Suggested delivery sequence

1. Approve the contract and threat model.
2. Land entity resolution and typed `AccessContext`.
3. Land audience-gated storage/read enforcement with exhaustive isolation tests.
4. Complete authorized export/restore policy, portable identity selection/provenance, and Phase 6A reconnection with Alice/Bob round-trip and destination-conflict tests.
5. Build on the delivered Phase 3 server-only candidate storage/provenance foundation: next implement atomic protected publication and audit generic inspection/curation before any end-user/model enablement, preserving the portable-person/host-binding distinction.
6. Add cross-hat projection and on-demand retrieval.
7. Add user controls, audits, metrics, and staged rollout; do not enable end-user capture, publication or recall until the required portability and other release gates pass.

No existing conversation-local note should be silently reclassified during this work. Existing data needs an explicit, auditable migration or a new user request before it becomes audience-gated Bear-wide personal context.
