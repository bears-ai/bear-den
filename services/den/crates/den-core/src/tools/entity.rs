use serde::Deserialize;
use serde_json::Value;

use crate::tools::context::DenToolInvocationContext;
use crate::{DenError, RuntimeContextLabel};

#[derive(Debug, Deserialize)]
pub struct EntityBrowseArguments {
    #[serde(default)]
    pub entity_type: Option<String>,
    #[serde(default)]
    pub limit: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub struct EntityResolveArguments {
    pub entity_id: String,
    #[serde(default)]
    pub include_relations: Option<bool>,
    #[serde(default)]
    pub include_handles: Option<bool>,
}

#[derive(Debug, Deserialize)]
pub struct EntityLinkMemoryArguments {
    pub memory_id: String,
    pub entity_id: String,
    pub relation: String,
    #[serde(default)]
    pub qualifiers: Option<Value>,
    #[serde(default)]
    pub confidence: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct EntityMergeArguments {
    pub survivor_entity_id: String,
    pub loser_entity_id: String,
}

#[derive(Debug, Deserialize)]
pub struct EntitySplitArguments {
    pub new_entity_type: String,
    #[serde(default)]
    pub display_name: Option<String>,
    #[serde(default)]
    pub handle_ids_to_move: Vec<String>,
    #[serde(default)]
    pub resolution: Option<String>,
    #[serde(default)]
    pub trust: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct EntityWriteAccessRuleArguments {
    pub memory_id: String,
    pub entity_id: String,
    pub relation: String,
    #[serde(default)]
    pub qualifiers: Option<Value>,
    #[serde(default)]
    pub confidence: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct EntityWriteAnchorArguments {
    pub entity_id: String,
    pub kind: String,
    pub title: String,
    pub body: String,
    #[serde(default)]
    pub salience: Option<String>,
    #[serde(default)]
    pub supersedes_memory_id: Option<String>,
}

#[allow(async_fn_in_trait)]
pub trait EntityOps: Send + Sync {
    async fn browse_entities(
        &self,
        context: &DenToolInvocationContext,
        role: RuntimeContextLabel,
        arguments: Value,
    ) -> Result<Value, DenError>;

    async fn resolve_entity(
        &self,
        context: &DenToolInvocationContext,
        role: RuntimeContextLabel,
        arguments: Value,
    ) -> Result<Value, DenError>;

    async fn link_memory_entity(
        &self,
        context: &DenToolInvocationContext,
        role: RuntimeContextLabel,
        arguments: Value,
    ) -> Result<Value, DenError>;
}

pub async fn entity_browse(
    ops: &impl EntityOps,
    context: &DenToolInvocationContext,
    role: RuntimeContextLabel,
    arguments: Value,
) -> Result<Value, DenError> {
    ops.browse_entities(context, role, arguments).await
}

pub async fn entity_resolve(
    ops: &impl EntityOps,
    context: &DenToolInvocationContext,
    role: RuntimeContextLabel,
    arguments: Value,
) -> Result<Value, DenError> {
    ops.resolve_entity(context, role, arguments).await
}

pub async fn entity_link_memory(
    ops: &impl EntityOps,
    context: &DenToolInvocationContext,
    role: RuntimeContextLabel,
    arguments: Value,
) -> Result<Value, DenError> {
    ops.link_memory_entity(context, role, arguments).await
}

pub async fn entity_merge(
    _ops: &impl EntityOps,
    _context: &DenToolInvocationContext,
    _role: RuntimeContextLabel,
    _arguments: Value,
) -> Result<Value, DenError> {
    Err(DenError::NotFound(
        "entity_merge model helper is retired".to_string(),
    ))
}

pub async fn entity_split(
    _ops: &impl EntityOps,
    _context: &DenToolInvocationContext,
    _role: RuntimeContextLabel,
    _arguments: Value,
) -> Result<Value, DenError> {
    Err(DenError::NotFound(
        "entity_split model helper is retired".to_string(),
    ))
}
pub async fn entity_write_access_rule(
    _ops: &impl EntityOps,
    _context: &DenToolInvocationContext,
    _role: RuntimeContextLabel,
    _arguments: Value,
) -> Result<Value, DenError> {
    Err(DenError::NotFound(
        "entity_write_access_rule model helper is retired".to_string(),
    ))
}

pub async fn entity_write_anchor(
    _ops: &impl EntityOps,
    _context: &DenToolInvocationContext,
    _role: RuntimeContextLabel,
    _arguments: Value,
) -> Result<Value, DenError> {
    Err(DenError::NotFound(
        "entity_write_anchor model helper is retired".to_string(),
    ))
}
