#[path = "history.rs"]
mod history;

use super::protocol::RpcError;
use den_cabinet::{
    ActorScope, CabinetAttachmentRef, CabinetError, CabinetItemRef, CabinetVersionRef, Lifecycle,
    ReadRequest, SearchFilters, SearchRequest,
};
use den_core::tools::{
    constants::{DEN_CABINET_HISTORY, DEN_CABINET_READ, DEN_CABINET_SEARCH},
    descriptor::{builtin_den_tool_descriptor_for_provider_name, DenToolDescriptor},
};
use den_service::{
    artifacts::bytes::ArtifactByteReader,
    cabinet::{self, attachment_read},
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::PgPool;

#[derive(Clone, Copy)]
enum Operation {
    Search,
    Read,
    History,
}
impl Operation {
    const ALL: [Self; 3] = [Self::Search, Self::Read, Self::History];
    fn canonical(self) -> &'static str {
        match self {
            Self::Search => DEN_CABINET_SEARCH,
            Self::Read => DEN_CABINET_READ,
            Self::History => DEN_CABINET_HISTORY,
        }
    }
    fn descriptor(self) -> Result<DenToolDescriptor, RpcError> {
        let descriptor = builtin_den_tool_descriptor_for_provider_name(self.canonical())
            .ok_or(RpcError::Internal)?;
        // Explicit delegated-human projection, not a pretend native audience.
        // Fail closed if the shared permission metadata stops being read-only.
        if descriptor.permissions != ["cabinet.read"] {
            return Err(RpcError::Internal);
        }
        Ok(descriptor)
    }
    fn resolve(name: &str) -> Result<Self, RpcError> {
        let descriptor =
            builtin_den_tool_descriptor_for_provider_name(name).ok_or(RpcError::InvalidParams)?;
        Self::ALL
            .into_iter()
            .find(|operation| operation.canonical() == descriptor.name)
            .ok_or(RpcError::InvalidParams)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ListArgs {
    #[serde(default)]
    cursor: Option<String>,
    #[serde(default, rename = "_meta")]
    _meta: Option<serde_json::Map<String, Value>>,
}
pub(super) fn list(params: Value) -> Result<Value, RpcError> {
    let params = if params.is_null() { json!({}) } else { params };
    let args: ListArgs = serde_json::from_value(params).map_err(|_| RpcError::InvalidParams)?;
    if args.cursor.is_some() {
        return Err(RpcError::InvalidParams);
    }
    let tools = Operation::ALL.into_iter().map(|operation| {
        let descriptor = operation.descriptor()?;
        Ok(json!({
            "name": descriptor.provider_name,
            "title": descriptor.label,
            "description": descriptor.description,
            "inputSchema": descriptor.input_schema,
            "annotations": {"readOnlyHint": true, "destructiveHint": false, "idempotentHint": true, "openWorldHint": false},
            "_meta": {"den": {"canonicalName": descriptor.name, "permissions": descriptor.permissions, "approvalPolicy": descriptor.approval_policy, "nativeAudiences": descriptor.allowed_origins, "projection": "delegated_human_read_only"}}
        }))
    }).collect::<Result<Vec<_>, RpcError>>()?;
    Ok(json!({"tools": tools}))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CallArgs {
    name: String,
    #[serde(default = "empty_object")]
    arguments: Value,
    #[serde(default, rename = "_meta")]
    _meta: Option<serde_json::Map<String, Value>>,
}
fn empty_object() -> Value {
    json!({})
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct SearchArgs {
    query: String,
    #[serde(default)]
    lifecycle: Option<SearchLifecycle>,
}
#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum SearchLifecycle {
    Active,
    Archived,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ReadArgs {
    cabinet_ref: CabinetItemRef,
    #[serde(default)]
    version_ref: Option<CabinetVersionRef>,
    #[serde(default)]
    attachment_ref: Option<CabinetAttachmentRef>,
    #[serde(default)]
    offset_chars: usize,
    #[serde(default = "default_limit")]
    limit_chars: usize,
}
fn default_limit() -> usize {
    attachment_read::DEFAULT_TEXT_LIMIT
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct HistoryArgs {
    cabinet_ref: CabinetItemRef,
}
pub(super) enum Call {
    Search(SearchArgs),
    Read(ReadArgs),
    History(HistoryArgs),
}

pub(super) fn parse_call(params: Value) -> Result<Call, RpcError> {
    let args: CallArgs = serde_json::from_value(params).map_err(|_| RpcError::InvalidParams)?;
    let operation = Operation::resolve(&args.name)?;
    operation.descriptor()?;
    match operation {
        Operation::Search => {
            let args: SearchArgs =
                serde_json::from_value(args.arguments).map_err(|_| RpcError::InvalidParams)?;
            if args.query.len() > 4096 {
                return Err(RpcError::InvalidParams);
            }
            Ok(Call::Search(args))
        }
        Operation::Read => {
            let args: ReadArgs =
                serde_json::from_value(args.arguments).map_err(|_| RpcError::InvalidParams)?;
            if args.version_ref.is_some() && args.attachment_ref.is_some()
                || !(1..=attachment_read::MAX_TEXT_LIMIT).contains(&args.limit_chars)
            {
                return Err(RpcError::InvalidParams);
            }
            Ok(Call::Read(args))
        }
        Operation::History => serde_json::from_value(args.arguments)
            .map(Call::History)
            .map_err(|_| RpcError::InvalidParams),
    }
}

#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
enum ToolErrorKind {
    NotFound,
    NotAuthorized,
    Conflict,
    Validation,
    Policy,
    Unavailable,
}
#[derive(Serialize)]
struct ToolError {
    kind: ToolErrorKind,
    message: &'static str,
}
impl From<CabinetError> for ToolError {
    fn from(error: CabinetError) -> Self {
        match error {
            CabinetError::NotFound => Self {
                kind: ToolErrorKind::NotFound,
                message: "Cabinet item not found",
            },
            CabinetError::NotAuthorized => Self {
                kind: ToolErrorKind::NotAuthorized,
                message: "Not authorized for this operation",
            },
            CabinetError::Conflict { .. } => Self {
                kind: ToolErrorKind::Conflict,
                message: "Cabinet version conflict",
            },
            CabinetError::Validation(_) => Self {
                kind: ToolErrorKind::Validation,
                message: "Invalid Cabinet input",
            },
            CabinetError::Policy(_) => Self {
                kind: ToolErrorKind::Policy,
                message: "Cabinet policy prevents this operation",
            },
            CabinetError::Storage(_) => Self {
                kind: ToolErrorKind::Unavailable,
                message: "Cabinet temporarily unavailable",
            },
        }
    }
}
impl From<den_core::DenError> for ToolError {
    fn from(error: den_core::DenError) -> Self {
        match error {
            den_core::DenError::NotFound(_) | den_core::DenError::Authorization(_) => {
                CabinetError::NotFound.into()
            }
            den_core::DenError::ValidationError(_) => Self {
                kind: ToolErrorKind::Validation,
                message: "Attachment text unavailable or invalid range",
            },
            _ => Self {
                kind: ToolErrorKind::Unavailable,
                message: "Attachment temporarily unavailable",
            },
        }
    }
}

#[cfg(test)]
pub(super) fn tool_result(result: Result<Value, CabinetError>) -> Result<Value, RpcError> {
    finish(result.map_err(ToolError::from))
}
fn finish(result: Result<Value, ToolError>) -> Result<Value, RpcError> {
    let (value, is_error) = match result {
        Ok(value) => (value, false),
        Err(error) => (json!({"error": error}), true),
    };
    let text = serde_json::to_string(&value).map_err(|_| RpcError::Internal)?;
    if text.len() > 256 * 1024 {
        return finish(Err(ToolError {
            kind: ToolErrorKind::Validation,
            message: "Result exceeds the MCP response limit",
        }));
    }
    // Text fallback is required for older clients; structuredContent is the
    // June-2025 machine-readable projection of the identical object.
    Ok(
        json!({"content": [{"type": "text", "text": text}], "structuredContent": value, "isError": is_error}),
    )
}

pub(super) async fn dispatch(
    pool: &PgPool,
    reader: Option<&dyn ArtifactByteReader>,
    scope: &ActorScope,
    call: Call,
) -> Result<Value, RpcError> {
    let result: Result<Value, ToolError> = async {
        match call {
            Call::Search(args) => {
                let items = cabinet::search(pool, SearchRequest {
                    scope: scope.clone(), query: args.query,
                    filters: SearchFilters { lifecycle: args.lifecycle.map(|lifecycle| match lifecycle { SearchLifecycle::Active => Lifecycle::Active, SearchLifecycle::Archived => Lifecycle::Archived }), ..SearchFilters::default() },
                }).await?;
                Ok(json!({"items": items}))
            }
            Call::History(args) => {
                let history = history::read(pool, scope, &args.cabinet_ref).await?;
                Ok(serde_json::to_value(history).map_err(|_| ToolError {
                    kind: ToolErrorKind::Unavailable,
                    message: "Cabinet temporarily unavailable",
                })?)
            }
            Call::Read(args) => {
                if let Some(attachment) = args.attachment_ref {
                    let range = attachment_read::TextRange::new(args.offset_chars, args.limit_chars)?;
                    let text = attachment_read::read_text(pool, scope, &args.cabinet_ref, &attachment, range, reader).await?;
                    return Ok(json!({"attachment": text}));
                }
                let view = cabinet::read(pool, ReadRequest { scope: scope.clone(), cabinet_ref: args.cabinet_ref.clone(), version_ref: args.version_ref.clone() }).await?;
                let version = &view.version;
                let total_chars = version.content().chars().count();
                if args.offset_chars > total_chars { return Err(ToolError { kind: ToolErrorKind::Validation, message: "offset_chars is beyond the page text" }); }
                let text: String = version.content().chars().skip(args.offset_chars).take(args.limit_chars).collect();
                let end = args.offset_chars + text.chars().count();
                let attachments = if args.version_ref.is_none() { attachment_read::list(pool, scope, &args.cabinet_ref, reader.is_some()).await? } else { Vec::new() };
                // Recheck the exact selected version's review authority too,
                // not merely page read access, after attachment discovery.
                cabinet::read(pool, ReadRequest { scope: scope.clone(), cabinet_ref: args.cabinet_ref, version_ref: Some(version.version_ref().clone()) }).await?;
                Ok(json!({
                    "item": view.item,
                    "version": {"version_ref": version.version_ref(), "revision": version.revision(), "content": text, "content_sha256": version.content_sha256(), "base_version": version.base_version(), "review": version.review(), "authored_by": version.authored_by(), "authored_at": version.authored_at()},
                    "sources": view.sources, "proposed_title": view.proposed_title, "attachments": attachments,
                    "offset_chars": args.offset_chars, "total_chars": total_chars, "next_offset_chars": (end < total_chars).then_some(end)
                }))
            }
        }
    }.await;
    finish(result)
}
