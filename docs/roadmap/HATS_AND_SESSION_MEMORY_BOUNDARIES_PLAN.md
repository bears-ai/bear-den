# Hats and session memory boundaries

**Status:** Active
**Topic:** [Bear memory and hats](../topics/bear-memory-hats.md).

Implementation started: a Bear-owned hat schema, typed hat ID, internal create/list/allow-surface functions, and guarded canonical conversation/Job binding and current-eligibility lookup have database tests. Per-Bear SQLite can now store distinct source-local and hat-scoped records and migrate old profile-local rows without promoting them; storage tests cover new and upgraded databases. No user-facing route creates a hat binding or enables Work; bound turns already use internal hat bindings for memory. New opt-in scoped SQLite read/browse/keyword functions filter on canonical columns and apply the existing access-bearing entity rules; denial tests include a path collision with legacy records and a confined note. The legacy model-facing direct read now enforces its advertised profile/core wall on each record, fixing guessed cross-profile paths; the admin read remains separate. A Den-owned resolver now distinguishes legacy sessions from bound canonical conversations/Work runs and refuses to downgrade a Work run after hat eligibility is revoked. A separate vector filter expresses the same source/hat/core restriction, but new scope types are deliberately not indexed while member semantic search lacks canonical hit rechecking. Bound conversations and eligible Work runs now route model-facing memory writes/reads/browse/keyword search using the canonical grant, with a Postgres-backed two-conversation test of persistence and next-tool access. Bound turn-start projection replaces legacy profile highlights with source/hat highlights and scoped recall; derived hits are rechecked against SQLite before rendering, including stale archive and path-collision cases. Unbound conversations and Jobs still use profile-local memory, new scopes are deliberately not indexed while member semantic search lacks canonical hit rechecking, and hat promotion/Work-enablement review and user-facing binding are not shipped. The approved Den-tool continuation path preserves its Work-run ID. Bound prompt-memory selection and list/upsert/patch tools now admit Bear-wide reads and exact-client-session blocks only, with mutations limited to the current session. Postgres upserts cannot reassign a block ID to another Bear or scope, and patches are Bear/profile constrained. Scoped `memory_status` and `session_info` avoid exposing legacy profile counts or labels; bound turns ignore opaque supplied runtime context in favor of Den compilation. This is a *bound-path cutover*, not completion of Bear-wide session isolation: other human management/export surfaces, legacy unbound sessions, reviewed hat promotion, and retrospective memory import still need explicit audience/visibility review before enabling hat setup UI. The ordinary member memory library now admits only canonical shared and Bear-owned hat records, with direct-ID/history enforcement and access-bearing checks; raw entity and reflection/proposal inspection is admin-only. Member semantic search falls back to curated keyword until its derived hits have a canonical SQLite recheck. This plan tests a simpler Bear mental model rather than committing to a hat-first platform migration. The [topic](../topics/bear-memory-hats.md) distinguishes current behavior from the target. A hat is a configured responsibility and resource/capability *limit*; it is neither another Bear nor a replacement for a Job, Mission, work surface, Connection, or governance state. Canonical cognition remains in per-Bear SQLite. Transcripts, Docket state, prompt-memory blocks, and derived recall are distinct state owners. Do not add another writable copy of hat permissions or memory.

## Gate 0 — Verify the contract before widening runtime behavior

- Define typed authorization inputs without assuming a stance label: authenticated human/membership; immutable conversation/Job source and hat binding; actual trusted armature or channel origin; effective resource grants; approved Job/run context; Den supervision and approval state. Hat permissions can only narrow existing rights. Model text, prompts, tool results, and UI labels never grant authority. A client reconnection cannot change the durable conversation binding; a hat or work-surface change requires an explicit rebind with reevaluation. Work runs derive their hat from the assigned Job; a run may retain an immutable authorization snapshot for audit but may not keep a second writable grant.
- Define the ordinary memory read set as own source-unit uncurated records + authorized hat-curated records + Bear `core/`. No ordinary path to another session's raw memory. Only the privileged curation/review path may inspect source records across sessions, and it has no arbitrary outbound execution path. No cross-session raw access via semantic search, keyword search, direct memory read/browse, graph expansion, turn-start projection, prompt-memory blocks, exports exposed to the model, or legacy aliases.
- Do not assume `client_session_id` is a durable memory bucket: the state inventory identifies canonical conversation as the durable interactive container, distinct from client sessions. Confirm the stable Work-run and intake source IDs before writing migrations. Decide whether hat membership is Bear-wide or person-private before public rollout; the isolated prototype can use a clearly shared Bear-owned hat.
- Challenge whether a closed execution class is needed for curation, ingress, and outbound work. Do not remove `TrustProfile` first and later recreate the same boundary as ad hoc string flags. Update the state-machine inventory and the affected ADRs when a concrete authority design is accepted; this draft is not that decision.

**Proposed policy matrix to review** (describes the target, not current enforcement):

| Verified source | Ordinary memory available | Effects and hard denial |
| --- | --- | --- |
| Human conversation over a channel | Own conversation notes, hat-curated, Bear core | Den-hosted tools under channel policy; no armature-local tools without a verified armature. |
| Same conversation through a verified armature | Same notes as above | Local tools only through armature obligations/approvals; changing client connection does not change which conversation's notes are owned. |
| Interactive run after armature disconnect | Same memory owner, subject to current user/policy authorization | Governance may change; absent client cannot supply local tools or approvals. |
| Job-authorized background run | Own Work-run notes, approved hat-curated, Bear core | Only Job-scoped tools, resources, and egress; never raw notes from the originating conversation. |
| Authenticated inbound event intake | Only its own payload/source unit and authorized curated context | Can record an observation, never initiate outbound effects from the event itself. |
| Privileged curation process | Reviewable raw source notes and curated records needed for review | May promote with provenance; no arbitrary outbound execution. Not a general-purpose ordinary session. |

All ordinary rows deny other source units' uncurated memory regardless of common hat, historical profile, tool implementation, or knowledge of a record ID. This matrix deliberately does not grant permissions merely because an origin is named: Den must authenticate the source, check the hat, and enforce resource/action limits at execution. Review who may access raw notes through other human management endpoints and exports separately. The member-facing memory library now filters raw notes, but Bear admins deliberately retain an explicit broad inspection path.

**Exit:** one reviewed matrix of allowed/denied memory reads and tool/action classes for interactive channel, verified armature, disconnected continuation, approved Work run, inbound intake, and curator. A table-driven executable policy test belongs beside the authoritative typed resolver once it exists, not as a prompt-only assertion.

## Gate 1 — Set the product contract and build a prototype

Test a low-fidelity setup card for a named **Security review** hat:

> Purpose: Review security of Repository A. Resources: Repository A. While you're here: can inspect and propose edits with approval. Autonomous work: off. Memory: notes from this conversation stay here until reviewed; reviewed knowledge for this hat may be used in future conversations. Enabling autonomous work later requires reviewing the hat's existing memory first.

Use the card to settle the contract and build a narrow admin/session prototype on this pre-release branch. In automated scenarios, check that its claims match the runtime: a new conversation cannot see raw notes from another; reviewed knowledge for a shared hat can be reused; a Work run cannot act without a Job; and enabling Work requires reviewing already-curated hat memory. Keep stance names out of the ordinary UI, but expose derived denial reasons in advanced audit/diagnostics. Iterate the copy alongside the implementation rather than blocking code on external usability recruitment.

**Exit:** a working prototype and tests make the card's promises true. Later user research may improve the copy; it is not a pre-release implementation gate.

## Gate 2 — Prove one complete isolated workflow

Build the smallest real vertical slice in a dedicated test Bear: one Bear-owned hat, one bound resource, two separate conversations (one armature-backed), a source note and explicit promotion, plus a Job-bound Work run. Exercise credential/armature/Job permissions using the existing descriptor and obligation boundaries; do not turn the hat into a free-form allowlist of provider tool names. Preserve stable tool surfaces; a channel cannot manufacture armature tools by mentioning a hat.

Den Postgres owns hat configuration and conversation/Job binding; per-Bear SQLite owns canonical memory and promotion provenance. Changes to Postgres SQL use SQLx macros and timestamped `scripts/sqlx.sh migrate add` migrations; changes to per-Bear SQLite use its schema and upgrade paths. `MemoryStoreManager` is process-wide, not re-created by a new hat feature. Bind in both chat and BearWire entry points, Job creation/dispatch, `session_info`, and model context; require typed IDs rather than trusting model-supplied hat strings.

**Executable denial/allowance matrix** (test both keyword and Qdrant when configured, plus direct reads/browse and turn-start projection):

| Scenario | Expected |
| --- | --- |
| A writes private note with an unmistakable secret/injected instruction; B uses same hat and same legacy profile (including the case where B is a different Bear member) | B cannot see raw A note, even by ID/path/query/recall or the member-facing memory UI; A may read its own note after reconnect. |
| Curator promotes a safe derived fact from A to hat | B can see the curated fact with provenance, not A's raw source. |
| Different hat or Bear asks for the promoted fact | Denies hat memory (Bear `core/` remains its own separate scope). |
| Job-bound Work run uses same hat | Sees only reviewed hat/core knowledge and its own run notes; cannot fetch A or B raw memory; cannot execute unapproved actions. |
| Unverified chat claims an armature, or a disconnected armature attempts a client tool | No armature-owned execution. |
| Hat's Work eligibility or resource grants expand after curation or dispatch | Existing Work run gets no silent grant; Work-enablement requires review of existing hat memory. |

Memory passages, recall-index payloads, graph expansions, direct read paths, and human/admin endpoints must derive visibility from canonical scope; a stale derived index is never an authorization source. Test a malicious memory text that tries to impersonate policy. Curation may summarize/redact; it does not make content executable instruction or grant Job authority.

**Exit:** one end-to-end run demonstrates the above, including negative cases. Keep the slice isolated; do not roll it out to existing Bears merely because the happy path passes.

## Gate 3 — Inventory and migrate without widening access

- Inventory `profile_local` SQLite records, `memory_proposals`/promotions, imported logical paths, Den Postgres `prompt_memory_blocks`, reflection sources, and derived recall indexes. Use source provenance only when it is verifiably a canonical conversation or run ID; do not assume a client session ID survives reconnect. Preserve existing shared `core/` semantics while auditing what it contains.
- For profile-local records without verifiable source, retain a restricted legacy review/archive view. Never automatically declare them curated for a default hat or expose them to a new conversation. Plan retention/export/deletion and manual promotion; quantify how much prior continuity becomes unavailable.
- Update all SQL/SQLite/vector/keyword/graph read and write paths together. Rebuild/reconcile derived indexes and invalidate stale passages. A compatibility mode that continues to return all profile-local records to a new session is not a secure rollout.

**Exit:** restore/migration tests preserve provenance and curation outcomes without widening access; test old database files and imports, plus behavior with semantic recall disabled. Tool and UI access to archives follows explicit admin/review authorization rather than ordinary model search.

## Gate 4 — Product surface and architecture decision

- Make hats and effective resource/action limits visible when creating a conversation or Job; show active hat, own notes, pending review, hat-curated memory, and Bear-wide knowledge in Bear memory UI. Reuse existing Bear management pages and memory library, not a duplicate memory store. Keep work surfaces/Connections as separate resource cards; add the warning and re-review flow when enabling autonomous Work on a populated hat.
- Only after the workflow is enforced, compare the existing `chat`/`pair` entry paths and `TrustProfile`/`TurnAuthority` with typed session origin, armature, Job, hat, and governance inputs. Collapse `chat`/`pair` if verified armature plus policy fully preserves their distinctions. For `work`, `watch`, `curate`, remove an authoritative stance label only where the denial matrix still passes and no hidden authority has moved into prompts, UI flags, or tool-name checks. Keep diagnostic labels if useful.
- Update the [memory model](../architecture/memory-model.md), [state-machine inventory](../architecture/den-state-machine-inventory.md), [stance conceptual reference](../architecture/bear-stances.md), and affected ADRs with verified behavior; update public descriptions only when shipped. The plan is not evidence of delivery.

**Exit:** an admin can predict memory sharing and action authority without stance vocabulary; regressions cover persistence *and* next-turn model projection; the decision to retain/derive/remove any internal stance is documented with executable evidence.

## Implementation landmarks (not commitments that these files alone suffice)

- Canonical memory schema/upgrade: `services/den/crates/den-memory/src/schema.sql`, `migrate.rs`, `records.rs`; direct/keyword tools: `tools.rs` and Den-hosted memory tool wiring.
- Derived recall: `services/den/crates/den-service/src/recall/query.rs`, `reconcile.rs`, `policy.rs`; bound run filtering and canonical rechecks exist, but new source/hat writes are not indexed and member semantic search stays disabled until a canonical post-filter can be applied. Unbound legacy profile reads still need migration. The member library uses `services/den/crates/den-memory/src/library/mod.rs` rather than path-derived grants.
- Session and policy: `services/den/crates/den-core/src/profile.rs`, `effective_policy.rs`, `client_tools.rs`; `services/den/crates/den-bearwire/`, `services/den/crates/den-runtime/`, `services/den/crates/den-docket/`.
- Bear management UI and memory: `services/den/crates/den-web/src/bear/settings.rs`, `bear/memory.rs`, corresponding templates, and `services/den/crates/den-web/src/ROUTES.md`.

Validate each code slice with focused crate tests, SQLx offline checks/cache preparation where needed, docs link checks, and an end-to-end smoke test before changing the current-behavior claim on the topic page.
