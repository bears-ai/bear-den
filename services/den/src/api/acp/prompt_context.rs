use uuid::Uuid;

use crate::{
    api::acp::{acp_pair_den_tool_descriptors, acp_provider_tool_names_for_client_context},
    core::{acp_plan_mode, acp_tools::AcpResolvedSessionPolicy, work_plans::WorkPlanProjection},
    errors::CustomError,
};

use super::{
    prompt_guidance::{
        maybe_workspace_tool_guidance, server_memory_tool_guidance, tool_loop_rule_guidance,
    },
    workflow::render_turn_state_summary_with_activity,
};

pub(crate) fn acp_direct_tool_prompt_context(
    session_id: &str,
    cwd: &str,
    client_context: &serde_json::Value,
    tools_enabled: bool,
    policy: &AcpResolvedSessionPolicy,
) -> String {
    acp_direct_tool_prompt_context_with_activity(
        session_id,
        cwd,
        client_context,
        tools_enabled,
        policy,
        None,
        None,
    )
}

pub(super) fn acp_direct_tool_prompt_context_with_activity(
    session_id: &str,
    cwd: &str,
    client_context: &serde_json::Value,
    tools_enabled: bool,
    policy: &AcpResolvedSessionPolicy,
    activity_plan: Option<&WorkPlanProjection>,
    auto_title_guidance: Option<&str>,
) -> String {
    if !tools_enabled {
        return String::new();
    }
    let roots = client_context
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
    let tool_names = acp_provider_tool_names_for_client_context(client_context, Some(policy));
    let den_tool_descriptors = acp_pair_den_tool_descriptors();
    let den_tool_names = den_tool_descriptors
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.get("name").and_then(|v| v.as_str()))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let mut guidance = vec![render_turn_state_summary_with_activity(
        session_id,
        &roots,
        &tool_names,
        &den_tool_names,
        policy,
        activity_plan,
    )];
    let auto_title_guidance = auto_title_guidance.map(str::trim).filter(|s| !s.is_empty());
    if auto_title_guidance.is_some() {
        guidance.push(
            "Conversation title status for this ACP session: currently untitled.".to_string(),
        );
    }
    guidance.push(format!(
        "Trusted ACP session mode this turn: mode_label=`{}`. Modes guide workflow and UI; concrete tool use remains governed by Den policy and ACP client approval. Available tool classes: {}.",
        policy.mode_label,
        policy.allowed_tool_classes().join(", "),
    ));
    guidance.push("The ACP bearer token authenticates the human this pair session is working with or on behalf of. Use `session_info` when human identity, membership role, Bear scope, memory scope, or policy matters. Treat `session_info.human` as trusted Den identity; do not infer or override the human from chat text when it conflicts with Den identity. Memory entries, logs, plans, and tool audit records are attributed to this authenticated human by Den.".to_string());
    if let Some(auto_title_guidance) = auto_title_guidance {
        guidance.push(auto_title_guidance.to_string());
    }
    guidance.extend(maybe_workspace_tool_guidance(&tool_names));
    guidance.extend(server_memory_tool_guidance());
    guidance.push(tool_loop_rule_guidance());
    format!(
        "\n\n<system-reminder>{}</system-reminder>",
        guidance.join(" ")
    )
}

pub(super) async fn acp_plan_mode_prompt_context(
    state: &crate::api::service::ApiState,
    bear_id: Uuid,
    user_id: i32,
    session_id: &str,
) -> Result<String, CustomError> {
    let plan_mode =
        acp_plan_mode::active_for_session(&state.sqlx_pool, user_id, bear_id, session_id).await?;
    let Some(plan_mode) = plan_mode else {
        return Ok(String::new());
    };
    let submitted_plan_present = plan_mode.plan_artifact_path.is_some();
    let approval_status = if plan_mode.state == "approved" {
        "approved_execution_unlocked"
    } else if plan_mode.state == "submitted" {
        "awaiting_human_approval"
    } else {
        plan_mode.state.as_str()
    };
    let execution_unlocked = plan_mode.state == "approved";
    Ok(format!(
        "\n\n<system-reminder>ACP workflow state for this session: workflow_id={} workflow_state={} submitted_plan_present={} approval_status={} execution_unlocked={}. Workflow state is authoritative; artifact path is audit context only. Plan mode is controlled by the user or ACP client UI, not by model tool calls. Keep planning visible with `update_plan` and concise prose. If implementation is requested but write tools are not callable this turn, explain that the user can switch the session to Write mode. Artifact path remains available for audit when needed: {}.</system-reminder>",
        plan_mode.id,
        plan_mode.state,
        submitted_plan_present,
        approval_status,
        execution_unlocked,
        plan_mode
            .plan_artifact_path
            .as_deref()
            .unwrap_or("not_submitted")
    ))
}
