# Artifacts, Garage (S3), and Cabinet separation — Architecture Decision Record

## Status: Proposed

## Date: 2026-04-19

---

## Context

**Artifacts** (files produced or consumed around agent work) **must not** be stored inside **Letta**. They belong in **object storage** with explicit lifecycle and provenance.

**Garage** is the BEARS **S3-compatible** object store ([Garage Coolify deploy](../../../services/garage/COOLIFY_DEPLOY.md)). Den already plans to use it for presigned upload/download.

**Cabinet** (Outline-backed, Phase 2+) uses object storage for **attachments** in a **different concern**: long-lived documents, human editing, deck policy—not ephemeral chat/run blobs.

### Harness vs control plane vs object storage

BEARS uses three product layers ([DEN_ARCHITECTURE.md](../DEN_ARCHITECTURE.md#three-layers-names)) plus **Garage** as infrastructure:

| Piece | Role for artifacts |
|-------|-------------------|
| **Harness (Letta Code)** | **Runtime I/O** during agent work: the **tool loop** runs here; uploads/downloads of artifact bytes are **initiated in the harness** (tools, skills, terminal helpers)—typically **via** Den-issued **presigned URLs** or thin **Den HTTP APIs** that enforce `bear_id` / `conversation_id` / membership before writing. |
| **Control plane (Den)** | **Policy and lifecycle**: bucket names, credential scope, **presigned URL issuance**, optional **artifact registry** rows in Postgres, **garbage collection** jobs, provenance **metadata** contract. Den does **not** run inside Letta’s tool sandbox; it **governs** how the harness and browser may use S3. |
| **Garage** | **Infrastructure** (like Postgres): S3-compatible storage **outside** Letta. Neither “Den” nor “Letta” as a layer—**both** the harness path and Den reach it with different roles. |
| **Letta (persistence)** | **No blob storage** for artifacts—only **references** in messages/blocks if needed. |

**Summary:** Your mental model—“file storage **during** agent use feels like **Harness**”—is right for **who performs the write in the turn**. **Den** still **owns** buckets, GC, and URL policy; the ADR emphasizes that side because it is GitOps- and security-relevant.

---

## Decisions

### 1. Artifacts live in Garage, not Letta

All **binary or large** outputs from agent use—including tool runs, **skills**-mediated steps, image/file generation, and **routine** runs—are stored as **objects** in a dedicated **artifacts bucket** (see § Bucket layout). **Letta** holds **references** (e.g. conversation id + object key or Den-issued id) as needed, not the bytes.

### 2. Single artifacts bucket, unified model for agent output and human upload

**User-uploaded files** use the **same bucket** and **same structural conventions** as agent-generated artifacts. **Provenance** is always recorded in **metadata** (see § Metadata): at minimum distinguish **`source: agent`** vs **`source: human_upload`** (and extend with tool name, bear id, user id, etc.).

### 3. Conversation association

Objects are **associated with a Letta `conversation_id`** (and Den’s `bear_id` / `user_id` in metadata) so the UI and GC can scope by thread. Exact **key prefix scheme** is implementation-defined but must be stable and documented in Den (e.g. `conversations/{conversation_id}/{artifact_id}` or equivalent).

### 4. Ephemeral by design + garbage collection

The artifacts bucket is for **temporary** working files unless promoted elsewhere. **Den** (or a scheduled worker) runs **garbage collection**: delete or lifecycle-expire objects older than policy (per-org TTL, quota, or both). **GC must not** apply to the **Cabinet** bucket.

### 5. Cabinet attachments: separate bucket, Outline-mediated

**Cabinet** document attachments (Outline storage, Phase 2+) use a **different S3 bucket** (and credentials/policy as needed). **Outline** (or Den’s Cabinet adapter) **owns** those objects; Den does not run artifact GC there.

**Do not conflate:** `bears-artifacts` (ephemeral, GC) vs **Cabinet bucket** (durable knowledge, ACLs, Outline).

### 6. Promotion path (future)

**Optional product direction:** UI to **move** or **copy** an artifact from the artifacts bucket **into Cabinet** (new Outline attachment / document), with user confirmation and policy checks. Not required for initial artifact storage.

---

## Metadata (required direction)

Every object should carry **robust metadata** (S3 user metadata and/or sidecar manifest in Den’s DB):

| Field (conceptual) | Example |
|--------------------|---------|
| `conversation_id` | Letta conversation |
| `bear_id` | Den bear |
| `user_id` | Acting user (or null for system/routine if applicable) |
| `source` | `agent` \| `human_upload` \| `routine` (if useful) |
| `provenance` | Tool name, skill name, parent message id, routine id, content type, sha256 |
| `created_at` | ISO timestamp |

Exact schema is a Den implementation detail; **provenance must be sufficient** for audit and for GC rules.

---

## Bucket layout (reference)

| Bucket | Purpose | GC |
|--------|---------|-----|
| **`bears-artifacts`** (name may vary per deploy) | Agent outputs, human uploads in chat, routine file outputs | **Yes** (Den policy) |
| **`bears-cabinet`** (or name aligned with Outline) | Cabinet / Outline attachments only | **No** (artifact GC rules); lifecycle per Outline/Cabinet policy |

Deploy: create **both** buckets in Garage; scope keys to least privilege (Den service key: artifacts read/write; Cabinet key: cabinet bucket only or via Outline).

---

## Consequences

- **Den:** S3 client, presigned URLs, artifact registry table (optional) for query/GC, **GC job** (cron or queue worker).
- **Letta Code / harness:** Tools that “save a file” **upload to artifacts bucket** via Den API or presigned URL; never persist large blobs in Letta DB.
- **Routines:** Routine outputs that are files **land in artifacts bucket** with `routine_id` (and bear) in metadata — see [routines-automation.md](routines-automation.md).
- **Phase 1:** Garage + artifacts bucket + metadata + GC may trail **first** chat path; document order in [PHASE1_BOOTSTRAP.md](../../planning/PHASE1_BOOTSTRAP.md).

---

## References

- [Garage Coolify deploy](../../../services/garage/COOLIFY_DEPLOY.md)
- [PLAN.md — Artifacts and object storage](../../planning/PLAN.md#artifacts-and-object-storage-garage)
- [routines-automation.md](routines-automation.md)
- [DEN_ARCHITECTURE.md](../DEN_ARCHITECTURE.md)
