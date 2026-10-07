use axum::http::HeaderMap;
use den_core::{
    ids::{BearId, HatId, UserId},
    DenError,
};

use serde_json::{json, Value};
use sqlx::PgPool;

use bearwire_protocol::{
    methods::{
        RunStartRequest, SessionCurrentTaskClearRequest, SessionCurrentTaskSelectionRequest,
        SessionCurrentTaskStartRequest, SessionExecutionDiagnosticsRequest, SessionHatListRequest,
        SessionHatSelectRequest, SessionHatWorkspaceToolCheckRequest, SessionIdRequest,
        SessionModelSetRequest, SessionOpenRequest, SessionStateRequest,
    },
    wire::BearWireEvent,
};
use den_http::errors::CustomError;
use den_runtime::{
    bearwire_events,
    conversation_review::{
        ConversationReview, ConversationReviewFinding, ConversationReviewFindingDetail,
        ConversationReviewTrigger, FindingSource,
    },
    current_task::{preview_session_current_task_selection, select_session_current_task},
    pair_reflection::create_pair_reflection_proposals_from_latest_summary,
    runtime::compaction::{
        prepare_turn_compaction, CompactionSource, TurnCompactionState, TurnCompactionTrigger,
    },
    runtime::task_context::{resolve_runtime_task_context, RuntimeTaskResolveRequest},
    turn_ids::ClientSessionId,
};
use den_service::{
    bears::{
        db as bears_db,
        hats::{
            self,
            access::{HatAccessGrant, ReadOnlyWorkspaceAction, WorkspaceRoot},
        },
    },
    client_sessions, DenState,
};

use crate::auth::{authenticate_for_bear_slug, authenticated_bear};
use crate::methods::{
    conversation::{authorize_existing_conversation, conversation_viewer},
    parse_params, DEFAULT_CLIENT,
};

mod access;
mod admission;
mod model;
mod open;
mod projection;
mod reflection;
mod tasks;

pub(crate) use model::{session_model_get_result, session_model_set_result};
pub(crate) use open::session_open_result;
use projection::session_state_payload;
#[cfg(test)]
use projection::{active_activity_plan_projection, session_current_task_projection};
pub use reflection::reflect_open_sessions_once;
use reflection::reflect_pair_session;
pub(crate) use tasks::{
    session_current_task_clear_result, session_current_task_select_result,
    session_current_task_selection_request_result, session_current_task_start_result,
    start_session_task_execution,
};

/// Client session IDs are used as unscoped keys by Work bindings and turn runs.
/// A scoped client_sessions lookup is not enough to authorize those operations.
pub(super) async fn require_exclusive_client_session_id(
    pool: &PgPool,
    session_id: &ClientSessionId,
    user_id: UserId,
    bear_id: BearId,
) -> Result<(), CustomError> {
    let rows = sqlx::query!(
        r#"
        SELECT user_id, bear_id
        FROM client_sessions
        WHERE client_session_id = $1
        LIMIT 2
        "#,
        session_id.as_str(),
    )
    .fetch_all(pool)
    .await?;
    match rows.as_slice() {
        [] => Ok(()),
        [row] if row.user_id == user_id.get() && row.bear_id == bear_id.as_uuid() => Ok(()),
        _ => Err(CustomError::NotFound(
            "BearWire session not found".to_string(),
        )),
    }
}

pub(super) fn interactive_session_policy() -> den_core::EffectivePolicy {
    den_core::EffectivePolicy::compile_for_origin(
        den_core::TurnExecutionOrigin::ArmatureConversation(
            den_core::ArmatureAvailability::Connected,
        ),
        den_core::Governance::Interactive,
    )
}

fn resolved_or_stored_conversation_id(session: &client_sessions::ClientSessionRow) -> &str {
    session
        .resolved_conversation_id
        .as_deref()
        .filter(|id| !id.trim().is_empty())
        .unwrap_or(session.conversation_id.as_str())
}

pub(super) async fn require_session_conversation_access(
    state: &DenState,
    session: &client_sessions::ClientSessionRow,
) -> Result<(), CustomError> {
    access::readable_source(state, session).await?;
    Ok(())
}

pub(crate) async fn hats_list_result(
    state: &DenState,
    headers: &HeaderMap,
    params: &Value,
) -> Result<Value, CustomError> {
    let (user_id, bear) = authenticated_bear(state, headers, params).await?;
    let request: SessionHatListRequest = parse_params(params)?;
    let bear_id = BearId::new(bear.id);
    let ide_default_hat_id = hats::ide_default_hat(&state.sqlx_pool, bear_id).await?;
    let selected_hat_id = if let Some(session_id) = request.session_id.as_deref() {
        require_exclusive_client_session_id(
            &state.sqlx_pool,
            &ClientSessionId::new(session_id.to_string())?,
            UserId::new(user_id),
            bear_id,
        )
        .await?;
        let session = client_sessions::find_for_user_bear_session_id(
            &state.sqlx_pool,
            user_id,
            bear.id,
            session_id,
        )
        .await?
        .ok_or_else(|| CustomError::NotFound("IDE session not found".into()))?;
        let source = access::project_source(state, &session).await?;
        if let Some(conversation) = source.conversation {
            if let Some(work) =
                den_docket::work_runs::get_live_work_run_by_session(&state.sqlx_pool, session_id)
                    .await?
            {
                hats::bindings::job_hat(&state.sqlx_pool, bear_id, work.job_id).await?
            } else {
                hats::bindings::conversation_hat(&state.sqlx_pool, bear_id, conversation.id).await?
            }
        } else {
            None
        }
    } else {
        None
    };
    let may_manage_hat_policy = bears_db::role_is_bear_admin(
        bears_db::membership_role_for_user(&state.sqlx_pool, user_id, bear.id)
            .await?
            .flatten()
            .as_deref(),
    );
    Ok(json!({
        "hats": hats::list_hats(&state.sqlx_pool, bear_id).await?,
        "ide_default_hat_id": ide_default_hat_id,
        "selected_hat_id": selected_hat_id,
        "may_manage_hat_policy": may_manage_hat_policy,
    }))
}

/// Read a narrowly targeted hat grant for a verified human conversation.
/// This is advisory to the connected armature: it must still prove its current
/// canonical workspace root, target path, OS permissions, and tool obligation.
pub(crate) async fn hat_workspace_tool_check_result(
    state: &DenState,
    headers: &HeaderMap,
    params: &Value,
) -> Result<Value, CustomError> {
    let (user_id, bear) = authenticated_bear(state, headers, params).await?;
    let request: SessionHatWorkspaceToolCheckRequest = parse_params(params)?;
    let session_id = ClientSessionId::new(request.session_id)?;
    let bear_id = BearId::new(bear.id);
    require_exclusive_client_session_id(
        &state.sqlx_pool,
        &session_id,
        UserId::new(user_id),
        bear_id,
    )
    .await?;
    let session = client_sessions::find_for_user_bear_session_id(
        &state.sqlx_pool,
        user_id,
        bear.id,
        session_id.as_str(),
    )
    .await?
    .ok_or_else(|| CustomError::NotFound("IDE session not found".into()))?;
    if session.closed_at.is_some() || session.archived_at.is_some() {
        return Err(CustomError::Authorization(
            "closed or archived IDE sessions cannot use hat workspace grants".into(),
        ));
    }
    if den_docket::work_runs::get_work_run_by_session(&state.sqlx_pool, session_id.as_str())
        .await?
        .is_some()
    {
        return Err(CustomError::Authorization(
            "Work sessions cannot use interactive workspace grants".into(),
        ));
    }
    interactive_session_policy()
        .capabilities
        .require(den_core::BearCapability::UseArmatureTools)?;
    let viewer = conversation_viewer(state, bear.id, user_id).await?;
    let conversation_id = resolved_or_stored_conversation_id(&session);
    let conversation =
        authorize_existing_conversation(&viewer, &state.sqlx_pool, bear.id, conversation_id)
            .await?
            .ok_or_else(|| CustomError::NotFound("conversation not found".into()))?;
    let workspace_root = WorkspaceRoot::parse(&request.workspace_root)?;
    if !session
        .trusted_workspace_context()
        .roots
        .iter()
        .filter_map(|root| WorkspaceRoot::parse(root).ok())
        .any(|root| root == workspace_root)
    {
        return Ok(json!({ "allowed": false }));
    }
    let grant = HatAccessGrant::ReadOnlyToolInWorkspace(
        ReadOnlyWorkspaceAction::from_provider_name(&request.tool_name)?,
        workspace_root,
    );
    let allowed = hats::access::has_grant_for_own_conversation(
        &state.sqlx_pool,
        bear_id,
        conversation.id,
        UserId::new(user_id),
        &grant,
    )
    .await?;
    Ok(json!({ "allowed": allowed, "eligible": true }))
}

pub(crate) async fn session_hat_select_result(
    state: &DenState,
    headers: &HeaderMap,
    params: &Value,
) -> Result<Value, CustomError> {
    let (user_id, bear) = authenticated_bear(state, headers, params).await?;
    let request: SessionHatSelectRequest = parse_params(params)?;
    let session_id = ClientSessionId::new(request.session_id)?;
    let bear_id = BearId::new(bear.id);
    require_exclusive_client_session_id(
        &state.sqlx_pool,
        &session_id,
        UserId::new(user_id),
        bear_id,
    )
    .await?;
    let session = client_sessions::find_for_user_bear_session_id(
        &state.sqlx_pool,
        user_id,
        bear.id,
        session_id.as_str(),
    )
    .await?
    .ok_or_else(|| CustomError::NotFound("IDE session not found".into()))?;
    let requested = uuid::Uuid::parse_str(&request.hat_id)
        .map_err(|_| CustomError::ValidationError("hat_id must be a UUID".into()))?;
    let hat_id = HatId::new(requested);
    hats::manage::get_hat(&state.sqlx_pool, bear_id, hat_id).await?;
    let source = access::project_source(state, &session).await?;
    if !source.access.may_select_hat {
        return Err(CustomError::Authorization(
            "a hat can only be selected for your live IDE source before its first turn".into(),
        ));
    }
    if let Some(conversation) = source.conversation {
        access::require_live_source(state, &session).await?;
        hats::bindings::select_initial_conversation_hat(
            &state.sqlx_pool,
            bear_id,
            conversation.id,
            UserId::new(user_id),
            session_id.as_str(),
            hat_id,
        )
        .await?;
    } else {
        admission::materialize_pending(&state.sqlx_pool, &session, hat_id).await?;
    }
    let session = client_sessions::find_for_user_bear_session_id(
        &state.sqlx_pool,
        user_id,
        bear.id,
        session_id.as_str(),
    )
    .await?
    .ok_or_else(|| CustomError::NotFound("IDE session not found".into()))?;
    let conversation_id = resolved_or_stored_conversation_id(&session).to_string();
    Ok(json!({
        "ok": true, "hat_id": hat_id, "conversation_id": conversation_id,
        "session": session_state_payload(state, session, bear.work_enabled).await?,
    }))
}

pub(crate) async fn session_compact_result(
    state: &DenState,
    headers: &HeaderMap,
    params: &Value,
) -> Result<Value, CustomError> {
    let (user_id, bear) = authenticated_bear(state, headers, params).await?;
    let request: SessionIdRequest = parse_params(params)?;
    let session_id = request.session_id;
    let session = client_sessions::find_for_user_bear_session(
        &state.sqlx_pool,
        user_id,
        &bear.slug,
        &session_id,
    )
    .await?
    .ok_or_else(|| CustomError::NotFound("BearWire session not found".to_string()))?;
    let conversation_id = resolved_or_stored_conversation_id(&session);
    access::require_live_source(state, &session).await?;
    let origin =
        if den_docket::work_runs::get_live_work_run_by_session(&state.sqlx_pool, &session_id)
            .await?
            .is_some()
        {
            den_core::TurnExecutionOrigin::AuthorizedWorkRun(
                den_core::ArmatureAvailability::Connected,
            )
        } else {
            den_core::TurnExecutionOrigin::ArmatureConversation(
                den_core::ArmatureAvailability::Connected,
            )
        };
    let state_result = prepare_turn_compaction(
        &state.sqlx_pool,
        &state.config,
        bear.id,
        conversation_id,
        CompactionSource::Turn(origin),
        TurnCompactionTrigger::Manual,
    )
    .await?;

    let compacted = state_result
        .as_ref()
        .is_some_and(|state| state.compacted_seq_cutoff.is_some());
    Ok(json!({
        "ok": true,
        "session_id": session_id,
        "conversation_id": conversation_id,
        "compact_result": {
            "status": if compacted { "applied" } else { "skipped" },
            "reason": "bearwire_manual",
            "compacted_seq_cutoff": state_result.as_ref().and_then(|state| state.compacted_seq_cutoff),
            "group_count": state_result.as_ref().map(|state| state.groups.len()).unwrap_or(0),
        }
    }))
}

pub(crate) async fn session_close_result(
    state: &DenState,
    headers: &HeaderMap,
    params: &Value,
) -> Result<Value, CustomError> {
    let (user_id, bear) = authenticated_bear(state, headers, params).await?;
    let request: SessionIdRequest = parse_params(params)?;
    let session_id = request.session_id;
    require_exclusive_client_session_id(
        &state.sqlx_pool,
        &ClientSessionId::new(session_id.clone())?,
        UserId::new(user_id),
        BearId::new(bear.id),
    )
    .await?;
    let Some(session) = client_sessions::find_for_user_bear_session(
        &state.sqlx_pool,
        user_id,
        &bear.slug,
        &session_id,
    )
    .await?
    else {
        return Ok(json!({ "ok": true, "closed": false, "session_id": session_id }));
    };
    require_session_conversation_access(state, &session).await?;
    let reflection_payload = if access::project_source(state, &session).await?.access.state
        != bearwire_protocol::session::SessionAccessState::Executable
    {
        json!({"status": "skipped"})
    } else {
        match reflect_pair_session(&state.sqlx_pool, state, &session, "session_close").await {
            Ok(payload) => payload,
            Err(error) => {
                tracing::warn!(
                    bear_id = %bear.id,
                    session_id = %session_id,
                    error = %error,
                    "pair reflection failed during session close"
                );
                json!({
                    "status": "failed_open",
                    "error": error.to_string(),
                })
            }
        }
    };
    client_sessions::mark_closed(&state.sqlx_pool, session.id).await?;
    let disconnected = den_docket::work_runs::disconnect_attached_work_run(
        &state.sqlx_pool,
        &session_id,
        den_docket::work_runs::ATTACHED_DISCONNECT_TIMEOUT,
    )
    .await?
    .is_some();
    let mut event = BearWireEvent::ephemeral(
        "session.closed",
        json!({
            "session_id": session_id,
            "bear_slug": bear.slug,
            "pair_reflection": reflection_payload,
        }),
    );
    event.bear_id = Some(bear.id.to_string());
    event.human_id = Some(user_id.to_string());
    event.session_id = Some(session_id.clone());
    let persisted = bearwire_events::append_bearwire_event(
        &state.sqlx_pool,
        &session_id,
        Some(bear.id),
        Some(user_id),
        event,
    )
    .await?;
    Ok(json!({
        "ok": true,
        "closed": true,
        "session_id": session_id,
        "event_sequence": persisted.sequence_no,
        "pair_reflection": reflection_payload,
        "attached_work_disconnected": disconnected,
    }))
}

pub(crate) async fn session_state_result(
    state: &DenState,
    headers: &HeaderMap,
    params: &Value,
) -> Result<Value, CustomError> {
    let request: SessionStateRequest = parse_params(params)?;
    let Some(bear_slug) = request
        .bear_slug
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    else {
        return Ok(json!({
            "status": "available",
            "note": "Provide bear_slug and optional session_id for authenticated BearWire session state.",
            "params": params,
        }));
    };
    let user_id = authenticate_for_bear_slug(state, headers, bear_slug).await?;
    let bear = bears_db::bear_for_user_by_slug(&state.sqlx_pool, user_id, bear_slug)
        .await?
        .ok_or_else(|| CustomError::NotFound("Bear not found or token lacks access".to_string()))?;
    if let Some(session_id) = request
        .session_id
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        let session = client_sessions::find_for_user_bear_session(
            &state.sqlx_pool,
            user_id,
            bear_slug,
            session_id,
        )
        .await?;
        return Ok(json!({
            "kind": "single",
            "bear_slug": bear_slug,
            "session": match session {
                Some(session) => Some(session_state_payload(state, session, bear.work_enabled).await?),
                None => None,
            },
        }));
    }

    let include_closed = request.include_closed.unwrap_or(false);
    let limit = request.limit.unwrap_or(50).clamp(1, 100);
    let sessions = client_sessions::list_for_user_bear(
        &state.sqlx_pool,
        client_sessions::SessionListParams {
            user_id,
            bear_slug,
            include_closed,
            cwd_filter: None,
            limit,
            cursor_updated_at: None,
            cursor_id: None,
        },
    )
    .await?;
    let mut sessions_payload = Vec::with_capacity(sessions.len());
    for session in sessions {
        sessions_payload.push(session_state_payload(state, session, bear.work_enabled).await?);
    }
    Ok(json!({
        "kind": "list",
        "bear_slug": bear_slug,
        "sessions": sessions_payload,
    }))
}

pub(crate) async fn session_execution_diagnostics_result(
    state: &DenState,
    headers: &HeaderMap,
    params: &Value,
) -> Result<Value, CustomError> {
    let request: SessionExecutionDiagnosticsRequest = parse_params(params)?;
    let (user_id, bear) = authenticated_bear(state, headers, params).await?;
    let session = client_sessions::find_for_user_bear_session_id(
        &state.sqlx_pool,
        user_id,
        bear.id,
        &request.session_id,
    )
    .await?
    .ok_or_else(|| CustomError::NotFound("client session not found".to_string()))?;
    require_session_conversation_access(state, &session).await?;
    let diagnostics = crate::methods::focused_execution::focused_execution_diagnostics(
        state,
        user_id,
        bear.id,
        &session.client_session_id,
        request.limit.unwrap_or(32).clamp(1, 100),
    )
    .await?;
    Ok(json!({
        "kind": "focused_execution_diagnostics",
        "bear_slug": bear.slug,
        "session_id": session.client_session_id,
        "diagnostics": diagnostics,
    }))
}

#[cfg(test)]
mod tests;
