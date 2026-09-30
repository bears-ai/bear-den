//! Curated semantic retrieval for Bear members. The vector index may rank
//! candidate IDs, but canonical SQLite alone decides visibility and snippet text.

use den_core::{config::Config, DenError};
use den_memory::{
    library::{self, CuratedMemoryGrant},
    BearMemoryStore,
};
use serde_json::{json, Value};
use uuid::Uuid;

use super::{
    bear_scope_conditions, disabled_projection, search_passages, DisabledRecallReason,
    QdrantRecall, RecallProjection, RecalledPassage,
};

#[cfg(test)]
mod live_qdrant_tests;
#[cfg(test)]
mod tests;

fn curated_scope_filter(
    bear_id: Uuid,
    embedding_standard: &str,
    grant: &CuratedMemoryGrant,
) -> Value {
    let mut should = vec![json!({ "key": "scope_type", "match": { "value": "shared" } })];
    if !grant.hat_ids().is_empty() {
        should.push(json!({
            "must": [
                { "key": "scope_type", "match": { "value": "hat" } },
                { "key": "scope_hat_id", "match": { "any": grant.hat_ids().iter().map(ToString::to_string).collect::<Vec<_>>() } },
            ]
        }));
    }
    let mut must = bear_scope_conditions(bear_id, embedding_standard);
    must.push(json!({ "should": should }));
    json!({ "must": must })
}

pub async fn search_curated_library(
    config: &Config,
    bear_id: Uuid,
    grant: &CuratedMemoryGrant,
    query_text: &str,
    limit: usize,
) -> Result<RecallProjection, DenError> {
    let Some(qdrant) = QdrantRecall::from_config(config) else {
        return Ok(disabled_projection(DisabledRecallReason::QdrantUnset));
    };
    let embedder = den_llm::EmbeddingClient::new(config);
    if !embedder.is_enabled() {
        return Ok(disabled_projection(DisabledRecallReason::EmbeddingsUnset));
    }
    // Fetch the bounded candidate pool before the canonical post-filter; stale
    // or forbidden hits must not displace an authorized hit in the same page.
    search_passages(
        &qdrant,
        &embedder,
        curated_scope_filter(bear_id, &config.embedding_standard, grant),
        &config.embedding_standard,
        query_text,
        limit.saturating_mul(3).min(50),
    )
    .await
}

pub async fn retain_curated_candidates(
    store: &BearMemoryStore,
    grant: &CuratedMemoryGrant,
    projection: &mut RecallProjection,
    limit: usize,
) -> Result<(), DenError> {
    let mut admitted = Vec::new();
    for passage in projection.passages.drain(..) {
        let Some(record) = library::current_detail(store, grant, &passage.memory_id).await? else {
            continue;
        };
        admitted.push(RecalledPassage {
            memory_id: record.memory_id,
            logical_path: record.logical_path,
            kind: Some(record.kind),
            score: passage.score,
            salience: "normal".into(),
            lifecycle_status: "active".into(),
            freshness_trend: "stable".into(),
            // Do not echo a passage chunk, path, scope, or text from Qdrant.
            text: record.content_text.chars().take(480).collect(),
            conflicts_with: Vec::new(),
        });
        if admitted.len() >= limit {
            break;
        }
    }
    projection.passages = admitted;
    Ok(())
}
