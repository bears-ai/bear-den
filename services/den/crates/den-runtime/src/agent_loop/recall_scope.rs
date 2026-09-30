//! Recheck derived recall against canonical SQLite before projecting bound memory.
//! A stale or mis-scoped Qdrant payload is not an authorization decision.

use den_core::DenError;
use den_memory::{
    scoped::{self, MemoryReadGrant},
    AccessContext, BearMemoryStore,
};
use den_service::recall::RecallProjection;

#[cfg(test)]
mod tests;

pub(super) async fn retain_canonical_passages(
    store: &BearMemoryStore,
    grant: MemoryReadGrant,
    projection: &mut RecallProjection,
) -> Result<(), DenError> {
    let mut admitted = Vec::new();
    for passage in projection.passages.drain(..) {
        let Some(path) = passage.logical_path.as_deref() else {
            continue;
        };
        let records = scoped::read_path(store, grant, &AccessContext::empty(), path, 64).await?;
        if let Some(record) = records
            .into_iter()
            .find(|record| record.memory_id == passage.memory_id)
        {
            let mut passage = passage;
            passage.logical_path = record.logical_path;
            passage.kind = Some(record.kind);
            passage.salience = record.salience;
            passage.lifecycle_status = record.lifecycle_status;
            passage.freshness_trend = record.freshness_trend;
            passage.text = record.content_text.chars().take(480).collect();
            admitted.push(passage);
        }
    }
    projection.passages = admitted;
    Ok(())
}
