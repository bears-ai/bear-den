//! Indexing policy + Qdrant payload shaping for Bear memory recall (ADR-0038 §2, §4).
//!
//! Policy decides *which* canonical records get embedded; payload shaping builds the
//! filterable Qdrant point payload and a deterministic point id so re-indexing the same
//! (bear, memory, chunk) tuple overwrites rather than duplicates.

use den_core::ids::HatId;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use super::chunking::Chunk;

/// Source class for the recall index payload (ADR-0038 §2).
pub const SOURCE_CLASS_BEAR_MEMORY: &str = "bear_memory";

/// Kinds never indexed regardless of scope (ephemeral / streaming content).
const EXCLUDED_KINDS: &[&str] = &["scratch", "log"];

/// Profile-local kinds that *are* worth indexing (ADR-0038 §4: note/decision/summary).
const PROFILE_LOCAL_INDEXABLE_KINDS: &[&str] = &["note", "decision", "summary"];

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
/// - `profile_local` records: only `note` / `decision` / `summary` (legacy).
pub fn is_indexable(scope_type: &str, kind: &str, visibility: &str) -> bool {
    if visibility != "normal" {
        return false;
    }
    if EXCLUDED_KINDS.contains(&kind) {
        return false;
    }
    match scope_type {
        "shared" | "hat" => true,
        "profile_local" => PROFILE_LOCAL_INDEXABLE_KINDS.contains(&kind),
        _ => false,
    }
}

impl IndexRequest {
    pub fn is_indexable(&self) -> bool {
        is_indexable(&self.scope_type, &self.kind, &self.visibility)
            && (self.scope_type == "hat") == self.scope_hat_id.is_some()
            && !matches!(self.lifecycle_status.as_str(), "archived" | "superseded")
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
mod tests {
    use super::*;

    #[test]
    fn shared_normal_records_are_indexable() {
        assert!(is_indexable("shared", "overview", "normal"));
        assert!(is_indexable("shared", "note", "normal"));
    }

    #[test]
    fn profile_local_only_indexes_select_kinds() {
        assert!(is_indexable("profile_local", "note", "normal"));
        assert!(is_indexable("profile_local", "decision", "normal"));
        assert!(is_indexable("profile_local", "summary", "normal"));
        assert!(!is_indexable("profile_local", "overview", "normal"));
    }

    #[test]
    fn non_normal_visibility_and_ephemeral_kinds_excluded() {
        assert!(!is_indexable("shared", "overview", "hidden"));
        assert!(!is_indexable("shared", "scratch", "normal"));
        assert!(!is_indexable("shared", "log", "normal"));
        assert!(!is_indexable("unknown_scope", "note", "normal"));
        assert!(is_indexable("hat", "note", "normal"));
        assert!(!is_indexable("source_local", "note", "normal"));
    }

    #[test]
    fn hat_index_requires_canonical_hat_id_and_never_indexes_raw_sources() {
        let hat = HatId::new(Uuid::new_v4());
        let req = IndexRequest {
            bear_id: Uuid::new_v4(),
            memory_id: Uuid::new_v4().to_string(),
            sequence_no: 1,
            logical_path: Some(format!("hat_memory/{hat}/note.md")),
            scope_type: "hat".into(),
            scope_profile: None,
            scope_hat_id: Some(hat),
            work_surface_ref: None,
            kind: "note".into(),
            visibility: "normal".into(),
            content_text: "reviewed knowledge".into(),
            salience: "normal".into(),
            lifecycle_status: "active".into(),
            freshness_trend: "stable".into(),
            entity_ids: vec![],
        };
        assert!(req.is_indexable());
        let payload = build_payload(
            &req,
            &Chunk {
                index: 0,
                text: "reviewed knowledge".into(),
                content_hash: "x".into(),
            },
            "test-standard",
        );
        assert_eq!(payload["scope_hat_id"], hat.to_string());
        assert!(!IndexRequest {
            scope_hat_id: None,
            ..req.clone()
        }
        .is_indexable());
        assert!(!IndexRequest {
            scope_type: "source_local".into(),
            ..req
        }
        .is_indexable());
    }

    #[test]
    fn point_id_is_deterministic_and_varies_by_chunk() {
        let bear = Uuid::nil();
        let a = point_id(bear, "mem-1", 0, "bears-embed-v1");
        let a2 = point_id(bear, "mem-1", 0, "bears-embed-v1");
        let b = point_id(bear, "mem-1", 1, "bears-embed-v1");
        let c = point_id(bear, "mem-2", 0, "bears-embed-v1");
        assert_eq!(a, a2);
        assert_ne!(a, b);
        assert_ne!(a, c);
        // Valid UUID format.
        assert!(Uuid::parse_str(&a).is_ok());
    }

    #[test]
    fn archived_lifecycle_records_are_not_indexable() {
        let req = IndexRequest {
            bear_id: Uuid::nil(),
            memory_id: "mem-archived".into(),
            sequence_no: 1,
            logical_path: Some("core/old.md".into()),
            scope_type: "shared".into(),
            scope_profile: None,
            scope_hat_id: None,
            work_surface_ref: None,
            kind: "note".into(),
            visibility: "normal".into(),
            content_text: "old body".into(),
            salience: "normal".into(),
            lifecycle_status: "archived".into(),
            freshness_trend: "stale".into(),
            entity_ids: Vec::new(),
        };
        assert!(!req.is_indexable());
    }

    #[test]
    fn payload_carries_required_fields() {
        let req = IndexRequest {
            bear_id: Uuid::nil(),
            memory_id: "mem-1".into(),
            sequence_no: 1,
            logical_path: Some("core/work_surfaces/x/overview.md".into()),
            scope_type: "shared".into(),
            scope_profile: None,
            scope_hat_id: None,
            work_surface_ref: Some("x".into()),
            kind: "overview".into(),
            visibility: "normal".into(),
            content_text: "body".into(),
            salience: "high".into(),
            lifecycle_status: "active".into(),
            freshness_trend: "stable".into(),
            entity_ids: vec!["ent-1".into(), "ent-2".into()],
        };
        let chunk = Chunk {
            index: 0,
            text: "body".into(),
            content_hash: "abc".into(),
        };
        let payload = build_payload(&req, &chunk, "bears-embed-v1");
        assert_eq!(payload["source_class"], SOURCE_CLASS_BEAR_MEMORY);
        assert_eq!(payload["embedding_standard"], "bears-embed-v1");
        assert_eq!(payload["memory_id"], "mem-1");
        assert_eq!(payload["chunk_index"], 0);
        assert_eq!(payload["content_hash"], "abc");
        assert_eq!(payload["work_surface_ref"], "x");
        assert!(payload["scope_hat_id"].is_null());
        assert_eq!(payload["kind"], "overview");
        assert_eq!(payload["salience"], "high");
        assert_eq!(payload["lifecycle_status"], "active");
        assert_eq!(payload["freshness_trend"], "stable");
        assert_eq!(payload["text"], "body");
        assert_eq!(payload["entity_ids"], serde_json::json!(["ent-1", "ent-2"]));
    }
}
