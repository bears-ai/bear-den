//! Memory read tools (`memory_read`, `memory_browse`, `memory_search`,
//! `memory_status`) — orchestration layer.
//!
//! Runtime-agnostic: depends only on [`RoleMemoryStore`] (and, for the status
//! diagnostic, [`PromptMemoryStore`]) plus `den-core` types. The `den` crate
//! provides the concrete SQLite store and thin
//! `CustomError`-mapping wrappers.

pub mod store;

pub use store::RoleMemoryStore;

use crate::{BearProfile, DenError};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::tools::{
    context::DenToolInvocationContext,
    prompt_memory::{PromptMemoryBlock, PromptMemoryBlockState, PromptMemoryStore},
    support::{
        clean_limited_strings, clean_optional, validate_bounded_text,
        validate_memory_write_entry_semantics, validate_optional_object,
    },
};

#[derive(Debug, Deserialize)]
pub struct MemoryReadArguments {
    pub path: String,
}

#[derive(Debug, Deserialize)]
pub struct MemorySearchArguments {
    pub query: String,
    #[serde(default)]
    pub limit: Option<usize>,
}

#[derive(Debug, Deserialize)]
pub struct MemoryWriteEntryArguments {
    pub kind: String,
    pub title: String,
    pub body: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub refs: Option<Value>,
    #[serde(default)]
    pub lifecycle: Option<Value>,
    #[serde(default)]
    pub source: Option<Value>,
    #[serde(default)]
    pub content_class: Option<String>,
    #[serde(default)]
    pub domain: Option<String>,
    #[serde(default)]
    pub semantic_confirmation_token: Option<String>,
}

/// A fully-prepared role-memory entry handed to [`RoleMemoryStore::write_entry`].
///
/// Carries the validated fields the native SQLite write path needs; the executor
/// populates it after validation + source merging.
#[derive(Debug, Clone)]
pub struct RoleMemoryEntryWrite {
    pub kind: String,
    pub title: String,
    pub body: String,
    pub tags: Vec<String>,
    pub refs: Option<Value>,
    pub lifecycle: Option<Value>,
    pub source: Option<Value>,
    pub author: Option<String>,
    pub conversation_id: Option<String>,
    pub session_id: Option<String>,
    pub client_session_id: Option<String>,
    pub conversation_selection: Option<String>,
    pub runtime_target: Option<String>,
    pub binding_id: Option<String>,
    pub profile: Option<String>,
    pub request_id: Option<String>,
}

/// Merge a caller-supplied `source` object with trusted human + session identity
/// drawn from the invocation context (author identity is passed as primitives).
pub fn merge_memory_entry_source_with_human(
    source: Option<Value>,
    context: &DenToolInvocationContext,
    author_username: Option<String>,
    author_display_name: Option<String>,
) -> Option<Value> {
    let mut source_obj = source
        .and_then(|value| value.as_object().cloned())
        .unwrap_or_default();
    let display_name = author_display_name.map(Value::from).unwrap_or(Value::Null);
    source_obj.insert(
        "human".to_string(),
        json!({
            "user_id": context.user_id,
            "username": author_username.or_else(|| context.username.clone()),
            "display_name": display_name,
            "membership_role": context.membership_role,
            "authenticated_by": "client_token"
        }),
    );
    source_obj.insert(
        "session".to_string(),
        json!({
            "conversation_id": clean_optional(&context.conversation_id),
            "session_id": clean_optional(&context.session_id),
            "client_session_id": context.client_session_id,
            "conversation_selection": context.conversation_selection,
            "runtime_target": context.runtime_target,
            "request_id": context.request_id,
        }),
    );
    Some(Value::Object(source_obj))
}

/// The client session id, when the invocation arrived over a stateful client channel.
pub fn source_client_session_id(context: &DenToolInvocationContext) -> Option<String> {
    let has_stateful_client = [
        context.channel.family.as_deref(),
        context.channel.client.as_deref(),
        context.channel.protocol.as_deref(),
    ]
    .into_iter()
    .flatten()
    .any(|value| matches!(value.to_ascii_lowercase().as_str(), "armature" | "bearwire"));
    if has_stateful_client {
        clean_optional(&context.session_id)
    } else {
        None
    }
}

pub async fn write_memory_entry(
    memory: &impl RoleMemoryStore,
    context: &DenToolInvocationContext,
    role: BearProfile,
    arguments: Value,
    author_username: Option<String>,
    author_display_name: Option<String>,
) -> Result<Value, DenError> {
    if role != BearProfile::Pair {
        return Err(DenError::Authorization(
            "den.memory.write_entry is currently available only to the pair role".to_string(),
        ));
    }
    let args: MemoryWriteEntryArguments = serde_json::from_value(arguments)?;
    let kind = validate_memory_write_entry_semantics(&args, context)?;
    let title = validate_bounded_text("title", &args.title, 1, 200)?;
    let body = validate_bounded_text("body", &args.body, 1, 50_000)?;
    let tags = clean_limited_strings(args.tags, 20, 80);
    validate_optional_object("refs", &args.refs)?;
    validate_optional_object("lifecycle", &args.lifecycle)?;
    validate_optional_object("source", &args.source)?;
    let source = merge_memory_entry_source_with_human(
        args.source,
        context,
        author_username.clone(),
        author_display_name,
    );
    let author = author_username.or_else(|| context.username.clone());
    let entry = RoleMemoryEntryWrite {
        kind,
        title,
        body,
        tags,
        refs: args.refs,
        lifecycle: args.lifecycle,
        source,
        author,
        conversation_id: clean_optional(&context.conversation_id),
        session_id: source_client_session_id(context)
            .or_else(|| clean_optional(&context.session_id)),
        client_session_id: context
            .client_session_id
            .clone()
            .or_else(|| source_client_session_id(context)),
        conversation_selection: context.conversation_selection.clone(),
        runtime_target: context.runtime_target.clone(),
        binding_id: Some(context.binding_id.clone()),
        profile: context.profile.map(|role| role.as_str().to_string()),
        request_id: context.request_id.clone(),
    };
    memory.write_entry(context, role, entry).await
}

/// Summarize active prompt-memory blocks for the `memory_status` diagnostic.
pub fn prompt_memory_diagnostic_summary(blocks: &[PromptMemoryBlock]) -> Value {
    let active_blocks = blocks
        .iter()
        .filter(|block| block.state == PromptMemoryBlockState::Active)
        .collect::<Vec<_>>();
    let mut active_by_scope = serde_json::Map::new();
    let mut active_by_type = serde_json::Map::new();
    for block in &active_blocks {
        let scope_key = serde_json::to_string(&block.scope)
            .unwrap_or_else(|_| "\"unknown\"".to_string())
            .trim_matches('"')
            .to_string();
        let scope_count = active_by_scope.entry(scope_key).or_insert_with(|| json!(0));
        *scope_count = json!(scope_count.as_i64().unwrap_or(0) + 1);

        let type_key = serde_json::to_string(&block.block_type)
            .unwrap_or_else(|_| "\"unknown\"".to_string())
            .trim_matches('"')
            .to_string();
        let type_count = active_by_type.entry(type_key).or_insert_with(|| json!(0));
        *type_count = json!(type_count.as_i64().unwrap_or(0) + 1);
    }
    let active_blocks = active_blocks
        .into_iter()
        .map(|block| {
            json!({
                "block_id": block.id,
                "scope": block.scope,
                "block_type": block.block_type,
                "title": block.title,
                "work_surface": block.work_surface,
                "session_id": block.session_id,
                "priority": block.priority,
                "updated_at": Value::Null,
            })
        })
        .collect::<Vec<_>>();
    json!({
        "status": if active_blocks.is_empty() { "empty" } else { "ok" },
        "source": "prompt_memory_blocks",
        "active_count": active_blocks.len(),
        "active_by_scope": active_by_scope,
        "active_by_type": active_by_type,
        "active_blocks": active_blocks,
    })
}

pub async fn memory_status(
    memory: &impl RoleMemoryStore,
    prompt: &impl PromptMemoryStore,
    context: &DenToolInvocationContext,
    role: BearProfile,
) -> Result<Value, DenError> {
    let (mut base, visibility) = memory.status_base(context, role).await?;
    let blocks = prompt.list_blocks(context.bear_id, role.as_str()).await?;
    let blocks: Vec<_> = blocks
        .into_iter()
        .filter(|block| visibility.allows(block, &context.session_id))
        .collect();
    let diagnostic = prompt_memory_diagnostic_summary(&blocks);
    if let Some(obj) = base.as_object_mut() {
        obj.insert("prompt_memory_diagnostic".to_string(), diagnostic);
        obj.insert("bear_id".to_string(), json!(context.bear_id));
    }
    Ok(base)
}

pub async fn memory_browse(
    memory: &impl RoleMemoryStore,
    context: &DenToolInvocationContext,
    role: BearProfile,
) -> Result<Value, DenError> {
    memory.browse(context, role).await
}

pub async fn memory_read(
    memory: &impl RoleMemoryStore,
    context: &DenToolInvocationContext,
    role: BearProfile,
    arguments: Value,
) -> Result<Value, DenError> {
    let args: MemoryReadArguments = serde_json::from_value(arguments)?;
    let path = args.path.trim();
    if path.is_empty() {
        return Err(DenError::ValidationError(
            "path must not be empty".to_string(),
        ));
    }
    memory.read(context, role, path).await
}

pub async fn memory_search(
    memory: &impl RoleMemoryStore,
    context: &DenToolInvocationContext,
    role: BearProfile,
    arguments: Value,
) -> Result<Value, DenError> {
    let args: MemorySearchArguments = serde_json::from_value(arguments)?;
    let query = args.query.trim();
    if query.is_empty() {
        return Err(DenError::ValidationError(
            "query must not be empty".to_string(),
        ));
    }
    let limit = args.limit.map(|n| n.clamp(1, 50) as i64).unwrap_or(10);
    memory.search(context, role, query, limit).await
}

#[cfg(test)]
mod tests;
