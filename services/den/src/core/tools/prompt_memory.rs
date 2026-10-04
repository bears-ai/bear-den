//! `den`-side wiring for the prompt-memory tools.
//!
//! The orchestration (descriptor authorization, validation, result shaping) lives in
//! `den-tools`; here we provide the Postgres-backed [`PromptMemoryStore`],
//! wired into the dispatcher via `DenToolContext`.

use sqlx::PgPool;
use uuid::Uuid;

use den_core::tools::{
    context::DenToolInvocationContext,
    prompt_memory::{
        PromptMemoryBlock, PromptMemoryBlockPatch, PromptMemoryBlockWrite, PromptMemoryStore,
        PromptMemoryVisibility,
    },
};
use den_core::{ids::BearId, RuntimeContextLabel};

use crate::errors::DenError;
use den_service::{
    bears::hats::memory_binding,
    prompt_memory_block_store::{
        archive_conflicting_prompt_memory_blocks, archive_prompt_memory_blocks_superseded_by,
        list_prompt_memory_blocks_for_bear_profile, patch_prompt_memory_block,
        upsert_prompt_memory_block,
    },
};

/// Postgres-backed [`PromptMemoryStore`] over a pool reference.
pub(crate) struct DenPromptMemoryStore<'a> {
    pool: &'a PgPool,
}

impl<'a> DenPromptMemoryStore<'a> {
    pub(crate) fn new(pool: &'a PgPool) -> Self {
        Self { pool }
    }
}

impl PromptMemoryStore for DenPromptMemoryStore<'_> {
    async fn visibility(
        &self,
        context: &DenToolInvocationContext,
        _role: RuntimeContextLabel,
    ) -> Result<PromptMemoryVisibility, DenError> {
        memory_binding::for_external_conversation(
            self.pool,
            BearId::new(context.bear_id),
            &context.conversation_id,
        )
        .await?;
        Ok(PromptMemoryVisibility::BoundSession)
    }

    async fn list_blocks(
        &self,
        bear_id: Uuid,
        profile_slug: &str,
    ) -> Result<Vec<PromptMemoryBlock>, DenError> {
        list_prompt_memory_blocks_for_bear_profile(self.pool, bear_id, profile_slug).await
    }

    async fn upsert_block(&self, write: &PromptMemoryBlockWrite) -> Result<(), DenError> {
        upsert_prompt_memory_block(self.pool, write).await
    }

    async fn patch_block(
        &self,
        bear_id: Uuid,
        profile: RuntimeContextLabel,
        block_id: &str,
        patch: &PromptMemoryBlockPatch,
    ) -> Result<(), DenError> {
        patch_prompt_memory_block(self.pool, bear_id, profile, block_id, patch).await
    }

    async fn archive_conflicting(&self, write: &PromptMemoryBlockWrite) -> Result<u64, DenError> {
        archive_conflicting_prompt_memory_blocks(self.pool, write).await
    }

    async fn archive_superseded_by(
        &self,
        bear_id: Uuid,
        profile_slug: &str,
        supersedes_block_id: &str,
    ) -> Result<u64, DenError> {
        archive_prompt_memory_blocks_superseded_by(
            self.pool,
            bear_id,
            profile_slug,
            supersedes_block_id,
        )
        .await
    }
}
