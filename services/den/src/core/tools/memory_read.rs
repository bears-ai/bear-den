//! `den`-side wiring for the memory read tools.
//!
//! Orchestration (arg parsing, validation, status diagnostic composition) lives
//! in `den-tools`; here we provide the concrete [`RoleMemoryStore`] over the
//! native per-Bear SQLite store, plus thin wrappers that adapt
//! `DenToolInvocationContext` and map `DenError` back to `CustomError`.

use serde_json::{json, Value};
use sqlx::PgPool;

use den_core::tools::memory::{RoleMemoryEntryWrite, RoleMemoryStore};
use den_core::tools::prompt_memory::PromptMemoryVisibility;

use crate::{
    config::Config,
    core::tools::{prompt_memory::DenPromptMemoryStore, session::DenToolInvocationContext},
    errors::{CustomError, DenError},
};
use den_core::ids::BearId;
use den_memory::{scoped, tools as sqlite_memory, AccessContext, MemoryStoreManager};
use den_service::bears::{
    hats::memory_binding::{self, ResolvedMemoryBinding},
    RuntimeContextLabel,
};

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

    async fn binding(
        &self,
        context: &DenToolInvocationContext,
        role: RuntimeContextLabel,
    ) -> Result<ResolvedMemoryBinding, DenError> {
        let bear_id = BearId::new(context.bear_id);
        match role {
            RuntimeContextLabel::Curation | RuntimeContextLabel::Observation => {
                Err(DenError::Authorization(
                    "internal workers cannot use ordinary model-facing memory tools".into(),
                ))
            }
            RuntimeContextLabel::JobRun => {
                let run_id = context.work_run_id.ok_or_else(|| {
                    DenError::Authorization("Work memory requires an authenticated Work run".into())
                })?;
                memory_binding::for_work_run(self.pool, bear_id, run_id).await
            }
            RuntimeContextLabel::ArmatureConversation
            | RuntimeContextLabel::ChannelConversation => {
                memory_binding::for_external_conversation(
                    self.pool,
                    bear_id,
                    &context.conversation_id,
                )
                .await
            }
        }
    }

    pub(crate) async fn prompt_visibility(
        &self,
        context: &DenToolInvocationContext,
        role: RuntimeContextLabel,
    ) -> Result<PromptMemoryVisibility, DenError> {
        self.binding(context, role).await?;
        Ok(PromptMemoryVisibility::BoundSession)
    }
}

impl RoleMemoryStore for DenRoleMemoryStore<'_> {
    async fn read(
        &self,
        context: &DenToolInvocationContext,
        role: RuntimeContextLabel,
        path: &str,
    ) -> Result<Value, DenError> {
        let ResolvedMemoryBinding::Bound(grant) = self.binding(context, role).await?;
        let store = self.stores.store_for_bear(context.bear_id).await?;
        let records = scoped::read_path(&store, grant, &AccessContext::empty(), path, 20).await?;
        Ok(sqlite_memory::render_memory_read(path, records))
    }

    async fn browse(
        &self,
        context: &DenToolInvocationContext,
        role: RuntimeContextLabel,
    ) -> Result<Value, DenError> {
        let ResolvedMemoryBinding::Bound(grant) = self.binding(context, role).await?;
        let store = self.stores.store_for_bear(context.bear_id).await?;
        let paths = scoped::browse(&store, grant, &AccessContext::empty()).await?;
        let children: Vec<Value> = paths.into_iter().map(|path| {
            json!({ "name": path.rsplit('/').next().unwrap_or(&path), "path": path, "type": "file" })
        }).collect();
        Ok(json!({ "ok": true, "configured": true, "storage": "sqlite", "children": children }))
    }

    async fn search(
        &self,
        context: &DenToolInvocationContext,
        role: RuntimeContextLabel,
        query: &str,
        limit: i64,
    ) -> Result<Value, DenError> {
        let ResolvedMemoryBinding::Bound(grant) = self.binding(context, role).await?;
        // Until the derived index is access-gated at turn time, do not union
        // potentially stale vector/graph hits into a bound session's results.
        let store = self.stores.store_for_bear(context.bear_id).await?;
        let records = scoped::search(&store, grant, &AccessContext::empty(), query, limit).await?;
        let hits: Vec<Value> = records
            .into_iter()
            .map(|record| {
                json!({
                    "memory_id": record.memory_id,
                    "path": record.logical_path,
                    "kind": record.kind,
                    "salience": record.salience,
                    "lifecycle_status": record.lifecycle_status,
                    "freshness_trend": record.freshness_trend,
                    "supersedes_memory_id": record.supersedes_memory_id,
                    "invalid_at": record.invalid_at,
                    "score": Value::Null,
                    "snippet": record.content_text.chars().take(240).collect::<String>(),
                    "sequence_no": record.sequence_no,
                })
            })
            .collect();
        Ok(
            json!({ "ok": true, "configured": true, "storage": "sqlite", "strategy": "keyword", "query": query, "hits": hits }),
        )
    }

    async fn status_base(
        &self,
        context: &DenToolInvocationContext,
        role: RuntimeContextLabel,
    ) -> Result<(Value, PromptMemoryVisibility), DenError> {
        let ResolvedMemoryBinding::Bound(grant) = self.binding(context, role).await?;
        let store = self.stores.store_for_bear(context.bear_id).await?;
        let paths = scoped::browse(&store, grant, &AccessContext::empty()).await?;
        Ok((
            json!({
                "configured": true, "available": true, "storage": "sqlite", "scope": "bound",
                "file_count": paths.len(),
                "recall": { "available": false, "reason": "scope_limited" }
            }),
            PromptMemoryVisibility::BoundSession,
        ))
    }

    async fn write_entry(
        &self,
        context: &DenToolInvocationContext,
        role: RuntimeContextLabel,
        entry: RoleMemoryEntryWrite,
    ) -> Result<Value, DenError> {
        let ResolvedMemoryBinding::Bound(grant) = self.binding(context, role).await?;
        let written = sqlite_memory::sqlite_write_source_entry(
            self.stores,
            context.bear_id,
            grant.source(),
            role.as_str(),
            sqlite_memory::SqliteMemoryEntryWrite {
                kind: &entry.kind,
                title: &entry.title,
                body: &entry.body,
                tags: &entry.tags,
                refs: entry.refs,
                lifecycle: entry.lifecycle,
                source: entry.source,
                author: entry.author,
            },
        )
        .await?;
        // Async-index this write into derived recall (ADR-0038 Phase 1b); best-effort.
        den_runtime::reflection_conductor::enqueue_recall_index_if_enabled(
            self.pool,
            self.config,
            context.bear_id,
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
    role: RuntimeContextLabel,
) -> Result<Value, CustomError> {
    let memory = DenRoleMemoryStore::new(pool, config, stores);
    let prompt = DenPromptMemoryStore::new(pool);
    den_core::tools::memory::memory_status(&memory, &prompt, context, role)
        .await
        .map_err(CustomError::from)
}

pub(crate) async fn memory_status_value(
    config: &Config,
    stores: &MemoryStoreManager,
    context: &DenToolInvocationContext,
    role: RuntimeContextLabel,
    pool: &PgPool,
) -> Result<Value, CustomError> {
    memory_status(pool, config, stores, context, role).await
}
