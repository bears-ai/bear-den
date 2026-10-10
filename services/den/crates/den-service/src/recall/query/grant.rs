//! Derived recall filter for a Den-verified source/hat binding.
//!
//! This is a filter, not an authority source: callers must resolve the grant
//! from canonical Den state, and the index may lag or omit entire scopes.

use den_core::{config::Config, ids::BearId, DenError};
use sqlx::PgPool;

use crate::recall::authenticated_embedder;
use den_memory::scoped::MemoryReadGrant;
use serde_json::{json, Value};
use uuid::Uuid;

use super::{
    bear_scope_conditions, disabled_projection, search_passages, DisabledRecallReason,
    PassageEmbedder, QdrantRecall, RecallProjection,
};

#[cfg(test)]
mod tests;

fn grant_scope_filter(bear_id: Uuid, embedding_standard: &str, grant: MemoryReadGrant) -> Value {
    let mut should = vec![json!({ "key": "scope_type", "match": { "value": "shared" } })];
    if let Some(hat_id) = grant.hat_id() {
        should.push(json!({
            "must": [
                { "key": "scope_type", "match": { "value": "hat" } },
                { "key": "scope_hat_id", "match": { "value": hat_id.to_string() } },
            ]
        }));
    }
    let mut must = bear_scope_conditions(bear_id, embedding_standard);
    must.push(json!({ "should": should }));
    json!({ "must": must })
}

/// Turn-start semantic candidates from shared core and the verified hat only.
/// Source-local notes remain canonical SQLite reads, never derived recall.
/// Callers must reconstruct candidates from current SQLite before model projection.
pub async fn recall_for_turn_with_grant<E: PassageEmbedder + ?Sized>(
    qdrant: &QdrantRecall,
    embedder: &E,
    embedding_standard: &str,
    bear_id: Uuid,
    grant: MemoryReadGrant,
    query_text: &str,
    limit: usize,
) -> Result<RecallProjection, DenError> {
    search_passages(
        qdrant,
        embedder,
        grant_scope_filter(bear_id, embedding_standard, grant),
        embedding_standard,
        query_text,
        limit,
    )
    .await
}

pub async fn search_bear_memory_with_grant(
    pool: &PgPool,
    config: &Config,
    bear_id: Uuid,
    grant: MemoryReadGrant,
    query_text: &str,
    limit: usize,
) -> Result<RecallProjection, DenError> {
    let Some(qdrant) = QdrantRecall::from_config(config) else {
        return Ok(disabled_projection(DisabledRecallReason::QdrantUnset));
    };
    let Some(embedder) = authenticated_embedder(pool, config, BearId::new(bear_id)).await? else {
        return Ok(disabled_projection(DisabledRecallReason::EmbeddingsUnset));
    };
    recall_for_turn_with_grant(
        &qdrant,
        &embedder,
        &config.embedding_standard,
        bear_id,
        grant,
        query_text,
        limit,
    )
    .await
}
