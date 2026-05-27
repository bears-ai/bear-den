use serde::Deserialize;
use tokio::sync::oneshot;
use uuid::Uuid;

use crate::{
    api::{
        acp::{
            acp_den_provider_to_canonical_tool_name, acp_tool_timeout_ms_for_provider,
            default_unavailable_context_budget, pending_web_fetch_approvals, AcpGatewayEvent,
            AcpStreamContext, PendingWebFetchApproval, ToolExecutionRoute,
        },
        acp::types::PersistedToolRequestEffect,
    },
    core::{
        acp_sessions,
        acp_tool_turns::{AcpToolResultRequest, AcpToolTurnRegistration},
        bears::{db as bears_db, BearAgentRole},
        den_tools::{self, DenToolChannelContext, DenToolInvocationContext},
        web_policy,
    },
    errors::CustomError,
};

pub(in crate::api::acp) async fn persist_stream_event_side_effects(
    context: &AcpStreamContext,
    event: &mut AcpGatewayEvent,
) -> Result<Option<PersistedToolRequestEffect>, CustomError> {
    let mut tool_request_effect = None;
    match event {
        AcpGatewayEvent::ConversationResolved { conversation_id } => {
            acp_sessions::mark_resolved(
                &context.pool,
                context.user_id,
                context.bear_id,
                &context.acp_session_id,
                conversation_id,
            )
            .await?;
        }
        AcpGatewayEvent::ToolRequest {
            tool_call_id,
            approval_request_id,
            tool_name,
            request_id,
            args,
            result_tx,
            result_rx: _,
            approval_required,
            approval_reason,
            ..
        } => {
            let route = super::super::tool_execution_route(tool_name, args);
            let effect_tool_call_id = tool_call_id.clone();
            let effect_tool_name = tool_name.clone();
            let mut effect_den_server_result_rx = None;
            tracing::info!(
                request_id = %context.request_id,
                acp_session_id = %context.acp_session_id,
                tool_request_id = %request_id,
                tool_call_id = %tool_call_id,
                tool_name = %tool_name,
                route = ?route,
                approval_required = %approval_required,
                approval_request_id = ?approval_request_id,
                "ACP tool request route classified"
            );
            match route {
                ToolExecutionRoute::Unsupported => {
                    let result_tx = result_tx.take().ok_or_else(|| {
                        CustomError::System(
                            "ACP unsupported tool request missing result channel".to_string(),
                        )
                    })?;
                    *approval_required = false;
                    *approval_reason = None;
                    let detail = args
                        .get("_unsupported_detail")
                        .and_then(|v| v.as_str())
                        .unwrap_or("Unsupported ACP/Den tool")
                        .to_string();
                    let result = AcpToolResultRequest {
                        turn_id: None,
                        request_id: Some(context.request_id.to_string()),
                        tool_call_id: Some(tool_call_id.clone()),
                        tool_name: Some(tool_name.clone()),
                        approval_request_id: approval_request_id.clone(),
                        status: "unsupported".to_string(),
                        content: Some(detail.clone()),
                        structured_content: serde_json::json!({}),
                        diagnostic: serde_json::json!({
                            "component": "den.acp",
                            "phase": "unsupported_tool_settled",
                            "tool_name": tool_name,
                            "tool_call_id": tool_call_id,
                        }),
                        ..Default::default()
                    };
                    let _ = result_tx.send(result);
                    tracing::warn!(
                        request_id = %context.request_id,
                        acp_session_id = %context.acp_session_id,
                        tool_request_id = %request_id,
                        tool_call_id = %tool_call_id,
                        tool_name = %tool_name,
                        detail = %detail,
                        "ACP unsupported tool request settled with error result"
                    );
                }
                ToolExecutionRoute::DenServer => {
                    let canonical_name = acp_den_provider_to_canonical_tool_name(&effect_tool_name)
                        .ok_or_else(|| CustomError::System("missing Den tool route".to_string()))?;
                    let tool_request_id = request_id.clone();
                    if canonical_name == den_tools::DEN_WEB_FETCH {
                        route_web_fetch_tool_request(context, event, false).await?;
                    } else {
                        route_direct_den_tool_request(context, event, canonical_name).await?;
                    }
                    if let AcpGatewayEvent::ToolRequest { result_rx, .. } = event {
                        if let Some(result_rx) = result_rx.take() {
                            effect_den_server_result_rx = Some(result_rx);
                        }
                    }
                    tracing::info!(
                        request_id = %context.request_id,
                        acp_session_id = %context.acp_session_id,
                        tool_request_id = %tool_request_id,
                        tool_call_id = %effect_tool_call_id,
                        tool_name = %effect_tool_name,
                        canonical_tool_name = %canonical_name,
                        "ACP Den server tool routed"
                    );
                }
                ToolExecutionRoute::AdapterLocal => {
                    let result_tx = result_tx.take().ok_or_else(|| {
                        CustomError::System("ACP tool request missing result channel".to_string())
                    })?;
                    context.tool_turns.register(AcpToolTurnRegistration {
                        user_id: context.user_id,
                        bear_id: context.bear_id,
                        bear_slug: context.bear_slug.clone(),
                        acp_session_id: context.acp_session_id.clone(),
                        request_id: context.request_id,
                        tool_call_id: tool_call_id.clone(),
                        tool_name: tool_name.clone(),
                        approval_request_id: approval_request_id.clone(),
                        timeout_ms: acp_tool_timeout_ms_for_provider(tool_name),
                        result_tx,
                    })?;
                    tracing::info!(
                        request_id = %context.request_id,
                        acp_session_id = %context.acp_session_id,
                        tool_request_id = %request_id,
                        tool_call_id = %tool_call_id,
                        tool_name = %tool_name,
                        approval_required = %approval_required,
                        approval_request_id = ?approval_request_id,
                        "ACP adapter-local tool obligation registered"
                    );
                }
            }
            tool_request_effect = Some(PersistedToolRequestEffect {
                tool_call_id: effect_tool_call_id,
                tool_name: effect_tool_name,
                route,
                den_server_result_rx: effect_den_server_result_rx,
            });
        }
        _ => {}
    }
    Ok(tool_request_effect)
}

pub(in crate::api::acp) async fn route_web_fetch_tool_request(
    context: &AcpStreamContext,
    event: &mut AcpGatewayEvent,
    _plan_mode_active: bool,
) -> Result<(), CustomError> {
    let AcpGatewayEvent::ToolRequest {
        tool_call_id,
        approval_request_id,
        tool_name,
        request_id,
        args,
        result_tx,
        approval_required,
        approval_reason,
        ..
    } = event
    else {
        return Ok(());
    };
    let result_tx = result_tx.take().ok_or_else(|| {
        CustomError::System("ACP Den web_fetch request missing result channel".to_string())
    })?;
    *approval_required = false;
    *approval_reason = None;
    let web_args = match serde_json::from_value::<WebFetchToolArgs>(args.clone()) {
        Ok(args) => args,
        Err(err) => {
            settle_den_tool_error(
                result_tx,
                context,
                tool_call_id,
                tool_name,
                approval_request_id.as_deref(),
                "den_server_tool_validation_failed",
                format!("web_fetch arguments are invalid: {err}"),
            );
            return Ok(());
        }
    };
    let (normalized, decision) = match web_policy::decide_web_fetch_approval(
        &context.pool,
        context.bear_id,
        web_args.url.trim(),
    )
    .await
    {
        Ok(value) => value,
        Err(err) => {
            settle_den_tool_error(
                result_tx,
                context,
                tool_call_id,
                tool_name,
                approval_request_id.as_deref(),
                "web_fetch_approval_policy_failed",
                err.to_string(),
            );
            return Ok(());
        }
    };
    if decision.is_approved() && web_policy::is_local_web_url(&normalized) {
        *tool_name = "local_web_fetch".to_string();
        args["url"] = serde_json::json!(normalized.url);
        context.tool_turns.register(AcpToolTurnRegistration {
            user_id: context.user_id,
            bear_id: context.bear_id,
            bear_slug: context.bear_slug.clone(),
            acp_session_id: context.acp_session_id.clone(),
            request_id: context.request_id,
            tool_call_id: tool_call_id.clone(),
            tool_name: tool_name.clone(),
            approval_request_id: approval_request_id.clone(),
            timeout_ms: acp_tool_timeout_ms_for_provider(tool_name),
            result_tx,
        })?;
        return Ok(());
    }
    if matches!(decision, web_policy::WebApprovalDecision::RequiresApproval) {
        let permission_id = format!("perm-{}", Uuid::new_v4());
        pending_web_fetch_approvals().lock().await.insert(
            permission_id.clone(),
            PendingWebFetchApproval {
                user_id: context.user_id,
                bear_id: context.bear_id,
                result_tx,
                context: context.clone(),
                provider_name: tool_name.clone(),
                tool_call_id: tool_call_id.clone(),
                approval_request_id: approval_request_id.clone(),
                args: args.clone(),
                normalized_url: normalized.clone(),
            },
        );
        *event = AcpGatewayEvent::PermissionRequest {
            request_id: request_id.clone(),
            permission_id,
            tool_call_id: tool_call_id.clone(),
            tool_name: tool_name.clone(),
            title: "Fetch URL".to_string(),
            reason: format!(
                "BEARS wants to fetch {}. Approve this URL or host?",
                normalized.url
            ),
            target: serde_json::json!({ "kind": "url", "url": normalized.url, "host": normalized.host }),
            options: vec![
                "allow_once".to_string(),
                "allow_url".to_string(),
                "allow_host".to_string(),
                "reject_once".to_string(),
            ],
        };
        return Ok(());
    }
    let result = invoke_acp_den_tool(
        context,
        den_tools::DEN_WEB_FETCH,
        tool_name,
        tool_call_id,
        approval_request_id.as_deref(),
        args.clone(),
    )
    .await;
    let _ = result_tx.send(result);
    tracing::info!(
        request_id = %context.request_id,
        acp_session_id = %context.acp_session_id,
        tool_request_id = %request_id,
        tool_call_id = %tool_call_id,
        tool_name = %tool_name,
        canonical_tool_name = %den_tools::DEN_WEB_FETCH,
        web_approval_decision = %decision.as_str(),
        "ACP Den web_fetch tool executed"
    );
    Ok(())
}

pub(in crate::api::acp) async fn route_direct_den_tool_request(
    context: &AcpStreamContext,
    event: &mut AcpGatewayEvent,
    canonical_name: &str,
) -> Result<(), CustomError> {
    let AcpGatewayEvent::ToolRequest {
        tool_call_id,
        approval_request_id,
        tool_name,
        request_id,
        args,
        result_tx,
        approval_required,
        approval_reason,
        ..
    } = event
    else {
        return Ok(());
    };
    let result_tx = result_tx.take().ok_or_else(|| {
        CustomError::System("ACP Den tool request missing result channel".to_string())
    })?;
    *approval_required = false;
    *approval_reason = None;
    let result = invoke_acp_den_tool(
        context,
        canonical_name,
        tool_name,
        tool_call_id,
        approval_request_id.as_deref(),
        args.clone(),
    )
    .await;
    let _ = result_tx.send(result);
    tracing::info!(
        request_id = %context.request_id,
        acp_session_id = %context.acp_session_id,
        tool_request_id = %request_id,
        tool_call_id = %tool_call_id,
        tool_name = %tool_name,
        canonical_tool_name = %canonical_name,
        "ACP Den server tool executed"
    );
    Ok(())
}

#[derive(Debug, Deserialize)]
struct WebFetchToolArgs {
    url: String,
}

pub(in crate::api::acp) fn settle_den_tool_error(
    result_tx: oneshot::Sender<AcpToolResultRequest>,
    context: &AcpStreamContext,
    tool_call_id: &str,
    tool_name: &str,
    approval_request_id: Option<&str>,
    phase: &str,
    message: impl Into<String>,
) {
    let message = message.into();
    let result = AcpToolResultRequest {
        turn_id: None,
        request_id: Some(context.request_id.to_string()),
        tool_call_id: Some(tool_call_id.to_string()),
        tool_name: Some(tool_name.to_string()),
        approval_request_id: approval_request_id.map(str::to_string),
        status: "error".to_string(),
        content: Some(message.clone()),
        structured_content: serde_json::json!({}),
        diagnostic: serde_json::json!({
            "component": "den.acp",
            "phase": phase,
            "tool_name": tool_name,
            "tool_call_id": tool_call_id,
            "error": message,
        }),
        ..Default::default()
    };
    let _ = result_tx.send(result);
}

pub(in crate::api::acp) async fn invoke_acp_runtime_local_tool(
    context: &AcpStreamContext,
    tool_name: &str,
    tool_call_id: &str,
    args: serde_json::Value,
) -> AcpToolResultRequest {
    match tool_name {
        "bear_environment" => {
            let tool_context = DenToolInvocationContext {
                bear_id: context.bear_id,
                bear_slug: context.bear_slug.clone(),
                role_agent_id: context.pair_agent_id.clone(),
                agent_role: Some(BearAgentRole::Pair),
                user_id: context.user_id,
                username: context
                    .user_profile
                    .as_ref()
                    .map(|user| user.username.clone()),
                membership_role: bears_db::membership_role_for_user(
                    &context.pool,
                    context.user_id,
                    context.bear_id,
                )
                .await
                .ok()
                .flatten()
                .flatten(),
                conversation_id: context
                    .resolved_conversation_id
                    .clone()
                    .unwrap_or_else(|| context.upstream_target.clone()),
                session_id: context.acp_session_id.clone(),
                acp_session_id: Some(context.acp_session_id.clone()),
                conversation_selection: Some(context.conversation_selection.clone()),
                runtime_target: Some(context.upstream_target.clone()),
                workspace_roots: context.workspace_roots.clone(),
                session_policy: context.session_policy.clone(),
                activity: context.activity.clone(),
                runtime: Some(
                    context
                        .role_runtime
                        .tool_turn_runtime_snapshot(&context.acp_session_id, &context.tool_turns),
                ),
                context_budget: Some(default_unavailable_context_budget()),
                request_id: Some(context.request_id.to_string()),
                channel: DenToolChannelContext {
                    family: Some("acp_runtime".to_string()),
                    client: Some(context.client.clone()),
                    protocol: Some("acp".to_string()),
                },
            };
            match den_tools::invoke_den_tool(
                &context.pool,
                context.config.as_ref(),
                den_tools::DEN_BEAR_ENVIRONMENT,
                args,
                tool_context,
            )
            .await
            {
                Ok(value) => AcpToolResultRequest {
                    turn_id: None,
                    request_id: Some(context.request_id.to_string()),
                    tool_call_id: Some(tool_call_id.to_string()),
                    tool_name: Some(tool_name.to_string()),
                    approval_request_id: None,
                    status: "ok".to_string(),
                    content: Some(
                        serde_json::to_string_pretty(&value).unwrap_or_else(|_| value.to_string()),
                    ),
                    structured_content: value,
                    diagnostic: serde_json::json!({
                        "component": "den.acp",
                        "phase": "runtime_local_tool_result",
                        "tool_name": tool_name,
                    }),
                    ..Default::default()
                },
                Err(err) => AcpToolResultRequest {
                    turn_id: None,
                    request_id: Some(context.request_id.to_string()),
                    tool_call_id: Some(tool_call_id.to_string()),
                    tool_name: Some(tool_name.to_string()),
                    approval_request_id: None,
                    status: "error".to_string(),
                    content: Some(err.to_string()),
                    structured_content: serde_json::json!({}),
                    diagnostic: serde_json::json!({
                        "component": "den.acp",
                        "phase": "runtime_local_tool_error",
                        "tool_name": tool_name,
                    }),
                    ..Default::default()
                },
            }
        }
        _ => AcpToolResultRequest {
            turn_id: None,
            request_id: Some(context.request_id.to_string()),
            tool_call_id: Some(tool_call_id.to_string()),
            tool_name: Some(tool_name.to_string()),
            approval_request_id: None,
            status: "error".to_string(),
            content: Some(format!("unsupported ACP runtime local tool: {tool_name}")),
            structured_content: serde_json::json!({}),
            diagnostic: serde_json::json!({
                "component": "den.acp",
                "phase": "runtime_local_tool_unsupported",
                "tool_name": tool_name,
            }),
            ..Default::default()
        },
    }
}

pub(in crate::api::acp) async fn invoke_acp_den_tool(
    context: &AcpStreamContext,
    canonical_name: &str,
    provider_name: &str,
    tool_call_id: &str,
    approval_request_id: Option<&str>,
    args: serde_json::Value,
) -> AcpToolResultRequest {
    if canonical_name == den_tools::DEN_BEAR_ENVIRONMENT {
        return invoke_acp_runtime_local_tool(context, "bear_environment", tool_call_id, args)
            .await;
    }
    let membership_role =
        bears_db::membership_role_for_user(&context.pool, context.user_id, context.bear_id)
            .await
            .ok()
            .flatten()
            .flatten();
    let tool_context = DenToolInvocationContext {
        bear_id: context.bear_id,
        bear_slug: context.bear_slug.clone(),
        role_agent_id: context.pair_agent_id.clone(),
        agent_role: Some(BearAgentRole::Pair),
        user_id: context.user_id,
        username: context
            .user_profile
            .as_ref()
            .map(|user| user.username.clone()),
        membership_role,
        conversation_id: context
            .resolved_conversation_id
            .clone()
            .unwrap_or_else(|| context.upstream_target.clone()),
        session_id: context.acp_session_id.clone(),
        acp_session_id: Some(context.acp_session_id.clone()),
        conversation_selection: Some(context.conversation_selection.clone()),
        runtime_target: Some(context.upstream_target.clone()),
        workspace_roots: context.workspace_roots.clone(),
        session_policy: context.session_policy.clone(),
        activity: context.activity.clone(),
        runtime: Some(
            context
                .role_runtime
                .tool_turn_runtime_snapshot(&context.acp_session_id, &context.tool_turns),
        ),
        context_budget: Some(default_unavailable_context_budget()),
        request_id: Some(context.request_id.to_string()),
        channel: DenToolChannelContext {
            family: Some("acp".to_string()),
            client: Some("api-direct".to_string()),
            protocol: Some("acp".to_string()),
        },
    };
    match den_tools::invoke_den_tool(
        &context.pool,
        context.config.as_ref(),
        canonical_name,
        args,
        tool_context,
    )
    .await
    {
        Ok(value) => AcpToolResultRequest {
            turn_id: None,
            request_id: Some(context.request_id.to_string()),
            tool_call_id: Some(tool_call_id.to_string()),
            tool_name: Some(provider_name.to_string()),
            approval_request_id: approval_request_id.map(str::to_string),
            status: "ok".to_string(),
            content: Some(
                serde_json::to_string_pretty(&value).unwrap_or_else(|_| value.to_string()),
            ),
            structured_content: value,
            diagnostic: serde_json::json!({
                "component": "den.acp",
                "phase": "den_server_tool_result",
                "canonical_tool_name": canonical_name,
            }),
            ..Default::default()
        },
        Err(err) => AcpToolResultRequest {
            turn_id: None,
            request_id: Some(context.request_id.to_string()),
            tool_call_id: Some(tool_call_id.to_string()),
            tool_name: Some(provider_name.to_string()),
            approval_request_id: approval_request_id.map(str::to_string),
            status: "error".to_string(),
            content: Some(err.to_string()),
            structured_content: serde_json::json!({}),
            diagnostic: serde_json::json!({
                "component": "den.acp",
                "phase": "den_server_tool_error",
                "canonical_tool_name": canonical_name,
                "error": err.to_string(),
            }),
            ..Default::default()
        },
    }
}
