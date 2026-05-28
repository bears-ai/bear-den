use axum::{
    extract::{Path, State},
    http::HeaderMap,
    response::{IntoResponse, Response},
    Json,
};
use uuid::Uuid;

use crate::{
    api::{
        acp::{
            compat::{
                acp_compatibility_error_response, adapter_contract_from_value,
                check_adapter_contract,
            },
            responses::acp_error_response,
        },
        service::ApiState,
    },
    core::{
        acp_plan_mode, acp_sessions,
        bears::{db as bears_db, BearAgentRole},
        archived_conversations,
    },
    errors::CustomError,
};

use crate::api::acp::{
    acp_archive_target_for_session, cancel_runtime_runs_by_id_or_skip,
    resolve_acp_turn_context, run_pair_reflection_summary, AcpCloseSessionResponse,
};

use super::auth::authenticate_acp_code_token;

pub(in crate::api::acp) async fn compact_session(
    State(state): State<ApiState>,
    Path((slug, session_id)): Path<(String, String)>,
    headers: HeaderMap,
    Json(body): Json<serde_json::Value>,
) -> Response {
    let response_request_id = Uuid::new_v4();
    let contract = adapter_contract_from_value(&body);
    if let Err(err) = check_adapter_contract(contract.as_ref()) {
        return acp_compatibility_error_response(err, response_request_id);
    }
    match compact_session_inner(state, slug, session_id, headers).await {
        Ok(response) => response,
        Err(err) => acp_error_response(err, response_request_id),
    }
}

pub(super) async fn compact_session_inner(
    state: ApiState,
    slug: String,
    session_id: String,
    headers: HeaderMap,
) -> Result<Response, CustomError> {
    let user_id = authenticate_acp_code_token(&state, &headers, &slug).await?;
    let Some(session) =
        acp_sessions::find_for_user_bear_session(&state.sqlx_pool, user_id, &slug, &session_id)
            .await?
    else {
        return Err(CustomError::NotFound("ACP session not found".to_string()));
    };
    let conversation_id = session
        .resolved_conversation_id
        .as_deref()
        .or_else(|| {
            let selection = session.conversation_id.trim();
            selection.starts_with("conv-").then_some(selection)
        })
        .ok_or_else(|| {
            CustomError::ValidationError(
                "ACP session has no resolved runtime conversation to compact".to_string(),
            )
        })?;
    let compact_result = state.letta.compact_conversation(conversation_id).await?;
    tracing::warn!(
        acp_session_id = %session_id,
        bear_id = %session.bear_id,
        conversation_id,
        "ACP session compact requested; no stale approval recovery attempted because compaction does not resolve pending runtime approvals"
    );
    Ok(Json(serde_json::json!({
        "ok": true,
        "compacted": true,
        "acp_session_id": session_id,
        "conversation_id": conversation_id,
        "approval_recovery": {
            "attempted": false,
            "reason": "compaction_only"
        },
        "compact_result": compact_result,
    }))
    .into_response())
}

pub(in crate::api::acp) async fn close_session(
    State(state): State<ApiState>,
    Path((slug, session_id)): Path<(String, String)>,
    headers: HeaderMap,
    body: Option<Json<serde_json::Value>>,
) -> Response {
    let response_request_id = Uuid::new_v4();
    let contract = body
        .as_ref()
        .and_then(|Json(value)| adapter_contract_from_value(value));
    if let Err(err) = check_adapter_contract(contract.as_ref()) {
        return acp_compatibility_error_response(err, response_request_id);
    }
    match close_session_inner(state, slug, session_id, headers).await {
        Ok(response) => response,
        Err(err) => acp_error_response(err, response_request_id),
    }
}

pub(in crate::api::acp) async fn cancel_session(
    State(state): State<ApiState>,
    Path((slug, session_id)): Path<(String, String)>,
    headers: HeaderMap,
    body: Option<Json<serde_json::Value>>,
) -> Response {
    let response_request_id = Uuid::new_v4();
    let contract = body
        .as_ref()
        .and_then(|Json(value)| adapter_contract_from_value(value));
    if let Err(err) = check_adapter_contract(contract.as_ref()) {
        return acp_compatibility_error_response(err, response_request_id);
    }
    match cancel_session_inner(state, slug, session_id, headers).await {
        Ok(response) => response,
        Err(err) => acp_error_response(err, response_request_id),
    }
}

pub(super) async fn cancel_session_inner(
    state: ApiState,
    slug: String,
    session_id: String,
    headers: HeaderMap,
) -> Result<Response, CustomError> {
    let user_id = authenticate_acp_code_token(&state, &headers, &slug).await?;
    let Some(session) =
        acp_sessions::find_for_user_bear_session(&state.sqlx_pool, user_id, &slug, &session_id)
            .await?
    else {
        return Ok(Json(serde_json::json!({
            "ok": true,
            "cancelled": false,
            "stream_turn": serde_json::Value::Null,
        }))
        .into_response());
    };
    let stream_cancel = state
        .acp_turn_cancellations
        .cancel_session(&session.acp_session_id);
    let active = state
        .acp_tool_turns
        .cancel_active_turn(&session.acp_session_id);
    let pair_agent_id =
        bears_db::role_agent_id(&state.sqlx_pool, session.bear_id, BearAgentRole::Pair)
            .await?
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
    let run_ids = stream_cancel
        .as_ref()
        .map(|turn| turn.run_ids.clone())
        .unwrap_or_default();
    if stream_cancel.is_some() && run_ids.is_empty() {
        tracing::warn!(
            acp_session_id = %session.acp_session_id,
            bear_id = %session.bear_id,
            conversation_id = %session.conversation_id,
            active_request_id = ?stream_cancel.as_ref().map(|turn| turn.request_id),
            active_conversation_id = ?stream_cancel.as_ref().and_then(|turn| turn.conversation_id.clone()),
            pair_agent_id = ?pair_agent_id,
            "ACP cancel found an active stream but no runtime run_ids; skipped upstream cancel to avoid agent-wide cancellation"
        );
    }
    let cancel_result = if let Some(agent_id) = pair_agent_id.as_deref() {
        cancel_runtime_runs_by_id_or_skip(
            state.letta.as_ref(),
            agent_id,
            &run_ids,
            "explicit_acp_session_cancel",
        )
        .await
    } else {
        serde_json::json!({ "ok": false, "error": "pair role agent id is missing" })
    };
    state
        .acp_tool_turns
        .cleanup_session(&session.acp_session_id);
    tracing::info!(
        bear_id = %session.bear_id,
        acp_session_id = %session.acp_session_id,
        conversation_id = %session.conversation_id,
        active_request_id = ?active.as_ref().map(|turn| turn.request_id).or_else(|| stream_cancel.as_ref().map(|turn| turn.request_id)),
        active_conversation_id = ?active.as_ref().and_then(|turn| turn.conversation_id.clone()).or_else(|| stream_cancel.as_ref().and_then(|turn| turn.conversation_id.clone())),
        pair_agent_id = ?pair_agent_id,
        run_ids = ?run_ids,
        cancel_result = %cancel_result,
        "ACP cancel requested; cancelled active pair turn and cleaned session tool state"
    );
    Ok(Json(serde_json::json!({
        "ok": true,
        "cancelled": active.is_some() || stream_cancel.is_some(),
        "active_turn": active.map(|turn| turn.diagnostic()),
        "stream_turn": stream_cancel.map(|turn| serde_json::json!({
            "acp_session_id": turn.acp_session_id,
            "request_id": turn.request_id,
            "conversation_id": turn.conversation_id,
            "run_ids": turn.run_ids,
        })),
        "cancel_result": cancel_result,
    }))
    .into_response())
}

pub(super) async fn close_session_inner(
    state: ApiState,
    slug: String,
    session_id: String,
    headers: HeaderMap,
) -> Result<Response, CustomError> {
    let user_id = authenticate_acp_code_token(&state, &headers, &slug).await?;

    let Some(session) =
        acp_sessions::find_for_user_bear_session(&state.sqlx_pool, user_id, &slug, &session_id)
            .await?
    else {
        return Ok(Json(AcpCloseSessionResponse {
            ok: true,
            archived: false,
            conversation_id: None,
            unwedged: None,
            workflow_state: None,
        })
        .into_response());
    };

    tracing::info!(
        bear_id = %session.bear_id,
        acp_session_id = %session.acp_session_id,
        conversation_id = %session.conversation_id,
        "ACP close requested; marking API-direct pair session closed"
    );
    acp_sessions::mark_closed(&state.sqlx_pool, session.id).await?;
    if let Err(err) = run_pair_reflection_summary(&state, &session, "session_close").await {
        tracing::warn!(
            bear_id = %session.bear_id,
            acp_session_id = %session.acp_session_id,
            error = %err,
            "Pair reflection summary failed during ACP close"
        );
    }
    let archive_target = acp_archive_target_for_session(&session);
    let mut archived = false;
    if let Some(archive_target) = archive_target.filter(|_| state.letta.is_enabled()) {
        state
            .letta
            .patch_conversation_archived(archive_target, true)
            .await?;
        archived_conversations::set_archived(
            &state.sqlx_pool,
            session.bear_id,
            archive_target,
            Some(user_id),
            "acp",
            true,
        )
        .await?;
        acp_sessions::mark_archived(&state.sqlx_pool, session.id).await?;
        archived = true;
    }

    let active_plan_mode = acp_plan_mode::active_for_session(
        &state.sqlx_pool,
        user_id,
        session.bear_id,
        &session.acp_session_id,
    )
    .await?;
    let turn_context = resolve_acp_turn_context(&session, active_plan_mode.as_ref(), None);
    Ok(Json(AcpCloseSessionResponse {
        ok: true,
        archived,
        conversation_id: archive_target.map(str::to_string),
        unwedged: None,
        workflow_state: Some(turn_context.workflow_state),
    })
    .into_response())
}
