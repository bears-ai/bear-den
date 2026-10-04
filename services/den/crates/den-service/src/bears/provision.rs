//! Initialize Bear memory, runtime plans, and managed configuration after Bear rows exist.
use sqlx::PgPool;
use uuid::Uuid;

use den_memory::MemoryStoreManager;

use super::db as bears_db;
use super::managed_blocks::compile_and_store_managed_config_for_bear;
use super::runtime_plan::default_runtime_plan;
use den_core::DenError;

/// Initialize local runtime prerequisites without creating or refreshing profile bindings.
/// The caller supplies the process-wide memory manager (or a clone sharing its pools).
pub async fn initialize_bear_native(
    pool: &PgPool,
    stores: &MemoryStoreManager,
    bear_id: Uuid,
) -> Result<(), DenError> {
    let bear = bears_db::get_bear(pool, bear_id)
        .await?
        .ok_or_else(|| DenError::NotFound("bear not found".to_string()))?;

    bears_db::ensure_default_runtime_plan(pool, bear_id, &default_runtime_plan()).await?;

    stores.store_for_bear(bear_id).await?;

    if bear.context_profile.is_some() {
        compile_and_store_managed_config_for_bear(pool, &bear).await?;
    }

    tracing::info!(%bear_id, "Bear native runtime prerequisites initialized");
    Ok(())
}
