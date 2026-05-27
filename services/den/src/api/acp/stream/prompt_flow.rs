use axum::http::StatusCode;
use uuid::Uuid;

use crate::{
    api::{
        acp::{
            acp_error_status_message, authenticate_acp_code_token_with_auth,
            history::acp_auto_title_instruction,
            paths::require_absolute_cwd,
            prompt_context::{
                acp_direct_tool_prompt_context_with_activity, acp_plan_mode_prompt_context,
            },
            requested_mode_from_prompt, resolve_acp_turn_context,
            stream::orchestration::{build_acp_sse_response, build_acp_stream_setup},
            AcpPromptRequest,
        },
        auth::{self, ApiError},
        service::ApiState,
    },
    core::{
        acp_plan_mode,
        acp_runtime::{
            ensure_acp_session_conversation, require_pair_runtime_binding,
            verify_acp_conversation_belongs_to_binding,
        },
        acp_sessions::{self, UpsertAcpSession},
        acp_tokens,
        acp_tools::acp_client_tool_descriptors_for_client_context,
        bears::{db as bears_db, BearAgentRole},
        den_tools,
        work_plans::{self, WorkPlanLookup},
    },
    errors::CustomError,
};

pub(in crate::api::acp) async fn run_prompt_flow(
    state: ApiState,
    slug: String,
    session_id: String,
    headers: axum::http::HeaderMap,
    body: AcpPromptRequest,
    request_id: Uuid,
) -> Result<Result<axum::response::Response, CustomError>, ApiError> {
    let slug = slug.trim().to_string();
    let token = auth::extract_bearer_token(&headers)?;
    let auth = authenticate_acp_code_token_with_auth(&state, &token, &slug)
        .await
        .map_err(|err| {
            let (status, code, message) = acp_error_status_message(&err);
            ApiError::new(status, code, message)
        })?;
    let user_id = auth.user_id;
    let tools_enabled = acp_tokens::scopes_contains(&auth.scopes, acp_tokens::acp_tools_scope());
    if !tools_enabled {
        tracing::info!(bear_slug = %slug, user_id = user_id, "ACP token lacks acp:tools; local client tools disabled for prompt");
    }
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

    let pair_runtime_binding =
        match require_pair_runtime_binding(&state.sqlx_pool, state.letta.as_ref(), &bear).await {
            Ok(binding) => binding,
            Err(err) => return Ok(Err(err)),
        };

    let session_id = session_id.trim();
    if session_id.is_empty() {
        return Ok(Err(CustomError::ValidationError(
            "session_id must not be empty".to_string(),
        )));
    }

    let client = super::super::normalize_acp_client(body.client.as_deref());
    let cwd = require_absolute_cwd(body.client_context.get("cwd").and_then(|v| v.as_str()))
        .map_err(|err| {
            let (status, code, message) = acp_error_status_message(&err);
            ApiError::new(status, code, message)
        })?;
    let existing_session =
        acp_sessions::find_for_user_bear_session(&state.sqlx_pool, user_id, &bear.slug, session_id)
            .await
            .map_err(|err| {
                ApiError::new(
                    StatusCode::UNPROCESSABLE_ENTITY,
                    "database",
                    err.to_string(),
                )
            })?;
    let is_new_session_binding = existing_session.is_none();
    let requested_initial_mode = match requested_mode_from_prompt(&body) {
        Ok(mode) => mode,
        Err(err) => return Ok(Err(err)),
    };
    let generated_conversation_id = super::super::new_acp_conversation_id(&client);
    let (conversation_resolution, ensure_conversation_result) = ensure_acp_session_conversation(
        state.letta.as_ref(),
        crate::core::runtime_contracts::EnsureConversationRequest {
            bear_id: bear.id,
            role: "pair".to_string(),
            acp_session_id: session_id.to_string(),
            requested_selection: body.conversation_id.clone(),
            binding: pair_runtime_binding.clone(),
        },
        existing_session.as_ref(),
        generated_conversation_id,
    )
    .await
    .map_err(|err| {
        let (status, code, message) = acp_error_status_message(&err);
        ApiError::new(status, code, message)
    })?;
    if conversation_resolution.requires_belongs_to_bear_check {
        verify_acp_conversation_belongs_to_binding(
            state.letta.as_ref(),
            &pair_runtime_binding,
            &conversation_resolution.session_selection,
        )
        .await
        .map_err(|err| {
            let (status, code, message) = acp_error_status_message(&err);
            ApiError::new(status, code, message)
        })?;
    }
    if ensure_conversation_result.created {
        tracing::info!(
            %request_id,
            acp_session_id = %session_id,
            bear_id = %bear.id,
            pending_conversation_id = %conversation_resolution.session_selection,
            resolved_conversation_id = %ensure_conversation_result.conversation.id,
            "ACP created fresh Letta conversation for new session"
        );
    }
    let runtime_session_id = format!("acp-api-direct:{client}:{}:{session_id}", bear.id);
    acp_sessions::upsert_session(
        &state.sqlx_pool,
        UpsertAcpSession {
            user_id,
            bear_id: bear.id,
            bear_slug: bear.slug.clone(),
            acp_session_id: session_id.to_string(),
            runtime_session_id: runtime_session_id.clone(),
            conversation_id: conversation_resolution.session_selection.clone(),
            resolved_conversation_id: conversation_resolution
                .resolved_conversation
                .as_ref()
                .map(|conversation| conversation.id.clone()),
            client: client.clone(),
            cwd: Some(cwd.clone()),
            current_mode: if is_new_session_binding {
                requested_initial_mode.map(str::to_string)
            } else {
                None
            },
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
    if is_new_session_binding {
        match requested_initial_mode {
            Some("plan") => {
                acp_plan_mode::enter_plan_mode(
                    &state.sqlx_pool,
                    acp_plan_mode::EnterPlanModeParams {
                        user_id,
                        bear_id: bear.id,
                        bear_slug: bear.slug.clone(),
                        acp_session_id: session_id.to_string(),
                        reason: "Client selected ACP Plan mode before first prompt".to_string(),
                        requested_by: acp_plan_mode::AcpPlanModeRequestedBy::User,
                        previous_permission_mode: Some("ask".to_string()),
                    },
                )
                .await
                .map_err(|err| {
                    let (status, code, message) = acp_error_status_message(&err);
                    ApiError::new(status, code, message)
                })?;
                acp_sessions::set_current_mode(
                    &state.sqlx_pool,
                    user_id,
                    bear.id,
                    session_id,
                    "plan",
                )
                .await
                .map_err(|err| {
                    let (status, code, message) = acp_error_status_message(&err);
                    ApiError::new(status, code, message)
                })?;
            }
            Some("write") => {
                acp_sessions::set_current_mode(
                    &state.sqlx_pool,
                    user_id,
                    bear.id,
                    session_id,
                    "write",
                )
                .await
                .map_err(|err| {
                    let (status, code, message) = acp_error_status_message(&err);
                    ApiError::new(status, code, message)
                })?;
            }
            _ => {}
        }
    }
    tracing::info!(
        %request_id,
        acp_session_id = %session_id,
        bear_slug = %bear.slug,
        bear_id = %bear.id,
        role = "pair",
        compatibility_binding_id = %pair_runtime_binding.binding_id,
        client = %client,
        cwd = %cwd,
        requested_conversation_id = body.conversation_id.as_deref().map(str::trim),
        conversation_id = %conversation_resolution.session_selection,
        conversation_selection_source = %conversation_resolution.selection_source.as_str(),
        resolved_conversation_id = conversation_resolution
            .resolved_conversation
            .as_ref()
            .map(|conversation| conversation.id.as_str()),
        history_target = conversation_resolution
            .history_target
            .as_ref()
            .map(|conversation| conversation.id.as_str()),
        archive_target = conversation_resolution
            .archive_target
            .as_ref()
            .map(|conversation| conversation.id.as_str()),
        letta_conversation_id = %conversation_resolution.upstream_target,
        "ACP gateway routing prompt to pair role via Letta API"
    );
    let active_plan_mode =
        acp_plan_mode::active_for_session(&state.sqlx_pool, user_id, bear.id, session_id)
            .await
            .map_err(|err| {
                let (status, code, message) = acp_error_status_message(&err);
                ApiError::new(status, code, message)
            })?;
    let session_mode =
        acp_sessions::find_for_user_bear_session(&state.sqlx_pool, user_id, &bear.slug, session_id)
            .await
            .map_err(|err| {
                ApiError::new(
                    StatusCode::UNPROCESSABLE_ENTITY,
                    "database",
                    err.to_string(),
                )
            })?
            .map(|session| session.current_mode)
            .unwrap_or_else(|| "ask".to_string());
    let synthetic_session_row = acp_sessions::AcpSessionRow {
        id: Uuid::nil(),
        user_id,
        bear_id: bear.id,
        bear_slug: bear.slug.clone(),
        acp_session_id: session_id.to_string(),
        runtime_session_id: "runtime-test".to_string(),
        conversation_id: conversation_resolution.session_selection.clone(),
        resolved_conversation_id: conversation_resolution
            .resolved_conversation
            .as_ref()
            .map(|conversation| conversation.id.clone()),
        client: client.clone(),
        cwd: Some(cwd.clone()),
        adapter_environment: None,
        current_mode: session_mode,
        conversation_title: acp_sessions::find_for_user_bear_session(
            &state.sqlx_pool,
            user_id,
            &bear.slug,
            session_id,
        )
        .await
        .map_err(|err| {
            ApiError::new(
                StatusCode::UNPROCESSABLE_ENTITY,
                "database",
                err.to_string(),
            )
        })?
        .and_then(|session| session.conversation_title),
        conversation_title_updated_at: None,
        conversation_title_synced_at: None,
        closed_at: None,
        archived_at: None,
        created_at: time::OffsetDateTime::UNIX_EPOCH,
        updated_at: time::OffsetDateTime::UNIX_EPOCH,
    };
    let auto_title_guidance = acp_auto_title_instruction(&synthetic_session_row);
    let resolved_policy =
        resolve_acp_turn_context(&synthetic_session_row, active_plan_mode.as_ref(), None).policy;
    let plan_mode_context = acp_plan_mode_prompt_context(&state, bear.id, user_id, session_id)
        .await
        .map_err(|err| {
            let (status, code, message) = acp_error_status_message(&err);
            ApiError::new(status, code, message)
        })?;
    let current_activity_plan = work_plans::get_visible_work_plan(
        &state.sqlx_pool,
        bear.id,
        BearAgentRole::Pair,
        user_id,
        WorkPlanLookup {
            plan_id: None,
            source_conversation_id: None,
            source_acp_session_id: Some(session_id.to_string()),
        },
    )
    .await
    .map_err(|err| {
        let (status, code, message) = acp_error_status_message(&err);
        ApiError::new(status, code, message)
    })?;
    let tool_prompt_context = acp_direct_tool_prompt_context_with_activity(
        session_id,
        &cwd,
        &body.client_context,
        tools_enabled,
        &resolved_policy,
        current_activity_plan.as_ref(),
        auto_title_guidance.as_deref(),
    );
    let merged_client_tool_descriptors = tools_enabled.then(|| {
        super::super::merge_acp_pair_tool_descriptors(acp_client_tool_descriptors_for_client_context(
            &body.client_context,
            Some(&resolved_policy),
        ))
    });
    let auto_title_tool_advertised = merged_client_tool_descriptors
        .as_ref()
        .and_then(|value| value.as_array())
        .is_some_and(|items| {
            items.iter().any(|item| {
                item.get("name")
                    .and_then(|v| v.as_str())
                    .is_some_and(|name| name == den_tools::DEN_CONVERSATION_SET_TITLE_PROVIDER)
            })
        });
    tracing::info!(
        %request_id,
        acp_session_id = %session_id,
        auto_title_guidance_injected = auto_title_guidance.is_some(),
        auto_title_tool_advertised,
        current_conversation_title = synthetic_session_row.conversation_title.as_deref(),
        resolved_conversation_id = synthetic_session_row.resolved_conversation_id.as_deref(),
        conversation_id = %synthetic_session_row.conversation_id,
        "ACP auto-title prompt state"
    );
    let plans = current_activity_plan.clone().into_iter().collect::<Vec<_>>();
    let activity_context = work_plans::render_workboard_prompt_context(&plans);
    tracing::info!(
        %request_id,
        acp_session_id = %session_id,
        prompt_message_len = prompt.len(),
        plan_mode_context_len = plan_mode_context.len(),
        activity_context_len = activity_context.len(),
        tool_prompt_context_len = tool_prompt_context.len(),
        prompt_has_trusted_mode_suffix = prompt.contains("Trusted ACP session mode this turn:"),
        prompt_has_system_reminder = prompt.contains("<system-reminder>"),
        "ACP prompt context assembly lengths"
    );
    let setup = build_acp_stream_setup(
        &state,
        user_id,
        &bear,
        session_id,
        &cwd,
        &body.client_context,
        &conversation_resolution,
        &current_activity_plan,
        &plan_mode_context,
        &activity_context,
        &tool_prompt_context,
        prompt,
        request_id,
    )
    .await?;

    build_acp_sse_response(
        state,
        user_id,
        request_id,
        session_id,
        &bear,
        &client,
        prompt,
        &pair_runtime_binding,
        &conversation_resolution,
        &resolved_policy,
        &current_activity_plan,
        merged_client_tool_descriptors,
        setup,
    )
    .await
}
