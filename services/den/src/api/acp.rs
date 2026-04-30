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
use std::{pin::Pin, task::Poll};
use tracing::Instrument;
use uuid::Uuid;

use crate::{
    api::{
        auth::{self, ApiError},
        oauth::OAuthScope,
        service::ApiState,
    },
    core::{acp_tokens, bears::db as bears_db, user},
    errors::CustomError,
};

pub fn router() -> Router<ApiState> {
    Router::new().route("/bears/{slug}/sessions/{session_id}/prompt", post(prompt))
}

#[derive(Debug, Deserialize)]
pub struct AcpPromptRequest {
    pub message: String,
    #[serde(default)]
    pub conversation_id: Option<String>,
    #[serde(default)]
    pub client: Option<String>,
}

#[derive(Debug, Serialize)]
struct AcpErrorResponse {
    error: String,
    error_code: &'static str,
    request_id: String,
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
    let user_id = if acp_tokens::is_acp_token(&token) {
        acp_tokens::authenticate_for_bear_slug(
            &state.sqlx_pool,
            &token,
            &slug,
            OAuthScope::AcpChat.as_str(),
        )
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
        })?
    } else {
        let principal = auth::authenticate_bearer(&headers)?;
        auth::require_scope(&principal, OAuthScope::AcpChat)?;
        principal.user_id
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
    let conversation_id = "default".to_string();
    let client = normalize_acp_client(body.client.as_deref());
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
        .post_bear_channel_message_for_channel_streaming(
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
        )
        .await
    {
        Ok(upstream) => upstream,
        Err(err) => return Ok(Err(err)),
    };

    let stream = AcpBearChannelSseStream::new(upstream.bytes_stream());
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
        _ => return None,
    };
    Some(Bytes::from(format!("data: {}\n\n", mapped)))
}

fn map_bear_channel_frame_to_acp_adapter_events(frame: &[u8]) -> Vec<Bytes> {
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
            if let Some(bytes) = map_bear_channel_event_to_acp_adapter_event(&value) {
                out.push(bytes);
            }
        }
    }
    out
}

struct AcpBearChannelSseStream {
    inner: Pin<Box<dyn Stream<Item = Result<Bytes, reqwest::Error>> + Send>>,
    buffer: Vec<u8>,
    pending: std::collections::VecDeque<Bytes>,
}

impl AcpBearChannelSseStream {
    fn new(inner: impl Stream<Item = Result<Bytes, reqwest::Error>> + Send + 'static) -> Self {
        Self {
            inner: Box::pin(inner),
            buffer: Vec::new(),
            pending: std::collections::VecDeque::new(),
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

        loop {
            match ready!(this.inner.as_mut().poll_next(cx)) {
                Some(Ok(chunk)) => {
                    this.buffer.extend_from_slice(&chunk);
                    while let Some(pos) = this.buffer.windows(2).position(|w| w == b"\n\n") {
                        let frame: Vec<u8> = this.buffer.drain(..pos + 2).collect();
                        for bytes in map_bear_channel_frame_to_acp_adapter_events(&frame) {
                            this.pending.push_back(bytes);
                        }
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
                        for bytes in map_bear_channel_frame_to_acp_adapter_events(&frame) {
                            this.pending.push_back(bytes);
                        }
                        if let Some(bytes) = this.pending.pop_front() {
                            return Poll::Ready(Some(Ok(bytes)));
                        }
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
}
