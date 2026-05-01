//! Minimal Agent Client Protocol (ACP) gateway for adapter clients.
//!
//! This is the Phase 7 basic-chat slice: Den authenticates, authorizes the selected bear,
//! injects trusted context, and maps text prompts to Codepool `bear_channel`. Client-tool
//! relay and full ACP stdio transport live in later slices / an external adapter.

use axum::{
    body::Body,
    extract::{Path, State},
    http::{header, HeaderMap, HeaderName, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    routing::post,
    Json, Router,
};
use bytes::Bytes;
use futures::{ready, Stream};
use serde::{Deserialize, Serialize};
use std::{future::Future, pin::Pin, task::Poll};
use tracing::Instrument;
use uuid::Uuid;

use crate::{
    api::{
        auth::{self, ApiError},
        oauth::OAuthScope,
        service::ApiState,
    },
    core::{
        acp_client_tools::{self, NewAcpClientToolCall},
        acp_sessions::{self, UpsertAcpSession},
        acp_tokens,
        bears::db as bears_db,
        codepool::CodepoolToolResultRequest,
        user,
    },
    errors::CustomError,
};

pub fn router() -> Router<ApiState> {
    Router::new()
        .route("/bears/{slug}/sessions/{session_id}/prompt", post(prompt))
        .route(
            "/bears/{slug}/sessions/{session_id}/tool-results/{call_id}",
            post(tool_result),
        )
        .route(
            "/bears/{slug}/sessions/{session_id}/close",
            post(close_session),
        )
}

#[derive(Debug, Deserialize)]
pub struct AcpPromptRequest {
    pub message: String,
    #[serde(default)]
    pub conversation_id: Option<String>,
    #[serde(default)]
    pub client: Option<String>,
    #[serde(default)]
    pub client_capabilities: serde_json::Value,
    #[serde(default)]
    pub client_context: serde_json::Value,
}

#[derive(Debug, Deserialize)]
pub struct AcpToolResultRequest {
    pub request_id: Uuid,
    #[serde(default = "default_conversation_id")]
    pub conversation_id: String,
    pub status: String,
    #[serde(default)]
    pub result: Option<serde_json::Value>,
    #[serde(default)]
    pub error: Option<serde_json::Value>,
    #[serde(default)]
    pub client_observation: Option<serde_json::Value>,
}

#[derive(Debug, Serialize)]
struct AcpToolResultResponse {
    ok: bool,
    delivered: bool,
    call_id: String,
}

#[derive(Debug, Serialize)]
struct AcpCloseSessionResponse {
    ok: bool,
    archived: bool,
    conversation_id: Option<String>,
}

#[derive(Debug, Serialize)]
struct AcpErrorResponse {
    error: String,
    error_code: &'static str,
    request_id: String,
}

fn default_conversation_id() -> String {
    "default".to_string()
}

fn acp_error_status_message(err: &CustomError) -> (StatusCode, &'static str, String) {
    match err {
        CustomError::Authentication(s) => (StatusCode::UNAUTHORIZED, "authentication", s.clone()),
        CustomError::Authorization(s) => (StatusCode::FORBIDDEN, "authorization", s.clone()),
        CustomError::NotFound(s) => (StatusCode::NOT_FOUND, "not_found", s.clone()),
        CustomError::ValidationError(s) => (StatusCode::BAD_REQUEST, "validation", s.clone()),
        CustomError::DatabaseUnavailable(s) => (
            StatusCode::SERVICE_UNAVAILABLE,
            "database_unavailable",
            s.clone(),
        ),
        CustomError::System(s)
        | CustomError::Database(s)
        | CustomError::Session(s)
        | CustomError::Parsing(s) => (StatusCode::UNPROCESSABLE_ENTITY, "unprocessable", s.clone()),
        CustomError::Render(s) => (StatusCode::INTERNAL_SERVER_ERROR, "render", s.clone()),
        CustomError::Email(s) => (StatusCode::FAILED_DEPENDENCY, "email", s.clone()),
        CustomError::Anyhow(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal",
            format!("{e:#}"),
        ),
    }
}

fn acp_error_response(err: CustomError, request_id: Uuid) -> Response {
    tracing::error!(%request_id, error = %err, "ACP prompt rejected");
    let (status, error_code, message) = acp_error_status_message(&err);
    let request_id_header = HeaderValue::from_str(&request_id.to_string())
        .unwrap_or_else(|_| HeaderValue::from_static("invalid"));
    let body = serde_json::to_string(&AcpErrorResponse {
        error: message,
        error_code,
        request_id: request_id.to_string(),
    })
    .unwrap_or_else(|_| "{\"error\":\"response serialization failed\"}".to_string());

    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "application/json; charset=utf-8")
        .header(HeaderName::from_static("x-request-id"), request_id_header)
        .body(Body::from(body))
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

fn api_auth_error_response(err: ApiError, request_id: Uuid) -> Response {
    tracing::error!(
        %request_id,
        error_code = err.error_code,
        error = %err.message,
        "ACP prompt authentication rejected"
    );
    let request_id_header = HeaderValue::from_str(&request_id.to_string())
        .unwrap_or_else(|_| HeaderValue::from_static("invalid"));
    let body = serde_json::to_string(&AcpErrorResponse {
        error: err.message,
        error_code: err.error_code,
        request_id: request_id.to_string(),
    })
    .unwrap_or_else(|_| "{\"error\":\"response serialization failed\"}".to_string());

    Response::builder()
        .status(err.status)
        .header(header::CONTENT_TYPE, "application/json; charset=utf-8")
        .header(HeaderName::from_static("x-request-id"), request_id_header)
        .body(Body::from(body))
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

fn normalize_acp_client(raw: Option<&str>) -> String {
    let value = raw
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("acp_adapter");
    match value.to_ascii_lowercase().as_str() {
        "zed" => "zed".to_string(),
        "opencode" => "opencode".to_string(),
        _ => "acp_adapter".to_string(),
    }
}

fn client_supports_read_text_file(client_capabilities: &serde_json::Value) -> bool {
    client_capabilities
        .pointer("/fs/readTextFile")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
        || client_capabilities
            .pointer("/filesystem/readTextFile")
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
}

fn acp_conversation_id(client: &str, session_id: &str) -> String {
    format!("new-acp-{client}-{}", stable_session_suffix(session_id))
}

fn stable_session_suffix(session_id: &str) -> String {
    session_id
        .chars()
        .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '-' })
        .collect::<String>()
        .trim_matches('-')
        .chars()
        .take(80)
        .collect()
}

fn authorized_client_tool_descriptors(
    has_acp_tools_scope: bool,
    client: &str,
    client_capabilities: &serde_json::Value,
    client_context: &serde_json::Value,
) -> Vec<serde_json::Value> {
    if !has_acp_tools_scope || !client_supports_read_text_file(client_capabilities) {
        return Vec::new();
    }
    vec![serde_json::json!({
        "id": "acp_fs_read_text_file",
        "name": "acp_fs_read_text_file",
        "title": "Read text file from editor workspace",
        "description": "Read a UTF-8 text file from the user's active editor workspace through the ACP client. Use this for inspecting project files that are available to the local editor session.",
        "provider": "acp_client",
        "execution_target": "acp_client",
        "scope": "client_connection",
        "client": client,
        "permissions": ["filesystem", "read"],
        "approval_policy": "on_sensitive_action",
        "input_schema": {
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Workspace-relative or client-accepted path to read."
                }
            },
            "required": ["path"],
            "additionalProperties": false
        },
        "output_schema": {
            "type": "object",
            "properties": {
                "content": { "type": "string" }
            },
            "required": ["content"],
            "additionalProperties": true
        },
        "acp": {
            "method": "fs/read_text_file",
            "requires_client_capability": "fs.readTextFile"
        },
        "client_context": client_context,
    })]
}

async fn prompt(
    State(state): State<ApiState>,
    Path((slug, session_id)): Path<(String, String)>,
    headers: HeaderMap,
    Json(body): Json<AcpPromptRequest>,
) -> impl IntoResponse {
    let request_id = Uuid::new_v4();
    let result = async { prompt_inner(state, slug, session_id, headers, body, request_id).await }
        .instrument(tracing::info_span!("acp_prompt", request_id = %request_id))
        .await;
    match result {
        Ok(Ok(response)) => response,
        Ok(Err(err)) => acp_error_response(err, request_id),
        Err(err) => api_auth_error_response(err, request_id),
    }
}

async fn close_session(
    State(state): State<ApiState>,
    Path((slug, session_id)): Path<(String, String)>,
    headers: HeaderMap,
) -> Response {
    let response_request_id = Uuid::new_v4();
    match close_session_inner(state, slug, session_id, headers).await {
        Ok(response) => response,
        Err(err) => acp_error_response(err, response_request_id),
    }
}

async fn close_session_inner(
    state: ApiState,
    slug: String,
    session_id: String,
    headers: HeaderMap,
) -> Result<Response, CustomError> {
    let token = auth::extract_bearer_token(&headers)
        .map_err(|err| CustomError::Authentication(err.message))?;
    let auth = if acp_tokens::is_acp_token(&token) {
        acp_tokens::authenticate_for_bear_slug_with_scopes(&state.sqlx_pool, &token, &slug)
            .await?
            .ok_or_else(|| {
                CustomError::Authentication(
                    "invalid, expired, revoked, or unauthorized ACP token".to_string(),
                )
            })?
    } else {
        return Err(CustomError::Authentication(
            "ACP session close requires a bear-scoped ACP Code token".to_string(),
        ));
    };

    let Some(session) = acp_sessions::find_for_user_bear_session(
        &state.sqlx_pool,
        auth.user_id,
        &slug,
        &session_id,
    )
    .await?
    else {
        return Ok(Json(AcpCloseSessionResponse {
            ok: true,
            archived: false,
            conversation_id: None,
        })
        .into_response());
    };

    acp_sessions::mark_closed(&state.sqlx_pool, session.id).await?;
    let archive_target = session
        .resolved_conversation_id
        .as_deref()
        .unwrap_or(&session.conversation_id);
    let mut archived = false;
    if archive_target.starts_with("conv-") && state.letta.is_enabled() {
        state
            .letta
            .patch_conversation_archived(archive_target, true)
            .await?;
        acp_sessions::mark_archived(&state.sqlx_pool, session.id).await?;
        archived = true;
    }

    Ok(Json(AcpCloseSessionResponse {
        ok: true,
        archived,
        conversation_id: Some(archive_target.to_string()),
    })
    .into_response())
}

async fn tool_result(
    State(state): State<ApiState>,
    Path((slug, session_id, call_id)): Path<(String, String, String)>,
    headers: HeaderMap,
    Json(body): Json<AcpToolResultRequest>,
) -> Response {
    let response_request_id = Uuid::new_v4();
    match tool_result_inner(state, slug, session_id, call_id, headers, body).await {
        Ok(response) => response,
        Err(err) => acp_error_response(err, response_request_id),
    }
}

async fn tool_result_inner(
    state: ApiState,
    slug: String,
    session_id: String,
    call_id: String,
    headers: HeaderMap,
    body: AcpToolResultRequest,
) -> Result<Response, CustomError> {
    let token = auth::extract_bearer_token(&headers)
        .map_err(|err| CustomError::Authentication(err.message))?;
    let auth = if acp_tokens::is_acp_token(&token) {
        acp_tokens::authenticate_for_bear_slug_with_scopes(&state.sqlx_pool, &token, &slug)
            .await?
            .ok_or_else(|| {
                CustomError::Authentication(
                    "invalid, expired, revoked, or unauthorized ACP token".to_string(),
                )
            })?
    } else {
        return Err(CustomError::Authentication(
            "ACP tool results require a bear-scoped ACP Code token".to_string(),
        ));
    };
    if !acp_tokens::scopes_contains(&auth.scopes, OAuthScope::AcpTools.as_str()) {
        return Err(CustomError::Authorization(
            "ACP tool result requires acp:tools scope".to_string(),
        ));
    }
    let db_status = normalize_tool_result_status(&body.status)?;
    let codepool_status = normalize_codepool_tool_result_status(&body.status)?;
    let pending = acp_client_tools::find_active_call_for_result(
        &state.sqlx_pool,
        auth.user_id,
        &slug,
        &session_id,
        body.request_id,
        &call_id,
    )
    .await?
    .ok_or_else(|| CustomError::NotFound("pending ACP client tool call not found".to_string()))?;

    acp_client_tools::mark_result_received(
        &state.sqlx_pool,
        pending.id,
        db_status,
        body.result.clone(),
        body.error.clone(),
        body.client_observation.clone(),
    )
    .await?;

    let codepool_payload = CodepoolToolResultRequest {
        conversation_id: body.conversation_id.trim().to_string(),
        request_id: body.request_id.to_string(),
        call_id: call_id.clone(),
        tool_name: pending.tool_name.clone(),
        status: codepool_status.to_string(),
        result: body.result,
        error: body.error,
    };
    let delivered = state
        .codepool
        .post_bear_channel_tool_result(
            &pending.codepool_session_id,
            &codepool_payload,
            body.request_id,
        )
        .await
        .map(|response| response.delivered)
        .unwrap_or(false);
    if delivered {
        acp_client_tools::mark_forwarded(&state.sqlx_pool, pending.id).await?;
    }
    Ok(Json(AcpToolResultResponse {
        ok: true,
        delivered,
        call_id,
    })
    .into_response())
}

fn normalize_tool_result_status(status: &str) -> Result<&'static str, CustomError> {
    match status.trim() {
        "ok" => Ok("result_received"),
        "error" => Ok("failed"),
        "cancelled" => Ok("cancelled"),
        "timeout" => Ok("timed_out"),
        other => Err(CustomError::ValidationError(format!(
            "unsupported tool result status: {other}"
        ))),
    }
}

fn normalize_codepool_tool_result_status(status: &str) -> Result<&'static str, CustomError> {
    match status.trim() {
        "ok" => Ok("ok"),
        "error" => Ok("error"),
        "cancelled" => Ok("cancelled"),
        "timeout" => Ok("timeout"),
        other => Err(CustomError::ValidationError(format!(
            "unsupported tool result status: {other}"
        ))),
    }
}

async fn prompt_inner(
    state: ApiState,
    slug: String,
    session_id: String,
    headers: HeaderMap,
    body: AcpPromptRequest,
    request_id: Uuid,
) -> Result<Result<Response, CustomError>, ApiError> {
    let token = auth::extract_bearer_token(&headers)?;
    let slug = slug.trim().to_string();
    let (user_id, has_acp_tools_scope) = if acp_tokens::is_acp_token(&token) {
        let auth =
            acp_tokens::authenticate_for_bear_slug_with_scopes(&state.sqlx_pool, &token, &slug)
                .await
                .map_err(|err| {
                    ApiError::new(
                        StatusCode::UNPROCESSABLE_ENTITY,
                        "database",
                        err.to_string(),
                    )
                })?
                .ok_or_else(|| {
                    ApiError::new(
                        StatusCode::UNAUTHORIZED,
                        "invalid_token",
                        "invalid, expired, revoked, or unauthorized ACP token",
                    )
                })?;
        if !acp_tokens::scopes_contains(&auth.scopes, OAuthScope::AcpChat.as_str()) {
            return Err(ApiError::new(
                StatusCode::UNAUTHORIZED,
                "invalid_token",
                "invalid, expired, revoked, or unauthorized ACP token",
            ));
        }
        (
            auth.user_id,
            acp_tokens::scopes_contains(&auth.scopes, OAuthScope::AcpTools.as_str()),
        )
    } else {
        let principal = auth::authenticate_bearer(&headers)?;
        auth::require_scope(&principal, OAuthScope::AcpChat)?;
        (
            principal.user_id,
            principal.scopes.contains(&OAuthScope::AcpTools),
        )
    };
    let prompt = body.message.trim();
    if prompt.is_empty() {
        return Ok(Err(CustomError::ValidationError(
            "message must not be empty".to_string(),
        )));
    }

    let slug = slug.trim();
    if slug.is_empty() {
        return Ok(Err(CustomError::NotFound("bear not found".to_string())));
    }

    let bear = bears_db::bear_for_user_by_slug(&state.sqlx_pool, user_id, slug)
        .await
        .map_err(|err| {
            ApiError::new(
                StatusCode::UNPROCESSABLE_ENTITY,
                "database",
                err.to_string(),
            )
        })?
        .ok_or_else(|| {
            ApiError::new(
                StatusCode::NOT_FOUND,
                "not_found",
                "bear not found or you do not have access",
            )
        })?;

    if bear
        .letta_agent_id
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .is_none()
    {
        return Ok(Err(CustomError::System(
            "This bear is not provisioned in Letta yet (missing letta_agent_id).".to_string(),
        )));
    }

    let session_id = session_id.trim();
    if session_id.is_empty() {
        return Ok(Err(CustomError::ValidationError(
            "session_id must not be empty".to_string(),
        )));
    }

    if let Some(raw_conversation_id) = body
        .conversation_id
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty() && *s != "default")
    {
        tracing::info!(
            %request_id,
            acp_session_id = %session_id,
            requested_conversation_id = %raw_conversation_id,
            "ACP gateway ignoring client conversation_id; loadSession is not supported yet"
        );
    }
    let client = normalize_acp_client(body.client.as_deref());
    let conversation_id = acp_conversation_id(&client, session_id);
    let client_tools = authorized_client_tool_descriptors(
        has_acp_tools_scope,
        &client,
        &body.client_capabilities,
        &body.client_context,
    );
    if !has_acp_tools_scope && client_supports_read_text_file(&body.client_capabilities) {
        tracing::info!(
            %request_id,
            acp_session_id = %session_id,
            bear_slug = %slug,
            client = %client,
            "acp_tools_scope_missing; omitting ACP client tool descriptors"
        );
    }
    let username = user::user_by_id(&state.sqlx_pool, user_id)
        .await
        .ok()
        .map(|u| u.username);
    let membership_role = bears_db::membership_role_for_user(&state.sqlx_pool, user_id, bear.id)
        .await
        .map_err(|err| {
            ApiError::new(
                StatusCode::UNPROCESSABLE_ENTITY,
                "database",
                err.to_string(),
            )
        })?
        .flatten();
    let runtime_plan =
        crate::core::bears::effective_runtime_plan(bear.runtime_plan.as_ref().map(|j| j.as_ref()));

    let channel_session_id = format!("acp:{client}:{}:{session_id}", bear.id);
    acp_sessions::upsert_session(
        &state.sqlx_pool,
        UpsertAcpSession {
            user_id,
            bear_id: bear.id,
            bear_slug: bear.slug.clone(),
            acp_session_id: session_id.to_string(),
            codepool_session_id: channel_session_id.clone(),
            conversation_id: conversation_id.clone(),
            client: client.clone(),
            cwd: body
                .client_context
                .get("cwd")
                .and_then(|v| v.as_str())
                .map(str::to_string),
        },
    )
    .await
    .map_err(|err| {
        ApiError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "database",
            err.to_string(),
        )
    })?;
    let letta_agent_id = bear.letta_agent_id.as_deref().unwrap_or("unknown");
    tracing::info!(
        %request_id,
        acp_session_id = %session_id,
        bear_slug = %bear.slug,
        bear_id = %bear.id,
        letta_agent_id = %letta_agent_id,
        client = %client,
        codepool_session_id = %channel_session_id,
        codepool_conversation_id = %conversation_id,
        "ACP gateway routing prompt to Codepool"
    );
    let upstream = match state
        .codepool
        .post_bear_channel_message_for_channel_with_client_tools_streaming(
            &channel_session_id,
            &conversation_id,
            &bear,
            user_id,
            username.as_deref(),
            membership_role.as_deref(),
            prompt,
            &runtime_plan,
            request_id,
            "coding_workspace",
            &client,
            "agent_client_protocol",
            false,
            true,
            client_tools,
        )
        .await
    {
        Ok(upstream) => upstream,
        Err(err) => return Ok(Err(err)),
    };

    let stream = AcpBearChannelSseStream::new(
        upstream.bytes_stream(),
        AcpStreamContext {
            pool: state.sqlx_pool.clone(),
            user_id,
            bear_id: bear.id,
            bear_slug: bear.slug.clone(),
            acp_session_id: session_id.to_string(),
            codepool_session_id: channel_session_id.clone(),
            conversation_id: conversation_id.clone(),
            request_id,
        },
    );
    let request_id_header = HeaderValue::from_str(&request_id.to_string()).map_err(|_| {
        ApiError::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "invalid_request_id",
            "invalid request id for response header",
        )
    })?;

    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "text/event-stream; charset=utf-8")
        .header(header::CACHE_CONTROL, "no-cache")
        .header(header::CONNECTION, "keep-alive")
        .header(HeaderName::from_static("x-request-id"), request_id_header)
        .body(Body::from_stream(stream))
        .map_err(|e| {
            ApiError::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                "response_build",
                format!("response build: {e}"),
            )
        })
        .map(Ok)
}

#[derive(Clone)]
struct AcpStreamContext {
    pool: sqlx::PgPool,
    user_id: i32,
    bear_id: Uuid,
    bear_slug: String,
    acp_session_id: String,
    codepool_session_id: String,
    conversation_id: String,
    request_id: Uuid,
}

async fn persist_stream_event_side_effects(
    context: &AcpStreamContext,
    event: &serde_json::Value,
) -> Result<(), CustomError> {
    if event.get("type").and_then(|v| v.as_str()) == Some("conversation_resolved") {
        if let Some(conversation_id) = event.get("conversation_id").and_then(|v| v.as_str()) {
            acp_sessions::mark_resolved(
                &context.pool,
                context.user_id,
                context.bear_id,
                &context.acp_session_id,
                conversation_id,
            )
            .await?;
        }
        return Ok(());
    }

    if event.get("type").and_then(|v| v.as_str()) != Some("client_tool_request") {
        return Ok(());
    }
    let call = event
        .get("call")
        .and_then(|v| v.as_object())
        .ok_or_else(|| {
            CustomError::ValidationError("client_tool_request missing call".to_string())
        })?;
    let call_id = call
        .get("id")
        .and_then(|v| v.as_str())
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| {
            CustomError::ValidationError("client_tool_request missing call.id".to_string())
        })?;
    let tool_name = call
        .get("name")
        .and_then(|v| v.as_str())
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| {
            CustomError::ValidationError("client_tool_request missing call.name".to_string())
        })?;
    let timeout_ms = call
        .get("timeout_ms")
        .and_then(|v| v.as_i64())
        .filter(|v| *v > 0)
        .unwrap_or(30_000);
    acp_client_tools::persist_sent_call(
        &context.pool,
        NewAcpClientToolCall {
            user_id: context.user_id,
            bear_id: context.bear_id,
            bear_slug: context.bear_slug.clone(),
            acp_session_id: context.acp_session_id.clone(),
            codepool_session_id: context.codepool_session_id.clone(),
            conversation_id: context.conversation_id.clone(),
            request_id: context.request_id,
            call_id: call_id.to_string(),
            tool_name: tool_name.to_string(),
            arguments: call
                .get("arguments")
                .cloned()
                .unwrap_or_else(|| serde_json::json!({})),
            descriptor: call
                .get("descriptor")
                .cloned()
                .unwrap_or_else(|| serde_json::json!({})),
            timeout_ms,
        },
    )
    .await
}

fn map_bear_channel_event_to_acp_adapter_event(event: &serde_json::Value) -> Option<Bytes> {
    let ty = event.get("type").and_then(|v| v.as_str()).unwrap_or("");
    let mapped = match ty {
        "assistant_delta" => serde_json::json!({
            "type": "agent_message_chunk",
            "content": { "type": "text", "text": event.get("text").and_then(|v| v.as_str()).unwrap_or("") },
        }),
        "reasoning_delta" => serde_json::json!({
            "type": "status",
            "content": { "type": "text", "text": event.get("text").and_then(|v| v.as_str()).unwrap_or("") },
        }),
        "error" => {
            let mut mapped = serde_json::json!({
                "type": "error",
                "message": event.get("message").and_then(|v| v.as_str()).unwrap_or("Upstream error"),
                "detail": event.get("detail").and_then(|v| v.as_str()),
            });
            if let Some(context) = event.get("context") {
                mapped["context"] = context.clone();
            }
            if let Some(request_id) = event.get("request_id") {
                mapped["request_id"] = request_id.clone();
            }
            mapped
        }
        "done" => serde_json::json!({
            "type": "done",
            "outcome": event.get("outcome").and_then(|v| v.as_str()).unwrap_or("ok"),
        }),
        "conversation_resolved" => serde_json::json!({
            "type": "conversation_resolved",
            "conversation_id": event.get("conversation_id").and_then(|v| v.as_str()),
        }),
        "client_tool_request" => {
            let call = event
                .get("call")
                .cloned()
                .unwrap_or_else(|| serde_json::json!({}));
            serde_json::json!({
                "type": "client_tool_request",
                "request_id": event.get("request_id").cloned(),
                "session_id": event.get("session_id").cloned(),
                "conversation_id": event.get("conversation_id").cloned(),
                "call_id": call.get("id").cloned(),
                "tool_name": call.get("name").cloned(),
                "arguments": call.get("arguments").cloned().unwrap_or_else(|| serde_json::json!({})),
                "descriptor": call.get("descriptor").cloned(),
                "approval_policy": call.get("approval_policy").cloned(),
                "timeout_ms": call.get("timeout_ms").cloned().unwrap_or_else(|| serde_json::json!(30000)),
            })
        }
        _ => return None,
    };
    Some(Bytes::from(format!("data: {}\n\n", mapped)))
}

fn parse_bear_channel_frame_values(frame: &[u8]) -> Vec<serde_json::Value> {
    let text = String::from_utf8_lossy(frame);
    let mut out = Vec::new();
    for line in text.lines() {
        let Some(data) = line.strip_prefix("data:") else {
            continue;
        };
        let data = data.trim();
        if data.is_empty() || data == "[DONE]" {
            continue;
        }
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(data) {
            out.push(value);
        }
    }
    out
}

#[cfg(test)]
fn map_bear_channel_frame_to_acp_adapter_events(frame: &[u8]) -> Vec<Bytes> {
    parse_bear_channel_frame_values(frame)
        .into_iter()
        .filter_map(|value| map_bear_channel_event_to_acp_adapter_event(&value))
        .collect()
}

async fn map_bear_channel_frame_to_acp_adapter_events_with_persistence(
    frame: Vec<u8>,
    context: AcpStreamContext,
) -> Result<Vec<Bytes>, std::io::Error> {
    let values = parse_bear_channel_frame_values(&frame);
    let mut out = Vec::new();
    for value in values {
        persist_stream_event_side_effects(&context, &value)
            .await
            .map_err(|err| std::io::Error::other(err.to_string()))?;
        if let Some(bytes) = map_bear_channel_event_to_acp_adapter_event(&value) {
            out.push(bytes);
        }
    }
    Ok(out)
}

struct AcpBearChannelSseStream {
    inner: Pin<Box<dyn Stream<Item = Result<Bytes, reqwest::Error>> + Send>>,
    buffer: Vec<u8>,
    pending: std::collections::VecDeque<Bytes>,
    context: AcpStreamContext,
    persist_future:
        Option<Pin<Box<dyn Future<Output = Result<Vec<Bytes>, std::io::Error>> + Send>>>,
}

impl AcpBearChannelSseStream {
    fn new(
        inner: impl Stream<Item = Result<Bytes, reqwest::Error>> + Send + 'static,
        context: AcpStreamContext,
    ) -> Self {
        Self {
            inner: Box::pin(inner),
            buffer: Vec::new(),
            pending: std::collections::VecDeque::new(),
            context,
            persist_future: None,
        }
    }
}

impl Stream for AcpBearChannelSseStream {
    type Item = Result<Bytes, std::io::Error>;

    fn poll_next(
        mut self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> Poll<Option<Self::Item>> {
        let this = self.as_mut().get_mut();
        if let Some(bytes) = this.pending.pop_front() {
            return Poll::Ready(Some(Ok(bytes)));
        }

        if let Some(fut) = this.persist_future.as_mut() {
            match ready!(fut.as_mut().poll(cx)) {
                Ok(bytes) => {
                    this.persist_future = None;
                    for item in bytes {
                        this.pending.push_back(item);
                    }
                    if let Some(bytes) = this.pending.pop_front() {
                        return Poll::Ready(Some(Ok(bytes)));
                    }
                }
                Err(err) => {
                    this.persist_future = None;
                    return Poll::Ready(Some(Err(err)));
                }
            }
        }

        loop {
            match ready!(this.inner.as_mut().poll_next(cx)) {
                Some(Ok(chunk)) => {
                    this.buffer.extend_from_slice(&chunk);
                    while let Some(pos) = this.buffer.windows(2).position(|w| w == b"\n\n") {
                        let frame: Vec<u8> = this.buffer.drain(..pos + 2).collect();
                        let context = this.context.clone();
                        this.persist_future = Some(Box::pin(async move {
                            map_bear_channel_frame_to_acp_adapter_events_with_persistence(
                                frame, context,
                            )
                            .await
                        }));
                        return self.poll_next(cx);
                    }
                    if let Some(bytes) = this.pending.pop_front() {
                        return Poll::Ready(Some(Ok(bytes)));
                    }
                }
                Some(Err(err)) => {
                    return Poll::Ready(Some(Err(std::io::Error::other(err.to_string()))));
                }
                None => {
                    if !this.buffer.is_empty() {
                        let frame = std::mem::take(&mut this.buffer);
                        let context = this.context.clone();
                        this.persist_future = Some(Box::pin(async move {
                            map_bear_channel_frame_to_acp_adapter_events_with_persistence(
                                frame, context,
                            )
                            .await
                        }));
                        return self.poll_next(cx);
                    }
                    return Poll::Ready(None);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_assistant_delta_to_acp_adapter_event() {
        let out = map_bear_channel_frame_to_acp_adapter_events(
            b"data: {\"type\":\"assistant_delta\",\"text\":\"hello\"}\n\n",
        );
        let text = String::from_utf8(out[0].to_vec()).unwrap();
        assert!(text.contains("\"type\":\"agent_message_chunk\""));
        assert!(text.contains("\"text\":\"hello\""));
    }

    #[test]
    fn maps_done_to_acp_adapter_event() {
        let out = map_bear_channel_frame_to_acp_adapter_events(
            b"data: {\"type\":\"done\",\"outcome\":\"ok\"}\n\n",
        );
        let text = String::from_utf8(out[0].to_vec()).unwrap();
        assert!(text.contains("\"type\":\"done\""));
    }

    #[test]
    fn builds_read_text_file_descriptor_only_when_authorized_and_supported() {
        let caps = serde_json::json!({ "fs": { "readTextFile": true } });
        let context = serde_json::json!({ "cwd": "/tmp/workspace" });
        let descriptors = authorized_client_tool_descriptors(true, "zed", &caps, &context);
        assert_eq!(descriptors.len(), 1);
        assert_eq!(descriptors[0]["name"], "acp_fs_read_text_file");
        assert_eq!(descriptors[0]["acp"]["method"], "fs/read_text_file");

        assert!(authorized_client_tool_descriptors(false, "zed", &caps, &context).is_empty());
        assert!(authorized_client_tool_descriptors(
            true,
            "zed",
            &serde_json::json!({ "fs": { "readTextFile": false } }),
            &context
        )
        .is_empty());
    }

    #[test]
    fn normalizes_acp_tool_result_statuses_for_db_and_codepool() {
        assert_eq!(
            normalize_tool_result_status("ok").unwrap(),
            "result_received"
        );
        assert_eq!(normalize_tool_result_status("error").unwrap(), "failed");
        assert_eq!(
            normalize_tool_result_status("cancelled").unwrap(),
            "cancelled"
        );
        assert_eq!(
            normalize_tool_result_status("timeout").unwrap(),
            "timed_out"
        );

        assert_eq!(normalize_codepool_tool_result_status("ok").unwrap(), "ok");
        assert_eq!(
            normalize_codepool_tool_result_status("error").unwrap(),
            "error"
        );
        assert_eq!(
            normalize_codepool_tool_result_status("cancelled").unwrap(),
            "cancelled"
        );
        assert_eq!(
            normalize_codepool_tool_result_status("timeout").unwrap(),
            "timeout"
        );
    }

    #[test]
    fn maps_client_tool_request_to_adapter_event() {
        let out = map_bear_channel_frame_to_acp_adapter_events(
            br#"data: {"type":"client_tool_request","request_id":"req-1","session_id":"s-1","conversation_id":"default","call":{"id":"call-1","name":"acp_fs_read_text_file","arguments":{"path":"README.md"},"timeout_ms":30000}}

"#,
        );
        let value: serde_json::Value = serde_json::from_str(
            String::from_utf8(out[0].to_vec())
                .unwrap()
                .trim_start_matches("data: ")
                .trim(),
        )
        .unwrap();
        assert_eq!(value["type"], "client_tool_request");
        assert_eq!(value["call_id"], "call-1");
        assert_eq!(value["tool_name"], "acp_fs_read_text_file");
        assert_eq!(value["arguments"]["path"], "README.md");
    }
}
