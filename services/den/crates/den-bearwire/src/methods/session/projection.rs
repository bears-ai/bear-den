use super::{
    access, client_sessions, json, resolve_runtime_task_context,
    resolved_or_stored_conversation_id, CustomError, DenError, DenState, RuntimeTaskResolveRequest,
    Value,
};

pub(super) async fn session_state_payload(
    state: &DenState,
    session: client_sessions::ClientSessionRow,
    work_enabled: bool,
) -> Result<Value, CustomError> {
    let source = access::project_source(state, &session).await?;
    let conversation_external_id = source
        .conversation
        .as_ref()
        .and_then(|conversation| conversation.external_conversation_id.as_deref());
    let conversation_runtime_id = conversation_external_id.map(str::to_string);
    let latest_context_budget = source
        .conversation
        .as_ref()
        .and_then(|conversation| conversation.latest_context_budget.clone());
    let trusted_workspace = session.trusted_workspace_context();
    let project_execution = work_enabled
        && source.access.state == bearwire_protocol::session::SessionAccessState::Executable;

    let runtime_task_context = if project_execution {
        let context = resolve_runtime_task_context(
            &state.sqlx_pool,
            RuntimeTaskResolveRequest {
                bear_id: session.bear_id,
                // An authorized session read is not a live armature-tool turn.
                policy: den_core::EffectivePolicy::compile_for_origin(
                    den_core::TurnExecutionOrigin::ArmatureConversation(
                        den_core::ArmatureAvailability::Absent,
                    ),
                    den_core::Governance::Interactive,
                ),
                user_id: Some(session.user_id),
                conversation_id: resolved_or_stored_conversation_id(&session).to_string(),
                client_session_id: session.client_session_id.clone(),
                cached_activity_plan_projection: None,
            },
        )
        .await
        .map_err(|error| match error {
            DenError::Database(message) => CustomError::Database(format!(
                "resolve session runtime task context for BearWire session.state: bear_id={}, client_session_id={}, conversation_id={}: {message}",
                session.bear_id, session.client_session_id, resolved_or_stored_conversation_id(&session)
            )),
            DenError::DatabaseUnavailable(message) => CustomError::DatabaseUnavailable(format!(
                "resolve session runtime task context for BearWire session.state: bear_id={}, client_session_id={}, conversation_id={}: {message}",
                session.bear_id, session.client_session_id, resolved_or_stored_conversation_id(&session)
            )),
            error => error.into(),
        })?;
        Some(context)
    } else {
        None
    };
    let current_task = runtime_task_context
        .as_ref()
        .and_then(session_current_task_projection);
    let active_activity_plan = runtime_task_context.as_ref().and_then(|focus| {
        focus.active_activity_plan().cloned().map(|plan| {
            active_activity_plan_projection(plan, focus.source.as_str(), current_task.clone())
        })
    });
    let focused_execution = if project_execution {
        Some(
            crate::methods::focused_execution::load_focused_execution_snapshot(
                state,
                session.user_id,
                session.bear_id,
                &session.client_session_id,
                crate::methods::focused_execution::FocusedExecutionLaunchState::AlreadyRunning,
            )
            .await?
            .to_wire(),
        )
    } else {
        None
    };

    Ok(json!({
        "id": session.id,
        "user_id": session.user_id,
        "bear_id": session.bear_id,
        "bear_slug": session.bear_slug,
        "client_session_id": session.client_session_id,
        "runtime_session_id": session.runtime_session_id,
        "conversation_id": session.conversation_id,
        "resolved_conversation_id": session.resolved_conversation_id,
        // The armature must use this field for user-visible history replay. It
        // is the external ID of the canonical persisted conversation, not the
        // client-supplied pending identifier.
        "history_conversation_id": conversation_external_id,
        "access": source.access,
        "client": session.client,
        "cwd": session.cwd,
        "adapter_environment": session.adapter_environment,
        "current_mode": session.current_mode,
        "conversation_title": session.conversation_title,
        "conversation_title_updated_at": session.conversation_title_updated_at,
        "conversation_title_synced_at": session.conversation_title_synced_at,
        "closed_at": session.closed_at,
        "archived_at": session.archived_at,
        "created_at": session.created_at,
        "updated_at": session.updated_at,
        "context_budget": latest_context_budget,
        "current_task": current_task,
        "diagnostics": {
            "trusted_workspace": trusted_workspace,
            "runtime_conversation_id": conversation_runtime_id,
            "active_activity_plan": active_activity_plan,
            "focused_execution": focused_execution,
        }
    }))
}

pub(super) fn session_current_task_projection(
    context: &den_runtime::runtime::task_context::RuntimeTaskContext,
) -> Option<Value> {
    let task_id = context.current_task_id?;
    if context.source != den_runtime::runtime::task_context::RuntimeTaskSource::SessionCurrentTask {
        return None;
    }
    let item = context
        .active_activity_plan()?
        .current_item
        .as_ref()
        .filter(|item| item.id == task_id.to_string())?;
    Some(json!({
        "id": item.id,
        "title": item.title,
        "summary": item.summary,
        "status": item.status,
        "source_ref": item.source_ref,
    }))
}

pub(super) fn active_activity_plan_projection(
    plan: den_docket::TaskListProjection,
    source: &str,
    current_task: Option<Value>,
) -> Value {
    let current_item_id = plan.current_item.as_ref().map(|item| item.id.clone());
    json!({
        "schema": "den.acp_plan_projection.v1",
        "source": source,
        "projection": "flat_current_level",
        "id": plan.id,
        "title": plan.title,
        "status": plan.status,
        "version": plan.version,
        "current_item_id": current_item_id,
        "current_task": current_task,
        "items": plan.items.into_iter().map(|item| {
            let selection = (current_item_id.as_deref() == Some(item.id.as_str()))
                .then_some("current");
            json!({
                "id": item.id,
                "title": item.title,
                "summary": item.summary,
                "status": item.status,
                "selection": selection,
                "blocked_reason": item.blocked_reason,
                "source_ref": item.source_ref,
                "sync_state": item.sync_state,
            })
        }).collect::<Vec<_>>(),
    })
}
