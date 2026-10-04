//! The unified [`DenToolContext`] — the single runtime type that implements every
//! `den_core::tools` capability sub-trait, so the relocated dispatcher
//! ([`den_core::tools::dispatch::invoke_den_tool`]) can take one `&impl ToolContext`.
//!
//! Each impl delegates to the per-capability concrete type already defined in the
//! sibling tool modules (web/runtime, memory_read, prompt_memory, memory_review,
//! work_surface, plan_mode, identity, environment, session). See
//! `docs/roadmap/DEN_CRATE_SPLIT_PLAN.md` (Phase B — dispatcher).

use serde_json::Value;
use sqlx::PgPool;
use uuid::Uuid;

use den_core::tools::context::DenToolInvocationContext;
use den_core::tools::{
    conversation::ConversationTitleOps,
    dispatch::ToolContext,
    entity::EntityOps,
    environment::EnvironmentOps,
    identity::{BearDirectory, BearMemberRecord, BearRecord, CurrentUser, SourceAuthorizer},
    memory::{RoleMemoryEntryWrite, RoleMemoryStore},
    plan_mode::{PlanModeExitView, PlanModeOps, PlanModeStatusView, PlanModeView},
    prompt_memory::{
        PromptMemoryBlock, PromptMemoryBlockPatch, PromptMemoryBlockWrite, PromptMemoryStore,
    },
    review::{
        MarkMemoryLifecycleRequest, MemoryProposalStatus, MemoryReviewStore, ObservationRecord,
        ObservationWriteRequest, RequestReviewRequest, ResolveProposalRequest,
    },
    web::{WebApproval, WebFetchAudit, WebFetcher, WebHttpResponse, WebUrl},
};

use crate::{
    config::Config,
    core::tools::{
        activity_payloads::{no_active_workplan_payload, plan_mode_workplan_payload},
        entity::DenEntityOps,
        environment::DenEnvironmentOps,
        identity::DenBearDirectory,
        memory_read::DenRoleMemoryStore,
        memory_review::DenMemoryReviewStore,
        plan_mode::DenPlanModeOps,
        prompt_memory::DenPromptMemoryStore,
        session::DenConversationTitleOps,
        web::runtime::DenWebFetcher,
    },
    errors::DenError,
};
use den_memory::MemoryStoreManager;
use den_service::bears::RuntimeContextLabel;

/// The composition root binding every Den tool capability to the runtime.
pub(crate) struct DenToolContext<'a> {
    pub(crate) pool: &'a PgPool,
    pub(crate) config: &'a Config,
    pub(crate) stores: &'a MemoryStoreManager,
}

impl<'a> DenToolContext<'a> {
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

    fn web(&self) -> DenWebFetcher<'a> {
        DenWebFetcher {
            pool: self.pool,
            config: self.config,
        }
    }

    fn memory(&self) -> DenRoleMemoryStore<'a> {
        DenRoleMemoryStore::new(self.pool, self.config, self.stores)
    }

    fn prompt(&self) -> DenPromptMemoryStore<'a> {
        DenPromptMemoryStore::new(self.pool)
    }

    fn review(&self) -> DenMemoryReviewStore<'a> {
        DenMemoryReviewStore::new(self.pool, self.config, self.stores)
    }

    fn plan_mode(&self) -> DenPlanModeOps<'a> {
        DenPlanModeOps {
            pool: self.pool,

            workplan_payload: plan_mode_workplan_payload,
            no_active_workplan: no_active_workplan_payload,
        }
    }

    fn directory(&self) -> DenBearDirectory<'a> {
        DenBearDirectory { pool: self.pool }
    }

    fn environment(&self) -> DenEnvironmentOps<'a> {
        DenEnvironmentOps {
            pool: self.pool,
            config: self.config,
            stores: self.stores,
        }
    }

    fn entity(&'a self) -> DenEntityOps<'a> {
        DenEntityOps::new(self)
    }

    fn conversation(&self) -> DenConversationTitleOps<'a> {
        DenConversationTitleOps { pool: self.pool }
    }
}

impl WebFetcher for DenToolContext<'_> {
    async fn decide_fetch_approval(
        &self,
        context: &DenToolInvocationContext,
        raw_url: &str,
    ) -> Result<(WebUrl, WebApproval), DenError> {
        self.web().decide_fetch_approval(context, raw_url).await
    }

    async fn record_fetch_attempt(&self, audit: WebFetchAudit<'_>) -> Result<(), DenError> {
        self.web().record_fetch_attempt(audit).await
    }

    async fn http_get(&self, url: &str) -> Result<WebHttpResponse, DenError> {
        self.web().http_get(url).await
    }

    async fn authorize_search(&self, context: &DenToolInvocationContext) -> Result<(), DenError> {
        self.web().authorize_search(context).await
    }

    async fn preferred_hosts(&self, bear_id: Uuid) -> Result<Vec<String>, DenError> {
        self.web().preferred_hosts(bear_id).await
    }

    fn normalize_host(&self, url: &str) -> Option<String> {
        self.web().normalize_host(url)
    }

    fn default_search_max_results(&self) -> usize {
        self.web().default_search_max_results()
    }

    async fn provider_search(&self, query: &str, max_results: usize) -> Result<Value, DenError> {
        self.web().provider_search(query, max_results).await
    }
}

impl RoleMemoryStore for DenToolContext<'_> {
    async fn read(
        &self,
        context: &DenToolInvocationContext,
        role: RuntimeContextLabel,
        path: &str,
    ) -> Result<Value, DenError> {
        self.memory().read(context, role, path).await
    }

    async fn browse(
        &self,
        context: &DenToolInvocationContext,
        role: RuntimeContextLabel,
    ) -> Result<Value, DenError> {
        self.memory().browse(context, role).await
    }

    async fn search(
        &self,
        context: &DenToolInvocationContext,
        role: RuntimeContextLabel,
        query: &str,
        limit: i64,
    ) -> Result<Value, DenError> {
        self.memory().search(context, role, query, limit).await
    }

    async fn status_base(
        &self,
        context: &DenToolInvocationContext,
        role: RuntimeContextLabel,
    ) -> Result<
        (
            Value,
            den_core::tools::prompt_memory::PromptMemoryVisibility,
        ),
        DenError,
    > {
        self.memory().status_base(context, role).await
    }

    async fn write_entry(
        &self,
        context: &DenToolInvocationContext,
        role: RuntimeContextLabel,
        entry: RoleMemoryEntryWrite,
    ) -> Result<Value, DenError> {
        self.memory().write_entry(context, role, entry).await
    }
}

impl PromptMemoryStore for DenToolContext<'_> {
    async fn visibility(
        &self,
        context: &DenToolInvocationContext,
        role: RuntimeContextLabel,
    ) -> Result<den_core::tools::prompt_memory::PromptMemoryVisibility, DenError> {
        self.prompt().visibility(context, role).await
    }

    async fn list_blocks(
        &self,
        bear_id: Uuid,
        profile_slug: &str,
    ) -> Result<Vec<PromptMemoryBlock>, DenError> {
        self.prompt().list_blocks(bear_id, profile_slug).await
    }

    async fn upsert_block(&self, write: &PromptMemoryBlockWrite) -> Result<(), DenError> {
        self.prompt().upsert_block(write).await
    }

    async fn patch_block(
        &self,
        bear_id: Uuid,
        profile: RuntimeContextLabel,
        block_id: &str,
        patch: &PromptMemoryBlockPatch,
    ) -> Result<(), DenError> {
        self.prompt()
            .patch_block(bear_id, profile, block_id, patch)
            .await
    }

    async fn archive_conflicting(&self, write: &PromptMemoryBlockWrite) -> Result<u64, DenError> {
        self.prompt().archive_conflicting(write).await
    }

    async fn archive_superseded_by(
        &self,
        bear_id: Uuid,
        profile_slug: &str,
        supersedes_block_id: &str,
    ) -> Result<u64, DenError> {
        self.prompt()
            .archive_superseded_by(bear_id, profile_slug, supersedes_block_id)
            .await
    }
}

impl MemoryReviewStore for DenToolContext<'_> {
    async fn find_observation(
        &self,
        bear_id: Uuid,
        observation_id: &str,
    ) -> Result<Option<ObservationRecord>, DenError> {
        self.review()
            .find_observation(bear_id, observation_id)
            .await
    }

    async fn record_observation(
        &self,
        request: ObservationWriteRequest,
    ) -> Result<ObservationRecord, DenError> {
        self.review().record_observation(request).await
    }

    async fn list_proposals(
        &self,
        bear_id: Uuid,
        status: Option<MemoryProposalStatus>,
        limit: i64,
    ) -> Result<Value, DenError> {
        self.review().list_proposals(bear_id, status, limit).await
    }

    async fn get_proposal(
        &self,
        bear_id: Uuid,
        proposal_id: Uuid,
    ) -> Result<Option<Value>, DenError> {
        self.review().get_proposal(bear_id, proposal_id).await
    }

    async fn resolve_proposal(&self, request: ResolveProposalRequest) -> Result<Value, DenError> {
        self.review().resolve_proposal(request).await
    }

    async fn request_review(&self, request: RequestReviewRequest) -> Result<Value, DenError> {
        self.review().request_review(request).await
    }

    async fn mark_memory_lifecycle(
        &self,
        request: MarkMemoryLifecycleRequest,
    ) -> Result<Value, DenError> {
        self.review().mark_memory_lifecycle(request).await
    }
}

impl EntityOps for DenToolContext<'_> {
    async fn browse_entities(
        &self,
        context: &DenToolInvocationContext,
        role: RuntimeContextLabel,
        arguments: Value,
    ) -> Result<Value, DenError> {
        self.entity().browse(context, role, arguments).await
    }

    async fn resolve_entity(
        &self,
        context: &DenToolInvocationContext,
        role: RuntimeContextLabel,
        arguments: Value,
    ) -> Result<Value, DenError> {
        self.entity().resolve(context, role, arguments).await
    }

    async fn link_memory_entity(
        &self,
        context: &DenToolInvocationContext,
        role: RuntimeContextLabel,
        arguments: Value,
    ) -> Result<Value, DenError> {
        self.entity().link_memory(context, role, arguments).await
    }
}

impl PlanModeOps for DenToolContext<'_> {
    async fn enter(
        &self,
        context: &DenToolInvocationContext,
        client_session_id: &str,
        reason: String,
        previous_permission_mode: Option<String>,
    ) -> Result<PlanModeView, DenError> {
        self.plan_mode()
            .enter(context, client_session_id, reason, previous_permission_mode)
            .await
    }

    async fn status(
        &self,
        context: &DenToolInvocationContext,
        client_session_id: &str,
    ) -> Result<PlanModeStatusView, DenError> {
        self.plan_mode().status(context, client_session_id).await
    }

    async fn record_approval(
        &self,
        context: &DenToolInvocationContext,
        client_session_id: &str,
        plan_mode_id: Option<Uuid>,
    ) -> Result<PlanModeView, DenError> {
        self.plan_mode()
            .record_approval(context, client_session_id, plan_mode_id)
            .await
    }

    async fn exit(
        &self,
        context: &DenToolInvocationContext,
        client_session_id: &str,
        plan_mode_id: Option<Uuid>,
        title: &str,
        body: &str,
    ) -> Result<PlanModeExitView, DenError> {
        self.plan_mode()
            .exit(context, client_session_id, plan_mode_id, title, body)
            .await
    }

    async fn cancel(
        &self,
        context: &DenToolInvocationContext,
        client_session_id: &str,
        plan_mode_id: Option<Uuid>,
    ) -> Result<PlanModeView, DenError> {
        self.plan_mode()
            .cancel(context, client_session_id, plan_mode_id)
            .await
    }
}

impl SourceAuthorizer for DenToolContext<'_> {
    async fn authorize_source(
        &self,
        context: &DenToolInvocationContext,
        origin: den_core::TurnExecutionOrigin,
    ) -> Result<(), DenError> {
        origin.require_ordinary_session()?;
        if context
            .client_session_id
            .as_deref()
            .is_some_and(|id| id != context.session_id)
        {
            return Err(DenError::Authorization(
                "Den tool client session disagrees with its source session".into(),
            ));
        }
        let source = den_runtime::agent_loop::require_ordinary_session_source(
            self.pool,
            den_runtime::agent_loop::OrdinarySessionSource {
                bear_id: context.bear_id,
                user_id: Some(context.user_id),
                origin,
                profile: context.profile.ok_or_else(|| {
                    DenError::Authorization("ordinary tool source has no profile".into())
                })?,
                conversation_id: &context.conversation_id,
                client_session_id: &context.session_id,
                work_run_id: context.work_run_id,
            },
        )
        .await?;
        if source.binding_id(context.bear_id.into()) != context.binding_id {
            return Err(DenError::Authorization(
                "Den tool binding does not match its canonical source".into(),
            ));
        }
        Ok(())
    }
}

impl BearDirectory for DenToolContext<'_> {
    async fn user_may_use_bear(&self, user_id: i32, bear_id: Uuid) -> Result<bool, DenError> {
        self.directory().user_may_use_bear(user_id, bear_id).await
    }

    async fn bear_self(&self, bear_id: Uuid) -> Result<Option<BearRecord>, DenError> {
        self.directory().bear_self(bear_id).await
    }

    async fn member_count(&self, bear_id: Uuid) -> Result<i64, DenError> {
        self.directory().member_count(bear_id).await
    }

    async fn members(&self, bear_id: Uuid) -> Result<Vec<BearMemberRecord>, DenError> {
        self.directory().members(bear_id).await
    }

    async fn current_user(&self, user_id: i32) -> Result<CurrentUser, DenError> {
        self.directory().current_user(user_id).await
    }
}

impl EnvironmentOps for DenToolContext<'_> {
    fn uses_native_runtime(&self) -> bool {
        self.environment().uses_native_runtime()
    }

    async fn memory_status_value(
        &self,
        context: &DenToolInvocationContext,
        role: RuntimeContextLabel,
    ) -> Result<Value, DenError> {
        self.environment().memory_status_value(context, role).await
    }

    async fn memory_visibility(
        &self,
        context: &DenToolInvocationContext,
        role: RuntimeContextLabel,
    ) -> Result<den_core::tools::prompt_memory::PromptMemoryVisibility, DenError> {
        self.environment().memory_visibility(context, role).await
    }

    async fn session_entities(
        &self,
        context: &DenToolInvocationContext,
        role: RuntimeContextLabel,
    ) -> Result<Value, DenError> {
        self.environment().session_entities(context, role).await
    }

    async fn fetch_adapter_environment(
        &self,
        context: &DenToolInvocationContext,
    ) -> Result<Option<Value>, DenError> {
        self.environment().fetch_adapter_environment(context).await
    }
}

impl ConversationTitleOps for DenToolContext<'_> {
    async fn set_title(
        &self,
        bear_id: Uuid,
        conversation_id: &str,
        title: &str,
    ) -> Result<u64, DenError> {
        self.conversation()
            .set_title(bear_id, conversation_id, title)
            .await
    }
}

impl ToolContext for DenToolContext<'_> {}
