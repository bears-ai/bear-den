//! Indexing policy + Qdrant payload shaping for Bear memory recall (ADR-0038 §2, §4).
//!
//! Policy decides *which* canonical records get embedded; payload shaping builds the
//! filterable Qdrant point payload and a deterministic point id so re-indexing the same
//! (bear, memory, chunk) tuple overwrites rather than duplicates.

use den_core::ids::HatId;
use den_memory::MemoryScopeType;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use super::chunking::Chunk;

/// Source class for the recall index payload (ADR-0038 §2).
pub const SOURCE_CLASS_BEAR_MEMORY: &str = "bear_memory";

/// Kinds never indexed regardless of scope (ephemeral / streaming content).
const EXCLUDED_KINDS: &[&str] = &["scratch", "log"];

/// The fields of a canonical memory record the indexer needs.
#[derive(Debug, Clone)]
pub struct IndexRequest {
    pub bear_id: Uuid,
    pub memory_id: String,
    /// Canonical SQLite `sequence_no` of the head record; drives the recall
    /// consistency watermark (ADR-0038 §8). Not part of the Qdrant payload.
    pub sequence_no: i64,
    pub logical_path: Option<String>,
    pub scope_type: String,
    pub scope_profile: Option<String>,
    pub scope_hat_id: Option<HatId>,
    pub work_surface_ref: Option<String>,
    pub kind: String,
    pub visibility: String,
    pub content_text: String,
    pub salience: String,
    pub lifecycle_status: String,
    pub freshness_trend: String,
    /// Resolved **descriptive** entity ids linked to this record (ADR-0042 §7 `recall_effect =
    /// boost`); denormalized into the passage payload so recall can filter/boost by entity. The
    /// access-bearing gate is excluded. Empty when the record has no descriptive relations.
    pub entity_ids: Vec<String>,
}

/// Whether a record is eligible for the recall index (ADR-0038 §4 default policy).
///
/// - Only `visibility = normal`.
/// - Never index ephemeral kinds (`scratch`, `log`).
/// - `shared` records: indexed.
/// - `hat` records: indexed only with a canonical typed hat ID.
/// - Raw `profile_local` and `source_local` records are never indexed, even for no-hat Bears.
pub fn is_indexable(scope_type: &str, kind: &str, visibility: &str) -> bool {
    if visibility != "normal" {
        return false;
    }
    if EXCLUDED_KINDS.contains(&kind) {
        return false;
    }
    matches!(
        MemoryScopeType::parse(scope_type),
        Some(MemoryScopeType::Shared | MemoryScopeType::Hat)
    )
}

impl IndexRequest {
    pub fn is_indexable(&self) -> bool {
        is_indexable(&self.scope_type, &self.kind, &self.visibility)
            && (MemoryScopeType::parse(&self.scope_type) == Some(MemoryScopeType::Hat))
                == self.scope_hat_id.is_some()
            && self.scope_profile.is_none()
            && !self.content_text.trim().is_empty()
            && !matches!(
                self.lifecycle_status.as_str(),
                "archived" | "archive-candidate" | "superseded"
            )
    }
}

/// Deterministic Qdrant point id for a (bear, memory, chunk, standard) tuple.
///
/// Derived from a SHA-256 of the key so re-indexing overwrites the same point. Formatted
/// as a UUID string (an id type Qdrant accepts).
pub fn point_id(
    bear_id: Uuid,
    memory_id: &str,
    chunk_index: i32,
    embedding_standard: &str,
) -> String {
    let key = format!("{bear_id}|{memory_id}|{chunk_index}|{embedding_standard}");
    let mut hasher = Sha256::new();
    hasher.update(key.as_bytes());
    let digest = hasher.finalize();
    let mut bytes = [0u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    Uuid::from_bytes(bytes).to_string()
}

/// Build the filterable Qdrant payload for a chunk (ADR-0038 §2).
///
/// `entity_id`s are intentionally omitted in Phase 1; carrying resolved entities into the
/// payload is the deferred entity-layer Phase 4 work.
pub fn build_payload(req: &IndexRequest, chunk: &Chunk, embedding_standard: &str) -> Value {
    json!({
        "embedding_standard": embedding_standard,
        "source_class": SOURCE_CLASS_BEAR_MEMORY,
        "content_hash": chunk.content_hash,
        "chunk_index": chunk.index,
        // Denormalized chunk text so recall can render snippets without a SQLite round-trip.
        // This is derived data; SQLite remains the source of truth (ADR-0038 §2).
        "text": chunk.text,
        "bear_id": req.bear_id.to_string(),
        "memory_id": req.memory_id,
        "scope_type": req.scope_type,
        "scope_profile": req.scope_profile,
        "scope_hat_id": req.scope_hat_id,
        "work_surface_ref": req.work_surface_ref,
        "logical_path": req.logical_path,
        "kind": req.kind,
        "visibility": req.visibility,
        "salience": req.salience,
        "lifecycle_status": req.lifecycle_status,
        "freshness_trend": req.freshness_trend,
        // Resolved descriptive entities for entity-scoped recall (filter/boost); derived data,
        // canonical relations remain the source of truth (ADR-0042 Phase 4 recall leg).
        "entity_ids": req.entity_ids,
    })
}

#[cfg(test)]
mod tests;
