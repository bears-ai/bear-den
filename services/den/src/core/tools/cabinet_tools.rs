//! Model-facing Cabinet tools: search/read/create/update over the Phase 1
//! facade in `den_service::cabinet`.
//!
//! Native execution carries a verified origin and checks descriptor audiences
//! again at the effect. The facade enforces the Bear-level `cabinet_enabled`
//! gate and Cabinet contract rules server-side.

use den_cabinet::{
    ActorScope, CabinetItemRef, CabinetSourceRef, CabinetVersionRef, CreateItemRequest,
    HistoryRequest, ItemKind, Lifecycle, LinkSourceRequest, NewSourceLink, ReadRequest,
    SearchFilters, SearchRequest, SourceKind, SourceRole, UnlinkSourceRequest, UpdateItemRequest,
};
use den_core::ids::{BearId, ConversationId};
use den_core::tools::constants::{
    DEN_CABINET_CREATE, DEN_CABINET_HISTORY, DEN_CABINET_LIFECYCLE, DEN_CABINET_READ,
    DEN_CABINET_SEARCH, DEN_CABINET_SOURCE_LINK, DEN_CABINET_UPDATE,
};
use den_core::tools::{
    context::DenToolInvocationContext, descriptor::builtin_den_tool_descriptor_for_provider_name,
};
use den_core::{BearProfile, Governance, TurnExecutionOrigin};
use den_http::errors::CustomError;
use serde::Deserialize;
use serde_json::{json, Value};
use sqlx::PgPool;

#[cfg(test)]
#[path = "cabinet_tools/authority_tests.rs"]
mod authority_tests;

pub(crate) fn is_cabinet_tool(tool_name: &str) -> bool {
    matches!(
        tool_name,
        DEN_CABINET_SEARCH
            | DEN_CABINET_READ
            | DEN_CABINET_CREATE
            | DEN_CABINET_UPDATE
            | DEN_CABINET_HISTORY
            | DEN_CABINET_SOURCE_LINK
            | DEN_CABINET_LIFECYCLE
    )
}

fn cabinet_error(error: den_cabinet::CabinetError) -> CustomError {
    CustomError::from(den_core::DenError::from(error))
}

fn parse_arguments<T: for<'de> Deserialize<'de>>(arguments: Value) -> Result<T, CustomError> {
    serde_json::from_value(arguments)
        .map_err(|error| CustomError::ValidationError(error.to_string()))
}

fn parse_item_ref(value: &str) -> Result<CabinetItemRef, CustomError> {
    CabinetItemRef::parse(value).map_err(|error| CustomError::ValidationError(error.to_string()))
}

fn actor_scope(context: &DenToolInvocationContext, role: BearProfile) -> ActorScope {
    let mut scope = ActorScope::bear(BearId::new(context.bear_id), role);
    if !context.conversation_id.is_empty() {
        scope.conversation_id = Some(ConversationId(context.conversation_id.clone()));
    }
    scope.run_id = context.work_run_id.map(|run_id| run_id.to_string());
    scope
}

#[derive(Clone, Copy)]
pub(crate) struct CabinetToolAuthority {
    pub origin: TurnExecutionOrigin,
    pub governance: Governance,
}

fn require_write_authority(
    tool_name: &str,
    authority: CabinetToolAuthority,
) -> Result<(), CustomError> {
    let descriptor = builtin_den_tool_descriptor_for_provider_name(tool_name)
        .ok_or_else(|| CustomError::NotFound(format!("unknown Cabinet tool: {tool_name}")))?;
    if descriptor.allows_origin(authority.origin)
        && !matches!(
            authority.governance,
            Governance::Observational | Governance::Frozen
        )
    {
        Ok(())
    } else {
        Err(CustomError::Authorization(
            "Cabinet write is not available to this execution origin and governance".into(),
        ))
    }
}

pub(crate) async fn invoke_cabinet_tool(
    pool: &PgPool,
    tool_name: &str,
    arguments: Value,
    context: &DenToolInvocationContext,
    authority: CabinetToolAuthority,
) -> Result<Value, CustomError> {
    let descriptor = builtin_den_tool_descriptor_for_provider_name(tool_name)
        .ok_or_else(|| CustomError::NotFound(format!("unknown Cabinet tool: {tool_name}")))?;
    if !descriptor.allows_origin(authority.origin) {
        return Err(CustomError::Authorization(
            "Cabinet tool is not available to this execution origin".into(),
        ));
    }
    let role =
        den_core::EffectivePolicy::compile_for_origin(authority.origin, authority.governance)
            .trust_profile;
    match tool_name {
        DEN_CABINET_SEARCH => cabinet_search(pool, context, role, arguments).await,
        DEN_CABINET_READ => cabinet_read(pool, context, role, arguments).await,
        DEN_CABINET_CREATE => cabinet_create(pool, context, role, authority, arguments).await,
        DEN_CABINET_UPDATE => cabinet_update(pool, context, role, authority, arguments).await,
        DEN_CABINET_HISTORY => cabinet_history(pool, context, role, arguments).await,
        DEN_CABINET_SOURCE_LINK => {
            cabinet_source_link(pool, context, role, authority, arguments).await
        }
        DEN_CABINET_LIFECYCLE => cabinet_lifecycle(pool, context, role, authority, arguments).await,
        other => Err(CustomError::NotFound(format!(
            "unknown cabinet tool: {other}"
        ))),
    }
}

#[derive(Debug, Deserialize)]
struct CabinetSearchArguments {
    query: String,
    #[serde(default)]
    lifecycle: Option<Lifecycle>,
}

async fn cabinet_search(
    pool: &PgPool,
    context: &DenToolInvocationContext,
    role: BearProfile,
    arguments: Value,
) -> Result<Value, CustomError> {
    let args: CabinetSearchArguments = parse_arguments(arguments)?;
    let items = den_service::cabinet::search(
        pool,
        SearchRequest {
            scope: actor_scope(context, role),
            query: args.query,
            filters: SearchFilters {
                lifecycle: args.lifecycle,
                ..SearchFilters::default()
            },
        },
    )
    .await
    .map_err(cabinet_error)?;
    Ok(json!({ "domain": "cabinet", "items": items }))
}

#[derive(Debug, Deserialize)]
struct CabinetReadArguments {
    cabinet_ref: String,
    #[serde(default)]
    version_ref: Option<String>,
}

async fn cabinet_read(
    pool: &PgPool,
    context: &DenToolInvocationContext,
    role: BearProfile,
    arguments: Value,
) -> Result<Value, CustomError> {
    let args: CabinetReadArguments = parse_arguments(arguments)?;
    let view = den_service::cabinet::read(
        pool,
        ReadRequest {
            scope: actor_scope(context, role),
            cabinet_ref: parse_item_ref(&args.cabinet_ref)?,
            version_ref: args
                .version_ref
                .as_deref()
                .map(CabinetVersionRef::parse)
                .transpose()
                .map_err(|error| CustomError::ValidationError(error.to_string()))?,
        },
    )
    .await
    .map_err(cabinet_error)?;
    Ok(
        json!({ "domain": "cabinet", "item": view.item, "version": view.version, "sources": view.sources }),
    )
}

#[derive(Debug, Deserialize)]
struct CabinetCreateArguments {
    title: String,
    content: String,
    #[serde(default)]
    source_links: Vec<NewSourceLink>,
}

async fn cabinet_create(
    pool: &PgPool,
    context: &DenToolInvocationContext,
    role: BearProfile,
    authority: CabinetToolAuthority,
    arguments: Value,
) -> Result<Value, CustomError> {
    require_write_authority(DEN_CABINET_CREATE, authority)?;
    let args: CabinetCreateArguments = parse_arguments(arguments)?;
    let view = den_service::cabinet::create_item(
        pool,
        CreateItemRequest {
            scope: actor_scope(context, role),
            kind: ItemKind::Document,
            title: args.title,
            content: args.content,
            collection_ref: None,
            mission_ref: None,
            source_links: args.source_links,
        },
    )
    .await
    .map_err(cabinet_error)?;
    Ok(
        json!({ "domain": "cabinet", "item": view.item, "version": view.version, "sources": view.sources }),
    )
}

#[derive(Debug, Deserialize)]
struct CabinetUpdateArguments {
    cabinet_ref: String,
    content: String,
    base_version: String,
    #[serde(default)]
    title: Option<String>,
}

async fn cabinet_update(
    pool: &PgPool,
    context: &DenToolInvocationContext,
    role: BearProfile,
    authority: CabinetToolAuthority,
    arguments: Value,
) -> Result<Value, CustomError> {
    require_write_authority(DEN_CABINET_UPDATE, authority)?;
    let args: CabinetUpdateArguments = parse_arguments(arguments)?;
    let view = den_service::cabinet::update_item(
        pool,
        UpdateItemRequest {
            scope: actor_scope(context, role),
            cabinet_ref: parse_item_ref(&args.cabinet_ref)?,
            content: args.content,
            base_version: CabinetVersionRef::parse(&args.base_version)
                .map_err(|error| CustomError::ValidationError(error.to_string()))?,
            title: args.title,
        },
    )
    .await
    .map_err(cabinet_error)?;
    Ok(
        json!({ "domain": "cabinet", "item": view.item, "version": view.version, "sources": view.sources }),
    )
}

#[derive(Debug, Deserialize)]
struct CabinetHistoryArguments {
    cabinet_ref: String,
}

async fn cabinet_history(
    pool: &PgPool,
    context: &DenToolInvocationContext,
    role: BearProfile,
    arguments: Value,
) -> Result<Value, CustomError> {
    let args: CabinetHistoryArguments = parse_arguments(arguments)?;
    let versions = den_service::cabinet::history(
        pool,
        HistoryRequest {
            scope: actor_scope(context, role),
            cabinet_ref: parse_item_ref(&args.cabinet_ref)?,
        },
    )
    .await
    .map_err(cabinet_error)?;
    Ok(json!({ "domain": "cabinet", "versions": versions }))
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
enum SourceLinkAction {
    #[default]
    Add,
    Remove,
}

#[derive(Debug, Deserialize)]
struct CabinetSourceLinkArguments {
    cabinet_ref: String,
    #[serde(default)]
    action: SourceLinkAction,
    #[serde(default)]
    source_kind: Option<SourceKind>,
    #[serde(default)]
    locator: Option<String>,
    #[serde(default)]
    role: Option<SourceRole>,
    #[serde(default)]
    source_ref: Option<String>,
}

async fn cabinet_source_link(
    pool: &PgPool,
    context: &DenToolInvocationContext,
    role: BearProfile,
    authority: CabinetToolAuthority,
    arguments: Value,
) -> Result<Value, CustomError> {
    require_write_authority(DEN_CABINET_SOURCE_LINK, authority)?;
    let args: CabinetSourceLinkArguments = parse_arguments(arguments)?;
    let cabinet_ref = parse_item_ref(&args.cabinet_ref)?;
    let scope = actor_scope(context, role);

    match args.action {
        SourceLinkAction::Add => {
            let (Some(source_kind), Some(locator), Some(link_role)) =
                (args.source_kind, args.locator, args.role)
            else {
                return Err(CustomError::ValidationError(
                    "adding a source link requires source_kind, locator, and role".to_string(),
                ));
            };
            let link = den_service::cabinet::link_source(
                pool,
                LinkSourceRequest {
                    scope,
                    cabinet_ref,
                    link: NewSourceLink {
                        source_kind,
                        locator,
                        role: link_role,
                    },
                },
            )
            .await
            .map_err(cabinet_error)?;
            Ok(json!({ "domain": "cabinet", "action": "add", "source": link }))
        }
        SourceLinkAction::Remove => {
            let Some(source_ref) = args.source_ref.as_deref() else {
                return Err(CustomError::ValidationError(
                    "removing a source link requires source_ref".to_string(),
                ));
            };
            let source_ref = CabinetSourceRef::parse(source_ref)
                .map_err(|error| CustomError::ValidationError(error.to_string()))?;
            den_service::cabinet::unlink_source(
                pool,
                UnlinkSourceRequest {
                    scope,
                    cabinet_ref,
                    source_ref,
                },
            )
            .await
            .map_err(cabinet_error)?;
            Ok(json!({ "domain": "cabinet", "action": "remove", "removed": true }))
        }
    }
}

#[derive(Debug, Deserialize)]
struct CabinetLifecycleArguments {
    cabinet_ref: String,
    lifecycle: Lifecycle,
}

async fn cabinet_lifecycle(
    pool: &PgPool,
    context: &DenToolInvocationContext,
    role: BearProfile,
    authority: CabinetToolAuthority,
    arguments: Value,
) -> Result<Value, CustomError> {
    require_write_authority(DEN_CABINET_LIFECYCLE, authority)?;
    let args: CabinetLifecycleArguments = parse_arguments(arguments)?;
    let cabinet_ref = parse_item_ref(&args.cabinet_ref)?;
    let scope = actor_scope(context, role);
    match args.lifecycle {
        Lifecycle::Archived => den_service::cabinet::archive_item(pool, &scope, &cabinet_ref)
            .await
            .map_err(cabinet_error)?,
        Lifecycle::Active => den_service::cabinet::restore_item(pool, &scope, &cabinet_ref)
            .await
            .map_err(cabinet_error)?,
        Lifecycle::Deleted => {
            return Err(CustomError::Authorization(
                "deleting a Cabinet item is reserved to people; archive it instead".to_string(),
            ))
        }
    }
    Ok(json!({ "domain": "cabinet", "cabinet_ref": args.cabinet_ref, "lifecycle": args.lifecycle }))
}
