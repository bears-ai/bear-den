use anyhow::{anyhow, Context, Result};

use reqwest::header::{HeaderValue, AUTHORIZATION, CONTENT_TYPE};
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::time::{sleep, Duration, Instant};
use uuid::Uuid;

use crate::{
    adapter_contract_context, classify_completed_turn_without_text, den_request_context, env_bool,
    handle_conversation_resolved_projection, handle_permission_request_event,
    handle_plan_update_projection, handle_session_info_projection, handle_status_text_for_turn,
    is_den_server_tool_request, plan_entries_from_plan_update_event,
    project_den_owned_tool_request, send_agent_message_chunk_for_turn,
    send_agent_thought_chunk_for_turn, send_tool_call_update_for_turn, spawn_tool_request_task,
    truncate_for_log, AdapterSharedState, AdapterState, CompletedTurnWithoutText, Config,
    SseFrameOutcome, SseStreamDiagnostics, ToolCallUpdatePayload,
};

const BEARWIRE_POLL_INTERVAL: Duration = Duration::from_millis(250);
const BEARWIRE_PROMPT_TIMEOUT: Duration = Duration::from_secs(600);
const BEARWIRE_DEADLINE_RECONCILIATION_TIMEOUT: Duration = Duration::from_secs(10);
const BEARWIRE_EVENT_FETCH_FAILURE_GRACE: Duration = Duration::from_secs(10);
const BEARWIRE_EVENT_FETCH_MAX_BACKOFF: Duration = Duration::from_secs(5);
const BEARWIRE_TOOL_RAW_OUTPUT_PREVIEW_CHARS: usize = 24 * 1024;
const BEARWIRE_OBLIGATION_SYNC_INTERVAL: Duration = Duration::from_secs(1);
const BEARWIRE_RUN_STATE_DIAGNOSTIC_INTERVAL: Duration = Duration::from_secs(5);

fn is_optional_runtime_metadata_event(event_type: &str) -> bool {
    matches!(
        event_type,
        "session.opened"
            | "session.state"
            | "run.accepted"
            | "run.started"
            | "run.recovering"
            | "run.recovered"
            | "docket.execution.started"
            | "docket.execution.ended"
            | "runtime.objective_orientation"
    )
}

fn legacy_approval_free_read_only_tool(tool_name: &str) -> bool {
    matches!(
        tool_name,
        "fs_read_text_file" | "fs_list_directory" | "fs_find_paths" | "fs_search_files" | "fs_stat"
    )
}

fn generic_tool_summary(summary: &str) -> bool {
    matches!(summary.trim(), "Tool failed." | "Tool completed.")
}

fn generic_tool_summary_for_tool(summary: &str, tool_name: &str) -> bool {
    let normalized = summary.trim().trim_end_matches('.').to_ascii_lowercase();
    if normalized == format!("finished {}", tool_name.to_ascii_lowercase()) {
        return true;
    }
    if matches!(normalized.as_str(), "tool failed" | "tool completed") {
        return true;
    }
    let status_suffix = normalized
        .strip_suffix(" completed")
        .or_else(|| normalized.strip_suffix(" failed"));
    let Some(prefix) = status_suffix else {
        return false;
    };
    if prefix.strip_prefix("local tool ").is_some() {
        return true;
    }
    let fallback = crate::fallback_tool_title(tool_name).to_ascii_lowercase();
    let friendly = crate::friendly_tool_title(tool_name).to_ascii_lowercase();
    prefix == fallback || prefix == friendly
}

fn default_tool_status_summary(tool_name: &str, failed: bool) -> String {
    if !failed {
        match tool_name {
            "session_info" => return "Inspected session.".to_string(),
            "set_conversation_title" => return "Set conversation title.".to_string(),
            "memory_browse" => return "Browsed memory.".to_string(),
            "memory_read" => return "Read memory.".to_string(),
            "memory_search" => return "Searched memory.".to_string(),
            "memory_write_entry" => return "Wrote memory entry.".to_string(),
            "memory_request_review" => return "Requested memory review.".to_string(),
            "web_fetch" | "local_web_fetch" => return "Fetched URL.".to_string(),
            "web_search" => return "Searched web.".to_string(),
            "list_task_lists" => return "Listed task lists.".to_string(),
            "get_task_list_status" => return "Read task list status.".to_string(),
            "update_task_list" | "update_plan" => return "Updated task list.".to_string(),
            "request_task_list_handoff" | "request_work_handoff" => {
                return "Requested work handoff.".to_string();
            }
            "git_status" => return "Checked git status.".to_string(),
            "git_diff" => return "Read git diff.".to_string(),
            "git_log" => return "Read git log.".to_string(),
            "git_show" => return "Read git revision.".to_string(),
            "git_add" => return "Staged git changes.".to_string(),
            "git_restore" => return "Restored git paths.".to_string(),
            "git_commit" => return "Created git commit.".to_string(),
            "git_stash" => return "Created git stash.".to_string(),
            _ => {}
        }
    }
    let title = crate::friendly_tool_title(tool_name);
    if failed {
        format!("{title} failed.")
    } else {
        format!("{title} completed.")
    }
}

fn normalized_tool_arguments(data: &Value) -> Option<Value> {
    const ARGUMENT_POINTERS: &[&str] = &[
        "/args",
        "/arguments",
        "/input",
        "/raw_input",
        "/data/args",
        "/data/arguments",
        "/data/input",
        "/data/raw_input",
        "/tool_call/args",
        "/tool_call/arguments",
        "/tool_call/input",
        "/tool_call/raw_input",
        "/data/tool_call/args",
        "/data/tool_call/arguments",
        "/data/tool_call/input",
        "/data/tool_call/raw_input",
    ];

    ARGUMENT_POINTERS.iter().find_map(|pointer| {
        let candidate = data.pointer(pointer)?;
        match candidate {
            Value::String(raw) => serde_json::from_str(raw).ok(),
            Value::Object(_) => Some(candidate.clone()),
            _ => None,
        }
    })
}

fn display_command_arg(arg: &str) -> String {
    if arg.chars().any(char::is_whitespace) {
        serde_json::to_string(arg).unwrap_or_else(|_| arg.to_string())
    } else {
        arg.to_string()
    }
}

fn command_name_from_tool_event(data: &Value) -> Option<String> {
    let tool_name = data.get("tool_name").and_then(Value::as_str)?;
    if !matches!(
        tool_name,
        "run_command" | "process_run" | "terminal_run_command"
    ) {
        return None;
    }
    let arguments = normalized_tool_arguments(data)?;
    let command = arguments
        .get("command")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|command| !command.is_empty())
        .map(str::to_string)?;
    let arg_list = arguments
        .get("args")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let summary = if arg_list.is_empty() {
        command
    } else {
        format!(
            "{} {}",
            command,
            arg_list
                .iter()
                .map(|arg| display_command_arg(arg))
                .collect::<Vec<_>>()
                .join(" ")
        )
    };
    Some(summary)
}

fn default_tool_status_summary_with_context(data: &Value, tool_name: &str, failed: bool) -> String {
    if tool_name == "git_commit" && !failed {
        let result = data
            .get("structured_content")
            .or_else(|| data.get("result"))
            .unwrap_or(data);
        if let (Some(short_sha), Some(subject)) = (
            result.get("short_sha").and_then(Value::as_str),
            result.get("subject").and_then(Value::as_str),
        ) {
            let short_sha = short_sha.trim();
            let subject = subject.trim();
            if !short_sha.is_empty() && !subject.is_empty() {
                return format!("Git commit created: {short_sha} {subject}.");
            }
        }
    }

    if tool_name == "set_conversation_title" && !failed {
        if let Some(title) = normalized_tool_arguments(data)
            .as_ref()
            .and_then(|args| args.get("title"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|title| !title.is_empty())
        {
            return format!("Set conversation title to {title:?}.");
        }
    }

    let base = default_tool_status_summary(tool_name, failed);
    let Some(command) = command_name_from_tool_event(data) else {
        return base;
    };
    format!("{base} Command: `{command}`.")
}

fn tool_call_finished_summary(data: &Value, tool_name: &str, failed: bool) -> String {
    let candidate = [
        data.get("error_message").and_then(Value::as_str),
        data.get("summary").and_then(Value::as_str),
        data.get("message").and_then(Value::as_str),
        data.get("content").and_then(Value::as_str),
        data.get("detail").and_then(Value::as_str),
        data.pointer("/diagnostic/message").and_then(Value::as_str),
        data.pointer("/diagnostic/error").and_then(Value::as_str),
        data.pointer("/diagnostic/reason").and_then(Value::as_str),
    ]
    .into_iter()
    .flatten()
    .map(str::trim)
    .find(|message| {
        !message.is_empty()
            && !generic_tool_summary(message)
            && !generic_tool_summary_for_tool(message, tool_name)
    });

    match candidate {
        Some(message) => message.to_string(),
        None => {
            let structured_preview = data
                .get("structured_content")
                .map(|structured| crate::tool_completion_preview(tool_name, structured))
                .filter(|preview| !preview.trim().is_empty());
            if let Some(preview) = structured_preview {
                return preview;
            }
            if crate::is_placeholder_tool_name(tool_name) {
                let status = if failed { "failed" } else { "completed" };
                let tool_call_id = data
                    .pointer("/tool_call/id")
                    .and_then(Value::as_str)
                    .filter(|id| !id.trim().is_empty())
                    .unwrap_or("unknown");
                return format!(
                    "Tool call {status} (tool_call_id={tool_call_id}). Details: `{}`",
                    crate::compact_tool_json_detail(data, 1_200)
                );
            }
            default_tool_status_summary_with_context(data, tool_name, failed)
        }
    }
}

fn compact_json_preview(value: &Value, max_chars: usize) -> Value {
    let mut serialized = value.to_string();
    if serialized.chars().count() <= max_chars {
        return value.clone();
    }
    serialized = serialized.chars().take(max_chars).collect::<String>();
    json!({
        "preview": serialized,
        "truncated": true,
        "preview_max_chars": max_chars,
        "original_kind": match value {
            Value::Null => "null",
            Value::Bool(_) => "bool",
            Value::Number(_) => "number",
            Value::String(_) => "string",
            Value::Array(_) => "array",
            Value::Object(_) => "object",
        },
    })
}

fn bearwire_env_value() -> Option<String> {
    std::env::var("BEARS_BEARWIRE")
        .ok()
        .map(|value| value.trim().to_ascii_lowercase())
}

pub(crate) fn enabled() -> bool {
    match bearwire_env_value().as_deref() {
        None | Some("") | Some("auto") => true,
        Some("0" | "false" | "no" | "off" | "disabled") => false,
        Some(_) => env_bool("BEARS_BEARWIRE"),
    }
}

pub(crate) fn required() -> bool {
    true
}

pub(crate) fn mode_summary() -> String {
    let raw = std::env::var("BEARS_BEARWIRE").unwrap_or_else(|_| "<unset>".to_string());
    let mode = if required() {
        "required"
    } else if raw.trim().is_empty() || raw == "<unset>" || raw.trim().eq_ignore_ascii_case("auto") {
        "auto"
    } else if enabled() {
        "enabled"
    } else {
        "disabled"
    };
    format!(
        "{mode} (BEARS_BEARWIRE={raw}, BEARS_BEARWIRE_REQUIRED={})",
        required()
    )
}

pub(crate) async fn protocol_status(http: &reqwest::Client, config: &Config) -> String {
    if !enabled() {
        return format!("disabled; {}", mode_summary());
    }
    match rpc_call(http, config, "initialize", json!({})).await {
        Ok(value) => {
            let protocol = value
                .get("protocol")
                .and_then(Value::as_str)
                .unwrap_or("unknown");
            let version = value
                .get("version")
                .and_then(Value::as_i64)
                .map(|v| v.to_string())
                .unwrap_or_else(|| "unknown".to_string());
            let server_name = value
                .pointer("/server/name")
                .and_then(Value::as_str)
                .unwrap_or("unknown");
            let server_version = value
                .pointer("/server/version")
                .and_then(Value::as_str)
                .unwrap_or("unknown");
            let git_sha = value
                .pointer("/server/git_sha")
                .and_then(Value::as_str)
                .unwrap_or("unknown");
            let legacy = value
                .get("legacy_acp_enabled")
                .and_then(Value::as_bool)
                .map(|v| v.to_string())
                .unwrap_or_else(|| "unknown".to_string());
            let legacy_deprecated = value
                .get("legacy_acp_deprecated")
                .and_then(Value::as_bool)
                .map(|v| v.to_string())
                .unwrap_or_else(|| "unknown".to_string());
            let legacy_phase = value
                .get("legacy_acp_removal_phase")
                .and_then(Value::as_str)
                .unwrap_or("unknown");
            format!(
                "{}; Den advertises protocol={} version={} server={} {} git_sha={} legacy_acp_enabled={} legacy_acp_deprecated={} legacy_acp_removal_phase={}",
                mode_summary(),
                protocol,
                version,
                server_name,
                server_version,
                git_sha,
                legacy,
                legacy_deprecated,
                legacy_phase,
            )
        }
        Err(err) => format!("{}; initialize failed: {err:#}", mode_summary()),
    }
}

pub(crate) async fn validate_code_token(http: &reqwest::Client, config: &Config) -> Result<()> {
    let initialize = rpc_call(http, config, "initialize", json!({})).await?;
    if initialize.get("protocol").and_then(Value::as_str) != Some("bearwire")
        || initialize.get("version").and_then(Value::as_i64) != Some(1)
    {
        return Err(anyhow!(
            "Den did not advertise BearWire v1 support: {initialize}"
        ));
    }

    let result = rpc_call(
        http,
        config,
        "session.state",
        json!({
            "bear_slug": config.bear,
            "limit": 1,
        }),
    )
    .await?;
    if result.get("kind").and_then(Value::as_str).is_some() {
        Ok(())
    } else {
        Err(anyhow!(
            "BearWire session.state returned unexpected result: {result}"
        ))
    }
}

pub(crate) async fn post_session_open(
    http: &reqwest::Client,
    config: &Config,
    session_id: &str,
    client_context: Value,
    conversation_id: Option<&str>,
    requested_mode: &str,
) -> Result<Value> {
    let cwd = client_context
        .get("cwd")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);

    rpc_call(
        http,
        config,
        "session.open",
        json!({
            "bear_slug": config.bear,
            "session_id": session_id,
            "conversation_id": conversation_id,
            "client": config.client,
            "cwd": cwd,
            "mode": requested_mode,
            "adapter_contract": adapter_contract_context(),
            "client_context": client_context,
        }),
    )
    .await
    .context("BearWire session.open failed")
}

pub(crate) async fn handle_prompt(
    http: &reqwest::Client,
    config: &Config,
    adapter_state: &mut AdapterState,
    shared_state: &AdapterSharedState,
    response: crate::PromptResponseGuard,
    session_id: &str,
    prompt: &str,
    prompt_context: Value,
    client_context: Value,
    conversation_id: Option<&str>,
    requested_mode: &str,
    turn_token: Uuid,
) -> Result<()> {
    let session_result = post_session_open(
        http,
        config,
        session_id,
        client_context.clone(),
        conversation_id,
        requested_mode,
    )
    .await?;

    if crate::bear_debug_verbose() {
        eprintln!(
            "bear-armature: BearWire session.open ok session_id={} result={}",
            session_id,
            truncate_for_log(&session_result.to_string(), 360)
        );
    }
    if let Err(err) = crate::sync_session_model_from_den(
        http,
        Some(config),
        shared_state,
        adapter_state,
        session_id,
    )
    .await
    {
        eprintln!(
            "bear-armature: failed to sync model config after session.open session_id={} error={err:#}",
            session_id
        );
    }

    let cwd = client_context
        .get("cwd")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);

    let run_result = rpc_call(
        http,
        config,
        "run.start",
        json!({
            "bear_slug": config.bear,
            "session_id": session_id,
            "conversation_id": conversation_id,
            "client": config.client,
            "cwd": cwd,
            "prompt": prompt,
            "prompt_context": prompt_context,
            "requested_mode": requested_mode,
            "adapter_contract": adapter_contract_context(),
            "client_context": client_context,
        }),
    )
    .await
    .context("BearWire run.start failed")?;

    follow_run(
        http,
        config,
        adapter_state,
        shared_state,
        response,
        session_id,
        run_result,
        turn_token,
    )
    .await
}

fn cancellation_matches_prompt_turn(
    notice: &crate::CancellationNotice,
    session_id: &str,
    turn_token: Uuid,
) -> bool {
    notice.scope == crate::CancellationScope::PromptAndTools
        && notice.session_id == session_id
        && notice
            .turn_token
            .is_none_or(|cancelled_turn| cancelled_turn == turn_token)
}

fn tools_cancellation_notice(
    session_id: &str,
    turn_token: Uuid,
    origin: crate::CancellationOrigin,
) -> crate::CancellationNotice {
    crate::CancellationNotice {
        session_id: session_id.to_string(),
        turn_token: Some(turn_token),
        conversation_id: None,
        scope: crate::CancellationScope::ToolsOnly,
        origin,
    }
}

async fn settle_terminal_delivery_tools(
    shared_state: &AdapterSharedState,
    session_id: &str,
    turn_token: Uuid,
    reason: &str,
) -> Result<()> {
    let terminalization_result =
        crate::terminalize_tool_cards_for_turn(shared_state, session_id, turn_token, reason).await;
    let _ = shared_state.cancellation_tx.send(tools_cancellation_notice(
        session_id,
        turn_token,
        crate::CancellationOrigin::RunTerminal,
    ));
    shared_state
        .tool_tasks
        .cancel_turn(session_id, turn_token)
        .await;
    terminalization_result
}

fn terminal_event_tool_card_reason(current_run_id: &str, event: &Value) -> Option<&'static str> {
    if current_run_id != "<unknown>"
        && event_run_id(event).is_some_and(|event_run_id| event_run_id != current_run_id)
    {
        return None;
    }
    match event.get("type").and_then(Value::as_str) {
        Some("run.completed") => Some("Run completed; result unavailable."),
        Some("run.failed") => Some("Run failed; result unavailable."),
        Some("run.cancelled") => Some("Run cancelled; result unavailable."),
        Some("run.interrupted") => Some("Run interrupted; result unavailable."),
        _ => None,
    }
}

async fn handle_bearwire_event_with_terminal_cleanup(
    config: &Config,
    adapter_state: &mut AdapterState,
    shared_state: &AdapterSharedState,
    session_id: &str,
    current_run_id: &str,
    event: &Value,
    frame_sequence: Option<i64>,
    diagnostics: &mut SseStreamDiagnostics,
    turn_token: Uuid,
) -> Result<SseFrameOutcome> {
    let terminal_reason = terminal_event_tool_card_reason(current_run_id, event);
    let event_result = handle_bearwire_event(
        config,
        adapter_state,
        shared_state,
        session_id,
        current_run_id,
        event,
        frame_sequence,
        diagnostics,
        turn_token,
    )
    .await;
    if let Some(reason) = terminal_reason {
        settle_terminal_delivery_tools(shared_state, session_id, turn_token, reason).await?;
    }
    event_result
}

fn cancelled_run_frame_outcome() -> SseFrameOutcome {
    SseFrameOutcome {
        saw_done: true,
        saw_error: true,
        saw_visible_output: true,
        ..Default::default()
    }
}

async fn wait_for_matching_prompt_cancellation(
    cancellation_rx: &mut tokio::sync::broadcast::Receiver<crate::CancellationNotice>,
    session_id: &str,
    turn_token: Uuid,
) {
    loop {
        match cancellation_rx.recv().await {
            Ok(notice) if cancellation_matches_prompt_turn(&notice, session_id, turn_token) => {
                return
            }
            Ok(_) | Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
            Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                std::future::pending::<()>().await
            }
        }
    }
}

async fn report_missing_terminal_event(
    shared_state: &AdapterSharedState,
    session_id: &str,
    run_id: &str,
    turn_token: Uuid,
    outcome: RunTerminalOutcome,
    state_summary: &str,
) -> Result<()> {
    tracing::error!(
        target: "bear_armature::lifecycle",
        session_id,
        run_id,
        terminal_state = ?outcome,
        state = %state_summary,
        "BearWire protocol violation: canonical terminal run state has no matching terminal event"
    );
    eprintln!(
        "bear-armature: BearWire protocol violation session_id={} run_id={} terminal_state={:?} state={}",
        session_id,
        run_id,
        outcome,
        truncate_for_log(state_summary, 500),
    );
    let message = format!(
        "**Armature**: Den reported this run as {}, but its matching `{}` event was missing. The Armature stopped waiting to avoid leaving this prompt active. Run `{}`.",
        outcome.as_str(),
        outcome.event_type(),
        truncate_for_log(run_id, 120),
    );
    send_agent_message_chunk_for_turn(shared_state, session_id, turn_token, &message).await
}

async fn reconcile_run_state_projection(
    config: &Config,
    adapter_state: &mut AdapterState,
    shared_state: &AdapterSharedState,
    session_id: &str,
    run_id: &str,
    state: &Value,
    diagnostics: &mut SseStreamDiagnostics,
    turn_token: Uuid,
    deadline_expired: bool,
) -> Result<Option<SseFrameOutcome>> {
    let event = match reconciliation_decision(state, deadline_expired) {
        ReconciliationDecision::DeliverTerminalEvent(event) => Some(event),
        ReconciliationDecision::TerminalEventMissing(outcome) => {
            let summary = canonical_run_state_summary(state);
            settle_terminal_delivery_tools(
                shared_state,
                session_id,
                turn_token,
                outcome.tool_card_reason(),
            )
            .await?;
            report_missing_terminal_event(
                shared_state,
                session_id,
                run_id,
                turn_token,
                outcome,
                &summary,
            )
            .await?;
            if outcome.is_error() {
                return Err(anyhow!(
                    "BearWire protocol violation: terminal run state had no matching event. run_id={run_id}; {summary}"
                ));
            }
            let mut projected = SseFrameOutcome::default();
            projected.saw_done = true;
            projected.saw_visible_output = true;
            return Ok(Some(projected));
        }
        ReconciliationDecision::DeadlineExceeded => {
            let summary = canonical_run_state_summary(state);
            tracing::error!(
                target: "bear_armature::lifecycle",
                session_id,
                run_id,
                state = %summary,
                "BearWire delivery deadline expired while canonical run state remained nonterminal"
            );
            return Err(anyhow!(
                "BearWire delivery deadline expired after {}s while the run remained nonterminal. run_id={run_id}; {summary}",
                BEARWIRE_PROMPT_TIMEOUT.as_secs()
            ));
        }
        ReconciliationDecision::Continue => match decode_run_state(state) {
            DecodedRunState::Nonterminal => latest_terminal_event_from_run_state(state),
            DecodedRunState::Terminal(_) => None,
        },
    };

    match event {
        Some(event) => handle_bearwire_event_with_terminal_cleanup(
            config,
            adapter_state,
            shared_state,
            session_id,
            run_id,
            event,
            None,
            diagnostics,
            turn_token,
        )
        .await
        .map(Some),
        None => Ok(None),
    }
}

fn observe_frame_outcome(
    outcome: &SseFrameOutcome,
    saw_done: &mut bool,
    saw_visible_output: &mut bool,
    saw_tool_activity: &mut bool,
    saw_error: &mut bool,
) {
    *saw_done |= outcome.saw_done;
    *saw_visible_output |= outcome.saw_visible_output;
    *saw_tool_activity |= outcome.saw_tool_activity;
    *saw_error |= outcome.saw_error;
}

async fn reconcile_after_delivery_deadline(
    http: &reqwest::Client,
    config: &Config,
    adapter_state: &mut AdapterState,
    shared_state: &AdapterSharedState,
    response: crate::PromptResponseGuard,
    session_id: &str,
    run_id: &str,
    turn_token: Uuid,
) -> Result<()> {
    let reconciliation_result = if run_id == "<unknown>" {
        Err(anyhow!(
            "BearWire delivery deadline expired before run.start supplied a run_id"
        ))
    } else {
        let mut diagnostics = SseStreamDiagnostics::default();
        match tokio::time::timeout(BEARWIRE_DEADLINE_RECONCILIATION_TIMEOUT, async {
            let state = fetch_run_state(http, config, session_id, run_id)
                .await
                .context(
                    "BearWire delivery deadline expired and run.state reconciliation failed",
                )?;
            reconcile_run_state_projection(
                config,
                adapter_state,
                shared_state,
                session_id,
                run_id,
                &state,
                &mut diagnostics,
                turn_token,
                true,
            )
            .await
        })
        .await
        {
            Ok(result) => result,
            Err(_) => Err(anyhow!(
                "BearWire delivery deadline reconciliation exceeded {}s",
                BEARWIRE_DEADLINE_RECONCILIATION_TIMEOUT.as_secs()
            )),
        }
    };

    let terminalization_result = crate::terminalize_tool_cards_for_turn(
        shared_state,
        session_id,
        turn_token,
        "Delivery deadline exceeded; result unavailable.",
    )
    .await;
    let _ = shared_state.cancellation_tx.send(tools_cancellation_notice(
        session_id,
        turn_token,
        crate::CancellationOrigin::DeliveryDeadline,
    ));
    shared_state
        .tool_tasks
        .cancel_turn(session_id, turn_token)
        .await;

    terminalization_result?;
    let outcome = reconciliation_result?
        .expect("expired deadline reconciliation must produce a terminal outcome");
    if !outcome.saw_done {
        return Err(anyhow!(
            "BearWire deadline reconciliation did not terminate projection. run_id={run_id}"
        ));
    }
    if let Some(response_id) = response.claim() {
        crate::write_prompt_end_turn_response(response_id).await
    } else {
        Ok(())
    }
}

/// Project an already-started Den run into the current ACP prompt until Den
/// reaches a canonical prompt boundary. `/focus` uses this after the deep Den
/// command has selected and launched Docket-owned execution.
pub(crate) async fn follow_run(
    http: &reqwest::Client,
    config: &Config,
    adapter_state: &mut AdapterState,
    shared_state: &AdapterSharedState,
    response: crate::PromptResponseGuard,
    session_id: &str,
    run_result: Value,
    turn_token: Uuid,
) -> Result<()> {
    let mut cancellation_rx = shared_state.cancellation_tx.subscribe();
    if !crate::is_current_prompt_turn(shared_state, session_id, turn_token, "follow_run_start")
        .await
    {
        return Ok(());
    }
    let run_id = run_result
        .get("run_id")
        .or_else(|| run_result.pointer("/pair_binding/run/id"))
        .and_then(Value::as_str)
        .unwrap_or("<unknown>")
        .to_string();
    if run_id != "<unknown>"
        && !crate::bind_prompt_turn_run(shared_state, session_id, turn_token, &run_id).await
    {
        return Ok(());
    }
    let delivery = tokio::time::timeout(
        BEARWIRE_PROMPT_TIMEOUT,
        follow_run_inner(
            http,
            config,
            adapter_state,
            shared_state,
            response.clone(),
            session_id,
            run_result,
            turn_token,
        ),
    );
    tokio::select! {
        _ = wait_for_matching_prompt_cancellation(&mut cancellation_rx, session_id, turn_token) => Ok(()),
        result = delivery => match result {
            Ok(result) => result,
            Err(_) => reconcile_after_delivery_deadline(
                http,
                config,
                adapter_state,
                shared_state,
                response,
                session_id,
                &run_id,
                turn_token,
            ).await,
        },
    }
}

async fn follow_run_inner(
    http: &reqwest::Client,
    config: &Config,
    adapter_state: &mut AdapterState,
    shared_state: &AdapterSharedState,
    response: crate::PromptResponseGuard,
    session_id: &str,
    run_result: Value,
    turn_token: Uuid,
) -> Result<()> {
    let run_id = run_result
        .get("run_id")
        .or_else(|| run_result.pointer("/pair_binding/run/id"))
        .and_then(Value::as_str)
        .unwrap_or("<unknown>");
    let mut after = run_result
        .get("event_sequence")
        .and_then(Value::as_i64)
        .map(|sequence| sequence.saturating_sub(1));
    if crate::bear_debug_verbose() {
        eprintln!(
            "bear-armature: BearWire run.start accepted session_id={} run_id={} after={:?}",
            session_id, run_id, after
        );
    }

    let mut diagnostics = SseStreamDiagnostics::default();
    let mut saw_done = false;
    let mut saw_visible_output = false;
    let mut saw_tool_activity = false;
    let mut saw_error = false;
    let started = Instant::now();
    let mut last_poll_log = Instant::now();
    let mut last_obligation_sync: Option<Instant> = None;
    let mut last_run_state_diagnostic_log = Instant::now();
    let mut last_run_state_summary: Option<String> = None;
    let mut logged_initial_wait = false;
    let mut consecutive_fetch_errors = 0usize;
    let mut first_fetch_error_at: Option<Instant> = None;

    'poll: loop {
        // Own this session's ACP projection lane before fetching the page. A
        // detached local tool can keep executing, but cannot emit an ACP update
        // ahead of lifecycle frames that are already in flight.
        let page_projection = shared_state
            .projection_dispatcher
            .begin_live_page(session_id, turn_token)
            .await;
        let replay = match fetch_events(http, config, session_id, after).await {
            Ok(replay) => {
                consecutive_fetch_errors = 0;
                first_fetch_error_at = None;
                replay
            }
            Err(err) => {
                consecutive_fetch_errors += 1;
                diagnostics.fetch_errors += 1;
                let failure_started = *first_fetch_error_at.get_or_insert_with(Instant::now);
                tracing::warn!(
                    target: "bear_armature::lifecycle",
                    session_id,
                    run_id,
                    after = ?after,
                    consecutive_fetch_errors,
                    error = %err,
                    "BearWire event fetch failed; reconciling canonical run state"
                );

                let mut state_reachable = false;
                if run_id != "<unknown>" {
                    match fetch_run_state(http, config, session_id, run_id).await {
                        Ok(state) => {
                            state_reachable = true;
                            let reconciled = reconcile_run_state_projection(
                                config,
                                adapter_state,
                                shared_state,
                                session_id,
                                run_id,
                                &state,
                                &mut diagnostics,
                                turn_token,
                                false,
                            )
                            .await?;
                            if let Some(outcome) = reconciled {
                                observe_frame_outcome(
                                    &outcome,
                                    &mut saw_done,
                                    &mut saw_visible_output,
                                    &mut saw_tool_activity,
                                    &mut saw_error,
                                );
                                if saw_done {
                                    break 'poll;
                                }
                            }
                            service_run_state_tool_obligations(
                                config,
                                shared_state,
                                session_id,
                                run_id,
                                &state,
                                turn_token,
                            )
                            .await?;
                        }
                        Err(state_err) => {
                            tracing::debug!(
                                target: "bear_armature::lifecycle",
                                session_id,
                                run_id,
                                error = %state_err,
                                "BearWire run.state reconciliation failed after event fetch error"
                            );
                        }
                    }
                }

                let command_active = shared_state
                    .tool_tasks
                    .has_active_execution(session_id)
                    .await;
                if !command_active
                    && !state_reachable
                    && failure_started.elapsed() >= BEARWIRE_EVENT_FETCH_FAILURE_GRACE
                {
                    tracing::error!(
                        target: "bear_armature::lifecycle",
                        session_id,
                        run_id,
                        after = ?after,
                        consecutive_fetch_errors,
                        outage_duration_ms = failure_started.elapsed().as_millis(),
                        event_fetch_error = %err,
                        "Den API is unavailable; event delivery and run-state reconciliation could not recover"
                    );
                    return Err(err).context(
                        "Den API connectivity failure: BearWire event delivery and run.state reconciliation both failed during the recovery grace period",
                    );
                }
                drop(page_projection);
                sleep(event_fetch_retry_delay(consecutive_fetch_errors)).await;
                continue;
            }
        };
        let replay_count = replay.frames.len();
        let next_after = replay.next_after;
        for frame in replay.frames {
            let sequence = frame.sequence;
            page_projection.observe_frame(sequence);
            let Some(event) = frame.event else {
                continue;
            };
            let event_result = handle_bearwire_event_with_terminal_cleanup(
                config,
                adapter_state,
                shared_state,
                session_id,
                run_id,
                &event,
                sequence,
                &mut diagnostics,
                turn_token,
            )
            .await;
            let outcome = match event_result {
                Ok(outcome) => outcome,
                Err(err) if event.get("type").and_then(Value::as_str) == Some("run.failed") => {
                    return Err(err);
                }
                Err(err) => {
                    diagnostics.observe_event_error(&event, &err);
                    tracing::warn!(
                        target: "bear_armature::lifecycle",
                        session_id,
                        run_id,
                        event_type = event.get("type").and_then(|value| value.as_str()).unwrap_or("<missing>"),
                        error = %err,
                        sample = %truncate_for_log(&event.to_string(), 360),
                        "BearWire event handling failed; skipping non-terminal event"
                    );
                    continue;
                }
            };
            saw_done |= outcome.saw_done;
            saw_visible_output |= outcome.saw_visible_output;
            saw_tool_activity |= outcome.saw_tool_activity;
            saw_error |= outcome.saw_error;
            if saw_done {
                break;
            }
        }
        after = next_after;
        if saw_done {
            if crate::bear_debug_verbose() {
                eprintln!(
                    "bear-armature: BearWire run terminal event received session_id={} run_id={} diagnostics={}",
                    session_id,
                    run_id,
                    diagnostics.summary()
                );
            }
            break;
        }
        if !logged_initial_wait
            && started.elapsed() >= Duration::from_secs(5)
            && !saw_visible_output
            && !saw_tool_activity
            && !saw_error
        {
            logged_initial_wait = true;
            if crate::bear_debug_verbose() {
                eprintln!(
                    "bear-armature: BearWire still waiting for first visible/tool event session_id={} run_id={} after={:?} elapsed_ms={} diagnostics={}",
                    session_id,
                    run_id,
                    after,
                    started.elapsed().as_millis(),
                    diagnostics.summary()
                );
            }
        }
        if crate::bear_debug_verbose() && last_poll_log.elapsed() >= Duration::from_secs(5) {
            eprintln!(
                "bear-armature: BearWire polling session_id={} run_id={} after={:?} replay_frames={} elapsed_ms={} diagnostics={}",
                session_id,
                run_id,
                after,
                replay_count,
                started.elapsed().as_millis(),
                diagnostics.summary()
            );
            last_poll_log = Instant::now();
        }
        let should_sync_obligations = run_id != "<unknown>"
            && last_obligation_sync
                .map(|last_sync| last_sync.elapsed() >= BEARWIRE_OBLIGATION_SYNC_INTERVAL)
                .unwrap_or(true);
        if should_sync_obligations {
            last_obligation_sync = Some(Instant::now());
            match fetch_run_state(http, config, session_id, run_id).await {
                Ok(state) => {
                    let reconciled = reconcile_run_state_projection(
                        config,
                        adapter_state,
                        shared_state,
                        session_id,
                        run_id,
                        &state,
                        &mut diagnostics,
                        turn_token,
                        false,
                    )
                    .await?;
                    if let Some(outcome) = reconciled {
                        observe_frame_outcome(
                            &outcome,
                            &mut saw_done,
                            &mut saw_visible_output,
                            &mut saw_tool_activity,
                            &mut saw_error,
                        );
                        if saw_done {
                            break 'poll;
                        }
                    }
                    if let Some(summary) = run_state_obligation_summary(&state) {
                        let should_log_summary = last_run_state_summary.as_deref()
                            != Some(summary.as_str())
                            || last_run_state_diagnostic_log.elapsed()
                                >= BEARWIRE_RUN_STATE_DIAGNOSTIC_INTERVAL;
                        if should_log_summary {
                            last_run_state_diagnostic_log = Instant::now();
                            tracing::trace!(
                                target: "bear_armature::lifecycle",
                                session_id,
                                run_id,
                                summary = %summary,
                                "BearWire run.state reports active client obligations"
                            );
                            if crate::bear_debug_verbose() {
                                eprintln!(
                                    "bear-armature: BearWire run.state obligations session_id={} run_id={} {}",
                                    session_id, run_id, summary
                                );
                            }
                            last_run_state_summary = Some(summary);
                        }
                    } else {
                        last_run_state_summary = None;
                    }
                    service_run_state_tool_obligations(
                        config,
                        shared_state,
                        session_id,
                        run_id,
                        &state,
                        turn_token,
                    )
                    .await?;
                }
                Err(err) => {
                    tracing::debug!(
                        target: "bear_armature::lifecycle",
                        session_id,
                        run_id,
                        error = %err,
                        "BearWire run.state obligation sync failed"
                    );
                }
            }
        }
        drop(page_projection);
        sleep(BEARWIRE_POLL_INTERVAL).await;
    }

    if !saw_done {
        return Err(anyhow!(
            "BearWire follow_run exited without a terminal delivery decision. run_id={run_id}. Diagnostics: {}",
            diagnostics.summary()
        ));
    }

    if saw_done && !saw_visible_output {
        match classify_completed_turn_without_text(saw_error, saw_tool_activity) {
            CompletedTurnWithoutText::Expected => {
                tracing::debug!(
                    target: "bear_armature::lifecycle",
                    session_id,
                    run_id,
                    diagnostics = %diagnostics.summary(),
                    "BearWire run completed without visible assistant output after tool activity"
                );
            }
            CompletedTurnWithoutText::Anomalous => {
                let reason = if saw_error {
                    "tool calls failed"
                } else {
                    "no tool activity or assistant output was observed"
                };
                let message = format!(
                    "**Armature**: Den completed this turn without an assistant response (run `{run_id}`); {reason}. Check the diagnostics above or send a follow-up message."
                );
                tracing::warn!(
                    target: "bear_armature::lifecycle",
                    session_id,
                    run_id,
                    diagnostics = %diagnostics.summary(),
                    "BearWire run completed without visible assistant output"
                );
                eprintln!(
                    "bear-armature: BearWire run completed without visible assistant output session_id={} run_id={} reason={} diagnostics={}",
                    session_id,
                    run_id,
                    reason,
                    diagnostics.summary()
                );
                send_agent_message_chunk_for_turn(shared_state, session_id, turn_token, &message)
                    .await?;
            }
        }
    }

    if let Some(response_id) = response.claim() {
        crate::write_prompt_end_turn_response(response_id).await
    } else {
        Ok(())
    }
}

pub(crate) async fn post_session_close(config: &Config, session_id: &str) -> Result<Value> {
    rpc_call(
        &reqwest::Client::new(),
        config,
        "session.close",
        json!({
            "bear_slug": config.bear,
            "session_id": session_id,
            "adapter_contract": adapter_contract_context(),
        }),
    )
    .await
}

pub(crate) async fn post_session_compact(config: &Config, session_id: &str) -> Result<Value> {
    rpc_call(
        &reqwest::Client::new(),
        config,
        "session.compact",
        json!({
            "bear_slug": config.bear,
            "session_id": session_id,
            "adapter_contract": adapter_contract_context(),
        }),
    )
    .await
}

pub(crate) async fn post_run_cancel(
    config: &Config,
    session_id: &str,
    run_id: &str,
) -> Result<Value> {
    rpc_call(
        &reqwest::Client::new(),
        config,
        "run.cancel",
        json!({
            "bear_slug": config.bear,
            "session_id": session_id,
            "run_id": run_id,
            "adapter_contract": adapter_contract_context(),
        }),
    )
    .await
}

pub(crate) async fn post_resource_update(
    config: &Config,
    session_id: &str,
    resource: Value,
) -> Result<Value> {
    rpc_call(
        &reqwest::Client::new(),
        config,
        "resource.update",
        json!({
            "bear_slug": config.bear,
            "session_id": session_id,
            "resource": resource,
            "adapter_contract": adapter_contract_context(),
        }),
    )
    .await
}

pub(crate) fn tool_result_rpc_params(
    config: &Config,
    session_id: &str,
    run_id: &str,
    tool_call_id: &str,
    payload: &Value,
    attempt_token: Option<&str>,
) -> Value {
    let status = payload
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or("ok");
    let error = if status == "ok" {
        Value::Null
    } else {
        payload
            .get("error")
            .cloned()
            .unwrap_or_else(|| payload.get("diagnostic").cloned().unwrap_or(Value::Null))
    };
    json!({
        "bear_slug": config.bear,
        "session_id": session_id,
        "run_id": run_id,
        "tool_call_id": tool_call_id,
        "tool_name": payload.get("tool_name").cloned().unwrap_or(Value::Null),
        "status": status,
        "content": payload.get("content").cloned().unwrap_or(Value::Null),
        "structured_content": payload.get("structured_content").cloned().unwrap_or(Value::Null),
        "diagnostic": payload.get("diagnostic").cloned().unwrap_or(Value::Null),
        "error": error,
        "attempt_token": attempt_token,
        "adapter_contract": adapter_contract_context(),
    })
}

pub(crate) async fn post_tool_result(
    config: &Config,
    session_id: &str,
    run_id: &str,
    tool_call_id: &str,
    payload: Value,
    attempt_token: Option<&str>,
) -> Result<Value> {
    rpc_call(
        &reqwest::Client::new(),
        config,
        "client.tool.result",
        tool_result_rpc_params(
            config,
            session_id,
            run_id,
            tool_call_id,
            &payload,
            attempt_token,
        ),
    )
    .await
}

pub(crate) async fn claim_tool_execution(
    config: &Config,
    session_id: &str,
    run_id: &str,
    obligation_id: &str,
    tool_call_id: &str,
) -> Result<Value> {
    rpc_call(
        &reqwest::Client::new(),
        config,
        "client.tool.claim",
        json!({
            "bear_slug": config.bear,
            "session_id": session_id,
            "run_id": run_id,
            "obligation_id": obligation_id,
            "tool_call_id": tool_call_id,
            "adapter_contract": adapter_contract_context(),
        }),
    )
    .await
}

pub(crate) async fn renew_tool_execution(
    config: &Config,
    session_id: &str,
    run_id: &str,
    obligation_id: &str,
    tool_call_id: &str,
    attempt_token: &str,
) -> Result<Value> {
    rpc_call(
        &reqwest::Client::new(),
        config,
        "client.tool.renew",
        json!({
            "bear_slug": config.bear,
            "session_id": session_id,
            "run_id": run_id,
            "obligation_id": obligation_id,
            "tool_call_id": tool_call_id,
            "attempt_token": attempt_token,
            "adapter_contract": adapter_contract_context(),
        }),
    )
    .await
}

async fn fetch_run_state(
    http: &reqwest::Client,
    config: &Config,
    session_id: &str,
    run_id: &str,
) -> Result<Value> {
    rpc_call(
        http,
        config,
        "run.state",
        json!({
            "bear_slug": config.bear,
            "session_id": session_id,
            "run_id": run_id,
            "limit": 20,
        }),
    )
    .await
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RunTerminalOutcome {
    Completed,
    Failed,
    Cancelled,
}

impl RunTerminalOutcome {
    fn from_persisted_state(state: &str) -> Option<Self> {
        match state {
            "completed" => Some(Self::Completed),
            "failed" => Some(Self::Failed),
            "cancelled" => Some(Self::Cancelled),
            _ => None,
        }
    }

    fn event_type(self) -> &'static str {
        match self {
            Self::Completed => "run.completed",
            Self::Failed => "run.failed",
            Self::Cancelled => "run.cancelled",
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }

    fn tool_card_reason(self) -> &'static str {
        match self {
            Self::Completed => "Run completed; result unavailable.",
            Self::Failed => "Run failed; result unavailable.",
            Self::Cancelled => "Run cancelled; result unavailable.",
        }
    }

    fn is_error(self) -> bool {
        self == Self::Failed
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DecodedRunState {
    Nonterminal,
    Terminal(RunTerminalOutcome),
}

#[derive(Debug, PartialEq)]
enum ReconciliationDecision<'a> {
    Continue,
    DeliverTerminalEvent(&'a Value),
    TerminalEventMissing(RunTerminalOutcome),
    DeadlineExceeded,
}

fn recent_run_events(state: &Value) -> impl DoubleEndedIterator<Item = &Value> {
    state
        .get("recent_events")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|entry| entry.get("event").or(Some(entry)))
}

fn latest_terminal_event_from_run_state(state: &Value) -> Option<&Value> {
    recent_run_events(state).rev().find(|event| {
        matches!(
            event.get("type").and_then(Value::as_str),
            Some("run.completed" | "run.failed" | "run.cancelled" | "run.interrupted")
        )
    })
}

fn decode_run_state(state: &Value) -> DecodedRunState {
    state
        .pointer("/run/state")
        .and_then(Value::as_str)
        .and_then(RunTerminalOutcome::from_persisted_state)
        .map(DecodedRunState::Terminal)
        .unwrap_or(DecodedRunState::Nonterminal)
}

fn reconciliation_decision(state: &Value, deadline_expired: bool) -> ReconciliationDecision<'_> {
    let has_open_obligations = state
        .get("open_obligations")
        .and_then(Value::as_array)
        .is_some_and(|obligations| !obligations.is_empty());

    if let DecodedRunState::Terminal(outcome) = decode_run_state(state) {
        if !has_open_obligations {
            let matching_event = recent_run_events(state).rev().find(|event| {
                event.get("type").and_then(Value::as_str) == Some(outcome.event_type())
            });
            return matching_event.map_or(
                ReconciliationDecision::TerminalEventMissing(outcome),
                ReconciliationDecision::DeliverTerminalEvent,
            );
        }
    }

    if deadline_expired {
        ReconciliationDecision::DeadlineExceeded
    } else {
        ReconciliationDecision::Continue
    }
}

fn event_fetch_retry_delay(consecutive_errors: usize) -> Duration {
    let exponent = consecutive_errors.saturating_sub(1).min(5) as u32;
    BEARWIRE_POLL_INTERVAL
        .saturating_mul(1_u32 << exponent)
        .min(BEARWIRE_EVENT_FETCH_MAX_BACKOFF)
}

fn canonical_run_state_summary(state: &Value) -> String {
    let run_state = state
        .pointer("/run/state")
        .and_then(Value::as_str)
        .unwrap_or("<missing>");
    let terminal_reason = state
        .pointer("/run/terminal_reason")
        .and_then(Value::as_str)
        .unwrap_or("<none>");
    let open_obligations = state
        .get("open_obligations")
        .and_then(Value::as_array)
        .map_or(0, Vec::len);
    let recent_event_types = state
        .get("recent_events")
        .and_then(Value::as_array)
        .map(|events| {
            events
                .iter()
                .filter_map(|entry| entry.get("event").or(Some(entry)))
                .filter_map(|event| event.get("type").and_then(Value::as_str))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    format!(
        "state={run_state}, terminal_reason={terminal_reason}, open_obligations={open_obligations}, recent_event_types={recent_event_types:?}"
    )
}

fn run_state_obligation_summary(state: &Value) -> Option<String> {
    let run_state = state
        .pointer("/run/state")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    let open = state
        .get("open_obligations")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    if open.is_empty() {
        return None;
    }
    let obligations = open
        .iter()
        .take(5)
        .map(|obligation| {
            let id = obligation
                .get("id")
                .or_else(|| obligation.get("obligation_id"))
                .and_then(Value::as_str)
                .unwrap_or("<unknown>");
            let state = obligation
                .get("state")
                .and_then(Value::as_str)
                .unwrap_or("<unknown>");
            let kind = obligation
                .get("kind")
                .and_then(Value::as_str)
                .unwrap_or("<unknown>");
            let expected = obligation
                .get("expected_responder_action")
                .and_then(Value::as_str)
                .unwrap_or("<unknown>");
            let tool_call = obligation
                .get("tool_call_id")
                .and_then(Value::as_str)
                .unwrap_or("<none>");
            let permission = obligation
                .get("permission_id")
                .and_then(Value::as_str)
                .unwrap_or("<none>");
            let updated = obligation
                .get("updated_at")
                .and_then(Value::as_str)
                .unwrap_or("<unknown>");
            format!(
                "id={id} state={state} kind={kind} expected={expected} tool_call={tool_call} permission={permission} updated_at={updated}"
            )
        })
        .collect::<Vec<_>>()
        .join("; ");
    Some(format!(
        "run_state={run_state} open_obligation_count={} open_obligations=[{}]",
        open.len(),
        obligations
    ))
}

fn obligation_request_payload<'a>(obligation: &'a Value) -> &'a Value {
    obligation
        .get("request_payload")
        .filter(|value| value.is_object())
        .unwrap_or(obligation)
}

fn obligation_id(obligation: &Value) -> Option<&str> {
    obligation
        .get("id")
        .or_else(|| obligation.get("obligation_id"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

fn obligation_open_for_client(obligation: &Value) -> bool {
    matches!(
        obligation
            .get("state")
            .and_then(Value::as_str)
            .unwrap_or_default(),
        "requested" | "waiting_for_client"
    )
}

fn obligation_execution_target_is_den(request: &Value, obligation: &Value) -> bool {
    request
        .get("execution_target")
        .or_else(|| obligation.get("execution_target"))
        .and_then(Value::as_str)
        .is_some_and(|target| target == "den")
}

fn policy_allows_approval_free_obligation_service(request: &Value, tool_name: &str) -> bool {
    let Some(policy) = request.get("policy").filter(|value| value.is_object()) else {
        return legacy_approval_free_read_only_tool(tool_name);
    };
    policy
        .get("execution_target")
        .and_then(Value::as_str)
        .is_some_and(|target| target == "armature_local")
        && policy
            .get("approval_required")
            .and_then(Value::as_bool)
            .is_some_and(|required| !required)
        && policy
            .get("approval_policy")
            .and_then(Value::as_str)
            .is_some_and(|policy| policy == "never")
        && policy
            .get("risk")
            .and_then(Value::as_str)
            .is_some_and(|risk| risk == "read_only")
}

fn actionable_tool_request_event_from_obligation(
    run_id: &str,
    obligation: &Value,
) -> Option<Value> {
    if !obligation_open_for_client(obligation) {
        return None;
    }
    let request = obligation_request_payload(obligation);
    if obligation_execution_target_is_den(request, obligation) {
        return None;
    }
    let expected = obligation
        .get("expected_responder_action")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let kind = obligation
        .get("kind")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let tool_name = request
        .get("tool_name")
        .or_else(|| obligation.get("tool_name"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())?;
    let tool_call_id = request
        .get("tool_call_id")
        .or_else(|| obligation.get("tool_call_id"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())?;
    let arguments = request
        .get("arguments")
        .cloned()
        .unwrap_or_else(|| json!({}));

    if expected == "permission_decision" || kind == "permission_decision" {
        let permission_id = request
            .get("approval_request_id")
            .or_else(|| obligation.get("permission_id"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())?;
        let obligation_id = obligation_id(obligation)?;
        return Some(json!({
            "type": "client.waiting",
            "run_id": run_id,
            "data": {
                "expected_responder_action": "permission_decision",
                "expected_client_method": "client.permission.result",
                "obligation_id": obligation_id,
                "tool_call": {
                    "id": tool_call_id,
                    "name": tool_name,
                    "kind": "function",
                    "arguments": arguments.clone(),
                },
                "permission": {
                    "id": permission_id,
                    "reason": request.get("approval_reason").cloned().unwrap_or_else(|| json!("BEARS requests permission.")),
                    "target": arguments,
                },
                "approval_required": true,
                "execution_target": "armature_local",
                "policy": request.get("policy").cloned().unwrap_or(Value::Null),
                "serviced_from_run_state": true,
            }
        }));
    }

    if expected != "tool_result" && kind != "tool_result" {
        return None;
    }
    if request
        .get("approval_required")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        return None;
    }
    let permission_granted = request
        .get("permission_granted")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if !permission_granted && !policy_allows_approval_free_obligation_service(request, tool_name) {
        return None;
    }
    Some(json!({
        "type": "tool_call.requested",
        "run_id": run_id,
        "data": {
            "obligation_id": obligation_id(obligation)?,
            "tool_call": {
                "id": tool_call_id,
                "name": tool_name,
                "kind": "function",
                "arguments": arguments,
            },
            "approval_required": false,
            "execution_target": "armature_local",
            "policy": request.get("policy").cloned().unwrap_or(Value::Null),
            "serviced_from_run_state": true,
        }
    }))
}

fn unsupported_required_client_obligation_error(obligation: &Value) -> Option<anyhow::Error> {
    if !obligation_open_for_client(obligation) {
        return None;
    }
    let request = obligation_request_payload(obligation);
    if obligation_execution_target_is_den(request, obligation) {
        return None;
    }

    let id = obligation_id(obligation).unwrap_or("<unknown>");
    let kind = obligation
        .get("kind")
        .and_then(Value::as_str)
        .unwrap_or("<unknown>");
    let expected = obligation
        .get("expected_responder_action")
        .or_else(|| obligation.get("expected_client_method"))
        .and_then(Value::as_str)
        .unwrap_or("<unknown>");
    let tool_call_id = request
        .get("tool_call_id")
        .or_else(|| obligation.get("tool_call_id"))
        .and_then(Value::as_str)
        .unwrap_or("<none>");
    let permission_id = request
        .get("approval_request_id")
        .or_else(|| obligation.get("permission_id"))
        .and_then(Value::as_str)
        .unwrap_or("<none>");
    let sample = truncate_for_log(&obligation.to_string(), 1000);

    if (expected == "tool_result" || kind == "tool_result")
        && request
            .get("approval_required")
            .and_then(Value::as_bool)
            .unwrap_or(false)
    {
        return Some(anyhow!(
            "BearWire invariant violation: tool_result obligation remains approval_required after permission settlement: id={id} tool_call={tool_call_id} permission={permission_id} sample={sample}"
        ));
    }

    Some(anyhow!(
        "unsupported required BearWire client obligation: id={id} kind={kind} expected={expected} tool_call={tool_call_id} permission={permission_id} sample={sample}"
    ))
}

async fn service_run_state_tool_obligations(
    config: &Config,
    shared_state: &AdapterSharedState,
    session_id: &str,
    run_id: &str,
    state: &Value,
    turn_token: Uuid,
) -> Result<()> {
    let Some(open) = state.get("open_obligations").and_then(Value::as_array) else {
        return Ok(());
    };
    for obligation in open {
        let Some(event) = actionable_tool_request_event_from_obligation(run_id, obligation) else {
            if let Some(err) = unsupported_required_client_obligation_error(obligation) {
                // A run.state snapshot can race permission/result settlement. Treat an
                // obligation this client cannot service as a reconciliation warning;
                // the next state poll can observe the settled obligation. Ending the
                // model turn here turns an approval redirect into a sandbox failure.
                tracing::warn!(
                    target: "bear_armature::lifecycle",
                    session_id,
                    run_id,
                    error = %err,
                    "cannot service BearWire obligation from this run.state snapshot"
                );
            }
            continue;
        };
        let tool_call_id = event
            .pointer("/data/tool_call/id")
            .and_then(Value::as_str)
            .unwrap_or("<unknown>");
        let tool_name = event
            .pointer("/data/tool_call/name")
            .and_then(Value::as_str)
            .unwrap_or("<unknown>");
        if shared_state
            .tool_tasks
            .get(session_id, tool_call_id)
            .await
            .is_some()
        {
            continue;
        }
        tracing::trace!(
            target: "bear_armature::lifecycle",
            session_id,
            run_id,
            tool_call_id,
            tool_name,
            event_type = event.get("type").and_then(|value| value.as_str()).unwrap_or("<unknown>"),
            "servicing local client obligation from BearWire run.state"
        );
        if event.get("type").and_then(Value::as_str) == Some("client.waiting") {
            let mut task_state = AdapterState {
                client_capabilities: shared_state.client_capabilities.lock().await.clone(),
                session_contexts: shared_state.session_contexts.lock().await.clone(),
                transport: shared_state.transport.clone(),
            };
            if let Err(err) = handle_permission_request_event(
                config,
                &mut task_state,
                shared_state,
                session_id,
                &event,
                turn_token,
            )
            .await
            {
                tracing::warn!(
                    target: "bear_armature::lifecycle",
                    session_id,
                    run_id,
                    tool_call_id,
                    tool_name,
                    error = %err,
                    "failed to service permission obligation from BearWire run.state"
                );
            }
        } else if is_den_server_tool_request(&event) {
            project_den_owned_tool_request(shared_state, session_id, &event, turn_token).await?;
        } else {
            spawn_tool_request_task(
                config.clone(),
                shared_state.clone(),
                session_id.to_string(),
                event,
                turn_token,
            );
        }
    }
    Ok(())
}

pub(crate) async fn post_permission_result(
    config: &Config,
    session_id: &str,
    run_id: &str,
    permission_id: &str,
    payload: Value,
) -> Result<Value> {
    rpc_call(
        &reqwest::Client::new(),
        config,
        "client.permission.result",
        json!({
            "bear_slug": config.bear,
            "session_id": session_id,
            "run_id": run_id,
            "permission_id": permission_id,
            "obligation_id": payload.get("obligation_id").cloned().unwrap_or(Value::Null),
            "decision": payload.get("decision").and_then(Value::as_str).unwrap_or("denied"),
            "reason": payload.get("reason").cloned().unwrap_or(Value::Null),
            "adapter_contract": adapter_contract_context(),
        }),
    )
    .await
}

pub(crate) async fn try_handle_prompt(
    http: &reqwest::Client,
    config: &Config,
    adapter_state: &mut AdapterState,
    shared_state: &AdapterSharedState,
    response: crate::PromptResponseGuard,
    session_id: &str,
    prompt: &str,
    prompt_context: Value,
    client_context: Value,
    conversation_id: Option<&str>,
    requested_mode: &str,
    turn_token: Uuid,
) -> Result<bool> {
    if !enabled() {
        return Err(anyhow!(
            "BearWire is disabled in this adapter process, and legacy ACP HTTP is retired. Enable BearWire by setting BEARS_BEARWIRE=auto or true."
        ));
    }
    match handle_prompt(
        http,
        config,
        adapter_state,
        shared_state,
        response,
        session_id,
        prompt,
        prompt_context,
        client_context,
        conversation_id,
        requested_mode,
        turn_token,
    )
    .await
    {
        Ok(()) => Ok(true),
        Err(err) => Err(err),
    }
}

pub(crate) async fn rpc_call(
    http: &reqwest::Client,
    config: &Config,
    method: &str,
    params: Value,
) -> Result<Value> {
    let url = format!("{}/bearwire/v1/rpc", config.api_url);
    let response = http
        .post(&url)
        .header(
            AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {}", config.token))?,
        )
        .header(CONTENT_TYPE, HeaderValue::from_static("application/json"))
        .json(&json!({
            "jsonrpc": "2.0",
            "id": format!("bearwire-{}", Uuid::new_v4().simple()),
            "method": method,
            "params": params,
        }))
        .send()
        .await
        .with_context(|| den_request_context(&url))?;
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(anyhow!(
            "BearWire RPC {method} HTTP {status}: {}",
            body.trim()
        ));
    }
    let value: Value = serde_json::from_str(&body)
        .with_context(|| format!("parse BearWire RPC {method} JSON: {body}"))?;
    if let Some(error) = value.get("error") {
        return Err(anyhow!("BearWire RPC {method} error: {error}"));
    }
    Ok(value.get("result").cloned().unwrap_or(Value::Null))
}

struct BearWireReplay {
    frames: Vec<BearWireFrame>,
    next_after: Option<i64>,
}

struct BearWireFrame {
    sequence: Option<i64>,
    event: Option<Value>,
}

async fn fetch_events(
    http: &reqwest::Client,
    config: &Config,
    session_id: &str,
    after: Option<i64>,
) -> Result<BearWireReplay> {
    fetch_event_page(http, config, session_id, after).await
}

async fn fetch_event_page(
    http: &reqwest::Client,
    config: &Config,
    session_id: &str,
    after: Option<i64>,
) -> Result<BearWireReplay> {
    let mut url = format!(
        "{}/bearwire/v1/sessions/{}/events/page?bear_slug={}",
        config.api_url,
        urlencoding::encode(session_id),
        urlencoding::encode(&config.bear)
    );
    if let Some(after) = after {
        url.push_str("&after=");
        url.push_str(&after.to_string());
    }
    let response = http
        .get(&url)
        .header(
            AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {}", config.token))?,
        )
        .send()
        .await
        .with_context(|| den_request_context(&url))?;
    let status = response.status();

    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        return Err(anyhow!(
            "BearWire event page HTTP {status}: {}",
            body.trim()
        ));
    }
    let body = response.text().await.unwrap_or_default();
    parse_event_page(&body, after)
        .with_context(|| format!("parse BearWire event page JSON: {body}"))
}

fn bearwire_plan_update_entries(event: &Value) -> Value {
    let data = event.get("data").unwrap_or(&Value::Null);
    data.pointer("/detail/entries")
        .or_else(|| data.pointer("/detail/plan/items"))
        .or_else(|| data.pointer("/plan/items"))
        .or_else(|| data.get("entries"))
        .cloned()
        .unwrap_or_else(|| json!([]))
}

fn format_den_status(message: &str) -> String {
    let message = message.trim();
    let message = message
        .strip_prefix("I stopped because ")
        .map(|rest| format!("This turn stopped because {rest}"))
        .or_else(|| {
            message
                .strip_prefix("I stopped ")
                .map(|rest| format!("This turn stopped {rest}"))
        })
        .unwrap_or_else(|| message.to_owned());
    format!("**Den**: {message}")
}

fn local_step_claim_timeout_user_message(data: &Value) -> Option<&'static str> {
    if data.get("reason").and_then(Value::as_str) != Some("client_obligation_timeout") {
        return None;
    }

    let obligations = data
        .pointer("/context/affected_obligations")
        .or_else(|| data.get("affected_obligations"))
        .and_then(Value::as_array)?;
    let local_step_never_started = obligations.iter().any(|obligation| {
        obligation
            .get("expected_responder_action")
            .and_then(Value::as_str)
            == Some("tool_result")
            && obligation.get("claimed").and_then(Value::as_bool) == Some(false)
    });
    local_step_never_started.then_some(
        "Builder Bear could not begin a local workspace operation before it timed out. No local operation was started, so send another message to retry.",
    )
}

fn bearwire_run_failed_user_message(event: &Value) -> String {
    let data = event.get("data").unwrap_or(&Value::Null);
    let local_step_timeout_message = local_step_claim_timeout_user_message(data);
    let user_message = data
        .get("user_message")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|message| !message.is_empty());
    let message = data
        .get("message")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|message| !message.is_empty())
        .unwrap_or("BearWire run failed");
    let reason = data
        .get("reason")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|reason| !reason.is_empty());
    let error_type = data
        .get("error_type")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|error_type| !error_type.is_empty());
    let detail = data
        .get("detail")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|detail| !detail.is_empty() && *detail != message);
    let run_id = event
        .get("run_id")
        .and_then(Value::as_str)
        .or_else(|| data.get("run_id").and_then(Value::as_str));
    let mut rendered = if let Some(message) = local_step_timeout_message {
        format_den_status(message)
    } else if let Some(user_message) = user_message {
        format_den_status(user_message)
    } else if let Some(reason) = reason {
        format_den_status(&format!("Run failed ({reason}): {message}"))
    } else {
        format_den_status(&format!("Run failed: {message}"))
    };
    if user_message.is_none() {
        if let Some(error_type) = error_type {
            rendered.push_str(&format!("\n\nError type: `{error_type}`"));
        }
        if let Some(detail) = detail {
            rendered.push_str(&format!("\n\nDetail: {}", truncate_for_log(detail, 1200)));
        }
    }
    if let Some(run_id) = run_id {
        rendered.push_str(&format!("\n\nRun: `{run_id}`"));
    }
    rendered
}

fn bearwire_run_failed_stderr_context(event: &Value) -> Option<String> {
    let data = event.get("data")?;
    let context = data.get("context")?;
    if context.is_null() {
        return None;
    }
    Some(truncate_for_log(&context.to_string(), 1200))
}

#[derive(Debug, Deserialize)]
struct BearWireToolCallCard {
    id: String,
    name: Option<String>,
    #[serde(default)]
    arguments: Option<Value>,
    #[serde(default)]
    display: Option<Value>,
}

#[derive(Debug, Deserialize)]
struct BearWireToolCallFinishedData {
    tool_call: BearWireToolCallCard,
}

impl BearWireToolCallFinishedData {
    fn parse(event: &Value) -> Result<Self> {
        let data = event
            .get("data")
            .cloned()
            .ok_or_else(|| anyhow!("BearWire tool completion missing data"))?;
        serde_json::from_value(data).context("parse canonical BearWire tool completion data")
    }
}

fn tool_call_finished_presentation(
    tool_call_id: &str,
    tool_name: &str,
    cached_input_args: Option<Value>,
    cached_display: Option<Value>,
    event_input_args: Option<Value>,
    event_display: Option<Value>,
) -> crate::ToolRequestPresentation {
    crate::ToolRequestPresentation {
        tool_call_id: tool_call_id.to_string(),
        tool_name: tool_name.to_string(),
        arguments: cached_input_args.or(event_input_args),
        display: event_display.or(cached_display),
    }
}

async fn handle_bearwire_tool_call_finished_event(
    shared_state: &AdapterSharedState,
    session_id: &str,
    event: &Value,
    failed: bool,
    turn_token: Uuid,
) -> Result<()> {
    let data = event.get("data").unwrap_or(&Value::Null);
    let canonical = BearWireToolCallFinishedData::parse(event)?;
    let lookup_tool_call_id = canonical.tool_call.id.as_str();
    let cached = shared_state
        .tool_tasks
        .get(session_id, lookup_tool_call_id)
        .await;
    let cached_input_args = cached.as_ref().and_then(|record| record.input_args.clone());
    let cached_display = cached.as_ref().and_then(|record| record.display.clone());
    let had_cached_start = cached.is_some();
    let tool_call_id = canonical.tool_call.id;
    let tool_name = canonical
        .tool_call
        .name
        .as_deref()
        .filter(|name| !crate::is_placeholder_tool_name(name))
        .map(str::to_string)
        .or_else(|| cached.as_ref().map(|record| record.tool_name.clone()))
        .ok_or_else(|| anyhow!("canonical BearWire tool completion missing tool_call.name"))?;
    let summary = tool_call_finished_summary(data, &tool_name, failed);
    let status = if failed { "failed" } else { "completed" };
    let presentation = tool_call_finished_presentation(
        &tool_call_id,
        &tool_name,
        cached_input_args,
        cached_display,
        canonical.tool_call.arguments,
        canonical.tool_call.display,
    );
    send_tool_call_update_for_turn(
        shared_state,
        session_id,
        turn_token,
        &tool_call_id,
        &tool_name,
        ToolCallUpdatePayload {
            status,
            text: &summary,
            request: Some(presentation),
            raw_output: Some(compact_json_preview(
                data,
                BEARWIRE_TOOL_RAW_OUTPUT_PREVIEW_CHARS,
            )),
            extra_content: Vec::new(),
        },
    )
    .await?;
    if had_cached_start {
        shared_state
            .tool_tasks
            .set_phase(
                session_id,
                &tool_call_id,
                &tool_name,
                crate::ToolTaskPhase::ResultPosted,
            )
            .await;
        let _ = shared_state
            .tool_tasks
            .remove(session_id, &tool_call_id)
            .await;
    }
    Ok(())
}

fn bearwire_message_delta_text(event: &Value) -> &str {
    event
        .get("data")
        .and_then(|data| {
            data.get("delta")
                .or_else(|| data.get("text"))
                .or_else(|| data.get("reasoning"))
                .or_else(|| data.get("thinking"))
                .or_else(|| data.get("thought"))
        })
        .and_then(Value::as_str)
        .unwrap_or("")
}

fn value_has_reasoning_marker(value: &Value) -> bool {
    value
        .as_str()
        .map(|raw| {
            let raw = raw.to_ascii_lowercase();
            raw.contains("reasoning") || raw.contains("thinking") || raw.contains("thought")
        })
        .unwrap_or(false)
}

fn bearwire_message_delta_is_reasoning(event: &Value) -> bool {
    let Some(data) = event.get("data") else {
        return false;
    };
    [
        data.get("kind"),
        data.get("role"),
        data.get("type"),
        data.get("message_type"),
        data.get("channel"),
        data.get("source"),
        data.get("part_kind"),
        data.pointer("/delta/kind"),
        data.pointer("/delta/role"),
        data.pointer("/delta/type"),
    ]
    .into_iter()
    .flatten()
    .any(value_has_reasoning_marker)
        || data.get("reasoning").is_some()
        || data.get("thinking").is_some()
        || data.get("thought").is_some()
}

fn parse_event_page(body: &str, after: Option<i64>) -> Result<BearWireReplay> {
    let value: Value = serde_json::from_str(body)?;
    let next_after = value.get("next_after").and_then(Value::as_i64).or(after);
    let frames = value
        .get("events")
        .and_then(Value::as_array)
        .map(|events| {
            events
                .iter()
                .map(|entry| BearWireFrame {
                    sequence: entry.get("sequence").and_then(Value::as_i64),
                    event: entry.get("event").cloned(),
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    Ok(BearWireReplay { frames, next_after })
}

fn event_run_id(event: &Value) -> Option<&str> {
    event
        .get("run_id")
        .and_then(Value::as_str)
        .or_else(|| event.pointer("/data/run_id").and_then(Value::as_str))
}

fn event_is_run_scoped(ty: &str) -> bool {
    ty.starts_with("run.")
        || ty.starts_with("message.")
        || ty.starts_with("tool_call.")
        || matches!(
            ty,
            "client.waiting"
                | "permission.requested"
                | "permission.granted"
                | "permission.denied"
                | "permission.expired"
        )
}

async fn handle_bearwire_event(
    config: &Config,
    adapter_state: &mut AdapterState,
    shared_state: &AdapterSharedState,
    session_id: &str,
    current_run_id: &str,
    event: &Value,
    frame_sequence: Option<i64>,
    diagnostics: &mut SseStreamDiagnostics,
    turn_token: Uuid,
) -> Result<SseFrameOutcome> {
    diagnostics.frames += 1;
    let outcome = SseFrameOutcome::default();
    tracing::trace!(
        target: "bear_armature::lifecycle",
        session_id,
        current_run_id,
        bearwire_sequence = ?frame_sequence,
        "handling ordered BearWire frame"
    );
    let ty = event.get("type").and_then(Value::as_str).unwrap_or("");
    if current_run_id != "<unknown>" && event_is_run_scoped(ty) {
        if let Some(event_run_id) = event_run_id(event) {
            if event_run_id != current_run_id {
                tracing::trace!(
                    target: "bear_armature::lifecycle",
                    session_id,
                    current_run_id,
                    event_run_id,
                    event_type = ty,
                    "ignoring BearWire event for non-current run"
                );
                return Ok(outcome);
            }
        }
    }
    let mut outcome = outcome;
    diagnostics.observe_event(event);

    match ty {
        "message.delta" => {
            let text = bearwire_message_delta_text(event);
            if bearwire_message_delta_is_reasoning(event) {
                if !text.is_empty() {
                    if crate::bear_debug_verbose() {
                        eprintln!(
                            "bear-armature: reclassified reasoning-tagged message.delta as thought session_id={} sample={}",
                            session_id,
                            truncate_for_log(text, 160)
                        );
                    }
                    send_agent_thought_chunk_for_turn(shared_state, session_id, turn_token, text)
                        .await?;
                }
            } else {
                outcome.saw_visible_output = !text.is_empty();
                if !text.is_empty() {
                    send_agent_message_chunk_for_turn(shared_state, session_id, turn_token, text)
                        .await?;
                }
            }
        }
        "message.reasoning.delta" => {
            let text = bearwire_message_delta_text(event);
            if !text.is_empty() {
                send_agent_thought_chunk_for_turn(shared_state, session_id, turn_token, text)
                    .await?;
            }
        }
        "run.progress" => {
            let data = event.get("data").unwrap_or(&Value::Null);
            let text = data.get("text").and_then(Value::as_str).unwrap_or("");
            let kind = data
                .get("kind")
                .and_then(Value::as_str)
                .unwrap_or("progress");
            if kind == "plan_update" {
                let entries_json = bearwire_plan_update_entries(event);
                let plan_event = json!({ "entries": entries_json });
                let entries = plan_entries_from_plan_update_event(&plan_event);
                let approval_fallback = data
                    .get("approval_fallback")
                    .or_else(|| data.pointer("/detail/approval_fallback"));
                outcome.saw_visible_output = true;
                outcome.saw_tool_activity = true;
                diagnostics.saw_tool_activity = true;
                handle_plan_update_projection(
                    shared_state,
                    session_id,
                    turn_token,
                    entries,
                    approval_fallback,
                )
                .await?;
                return Ok(outcome);
            }
            let elapsed_ms = data.get("elapsed_ms").and_then(Value::as_u64);
            // Progress is observability, not model-visible output. Do not let it satisfy
            // prompt completion checks or suppress first-assistant/tool-event diagnostics.
            if crate::bear_debug_verbose() {
                eprintln!(
                    "bear-armature: BearWire progress session_id={} run_id={} kind={} elapsed_ms={} text={}",
                    session_id,
                    event
                        .get("run_id")
                        .and_then(Value::as_str)
                        .unwrap_or("<unknown>"),
                    kind,
                    elapsed_ms
                        .map(|value| value.to_string())
                        .unwrap_or_else(|| "unknown".to_string()),
                    if text.is_empty() { "<empty>" } else { text }
                );
            }
            if !text.is_empty() {
                handle_status_text_for_turn(shared_state, session_id, turn_token, text).await?;
            }
        }
        "session_info_update" => {
            let data = event.get("data").unwrap_or(&Value::Null);
            let title = event
                .get("title")
                .or_else(|| data.get("title"))
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string);
            let updated_at = event
                .get("updated_at")
                .or_else(|| data.get("updated_at"))
                .and_then(Value::as_str)
                .map(str::to_string);
            let context_budget = event
                .pointer("/meta/bears/context_budget")
                .cloned()
                .or_else(|| event.pointer("/_meta/bears/context_budget").cloned())
                .or_else(|| data.pointer("/meta/bears/context_budget").cloned())
                .or_else(|| data.get("context_budget").cloned());
            let runtime = event
                .pointer("/meta/bears/runtime")
                .cloned()
                .or_else(|| event.pointer("/_meta/bears/runtime").cloned())
                .or_else(|| data.pointer("/meta/bears/runtime").cloned())
                .or_else(|| data.get("runtime").cloned());
            handle_session_info_projection(
                adapter_state,
                shared_state,
                session_id,
                turn_token,
                title,
                updated_at,
                context_budget,
                runtime,
            )
            .await?;
        }
        "session.bound" => {
            let conversation_id = event
                .pointer("/data/binding/conversation_id")
                .or_else(|| event.pointer("/data/conversation_id"))
                .and_then(Value::as_str);
            if let Some(conversation_id) = conversation_id {
                handle_conversation_resolved_projection(
                    config,
                    adapter_state,
                    shared_state,
                    session_id,
                    turn_token,
                    conversation_id,
                )
                .await?;
            }
        }
        "run.completed" => {
            tracing::trace!(
                target: "bear_armature::lifecycle",
                session_id,
                run_id = event.get("run_id").and_then(|value| value.as_str()).unwrap_or("<unknown>"),
                "BearWire run.completed received"
            );
            outcome.saw_done = true;
        }

        "run.failed" => {
            let run_id = event
                .get("run_id")
                .and_then(Value::as_str)
                .unwrap_or("<unknown>");
            let reason = event
                .pointer("/data/reason")
                .and_then(Value::as_str)
                .unwrap_or("<unknown>");
            let timed_out_permission = event
                .pointer("/data/context/affected_obligations")
                .or_else(|| event.pointer("/data/affected_obligations"))
                .and_then(Value::as_array)
                .and_then(|obligations| {
                    obligations.iter().find(|obligation| {
                        obligation
                            .get("expected_responder_action")
                            .and_then(Value::as_str)
                            == Some("permission_decision")
                            || obligation.get("kind").and_then(Value::as_str)
                                == Some("permission_decision")
                    })
                });
            if reason == "client_obligation_timeout" {
                let obligation_id = timed_out_permission
                    .and_then(|value| value.get("obligation_id"))
                    .and_then(|value| value.as_str())
                    .unwrap_or("<unknown>");
                let permission_id = timed_out_permission
                    .and_then(|value| value.get("permission_id"))
                    .and_then(|value| value.as_str())
                    .unwrap_or("<unknown>");
                let tool_call_id = timed_out_permission
                    .and_then(|value| value.get("tool_call_id"))
                    .and_then(|value| value.as_str())
                    .unwrap_or("<unknown>");
                let tool_name = timed_out_permission
                    .and_then(|value| value.get("tool_name"))
                    .and_then(|value| value.as_str())
                    .unwrap_or("<unknown>");
                let claimed = timed_out_permission
                    .and_then(|value| value.get("claimed"))
                    .and_then(|value| value.as_bool())
                    .unwrap_or(false);
                tracing::error!(
                    target: "bear_armature::lifecycle",
                    session_id,
                    run_id,
                    reason,
                    obligation_id,
                    permission_id,
                    tool_call_id,
                    tool_name,
                    claimed,
                    "run failed because a permission obligation timed out; check prior permission obligation projection and ACP dispatch events"
                );
            } else {
                tracing::warn!(
                    target: "bear_armature::lifecycle",
                    session_id,
                    run_id,
                    reason,
                    "BearWire run.failed received"
                );
            }
            let message = bearwire_run_failed_user_message(event);
            eprintln!(
                "bear-armature: BearWire run failed session_id={} message={}",
                session_id,
                truncate_for_log(&message, 500)
            );
            if let Some(context) = bearwire_run_failed_stderr_context(event) {
                eprintln!(
                    "bear-armature: BearWire run failed diagnostic session_id={} context={}",
                    session_id, context
                );
            }
            return Err(anyhow!(message));
        }
        "run.interrupted" => {
            // The server keeps this run retryable for reconciliation, but the current
            // client delivery is over and must not leave the conversation spinner active.
            outcome.saw_done = true;
            outcome.saw_error = true;
            outcome.saw_visible_output = true;
            diagnostics.saw_error = true;
            diagnostics.saw_visible_output = true;
            let message = format_den_status(
                "The model connection ended before the response finished. Send another message to retry.",
            );
            send_agent_message_chunk_for_turn(shared_state, session_id, turn_token, &message)
                .await?;
        }
        "run.cancelled" => {
            outcome = cancelled_run_frame_outcome();
            diagnostics.saw_error = true;
            diagnostics.saw_visible_output = true;
            let message = format_den_status("The request was cancelled.");
            eprintln!(
                "bear-armature: BearWire run cancelled session_id={} run_id={}",
                session_id,
                event
                    .get("run_id")
                    .and_then(Value::as_str)
                    .unwrap_or("<unknown>")
            );
            send_agent_message_chunk_for_turn(shared_state, session_id, turn_token, &message)
                .await?;
        }
        "tool_call.requested" => {
            tracing::trace!(
                target: "bear_armature::lifecycle",
                session_id,
                run_id = event.get("run_id").and_then(|value| value.as_str()).unwrap_or("<unknown>"),
                tool_call_id = event.pointer("/data/tool_call/id").and_then(|value| value.as_str()).unwrap_or("<unknown>"),
                tool_name = event.pointer("/data/tool_call/name").and_then(|value| value.as_str()).unwrap_or("<unknown>"),
                "BearWire tool_call.requested received"
            );
            outcome.saw_tool_activity = true;
            diagnostics.saw_tool_activity = true;
            if is_den_server_tool_request(event) {
                project_den_owned_tool_request(shared_state, session_id, event, turn_token).await?;
            } else {
                spawn_tool_request_task(
                    config.clone(),
                    shared_state.clone(),
                    session_id.to_string(),
                    event.clone(),
                    turn_token,
                );
            }
        }
        "tool_call.completed" | "tool_call.warning" | "tool_call.cancelled" => {
            tracing::trace!(
                target: "bear_armature::lifecycle",
                session_id,
                run_id = event.get("run_id").and_then(|value| value.as_str()).unwrap_or("<unknown>"),
                tool_call_id = event.pointer("/data/tool_call/id").and_then(|value| value.as_str()).unwrap_or("<unknown>"),
                status = ty,
                "BearWire tool_call terminal update received"
            );
            outcome.saw_tool_activity = true;
            diagnostics.saw_tool_activity = true;
            handle_bearwire_tool_call_finished_event(
                shared_state,
                session_id,
                event,
                false,
                turn_token,
            )
            .await?;
        }
        "tool_call.failed" => {
            let tool_name = event
                .pointer("/data/tool_call/name")
                .and_then(Value::as_str)
                .unwrap_or("<unknown>");
            let failure_summary = tool_call_finished_summary(
                event.get("data").unwrap_or(&Value::Null),
                tool_name,
                true,
            );
            tracing::warn!(
                target: "bear_armature::lifecycle",
                session_id,
                run_id = event.get("run_id").and_then(|value| value.as_str()).unwrap_or("<unknown>"),
                tool_call_id = event.pointer("/data/tool_call/id").and_then(|value| value.as_str()).unwrap_or("<unknown>"),
                tool_name,
                error = %truncate_for_log(&failure_summary, 500),
                "BearWire tool_call.failed received"
            );
            outcome.saw_tool_activity = true;
            outcome.saw_error = true;
            diagnostics.saw_tool_activity = true;
            diagnostics.saw_error = true;
            handle_bearwire_tool_call_finished_event(
                shared_state,
                session_id,
                event,
                true,
                turn_token,
            )
            .await?;
        }
        "client.waiting" => {
            tracing::trace!(
                target: "bear_armature::lifecycle",
                session_id,
                run_id = event.get("run_id").and_then(|value| value.as_str()).unwrap_or("<unknown>"),
                tool_call_id = event.pointer("/data/tool_call/id").and_then(|value| value.as_str()).unwrap_or("<unknown>"),
                tool_name = event.pointer("/data/tool_call/name").and_then(|value| value.as_str()).unwrap_or("<unknown>"),
                permission_id = event.pointer("/data/permission/id").and_then(|value| value.as_str()).unwrap_or("<unknown>"),
                "BearWire client.waiting received"
            );
            outcome.saw_tool_activity = true;
            outcome.saw_visible_output = true;
            diagnostics.saw_tool_activity = true;
            diagnostics.saw_visible_output = true;
            handle_permission_request_event(
                config,
                adapter_state,
                shared_state,
                session_id,
                event,
                turn_token,
            )
            .await?;
        }
        "tool_call.blocked" => {
            if crate::bear_debug_verbose() {
                eprintln!(
                    "bear-armature: ignoring legacy BearWire tool_call.blocked session_id={}; actionable permission waits must use client.waiting",
                    session_id
                );
            }
        }
        "permission.granted" | "permission.denied" | "permission.expired" => {
            diagnostics.saw_tool_activity = true;
            outcome.saw_tool_activity = true;
            if crate::bear_debug_verbose() {
                eprintln!(
                    "bear-armature: BearWire permission event session_id={} type={} permission_id={}",
                    session_id,
                    ty,
                    event
                        .pointer("/data/permission_id")
                        .and_then(Value::as_str)
                        .unwrap_or("<unknown>")
                );
            }
        }
        "run.paused" => {
            // `run.paused` is status/diagnostic state only. Actionable waits must arrive
            // as `client.waiting` with a persisted obligation. Keep this out of normal
            // stderr so stale pause status cannot look like a fresh permission request
            // after the armature already answered the matching obligation.
            // See docs/architecture/bearwire-run-stream-completion.md: an EOF after
            // this boundary is not a failed completion; reconcile canonical run state
            // and obligations before deciding whether the prompt may end.
            if crate::bear_debug_verbose() {
                let reason = event
                    .pointer("/data/reason")
                    .and_then(Value::as_str)
                    .unwrap_or("paused");
                let resume_token = event
                    .pointer("/data/resume_token")
                    .and_then(Value::as_str)
                    .unwrap_or("<none>");
                eprintln!(
                    "bear-armature: BearWire run paused status-only session_id={} reason={} resume_token={}",
                    session_id, reason, resume_token
                );
            }
        }
        "permission.requested" => {
            if crate::bear_debug_verbose() {
                eprintln!(
                    "bear-armature: ignoring legacy BearWire permission.requested session_id={}; actionable permission waits must use client.waiting",
                    session_id
                );
            }
        }
        ty if is_optional_runtime_metadata_event(ty) => {
            // Optional Den runtime metadata. It does not create a client obligation,
            // affect run liveness, or require an Armature projection.
            if crate::bear_debug_verbose() {
                eprintln!(
                    "bear-armature: ignoring optional BearWire runtime.objective_orientation session_id={}",
                    session_id
                );
            }
        }
        _ => {
            diagnostics.observe_unknown(event);
            eprintln!(
                "bear-armature: unknown BearWire event type {:?}; sample={}",
                ty,
                truncate_for_log(&event.to_string(), 240)
            );
        }
    }

    diagnostics.saw_visible_output |= outcome.saw_visible_output;
    diagnostics.saw_tool_activity |= outcome.saw_tool_activity;
    diagnostics.saw_error |= outcome.saw_error;
    diagnostics.saw_turn_complete |= outcome.saw_done;
    Ok(outcome)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn docket_transitions_are_optional_runtime_metadata() {
        assert!(is_optional_runtime_metadata_event(
            "runtime.objective_orientation"
        ));
        assert!(is_optional_runtime_metadata_event("run.recovering"));
        assert!(is_optional_runtime_metadata_event("run.recovered"));
        assert!(is_optional_runtime_metadata_event(
            "docket.execution.started"
        ));
        assert!(is_optional_runtime_metadata_event("docket.execution.ended"));
        assert!(!is_optional_runtime_metadata_event("runtime.unknown"));
    }

    #[test]
    fn terminal_projection_prefers_live_request_presentation() {
        let presentation = tool_call_finished_presentation(
            "call-read",
            "fs_read_text_file",
            Some(json!({ "path": "/workspace/README.md" })),
            Some(json!({ "title": "Read file: /workspace/README.md" })),
            None,
            None,
        );
        assert_eq!(
            presentation
                .arguments
                .as_ref()
                .and_then(|arguments| arguments.get("path"))
                .and_then(Value::as_str),
            Some("/workspace/README.md")
        );
        assert_eq!(
            presentation
                .display
                .as_ref()
                .and_then(|display| display.get("title"))
                .and_then(Value::as_str),
            Some("Read file: /workspace/README.md")
        );
    }

    #[test]
    fn parse_event_page_uses_server_owned_cursor() {
        let page = json!({
            "ok": true,
            "events": [
                {
                    "sequence": 42,
                    "event": {"type": "run.progress", "run_id": "run-1", "data": {}}
                }
            ],
            "next_after": 100,
            "has_more": true
        });

        let parsed = parse_event_page(&page.to_string(), Some(41)).unwrap();

        assert_eq!(parsed.next_after, Some(100));
        assert_eq!(parsed.frames.len(), 1);
        assert_eq!(parsed.frames[0].sequence, Some(42));
        assert_eq!(
            parsed.frames[0].event.as_ref().unwrap()["type"],
            "run.progress"
        );
    }

    #[test]
    fn parse_event_page_does_not_advance_without_next_after() {
        let page = json!({
            "ok": true,
            "events": [],
            "has_more": false
        });

        let parsed = parse_event_page(&page.to_string(), Some(41)).unwrap();

        assert_eq!(parsed.next_after, Some(41));
        assert!(parsed.frames.is_empty());
    }

    #[test]
    fn run_scoped_event_helpers_identify_foreign_terminal_events() {
        let event = json!({
            "type": "run.failed",
            "run_id": "run-old",
            "data": { "reason": "client_obligation_timeout" }
        });

        assert!(event_is_run_scoped("run.failed"));
        assert_eq!(event_run_id(&event), Some("run-old"));
        assert_ne!(event_run_id(&event), Some("run-current"));
        assert!(!event_is_run_scoped("session.bound"));
    }

    #[test]
    fn message_delta_with_reasoning_metadata_is_thought_not_visible_output() {
        let event = json!({
            "type": "message.delta",
            "data": {
                "kind": "reasoning_delta",
                "delta": "thinking about the next step"
            }
        });

        assert!(bearwire_message_delta_is_reasoning(&event));
        assert_eq!(
            bearwire_message_delta_text(&event),
            "thinking about the next step"
        );
    }

    #[test]
    fn message_delta_without_reasoning_metadata_is_visible_text() {
        let event = json!({
            "type": "message.delta",
            "data": {
                "delta": "hello user"
            }
        });

        assert!(!bearwire_message_delta_is_reasoning(&event));
        assert_eq!(bearwire_message_delta_text(&event), "hello user");
    }

    #[test]
    fn reasoning_text_fallback_is_extracted_from_compatibility_payload() {
        let event = json!({
            "type": "message.delta",
            "data": {
                "source": "provider_reasoning",
                "reasoning": "compat reasoning"
            }
        });

        assert!(bearwire_message_delta_is_reasoning(&event));
        assert_eq!(bearwire_message_delta_text(&event), "compat reasoning");
    }

    #[test]
    fn bearwire_finished_tool_requires_canonical_tool_call_identity() {
        let event = json!({
            "type": "tool_call.completed",
            "run_id": "run-1",
            "data": {
                "tool_call": {
                    "id": "call-1",
                    "name": "fs_read_text_file",
                    "title": "Read file",
                    "arguments": { "path": "/workspace/README.md" }
                },
                "summary": "Read file."
            }
        });

        let parsed = BearWireToolCallFinishedData::parse(&event).unwrap();

        assert_eq!(parsed.tool_call.id, "call-1");
        assert_eq!(parsed.tool_call.name.as_deref(), Some("fs_read_text_file"));
        assert_eq!(
            parsed.tool_call.arguments.as_ref().unwrap()["path"],
            "/workspace/README.md"
        );
    }

    #[test]
    fn bearwire_run_failed_user_message_includes_reason_and_run_id() {
        let event = json!({
            "type": "run.failed",
            "run_id": "run-123",
            "data": {
                "reason": "stream_error",
                "message": "Runtime stopped before producing assistant output: max_steps_exceeded"
            }
        });

        let message = bearwire_run_failed_user_message(&event);

        assert!(message.contains("stream_error"));
        assert!(message.contains("max_steps_exceeded"));
        assert!(message.contains("run-123"));
    }

    #[test]
    fn event_fetch_retry_uses_capped_exponential_backoff() {
        assert_eq!(event_fetch_retry_delay(1), Duration::from_millis(250));
        assert_eq!(event_fetch_retry_delay(2), Duration::from_millis(500));
        assert_eq!(event_fetch_retry_delay(5), Duration::from_secs(4));
        assert_eq!(
            event_fetch_retry_delay(20),
            BEARWIRE_EVENT_FETCH_MAX_BACKOFF
        );
    }

    #[test]
    fn missed_terminal_event_recovery_includes_failed_and_interrupted() {
        for event_type in ["run.failed", "run.interrupted"] {
            let state = json!({
                "recent_events": [
                    { "event": { "type": "run.started" } },
                    { "event": { "type": event_type, "run_id": "run-1" } }
                ]
            });
            assert_eq!(
                latest_terminal_event_from_run_state(&state)
                    .and_then(|event| event.get("type"))
                    .and_then(Value::as_str),
                Some(event_type)
            );
        }
    }

    #[tokio::test]
    async fn follow_run_cancellation_ignores_tools_only_and_stale_turns() {
        let (tx, mut rx) = tokio::sync::broadcast::channel(4);
        let current_turn = Uuid::new_v4();
        tx.send(tools_cancellation_notice(
            "session-1",
            current_turn,
            crate::CancellationOrigin::RunTerminal,
        ))
        .unwrap();
        tx.send(crate::CancellationNotice {
            session_id: "session-1".to_string(),
            turn_token: Some(Uuid::new_v4()),
            conversation_id: None,
            scope: crate::CancellationScope::PromptAndTools,
            origin: crate::CancellationOrigin::Steering,
        })
        .unwrap();
        tx.send(crate::CancellationNotice {
            session_id: "session-1".to_string(),
            turn_token: Some(current_turn),
            conversation_id: None,
            scope: crate::CancellationScope::PromptAndTools,
            origin: crate::CancellationOrigin::SessionCancel,
        })
        .unwrap();

        tokio::time::timeout(
            Duration::from_millis(50),
            wait_for_matching_prompt_cancellation(&mut rx, "session-1", current_turn),
        )
        .await
        .expect("matching prompt cancellation must wake follow_run");
    }

    #[tokio::test]
    async fn run_cancelled_handler_stops_tools_without_consuming_prompt_completion() {
        let (cancellation_tx, _) = tokio::sync::broadcast::channel(8);
        let mut cancellation_rx = cancellation_tx.subscribe();
        let shared_state = crate::AdapterSharedState {
            transport: crate::JsonRpcTransport::default(),
            client_capabilities: std::sync::Arc::new(tokio::sync::Mutex::new(Value::Null)),
            session_contexts: std::sync::Arc::new(tokio::sync::Mutex::new(
                std::collections::HashMap::new(),
            )),
            last_plan_update_hashes: std::sync::Arc::new(tokio::sync::Mutex::new(
                std::collections::HashMap::new(),
            )),
            surface_tool_statuses: std::sync::Arc::new(tokio::sync::Mutex::new(
                std::collections::HashMap::new(),
            )),
            tool_tasks: crate::ToolTaskRegistry::default(),
            mcp_registry: crate::tools::mcp::McpRegistry::default(),
            approval_cache: crate::approvals::ApprovalCache::default(),
            cancellation_tx,
            active_prompts: std::sync::Arc::new(tokio::sync::Mutex::new(
                std::collections::HashMap::new(),
            )),
            projection_dispatcher: crate::AcpProjectionDispatcher::default(),
        };
        let turn_token = Uuid::new_v4();
        let response = crate::PromptResponseGuard::new(json!("prompt-1"));
        shared_state.active_prompts.lock().await.insert(
            "session-1".to_string(),
            crate::ActivePromptTurn {
                token: turn_token,
                conversation_id: None,
                run_id: Some("run-1".to_string()),
                response: response.clone(),
            },
        );
        let config = crate::Config {
            api_url: "http://127.0.0.1:1".to_string(),
            bear: "test-bear".to_string(),
            token: "test-token".to_string(),
            client: "zed".to_string(),
        };
        shared_state
            .tool_tasks
            .try_register(
                "session-1",
                "call-local",
                "fs_read_text_file",
                Some(turn_token),
                crate::ToolExecutionOwnership::ArmatureLocal,
            )
            .await;
        crate::record_surface_tool_status(
            &shared_state,
            "session-1",
            "call-local",
            crate::SurfaceToolStatus::InProgress,
        )
        .await;
        let mut adapter_state = crate::AdapterState::default();
        let mut diagnostics = crate::SseStreamDiagnostics::default();

        let outcome = handle_bearwire_event_with_terminal_cleanup(
            &config,
            &mut adapter_state,
            &shared_state,
            "session-1",
            "run-1",
            &json!({ "type": "run.cancelled", "run_id": "run-1" }),
            Some(1),
            &mut diagnostics,
            turn_token,
        )
        .await
        .unwrap();
        let notice = cancellation_rx.recv().await.unwrap();

        assert!(outcome.saw_done);
        assert!(shared_state
            .tool_tasks
            .list_for_session("session-1")
            .await
            .is_empty());
        assert_eq!(
            crate::current_surface_tool_status(&shared_state, "session-1", "call-local").await,
            Some(crate::SurfaceToolStatus::Failed)
        );
        assert_eq!(notice.scope, crate::CancellationScope::ToolsOnly);
        assert_eq!(notice.origin, crate::CancellationOrigin::RunTerminal);
        assert!(!cancellation_matches_prompt_turn(
            &notice,
            "session-1",
            turn_token
        ));
        assert_eq!(response.claim(), Some(json!("prompt-1")));
    }

    #[test]
    fn reconciliation_decodes_all_terminal_outcomes_and_matching_events() {
        for (state_name, event_type) in [
            ("completed", "run.completed"),
            ("failed", "run.failed"),
            ("cancelled", "run.cancelled"),
        ] {
            let event = json!({ "type": event_type, "run_id": "run-1" });
            let state = json!({
                "run": { "state": state_name },
                "open_obligations": [],
                "recent_events": [{ "event": event.clone() }]
            });
            assert!(matches!(
                reconciliation_decision(&state, false),
                ReconciliationDecision::DeliverTerminalEvent(event)
                    if event.get("type").and_then(Value::as_str) == Some(event_type)
            ));
            assert!(terminal_event_tool_card_reason("run-1", &event)
                .is_some_and(|reason| reason.contains("result unavailable")));
        }
        assert!(terminal_event_tool_card_reason(
            "run-1",
            &json!({ "type": "run.interrupted", "run_id": "run-1" })
        )
        .is_some_and(|reason| reason.contains("result unavailable")));
    }

    #[test]
    fn reconciliation_flags_missing_terminal_event_instead_of_spinning() {
        let state = json!({
            "run": { "state": "completed" },
            "open_obligations": [],
            "recent_events": [{ "event": { "type": "run.started" } }]
        });
        assert_eq!(
            reconciliation_decision(&state, false),
            ReconciliationDecision::TerminalEventMissing(RunTerminalOutcome::Completed)
        );
    }

    #[test]
    fn deadline_reconciliation_rejects_paused_active_and_obligated_terminal_states() {
        for state in [
            json!({ "run": { "state": "paused" }, "open_obligations": [] }),
            json!({ "run": { "state": "running" }, "open_obligations": [] }),
            json!({
                "run": { "state": "completed" },
                "open_obligations": [{ "id": "obl-1" }],
                "recent_events": [{ "event": { "type": "run.completed" } }]
            }),
        ] {
            assert_eq!(
                reconciliation_decision(&state, false),
                ReconciliationDecision::Continue
            );
            assert_eq!(
                reconciliation_decision(&state, true),
                ReconciliationDecision::DeadlineExceeded
            );
        }
        let notice = tools_cancellation_notice(
            "session-1",
            Uuid::new_v4(),
            crate::CancellationOrigin::DeliveryDeadline,
        );
        assert_eq!(notice.scope, crate::CancellationScope::ToolsOnly);
        assert_eq!(notice.origin, crate::CancellationOrigin::DeliveryDeadline);
    }

    #[test]
    fn run_state_obligation_summary_reports_open_obligations() {
        let state = json!({
            "run": { "state": "waiting_for_tool_result" },
            "open_obligations": [{
                "id": "obl-1",
                "kind": "tool_result",
                "expected_responder_action": "tool_result",
                "state": "waiting_for_client",
                "tool_call_id": "call-1",
                "permission_id": null,
                "updated_at": "2026-07-10T00:00:00Z"
            }]
        });

        let summary = run_state_obligation_summary(&state).unwrap();

        assert!(summary.contains("waiting_for_tool_result"), "{summary}");
        assert!(summary.contains("obl-1"), "{summary}");
        assert!(summary.contains("call-1"), "{summary}");
        assert!(summary.contains("waiting_for_client"), "{summary}");
    }

    #[test]
    fn run_state_obligation_summary_accepts_continuation_obligation_shape() {
        let state = json!({
            "run": { "state": "waiting_for_tool_result" },
            "open_obligations": [{
                "obligation_id": "obl-2",
                "kind": "tool_result",
                "expected_responder_action": "tool_result",
                "state": "waiting_for_client",
                "tool_call_id": "call-2",
                "permission_id": null
            }]
        });

        let summary = run_state_obligation_summary(&state).unwrap();

        assert!(summary.contains("obl-2"), "{summary}");
        assert!(summary.contains("call-2"), "{summary}");
    }

    #[test]
    fn run_state_obligation_summary_ignores_clean_state() {
        let state = json!({ "run": { "state": "running" }, "open_obligations": [] });
        assert!(run_state_obligation_summary(&state).is_none());
    }

    #[test]
    fn reconstructs_approval_free_policy_allowed_tool_request_from_run_state() {
        let obligation = json!({
            "id": "obl-1",
            "kind": "tool_result",
            "expected_responder_action": "tool_result",
            "state": "waiting_for_client",
            "tool_call_id": "call-search",
            "request_payload": {
                "tool_call_id": "call-search",
                "tool_name": "fs_search_files",
                "arguments": { "path": "docs", "query": "BearWire", "limit": 100 },
                "approval_required": false,
                "execution_target": "armature_local",
                "policy": {
                    "execution_target": "armature_local",
                    "approval_required": false,
                    "approval_policy": "never",
                    "risk": "read_only"
                }
            }
        });

        let event = actionable_tool_request_event_from_obligation("run-1", &obligation)
            .expect("actionable approval-free tool");

        assert_eq!(event["type"], "tool_call.requested");
        assert_eq!(event["run_id"], "run-1");
        assert_eq!(event["data"]["tool_call"]["id"], "call-search");
        assert_eq!(event["data"]["tool_call"]["name"], "fs_search_files");
        assert_eq!(event["data"]["tool_call"]["arguments"]["query"], "BearWire");
        assert_eq!(event["data"]["serviced_from_run_state"], true);
    }

    #[test]
    fn reconstructs_permission_granted_tool_request_from_run_state() {
        let obligation = json!({
            "id": "obl-granted",
            "kind": "tool_result",
            "expected_responder_action": "tool_result",
            "state": "waiting_for_client",
            "tool_call_id": "call-edit",
            "permission_id": "perm-edit",
            "request_payload": {
                "tool_call_id": "call-edit",
                "tool_name": "fs_edit_file",
                "arguments": { "path": "README.md", "old_text": "a", "new_text": "b" },
                "approval_required": false,
                "approval_request_id": "perm-edit",
                "permission_granted": true,
                "execution_target": "armature_local",
                "policy": {
                    "execution_target": "armature_local",
                    "approval_required": true,
                    "approval_policy": "required",
                    "risk": "writes_workspace"
                }
            }
        });

        let event = actionable_tool_request_event_from_obligation("run-1", &obligation)
            .expect("actionable permission-granted tool");

        assert_eq!(event["type"], "tool_call.requested");
        assert_eq!(event["data"]["obligation_id"], "obl-granted");
        assert_eq!(event["data"]["tool_call"]["id"], "call-edit");
        assert_eq!(event["data"]["tool_call"]["name"], "fs_edit_file");
        assert_eq!(event["data"]["tool_call"]["arguments"]["new_text"], "b");
        assert_eq!(event["data"]["approval_required"], false);
    }

    #[test]
    fn reconstructs_permission_wait_from_run_state() {
        let obligation = json!({
            "id": "obl-perm",
            "kind": "permission_decision",
            "expected_responder_action": "permission_decision",
            "state": "waiting_for_client",
            "tool_call_id": "call-edit",
            "permission_id": "perm-edit",
            "request_payload": {
                "tool_call_id": "call-edit",
                "tool_name": "fs_edit_file",
                "arguments": { "path": "README.md", "old_text": "a", "new_text": "b" },
                "approval_required": true,
                "approval_request_id": "perm-edit",
                "approval_reason": "Edit README.md",
                "execution_target": "armature_local",
                "policy": {
                    "execution_target": "armature_local",
                    "approval_required": true,
                    "approval_policy": "required",
                    "risk": "writes_workspace"
                }
            }
        });

        let event = actionable_tool_request_event_from_obligation("run-1", &obligation)
            .expect("actionable permission wait");

        assert_eq!(event["type"], "client.waiting");
        assert_eq!(event["run_id"], "run-1");
        assert_eq!(
            event["data"]["expected_client_method"],
            "client.permission.result"
        );
        assert_eq!(event["data"]["obligation_id"], "obl-perm");
        assert_eq!(event["data"]["tool_call"]["id"], "call-edit");
        assert_eq!(event["data"]["permission"]["id"], "perm-edit");
        assert_eq!(event["data"]["serviced_from_run_state"], true);
    }

    #[test]
    fn does_not_auto_execute_mutating_or_approval_required_tool_result_obligations() {
        let mutating = json!({
            "kind": "tool_result",
            "expected_responder_action": "tool_result",
            "state": "waiting_for_client",
            "request_payload": {
                "tool_call_id": "call-edit",
                "tool_name": "fs_edit_file",
                "arguments": { "path": "README.md" },
                "approval_required": false,
                "execution_target": "armature_local"
            }
        });
        assert!(actionable_tool_request_event_from_obligation("run-1", &mutating).is_none());

        let policy_described_write = json!({
            "kind": "tool_result",
            "expected_responder_action": "tool_result",
            "state": "waiting_for_client",
            "request_payload": {
                "tool_call_id": "call-edit-policy",
                "tool_name": "fs_edit_file",
                "arguments": { "path": "README.md" },
                "approval_required": false,
                "execution_target": "armature_local",
                "policy": {
                    "execution_target": "armature_local",
                    "approval_required": false,
                    "approval_policy": "never",
                    "risk": "writes_workspace"
                }
            }
        });
        assert!(
            actionable_tool_request_event_from_obligation("run-1", &policy_described_write)
                .is_none()
        );

        let approval = json!({
            "kind": "tool_result",
            "expected_responder_action": "tool_result",
            "state": "waiting_for_client",
            "request_payload": {
                "tool_call_id": "call-read",
                "tool_name": "fs_read_text_file",
                "arguments": { "path": "README.md" },
                "approval_required": true,
                "execution_target": "armature_local"
            }
        });
        assert!(actionable_tool_request_event_from_obligation("run-1", &approval).is_none());
    }

    #[test]
    fn approval_required_tool_result_reports_server_invariant_violation() {
        let obligation = json!({
            "id": "obl-stale-approval",
            "kind": "tool_result",
            "expected_responder_action": "tool_result",
            "state": "waiting_for_client",
            "request_payload": {
                "tool_call_id": "call-edit",
                "tool_name": "fs_edit_file",
                "arguments": { "path": "README.md" },
                "approval_required": true,
                "approval_request_id": "perm-edit",
                "execution_target": "armature_local"
            }
        });

        let err = unsupported_required_client_obligation_error(&obligation)
            .expect("inconsistent tool obligation should fail the prompt");
        let message = err.to_string();

        assert!(message.contains("BearWire invariant violation"));
        assert!(message.contains(
            "tool_result obligation remains approval_required after permission settlement"
        ));
        assert!(!message.contains("unsupported required BearWire client obligation"));
    }

    #[test]
    fn does_not_service_den_owned_obligations_from_run_state() {
        let obligation = json!({
            "kind": "tool_result",
            "expected_responder_action": "tool_result",
            "state": "waiting_for_client",
            "request_payload": {
                "tool_call_id": "call-title",
                "tool_name": "set_conversation_title",
                "arguments": { "title": "Test" },
                "approval_required": false,
                "execution_target": "den"
            }
        });
        assert!(actionable_tool_request_event_from_obligation("run-1", &obligation).is_none());
    }

    #[test]
    fn unsupported_open_client_obligation_is_classified_for_reconciliation() {
        let obligation = json!({
            "id": "obl-context",
            "kind": "added_context",
            "expected_responder_action": "context_result",
            "state": "waiting_for_client",
            "request_payload": {
                "resource_id": "ctx-1",
                "execution_target": "armature_local"
            }
        });

        let err = unsupported_required_client_obligation_error(&obligation)
            .expect("unsupported open client obligation should fail the prompt");
        let message = err.to_string();

        assert!(message.contains("unsupported required BearWire client obligation"));
        assert!(message.contains("obl-context"));
        assert!(message.contains("added_context"));
        assert!(message.contains("context_result"));
    }

    #[test]
    fn unsupported_obligation_error_ignores_closed_and_den_obligations() {
        let closed = json!({
            "id": "obl-closed",
            "kind": "added_context",
            "expected_responder_action": "context_result",
            "state": "completed"
        });
        assert!(unsupported_required_client_obligation_error(&closed).is_none());

        let den_owned = json!({
            "id": "obl-den",
            "kind": "tool_result",
            "expected_responder_action": "tool_result",
            "state": "waiting_for_client",
            "request_payload": {
                "tool_call_id": "call-title",
                "execution_target": "den"
            }
        });
        assert!(unsupported_required_client_obligation_error(&den_owned).is_none());
    }

    #[test]
    fn compact_json_preview_truncates_large_raw_output() {
        let value = json!({ "content": "x".repeat(40 * 1024), "status": "ok" });

        let preview = compact_json_preview(&value, 1024);

        assert_eq!(preview["truncated"], true);
        assert_eq!(preview["preview_max_chars"], 1024);
        assert_eq!(preview["original_kind"], "object");
        assert!(preview["preview"].as_str().unwrap().chars().count() <= 1024);
    }

    #[test]
    fn format_den_status_prefixes_and_normalizes_system_first_person_copy() {
        assert_eq!(
            format_den_status("I stopped because this turn reached its budget."),
            "**Den**: This turn stopped because this turn reached its budget."
        );
        assert_eq!(
            format_den_status("The request was cancelled."),
            "**Den**: The request was cancelled."
        );
    }

    #[test]
    fn bearwire_run_failed_user_message_includes_error_detail() {
        let event = json!({
            "type": "run.failed",
            "run_id": "run-err",
            "data": {
                "message": "LLM responses provider error",
                "error_type": "invalid_request_error",
                "detail": "{\"code\":\"unsupported_parameter\",\"message\":\"tools[64].name is invalid\"}"
            }
        });

        let message = bearwire_run_failed_user_message(&event);

        assert!(message.contains("LLM responses provider error"));
        assert!(message.contains("invalid_request_error"));
        assert!(message.contains("unsupported_parameter"));
        assert!(message.contains("tools[64].name is invalid"));
        assert!(message.contains("run-err"));
    }

    #[test]
    fn bearwire_run_failed_user_message_hides_unclaimed_local_step_timeout_details() {
        let event = json!({
            "type": "run.failed",
            "run_id": "run-timeout",
            "data": {
                "reason": "client_obligation_timeout",
                "message": "The work surface did not return the requested result for fs_list_directory before the local-step wait expired.",
                "context": {
                    "affected_obligations": [{
                        "expected_responder_action": "tool_result",
                        "tool_name": "fs_list_directory",
                        "claimed": false
                    }]
                }
            }
        });

        let message = bearwire_run_failed_user_message(&event);

        assert!(message.contains("could not begin a local workspace operation"));
        assert!(message.contains("No local operation was started"));
        assert!(message.contains("Run: `run-timeout`"));
        assert!(!message.contains("fs_list_directory"));
        assert!(!message.contains("work surface did not return"));
    }

    #[test]
    fn bearwire_run_failed_user_message_has_fallback() {
        let event = json!({ "type": "run.failed" });

        let message = bearwire_run_failed_user_message(&event);

        assert_eq!(message, "**Den**: Run failed: BearWire run failed");
    }

    #[test]
    fn bearwire_run_failed_user_message_prefers_friendly_user_message() {
        let event = json!({
            "type": "run.failed",
            "run_id": "run-budget",
            "data": {
                "reason": "runtime_internal",
                "message": "I stopped because this turn exhausted its wall-clock budget (elapsed=252985ms/limit=240000ms).",
                "user_message": "BEARS stopped this turn after it ran too long. Recent tool results were preserved, but no final answer was delivered. Start a fresh turn to continue safely.",
                "context": {
                    "diagnostic": {
                        "elapsed_ms": 252985,
                        "limit_ms": 240000
                    }
                }
            }
        });

        let message = bearwire_run_failed_user_message(&event);
        let context = bearwire_run_failed_stderr_context(&event).expect("stderr context");

        assert!(message.starts_with("**Den**:"), "{message}");
        assert!(message.contains("ran too long"), "{message}");
        assert!(!message.contains("I stopped"), "{message}");
        assert!(!message.contains("elapsed=252985"), "{message}");
        assert!(context.contains("252985"), "{context}");
    }

    #[test]
    fn bearwire_run_failed_user_message_omits_context_but_stderr_helper_keeps_it() {
        let event = json!({
            "type": "run.failed",
            "run_id": "run-timeout",
            "data": {
                "reason": "continuation_watchdog_timeout",
                "message": "Den received the client result and started continuation request req-123, but no runtime event arrived within 30000ms.",
                "context": {
                    "continuation_request_id": "req-123",
                    "watchdog_timeout_ms": 30000,
                    "runtime_event_count": 0
                }
            }
        });

        let message = bearwire_run_failed_user_message(&event);
        let context = bearwire_run_failed_stderr_context(&event).expect("stderr context");

        assert!(!message.contains("Context:"));
        assert!(message.contains("run-timeout"));
        assert!(context.contains("continuation_request_id"));
        assert!(context.contains("req-123"));
    }

    #[test]
    fn terminal_projection_reuses_requested_display_when_completion_omits_it() {
        let requested_display = json!({
            "title": "Search for \"tool_call\" in this workspace",
            "kind": "search"
        });
        let cached_display = Some(requested_display.clone());
        let completion_display: Option<Value> = None;

        assert_eq!(
            completion_display.or(cached_display),
            Some(requested_display)
        );
    }

    #[test]
    fn tool_call_finished_summary_uses_tool_name_when_upstream_summary_is_generic() {
        let data = json!({
            "tool_name": "memory_read",
            "summary": "Tool failed."
        });

        assert_eq!(
            tool_call_finished_summary(&data, "memory_read", true),
            "Read memory failed."
        );
    }

    #[test]
    fn tool_call_finished_summary_replaces_provider_name_finished_summary_for_task_lists() {
        let data = json!({
            "tool_name": "list_task_lists",
            "summary": "Finished list_task_lists"
        });

        assert_eq!(
            tool_call_finished_summary(&data, "list_task_lists", false),
            "Listed task lists."
        );
    }

    #[test]
    fn tool_call_finished_summary_replaces_provider_name_finished_summary_for_common_den_tools() {
        for (tool_name, expected) in [
            ("session_info", "Inspected session."),
            ("memory_read", "Read memory."),
            ("memory_search", "Searched memory."),
            ("web_search", "Searched web."),
            ("git_status", "Checked git status."),
        ] {
            let data = json!({
                "tool_name": tool_name,
                "summary": format!("Finished {tool_name}")
            });

            assert_eq!(
                tool_call_finished_summary(&data, tool_name, false),
                expected
            );
        }
    }

    #[test]
    fn git_commit_finished_summary_includes_short_sha_and_subject() {
        let data = json!({
            "tool_name": "git_commit",
            "summary": "Finished git_commit",
            "structured_content": {
                "short_sha": "cc88ad9a",
                "subject": "fix: inject Pair Docket execution controls"
            }
        });

        assert_eq!(
            tool_call_finished_summary(&data, "git_commit", false),
            "Git commit created: cc88ad9a fix: inject Pair Docket execution controls."
        );
    }

    #[test]
    fn set_conversation_title_finished_summary_includes_requested_title_for_acp_tool_card() {
        let data = json!({
            "tool_call": {
                "id": "call-title",
                "name": "set_conversation_title",
                "arguments": { "title": "Test Armature ACP conversation title tool" }
            },
            "summary": "Finished set_conversation_title"
        });

        assert_eq!(
            tool_call_finished_summary(&data, "set_conversation_title", false),
            "Set conversation title to \"Test Armature ACP conversation title tool\"."
        );
    }

    #[test]
    fn set_conversation_title_finished_summary_reads_stringified_normalized_arguments() {
        let data = json!({
            "tool_call": {
                "id": "call-title",
                "name": "set_conversation_title",
                "arguments": r#"{"title":"Stringified ACP title"}"#
            },
            "summary": "Finished set_conversation_title"
        });

        assert_eq!(
            tool_call_finished_summary(&data, "set_conversation_title", false),
            "Set conversation title to \"Stringified ACP title\"."
        );
    }

    #[test]
    fn tool_call_finished_summary_adds_command_name_for_generic_run_command_summary() {
        let data = json!({
            "tool_name": "run_command",
            "summary": "Tool completed.",
            "args": {
                "command": "cargo",
                "args": ["test"]
            }
        });

        let summary = tool_call_finished_summary(&data, "run_command", false);

        assert!(summary.contains("Run Command completed."), "{summary}");
        assert!(summary.contains("Command: `cargo test`."), "{summary}");
    }

    #[test]
    fn tool_call_finished_summary_adds_git_subcommand_for_generic_summary() {
        let data = json!({
            "tool_name": "process_run",
            "summary": "Tool completed.",
            "args": {
                "command": "git",
                "args": ["status", "--short"]
            }
        });

        let summary = tool_call_finished_summary(&data, "process_run", false);

        assert!(summary.contains("Run process completed."), "{summary}");
        assert!(
            summary.contains("Command: `git status --short`."),
            "{summary}"
        );
    }

    #[test]
    fn tool_argument_normalization_covers_known_event_shapes() {
        let cases = [
            json!({ "args": { "command": "cargo", "args": ["test"] } }),
            json!({ "arguments": { "command": "cargo", "args": ["test"] } }),
            json!({ "input": { "command": "cargo", "args": ["test"] } }),
            json!({ "raw_input": { "command": "cargo", "args": ["test"] } }),
            json!({ "data": { "args": { "command": "cargo", "args": ["test"] } } }),
            json!({ "tool_call": { "arguments": { "command": "cargo", "args": ["test"] } } }),
            json!({ "data": { "tool_call": { "raw_input": { "command": "cargo", "args": ["test"] } } } }),
            json!({ "tool_call": { "arguments": r#"{"command":"cargo","args":["test"]}"# } }),
        ];

        for mut data in cases {
            data["tool_name"] = json!("run_command");
            data["summary"] = json!("Tool completed.");
            let summary = tool_call_finished_summary(&data, "run_command", false);
            assert!(
                summary.contains("Command: `cargo test`."),
                "{data}: {summary}"
            );
        }
    }

    #[test]
    fn successful_tool_result_params_do_not_put_diagnostic_in_error_field() {
        let config = Config {
            api_url: "http://den.test".to_string(),
            bear: "meta".to_string(),
            token: "token".to_string(),
            client: "zed".to_string(),
        };
        let params = tool_result_rpc_params(
            &config,
            "session-1",
            "run-1",
            "call-1",
            &json!({
                "tool_name": "fs_read_text_file",
                "status": "ok",
                "content": "",
                "structured_content": { "content": "hello" },
                "diagnostic": { "phase": "permission_local_tool_completed" }
            }),
            None,
        );

        assert_eq!(params["tool_name"], "fs_read_text_file");
        assert_eq!(
            params["diagnostic"]["phase"],
            "permission_local_tool_completed"
        );
        assert!(params["error"].is_null(), "{params}");
    }

    #[test]
    fn failed_tool_result_params_copy_diagnostic_to_error_field() {
        let config = Config {
            api_url: "http://den.test".to_string(),
            bear: "meta".to_string(),
            token: "token".to_string(),
            client: "zed".to_string(),
        };
        let params = tool_result_rpc_params(
            &config,
            "session-1",
            "run-1",
            "call-1",
            &json!({
                "tool_name": "fs_read_text_file",
                "status": "error",
                "content": "failed",
                "diagnostic": { "phase": "permission_local_tool_failed" }
            }),
            Some("attempt-1"),
        );

        assert_eq!(params["attempt_token"], "attempt-1");
        assert_eq!(params["error"]["phase"], "permission_local_tool_failed");
    }

    #[test]
    fn tool_call_finished_summary_for_placeholder_tool_exposes_details() {
        let data = json!({
            "tool_call_id": "call-unknown-1",
            "summary": "Tool completed.",
            "result": { "count": 3 }
        });

        let summary = tool_call_finished_summary(&data, "tool", false);

        assert!(summary.contains("Tool call completed"), "{summary}");
        assert!(summary.contains("call-unknown-1"), "{summary}");
        assert!(summary.contains("\"count\":3"), "{summary}");
        assert!(!matches!(
            summary.as_str(),
            "Tool completed." | "Tool failed."
        ));
    }

    #[test]
    fn tool_call_finished_summary_prefers_specific_error_detail() {
        let data = json!({
            "tool_name": "memory_read",
            "summary": "Tool failed.",
            "diagnostic": {
                "error": "permission denied reading pair/notes.md"
            }
        });

        assert_eq!(
            tool_call_finished_summary(&data, "memory_read", true),
            "permission denied reading pair/notes.md"
        );
    }

    #[test]
    fn bearwire_plan_update_entries_accepts_runtime_progress_detail() {
        let event = json!({
            "type": "run.progress",
            "data": {
                "kind": "plan_update",
                "text": null,
                "phase": "tool_result",
                "detail": {
                    "entries": [
                        { "id": "inspect", "title": "Inspect logs", "status": "completed" },
                        { "id": "patch", "title": "Patch ACP plan projection", "status": "in_progress" }
                    ]
                }
            }
        });

        let entries = bearwire_plan_update_entries(&event);

        assert_eq!(entries.as_array().map(Vec::len), Some(2));
        assert_eq!(entries[0]["title"], "Inspect logs");
        assert_eq!(entries[1]["status"], "in_progress");
    }

    #[test]
    fn bearwire_plan_update_entries_accepts_nested_plan_items() {
        let event = json!({
            "type": "run.progress",
            "data": {
                "kind": "plan_update",
                "detail": {
                    "plan": {
                        "items": [
                            { "id": "one", "title": "One", "status": "pending" }
                        ]
                    }
                }
            }
        });

        let entries = bearwire_plan_update_entries(&event);

        assert_eq!(entries.as_array().map(Vec::len), Some(1));
        assert_eq!(entries[0]["id"], "one");
    }
}
