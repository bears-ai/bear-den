# User-Associated Bear Memory Plan

**Status:** proposed implementation plan  
**Parent:** [ADR-0042 — Memory–Entity Relationships and the Bear Entity Layer](../decisions/adr-0042-memory-entity-relationships-and-bear-entity-layer.md)  
**Goal:** Let a Bear remember durable, user-specific context across that user's conversations and hats without exposing it to other authenticated users of the same Bear.

## Problem

Today, an ordinary `memory_write_entry` records a source-local observation: it is useful within the canonical conversation or work-run source, but does not provide the intended cross-conversation personal context. Conversely, an unrestricted Bear/core or hat record is shared knowledge and is not a safe destination for facts about an individual user.

The desired behavior for a statement such as “please remember that I am Hans” is:

1. the runtime binds the fact to the **authenticated Den user**, not to a name guessed from text;
2. it becomes a reviewable candidate associated with that user's resolved Bear entity;
3. curation may promote a minimal, durable fact to Bear-wide storage;
4. the promoted fact is retrievable across hats and conversations **only** when the current authenticated actor is that same user; and
5. the user can inspect, correct, or delete it.

This is the `audience` access-bearing relation from ADR-0042, not a new global “user memory” scope and not a promotion of private material into unrestricted `core/` memory.

## Scope and invariants

### In scope

- Bear-local resolved entities for authenticated Den users.
- Audience-gated personal-context records that survive conversation and hat changes.
- A narrowly scoped model-facing candidate/write operation for explicit personal-memory requests and durable preferences.
- Curation, recall, prompt projection, user controls, migration, and security testing.

### Out of scope

- Inferring that two different Den accounts belong to the same person.
- Trusting a model-provided user ID, entity ID, or textual name as identity authority.
- Automatically preserving arbitrary biographical or sensitive statements.
- Replacing source-local memory, ordinary hat memory, or unrestricted Bear/core memory.
- Introducing privacy isolation between hats.

### Hat organization versus Bear-level privacy

Hats separate working context and organize curation so hat-specific material does not accumulate indiscriminately in core memory. **Hat separation is a memory-maintenance boundary, not a privacy boundary.** Moving information between hats of the same Bear is not, by itself, leakage.

Privacy is enforced at the Bear level: records remain bound to their Bear, and personal context within that Bear is restricted to its authenticated-user audience. Changing hats must neither grant nor revoke access to an otherwise eligible personal fact. Publishing an audience-bound fact as unrestricted Bear/core knowledge is a privacy violation because it widens the user audience, not because it crosses a hat boundary.

Source provenance, selected-hat retrieval, and explicit core-promotion rules still serve context quality, traceability, and a clean core. They must not be described or tested as confidentiality guarantees between hats.

### Non-negotiable invariants

- The authenticated principal is the only authority for the current user identity. A token authenticating as User A is User A for this purpose; the memory layer cannot distinguish token holders beyond that principal.
- A record associated with User A must never be projected, searched, semantically recalled, or exposed in diagnostics to User B merely because both use the same Bear or hat.
- The relation that enforces privacy is structural and fail-closed. It must not depend on a recall caller remembering to add an optional filter.
- Promotion writes a concise, newly authored fact with provenance rather than copying raw conversation text into Bear-wide storage.
- The model may request a candidate classification, but server code resolves and attaches the authenticated user entity.
- Personal context remains correctable, retractable, deletable, and auditable.

## Target model

A preferred-name memory promoted for the active authenticated user should have the following shape:

```text
Bear-wide memory record
  content: "The authenticated user prefers to be called Hans."
  subject: Bear entity den:user:<authenticated-user-id>
  audience: Bear entity den:user:<authenticated-user-id>  [access-bearing]
  provenance: source-local observation + curation decision
```

The Bear-wide record permits cross-hat recall. The `audience` rule makes it visible only in an `AccessContext` containing that same resolved Bear entity. `subject` is descriptive; `audience` is the enforcement boundary.

## Phase 0 — Approve contract and threat model

Before schema or tool work, amend/accept ADR-0042 or add an implementation decision that fixes the following policy.

1. Define the supported personal-context categories for v1: preferred name, pronouns, explicit communication/accessibility preferences, and explicitly requested durable preferences.
2. Define sensitivity policy. Explicit requests may create a candidate; medical, financial, precise-location, credentials, third-party, relationship, or similarly sensitive facts require stronger policy and must not be automatically promoted.
3. Define retention, correction, deletion, export/inspection, opt-out, and review/audit requirements.
4. Specify that no cross-account entity merge is automatic, even if display names match.
5. Establish the minimal source-retention rule: curated output must contain only the fact needed for future assistance, while private source evidence follows existing lifecycle policy.
6. Identify every retrieval surface: initial turn projection, ordinary browse/search, semantic/index-assisted recall, curator views, diagnostics, exports, and API/MCP surfaces.

**Exit criteria:** reviewed threat model covers cross-Bear and cross-user leakage, stale identity mappings, index bypasses, curator authority, deletion propagation, and token/account semantics. Cross-hat movement within the same Bear is not a privacy threat in itself; core cleanliness is a separate curation concern.

## Phase 1 — Authenticated-user entity resolution

Implement the minimum Bear entity layer required by ADR-0042.

1. Add canonical `entities` and `entity_handles` storage, migrations, typed IDs, lifecycle state, trust/provenance, and canonical references.
2. Register the descriptor-owned `user` entity type and relevant handle types. A confirmed Den user should use a canonical reference such as `den:user:<uuid>`; display name and username are handles, not authority.
3. Implement an idempotent server-side resolver:
   ```text
   resolve_authenticated_user_entity(bear_id, authenticated_user_id)
   ```
   The resolver is the only normal write path for the current human entity and creates authoritative identities directly at the ADR-0042 confirmed/resolved lifecycle stage.
4. Ensure equivalent authenticated sessions for the same Bear and Den user resolve to the same Bear entity across supported clients; different Bear/user pairs resolve separately.
5. Do not infer or backfill person identity from historical transcript text. Any migration of existing data must be explicitly selected and auditable.

**Exit criteria:** given a verified session principal, all supported entry points can produce the same typed current-user entity, without accepting model/client-supplied entity identity.

## Phase 2 — Access-bearing audience rules and fail-closed recall

Implement ADR-0042's two-table relation model as the privacy foundation.

1. Add descriptor-routed descriptive relations (`memory_relations`) and access-bearing rules (`memory_access_rules`), with append-only lifecycle/supersession and a read view for cross-cutting inspection.
2. Register `subject` as descriptive and `audience` as access-bearing. Restrict access-rule writes to curation/authorized service paths.
3. Require a typed `AccessContext` as input to every recall assembler and repository query that can return memory. It must be constructed from the verified principal and resolved Bear entity.
4. Query audience rules as an enforced gate, not a ranking hint. If access context or a valid current-user entity is absent, do not return audience-gated records.
5. Apply equivalent gating before snippets, embeddings, graph expansion, caches, derived indexes, error messages, exports, and diagnostics are returned. Derived systems may index protected content only if they preserve record identity and are filtered before any text/snippet reaches a caller.
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
- visibility wording that accurately states the fact is available across that user's conversations with this Bear but not to other ordinary users; and
- export/inspection support under applicable product policy.

Provide tightly controlled curation/administrator workflows with decision audit records. Avoid granting ordinary Bear administrators a general read path to other users' audience-gated content. Any exceptional operator access must be explicit, policy-governed, and audited.

**Exit criteria:** a user can manage their own personal context without viewing another user's records, and curation decisions are reviewable.

## Phase 7 — Migration, observability, and rollout

1. Ship schema/read gate before enabling promotions or prompt projection.
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

### Regression and storage

- Existing source-local, hat-scoped, and unrestricted core records retain current behavior. Hat-specific material is not automatically promoted into core; this is a curation-quality regression check, not a cross-hat privacy test.
- All recall assemblers require `AccessContext`.
- Indexes and cached projections cannot bypass access rules.
- Imported/package data retains entity/relation lifecycle and does not become visible without a matched principal.

## Suggested delivery sequence

1. Approve the contract and threat model.
2. Land entity resolution and typed `AccessContext`.
3. Land audience-gated storage/read enforcement with exhaustive isolation tests.
4. Add personal-fact candidate and curation support.
5. Add cross-hat projection and on-demand retrieval.
6. Add user controls, audits, metrics, and staged rollout.

No existing conversation-local note should be silently reclassified during this work. Existing data needs an explicit, auditable migration or a new user request before it becomes audience-gated Bear-wide personal context.
