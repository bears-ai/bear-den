//! `den`-side wiring for the memory read tools.
//!
//! Orchestration (arg parsing, validation, status diagnostic composition) lives
//! in `den-tools`; here we provide the concrete [`RoleMemoryStore`] over the
//! native per-Bear SQLite store, plus thin wrappers that adapt
//! `DenToolInvocationContext` and map `DenError` back to `CustomError`.

use serde_json::Value;
use sqlx::PgPool;
use uuid::Uuid;

use den_core::tools::memory::{RoleMemoryEntryWrite, RoleMemoryStore};

use crate::{
    config::Config,
    core::tools::{prompt_memory::DenPromptMemoryStore, session::DenToolInvocationContext},
    errors::{CustomError, DenError},
};
use den_memory::{tools as sqlite_memory, MemoryStoreManager};
use den_service::bears::BearProfile;

/// Concrete [`RoleMemoryStore`] over the native SQLite memory runtime.
pub(crate) struct DenRoleMemoryStore<'a> {
    pool: &'a PgPool,
    config: &'a Config,
    stores: &'a MemoryStoreManager,
}

impl<'a> DenRoleMemoryStore<'a> {
    pub(crate) fn new(
        pool: &'a PgPool,
        config: &'a Config,
        stores: &'a MemoryStoreManager,
    ) -> Self {
        Self {
            pool,
            config,
            stores,
        }
    }
}

impl RoleMemoryStore for DenRoleMemoryStore<'_> {
    async fn read(&self, bear_id: Uuid, role: BearProfile, path: &str) -> Result<Value, DenError> {
        let store = self.stores.store_for_bear(bear_id).await?;
        sqlite_memory::sqlite_memory_read_for_profile(&store, role, path).await
    }

    async fn browse(&self, bear_id: Uuid, role: BearProfile) -> Result<Value, DenError> {
        let store = self.stores.store_for_bear(bear_id).await?;
        sqlite_memory::sqlite_memory_browse(&store, role.as_str()).await
    }

    /// Hybrid `memory_search` (ADR-0038 Phase 3): the **union** of the derived vector recall index
    /// and the SQL `LIKE` keyword leg, both scoped to memory visible to `role` (shared + own
    /// role-local). Vector hits rank first; keyword-only matches fill remaining slots. Fail-open:
    /// the vector leg degrades to keyword-only on any recall error (see `hybrid_memory_search`).
    async fn search(
        &self,
        bear_id: Uuid,
        role: BearProfile,
        query: &str,
        limit: i64,
    ) -> Result<Value, DenError> {
        let limit = usize::try_from(limit).unwrap_or(10);
        den_runtime::recall::hybrid_memory_search(
            self.config,
            self.stores,
            bear_id,
            role.as_str(),
            query,
            limit,
        )
        .await
    }

    /// Base status plus the recall consistency watermark (ADR-0038 §8): a `recall`
    /// object carrying `indexed_seq` / `canonical_seq` / `lag_count` / `fully_recallable`
    /// / `last_success_at` / `failed_run_count`, or `{"available": false, ...}` when
    /// recall is not configured. Watermark errors degrade the `recall` object rather
    /// than failing the whole status.
    async fn status_base(&self, bear_id: Uuid, role: BearProfile) -> Result<Value, DenError> {
        let store = self.stores.store_for_bear(bear_id).await?;
        let mut base = sqlite_memory::sqlite_memory_status(&store, role.as_str()).await?;
        let recall = match den_runtime::recall::recall_watermark(self.pool, self.config, &store)
            .await
        {
            Ok(watermark) => den_runtime::recall::recall_status_json(watermark.as_ref()),
            Err(err) => {
                serde_json::json!({ "available": false, "reason": format!("watermark unavailable: {err}") })
            }
        };
        if let Some(obj) = base.as_object_mut() {
            obj.insert("recall".to_string(), recall);
        }
        Ok(base)
    }

    async fn write_entry(
        &self,
        bear_id: Uuid,
        role: BearProfile,
        entry: RoleMemoryEntryWrite,
    ) -> Result<Value, DenError> {
        let written = sqlite_memory::sqlite_write_profile_entry(
            self.stores,
            bear_id,
            role.as_str(),
            &entry.kind,
            &entry.title,
            &entry.body,
            &entry.tags,
            entry.refs,
            entry.lifecycle,
            entry.source,
            entry.author,
        )
        .await?;
        // Async-index this write into derived recall (ADR-0038 Phase 1b); best-effort.
        den_runtime::reflection_conductor::enqueue_recall_index_if_enabled(
            self.pool,
            self.config,
            bear_id,
            "role_memory_write_entry",
        )
        .await;
        Ok(written)
    }
}

pub(crate) async fn memory_status(
    pool: &PgPool,
    config: &Config,
    stores: &MemoryStoreManager,
    context: &DenToolInvocationContext,
    role: BearProfile,
) -> Result<Value, CustomError> {
    let memory = DenRoleMemoryStore::new(pool, config, stores);
    let prompt = DenPromptMemoryStore::new(pool);
    den_core::tools::memory::memory_status(&memory, &prompt, context.bear_id, role)
        .await
        .map_err(CustomError::from)
}

pub(crate) async fn memory_status_value(
    config: &Config,
    stores: &MemoryStoreManager,
    context: &DenToolInvocationContext,
    role: BearProfile,
    pool: &PgPool,
) -> Result<Value, CustomError> {
    memory_status(pool, config, stores, context, role).await
}
