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
        if records
            .iter()
            .any(|record| record.memory_id == passage.memory_id)
        {
            admitted.push(passage);
        }
    }
    projection.passages = admitted;
    Ok(())
}
