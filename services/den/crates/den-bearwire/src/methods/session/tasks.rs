use super::{
    access, authenticated_bear, client_sessions, interactive_session_policy, json, parse_params,
    preview_session_current_task_selection, require_exclusive_client_session_id,
    require_session_conversation_access, select_session_current_task, BearId, ClientSessionId,
    CustomError, DenState, HeaderMap, RunStartRequest, SessionCurrentTaskClearRequest,
    SessionCurrentTaskSelectionRequest, SessionCurrentTaskStartRequest, UserId, Value,
};

pub(crate) async fn session_current_task_selection_request_result(
    state: &DenState,
    headers: &HeaderMap,
    params: &Value,
) -> Result<Value, CustomError> {
    let (user_id, bear) = authenticated_bear(state, headers, params).await?;
    let request: SessionCurrentTaskSelectionRequest = parse_params(params)?;
    let task_id = uuid::Uuid::parse_str(&request.task_id)
        .map_err(|_| CustomError::ValidationError("task_id must be a UUID".to_string()))?;
    require_exclusive_client_session_id(
        &state.sqlx_pool,
        &ClientSessionId::new(request.session_id.clone())?,
        UserId::new(user_id),
        BearId::new(bear.id),
    )
    .await?;
    let title = preview_session_current_task_selection(
        &state.sqlx_pool,
        user_id,
        bear.id,
        &request.session_id,
        task_id,
    )
    .await?;
    Ok(
        json!({"ok": true, "confirmation_required": true, "session_id": request.session_id, "task_id": task_id, "title": title}),
    )
}

pub(crate) async fn session_current_task_select_result(
    state: &DenState,
    headers: &HeaderMap,
    params: &Value,
) -> Result<Value, CustomError> {
    let (user_id, bear) = authenticated_bear(state, headers, params).await?;
    let request: SessionCurrentTaskSelectionRequest = parse_params(params)?;
    let task_id = uuid::Uuid::parse_str(&request.task_id)
        .map_err(|_| CustomError::ValidationError("task_id must be a UUID".to_string()))?;
    if !bear.work_enabled {
        return Err(CustomError::ValidationError(
            "focused task controls are disabled".to_string(),
        ));
    }
    require_exclusive_client_session_id(
        &state.sqlx_pool,
        &ClientSessionId::new(request.session_id.clone())?,
        UserId::new(user_id),
        BearId::new(bear.id),
    )
    .await?;
    let policy = interactive_session_policy();
    let result = select_session_current_task(
        &state.sqlx_pool,
        user_id,
        bear.id,
        &request.session_id,
        Some(task_id),
        &policy.capabilities,
    )
    .await?;
    Ok(
        json!({"ok": true, "session_id": request.session_id, "current_task_id": task_id, "title": result.title, "task_list": result.task_list}),
    )
}

pub(crate) async fn session_current_task_start_result(
    state: &DenState,
    headers: &HeaderMap,
    params: &Value,
) -> Result<Value, CustomError> {
    let (user_id, bear) = authenticated_bear(state, headers, params).await?;
    let request: SessionCurrentTaskStartRequest = parse_params(params)?;
    if !bear.work_enabled {
        return Err(CustomError::ValidationError(
            "focused task controls are disabled".to_string(),
        ));
    }
    require_exclusive_client_session_id(
        &state.sqlx_pool,
        &ClientSessionId::new(request.session_id.clone())?,
        UserId::new(user_id),
        BearId::new(bear.id),
    )
    .await?;
    if let Some(session) = client_sessions::find_for_user_bear_session_id(
        &state.sqlx_pool,
        user_id,
        bear.id,
        &request.session_id,
    )
    .await?
    {
        require_session_conversation_access(state, &session).await?;
    }
    if let Some(run) =
        den_runtime::turn_runs::active_run_for_session(&state.sqlx_pool, &request.session_id)
            .await?
            .filter(|run| run.bear_id == bear.id && run.user_id == user_id)
    {
        if den_runtime::turn_runs::technical_budget_recovery_snapshot(&state.sqlx_pool, &run.run_id)
            .await?
            .is_some()
        {
            let mut recovered = crate::methods::run::run_recover_result(
                state,
                headers,
                &json!({ "bear_slug": bear.slug, "run_id": run.run_id }),
            )
            .await?;
            recovered["recovered"] = json!(true);
            return Ok(recovered);
        }
    }
    let policy = interactive_session_policy();
    let execution = crate::methods::focused_execution::start_selected_session_task_execution(
        state,
        user_id,
        bear,
        &request.session_id,
        &policy.capabilities,
    )
    .await?;
    let run = execution.run.as_ref().ok_or_else(|| {
        CustomError::System("focused execution start returned no run authority".to_string())
    })?;
    let attempt = execution.attempt.as_ref().ok_or_else(|| {
        CustomError::System("focused execution start returned no attempt authority".to_string())
    })?;
    let task = execution.task.as_ref().ok_or_else(|| {
        CustomError::System("focused execution start returned no selected task".to_string())
    })?;
    Ok(json!({
        "ok": true,
        "queued": execution.launch_state
            == crate::methods::focused_execution::FocusedExecutionLaunchState::Queued,
        "claimed": execution.launch_state
            == crate::methods::focused_execution::FocusedExecutionLaunchState::Claimed,
        "started": execution.launch_state
            == crate::methods::focused_execution::FocusedExecutionLaunchState::Started,
        "reused": execution.launch_state
            == crate::methods::focused_execution::FocusedExecutionLaunchState::AlreadyRunning,
        "run_id": run.id,
        "session_id": execution.session_id,
        "task_id": task.id,
        "state": run.state,
        "execution_attempt_id": attempt.id,
        "execution_attempt_state": attempt.state,
        "launch_state": execution.launch_state,
        "fence_epoch": attempt.fence_epoch,
        "focused_execution": execution.to_wire(),
    }))
}

/// Starts focused execution for the session's selected task. Docket `/focus` uses
/// this after selecting its task so task assignment cannot leave loop control
/// inactive.
pub(crate) async fn start_session_task_execution(
    state: &DenState,
    user_id: i32,
    bear: den_service::bears::Bear,
    session_id: &str,
) -> Result<crate::methods::focused_execution::FocusedExecutionLaunchState, CustomError> {
    let session = client_sessions::find_for_user_bear_session_id(
        &state.sqlx_pool,
        user_id,
        bear.id,
        session_id,
    )
    .await?
    .ok_or_else(|| CustomError::NotFound("client session not found".to_string()))?;
    require_session_conversation_access(state, &session).await?;
    let task_id = session.current_task_id.ok_or_else(|| {
        CustomError::ValidationError(
            "no current session task is selected for this session".to_string(),
        )
    })?;
    access::require_live_source(state, &session).await?;
    let recovered_run_id = match crate::methods::focused_execution::reconcile_before_start(
        state, user_id, bear.id, task_id, session_id,
    )
    .await?
    {
        crate::methods::focused_execution::StartReconciliation::AlreadyRunning => {
            return Ok(
                crate::methods::focused_execution::FocusedExecutionLaunchState::AlreadyRunning,
            )
        }
        crate::methods::focused_execution::StartReconciliation::Launch { recovered_run_id } => {
            recovered_run_id
        }
    };

    let title = preview_session_current_task_selection(
        &state.sqlx_pool,
        user_id,
        bear.id,
        session_id,
        task_id,
    )
    .await?;

    // ponytail: this delegates to the established run.start lifecycle so task-start
    // cannot drift from Pair stream/event behavior.
    let mut start_params = serde_json::Map::new();
    start_params.insert("bear_slug".to_string(), json!(bear.slug));
    start_params.insert("session_id".to_string(), json!(session_id));
    start_params.insert(
        "prompt".to_string(),
        json!(format!("Start working on the selected task: {title}")),
    );
    start_params.insert("client".to_string(), json!(session.client));
    // Docket control is an explicit execution handoff. Its synthetic turn must receive
    // the same mutation/execution tool surface as an interactive Write turn.
    start_params.insert("requested_mode".to_string(), json!("write"));
    // Task starts are a deliberate Docket control handoff, not reconnect retries.
    start_params.insert("supersede_active_run".to_string(), json!(true));
    start_params.insert(
        "conversation_id".to_string(),
        json!(session.conversation_id),
    );
    if let Some(cwd) = session.cwd {
        start_params.insert("cwd".to_string(), json!(cwd));
    }
    if let Some(client_context) = session.adapter_environment {
        start_params.insert("client_context".to_string(), client_context);
    }
    let request: RunStartRequest = serde_json::from_value(Value::Object(start_params))
        .map_err(|err| CustomError::ValidationError(format!("invalid task start params: {err}")))?;
    let task_session_id = request.session_id.clone();
    let result = crate::methods::run::run_start_for_focused_task(
        state,
        request,
        user_id,
        bear.clone(),
        task_id,
    )
    .await?;
    let run_id = result["run_id"]
        .as_str()
        .ok_or_else(|| {
            CustomError::ValidationError("run.start returned a non-string run_id".to_string())
        })?
        .to_string();
    let launch_state = match result["launch_state"].as_str() {
        Some("queued") => crate::methods::focused_execution::FocusedExecutionLaunchState::Queued,
        Some("claimed") => crate::methods::focused_execution::FocusedExecutionLaunchState::Claimed,
        Some("started") => crate::methods::focused_execution::FocusedExecutionLaunchState::Started,
        Some("already_running") => {
            crate::methods::focused_execution::FocusedExecutionLaunchState::AlreadyRunning
        }
        _ => {
            return Err(CustomError::System(
                "run.start returned an invalid launch_state".to_string(),
            ))
        }
    };

    if let Some(recovered_run_id) = recovered_run_id {
        crate::methods::focused_execution::project_recovery_handoff(
            state,
            user_id,
            bear.id,
            &task_session_id,
            &recovered_run_id,
            &run_id,
            task_id,
            launch_state,
        )
        .await?;
    }
    Ok(launch_state)
}

pub(crate) async fn session_current_task_clear_result(
    state: &DenState,
    headers: &HeaderMap,
    params: &Value,
) -> Result<Value, CustomError> {
    let (user_id, bear) = authenticated_bear(state, headers, params).await?;
    let request: SessionCurrentTaskClearRequest = parse_params(params)?;
    if !bear.work_enabled {
        return Err(CustomError::ValidationError(
            "focused task controls are disabled".to_string(),
        ));
    }
    require_exclusive_client_session_id(
        &state.sqlx_pool,
        &ClientSessionId::new(request.session_id.clone())?,
        UserId::new(user_id),
        BearId::new(bear.id),
    )
    .await?;
    let policy = interactive_session_policy();
    let result = select_session_current_task(
        &state.sqlx_pool,
        user_id,
        bear.id,
        &request.session_id,
        None,
        &policy.capabilities,
    )
    .await?;
    Ok(
        json!({"ok": true, "session_id": request.session_id, "current_task_id": Value::Null, "task_list": result.task_list}),
    )
}
