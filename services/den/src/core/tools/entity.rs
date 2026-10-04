use den_core::tools::context::DenToolInvocationContext;
use den_core::tools::entity::{
    EntityBrowseArguments, EntityLinkMemoryArguments, EntityResolveArguments,
};
use den_memory::{
    self as memory_store, descriptors, EntityHandleRow, EntityRow, RelationClass, RelationRow,
};
use den_service::bears::RuntimeContextLabel;
use serde_json::{json, Value};

use crate::core::tools::context::DenToolContext;
use crate::errors::DenError;

pub(crate) struct DenEntityOps<'a> {
    ctx: &'a DenToolContext<'a>,
}

impl<'a> DenEntityOps<'a> {
    pub(crate) fn new(ctx: &'a DenToolContext<'a>) -> Self {
        Self { ctx }
    }

    pub(crate) async fn browse(
        &self,
        context: &DenToolInvocationContext,
        role: RuntimeContextLabel,
        arguments: Value,
    ) -> Result<Value, DenError> {
        let args: EntityBrowseArguments = serde_json::from_value(arguments)?;
        let limit = args.limit.unwrap_or(25).clamp(1, 100);
        let store = self.ctx.stores.store_for_bear(context.bear_id).await?;
        let entities: Vec<EntityRow> =
            memory_store::list_entities(&store, args.entity_type.as_deref(), limit).await?;
        Ok(json!({
            "ok": true,
            "bear_id": context.bear_id,
            "role": role.as_str(),
            "entity_type": args.entity_type,
            "entities": entities,
        }))
    }

    pub(crate) async fn resolve(
        &self,
        context: &DenToolInvocationContext,
        role: RuntimeContextLabel,
        arguments: Value,
    ) -> Result<Value, DenError> {
        let args: EntityResolveArguments = serde_json::from_value(arguments)?;
        let store = self.ctx.stores.store_for_bear(context.bear_id).await?;
        let Some(entity) = memory_store::resolve_live_entity(&store, &args.entity_id).await? else {
            return Ok(json!({
                "ok": false,
                "bear_id": context.bear_id,
                "role": role.as_str(),
                "entity_id": args.entity_id,
                "message": "entity not found",
            }));
        };
        let handles = if args.include_handles.unwrap_or(true) {
            let rows: Vec<EntityHandleRow> =
                memory_store::list_handles(&store, &entity.entity_id).await?;
            json!(rows)
        } else {
            Value::Null
        };
        let relations = if args.include_relations.unwrap_or(false) {
            let rows: Vec<RelationRow> =
                memory_store::list_relations_for_entity(&store, &entity.entity_id, 50).await?;
            json!(rows)
        } else {
            Value::Null
        };
        Ok(json!({
            "ok": true,
            "bear_id": context.bear_id,
            "role": role.as_str(),
            "entity": entity,
            "handles": handles,
            "relations": relations,
        }))
    }

    pub(crate) async fn link_memory(
        &self,
        context: &DenToolInvocationContext,
        role: RuntimeContextLabel,
        arguments: Value,
    ) -> Result<Value, DenError> {
        let args: EntityLinkMemoryArguments = serde_json::from_value(arguments)?;
        let descriptor = descriptors::relation(&args.relation).ok_or_else(|| {
            DenError::ValidationError(format!("unknown entity relation: {}", args.relation))
        })?;
        if descriptor.class != RelationClass::Descriptive {
            return Err(DenError::Authorization(
                "entity_link_memory can only write descriptive relations".to_string(),
            ));
        }
        let store = self.ctx.stores.store_for_bear(context.bear_id).await?;
        let qualifiers = args.qualifiers.unwrap_or_else(|| json!({}));
        let relation = memory_store::append_relation(
            &store,
            &args.memory_id,
            &args.entity_id,
            &args.relation,
            &qualifiers,
            role.as_str(),
            Some(context.binding_id.as_str()),
            args.confidence.as_deref(),
        )
        .await?;
        Ok(json!({
            "ok": true,
            "bear_id": context.bear_id,
            "role": role.as_str(),
            "relation": relation,
        }))
    }
}
