//! Prompt-memory tools (`upsert`, `list`, `patch`) — orchestration layer.
//!
//! Runtime-agnostic: depends only on the [`PromptMemoryStore`] capability seam,
//! the shared validators, and compatibility profile metadata. Dispatch authorizes
//! descriptor audiences and canonical sources before calling these helpers;
//! visibility and session checks constrain the resource, not the profile.
//! The `den` crate provides
//! the concrete store and thin `CustomError`-mapping wrappers.

pub mod store;
pub mod types;

pub use store::PromptMemoryStore;
pub use types::{
    PromptMemoryBlock, PromptMemoryBlockPatch, PromptMemoryBlockScope, PromptMemoryBlockState,
    PromptMemoryBlockType, PromptMemoryBlockWrite, PromptMemoryVisibility,
};

use crate::{DenError, RuntimeContextLabel};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::tools::{
    context::DenToolInvocationContext,
    validation::{validate_bounded_text, validate_optional_object},
};

/// Validate that a prompt-memory scope carries the qualifier it requires.
pub fn validate_prompt_memory_scope(
    scope: PromptMemoryBlockScope,
    work_surface: Option<&str>,
    session_id: Option<&str>,
) -> Result<(), DenError> {
    match scope {
        PromptMemoryBlockScope::WorkSurface if work_surface.is_none() => {
            Err(DenError::ValidationError(
                "prompt memory scope `work_surface` requires `work_surface`".to_string(),
            ))
        }
        PromptMemoryBlockScope::Session if session_id.is_none() => Err(DenError::ValidationError(
            "prompt memory scope `session` requires `session_id`".to_string(),
        )),
        _ => Ok(()),
    }
}

#[derive(Debug, Deserialize)]
pub struct PromptMemoryUpsertArguments {
    pub block_id: String,
    pub scope: PromptMemoryBlockScope,
    pub block_type: PromptMemoryBlockType,
    #[serde(default = "default_prompt_memory_state")]
    pub state: PromptMemoryBlockState,
    #[serde(default)]
    pub work_surface: Option<String>,
    #[serde(default)]
    pub session_id: Option<String>,
    pub title: String,
    pub body: String,
    #[serde(default)]
    pub priority: Option<i32>,
    #[serde(default)]
    pub supersedes_block_id: Option<String>,
    #[serde(default)]
    pub metadata: Option<Value>,
}

#[derive(Debug, Deserialize)]
pub struct PromptMemoryPatchArguments {
    pub block_id: String,
    #[serde(default = "default_prompt_memory_state")]
    pub state: PromptMemoryBlockState,
    pub title: String,
    pub body: String,
    #[serde(default)]
    pub priority: Option<i32>,
    #[serde(default)]
    pub supersedes_block_id: Option<String>,
    #[serde(default)]
    pub metadata: Option<Value>,
}

#[derive(Debug, Deserialize)]
pub struct PromptMemoryListArguments {
    #[serde(default)]
    pub include_archived: bool,
    #[serde(default)]
    pub scope: Option<PromptMemoryBlockScope>,
    #[serde(default)]
    pub block_type: Option<PromptMemoryBlockType>,
    #[serde(default)]
    pub work_surface: Option<String>,
    #[serde(default)]
    pub session_id: Option<String>,
}

pub fn default_prompt_memory_state() -> PromptMemoryBlockState {
    PromptMemoryBlockState::Active
}

fn empty_json_object() -> Value {
    json!({})
}

pub async fn prompt_memory_upsert(
    store: &impl PromptMemoryStore,
    context: &DenToolInvocationContext,
    role: RuntimeContextLabel,
    arguments: Value,
) -> Result<Value, DenError> {
    let args: PromptMemoryUpsertArguments = serde_json::from_value(arguments)?;
    let visibility = store.visibility(context, role).await?;
    if visibility == PromptMemoryVisibility::SharedOnly {
        return Err(DenError::Authorization(
            "prompt memory scope is unresolved".into(),
        ));
    }
    if visibility == PromptMemoryVisibility::BoundSession {
        if args.scope != PromptMemoryBlockScope::Session
            || args.session_id.as_deref() != Some(context.session_id.as_str())
        {
            return Err(DenError::Authorization(
                "bound prompt memory may only write its own session blocks".into(),
            ));
        }
        let existing = store.list_blocks(context.bear_id, role.as_str()).await?;
        for id in [
            Some(args.block_id.as_str()),
            args.supersedes_block_id.as_deref(),
        ]
        .into_iter()
        .flatten()
        {
            if let Some(block) = existing.iter().find(|block| block.id == id) {
                if block.scope != PromptMemoryBlockScope::Session
                    || block.session_id.as_deref() != Some(context.session_id.as_str())
                {
                    return Err(DenError::Authorization(
                        "bound prompt memory cannot replace another scope's block".into(),
                    ));
                }
            }
        }
    }
    let title = validate_bounded_text("title", &args.title, 1, 200)?;
    let body = validate_bounded_text("body", &args.body, 1, 50_000)?;
    let block_id = validate_bounded_text("block_id", &args.block_id, 1, 200)?;
    validate_prompt_memory_scope(
        args.scope,
        args.work_surface.as_deref(),
        args.session_id.as_deref(),
    )?;
    let priority = args.priority.unwrap_or(0).clamp(-1000, 1000);
    validate_optional_object("metadata", &args.metadata)?;
    let write = PromptMemoryBlockWrite {
        block_id,
        bear_id: Some(context.bear_id),
        profile_slug: Some(role.as_str().to_string()),
        scope: args.scope,
        block_type: args.block_type,
        state: args.state,
        work_surface: args.work_surface,
        session_id: args.session_id,
        title,
        body,
        priority,
        created_by_user_id: Some(context.user_id),
        supersedes_block_id: args.supersedes_block_id,
        metadata: args.metadata.unwrap_or_else(empty_json_object),
    };
    // The immutable Bear/scope guard on upsert must run before any archival side effects.
    store.upsert_block(&write).await?;
    let conflicting_archived = if write.state == PromptMemoryBlockState::Active {
        store.archive_conflicting(&write).await?
    } else {
        0
    };
    let superseded_archived =
        if let Some(supersedes_block_id) = write.supersedes_block_id.as_deref() {
            store
                .archive_superseded_by(context.bear_id, role.as_str(), supersedes_block_id)
                .await?
        } else {
            0
        };
    Ok(json!({
        "status": "ok",
        "block_id": write.block_id,
        "state": write.state,
        "title": write.title,
        "priority": write.priority,
        "metadata": write.metadata,
        "conflicting_archived": conflicting_archived,
        "superseded_archived": superseded_archived,
        "source": "prompt_memory_blocks"
    }))
}

pub async fn prompt_memory_list(
    store: &impl PromptMemoryStore,
    context: &DenToolInvocationContext,
    role: RuntimeContextLabel,
    arguments: Value,
) -> Result<Value, DenError> {
    let args: PromptMemoryListArguments = serde_json::from_value(arguments)?;
    let visibility = store.visibility(context, role).await?;
    let mut blocks = store.list_blocks(context.bear_id, role.as_str()).await?;
    blocks.retain(|block| visibility.allows(block, &context.session_id));
    if !args.include_archived {
        blocks.retain(|block| block.state != PromptMemoryBlockState::Archived);
    }
    if let Some(scope) = args.scope {
        blocks.retain(|block| block.scope == scope);
    }
    if let Some(block_type) = args.block_type {
        blocks.retain(|block| block.block_type == block_type);
    }
    if let Some(work_surface) = args.work_surface.as_deref() {
        let normalized = work_surface.trim();
        blocks.retain(|block| block.work_surface.as_deref() == Some(normalized));
    }
    if let Some(session_id) = args.session_id.as_deref() {
        let normalized = session_id.trim();
        blocks.retain(|block| block.session_id.as_deref() == Some(normalized));
    }
    Ok(json!({
        "status": "ok",
        "source": "prompt_memory_blocks",
        "count": blocks.len(),
        "filters": {
            "include_archived": args.include_archived,
            "scope": args.scope,
            "block_type": args.block_type,
            "work_surface": args.work_surface,
            "session_id": args.session_id,
        },
        "blocks": blocks,
    }))
}

pub async fn prompt_memory_patch(
    store: &impl PromptMemoryStore,
    context: &DenToolInvocationContext,
    role: RuntimeContextLabel,
    arguments: Value,
) -> Result<Value, DenError> {
    let args: PromptMemoryPatchArguments = serde_json::from_value(arguments)?;
    let visibility = store.visibility(context, role).await?;
    if visibility != PromptMemoryVisibility::Legacy {
        let blocks = store.list_blocks(context.bear_id, role.as_str()).await?;
        if visibility != PromptMemoryVisibility::BoundSession
            || !blocks.iter().any(|block| {
                block.id == args.block_id
                    && block.scope == PromptMemoryBlockScope::Session
                    && block.visible_in_bound_session(&context.session_id)
            })
        {
            return Err(DenError::Authorization(
                "bound prompt memory can only patch its own session blocks".into(),
            ));
        }
    }
    let title = validate_bounded_text("title", &args.title, 1, 200)?;
    let body = validate_bounded_text("body", &args.body, 1, 50_000)?;
    let block_id = validate_bounded_text("block_id", &args.block_id, 1, 200)?;
    let priority = args.priority.unwrap_or(0).clamp(-1000, 1000);
    validate_optional_object("metadata", &args.metadata)?;
    let patch = PromptMemoryBlockPatch {
        state: args.state,
        title,
        body,
        priority,
        supersedes_block_id: args.supersedes_block_id,
        metadata: args.metadata.unwrap_or_else(empty_json_object),
    };
    store
        .patch_block(context.bear_id, role, &block_id, &patch)
        .await?;
    Ok(json!({
        "status": "ok",
        "block_id": block_id,
        "state": patch.state,
        "title": patch.title,
        "priority": patch.priority,
        "metadata": patch.metadata,
        "source": "prompt_memory_blocks"
    }))
}
