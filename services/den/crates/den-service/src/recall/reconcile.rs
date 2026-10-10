//! Whole-Bear reconcile (ADR-0038 §6): bring the derived recall index in line with the
//! canonical SQLite heads. Indexes every indexable head record and removes passages for
//! memory ids that are no longer heads (supersede/delete). Idempotent and bounded per Bear.

use std::collections::HashSet;

use sqlx::PgPool;
use uuid::Uuid;

use den_core::{
    config::Config,
    ids::{BearId, HatId},
    DenError,
};

use den_memory::MemoryStoreManager;
use den_memory::{relations, BearMemoryStore, MemoryScopeType};

use super::indexer::{PassageEmbedder, RecallIndexer};
use super::policy::{is_indexable, IndexRequest};
use super::qdrant::QdrantRecall;
use super::registry;
use crate::bears::hats;

#[cfg(test)]
mod tests;

/// Aggregate result of a reconcile pass (diagnostics + tests).
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ReconcileOutcome {
    pub indexed_records: usize,
    pub embedded_chunks: usize,
    pub reused_chunks: usize,
    pub removed_records: usize,
    pub removed_points: usize,
}

/// The current indexable head record per logical path for a Bear (latest, normal-visibility,
/// not superseded). Non-indexable kinds/scopes are filtered out per [`is_indexable`].
#[derive(Debug, Clone, sqlx::FromRow)]
struct HeadRow {
    memory_id: String,
    sequence_no: i64,
    scope_type: String,
    scope_profile: Option<String>,
    scope_hat_id: Option<String>,
    kind: String,
    visibility: String,
    salience: String,
    metadata_json: String,
    supersedes_memory_id: Option<String>,
    invalid_at: Option<String>,
    logical_path: Option<String>,
    work_surface_ref: Option<String>,
    content_text: String,
}

pub async fn list_indexable_heads(store: &BearMemoryStore) -> Result<Vec<IndexRequest>, DenError> {
    let bear_id = store.bear_id();
    // sqlx-dynamic: canonical per-Bear SQLite has no compile-time query metadata;
    // this fixed query binds the Bear ID and never derives scope from a path.
    let rows = sqlx::query_as::<_, HeadRow>(
        r"
        SELECT m.memory_id, m.sequence_no, m.scope_type, m.scope_profile, m.scope_hat_id, m.kind, m.visibility,
               m.salience, m.metadata_json, m.supersedes_memory_id, m.invalid_at,
               m.logical_path, m.work_surface_ref, m.content_text
        FROM memory_records m
        WHERE m.bear_id = ?
          AND m.scope_type IN ('shared', 'hat')
          AND m.scope_profile IS NULL
          AND m.scope_source_kind IS NULL
          AND m.scope_source_id IS NULL
          AND m.visibility = 'normal'
          AND m.invalid_at IS NULL
          AND m.logical_path IS NOT NULL
          AND NOT EXISTS (
              SELECT 1 FROM memory_access_rules rules
              WHERE rules.bear_id = m.bear_id AND rules.src_memory_id = m.memory_id
          )
          AND NOT EXISTS (
              SELECT 1 FROM memory_records n
              WHERE n.bear_id = m.bear_id AND n.supersedes_memory_id = m.memory_id
          )
          AND m.sequence_no = (
              SELECT MAX(h.sequence_no) FROM memory_records h
              WHERE h.bear_id = m.bear_id
                AND h.logical_path = m.logical_path
                AND h.scope_type = m.scope_type
                AND h.scope_hat_id IS m.scope_hat_id
                AND h.visibility = 'normal'
          )
        ORDER BY m.logical_path
        ",
    )
    .bind(bear_id.to_string())
    .fetch_all(store.pool())
    .await
    .map_err(|e| DenError::System(format!("list indexable heads: {e}")))?;

    // One bulk pass for resolved descriptive entities, denormalized into each passage payload.
    let entity_ids_by_source = relations::descriptive_entity_ids_by_source(store).await?;

    Ok(rows
        .into_iter()
        .filter_map(|head| {
            if !is_indexable(&head.scope_type, &head.kind, &head.visibility) {
                return None;
            }
            let scope_hat_id = match head.scope_hat_id {
                Some(ref id) => Some(id.parse::<HatId>().ok()?),
                None => None,
            };
            if (head.scope_type == "hat") != scope_hat_id.is_some() {
                return None;
            }
            let entity_ids = entity_ids_by_source
                .get(&head.memory_id)
                .cloned()
                .unwrap_or_default();
            let metadata_json: serde_json::Value =
                serde_json::from_str(&head.metadata_json).unwrap_or_else(|_| serde_json::json!({}));
            let lifecycle_status = den_memory::lifecycle_status(
                &metadata_json,
                head.supersedes_memory_id.as_deref(),
                head.invalid_at.as_deref(),
            );
            let freshness_trend =
                den_memory::freshness_trend(&lifecycle_status, head.invalid_at.as_deref());
            let req = IndexRequest {
                bear_id,
                memory_id: head.memory_id,
                sequence_no: head.sequence_no,
                logical_path: head.logical_path,
                scope_type: head.scope_type,
                scope_profile: head.scope_profile,
                scope_hat_id,
                work_surface_ref: head.work_surface_ref,
                kind: head.kind,
                visibility: head.visibility,
                content_text: head.content_text,
                salience: head.salience,
                lifecycle_status,
                freshness_trend,
                entity_ids,
            };
            req.is_indexable().then_some(req)
        })
        .collect())
}

/// Canonical shared heads plus curated heads belonging to the Bear's current
/// hats. Raw legacy/source records and missing/foreign hats never enter this
/// set. Both reconciliation and the watermark use this exact eligibility set.
pub async fn list_authorized_indexable_heads(
    pg: &PgPool,
    store: &BearMemoryStore,
) -> Result<Vec<IndexRequest>, DenError> {
    let mut heads = list_indexable_heads(store).await?;
    let owned_hats: HashSet<HatId> = hats::list_hats(pg, BearId::new(store.bear_id()))
        .await?
        .into_iter()
        .map(|hat| hat.id)
        .collect();
    heads.retain(|head| match MemoryScopeType::parse(&head.scope_type) {
        Some(MemoryScopeType::Shared) => head.scope_hat_id.is_none(),
        Some(MemoryScopeType::Hat) => head.scope_hat_id.is_some_and(|id| owned_hats.contains(&id)),
        _ => false,
    });
    Ok(heads)
}

/// Reconcile a Bear's recall index against its canonical heads.
pub async fn reconcile_bear<E: PassageEmbedder>(
    pg: &PgPool,
    qdrant: &QdrantRecall,
    embedder: &E,
    store: &BearMemoryStore,
    embedding_standard: &str,
) -> Result<ReconcileOutcome, DenError> {
    let bear_id = store.bear_id();
    let heads = list_authorized_indexable_heads(pg, store).await?;
    let indexer = RecallIndexer::new(pg, qdrant, embedder, embedding_standard.to_string());

    let mut outcome = ReconcileOutcome::default();
    let head_ids: HashSet<&str> = heads.iter().map(|head| head.memory_id.as_str()).collect();
    // Remove ineligible derived data before any embedding call can fail. Canonical
    // SQLite history is untouched; no legacy record is promoted or reassigned.
    let indexed_ids = registry::list_indexed_memory_ids(pg, bear_id, embedding_standard).await?;
    for mid in indexed_ids {
        if !head_ids.contains(mid.as_str()) {
            let removed = indexer.remove_record(bear_id, &mid).await?;
            outcome.removed_records += 1;
            outcome.removed_points += removed;
        }
    }
    for req in &heads {
        let o = indexer.index_record(req).await?;
        outcome.indexed_records += 1;
        outcome.embedded_chunks += o.embedded_chunks;
        outcome.reused_chunks += o.reused_chunks;
        outcome.removed_points += o.removed_points;
    }

    Ok(outcome)
}

/// Operator-facing synchronous reindex (ADR-0038 Phase 5 tooling): reconcile one Bear's recall
/// index immediately with the live embedding client, bypassing the `recall_index` queue. Used by
/// the `den reindex` CLI. Errors if recall is disabled (`QDRANT_URL` unset).
pub async fn reindex_bear_now(
    pg: &PgPool,
    config: &Config,
    stores: &MemoryStoreManager,
    bear_id: Uuid,
) -> Result<ReconcileOutcome, DenError> {
    let qdrant = QdrantRecall::from_config(config)
        .ok_or_else(|| DenError::System("recall disabled (QDRANT_URL unset)".to_string()))?;
    let embedder = super::authenticated_embedder(pg, config, BearId::new(bear_id))
        .await?
        .ok_or_else(|| DenError::System("embeddings API is not configured".into()))?;
    let store = stores.store_for_bear(bear_id).await?;
    reconcile_bear(pg, &qdrant, &embedder, &store, &config.embedding_standard).await
}
