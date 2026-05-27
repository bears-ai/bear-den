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
            compat::acp_compatibility_error_response, responses::acp_error_response,
            AcpPermissionDecisionRequest, AcpPermissionDecisionResponse,
        },
        auth,
        service::ApiState,
    },
    core::{
        acp_plan_mode, acp_sessions, acp_tokens,
        acp_tool_turns::{AcpToolResultRequest, AcpToolTurnRegistration},
        acp_tools::acp_tool_policy_json_for_provider,
        den_tools, web_policy,
    },
    errors::CustomError,
};

use crate::api::acp::{
    check_adapter_contract, invoke_acp_den_tool, pending_web_fetch_approvals,
    plan_approval_fallback_payload, workflow_state_json, workflow_state_json_from_sources,
};

use super::auth::authenticate_acp_code_token_with_auth;

pub(in crate::api::acp) async fn permission_result(
    State(state): State<ApiState>,
    Path((slug, session_id, permission_id)): Path<(String, String, String)>,
    headers: HeaderMap,
    Json(body): Json<AcpPermissionDecisionRequest>,
) -> Response {
    let request_id = Uuid::new_v4();
    if let Err(err) = check_adapter_contract(body.adapter_contract.as_ref()) {
        return acp_compatibility_error_response(err, request_id);
    }
    match permission_result_inner(state, slug, session_id, permission_id, headers, body).await {
        Ok(response) => response,
        Err(err) => acp_error_response(err, request_id),
    }
}

pub(super) async fn permission_result_inner(
    state: ApiState,
    slug: String,
    session_id: String,
    permission_id: String,
    headers: HeaderMap,
    body: AcpPermissionDecisionRequest,
) -> Result<Response, CustomError> {
    let token = auth::extract_bearer_token(&headers)
        .map_err(|err| CustomError::Authentication(err.message))?;
    let auth = authenticate_acp_code_token_with_auth(&state, &token, &slug).await?;
    if !acp_tokens::scopes_contains(&auth.scopes, acp_tokens::acp_tools_scope()) {
        return Err(CustomError::Authorization(
            "ACP token is missing required acp:tools scope".to_string(),
        ));
    }
    if let Some(plan_mode_id) = body.plan_mode_id.or_else(|| {
        permission_id
            .strip_prefix("plan-mode-")
            .and_then(|raw| Uuid::parse_str(raw).ok())
    }) {
        let session = acp_sessions::find_for_user_bear_session(
            &state.sqlx_pool,
            auth.user_id,
            &slug,
            &session_id,
        )
        .await?
        .ok_or_else(|| CustomError::NotFound("ACP session not found".to_string()))?;
        let decision = body.decision.trim().to_ascii_lowercase();
        if decision == "timeout" {
            acp_sessions::set_current_mode(
                &state.sqlx_pool,
                auth.user_id,
                session.bear_id,
                &session_id,
                "plan",
            )
            .await?;
            let row = acp_plan_mode::get_by_id_for_bear(
                &state.sqlx_pool,
                auth.user_id,
                session.bear_id,
                plan_mode_id,
            )
            .await?;
            let policy = crate::core::acp_tools::resolve_session_policy_for_mode("plan", Some("submitted"));
            return Ok(Json(serde_json::json!({
                "accepted": true,
                "reason": "plan_mode_approval_request_timed_out",
                "local_tool_request": serde_json::Value::Null,
                "effective_mode": "plan",
                "session_policy": policy.to_json(),
                "plan_mode": row,
                "workflow_state": row.as_ref().map(|plan| workflow_state_json_from_sources(&policy, Some(plan), None)).unwrap_or_else(|| workflow_state_json(&policy)),
                "approval_fallback": row.as_ref().filter(|plan| plan.state == "submitted").map(plan_approval_fallback_payload),
                "message": "The transient ACP approval request timed out, but the submitted plan remains pending. The user may approve it through Den UI, ACP client mode controls, or a new ACP approval request."
            }))
            .into_response());
        }
        let row = if matches!(
            decision.as_str(),
            "approve" | "approved" | "allow" | "allow_once"
        ) {
            acp_plan_mode::approve_plan_mode(
                &state.sqlx_pool,
                auth.user_id,
                session.bear_id,
                &session_id,
                plan_mode_id,
            )
            .await?
        } else {
            acp_plan_mode::reject_plan_mode(
                &state.sqlx_pool,
                auth.user_id,
                session.bear_id,
                &session_id,
                plan_mode_id,
            )
            .await?
        };
        let effective_mode = if row.state == "approved" {
            acp_sessions::set_current_mode(
                &state.sqlx_pool,
                auth.user_id,
                session.bear_id,
                &session_id,
                "write",
            )
            .await?;
            "write"
        } else {
            acp_sessions::set_current_mode(
                &state.sqlx_pool,
                auth.user_id,
                session.bear_id,
                &session_id,
                "plan",
            )
            .await?;
            "plan"
        };
        let policy = crate::core::acp_tools::resolve_session_policy_for_mode(effective_mode, Some(row.state.as_str()));
        return Ok(Json(serde_json::json!({
            "accepted": true,
            "reason": format!("plan_mode_{}", row.state),
            "local_tool_request": serde_json::Value::Null,
            "effective_mode": effective_mode,
            "session_policy": policy.to_json(),
            "plan_mode": row,
            "workflow_state": workflow_state_json_from_sources(&policy, Some(&row), None),
            "approval_fallback": if row.state == "submitted" { Some(plan_approval_fallback_payload(&row)) } else { None },
        }))
        .into_response());
    }

    let pending = pending_web_fetch_approvals()
        .lock()
        .await
        .remove(&permission_id)
        .ok_or_else(|| CustomError::NotFound("pending permission request not found".to_string()))?;
    if pending.user_id != auth.user_id
        || pending.context.bear_slug != slug
        || pending.context.acp_session_id != session_id
    {
        return Err(CustomError::Authorization(
            "permission request does not belong to this ACP session".to_string(),
        ));
    }
    let decision = body.decision.as_str();
    if matches!(decision, "allow_url" | "allow_host") {
        let scope_kind = if decision == "allow_url" {
            "url"
        } else {
            "host"
        };
        let scope_value = if scope_kind == "url" {
            pending.normalized_url.url.clone()
        } else {
            pending.normalized_url.host.clone()
        };
        web_policy::record_web_approval(
            &state.sqlx_pool,
            pending.bear_id,
            scope_kind,
            &scope_value,
            Some(auth.user_id),
            "acp",
            None,
        )
        .await?;
    }
    if matches!(decision, "allow_once" | "allow_url" | "allow_host")
        && web_policy::is_local_web_url(&pending.normalized_url)
    {
        pending
            .context
            .tool_turns
            .register(AcpToolTurnRegistration {
                user_id: pending.user_id,
                bear_id: pending.bear_id,
                bear_slug: pending.context.bear_slug.clone(),
                acp_session_id: pending.context.acp_session_id.clone(),
                request_id: pending.context.request_id,
                tool_call_id: pending.tool_call_id.clone(),
                tool_name: pending.provider_name.clone(),
                approval_request_id: pending.approval_request_id.clone(),
                timeout_ms: acp_tool_policy_json_for_provider(&pending.provider_name)
                    .get("tool_timeout_ms")
                    .and_then(serde_json::Value::as_u64)
                    .unwrap_or(30_000),
                result_tx: pending.result_tx,
            })?;
        return Ok(Json(AcpPermissionDecisionResponse {
            accepted: true,
            reason: "local_tool_required".to_string(),
            local_tool_request: Some(serde_json::json!({
                "tool_call_id": pending.tool_call_id,
                "tool_name": "local_web_fetch",
                "result_tool_name": pending.provider_name,
                "args": { "url": pending.normalized_url.url },
                "policy": { "max_bytes": 262144, "total_timeout_ms": 120000 }
            })),
        })
        .into_response());
    }
    let result = if matches!(decision, "allow_once" | "allow_url" | "allow_host") {
        invoke_acp_den_tool(
            &pending.context,
            den_tools::DEN_WEB_FETCH,
            &pending.provider_name,
            &pending.tool_call_id,
            pending.approval_request_id.as_deref(),
            pending.args,
        )
        .await
    } else {
        AcpToolResultRequest {
            turn_id: None,
            request_id: Some(pending.context.request_id.to_string()),
            tool_call_id: Some(pending.tool_call_id.clone()),
            tool_name: Some(pending.provider_name.clone()),
            approval_request_id: pending.approval_request_id.clone(),
            status: "permission_denied".to_string(),
            content: Some(if decision == "timeout" {
                "web_fetch permission timed out".to_string()
            } else {
                "web_fetch permission denied".to_string()
            }),
            structured_content: serde_json::json!({}),
            diagnostic: serde_json::json!({ "component": "den.acp", "phase": "web_fetch_permission_denied" }),
            ..Default::default()
        }
    };
    let _ = pending.result_tx.send(result);
    Ok(Json(AcpPermissionDecisionResponse {
        accepted: true,
        reason: "delivered".to_string(),
        local_tool_request: None,
    })
    .into_response())
}
