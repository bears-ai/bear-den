use axum::{
    extract::{Path, Query, State},
    http::HeaderMap,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;
use uuid::Uuid;

use crate::{
    api::{
        acp::{
            check_adapter_contract,
            paths::{is_absolute_local_path, optional_absolute_cwd_filter},
            plan_approval_fallback_payload,
            responses::acp_error_response,
            tool_results::default_unavailable_context_budget,
            ACP_SESSIONS_PAGE_SIZE,
        },
        auth,
        service::ApiState,
    },
    core::{
        acp_plan_mode,
        acp_sessions,
        acp_tokens,
        bears::{db as bears_db, BearAgentRole},
        role_runtime::{RoleRuntime, RoleTurnScope},
        work_plans::{self, WorkPlanLookup},
    },
    errors::CustomError,
};

use super::{
    acp_session_row_to_http_with_modes,
    auth_session::{authenticate_acp_code_token, authenticate_acp_code_token_with_auth},
    decode_acp_sessions_cursor, encode_acp_sessions_cursor, format_acp_session_timestamp,
    resolve_acp_turn_context, tools_enabled_for_client, AcpAdapterEnvironmentRequest,
    AcpSessionsListHttpResponse, AcpSessionsListQuery, AcpSetModeRequest, AcpSetModeResponse,
};

pub(super) async fn list_acp_sessions(
    State(state): State<ApiState>,
    Path(slug): Path<String>,
    Query(query): Query<AcpSessionsListQuery>,
    headers: HeaderMap,
) -> Response {
    let request_id = Uuid::new_v4();
    match list_acp_sessions_inner(state, slug, query, headers).await {
        Ok(response) => response,
        Err(err) => acp_error_response(err, request_id),
    }
}

pub(super) async fn list_acp_sessions_inner(
    state: ApiState,
    slug: String,
    query: AcpSessionsListQuery,
    headers: HeaderMap,
) -> Result<Response, CustomError> {
    let user_id = authenticate_acp_code_token(&state, &headers, &slug).await?;
    let bear = bears_db::bear_for_user_by_slug(&state.sqlx_pool, user_id, slug.trim())
        .await?
        .ok_or_else(|| {
            CustomError::NotFound("bear not found or you do not have access".to_string())
        })?;
    let cursor = decode_acp_sessions_cursor(query.cursor.as_deref())?;
    let cwd_filter = optional_absolute_cwd_filter(query.cwd.as_deref())?;
    let fetch_limit = ACP_SESSIONS_PAGE_SIZE + 1;
    let mut rows = acp_sessions::list_for_user_bear(
        &state.sqlx_pool,
        acp_sessions::SessionListParams {
            user_id,
            bear_slug: &bear.slug,
            include_closed: query.include_closed,
            cwd_filter,
            limit: fetch_limit,
            cursor_updated_at: cursor.as_ref().map(|c| c.updated_at),
            cursor_id: cursor.as_ref().map(|c| c.id),
        },
    )
    .await?;
    let has_more = rows.len() > ACP_SESSIONS_PAGE_SIZE as usize;
    rows.truncate(ACP_SESSIONS_PAGE_SIZE as usize);
    let next_cursor = if has_more {
        rows.last().map(encode_acp_sessions_cursor)
    } else {
        None
    };
    let mut sessions = Vec::new();
    for row in rows {
        if row
            .cwd
            .as_deref()
            .map(str::trim)
            .filter(|s| is_absolute_local_path(s))
            .is_none()
        {
            tracing::warn!(
                acp_session_id = %row.acp_session_id,
                bear_slug = %row.bear_slug,
                "omitting ACP session list row with missing or non-absolute cwd"
            );
            continue;
        }
        let plan_mode = acp_plan_mode::active_for_session(
            &state.sqlx_pool,
            user_id,
            bear.id,
            &row.acp_session_id,
        )
        .await?
        .map(serde_json::to_value)
        .transpose()?;
        sessions.push(acp_session_row_to_http_with_modes(row, plan_mode));
    }
    Ok(Json(AcpSessionsListHttpResponse {
        sessions,
        next_cursor,
    })
    .into_response())
}

pub(super) async fn get_acp_session(
    State(state): State<ApiState>,
    Path((slug, session_id)): Path<(String, String)>,
    headers: HeaderMap,
) -> Response {
    let request_id = Uuid::new_v4();
    match get_acp_session_inner(state, slug, session_id, headers).await {
        Ok(response) => response,
        Err(err) => acp_error_response(err, request_id),
    }
}

pub(super) async fn get_acp_session_runtime(
    State(state): State<ApiState>,
    Path((slug, session_id)): Path<(String, String)>,
    headers: HeaderMap,
) -> Response {
    let request_id = Uuid::new_v4();
    match get_acp_session_runtime_inner(state, slug, session_id, headers).await {
        Ok(response) => response,
        Err(err) => acp_error_response(err, request_id),
    }
}

pub(super) async fn get_acp_session_runtime_inner(
    state: ApiState,
    slug: String,
    session_id: String,
    headers: HeaderMap,
) -> Result<Response, CustomError> {
    let user_id = authenticate_acp_code_token(&state, &headers, &slug).await?;
    let bear = bears_db::bear_for_user_by_slug(&state.sqlx_pool, user_id, slug.trim())
        .await?
        .ok_or_else(|| {
            CustomError::NotFound("bear not found or you do not have access".to_string())
        })?;
    let session_id = session_id.trim();
    if session_id.is_empty() {
        return Err(CustomError::ValidationError(
            "session_id must not be empty".to_string(),
        ));
    }
    let row =
        acp_sessions::find_for_user_bear_session(&state.sqlx_pool, user_id, &bear.slug, session_id)
            .await?
            .ok_or_else(|| CustomError::NotFound("ACP session not found".to_string()))?;
    let plan_mode =
        acp_plan_mode::active_for_session(&state.sqlx_pool, user_id, bear.id, session_id).await?;
    let activity_plan = work_plans::get_visible_work_plan(
        &state.sqlx_pool,
        bear.id,
        BearAgentRole::Pair,
        user_id,
        WorkPlanLookup {
            plan_id: None,
            source_conversation_id: row.resolved_conversation_id.clone().or_else(|| {
                let conversation_id = row.conversation_id.trim();
                conversation_id
                    .starts_with("conv-")
                    .then(|| conversation_id.to_string())
            }),
            source_acp_session_id: Some(session_id.to_string()),
        },
    )
    .await?;
    let turn_context = resolve_acp_turn_context(&row, plan_mode.as_ref(), activity_plan.as_ref());
    let role_scope = RoleTurnScope::acp_pair(
        bear.id,
        session_id.to_string(),
        row.resolved_conversation_id.clone(),
    );
    let role_runtime = RoleRuntime::with_turn_cancellations(
        state.acp_tool_turns.clone(),
        state.acp_turn_cancellations.clone(),
    );
    let runtime = role_runtime.tool_turn_runtime_snapshot(session_id, &state.acp_tool_turns);
    let active_turn = state
        .acp_tool_turns
        .active_turn_for_session(session_id)
        .map(|turn| turn.diagnostic());
    let stream_turn = state
        .acp_turn_cancellations
        .active_for_session(session_id)
        .map(|turn| {
            serde_json::json!({
                "acp_session_id": turn.acp_session_id,
                "request_id": turn.request_id,
                "conversation_id": turn.conversation_id,
                "run_ids": turn.run_ids,
            })
        });
    let pending = state
        .acp_tool_turns
        .pending_for_session(session_id)
        .into_iter()
        .map(|turn| turn.diagnostic())
        .collect::<Vec<_>>();
    let expired = state
        .acp_tool_turns
        .expired_pending_for_session(session_id)
        .into_iter()
        .map(|turn| turn.diagnostic())
        .collect::<Vec<_>>();
    let adapter_environment = if tools_enabled_for_client(&row.client) {
        row.adapter_environment.unwrap_or_else(|| {
            json!({
                "status": "unavailable",
                "note": "ACP adapter has not published an environment snapshot for this session yet.",
            })
        })
    } else {
        serde_json::json!({ "status": "not_applicable" })
    };
    Ok(Json(serde_json::json!({
        "ok": true,
        "bear_id": bear.id,
        "role": "pair",
        "channel_kind": "acp_session",
        "acp_session_id": session_id,
        "title": row.conversation_title,
        "conversation_title_updated_at": row
            .conversation_title_updated_at
            .map(format_acp_session_timestamp),
        "conversation_title_synced_at": row
            .conversation_title_synced_at
            .map(format_acp_session_timestamp),
        "conversation": {
            "session_selection": row.conversation_id,
            "resolved_conversation_id": row.resolved_conversation_id,
            "upstream_target": row.resolved_conversation_id
                .as_deref()
                .or_else(|| row.conversation_id.starts_with("conv-").then_some(row.conversation_id.as_str()))
                .unwrap_or("unresolved"),
        },
        "active_turn": {
            "active": active_turn.is_some(),
            "turn": active_turn,
        },
        "stream_turn": {
            "active": stream_turn.is_some(),
            "turn": stream_turn,
        },
        "pending_tools": pending,
        "expired_tools": expired,
        "tool_turns": role_runtime.pending_diagnostics(&role_scope),
        "runtime": runtime,
        "adapter_environment": adapter_environment,
        "context_budget": default_unavailable_context_budget(),
        "turn_state": turn_context.workflow_state,
        "session_policy": turn_context.policy.to_json(),
        "activity": activity_plan,
        "plan_mode": plan_mode,
    }))
    .into_response())
}

pub(super) async fn set_session_mode(
    State(state): State<ApiState>,
    Path((slug, session_id)): Path<(String, String)>,
    headers: HeaderMap,
    Json(body): Json<AcpSetModeRequest>,
) -> Response {
    let request_id = Uuid::new_v4();
    match set_session_mode_inner(state, slug, session_id, headers, body).await {
        Ok(response) => response,
        Err(err) => acp_error_response(err, request_id),
    }
}

pub(super) async fn post_adapter_environment(
    State(state): State<ApiState>,
    Path((slug, session_id)): Path<(String, String)>,
    headers: HeaderMap,
    Json(body): Json<AcpAdapterEnvironmentRequest>,
) -> Response {
    let request_id = Uuid::new_v4();
    match post_adapter_environment_inner(state, slug, session_id, headers, body).await {
        Ok(response) => response,
        Err(err) => acp_error_response(err, request_id),
    }
}

pub(super) async fn post_adapter_environment_inner(
    state: ApiState,
    slug: String,
    session_id: String,
    headers: HeaderMap,
    body: AcpAdapterEnvironmentRequest,
) -> Result<Response, CustomError> {
    let token = auth::extract_bearer_token(&headers)
        .map_err(|err| CustomError::Authentication(err.message))?;
    let auth = authenticate_acp_code_token_with_auth(&state, &token, &slug).await?;
    if !acp_tokens::scopes_contains(&auth.scopes, acp_tokens::acp_tools_scope()) {
        return Err(CustomError::Authorization(
            "ACP token is missing required acp:tools scope".to_string(),
        ));
    }
    let session = acp_sessions::find_for_user_bear_session(
        &state.sqlx_pool,
        auth.user_id,
        &slug,
        &session_id,
    )
    .await?
    .ok_or_else(|| CustomError::NotFound("ACP session not found".to_string()))?;
    acp_sessions::update_adapter_environment(
        &state.sqlx_pool,
        auth.user_id,
        session.bear_id,
        &session_id,
        &body.environment,
    )
    .await?;
    let client_title = body
        .conversation_title
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .or_else(|| {
            body.environment
                .get("thread_title")
                .or_else(|| body.environment.get("conversation_title"))
                .or_else(|| body.environment.get("title"))
                .and_then(|value| value.as_str())
                .map(str::trim)
                .filter(|value| !value.is_empty())
        });
    if client_title.is_some() {
        acp_sessions::update_client_conversation_title(
            &state.sqlx_pool,
            auth.user_id,
            session.bear_id,
            &session_id,
            client_title,
        )
        .await?;
    }
    Ok(Json(serde_json::json!({
        "accepted": true,
        "reason": "stored",
    }))
    .into_response())
}

pub(super) async fn set_session_mode_inner(
    state: ApiState,
    slug: String,
    session_id: String,
    headers: HeaderMap,
    body: AcpSetModeRequest,
) -> Result<Response, CustomError> {
    if let Err(err) = check_adapter_contract(body.adapter_contract.as_ref()) {
        return Ok(super::compat::acp_compatibility_error_response(err, Uuid::new_v4()));
    }
    let user_id = authenticate_acp_code_token(&state, &headers, &slug).await?;
    let bear = bears_db::bear_for_user_by_slug(&state.sqlx_pool, user_id, slug.trim())
        .await?
        .ok_or_else(|| {
            CustomError::NotFound("bear not found or you do not have access".to_string())
        })?;
    let session_id = session_id.trim();
    if session_id.is_empty() {
        return Err(CustomError::ValidationError(
            "session_id must not be empty".to_string(),
        ));
    }
    let requested_mode = body.mode.trim().to_ascii_lowercase();
    if !matches!(requested_mode.as_str(), "ask" | "plan" | "write") {
        return Err(CustomError::ValidationError(
            "mode must be one of ask, plan, write".to_string(),
        ));
    }
    let existing =
        acp_sessions::find_for_user_bear_session(&state.sqlx_pool, user_id, &bear.slug, session_id)
            .await?;
    let Some(_existing) = existing else {
        return Err(CustomError::NotFound("ACP session not found".to_string()));
    };

    let reason = body
        .reason
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("User selected ACP session mode");

    let effective_mode;
    let message;
    match requested_mode.as_str() {
        "plan" => {
            acp_plan_mode::enter_plan_mode(
                &state.sqlx_pool,
                acp_plan_mode::EnterPlanModeParams {
                    user_id,
                    bear_id: bear.id,
                    bear_slug: bear.slug.clone(),
                    acp_session_id: session_id.to_string(),
                    reason: reason.to_string(),
                    requested_by: acp_plan_mode::AcpPlanModeRequestedBy::User,
                    previous_permission_mode: Some("ask".to_string()),
                },
            )
            .await?;
            acp_sessions::set_current_mode(&state.sqlx_pool, user_id, bear.id, session_id, "plan")
                .await?;
            effective_mode = "plan".to_string();
            message = "Plan mode entered. Planning is active; concrete tool use remains governed by Den policy and ACP client approval.".to_string();
        }
        "ask" => {
            if let Some(active) =
                acp_plan_mode::active_for_session(&state.sqlx_pool, user_id, bear.id, session_id)
                    .await?
            {
                acp_plan_mode::cancel_plan_mode(
                    &state.sqlx_pool,
                    user_id,
                    bear.id,
                    session_id,
                    Some(active.id),
                )
                .await?;
                message = "Plan mode cancelled; returned to Ask.".to_string();
            } else {
                message = "Returned to Ask according to Den session policy.".to_string();
            }
            acp_sessions::set_current_mode(&state.sqlx_pool, user_id, bear.id, session_id, "ask")
                .await?;
            effective_mode = "ask".to_string();
        }
        "write" => {
            let active_plan =
                acp_plan_mode::active_for_session(&state.sqlx_pool, user_id, bear.id, session_id)
                    .await?;
            if let Some(active) = active_plan.as_ref() {
                match active.state.as_str() {
                    "submitted" => {
                        acp_plan_mode::approve_plan_mode(
                            &state.sqlx_pool,
                            user_id,
                            bear.id,
                            session_id,
                            active.id,
                        )
                        .await?;
                        message = "Write mode enabled by user request; the submitted plan was approved by the authenticated ACP human.".to_string();
                    }
                    "active" => {
                        acp_plan_mode::cancel_plan_mode(
                            &state.sqlx_pool,
                            user_id,
                            bear.id,
                            session_id,
                            Some(active.id),
                        )
                        .await?;
                        message = "Write mode enabled by user request; the unsubmitted plan draft was closed so the mode change could take effect.".to_string();
                    }
                    _ => {
                        message = "Write mode enabled by user request. Concrete tool use remains subject to Den policy and ACP client approval.".to_string();
                    }
                }
            } else {
                message = "Write mode enabled by user request. Concrete tool use remains subject to Den policy and ACP client approval.".to_string();
            }
            acp_sessions::set_current_mode(&state.sqlx_pool, user_id, bear.id, session_id, "write")
                .await?;
            effective_mode = "write".to_string();
            tracing::info!(
                bear_id = %bear.id,
                acp_session_id = %session_id,
                requested_mode = %requested_mode,
                effective_mode = %effective_mode,
                active_plan_id = ?active_plan.as_ref().map(|plan| plan.id),
                active_plan_state = ?active_plan.as_ref().map(|plan| plan.state.as_str()),
                "ACP session mode changed to write by authenticated user request"
            );
        }
        _ => unreachable!(),
    }

    let plan_mode_row =
        acp_plan_mode::active_for_session(&state.sqlx_pool, user_id, bear.id, session_id).await?;
    let plan_mode = plan_mode_row
        .clone()
        .map(serde_json::to_value)
        .transpose()?;
    let synthetic_row = acp_sessions::AcpSessionRow {
        current_mode: effective_mode.clone(),
        ..acp_sessions::find_for_user_bear_session(
            &state.sqlx_pool,
            user_id,
            &bear.slug,
            session_id,
        )
        .await?
        .ok_or_else(|| CustomError::NotFound("ACP session not found".to_string()))?
    };
    let turn_context = resolve_acp_turn_context(&synthetic_row, plan_mode_row.as_ref(), None);
    Ok(Json(AcpSetModeResponse {
        requested_mode,
        effective_mode: turn_context.effective_mode,
        session_policy: turn_context.policy.to_json(),
        workflow_state: turn_context.workflow_state,
        plan_mode,
        message,
    })
    .into_response())
}

pub(super) async fn get_acp_session_inner(
    state: ApiState,
    slug: String,
    session_id: String,
    headers: HeaderMap,
) -> Result<Response, CustomError> {
    let user_id = authenticate_acp_code_token(&state, &headers, &slug).await?;
    let bear = bears_db::bear_for_user_by_slug(&state.sqlx_pool, user_id, slug.trim())
        .await?
        .ok_or_else(|| {
            CustomError::NotFound("bear not found or you do not have access".to_string())
        })?;
    let session_id = session_id.trim();
    if session_id.is_empty() {
        return Err(CustomError::ValidationError(
            "session_id must not be empty".to_string(),
        ));
    }
    let row =
        acp_sessions::find_for_user_bear_session(&state.sqlx_pool, user_id, &bear.slug, session_id)
            .await?
            .ok_or_else(|| CustomError::NotFound("ACP session not found".to_string()))?;
    let plan_mode =
        acp_plan_mode::active_for_session(&state.sqlx_pool, user_id, bear.id, session_id).await?;
    let approval_fallback = plan_mode
        .as_ref()
        .filter(|plan| plan.state == "submitted")
        .map(plan_approval_fallback_payload);
    let mut response = serde_json::to_value(acp_session_row_to_http_with_modes(
        row,
        plan_mode.map(serde_json::to_value).transpose()?,
    ))?;
    if let Some(approval_fallback) = approval_fallback {
        response["approval_fallback"] = approval_fallback;
    }
    Ok(Json(response).into_response())
}
