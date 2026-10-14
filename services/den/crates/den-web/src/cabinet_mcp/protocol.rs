use axum::{
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use den_service::artifacts::bytes::ArtifactByteReader;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::PgPool;

use super::{
    auth::{AuthError, Principal},
    tools,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
pub(super) enum Version {
    #[serde(rename = "2025-03-26")]
    March2025,
    #[serde(rename = "2025-06-18")]
    June2025,
}
impl Version {
    fn header(self) -> HeaderValue {
        HeaderValue::from_static(match self {
            Self::March2025 => "2025-03-26",
            Self::June2025 => "2025-06-18",
        })
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(untagged)]
pub(super) enum Id {
    String(String),
    Number(i64),
    Unsigned(u64),
}

#[derive(Debug, Deserialize)]
pub(super) enum Method {
    #[serde(rename = "initialize")]
    Initialize,
    #[serde(rename = "notifications/initialized")]
    Initialized,
    #[serde(rename = "tools/list")]
    List,
    #[serde(rename = "tools/call")]
    Call,
    #[serde(rename = "ping")]
    Ping,
    #[serde(other)]
    Unknown,
}

#[derive(Deserialize)]
struct Envelope {
    jsonrpc: String,
    method: Method,
    #[serde(default)]
    params: Value,
}
pub(super) struct Request {
    pub id: Option<Id>,
    pub method: Method,
    pub params: Value,
}

#[derive(Debug, Clone, Copy)]
pub(super) enum RpcError {
    Parse,
    InvalidRequest,
    MethodNotFound,
    InvalidParams,
    Internal,
}
impl RpcError {
    fn details(self) -> (i32, &'static str) {
        match self {
            Self::Parse => (-32700, "Parse error"),
            Self::InvalidRequest => (-32600, "Invalid Request"),
            Self::MethodNotFound => (-32601, "Method not found"),
            Self::InvalidParams => (-32602, "Invalid params"),
            Self::Internal => (-32603, "Internal error"),
        }
    }
}

pub(super) fn parse(body: &[u8]) -> Result<Option<Request>, RpcError> {
    let value: Value = serde_json::from_slice(body).map_err(|_| RpcError::Parse)?;
    let object = value.as_object().ok_or(RpcError::InvalidRequest)?;
    let id = object
        .get("id")
        .map(|value| {
            serde_json::from_value::<Id>(value.clone()).map_err(|_| RpcError::InvalidRequest)
        })
        .transpose()?;
    if object.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
        return Err(RpcError::InvalidRequest);
    }
    let has_result = object.contains_key("result");
    let has_error = object.contains_key("error");
    if !object.contains_key("method") {
        if id.is_none() || has_result == has_error || object.contains_key("params") {
            return Err(RpcError::InvalidRequest);
        }
        if let Some(error) = object.get("error") {
            let _: ClientError =
                serde_json::from_value(error.clone()).map_err(|_| RpcError::InvalidRequest)?;
        }
        // Streamable HTTP accepts client responses with 202. This stateless
        // server issues no requests, so there is no pending response to deliver.
        return Ok(None);
    }
    if has_result
        || has_error
        || object
            .get("params")
            .is_some_and(|params| !params.is_object())
    {
        return Err(RpcError::InvalidRequest);
    }
    let envelope: Envelope = serde_json::from_value(value).map_err(|_| RpcError::InvalidRequest)?;
    if envelope.jsonrpc != "2.0" || !(envelope.params.is_null() || envelope.params.is_object()) {
        return Err(RpcError::InvalidRequest);
    }
    Ok(Some(Request {
        id,
        method: envelope.method,
        params: envelope.params,
    }))
}

#[derive(Deserialize)]
struct ClientError {
    #[serde(rename = "code")]
    _code: i32,
    #[serde(rename = "message")]
    _message: String,
}

pub(super) fn version(headers: &HeaderMap) -> Result<Version, AuthError> {
    if headers.get_all("mcp-protocol-version").iter().count() > 1 {
        return Err(AuthError::InvalidHeaders);
    }
    match headers
        .get("mcp-protocol-version")
        .map(HeaderValue::to_str)
        .transpose()
        .map_err(|_| AuthError::InvalidHeaders)?
    {
        None | Some("2025-03-26") => Ok(Version::March2025),
        Some("2025-06-18") => Ok(Version::June2025),
        _ => Err(AuthError::InvalidHeaders),
    }
}

fn accepts(headers: &HeaderMap, expected: &str) -> bool {
    headers
        .get_all(header::ACCEPT)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(','))
        .any(|entry| {
            let mut parts = entry.split(';');
            parts
                .next()
                .is_some_and(|mime| mime.trim().eq_ignore_ascii_case(expected))
                && parts.all(|parameter| {
                    parameter.trim().split_once('=').is_none_or(|(key, value)| {
                        !key.trim().eq_ignore_ascii_case("q")
                            || value
                                .trim()
                                .parse::<f32>()
                                .is_ok_and(|quality| quality > 0.0 && quality <= 1.0)
                    })
                })
        })
}

pub(super) fn validate_headers(headers: &HeaderMap) -> Result<(), AuthError> {
    version(headers)?;
    if !accepts(headers, "application/json") || !accepts(headers, "text/event-stream") {
        return Err(AuthError::NotAcceptable);
    }
    if headers.get_all(header::CONTENT_TYPE).iter().count() != 1
        || !headers
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.split(';').next())
            .is_some_and(|mime| mime.trim().eq_ignore_ascii_case("application/json"))
    {
        return Err(AuthError::UnsupportedMediaType);
    }
    Ok(())
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct Ping {
    #[serde(default, rename = "_meta")]
    _meta: Option<serde_json::Map<String, Value>>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Initialize {
    protocol_version: String,
    capabilities: ClientCapabilities,
    client_info: ClientInfo,
}
#[derive(Deserialize)]
struct ClientCapabilities {}
#[derive(Deserialize)]
struct ClientInfo {
    name: String,
    version: String,
}

pub(super) fn initialize(params: Value) -> Result<(Version, Value), RpcError> {
    let params: Initialize = serde_json::from_value(params).map_err(|_| RpcError::InvalidParams)?;
    let _ = params.capabilities;
    if params.client_info.name.is_empty() || params.client_info.version.is_empty() {
        return Err(RpcError::InvalidParams);
    }
    let negotiated = if params.protocol_version == "2025-03-26" {
        Version::March2025
    } else {
        Version::June2025
    };
    Ok((
        negotiated,
        json!({
            "protocolVersion": negotiated,
            "capabilities": {"tools": {"listChanged": false}},
            "serverInfo": {"name": "cabinet", "version": env!("CARGO_PKG_VERSION")}
        }),
    ))
}

pub(super) fn response(
    id: Option<Id>,
    version: Version,
    result: Result<Value, RpcError>,
) -> Response {
    let (status, payload) = match result {
        Ok(mut result) => {
            if version == Version::March2025 {
                if let Some(object) = result.as_object_mut() {
                    object.remove("structuredContent");
                }
            }
            (
                StatusCode::OK,
                json!({"jsonrpc": "2.0", "id": id, "result": result}),
            )
        }
        Err(error) => {
            let (code, message) = error.details();
            let status = if matches!(error, RpcError::Parse | RpcError::InvalidRequest) {
                StatusCode::BAD_REQUEST
            } else {
                StatusCode::OK
            };
            (
                status,
                json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}}),
            )
        }
    };
    let mut response = (status, Json(payload)).into_response();
    response
        .headers_mut()
        .insert("mcp-protocol-version", version.header());
    response
}

pub(super) async fn handle(
    pool: &PgPool,
    reader: Option<&dyn ArtifactByteReader>,
    principal: &Principal,
    headers: &HeaderMap,
    body: &[u8],
) -> Response {
    // Headers have already been checked by the authenticated transport boundary.
    let mut version = version(headers).unwrap_or(Version::March2025);
    let request = match parse(body) {
        Ok(Some(request)) => request,
        Ok(None) => return StatusCode::ACCEPTED.into_response(),
        Err(error) => return response(None, version, Err(error)),
    };
    if request.id.is_none() {
        // Never dispatch a side effect disguised as a notification. Unknown
        // notifications are ignored; this server has no notification handlers.
        return StatusCode::ACCEPTED.into_response();
    }
    let result = match request.method {
        Method::Initialize => initialize(request.params).map(|(negotiated, result)| {
            version = negotiated;
            result
        }),
        Method::List => tools::list(request.params),
        Method::Call => match tools::parse_call(request.params) {
            Ok(call) => {
                let result = tools::dispatch(pool, reader, &principal.scope, call).await;
                // IDs are trusted server-side provenance, never clientInfo or
                // user-supplied actor fields. Do not log arguments or credentials.
                tracing::info!(user_id = ?principal.scope.actor, oauth_client_id = %principal.client_id, oauth_token_id = principal.token_id, "Cabinet MCP read completed");
                result
            }
            Err(error) => Err(error),
        },
        Method::Ping => {
            let params = if request.params.is_null() {
                json!({})
            } else {
                request.params
            };
            serde_json::from_value::<Ping>(params)
                .map(|_| json!({}))
                .map_err(|_| RpcError::InvalidParams)
        }
        Method::Initialized | Method::Unknown => Err(RpcError::MethodNotFound),
    };
    response(request.id, version, result)
}
