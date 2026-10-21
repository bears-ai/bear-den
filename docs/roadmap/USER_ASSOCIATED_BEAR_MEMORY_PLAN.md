# User-Associated Bear Memory Plan

**Status:** Active — identity and read-gate foundation implemented; authorized portability reconnection is required before personal-memory enablement and remains unimplemented

**Topic:** [Bear memory and hats](../topics/bear-memory-hats.md)

**Parent:** [ADR-0042 — Memory–Entity Relationships and the Bear Entity Layer](../decisions/adr-0042-memory-entity-relationships-and-bear-entity-layer.md)  
**Goal:** Let a Bear remember durable, user-specific context across that person's conversations, hats, and authorized moves between Den hosts, without confusing distinct people or exposing their context through ordinary interactions with other users.

## Implementation baseline and delivery gates

**2026-10-21, code/test only; not deployed:** the generic entity/handle tables, descriptor registry, descriptive/access-bearing relation tables, and `AccessContext` already existed. This plan reuses that infrastructure rather than introducing a parallel entity model.

The first implementation slice adds an atomic authenticated-user resolver over the existing `person` type, a canonical `authenticated_user_entities` authority table, principal bindings protected from generic mutation, and trusted `session_info.entities.human` wiring. Names, historical handles, and imported references never establish authentication authority. The read gate requires an exact current verified binding for a user audience; unknown rules, invalid targets, withdrawn-last-rule histories, and quarantined identities fail closed. Bundle import rejects executable/noncanonical schema (including triggers and foreign-key cascades) before initialization and retains quarantined user-identity markers so old audiences cannot become generic grants. See the [ADR-0042 amendment](../decisions/adr-0042-memory-entity-relationships-and-bear-entity-layer.md#authenticated-principal-binding-amendment-2026-10-21).

**Portability gap:** current import stops at quarantine, and subsequent normal resolution creates a fresh person. There is no authorized reconnection operation yet. This is a safe default but does not satisfy memory continuity after a move. Phase 6A below is a required delivery milestone, not an indefinite exception; ordinary immutability guards must remain while a dedicated restore operation establishes a new host-local binding to the original portable person.

This is **not** an enabled personal-memory feature. Ordinary recall callers still use their existing empty user-audience contexts. No personal-fact model tool, promotion path, user-context projection, or management UI is enabled by this slice. Phase 0's remaining product policy and the following implementation gates must be satisfied before enabling personal-fact storage/promotion:

- **Atomic protected publication:** record, mandatory audience, provenance, curation decision, and correction/supersession commit together; withdrawal never turns retained content unrestricted.
- **Audience-preserving derivation:** personal context rewritten into ordinary notes or summaries must not reach unrestricted hat/core publication without separately authorized audience widening. Crossing hats itself is not leakage.
- **Protected secondary outputs and export authority:** ordinary inspection, curation briefings/transcripts, diagnostics, and worker artifacts must honor personal audiences. Separately authorized private backup/migration may carry the Bear cognition allowed by the export grant, including distinct personal audiences; do not categorically reject the full SQLite snapshot or mistake it for a selectively shareable export. Its disclosure, contents, and permission checks must be explicit.
- **Usable portability:** an authorized exporter/restorer must be able to reconnect imported Alice to destination Alice without making imported Bob into Alice. Preserve portable person IDs and memory relations; quarantine means no access until explicit authorized reconnection, not permanent loss of usable memory. This must work without the source Den remaining online.
- **Removal and replay semantics:** define historical evidence retention separately from active model reuse; cover persisted tool results, summaries, resumed sessions, harvest, delayed retries, and in-flight corrections/removal.
- **User control and rollout:** inspection/correction/removal/opt-out and scoped pending-candidate deduplication must be available and tested before ordinary users can create durable personal context.

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

Before schema or tool work, amend/accept ADR-0042 or add an implementation decision that fixes the following policy.

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
   The normal resolver uses the current active binding and creates a fresh confirmed/asserted person, derived handle, and binding only when no binding exists. The canonical binding table—not a portable reference—is local authentication authority. Generic entity/handle mutation, merge, and split remain prohibited for active or quarantined user identities. The dedicated Phase 6A reconnection operation must be able to establish a new destination binding to an existing imported person; normal resolution must then reuse it rather than allocate another person.
4. Ensure equivalent authenticated sessions for the same Bear and Den account resolve to the same person across supported clients. Different local principals remain distinct by default; authorized import/reconnection preserves the selected portable person across a change of Bear/host/account IDs.
5. Do not infer or backfill person identity from historical transcript text. Any migration of existing data must be explicitly selected and auditable.

**Exit criteria:** given a verified session principal, all supported entry points can produce the same typed current-user entity, without accepting model/client-supplied entity identity.

## Phase 2 — Access-bearing audience rules and fail-closed recall

**Read-gate hardening implemented; complete recall/repository integration remains pending.**

1. Reuse existing descriptor-routed descriptive relations (`memory_relations`), access-bearing rules (`memory_access_rules`), and the cross-cutting read view. Preserve append-only lifecycle/supersession.
2. Reuse the existing descriptive `subject` and access-bearing `audience` descriptors. Restrict access-rule writes to curation/authorized service paths. Verified-user contexts must match the current Bear/principal/entity binding; raw entity IDs cannot authorize user audiences. Missing/unresolved/quarantined targets, unknown rules, and a history with no remaining active rules deny access.
3. Require a typed `AccessContext` as input to every recall assembler and repository query that can return memory. It must be constructed from the verified principal and resolved Bear entity.
4. Query audience rules as an enforced gate, not a ranking hint. If access context or a valid current-user entity is absent, do not return audience-gated records.
5. Apply equivalent gating before ordinary snippets, embeddings, graph expansion, caches, derived indexes, error messages, scoped exports, and diagnostics are returned. Full private backup/migration uses its explicit export authorization boundary, not an accidental bypass or the current user's recall filter. Derived systems may index protected content only if they preserve record identity and are filtered before any text/snippet reaches a caller.
6. Keep existing source-local, selected-hat, and unrestricted core semantics unchanged; audience-gated Bear records are an additional eligible class only after the access gate succeeds. Hat selection organizes recall, not personal-memory authorization; do not add hat-specific privacy gates.

**Exit criteria:** omission of access context is impossible or rejected by API shape, and integration tests demonstrate no User B path can retrieve User A's audience-gated record.

## Phase 3 — Personal-fact candidate surface

Introduce a purpose-built model-facing operation instead of broadening ordinary `memory_write_entry` into a publication mechanism. Illustrative shape:

```text
memory_propose_personal_fact(
  fact_type: preferred_name | pronouns | preference | accessibility_preference,
  value: ...,
  evidence: explicit_user_request | explicit_user_statement,
  retention: durable | temporary,
  correction_of: optional personal-memory reference
)
```

Server behavior:

1. Resolve the authenticated current user entity server-side and bind the candidate to it.
2. Store source provenance and a minimized proposed statement; do not make the model supply target identities or audience rules.
3. Validate descriptor, category, retention, size, duplication, and sensitivity policy.
4. Create a source-local observation when appropriate plus a typed promotion/review candidate pointing to the current user entity.
5. Support correction/supersession references and explicit opt-out.
6. Keep general `memory_write_entry` source-local by default. It may record descriptive provenance internally but cannot independently create unrestricted Bear-wide personal context.

Extend proposal data so it can represent: source record, verified source owner, target user entity, proposed Bear-level destination, mandatory `audience` rule, category, sensitivity, intent, confidence, concise proposed content, and correction/supersession target.

**Exit criteria:** “please remember that I am Hans” can create a reviewable, current-user-bound candidate without any model-provided account or entity identifier.

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

**Required before personal-memory enablement; not implemented by the identity foundation.** Quarantine is the pre-reconnection safety state, not a terminal portability policy. Implement a narrow human-authorized restore operation without relaxing generic entity guards or allowing models to reassign identity.

1. **Export provenance and selection:** carry stable person IDs, distinct source identity labels, source-instance/account provenance, and the exporter's person association where known and authorized. Preserve records, relations, lifecycle, and provenance within the export grant. Labels and source account IDs help a human distinguish Alice from Bob; they are not destination grants. Do not add transcripts, credentials, or live source-host dependencies.
2. **Import safely:** preserve portable person IDs and memory relations while revoking all source authentication authority. Keep imported user identities classified as user identities, never generic raw-ID audiences. Unmapped people remain inaccessible to ordinary recall. Existing removal/supersession states must not be resurrected by reconnection.
3. **Review an explicit mapping:** show an authorized restore actor the selected imported person and verified destination account, with a separate confirmation and clear audience consequences. Check current restore/reconnection rights and destination-account eligibility server-side. A normal member, model, forged user ID, matching username, or possession of a bundle/UUID alone cannot invoke the operation. Full export authority does not implicitly map all people to the importer.
4. **Preserve identity, replace attachment:** establish new host-local authentication authority for the same portable `entity_id`, restoring only the necessary current resolution/binding metadata. Do not rewrite memory IDs, retarget personal audiences to the importer en masse, copy facts into core, or merge Alice with Bob. Retain source-binding provenance and the new mapping decision in an immutable audit receipt.
5. **Handle an existing destination identity:** the current one-active-entity-per-Bear/account constraint and a person created by an earlier `session_info` call must not make reconnection impossible. Before implementation, fix a conflict policy for replacing a provisional/unused local attachment and for reviewing same-person continuity when the destination identity already has memory, pending candidates, or provenance. Preserve all material and references; require explicit reviewed reconciliation, never silently merge distinct people or discard new context. A conflict remains safely pending with an actionable resolution path.
6. **Commit atomically and revalidate:** confirm the selected import/person/binding versions and current actor rights at commit. Commit destination binding, necessary entity/handle updates, previous-binding disposition, and audit receipt together. Prevent concurrent claims, duplicate confirmations, and partial activation. A second principal cannot silently steal an already connected identity.
7. **Rebuild current authority:** invalidate or regenerate affected cached contexts/projections and derived identity metadata. Subsequent ordinary resolution and recall must use the approved destination binding. Old source principals, unrelated destination users, and stale contexts must not acquire access. Support audited correction/revocation of an erroneous reconnection without turning the audience unrestricted.
8. **Preserve continued portability:** a second export/move must retain the same portable person and its history. Restore must work with the source Den unavailable; imported source metadata is not trusted as live authentication or a substitute for the destination's authorized mapping decision.

**Exit criteria:** Alice can export cognition she is authorized to transport, import it with the source offline, explicitly connect imported Alice to a differently numbered destination account, and recall the same eligible records across hats. Imported Bob remains a distinct, unmapped person until separately authorized. Both an unused auto-created destination identity and one with existing context have a tested, no-data-loss conflict-resolution path. Repeated export/import, failed or revoked reconnection, and concurrent/replayed confirmations preserve identity and audience integrity.

## Phase 7 — Migration, observability, and rollout

1. Ship schema/read gate and complete Phase 6A's authorized export/reconnection acceptance before enabling personal-fact storage, promotions, or prompt projection.
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
- Full private backup preserves all cognition within its export grant; scoped/shared export omits excluded material. Neither grants ordinary recall of every included person's context.
- Reconnection works with the source Den offline and without source-host credentials or transcript migration.
- An already-created destination identity, including one with personal facts/candidates/provenance, has an explicit no-data-loss resolution path; no silent identity merge or audience widening occurs.
- Concurrent claims, revoked permissions, stale versions, replayed confirmations, transaction failure, and revoked/corrected mappings fail safely without partial activation or stale-context grants.
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
5. Add personal-fact candidate and curation support, preserving the portable-person/host-binding distinction.
6. Add cross-hat projection and on-demand retrieval.
7. Add user controls, audits, metrics, and staged rollout; do not enable personal-memory storage until the required portability and other release gates pass.

No existing conversation-local note should be silently reclassified during this work. Existing data needs an explicit, auditable migration or a new user request before it becomes audience-gated Bear-wide personal context.
