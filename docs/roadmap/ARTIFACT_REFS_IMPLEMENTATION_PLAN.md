# Artifact refs implementation plan

**Status:** In progress — registry/core service, current citation surfaces, Cabinet retention/linking/downloads and bounded human Cabinet uploads are implemented locally. Live Garage validation, interrupted-upload recovery/GC workers and future consumer/model surfaces remain open.
**Primary ADR:** [ADR-0004: Artifacts, Garage (S3), and Cabinet separation](../decisions/adr-0004-artifacts-garage.md)  
**Related ADRs:** [ADR-0006](../decisions/adr-0006-bear-work-surfaces.md), [ADR-0034](../decisions/adr-0034-jobs-and-tasks-work-management.md), [ADR-0041](../decisions/adr-0041-archival-recall-and-async-curation.md), [ADR-0053](../decisions/adr-0053-stance-scoped-delegated-runs.md)

## Goal

Make artifact refs the general Den handle for durable content objects: uploaded files, generated reports, patches, screenshots, logs, Cabinet attachments, Docket evidence, and future delegated-run outputs.

Artifact refs should be Den-minted, registry-backed, permission-checked, provenance-rich, and stable after finalization. Models and clients should never use object keys, filesystem paths, signed URLs, or model-invented IDs as protocol handles.

## Relationship to Jobs, runs, and other Den subjects

Artifacts are durable content/evidence/output handles. Jobs are durable work-management state. Runs are execution attempts. Keep those nouns separate:

- Jobs define the work objective, task tree, acceptance criteria, status, commit/work-surface policy, and completion decisions.
- Runs record what happened during an execution attempt.
- Artifacts hold or point at durable content such as logs, reports, diffs, screenshots, uploads, and receipts.
- Artifact existence is evidence, not truth: criterion/task/job status remains an explicit Docket decision that may cite artifact refs.

Docket and runtime records should link to artifact refs; they should not own blobs or use object keys/workspace paths as durable identity. The first implementation should prefer one small generic attachment/link model over per-surface special cases unless policy or query needs force a split:

```text
artifact_links
- artifact_ref
- subject_kind    -- conversation | job | task | run | criterion | delegated_run | cabinet_item | etc.
- subject_id
- role            -- input | source | output | evidence | test_report | diff | completion_receipt | etc.
- created_by_role
- created_at
```

Job-level artifact collections can be projections over direct job links plus task/run/criterion links. Do not add a separate "job artifact collection" object until users need manual curation, ordering, or policy that links cannot cover.

Lifecycle and authorization remain artifact-service responsibilities. A Job link may request or justify promotion, for example from ephemeral run output to job-audit evidence, but Job visibility must not bypass artifact read policy.

## Non-goals

- No full Cabinet product rebuild.
- No general document management UI.
- No delegated-run implementation dependency; artifacts should land first and be useful without subagents.
- No content-addressed public refs. Store hashes for integrity, but keep refs opaque.
- No cross-storage abstraction beyond what artifact refs need now.
- No requirement that artifact refs be attached to Jobs. Conversation-scoped and delegated-run-scoped artifacts are valid.

## ADR fulfillment

This plan fulfills ADR-0004 by implementing:

- [ ] Den-minted opaque `artifact_...` refs.
- [ ] Required artifact registry rows for every artifact ref.
- [ ] Storage backends hidden behind the registry (`garage_artifacts`, future `garage_cabinet`, `db_text`, `external_ref`, etc.).
- [ ] Two-phase creation: reserve -> write/upload -> finalize.
- [ ] Pending artifacts unreadable as finalized content.
- [ ] Finalized artifacts treated as stable snapshots.
- [ ] Permission checks on read, write, mount/copy, attach, promote, and delete.
- [ ] Provenance links to conversation, run, task, job, work surface, creating stance, source, and user/bear where applicable.
- [ ] Generic artifact links from refs to Den subjects such as conversations, jobs, tasks, runs, criteria, delegated runs, and Cabinet items.
- [ ] Lifecycle states for pending, ephemeral, promoted, Cabinet-durable, archived, and deleted.
- [ ] GC that respects lifecycle and Cabinet attachment state.

## Current execution status

**Local execution started.** Phase 1's registry/core-service slice is already present in the workspace through migration `20260716120000_artifact_refs` and `den-service::artifacts`. During verification, `den-service` was missing SQLx's existing `migrate` feature, so the artifact tests could not compile; enabling that feature is the only Phase-1 code change made in this execution so far.

Verified locally:

```text
SQLX_OFFLINE=true cargo test -p den-service artifacts::tests --lib
SQLX_OFFLINE=true cargo check -p den-service
```

Both pass after that manifest correction. Phase 1 is functionally implemented; Phase 3 now records the required structured output/validation handoff, so it is no longer blocked on a platform claim to correctness. That earlier checkpoint prioritized Phase 3 integration; the newer Cabinet upload slice below now also implements a bounded Phase 2 web consumer.

**Phase 3 progress:** Docket now rejects a `done` task update unless `result_refs` contains structured `primary_output` (`git_commit` or `den_artifact`) evidence and a recorded validation attempt that names the same ref and, when supplied, immutable identity, command, and execution provenance. This is a provenance/handoff integrity check, not a judgment that the output is correct. Completion writes a Docket-owned **evidence receipt** containing the recorded output, identity, and validation attempt. Artifact finalization/link checks are available for workflows that explicitly require them, but are not settlement gates; Git commit reachability/OID resolution is likewise not a universal gate. BearWire's `docket.jobs.diagnostics` now composes Docket job/task state with access-filtered, non-clickable task artifact citations via `den-service`; the citation projection is covered by a serialization check that excludes storage keys, digests, provenance, and metadata. The same diagnostics response now includes a minimal safe run summary and access-filtered run artifact citations. Its existing criterion diagnostics projection also returns access-filtered criterion citations. Conversation history renders both task-linked and conversation-linked citations as non-clickable structured resources containing only opaque `artifact_...` identifiers, with focused checks confirming that no backing fields are rendered. The remaining unchecked Phase 3 work is provenance when future work/runtime flows create artifacts; it has no producer yet, so no integration should be invented ahead of that flow.

**Cabinet/Mission slice (local branch, 2026-10-05):** existing finalized artifacts can be linked/detached on Cabinet pages and downloaded after independent page/artifact authorization. Registry links canonically retain attachments and private published-document snapshot citations; a database trigger prevents deletion/expiration while retained, and GC excludes them. Docket owns optional Job Mission page annotations; Job detail lists readable JSON evidence and serves scoped downloads of captured copies. Snapshot capture preserves exact version/title/content/hash, stays private, and does not settle a Job or promote Work access. Database-backed `den-web/src/work/tests/knowledge.rs` covers independent ACLs, hidden metadata, cross-Job downloads, stale revisions and retention after source-page tombstone. Garage downloads use configured storage with a 16 MiB size/hash-verified proxy; at that checkpoint new uploads and byte-flow checks remained open. The newer human upload implementation and its substitute-based validation are recorded below; live Garage validation is still open. Snapshot retention release remains operator-only and retained artifacts may block Bear deletion via cascades; see the [Cabinet guide](../guides/cabinet.md#attachments-and-private-copies).

**Human Cabinet uploads (local branch, 2026-10-06):** `den-web/src/cabinet/uploads.rs` accepts a bounded multipart form without JavaScript, with typed Bear/role/audience parsing. `den-service::cabinet::uploads` reserves a pending artifact and mints a non-deserializable admission receipt; private access is default, Bear-visible access requires explicit user acknowledgement. `den-web/src/core/s3/artifacts.rs` performs internal signed PUT/read-back verification (16 MiB, exact size/SHA-256, redirects disabled). Publication rechecks the active page and locks current Bear membership before finalizing and linking atomically. Failures remain unreadable and attempt registry/blob cleanup, but ambiguous successful commits never delete retained bytes. `den-web/src/work/tests/uploads.rs` covers reserve → byte-store substitute → verify → finalize/link → download, private/shared audiences, malformed/oversized requests, failed/corrupt storage, mid-transfer archive/page restriction/membership removal, pending readability and cleanup/replay. All 124 web tests pass. This is not a live Garage validation or a general model upload API; automatic crash/orphan recovery and a GC worker remain pending. No new dependencies, feature flags, storage configuration or schema migration were needed.

## Implementation phases

### Phase 0 — Inventory and initial decisions

**Goal:** Identify current artifact-like storage and lock the smallest initial contract before implementation.

Current ad hoc artifact-like stores:

- `tool_output_artifacts` (`services/den/migrations/20260625120000_tool_output_artifacts.up.sql`, `services/den/crates/den-runtime/src/tool_output_artifacts.rs`): stores compacted/full tool outputs as Postgres text or JSON and exposes `tool-output://<uuid>` refs. Access is currently scoped by `bear_id` and `session_id`, so these refs are session-local tool-output handles, not durable Den artifact refs.
- `conversation_compaction_artifacts` (`services/den/migrations/20260530213000_conversation_persistence.up.sql`, `services/den/crates/den-runtime/src/runtime/compaction/artifact_store.rs`): stores iterative summaries and other compaction payloads as conversation-scoped JSON with source-span provenance. These are durable conversation artifacts in shape, but not registry-backed `artifact_...` refs.
- Docket task/run evidence payloads (`services/den/migrations/20260622120000_docket_jobs_tasks.up.sql`, `services/den/migrations/20260710120000_docket_work_runs.up.sql`): `bear_task_run_state.result_refs`, `bear_job_criteria_state.evidence`, and `bear_work_runs.result_refs` are untyped JSONB bags. They can cite evidence today, but they do not enforce artifact identity, lifecycle, storage hiding, or authorization through a registry.
- Runtime checkpoint/audit outputs: loop-control checkpoints are currently runtime/audit events rather than a permanent artifact store. When retained beyond transient logs, they should become `runtime_checkpoint` artifacts linked to runs instead of creating another checkpoint-specific durable store.
- Garage/S3 config already exists in Den config (`S3_ENDPOINT`, `S3_BUCKET`, `S3_REGION`, credentials, public URL, path-style flag). Phase 1 does not need new storage config or a new dependency.

Initial implementation decisions:

- **Ref format:** Den-minted `artifact_` + UUID v4, using the already-installed `uuid` crate. Refs remain opaque and stable; do not add ULID/UUIDv7 unless ordered refs become a real requirement.
- **First storage kinds:** `db_text` for metadata-only/small text JSON/diff artifacts in the registry service, and `garage_artifacts` for byte-backed artifacts in the next phase. Avoid a storage-backend trait until a second implemented backend makes it pay for itself.
- **Lifecycle values:** start with `pending`, `finalized`, `ephemeral`, `promoted`, `cabinet_durable`, `archived`, and `deleted`. Reads of complete content require a non-pending, non-deleted finalized/promoted/durable state; finalize is one-way.
- **Visibility/auth model:** begin with bear-scoped ownership plus visibility values aligned with Docket (`private_to_profile`, `same_user`, `bear_visible`) and optional subject links. Artifact links may justify access, but every read/write/attach/delete operation must still pass through the artifact service authorization check.
- **Link model:** use the generic `artifact_links` table from this plan for conversation, job, task, run, criterion, delegated-run, and Cabinet-item subjects. Do not add per-surface attachment tables in the first slice.
- **Migration strategy:** new code should emit canonical `artifact_...` refs. Existing `tool-output://...`, compaction artifact IDs, and Docket JSON evidence remain readable only until their callers are migrated, then the pre-release parallel paths should be removed or replaced cleanly rather than kept as deprecated models.
- **Dependency strategy:** no new dependency for Phase 1. Use `uuid`, `serde`, `serde_json`, `sqlx`, and existing config/service patterns.

**Exit gate:** The inventory above is reflected in the Phase 1 schema/service shape, and no new dependency or broad abstraction is introduced before code proves it is needed.

### Phase 1 — Registry and core service

**Goal:** Create the canonical artifact identity and lifecycle API.

- [ ] Add artifact registry schema and migration.
- [ ] Add artifact model types with `artifact_ref`, `kind`, `storage_kind`, metadata, provenance, visibility, lifecycle, and timestamps.
- [ ] Add service methods:
  - [ ] `reserve_artifact(...) -> artifact_ref`
  - [ ] `finalize_artifact(...)`
  - [ ] `get_artifact_metadata(...)`
  - [ ] `authorize_artifact_access(...)`
  - [ ] `mark_artifact_deleted(...)`
- [ ] Generate opaque, stable refs in Den only as `artifact_` + UUID v4 using the existing `uuid` crate; models and clients cannot mint them.
- [ ] Add small service-level checks for reserve/finalize/read authorization and invalid lifecycle transitions.

**Exit gate:** Den can reserve, finalize, read metadata for, and lifecycle-check an artifact without exposing storage keys.

### Phase 2 — Garage-backed upload/download

**Goal:** Make artifact refs usable for real bytes.

- [x] Add internal presigned upload issuance for pending Garage artifacts (Cabinet proxy; not browser-visible write URLs).
- [x] Add finalize path that records object key, size, content type, and SHA-256; Cabinet publishes only after read-back verification.
- [x] Add permission-gated internal download issuance (proxy); inline previews/direct browser URL APIs remain deferred.
- [x] Enforce pending-vs-finalized readability.
- [x] Add GC candidate query for expired ephemeral artifacts, excluding retained links; automatic GC/pending-upload recovery remains deferred.
- [x] Add integration check for reserve -> upload/write substitute -> finalize -> metadata/read flow (`den-web/src/work/tests/uploads.rs`; not live Garage).

**Exit gate:** Human uploads and agent-produced files can become finalized artifact refs backed by Garage.

### Phase 3 — Conversation and Docket evidence integration

**Goal:** Replace ad hoc content handles in common Den surfaces.

- [x] Allow conversation events/messages to cite artifact refs for attachments and generated outputs.
- [x] Add generic artifact link/attachment records for Den subjects, including at least conversation, job, task, run, criterion, and delegated-run anchors.
- [x] Allow Docket jobs/tasks/runs/criteria evidence to attach artifact refs with roles such as `primary_output`, `input`, `source`, `evidence`, `test_report`, `diff`, `runtime_checkpoint`, or `completion_receipt`. A primary output may be a Git-commit `external_ref` or Den-owned content artifact, but a finalized/link-verified artifact is only required when the task or work-surface policy says so.
- [x] Record task-required validation attempts against the task's primary-output ref and, when available, stable identity (Git OID or finalized content digest), including the executed check, observed result, execution provenance, and durable diagnostics where relevant. Identity agreement preserves evidence integrity; it does not prove correctness.
- [x] Settle task completion only after structured primary-output evidence and the task's required validation evidence are recorded. Preserve candidate evidence and a blocked recovery path when an explicitly required observation (such as publication or finalization) fails.
- [ ] Add run/task provenance when artifacts are created by work/runtime activity.
- [x] Render artifact refs in task/run completion receipts.
- [x] Keep criterion/task/job state separate from artifact presence; completion decisions cite evidence refs but are not implied by them.
- [x] Stop leaking object keys or workspace paths as durable evidence handles.

**Exit gate:** A task/run can produce or cite artifact-backed evidence. A Docket task that requires output records structured primary-output evidence and its required validation attempts, with consistent identities where supplied; UI/model layers can display artifact metadata by ref. Finalization, link verification, publication observations, and successful checks remain explicit stronger policies, not universal correctness certificates.

### Phase 4 — Cabinet attachment integration

**Goal:** Let Cabinet use artifact refs without conflating Cabinet records with blob storage. The Cabinet item, ACL, and review contract is owned by the [Cabinet implementation plan](CABINET_IMPLEMENTATION_PLAN.md).

- [x] Add Cabinet attachment link model: Cabinet item -> artifact ref + attachment role (existing `artifact_links`, deterministic typed attachment ref).
- [x] Support `cabinet_durable` lifecycle or equivalent retention promotion for attached artifacts (retention derived from Cabinet link existence, no duplicate writable status).
- [x] Ensure artifact GC never deletes Cabinet-durable attachments (candidate exclusion plus deletion/lifecycle trigger).
- [x] Preserve Cabinet ACL/retention policy separately from ephemeral run artifact policy (independent artifact reader authorization, no audience promotion).
- [x] Add attach/detach operations with authorization checks.

**Exit gate:** Cabinet items can attach artifact refs as source documents, generated reports, or evidence, with durable retention semantics.

### Phase 5 — Work surface produce/consume flows

**Goal:** Make the artifact/work-surface boundary explicit.

- [ ] Record `work_surface_ref`, git branch/commit, source paths, and command/run provenance on artifacts produced from work surfaces.
- [ ] Add explicit copy/mount affordance for artifacts consumed by work surfaces.
- [ ] Keep artifact refs separate from filesystem paths and work surface IDs.
- [ ] Add a small check that a work-surface-produced artifact records provenance and can be resolved after the workspace path changes.

**Exit gate:** Work surfaces can produce artifacts and consume uploaded artifacts without treating paths as artifact identity.

### Runtime checkpoint artifacts

Agent loop-control checkpoints should revisit this artifact-ref plan when checkpoint retention is implemented. Runtime checkpoint reports are artifacts rather than status reports: durable audit/debug payloads attached primarily to runs, optionally linked to focused Jobs/tasks as audit evidence, and never task/job status transitions by themselves.

Expected shape:

- artifact kind: `runtime_checkpoint`;
- primary link: `subject_kind = run`, `role = checkpoint`;
- optional links: focused Job/task with an audit/evidence role;
- checkpoint presence does not imply task completion, blockage, waiver, cancellation, or Docket history-visible progress.

If loop-control lands before artifact refs, any temporary checkpoint table should include a migration path to `runtime_checkpoint` artifact refs instead of becoming a permanent competing store.

## Human UI affordances

**Cabinet file inspection (local branch, 2026-10-06):** attachment names open a server-rendered content-first page with Details for selected audience/owner/creator/type/size/timestamp/ref metadata. Source text (including Markdown, diffs, HTML, scripts and SVG) is escaped; JSON is pretty-printed. A 256 KiB UTF-8-safe preview cap does not truncate downloads. Raster images/PDFs require both recognized MIME and matching signature, then use separately authorized full-byte responses with size/hash checks, no-store/nosniff/no-referrer and sandbox CSP; PDF frames are sandboxed too. Unsupported/invalid types have downloads, while missing storage/content or files over the 16 MiB transfer ceiling are explicitly unavailable. Raw provenance/metadata and storage keys/URLs remain hidden. `den-web/src/work/tests/previews.rs` covers private metadata, cross-page/detached refs, MIME confusion, revocation before/during byte requests and text bounds; all 130 web tests pass. No dependencies, SQL/migrations or cache entries change. Live Garage and native browser rendering are not verified; broader artifact consumers remain open.

Build the small surfaces first:

- [ ] Artifact chip/card rendering anywhere an artifact ref appears.
- [ ] Metadata view: title, kind, content type, size, source, creator, lifecycle, visibility (Cabinet attachment details delivered; general artifact views remain open).
- [ ] Preview for text, markdown, diff, JSON, images, and browser-supported PDFs; download fallback for everything else (Cabinet bounded source/raster/PDF slice delivered; native-rendering validation and other consumers remain open).
- [ ] Provenance panel showing creating run/task/conversation/work surface when present.
- [ ] Lifecycle actions where authorized: keep, archive, delete, attach to Cabinet, detach from Cabinet, promote to durable, copy/mount into work surface.
- [ ] Permission-aware unavailable state that avoids metadata leakage when policy requires it.
- [ ] Artifact collections on runs, Docket tasks, conversations, and Cabinet items.

## Model experience affordances

Models should work with typed refs and summaries, not storage details.

- [ ] Expose artifact metadata as a compact model-facing shape:
  - `artifact_ref`
  - `kind`
  - `title`
  - `summary`
  - `content_type`
  - `readable`
  - `lifecycle`
- [ ] Add/read model-facing operations only as needed:
  - [ ] `get_artifact_metadata`
  - [ ] `read_artifact_text` or equivalent ranged text read
  - [ ] `preview_artifact` for non-text/large content
  - [ ] `create_artifact` / reserve+finalize wrapper where appropriate
  - [ ] `attach_artifact`
  - [ ] `copy_or_mount_artifact_to_work_surface`
- [ ] Ensure models may suggest titles/summaries/filenames but cannot mint refs.
- [ ] Ensure model-visible errors explain authorization/lifecycle failures without exposing backing storage.

## Acceptance criteria

Artifact refs are ready when:

- [ ] No model-facing or client-facing protocol needs Garage object keys or filesystem paths as durable content handles.
- [ ] Every artifact ref resolves through Den registry and permission checks.
- [ ] Pending artifacts cannot be read as complete artifacts.
- [ ] Finalized artifacts are stable snapshots.
- [ ] Conversation, Docket, Cabinet, and work-surface flows can cite artifact refs.
- [ ] A Docket task that requires an output records structured primary-output evidence and any task-required validation attempt before completion. Where an output identity is recorded, associated validation names the same identity; terminal run telemetry and worker narratives alone do not settle task/job state. This is a provenance and reviewability contract, not a correctness certificate. Artifact finalization, publication observation, and successful checks are stronger gates only when the applicable task or work-surface policy requires them.
- [ ] Users can preview/download/attach artifacts from obvious UI affordances.
- [ ] Models can cite, inspect, create, and attach artifacts through typed operations without storage trivia.
- [ ] GC respects lifecycle and never deletes Cabinet-durable attachments.
