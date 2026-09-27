use serde_json::{json, Value};
use sqlx::PgPool;

use den_core::tools::environment::EnvironmentOps;
use den_core::tools::prompt_memory::PromptMemoryVisibility;

use crate::{
    config::Config,
    core::tools::{
        memory_read::{memory_status_value, DenRoleMemoryStore},
        session::DenToolInvocationContext,
    },
    errors::{CustomError, DenError},
};
use den_memory as memory_store;
use den_memory::MemoryStoreManager;
use den_service::bears::BearProfile;
use den_service::client_sessions;

/// Concrete [`EnvironmentOps`] over the runtime pool/config.
pub(crate) struct DenEnvironmentOps<'a> {
    pub(crate) pool: &'a PgPool,
    pub(crate) config: &'a Config,
    pub(crate) stores: &'a MemoryStoreManager,
}

impl EnvironmentOps for DenEnvironmentOps<'_> {
    fn uses_native_runtime(&self) -> bool {
        true
    }

    async fn memory_status_value(
        &self,
        context: &DenToolInvocationContext,
        role: BearProfile,
    ) -> Result<Value, DenError> {
        memory_status_value(self.config, self.stores, context, role, self.pool)
            .await
            .map_err(CustomError::into_den)
    }

    async fn memory_visibility(
        &self,
        context: &DenToolInvocationContext,
        role: BearProfile,
    ) -> Result<PromptMemoryVisibility, DenError> {
        DenRoleMemoryStore::new(self.pool, self.config, self.stores)
            .prompt_visibility(context, role)
            .await
    }

    async fn session_entities(
        &self,
        context: &DenToolInvocationContext,
        _role: BearProfile,
    ) -> Result<Value, DenError> {
        let store = self.stores.store_for_bear(context.bear_id).await?;
        let mut human = Value::Null;
        for (handle_type, handle_value) in [
            ("den_user", Some(context.user_id.to_string())),
            ("session_human", context.username.clone()),
        ] {
            let Some(value) = handle_value
                .as_deref()
                .map(str::trim)
                .filter(|v| !v.is_empty())
            else {
                continue;
            };
            if let Some(entity) =
                memory_store::find_entity_by_handle(&store, handle_type, value).await?
            {
                human = json!({
                    "source": handle_type,
                    "entity": entity,
                });
                break;
            }
        }

        let mut work_surface = Value::Null;
        let mut candidates: Vec<(&str, String)> = Vec::new();
        candidates.extend(
            context
                .workspace_roots
                .iter()
                .cloned()
                .map(|root| ("workspace_root", root)),
        );
        candidates.extend(
            context
                .workspace_roots
                .iter()
                .cloned()
                .map(|root| ("checkout", root)),
        );
        if let Some(target) = context.runtime_target.clone() {
            candidates.push(("checkout", target));
        }
        for (handle_type, value) in candidates {
            let value = value.trim();
            if value.is_empty() {
                continue;
            }
            if let Some(entity) =
                memory_store::find_entity_by_handle(&store, handle_type, value).await?
            {
                work_surface = json!({
                    "source": handle_type,
                    "entity": entity,
                });
                break;
            }
        }

        Ok(json!({
            "status": "ok",
            "human": human,
            "work_surface": work_surface,
        }))
    }

    async fn fetch_adapter_environment(
        &self,
        context: &DenToolInvocationContext,
    ) -> Result<Option<Value>, DenError> {
        let Some(client_session_id) = context.client_session_id.as_deref() else {
            return Ok(None);
        };
        let session = client_sessions::find_for_user_bear_session(
            self.pool,
            context.user_id,
            &context.bear_slug,
            client_session_id,
        )
        .await?;
        Ok(session.map(|session| {
            let workspace = session.trusted_workspace_context();
            json!({
                "status": "ok",
                "adapter_environment": session.adapter_environment,
                "trusted_workspace": workspace,
            })
        }))
    }
}
