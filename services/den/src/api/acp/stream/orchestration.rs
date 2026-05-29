use axum::{
    body::Body,
    http::{header, HeaderName, HeaderValue, StatusCode},
    response::Response,
};
use uuid::Uuid;

use crate::{
    api::{
        acp::{
            acp_error_status_message, acp_stream_tokens_enabled,
            history::pending_session_title_update_event, AcpGatewayEvent, AcpStreamContext,
        },
        auth::ApiError,
        service::ApiState,
    },
    core::{
        acp_runtime::AcpConversationResolution,
        acp_tools::AcpResolvedSessionPolicy,
        bears::Bear,
        letta::RuntimeContinuationContext,
        role_runtime::{AcpTurnLifecycleContext, AcpTurnLifecycleRuntime},
        runtime_provider::RoleRuntimeBinding,
        user,
    },
    errors::CustomError,
};

use super::sse_stream::AcpRuntimeSseStream;

pub(in crate::api::acp) struct AcpStreamSetup {
    pub(in crate::api::acp) initial_events: Vec<AcpGatewayEvent>,
    pub(in crate::api::acp) session_info_event_sent: bool,
    pub(in crate::api::acp) workspace_roots: Vec<String>,
    pub(in crate::api::acp) stream_tokens: bool,
    pub(in crate::api::acp) turn_runtime_context: String,
}

pub(in crate::api::acp) async fn build_acp_stream_setup(
    state: &ApiState,
    user_id: i32,
    bear: &Bear,
    session_id: &str,
    cwd: &str,
    client_context: &serde_json::Value,
    conversation_resolution: &AcpConversationResolution,
    current_activity_plan: &Option<crate::core::work_plans::WorkPlanProjection>,
    plan_mode_context: &str,
    activity_context: &str,
    tool_prompt_context: &str,
    prompt: &str,
    request_id: Uuid,
) -> Result<AcpStreamSetup, ApiError> {
    let mut initial_events = Vec::new();
    let mut session_info_event_sent = false;
    if let Some(conversation) = conversation_resolution.resolved_conversation.clone() {
        initial_events.push(AcpGatewayEvent::ConversationResolved {
            conversation_id: conversation.id,
        });
    }
    if let Some(title_event) = pending_session_title_update_event(
        &state.sqlx_pool,
        user_id,
        bear.id,
        &bear.slug,
        session_id,
    )
    .await
    .map_err(|err| {
        let (status, code, message) = acp_error_status_message(&err);
        ApiError::new(status, code, message)
    })? {
        session_info_event_sent = true;
        initial_events.push(title_event);
    }
    if let Some(plan_event) = current_activity_plan.clone().map(AcpGatewayEvent::PlanUpdate) {
        initial_events.push(plan_event);
    }
    let turn_runtime_context =
        format!("{plan_mode_context}{activity_context}{tool_prompt_context}");
    tracing::info!(
        %request_id,
        acp_session_id = %session_id,
        upstream_user_prompt_len = prompt.len(),
        turn_runtime_context_len = turn_runtime_context.len(),
        turn_runtime_context_has_trusted_mode_suffix =
            turn_runtime_context.contains("Trusted ACP session mode this turn:"),
        turn_runtime_context_has_system_reminder =
            turn_runtime_context.contains("<system-reminder>"),
        runtime_context_sent_as_user_content = false,
        "ACP final upstream prompt assembly"
    );
    let workspace_roots = client_context
        .get("workspace_roots")
        .or_else(|| client_context.get("workspaceRoots"))
        .and_then(|v| v.as_array())
        .map(|items| {
            items
                .iter()
                .filter_map(|v| v.as_str())
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .filter(|items| !items.is_empty())
        .unwrap_or_else(|| vec![cwd.to_string()]);
    let stream_tokens = acp_stream_tokens_enabled();

    Ok(AcpStreamSetup {
        initial_events,
        session_info_event_sent,
        workspace_roots,
        stream_tokens,
        turn_runtime_context,
    })
}

pub(in crate::api::acp) async fn build_acp_sse_response(
    state: ApiState,
    user_id: i32,
    request_id: Uuid,
    session_id: &str,
    bear: &Bear,
    client: &str,
    prompt: &str,
    pair_runtime_binding: &RoleRuntimeBinding,
    conversation_resolution: &AcpConversationResolution,
    resolved_policy: &AcpResolvedSessionPolicy,
    current_activity_plan: &Option<crate::core::work_plans::WorkPlanProjection>,
    merged_client_tool_descriptors: Option<serde_json::Value>,
    setup: AcpStreamSetup,
) -> Result<Result<Response, CustomError>, ApiError> {
    let client_tool_descriptors = merged_client_tool_descriptors.clone();
    let turn_lifecycle = AcpTurnLifecycleRuntime::new(
        state.acp_tool_turns.clone(),
        state.acp_turn_cancellations.clone(),
    );
    let lifecycle_lease = match turn_lifecycle.acquire_pair_turn(
        AcpTurnLifecycleContext {
            bear_id: bear.id,
            acp_session_id: session_id.to_string(),
            resolved_conversation_id: conversation_resolution
                .resolved_conversation
                .as_ref()
                .map(|conversation| conversation.id.clone()),
        },
        request_id,
    ) {
        Ok(lease) => lease,
        Err(err) => return Ok(Err(err)),
    };
    let role_runtime = lifecycle_lease.role_runtime.clone();
    let turn_scope = lifecycle_lease.turn_scope.clone();
    let active_turn_guard = lifecycle_lease.active_turn_guard;
    let cancel_handle = lifecycle_lease.cancel_handle;
    let cancel_rx = lifecycle_lease.cancel_rx;

    let upstream = match crate::core::acp_turn_runner::start_acp_turn_stream_with_retries(
        crate::core::acp_turn_runner::AcpTurnStartRequest {
            state: &state,
            request_id,
            session_id,
            bear_id: bear.id,
            binding: pair_runtime_binding,
            upstream_target: &conversation_resolution.upstream_target,
            prompt,
            client_tools: client_tool_descriptors.clone(),
            runtime_context_len: setup.turn_runtime_context.len(),
            stream_tokens: setup.stream_tokens,
        },
    )
    .await
    {
        Ok(upstream) => upstream,
        Err(err) => return Ok(Err(err)),
    };

    let session_policy = resolved_policy.to_json();
    let activity = current_activity_plan.as_ref().map(|plan| serde_json::json!(plan));
    let stream = AcpRuntimeSseStream::new(
        upstream,
        AcpStreamContext {
            pool: state.sqlx_pool.clone(),
            tool_turns: state.acp_tool_turns.clone(),
            user_id,
            user_profile: user::user_by_id(&state.sqlx_pool, user_id).await.ok(),
            bear_id: bear.id,
            bear_slug: bear.slug.clone(),
            acp_session_id: session_id.to_string(),
            client: client.to_string(),
            conversation_selection: conversation_resolution.session_selection.clone(),
            resolved_conversation_id: conversation_resolution
                .resolved_conversation
                .as_ref()
                .map(|conversation| conversation.id.clone()),
            upstream_target: conversation_resolution.upstream_target.clone(),
            workspace_roots: setup.workspace_roots.clone(),
            session_policy: Some(session_policy),
            activity,
            request_id,
            pair_agent_id: pair_runtime_binding.binding_id.clone(),
            config: state.config.clone(),
            role_runtime,
            turn_scope,
        },
        setup.initial_events,
        setup.session_info_event_sent,
        state.letta.clone(),
        RuntimeContinuationContext {
            conversation_id: conversation_resolution.upstream_target.clone(),
            agent_id: Some(pair_runtime_binding.binding_id.clone()),
            client_tools: client_tool_descriptors,
            stream_tokens: setup.stream_tokens,
            max_steps: 4,
        },
        active_turn_guard,
    )
    .with_cancel_registration(cancel_handle, cancel_rx);
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
