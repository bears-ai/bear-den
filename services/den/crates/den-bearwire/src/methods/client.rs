use std::time::Duration;
use std::time::Instant;

use axum::http::HeaderMap;
use futures::StreamExt;
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

use bearwire_protocol::{
    methods::{
        deserialize_optional_string, deserialize_required_string, deserialize_string,
        ClientPermissionResultRequest,
    },
    wire::{BearWireEvent, ExecutionTargetWire, ToolCallFinishWire, ToolCallRequestedWire},
};
use den_core::{
    client_tools::{client_tool_policy_json_for_provider, ClientToolName},
    tools::{
        constants::DEN_WEB_FETCH,
        result_compaction::{
            compact_client_tool_result, compact_client_tool_result_with_artifact,
            ClientToolResultInput, ToolResultStatus,
        },
    },
    DenError,
};
use den_docket::{DocketPairBoundedOutcome, DocketService};
use den_http::{errors::CustomError, web_policy};
use den_protocol::{
    RoleRuntimeBinding, RuntimeApprovalDecision, RuntimeContinuation, RuntimeConversationRef,
    RuntimeToolResultStatus,
};
use den_runtime::{
    agent_loop::native_llm_handshake_timeout,
    bearwire_events,
    client_obligation_coordinator::{
        self, PermissionResultCoordinatorOutcome, ToolResultCoordinatorOutcome,
    },
    native_runtime::continue_native_client_turn_event_stream,
    runtime::bearwire_projection::wire::{tool_call_finish_wire, tool_call_wire},
    tool_output_artifacts::{
        create_tool_output_artifact, ToolOutputArtifactInput, ToolOutputArtifactRecord,
    },
    turn_obligations::{self, ExpectedResponderAction},
    turn_runner::{default_tool_continue_stream_context, TurnContinueRequest},
    turn_runs,
};
use den_service::{
    artifacts::{
        self, ArtifactStorageKind, ArtifactVisibility, AttachDocketArtifactInput,
        DocketArtifactRole, DocketArtifactTargetKind, FinalizeGitCommitArtifactInput,
        GitObjectFormat, ReserveArtifactInput,
    },
    bears::{db as bears_db, BearProfile},
    client_sessions, DenState,
};

use crate::auth::authenticated_bear;
use crate::methods::parse_params;
use crate::methods::run::{
    docket_bounded_slice_continuation, fail_run_lifecycle, persist_run_progress,
    persist_runtime_event_as_bearwire, report_pair_bounded_outcome, RunFailureReason,
};

fn deserialize_tool_result_status<'de, D>(deserializer: D) -> Result<ToolResultStatus, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let raw = deserialize_string(deserializer)?;
    ToolResultStatus::parse(&raw)
        .ok_or_else(|| serde::de::Error::custom(format!("unsupported tool result status: {raw}")))
}

#[derive(Debug, Clone, Deserialize)]
struct ClientToolResultRequest {
    #[serde(deserialize_with = "deserialize_required_string")]
    run_id: String,
    #[serde(deserialize_with = "deserialize_required_string")]
    session_id: String,
    #[serde(deserialize_with = "deserialize_required_string")]
    tool_call_id: String,
    #[serde(default, deserialize_with = "deserialize_optional_string")]
    attempt_token: Option<String>,
    #[serde(
        default = "default_tool_result_status",
        deserialize_with = "deserialize_tool_result_status"
    )]
    status: ToolResultStatus,
    #[serde(default, deserialize_with = "deserialize_optional_string")]
    tool_name: Option<String>,
    #[serde(default, deserialize_with = "deserialize_optional_string")]
    content: Option<String>,
    /// Intentionally raw: tool-specific structured result payloads vary by tool family.
    #[serde(default)]
    structured_content: Value,
    /// Intentionally raw: some tools surface structured error objects instead of plain text.
    #[serde(default)]
    error: Value,
}

fn default_tool_result_status() -> ToolResultStatus {
    ToolResultStatus::Ok
}

impl ClientToolResultRequest {
    fn into_run_session_input(self) -> (String, String, Option<String>, ClientToolResultInput) {
        let input = ClientToolResultInput::new(
            self.tool_call_id,
            self.tool_name,
            self.status,
            self.content,
            self.structured_content,
            self.error,
        );
        (self.run_id, self.session_id, self.attempt_token, input)
    }
}

fn permission_reason_text(reason: &Option<Value>) -> Option<String> {
    reason.as_ref().and_then(Value::as_str).map(str::to_string)
}

/// Recognize the one Cargo failure that a work sandbox cannot fix itself: its
/// deliberately offline, read-only dependency cache lacks a required crate.
/// This consumes the client tool's structured result, never Armature stderr.
fn cargo_offline_cache_miss_evidence(tool_name: Option<&str>, value: &Value) -> Option<Value> {
    if !matches!(tool_name, Some("process_run") | Some("run_command")) {
        return None;
    }
    let result = value.get("result").unwrap_or(value);
    let command = result.get("command").and_then(Value::as_str)?;
    if command != "cargo" || result.get("exit_code").and_then(Value::as_i64) == Some(0) {
        return None;
    }
    let stderr = result
        .get("stderr")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if !stderr.contains("offline mode") || !stderr.contains("no matching package named") {
        return None;
    }
    let package = stderr
        .split("no matching package named `")
        .nth(1)
        .and_then(|rest| rest.split('`').next())
        .filter(|name| !name.is_empty());
    Some(json!({
        "code": "cargo_offline_cache_miss",
        "stage": "validation",
        "retryable": false,
        "preparation_state": "not_attempted",
        "command": "cargo",
        "cwd": result.get("cwd").and_then(Value::as_str),
        "exit_code": result.get("exit_code"),
        "required_package": package,
        "action": "Prepare Rust dependencies with the hosted dependency tool, then retry Cargo.",
        "diagnostic": "Cargo could not resolve a required package from the sandbox's offline dependency cache.",
    }))
}

pub(crate) async fn persist_work_git_commit_artifact(
    state: &DenState,
    bear_id: Uuid,
    user_id: i32,
    session_id: &str,
    tool_name: Option<&str>,
    status: ToolResultStatus,
    structured_content: &Value,
) {
    if tool_name != Some("git_commit") || status != ToolResultStatus::Ok {
        return;
    }
    let result = structured_content
        .get("result")
        .unwrap_or(structured_content);
    if result.get("ok").and_then(Value::as_bool) != Some(true) {
        return;
    }
    let (Some(repository), Some(commit_oid)) = (
        result.get("repo_path").and_then(Value::as_str),
        result.get("sha").and_then(Value::as_str),
    ) else {
        return;
    };
    let work_run =
        den_docket::work_runs::get_live_work_run_by_session(&state.sqlx_pool, session_id)
            .await
            .ok()
            .flatten()
            .filter(|run| run.bear_id == bear_id);
    let pair_attempt = if work_run.is_none() {
        den_docket::PgDocketService::from_pool(&state.sqlx_pool)
            .get_live_session_task_execution_attempt_for_session(bear_id, session_id)
            .await
            .ok()
            .flatten()
    } else {
        None
    };
    let Some(task_id) = work_run
        .as_ref()
        .and_then(|run| run.executing_task_id)
        .or_else(|| pair_attempt.as_ref().map(|attempt| attempt.task_id))
    else {
        return;
    };
    let artifact = async {
        let artifact = artifacts::reserve_artifact(
            &state.sqlx_pool,
            ReserveArtifactInput {
                bear_id,
                created_by_user_id: Some(user_id),
                owner_profile: BearProfile::Pair,
                kind: "git_commit".to_string(),
                title: result.get("subject").and_then(Value::as_str).map(str::to_string),
                summary: Some(format!("Git commit {commit_oid}")),
                content_type: None,
                storage_kind: ArtifactStorageKind::ExternalGitCommit,
                visibility: ArtifactVisibility::BearVisible,
                provenance: json!({ "source": "bearwire_client", "tool": "git_commit", "session_id": session_id, "work_run_id": work_run.as_ref().map(|run| run.id), "execution_attempt_id": pair_attempt.as_ref().map(|attempt| attempt.id) }),
                metadata: json!({ "tool_result": result }),
                expires_at: None,
            },
        ).await?;
        let artifact = artifacts::finalize_git_commit_artifact(
            &state.sqlx_pool,
            FinalizeGitCommitArtifactInput {
                artifact_ref: artifact.artifact_ref.clone(), bear_id,
                repository: repository.to_string(), object_format: GitObjectFormat::Sha1,
                commit_oid: commit_oid.to_string(), output_ref: commit_oid.to_string(),
                metadata: json!({ "work_run_id": work_run.as_ref().map(|run| run.id), "execution_attempt_id": pair_attempt.as_ref().map(|attempt| attempt.id) }),
            },
        ).await?;
        artifacts::attach_docket_artifact(
            &state.sqlx_pool,
            AttachDocketArtifactInput {
                artifact_ref: artifact.artifact_ref, bear_id,
                target_kind: DocketArtifactTargetKind::Task,
                target_id: task_id,
                role: DocketArtifactRole::Output,
                metadata: json!({ "candidate": true, "work_run_id": work_run.as_ref().map(|run| run.id), "execution_attempt_id": pair_attempt.as_ref().map(|attempt| attempt.id) }),
                created_by_user_id: Some(user_id),
            },
        ).await
    }.await;
    if let Err(error) = artifact {
        tracing::warn!(%error, session_id, "could not persist Git commit artifact");
    }
}

async fn persist_work_cargo_evidence(
    state: &DenState,
    session_id: &str,
    tool_name: Option<&str>,
    structured_content: &Value,
) {
    let Some(evidence) = cargo_offline_cache_miss_evidence(tool_name, structured_content) else {
        return;
    };
    let Ok(Some(work_run)) =
        den_docket::work_runs::get_live_work_run_by_session(&state.sqlx_pool, session_id).await
    else {
        return;
    };
    if let Err(error) = den_docket::work_runs::merge_work_run_result_refs(
        &state.sqlx_pool,
        work_run.id,
        &json!({ "cargo_failure": evidence }),
    )
    .await
    {
        tracing::warn!(work_run_id = %work_run.id, %error, "could not persist Cargo diagnostic evidence");
    }
}

fn require_settlement_result(
    result: Option<turn_runs::TurnObligationResultRow>,
    context: &'static str,
) -> Result<turn_runs::TurnObligationResultRow, CustomError> {
    result.ok_or_else(|| {
        CustomError::System(format!(
            "{context} should include persisted obligation result row"
        ))
    })
}

fn bearwire_finish_payload(input: &ClientToolResultInput, compacted: Value) -> ToolCallFinishWire {
    let error_message = (input.status != ToolResultStatus::Ok)
        .then(|| input.content.as_deref().or_else(|| input.error.as_str()))
        .flatten();
    tool_call_finish_wire(
        &input.tool_call_id,
        input.tool_name.as_deref(),
        input.status.as_str(),
        None,
        error_message,
        input.content.as_deref(),
        (!input.structured_content.is_null()).then(|| input.structured_content.clone()),
        (!input.error.is_null()).then(|| input.error.clone()),
        Some(compacted),
    )
}

pub(crate) fn continuation_watchdog_timeout() -> Duration {
    continuation_watchdog_timeout_from_raw(
        std::env::var("BEARS_BEARWIRE_CONTINUATION_WATCHDOG_MS")
            .ok()
            .as_deref(),
    )
}

fn continuation_watchdog_timeout_from_raw(raw: Option<&str>) -> Duration {
    let millis = raw
        .and_then(|value| value.trim().parse::<u64>().ok())
        .map(|value| value.clamp(1_000, 600_000))
        .unwrap_or(30_000);
    Duration::from_millis(millis)
}

pub(crate) fn continuation_first_event_watchdog_timeout(
    handshake_timeout: Duration,
    idle_timeout: Duration,
) -> Duration {
    handshake_timeout
        .checked_add(idle_timeout)
        .unwrap_or(Duration::from_millis(u64::MAX))
}

fn continuation_retry_pauses() -> &'static [Duration] {
    static PAUSES: [Duration; 3] = [
        Duration::from_secs(2),
        Duration::from_secs(4),
        Duration::from_secs(54),
    ];
    &PAUSES
}

fn is_retryable_continuation_stream_error(message: &str) -> bool {
    let message = message.to_ascii_lowercase();
    [
        "llm byte stream produced no data",
        "connection reset",
        "connection closed",
        "connection aborted",
        "broken pipe",
        "unexpected eof",
        "stream reset",
        "http/2 stream",
        "request timeout",
        "request timed out",
        "operation timed out",
        "gateway timeout",
        "upstream timeout",
        "temporarily unavailable",
        "too many requests",
        "rate limit",
        "rate_limit",
        "overloaded",
        "status 408",
        "status: 408",
        "http 408",
        "status 429",
        "status: 429",
        "http 429",
        "status 500",
        "status: 500",
        "http 500",
        "status 502",
        "status: 502",
        "http 502",
        "status 503",
        "status: 503",
        "http 503",
        "status 504",
        "status: 504",
        "http 504",
    ]
    .iter()
    .any(|needle| message.contains(needle))
}

fn should_retry_continuation_error(message: &str, attempt_index: usize) -> bool {
    is_retryable_continuation_stream_error(message)
        && attempt_index < continuation_retry_pauses().len()
}

fn is_sibling_continuation_claim(err: &DenError) -> bool {
    matches!(
        err,
        DenError::TechnicalBudgetContinuationAlreadyClaimed { .. }
    )
}

fn should_retry_non_terminal_continuation_eof(
    terminal_event_seen: bool,
    wait_event_seen: bool,
    cancellation_seen: bool,
    attempt_index: usize,
) -> bool {
    // ponytail: provider/client disconnects sometimes surface as clean EOF instead of
    // an error; retry this only while the existing continuation retry budget remains.
    !terminal_event_seen
        && !wait_event_seen
        && !cancellation_seen
        && attempt_index < continuation_retry_pauses().len()
}

type ContinuationStreamBoundary = crate::methods::run::RuntimeStreamBoundary;

fn continuation_stream_boundary(
    event: &den_protocol::RuntimeStreamEvent,
) -> ContinuationStreamBoundary {
    crate::methods::run::runtime_stream_boundary(event)
}

/// Metadata sufficient to identify a stalled request without persisting model
/// arguments, tool input, or provider payloads.
fn safe_tool_request_forensics(event: &den_protocol::RuntimeStreamEvent) -> Option<Value> {
    let den_protocol::RuntimeStreamEvent::Semantic(
        den_protocol::RuntimeSemanticEvent::ToolCallRequested {
            tool_call_id,
            tool_name,
            approval_request_id,
            approval_required,
            run_id,
            ..
        },
    ) = event
    else {
        return None;
    };

    Some(json!({
        "tool_call_id": tool_call_id,
        "tool_name": tool_name,
        "request_class": if run_id.as_deref().is_some_and(|id| !id.trim().is_empty()) {
            "obligation_backed"
        } else {
            "den_owned"
        },
        "approval_required": approval_required,
        "approval_request_id_present": approval_request_id.as_deref().is_some_and(|id| !id.trim().is_empty()),
        "response_status": "not_observed_before_timeout",
    }))
}

fn continuation_retry_pauses_seconds() -> Vec<u64> {
    continuation_retry_pauses()
        .iter()
        .map(Duration::as_secs)
        .collect()
}

fn continuation_conversation_id(session: &client_sessions::ClientSessionRow) -> String {
    session
        .resolved_conversation_id
        .clone()
        .unwrap_or_else(|| session.conversation_id.clone())
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct WebFetchPermissionPayload {
    #[serde(rename = "tool_name")]
    _tool_name: String,
    arguments: WebFetchPermissionArguments,
    #[serde(default, rename = "tool_call_id")]
    _tool_call_id: Option<String>,
    #[serde(default, rename = "approval_required")]
    _approval_required: Option<bool>,
    #[serde(default, rename = "approval_request_id")]
    _approval_request_id: Option<String>,
    #[serde(default, rename = "request_id")]
    _request_id: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct WebFetchPermissionArguments {
    url: Option<String>,
    host: Option<String>,
    site_account: Option<String>,
}

async fn record_web_fetch_approval_from_permission(
    pool: &sqlx::PgPool,
    bear_id: uuid::Uuid,
    user_id: i32,
    decision: &str,
    obligation_payload: &Value,
) -> Result<(), CustomError> {
    let tool_name = obligation_payload
        .get("tool_name")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if !matches!(decision, "allow_once" | "allow_site_account" | "allow_host") {
        return Ok(());
    }
    let Some(descriptor) =
        den_core::tools::descriptor::builtin_den_tool_descriptor_for_provider_name(tool_name)
    else {
        return Ok(());
    };
    if descriptor.name != DEN_WEB_FETCH {
        return Ok(());
    }
    let payload: WebFetchPermissionPayload = serde_json::from_value(obligation_payload.clone())
        .map_err(|_| {
            CustomError::ValidationError(
                "web_fetch permission payload has an invalid shape".to_string(),
            )
        })?;
    let url = payload
        .arguments
        .url
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            CustomError::ValidationError("web_fetch permission payload missing url".to_string())
        })?;
    let (scope_kind, scope_value) = if decision == "allow_host" {
        let host = match payload
            .arguments
            .host
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            Some(host) => web_policy::normalize_web_host(host)?,
            None => web_policy::normalize_web_url(url)?.host,
        };
        ("host", host)
    } else if decision == "allow_site_account" {
        let scope_value = payload
            .arguments
            .site_account
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
            .or_else(|| {
                let normalized = web_policy::normalize_web_url(url).ok()?;
                if normalized.host == "github.com" {
                    let account = url
                        .split("github.com/")
                        .nth(1)?
                        .split('/')
                        .find(|segment| !segment.is_empty())?;
                    Some(format!("github.com/{account}"))
                } else {
                    None
                }
            })
            .ok_or_else(|| {
                CustomError::ValidationError(
                    "web_fetch permission payload missing supported site account scope".to_string(),
                )
            })?;
        ("site_account", scope_value)
    } else {
        let host = match payload
            .arguments
            .host
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            Some(host) => web_policy::normalize_web_host(host)?,
            None => web_policy::normalize_web_url(url)?.host,
        };
        ("host", host)
    };
    let ttl_seconds = if decision == "allow_once" {
        Some(60 * 60)
    } else {
        None
    };
    web_policy::record_web_approval(
        pool,
        bear_id,
        scope_kind,
        &scope_value,
        Some(user_id),
        "acp",
        ttl_seconds,
    )
    .await?;
    Ok(())
}

fn continuation_unavailable_response(
    run: &turn_runs::TurnRunRow,
    session_id: &str,
    conversation_id: &str,
    obligation_state: &str,
    obligation_id: impl ToString,
) -> Value {
    json!({
        "ok": false,
        "status": "continuation_unavailable",
        "reason": "native_agent_loop_session_not_found",
        "run_state": run.state,
        "obligation_state": obligation_state,
        "diagnostic": {
            "component": "den-bearwire",
            "phase": "native_session_missing_before_continuation",
            "run_id": run.run_id,
            "session_id": session_id,
            "conversation_id": conversation_id,
            "obligation_id": obligation_id.to_string(),
            "message": "Den cannot accept this client result for continuation because the in-memory native agent loop session is not present. This usually means Den restarted or the run was orphaned; retry the turn in a fresh session."
        }
    })
}

pub(crate) fn spawn_continuation_task(
    state: &DenState,
    run: turn_runs::TurnRunRow,
    binding_id: String,
    conversation_id: String,
    continuation: RuntimeContinuation,
) {
    let pool = state.sqlx_pool.clone();
    let livestream_state = state.clone();
    let config = state.config.clone();
    let memory_stores = state.memory_stores.clone();
    let request_id = Uuid::new_v4();
    let (cancel_handle, mut cancel_rx) = state.turn_cancellations.register(
        run.session_id.clone(),
        request_id,
        Some(conversation_id.clone()),
    );
    let _ = cancel_handle.record_run_id(&run.run_id);
    tokio::spawn(async move {
        let _cancel_handle = cancel_handle;
        let Some(run) = turn_runs::begin_claimed_run_continuation(&pool, &run.run_id)
            .await
            .unwrap_or_else(|error| {
                tracing::error!(
                    run_id = %run.run_id,
                    error = %error,
                    "failed to begin claimed BearWire continuation"
                );
                None
            })
        else {
            tracing::info!(
                run_id = %run.run_id,
                "BearWire continuation claim was already consumed"
            );
            return;
        };
        let continuation_started_at = Instant::now();
        persist_run_progress(
            &pool,
            &run.session_id,
            &run.run_id,
            run.bear_id,
            run.user_id,
            continuation_started_at,
            "continuation_started",
            "Continuing Pair stance run after client result…",
            json!({
                "request_id": request_id,
            }),
        )
        .await;
        let binding = RoleRuntimeBinding {
            binding_id,
            compatibility_backend: Some("native".to_string()),
        };
        let retry_pauses = continuation_retry_pauses();
        'continuation_attempts: for attempt_index in 0..=retry_pauses.len() {
            let attempt_number = attempt_index + 1;
            if attempt_index > 0 {
                let pause = retry_pauses[attempt_index - 1];
                persist_run_progress(
                    &pool,
                    &run.session_id,
                    &run.run_id,
                    run.bear_id,
                    run.user_id,
                    continuation_started_at,
                    "continuation_retry_waiting",
                    &format!(
                        "LLM continuation stream stalled; retrying after {} seconds…",
                        pause.as_secs()
                    ),
                    json!({
                        "request_id": request_id,
                        "attempt": attempt_number,
                        "pause_seconds": pause.as_secs(),
                        "retry_pauses_seconds": continuation_retry_pauses_seconds(),
                    }),
                )
                .await;
                tokio::select! {
                    changed = cancel_rx.changed() => {
                        if changed.is_ok() && *cancel_rx.borrow() {
                            tracing::info!(
                                session_id = %run.session_id,
                                run_id = %run.run_id,
                                request_id = %request_id,
                                attempt = attempt_number,
                                "BearWire continuation retry wait observed cancellation"
                            );
                            return;
                        }
                    }
                    () = tokio::time::sleep(pause) => {}
                }
            }
            let continuation_future = continue_native_client_turn_event_stream(
                TurnContinueRequest {
                    sqlx_pool: &pool,
                    config: config.as_ref(),
                    memory_stores: &memory_stores,
                    request_id,
                    run_id: Some(&run.run_id),
                    client_session_id: &run.session_id,
                    conversation: RuntimeConversationRef {
                        id: conversation_id.clone(),
                    },
                    binding: &binding,
                    continuation: continuation.clone(),
                    stream_context: default_tool_continue_stream_context(),
                },
                BearProfile::Pair,
            );
            tokio::pin!(continuation_future);
            let result = tokio::select! {
                changed = cancel_rx.changed() => {
                    if changed.is_ok() && *cancel_rx.borrow() {
                        tracing::info!(
                            session_id = %run.session_id,
                            run_id = %run.run_id,
                            request_id = %request_id,
                            "BearWire continuation startup observed cancellation"
                        );
                        return;
                    }
                    continuation_future.await
                }
                result = &mut continuation_future => result,
            };
            match result {
                Ok((_continuation, mut stream)) => {
                    persist_run_progress(
                        &pool,
                        &run.session_id,
                        &run.run_id,
                        run.bear_id,
                        run.user_id,
                        continuation_started_at,
                        "continuation_model_stream_waiting",
                        "Waiting for model output after local tool/permission result…",
                        json!({
                            "request_id": request_id,
                        }),
                    )
                    .await;
                    let mut first_event_seen = false;
                    let mut provider_activity_seen = false;
                    let mut runtime_event_count = 0usize;
                    let mut terminal_event_seen = false;
                    let mut wait_event_seen = false;
                    let mut cancellation_seen = false;
                    let mut last_event_kind: Option<&'static str> = None;
                    // Keep only request metadata that is safe to persist in a failure record.
                    // Never retain tool arguments or raw provider payloads here.
                    let mut last_tool_request: Option<Value> = None;
                    let mut last_runtime_event_at: Option<Instant> = None;
                    let mut last_provider_activity_at: Option<Instant> = None;
                    let mut last_event_sequence: Option<i64> = None;
                    let mut retryable_stream_error: Option<String> = None;
                    let mut fatal_stream_failure_seen = false;
                    let idle_watchdog_timeout = continuation_watchdog_timeout();
                    let handshake_timeout = native_llm_handshake_timeout();
                    let first_event_watchdog_timeout = continuation_first_event_watchdog_timeout(
                        handshake_timeout,
                        idle_watchdog_timeout,
                    );
                    loop {
                        let watchdog_phase = if first_event_seen {
                            "between_runtime_events"
                        } else if provider_activity_seen {
                            "provider_inactive_before_first_semantic_event"
                        } else {
                            "provider_handshake"
                        };
                        let watchdog_timeout = if first_event_seen || provider_activity_seen {
                            idle_watchdog_timeout
                        } else {
                            first_event_watchdog_timeout
                        };
                        let item = tokio::select! {
                            changed = cancel_rx.changed() => {
                                if changed.is_ok() && *cancel_rx.borrow() {
                                    cancellation_seen = true;
                                    tracing::info!(
                                        session_id = %run.session_id,
                                        run_id = %run.run_id,
                                        request_id = %request_id,
                                        "BearWire continuation stream observed cancellation"
                                    );
                                    break;
                                }
                                continue;
                            }
                            timed = tokio::time::timeout(watchdog_timeout, stream.next()) => {
                                match timed {
                                    Ok(item) => item,
                                    Err(_) => {
                                        let last_event_age_ms = last_runtime_event_at
                                            .map(|at| at.elapsed().as_millis());
                                        let last_provider_activity_age_ms = last_provider_activity_at
                                            .map(|at| at.elapsed().as_millis());
                                        let context = json!({
                                            "continuation_request_id": request_id,
                                            "watchdog_phase": watchdog_phase,
                                            "watchdog_timeout_ms": watchdog_timeout.as_millis(),
                                            "first_event_watchdog_timeout_ms": first_event_watchdog_timeout.as_millis(),
                                            "handshake_timeout_ms": handshake_timeout.as_millis(),
                                            "idle_watchdog_timeout_ms": idle_watchdog_timeout.as_millis(),
                                            "continuation_elapsed_ms": continuation_started_at.elapsed().as_millis(),
                                            "runtime_event_count": runtime_event_count,
                                            "first_event_seen": first_event_seen,
                                            "provider_activity_seen": provider_activity_seen,
                                            "last_provider_activity_age_ms": last_provider_activity_age_ms,
                                            "terminal_event_seen": terminal_event_seen,
                                            "wait_event_seen": wait_event_seen,
                                            "last_event_kind": last_event_kind,
                                            "last_tool_request": last_tool_request,
                                            "last_event_sequence": last_event_sequence,
                                            "last_event_age_ms": last_event_age_ms,
                                            "diagnostic_note": "The continuation watchdog observes typed provider activity and semantic runtime events. Provider activity is process-local and is not persisted to BearWire or transcript history.",
                                        });
                                        let message = if first_event_seen {
                                            format!(
                                                "Den received the client result and continuation request {request_id} emitted runtime events, but no further provider or semantic activity arrived within {}ms.",
                                                watchdog_timeout.as_millis()
                                            )
                                        } else if provider_activity_seen {
                                            format!(
                                                "Den received provider bytes for continuation request {request_id}, but provider activity stopped before the first semantic event for {}ms.",
                                                watchdog_timeout.as_millis()
                                            )
                                        } else {
                                            format!(
                                                "Den received the client result and started continuation request {request_id}, but no runtime event arrived within {}ms. This includes the configured LLM handshake allowance ({}ms) plus the continuation idle watchdog ({}ms). This usually means the resumed model/runtime stream stalled before emitting its first event.",
                                                watchdog_timeout.as_millis(),
                                                handshake_timeout.as_millis(),
                                                idle_watchdog_timeout.as_millis()
                                            )
                                        };
                                        fail_run_lifecycle(
                                            &pool,
                                            &run.session_id,
                                            &run.run_id,
                                            run.bear_id,
                                            run.user_id,
                                            RunFailureReason::ContinuationWatchdogTimeout,
                                            message,
                                            Some(context),
                                        )
                                        .await;
                                        fatal_stream_failure_seen = true;
                                        break;
                                    }
                                }
                            }
                        };
                        let Some(item) = item else {
                            break;
                        };
                        match item {
                            Ok(den_protocol::RuntimeStreamEvent::ProviderActivity) => {
                                provider_activity_seen = true;
                                last_provider_activity_at = Some(Instant::now());
                            }
                            Ok(runtime_event) => {
                                runtime_event_count += 1;
                                last_runtime_event_at = Some(Instant::now());
                                let event_kind =
                                    crate::methods::run::runtime_event_kind(&runtime_event);
                                last_event_kind = Some(event_kind);
                                last_tool_request = safe_tool_request_forensics(&runtime_event);
                                let stream_boundary = continuation_stream_boundary(&runtime_event);
                                match stream_boundary {
                                    ContinuationStreamBoundary::Terminal => {
                                        terminal_event_seen = true;
                                    }
                                    ContinuationStreamBoundary::ClientWait => {
                                        wait_event_seen = true;
                                    }
                                    ContinuationStreamBoundary::BoundedSlice => {
                                        if let Some(continuation) =
                                            docket_bounded_slice_continuation(
                                                report_pair_bounded_outcome(
                                                    &livestream_state,
                                                    run.user_id,
                                                    run.bear_id,
                                                    &run.session_id,
                                                    &run.run_id,
                                                    DocketPairBoundedOutcome::Progress,
                                                )
                                                .await,
                                            )
                                        {
                                            let _ = turn_runs::transition_run(
                                                &pool,
                                                &run.run_id,
                                                turn_runs::TurnRunState::Continuing,
                                                Some("docket_bounded_slice"),
                                            )
                                            .await;
                                            spawn_continuation_task(
                                                &livestream_state,
                                                run.clone(),
                                                binding.binding_id.clone(),
                                                conversation_id.clone(),
                                                continuation,
                                            );
                                            // This worker hands control to the newly spawned
                                            // continuation; do not retry or fail this stream EOF.
                                            wait_event_seen = true;
                                        } else {
                                            wait_event_seen = true;
                                        }
                                    }
                                    ContinuationStreamBoundary::Continue => {}
                                }
                                if !first_event_seen {
                                    first_event_seen = true;
                                    persist_run_progress(
                                        &pool,
                                        &run.session_id,
                                        &run.run_id,
                                        run.bear_id,
                                        run.user_id,
                                        continuation_started_at,
                                        "continuation_first_runtime_event",
                                        "Received first runtime event after continuation.",
                                        json!({
                                            "request_id": request_id,
                                            "event_kind": event_kind,
                                            "runtime_event_count": runtime_event_count,
                                        }),
                                    )
                                    .await;
                                }
                                persist_runtime_event_as_bearwire(
                                    &livestream_state,
                                    &pool,
                                    &run.session_id,
                                    &run.run_id,
                                    run.bear_id,
                                    run.user_id,
                                    runtime_event,
                                    request_id,
                                    Some(continuation_started_at),
                                )
                                .await;
                                last_event_sequence =
                                    bearwire_events::latest_event_sequence(&pool, &run.session_id)
                                        .await
                                        .ok()
                                        .flatten();
                                if stream_boundary != ContinuationStreamBoundary::Continue {
                                    break;
                                }
                            }
                            Err(err) => {
                                if is_sibling_continuation_claim(&err) {
                                    tracing::info!(
                                        session_id = %run.session_id,
                                        run_id = %run.run_id,
                                        request_id = %request_id,
                                        "BearWire continuation claim is already owned by a sibling; leaving the run active"
                                    );
                                    return;
                                }
                                let failure_reason = match err {
                                    DenError::RunStateConflict { .. } => {
                                        RunFailureReason::ContinuationRunStateConflict
                                    }
                                    DenError::LoopControlLedgerPersistence(_) => {
                                        RunFailureReason::ContinuationLoopControlLedgerPersistence
                                    }
                                    DenError::TechnicalBudgetContinuation(_) => {
                                        RunFailureReason::ContinuationTechnicalBudgetSetup
                                    }
                                    _ => RunFailureReason::ContinuationStreamError,
                                };
                                let err_message = err.to_string();
                                let is_retryable =
                                    is_retryable_continuation_stream_error(&err_message);
                                if should_retry_continuation_error(&err_message, attempt_index) {
                                    retryable_stream_error = Some(err_message);
                                    break;
                                }
                                fail_run_lifecycle(
                                    &pool,
                                    &run.session_id,
                                    &run.run_id,
                                    run.bear_id,
                                    run.user_id,
                                    failure_reason,
                                    err_message,
                                    Some(json!({
                                        "attempt": attempt_number,
                                        "retryable": is_retryable,
                                        "retry_exhausted": is_retryable,
                                        "retry_pauses_seconds": continuation_retry_pauses_seconds(),
                                    })),
                                )
                                .await;
                                break 'continuation_attempts;
                            }
                        }
                    }
                    if fatal_stream_failure_seen {
                        break 'continuation_attempts;
                    }
                    if let Some(err_message) = retryable_stream_error {
                        persist_run_progress(
                        &pool,
                        &run.session_id,
                        &run.run_id,
                        run.bear_id,
                        run.user_id,
                        continuation_started_at,
                        "continuation_stream_retryable_error",
                        "LLM continuation stream hit a transient error; will retry after backoff.",
                        json!({
                            "request_id": request_id,
                            "attempt": attempt_number,
                            "error": err_message,
                            "retry_pauses_seconds": continuation_retry_pauses_seconds(),
                        }),
                    )
                    .await;
                        continue 'continuation_attempts;
                    }
                    let terminal_or_wait_seen = terminal_event_seen || wait_event_seen;
                    if should_retry_non_terminal_continuation_eof(
                        terminal_event_seen,
                        wait_event_seen,
                        cancellation_seen,
                        attempt_index,
                    ) {
                        let pause = retry_pauses[attempt_index];
                        persist_run_progress(
                        &pool,
                        &run.session_id,
                        &run.run_id,
                        run.bear_id,
                        run.user_id,
                        continuation_started_at,
                        "continuation_stream_non_terminal_eof_retrying",
                        "LLM continuation stream ended before a terminal runtime event; will retry after backoff.",
                        json!({
                            "request_id": request_id,
                            "attempt": attempt_number,
                            "pause_seconds": pause.as_secs(),
                            "runtime_event_count": runtime_event_count,
                            "first_event_seen": first_event_seen,
                            "terminal_event_seen": terminal_event_seen,
                            "wait_event_seen": wait_event_seen,
                            "cancellation_seen": cancellation_seen,
                            "last_event_kind": last_event_kind,
                            "last_event_sequence": last_event_sequence,
                            "retry_pauses_seconds": continuation_retry_pauses_seconds(),
                        }),
                    )
                    .await;
                        continue 'continuation_attempts;
                    }
                    if !terminal_or_wait_seen && !cancellation_seen {
                        fail_run_lifecycle(
                        &pool,
                        &run.session_id,
                        &run.run_id,
                        run.bear_id,
                        run.user_id,
                        RunFailureReason::ContinuationStreamEndedWithoutRuntimeTerminal,
                        if first_event_seen {
                            "Continuation stream ended after non-terminal runtime events but did not emit a tool request, completion, cancellation, or error.".to_string()
                        } else {
                            "Continuation stream ended without emitting any runtime event after the local tool/permission result.".to_string()
                        },
                        Some(json!({
                            "request_id": request_id,
                            "runtime_event_count": runtime_event_count,
                            "first_event_seen": first_event_seen,
                            "terminal_event_seen": terminal_event_seen,
                            "wait_event_seen": wait_event_seen,
                            "cancellation_seen": cancellation_seen,
                            "last_event_kind": last_event_kind,
                            "attempt": attempt_number,
                            "retry_exhausted": true,
                            "retry_pauses_seconds": continuation_retry_pauses_seconds(),
                        })),
                    )
                    .await;
                    } else {
                        persist_run_progress(
                            &pool,
                            &run.session_id,
                            &run.run_id,
                            run.bear_id,
                            run.user_id,
                            continuation_started_at,
                            if terminal_event_seen {
                                "continuation_stream_ended_after_terminal"
                            } else {
                                "continuation_stream_ended_after_wait"
                            },
                            if terminal_event_seen {
                                "Continuation stream ended after a terminal runtime event."
                            } else {
                                "Continuation stream ended after emitting another client wait."
                            },
                            json!({
                                "request_id": request_id,
                                "runtime_event_count": runtime_event_count,
                                "first_event_seen": first_event_seen,
                                "terminal_event_seen": terminal_event_seen,
                                "wait_event_seen": wait_event_seen,
                                "last_event_kind": last_event_kind,
                            }),
                        )
                        .await;
                    }
                    break 'continuation_attempts;
                }
                Err(err) => {
                    let err_message = err.to_string();
                    let is_retryable = is_retryable_continuation_stream_error(&err_message);
                    if should_retry_continuation_error(&err_message, attempt_index) {
                        persist_run_progress(
                        &pool,
                        &run.session_id,
                        &run.run_id,
                        run.bear_id,
                        run.user_id,
                        continuation_started_at,
                        "continuation_start_retryable_error",
                        "LLM continuation startup hit a transient error; will retry after backoff.",
                        json!({
                            "request_id": request_id,
                            "attempt": attempt_number,
                            "error": err_message,
                            "retry_pauses_seconds": continuation_retry_pauses_seconds(),
                        }),
                    )
                    .await;
                        continue 'continuation_attempts;
                    }
                    fail_run_lifecycle(
                        &pool,
                        &run.session_id,
                        &run.run_id,
                        run.bear_id,
                        run.user_id,
                        RunFailureReason::ContinuationStartFailed,
                        err_message,
                        Some(json!({
                            "attempt": attempt_number,
                            "retryable": is_retryable,
                            "retry_exhausted": is_retryable,
                            "retry_pauses_seconds": continuation_retry_pauses_seconds(),
                        })),
                    )
                    .await;
                    break 'continuation_attempts;
                }
            }
        }
    });
}

#[derive(Debug, Clone, Deserialize)]
struct ClientToolLeaseRequest {
    #[serde(deserialize_with = "deserialize_required_string")]
    run_id: String,
    #[serde(deserialize_with = "deserialize_required_string")]
    session_id: String,
    #[serde(deserialize_with = "deserialize_required_string")]
    obligation_id: String,
    #[serde(deserialize_with = "deserialize_required_string")]
    tool_call_id: String,
    #[serde(default, deserialize_with = "deserialize_optional_string")]
    attempt_token: Option<String>,
}

async fn authenticated_tool_lease(
    state: &DenState,
    headers: &HeaderMap,
    params: &Value,
) -> Result<(ClientToolLeaseRequest, turn_runs::TurnRunRow), CustomError> {
    let (user_id, bear) = authenticated_bear(state, headers, params).await?;
    let request: ClientToolLeaseRequest = parse_params(params)?;
    let run = turn_runs::get_run(&state.sqlx_pool, &request.run_id)
        .await?
        .ok_or_else(|| CustomError::ValidationError("BearWire run not found".to_string()))?;
    if run.bear_id != bear.id || run.user_id != user_id || run.session_id != request.session_id {
        return Err(CustomError::Authorization(
            "run does not belong to authenticated Bear/session".to_string(),
        ));
    }
    if matches!(run.state.as_str(), "completed" | "failed" | "cancelled") {
        return Err(CustomError::ValidationError(format!(
            "cannot lease tool execution for terminal run {}",
            run.run_id
        )));
    }
    Ok((request, run))
}

pub(crate) async fn client_tool_claim_result(
    state: &DenState,
    headers: &HeaderMap,
    params: &Value,
) -> Result<Value, CustomError> {
    let (request, _run) = authenticated_tool_lease(state, headers, params).await?;
    let obligation_id = Uuid::parse_str(&request.obligation_id)
        .map_err(|_| CustomError::ValidationError("invalid obligation_id".to_string()))?;
    let attempt_token = Uuid::new_v4().to_string();
    let hash = turn_obligations::lease_attempt_token_hash(&attempt_token);
    let Some(obligation) = turn_obligations::claim_tool_execution(
        &state.sqlx_pool,
        obligation_id,
        &request.run_id,
        &request.session_id,
        &request.tool_call_id,
        &hash,
    )
    .await?
    else {
        return Ok(json!({ "ok": false, "status": "claim_rejected" }));
    };
    Ok(json!({
        "ok": true,
        "status": "claimed",
        "attempt_token": attempt_token,
        "lease_expires_at": obligation.lease_expires_at,
        "renew_after_ms": turn_obligations::TOOL_LEASE_RENEW_AFTER_SECONDS * 1000,
    }))
}

pub(crate) async fn client_tool_renew_result(
    state: &DenState,
    headers: &HeaderMap,
    params: &Value,
) -> Result<Value, CustomError> {
    let (request, _run) = authenticated_tool_lease(state, headers, params).await?;
    let obligation_id = Uuid::parse_str(&request.obligation_id)
        .map_err(|_| CustomError::ValidationError("invalid obligation_id".to_string()))?;
    let attempt_token = request.attempt_token.ok_or_else(|| {
        CustomError::ValidationError("client.tool.renew requires attempt_token".to_string())
    })?;
    let hash = turn_obligations::lease_attempt_token_hash(&attempt_token);
    let Some(obligation) = turn_obligations::renew_tool_execution(
        &state.sqlx_pool,
        obligation_id,
        &request.run_id,
        &request.session_id,
        &request.tool_call_id,
        &hash,
    )
    .await?
    else {
        return Ok(json!({ "ok": false, "status": "lease_lost" }));
    };
    Ok(json!({
        "ok": true,
        "status": "renewed",
        "lease_expires_at": obligation.lease_expires_at,
        "renew_after_ms": turn_obligations::TOOL_LEASE_RENEW_AFTER_SECONDS * 1000,
    }))
}

pub(crate) async fn client_tool_result_result(
    state: &DenState,
    headers: &HeaderMap,
    params: &Value,
) -> Result<Value, CustomError> {
    let (user_id, bear) = authenticated_bear(state, headers, params).await?;
    let request: ClientToolResultRequest = parse_params(params)?;
    let (run_id, session_id, attempt_token, input) = request.into_run_session_input();
    let tool_call_id = input.tool_call_id.clone();
    let status = input.status;
    let Some(run) = turn_runs::get_run(&state.sqlx_pool, &run_id).await? else {
        return Ok(json!({
            "ok": false,
            "status": "late_result_ignored",
            "reason": "run_not_found",
        }));
    };
    if run.bear_id != bear.id || run.user_id != user_id || run.session_id != session_id {
        return Err(CustomError::Authorization(
            "run does not belong to authenticated Bear/session".to_string(),
        ));
    }
    if matches!(run.state.as_str(), "completed" | "failed" | "cancelled") {
        return Ok(json!({
            "ok": false,
            "status": "late_result_ignored",
            "run_state": run.state,
            "reason": "run_is_terminal",
        }));
    }
    let obligation =
        turn_obligations::get_tool_call_obligation(&state.sqlx_pool, &run_id, &tool_call_id)
            .await?
            .ok_or_else(|| {
                CustomError::ValidationError(
                    "BearWire tool result has no persisted tool-call obligation".to_string(),
                )
            })?;
    if !turn_obligations::obligation_accepts_responder_action(
        &obligation,
        ExpectedResponderAction::ToolResult,
    ) {
        return Err(CustomError::ValidationError(format!(
            "BearWire tool obligation {} does not accept client.tool.result (expected {}, state {})",
            obligation.id,
            obligation.expected_responder_action,
            obligation.state
        )));
    }
    let session = client_sessions::find_for_user_bear_session(
        &state.sqlx_pool,
        user_id,
        &bear.slug,
        &session_id,
    )
    .await?
    .ok_or_else(|| {
        CustomError::Session("BearWire session disappeared during run continuation".to_string())
    })?;
    let continuation_conversation_id = continuation_conversation_id(&session);
    let mut compacted = compact_client_tool_result(&input);
    if compacted.truncated {
        if let Ok(artifact) = create_tool_output_artifact(
            &state.sqlx_pool,
            ToolOutputArtifactInput {
                bear_id: bear.id,
                user_id: Some(user_id),
                session_id: session_id.clone(),
                conversation_id: Some(continuation_conversation_id.clone()),
                run_id: Some(run_id.clone()),
                tool_call_id: tool_call_id.clone(),
                tool_name: input.tool_name.clone(),
                source: "bearwire_client",
                content_text: input.content.clone(),
                content_json: Some(params.clone()),
                metadata: json!({ "status": status.as_str() }),
            },
        )
        .await
        {
            compacted = compact_client_tool_result_with_artifact(
                &input,
                Some(compacted_artifact_ref(&artifact)),
            );
        }
    }
    let payload = compacted.payload.clone();
    let event_payload = bearwire_finish_payload(&input, payload.clone());
    persist_work_git_commit_artifact(
        state,
        bear.id,
        user_id,
        &session_id,
        input.tool_name.as_deref(),
        status,
        &input.structured_content,
    )
    .await;
    persist_work_cargo_evidence(
        state,
        &session_id,
        input.tool_name.as_deref(),
        &input.structured_content,
    )
    .await;

    if !den_runtime::native_runtime::native_client_session_exists(
        &continuation_conversation_id,
        &session_id,
    ) {
        return Ok(continuation_unavailable_response(
            &run,
            &session_id,
            &continuation_conversation_id,
            &obligation.state,
            obligation.id,
        ));
    }
    let binding_id = bears_db::profile_binding_id(&state.sqlx_pool, bear.id, BearProfile::Pair)
        .await?
        .ok_or_else(|| {
            CustomError::System("Bear pair profile binding not configured".to_string())
        })?;
    let attempt_token = attempt_token.ok_or_else(|| {
        CustomError::ValidationError("client.tool.result requires attempt_token".to_string())
    })?;
    let attempt_token_hash = turn_obligations::lease_attempt_token_hash(&attempt_token);
    let coordinator_outcome = client_obligation_coordinator::record_and_settle_tool_result(
        &state.sqlx_pool,
        &run,
        &obligation,
        &attempt_token_hash,
        "tool",
        &tool_call_id,
        payload.clone(),
    )
    .await?;
    match coordinator_outcome {
        ToolResultCoordinatorOutcome::DuplicateConflict { existing_hash } => {
            Err(CustomError::ValidationError(format!(
                "conflicting duplicate tool result for {tool_call_id}; existing hash {existing_hash}"
            )))
        }
        ToolResultCoordinatorOutcome::DuplicateIdentical { result, run_state } => Ok(json!({
            "ok": true,
            "duplicate": true,
            "result_id": result.id,
            "run_state": run_state,
            "obligation_state": obligation.state,
        })),
        ToolResultCoordinatorOutcome::IgnoredLateResult {
            run_state,
            obligation_state,
        } => Ok(json!({
            "ok": false,
            "status": "late_result_ignored",
            "run_state": run_state,
            "obligation_state": obligation_state,
        })),
        ToolResultCoordinatorOutcome::WaitingForMoreClientResults {
            run: transitioned,
            open_obligations,
            result,
        } => {
            let result = require_settlement_result(result, "record-and-settle tool outcome")?;
            let mut event = BearWireEvent::tool_call_finished(event_payload.clone());
            event.bear_id = Some(bear.id.to_string());
            event.human_id = Some(user_id.to_string());
            event.session_id = Some(session_id.clone());
            event.run_id = Some(run_id.clone());
            event.subject = Some(format!("resource/tool_call/{tool_call_id}"));
            let persisted = bearwire_events::append_bearwire_event(
                &state.sqlx_pool,
                &session_id,
                Some(bear.id),
                Some(user_id),
                event,
            )
            .await?;
            let content = compacted.content.clone();
            let continuation_status = match status {
                ToolResultStatus::Ok => RuntimeToolResultStatus::Ok,
                ToolResultStatus::Timeout => RuntimeToolResultStatus::Timeout,
                ToolResultStatus::Error
                | ToolResultStatus::Incomplete
                | ToolResultStatus::Cancelled => RuntimeToolResultStatus::Error,
            };
            den_runtime::native_runtime::record_native_client_tool_result(
                &state.sqlx_pool,
                &continuation_conversation_id,
                &session_id,
                &Uuid::new_v4().to_string(),
                Some(&run_id),
                &tool_call_id,
                obligation.permission_id.as_deref(),
                continuation_status,
                content,
            )
            .await?;
            Ok(json!({
                "ok": true,
                "duplicate": false,
                "result_id": result.id,
                "event_sequence": persisted.sequence_no,
                "run_state": transitioned.map(|run| run.state).unwrap_or_else(|| "unknown".to_string()),
                "continuation": "waiting_for_more_client_results",
                "open_obligation_count": open_obligations.len(),
                "open_obligations": open_obligations.into_iter().map(|obligation| json!({
                    "obligation_id": obligation.id,
                    "kind": obligation.kind,
                    "expected_responder_action": obligation.expected_responder_action,
                    "tool_call_id": obligation.tool_call_id,
                    "permission_id": obligation.permission_id,
                    "state": obligation.state,
                })).collect::<Vec<_>>(),
            }))
        }
        ToolResultCoordinatorOutcome::ContinueModel {
            run: transitioned,
            result,
        } => {
            let result = require_settlement_result(result, "record-and-settle tool outcome")?;
            let mut event = BearWireEvent::tool_call_finished(event_payload.clone());
            event.bear_id = Some(bear.id.to_string());
            event.human_id = Some(user_id.to_string());
            event.session_id = Some(session_id.clone());
            event.run_id = Some(run_id.clone());
            event.subject = Some(format!("resource/tool_call/{tool_call_id}"));
            let persisted = bearwire_events::append_bearwire_event(
                &state.sqlx_pool,
                &session_id,
                Some(bear.id),
                Some(user_id),
                event,
            )
            .await?;
            let content = compacted.content.clone();
            let continuation_status = match status {
                ToolResultStatus::Ok => RuntimeToolResultStatus::Ok,
                ToolResultStatus::Timeout => RuntimeToolResultStatus::Timeout,
                ToolResultStatus::Error
                | ToolResultStatus::Incomplete
                | ToolResultStatus::Cancelled => RuntimeToolResultStatus::Error,
            };
            let Some(transitioned) = transitioned else {
                return Ok(json!({
                    "ok": true,
                    "duplicate": false,
                    "result_id": result.id,
                    "event_sequence": persisted.sequence_no,
                    "run_state": "continuing",
                    "continuation": "already_started",
                }));
            };
            spawn_continuation_task(
                state,
                transitioned.clone(),
                binding_id,
                continuation_conversation_id,
                RuntimeContinuation::ToolResult {
                    tool_call_id: tool_call_id.clone(),
                    approval_request_id: obligation.permission_id.clone(),
                    status: continuation_status,
                    content,
                },
            );
            Ok(json!({
                "ok": true,
                "duplicate": false,
                "result_id": result.id,
                "event_sequence": persisted.sequence_no,
                "run_state": transitioned.state,
                "continuation": "started",
            }))
        }
    }
}

pub(crate) async fn client_permission_result_result(
    state: &DenState,
    headers: &HeaderMap,
    params: &Value,
) -> Result<Value, CustomError> {
    let (user_id, bear) = authenticated_bear(state, headers, params).await?;
    let request: ClientPermissionResultRequest = parse_params(params)?;
    let reason = permission_reason_text(&request.reason);
    let run_id = request.run_id;
    let session_id = request.session_id;
    let permission_id = request.permission_id;
    let obligation_id = request.obligation_id;
    let decision = request.decision;
    let Some(run) = turn_runs::get_run(&state.sqlx_pool, &run_id).await? else {
        return Ok(json!({
            "ok": false,
            "status": "late_result_ignored",
            "reason": "run_not_found",
        }));
    };
    if run.bear_id != bear.id || run.user_id != user_id || run.session_id != session_id {
        return Err(CustomError::Authorization(
            "run does not belong to authenticated Bear/session".to_string(),
        ));
    }
    if matches!(run.state.as_str(), "completed" | "failed" | "cancelled") {
        return Ok(json!({
            "ok": false,
            "status": "late_result_ignored",
            "run_state": run.state,
            "reason": "run_is_terminal",
        }));
    }
    let obligation =
        turn_obligations::get_permission_obligation(&state.sqlx_pool, &run_id, &permission_id)
            .await?
            .ok_or_else(|| {
                CustomError::ValidationError(
                    "BearWire permission result has no persisted permission obligation".to_string(),
                )
            })?;
    if let Some(obligation_id) = obligation_id
        .as_deref()
        .map(str::trim)
        .filter(|id| !id.is_empty())
    {
        if obligation.id.to_string() != obligation_id {
            return Err(CustomError::ValidationError(
                "BearWire permission result obligation_id does not match persisted permission obligation".to_string(),
            ));
        }
    }
    if !turn_obligations::obligation_accepts_responder_action(
        &obligation,
        ExpectedResponderAction::PermissionDecision,
    ) {
        // The client may receive/replay a permission prompt after its decision has
        // already advanced this obligation to tool-result handling. Treat that
        // stale acknowledgement as idempotent rather than surfacing an RPC error.
        tracing::debug!(
            session_id = %session_id,
            run_id = %run_id,
            permission_id = %permission_id,
            obligation_id = %obligation.id,
            expected_responder_action = %obligation.expected_responder_action,
            obligation_state = %obligation.state,
            "ignoring stale BearWire permission result"
        );
        return Ok(json!({
            "ok": true,
            "duplicate": true,
            "status": "late_result_ignored",
            "run_state": run.state,
            "obligation_state": obligation.state,
        }));
    }
    let normalized_decision = decision.normalized();
    let payload = json!({
        "permission_id": permission_id,
        "decision": normalized_decision,
        "reason": request.reason.unwrap_or(Value::Null),
    });

    let session = client_sessions::find_for_user_bear_session(
        &state.sqlx_pool,
        user_id,
        &bear.slug,
        &session_id,
    )
    .await?
    .ok_or_else(|| {
        CustomError::Session("BearWire session disappeared during run continuation".to_string())
    })?;
    let continuation_conversation_id = continuation_conversation_id(&session);
    if !den_runtime::native_runtime::native_client_session_exists(
        &continuation_conversation_id,
        &session_id,
    ) {
        return Ok(continuation_unavailable_response(
            &run,
            &session_id,
            &continuation_conversation_id,
            &obligation.state,
            obligation.id,
        ));
    }
    let binding_id = bears_db::profile_binding_id(&state.sqlx_pool, bear.id, BearProfile::Pair)
        .await?
        .ok_or_else(|| {
            CustomError::System("Bear pair profile binding not configured".to_string())
        })?;
    let coordinator_outcome = client_obligation_coordinator::record_and_settle_permission_result(
        &state.sqlx_pool,
        &run,
        &obligation,
        normalized_decision,
        "permission",
        &permission_id,
        payload.clone(),
    )
    .await?;
    match coordinator_outcome {
        PermissionResultCoordinatorOutcome::DuplicateConflict { existing_hash } => {
            Err(CustomError::ValidationError(format!(
                "conflicting duplicate permission result for {permission_id}; existing hash {existing_hash}"
            )))
        }
        PermissionResultCoordinatorOutcome::DuplicateIdentical { result, run_state } => Ok(json!({
            "ok": true,
            "duplicate": true,
            "result_id": result.id,
            "run_state": run_state,
            "obligation_state": obligation.state,
        })),
        PermissionResultCoordinatorOutcome::IgnoredLateResult {
            run_state,
            obligation_state,
        } => Ok(json!({
            "ok": false,
            "status": "late_result_ignored",
            "run_state": run_state,
            "obligation_state": obligation_state,
        })),
        PermissionResultCoordinatorOutcome::DispatchLocalTool {
            run: transitioned,
            tool_obligation,
            tool_call_id,
            tool_name,
            args,
            result,
        } => {
            let result = require_settlement_result(result, "record-and-settle permission outcome")?;
            den_docket::work_runs::settle_attached_work_run_permission(
                &state.sqlx_pool,
                &session_id,
            )
            .await?;
            if normalized_decision == "granted" {
                record_web_fetch_approval_from_permission(
                    &state.sqlx_pool,
                    bear.id,
                    user_id,
                    decision.raw(),
                    &obligation.request_payload,
                )
                .await?;
            }
            let event_type = match normalized_decision {
                "granted" => "permission.granted",
                "expired" => "permission.expired",
                _ => "permission.denied",
            };
            let mut event = BearWireEvent::ephemeral(event_type, payload);
            event.bear_id = Some(bear.id.to_string());
            event.human_id = Some(user_id.to_string());
            event.session_id = Some(session_id.clone());
            event.run_id = Some(run_id.clone());
            event.subject = Some(format!("resource/permission_request/{permission_id}"));
            let persisted = bearwire_events::append_bearwire_event(
                &state.sqlx_pool,
                &session_id,
                Some(bear.id),
                Some(user_id),
                event,
            )
            .await?;
            let policy = ClientToolName::from_provider_alias(&tool_name)
                .map(|_| client_tool_policy_json_for_provider(&tool_name));
            let mut dispatch_event = BearWireEvent::tool_call_requested(ToolCallRequestedWire {
                expected_responder_action: Some("tool_result".to_string()),
                obligation_id: Some(tool_obligation.id.to_string()),
                tool_call: tool_call_wire(
                    &tool_call_id,
                    &tool_name,
                    None,
                    "function",
                    &args,
                ),
                approval_required: false,
                execution_target: ExecutionTargetWire::ArmatureLocal,
                policy: policy.clone(),
                approval_request_id: Some(permission_id.clone()),
                reason: None,
            });
            dispatch_event.bear_id = Some(bear.id.to_string());
            dispatch_event.human_id = Some(user_id.to_string());
            dispatch_event.session_id = Some(session_id.clone());
            dispatch_event.run_id = Some(run_id.clone());
            dispatch_event.subject = Some(format!("resource/tool_call/{tool_call_id}"));
            let dispatch_persisted = bearwire_events::append_bearwire_event(
                &state.sqlx_pool,
                &session_id,
                Some(bear.id),
                Some(user_id),
                dispatch_event,
            )
            .await?;
            Ok(json!({
                "ok": true,
                "duplicate": false,
                "result_id": result.id,
                "event_sequence": persisted.sequence_no,
                "local_tool_event_sequence": dispatch_persisted.sequence_no,
                "run_state": transitioned.map(|run| run.state).unwrap_or_else(|| "unknown".to_string()),
                "continuation": "waiting_for_tool_result",
                "obligation_state": tool_obligation.state,
                "local_tool_request": {
                    "tool_call_id": tool_call_id,
                    "tool_name": tool_name,
                    "result_tool_name": tool_name,
                    "args": args,
                    "permission_id": permission_id,
                    "obligation_id": tool_obligation.id.to_string(),
                    "policy": policy,
                }
            }))
        }
        PermissionResultCoordinatorOutcome::ContinueModel { run: transitioned, result } => {
            let result = require_settlement_result(result, "record-and-settle permission outcome")?;
            den_docket::work_runs::settle_attached_work_run_permission(
                &state.sqlx_pool,
                &session_id,
            )
            .await?;
            if normalized_decision == "granted" {
                record_web_fetch_approval_from_permission(
                    &state.sqlx_pool,
                    bear.id,
                    user_id,
                    decision.raw(),
                    &obligation.request_payload,
                )
                .await?;
            }
            let event_type = match normalized_decision {
                "granted" => "permission.granted",
                "expired" => "permission.expired",
                _ => "permission.denied",
            };
            let mut event = BearWireEvent::ephemeral(event_type, payload);
            event.bear_id = Some(bear.id.to_string());
            event.human_id = Some(user_id.to_string());
            event.session_id = Some(session_id.clone());
            event.run_id = Some(run_id.clone());
            event.subject = Some(format!("resource/permission_request/{permission_id}"));
            let persisted = bearwire_events::append_bearwire_event(
                &state.sqlx_pool,
                &session_id,
                Some(bear.id),
                Some(user_id),
                event,
            )
            .await?;
            let decision = if normalized_decision == "granted" {
                RuntimeApprovalDecision::Approve
            } else {
                RuntimeApprovalDecision::Deny
            };
            let Some(transitioned) = transitioned else {
                return Ok(json!({
                    "ok": true,
                    "duplicate": false,
                    "result_id": result.id,
                    "event_sequence": persisted.sequence_no,
                    "run_state": "continuing",
                    "continuation": "already_started",
                }));
            };
            spawn_continuation_task(
                state,
                transitioned.clone(),
                binding_id,
                continuation_conversation_id,
                RuntimeContinuation::ApprovalDecision {
                    approval_request_id: permission_id.clone(),
                    tool_call_id: obligation.tool_call_id.clone(),
                    decision,
                    reason,
                },
            );
            Ok(json!({
                "ok": true,
                "duplicate": false,
                "result_id": result.id,
                "event_sequence": persisted.sequence_no,
                "run_state": transitioned.state,
                "continuation": "started",
            }))
        }
    }
}

fn compacted_artifact_ref(artifact: &ToolOutputArtifactRecord) -> &str {
    artifact
        .durable_artifact_ref
        .as_deref()
        .unwrap_or(&artifact.artifact_ref)
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durable_tool_output_ref_is_preferred_for_compaction() {
        let artifact = ToolOutputArtifactRecord {
            id: Uuid::nil(),
            artifact_ref: "tool-output://legacy".to_string(),
            durable_artifact_ref: Some("artifact_durable".to_string()),
        };
        assert_eq!(compacted_artifact_ref(&artifact), "artifact_durable");
    }

    #[test]
    fn legacy_tool_output_ref_is_used_when_durable_creation_fails() {
        let artifact = ToolOutputArtifactRecord {
            id: Uuid::nil(),
            artifact_ref: "tool-output://legacy".to_string(),
            durable_artifact_ref: None,
        };
        assert_eq!(compacted_artifact_ref(&artifact), "tool-output://legacy");
    }

    #[test]
    fn cargo_offline_cache_miss_is_classified_from_structured_process_result() {
        let evidence = cargo_offline_cache_miss_evidence(
            Some("process_run"),
            &json!({
                "command": "cargo",
                "cwd": "/workspace/services/den",
                "exit_code": 101,
                "stderr": "error: no matching package named `serde` found\nnote: offline mode (via `--offline`) can sometimes cause surprising resolution failures"
            }),
        )
        .expect("offline Cargo cache miss should be recognized");
        assert_eq!(evidence["code"], "cargo_offline_cache_miss");
        assert_eq!(evidence["required_package"], "serde");
        assert_eq!(evidence["preparation_state"], "not_attempted");
    }

    #[test]
    fn cargo_failures_without_offline_cache_miss_are_not_misclassified() {
        assert!(cargo_offline_cache_miss_evidence(
            Some("process_run"),
            &json!({"command": "cargo", "exit_code": 101, "stderr": "error: could not compile"}),
        )
        .is_none());
    }

    #[test]
    fn permission_reason_text_accepts_only_string_reasons() {
        assert_eq!(
            permission_reason_text(&Some(json!("network access"))),
            Some("network access".to_string())
        );
        assert_eq!(
            permission_reason_text(&Some(json!({"code": "denied"}))),
            None
        );
        assert_eq!(permission_reason_text(&None), None);
    }

    #[test]
    fn web_fetch_permission_payload_rejects_unknown_fields() {
        let payload = json!({
            "tool_name": DEN_WEB_FETCH,
            "arguments": {"url": "https://example.com"},
            "approval_required": true,
            "unexpected": true,
        });

        assert!(serde_json::from_value::<WebFetchPermissionPayload>(payload).is_err());
    }

    #[test]
    fn web_fetch_permission_arguments_reject_unknown_fields() {
        let payload = json!({
            "tool_name": DEN_WEB_FETCH,
            "arguments": {"url": "https://example.com", "unexpected": true},
        });

        assert!(serde_json::from_value::<WebFetchPermissionPayload>(payload).is_err());
    }

    #[test]
    fn continuation_retry_schedule_and_idle_error_classification() {
        assert_eq!(continuation_retry_pauses_seconds(), vec![2, 4, 54]);
        assert!(is_sibling_continuation_claim(
            &DenError::TechnicalBudgetContinuationAlreadyClaimed {
                run_id: "run-1".to_string(),
            }
        ));
        assert!(!is_sibling_continuation_claim(
            &DenError::RunStateConflict {
                operation: "technical budget continuation claim",
                run_id: "run-1".to_string(),
                expected_state: "running",
                actual_state: Some("completed".to_string()),
            }
        ));
        assert!(!is_sibling_continuation_claim(
            &DenError::RunStateConflict {
                operation: "technical budget continuation claim",
                run_id: "run-1".to_string(),
                expected_state: "running",
                actual_state: None,
            }
        ));
        for message in [
            "Server Error: LLM byte stream produced no data for 30s",
            "Server Error: connection reset by peer",
            "Server Error: HTTP/2 stream reset",
            "Server Error: upstream timeout",
            "Server Error: provider temporarily unavailable",
            "Server Error: rate limit exceeded",
            "Server Error: overloaded",
            "Server Error: status 429 Too Many Requests",
            "Server Error: HTTP 503 Service Unavailable",
            "Server Error: gateway timeout",
        ] {
            assert!(
                is_retryable_continuation_stream_error(message),
                "expected retryable: {message}"
            );
        }
        for message in [
            "Server Error: some other continuation failure",
            "Validation Error: invalid continuation payload",
            "Authorization failed for continuation",
            "model not found",
            "permission denied",
        ] {
            assert!(
                !is_retryable_continuation_stream_error(message),
                "expected non-retryable: {message}"
            );
            assert!(
                !should_retry_continuation_error(message, 0),
                "expected no retry: {message}"
            );
        }
        assert!(should_retry_continuation_error(
            "Server Error: LLM byte stream produced no data for 30s",
            0
        ));
        assert!(!should_retry_continuation_error(
            "Server Error: LLM byte stream produced no data for 30s",
            continuation_retry_pauses().len()
        ));
    }

    #[test]
    fn non_terminal_continuation_eof_uses_existing_retry_budget() {
        assert!(should_retry_non_terminal_continuation_eof(
            false, false, false, 0
        ));
        assert!(!should_retry_non_terminal_continuation_eof(
            true, false, false, 0
        ));
        assert!(!should_retry_non_terminal_continuation_eof(
            false, true, false, 0
        ));
        assert!(!should_retry_non_terminal_continuation_eof(
            false, false, true, 0
        ));
        assert!(!should_retry_non_terminal_continuation_eof(
            false,
            false,
            false,
            continuation_retry_pauses().len()
        ));
    }

    #[test]
    fn continuation_eof_retries_only_before_terminal_wait_or_cancellation() {
        assert!(should_retry_non_terminal_continuation_eof(
            false, false, false, 0
        ));
        assert!(!should_retry_non_terminal_continuation_eof(
            true, false, false, 0
        ));
        assert!(!should_retry_non_terminal_continuation_eof(
            false, true, false, 0
        ));
        assert!(!should_retry_non_terminal_continuation_eof(
            false, false, true, 0
        ));
        assert!(!should_retry_non_terminal_continuation_eof(
            false, false, false, 3
        ));
    }

    #[test]
    fn continuation_stream_boundary_stops_on_terminal_or_client_wait() {
        let completed = den_protocol::RuntimeStreamEvent::Semantic(
            den_protocol::RuntimeSemanticEvent::TurnCompleted { turn: None },
        );
        let failed =
            den_protocol::RuntimeStreamEvent::Semantic(den_protocol::RuntimeSemanticEvent::Error {
                message: "boom".to_string(),
                detail: None,
                error_type: None,
                request_id: None,
                context: None,
            });
        let wait = den_protocol::RuntimeStreamEvent::Semantic(
            den_protocol::RuntimeSemanticEvent::ToolCallRequested {
                tool_call_id: "tool-1".to_string(),
                tool_name: "read_text_file".to_string(),
                title: None,
                kind: None,
                arguments: json!({}),
                approval_request_id: None,
                approval_required: false,
                approval_reason: None,
                run_id: None,
            },
        );
        let checkpoint = den_protocol::RuntimeStreamEvent::Semantic(
            den_protocol::RuntimeSemanticEvent::ToolCallRequested {
                tool_call_id: "checkpoint-1".to_string(),
                tool_name: den_runtime::agent_loop::RUNTIME_CHECKPOINT_TOOL_NAME.to_string(),
                title: None,
                kind: None,
                arguments: json!({}),
                approval_request_id: None,
                approval_required: false,
                approval_reason: None,
                run_id: None,
            },
        );
        let den_tool = den_protocol::RuntimeStreamEvent::Semantic(
            den_protocol::RuntimeSemanticEvent::ToolCallRequested {
                tool_call_id: "den-tool-1".to_string(),
                tool_name: "list_jobs".to_string(),
                title: None,
                kind: None,
                arguments: json!({}),
                approval_request_id: None,
                approval_required: false,
                approval_reason: None,
                run_id: None,
            },
        );
        let den_approval_wait = den_protocol::RuntimeStreamEvent::Semantic(
            den_protocol::RuntimeSemanticEvent::ToolCallRequested {
                tool_call_id: "den-approval-1".to_string(),
                tool_name: "web_fetch".to_string(),
                title: None,
                kind: None,
                arguments: json!({"url":"https://example.com"}),
                approval_request_id: Some("approval-1".to_string()),
                approval_required: true,
                approval_reason: Some("network access".to_string()),
                run_id: None,
            },
        );
        let delta = den_protocol::RuntimeStreamEvent::Semantic(
            den_protocol::RuntimeSemanticEvent::AssistantTextDelta {
                text: "still streaming".to_string(),
            },
        );
        let bounded_slice = den_protocol::RuntimeStreamEvent::Semantic(
            den_protocol::RuntimeSemanticEvent::BoundedSlice {
                reason: "task remains actionable".to_string(),
            },
        );

        assert_eq!(
            continuation_stream_boundary(&completed),
            ContinuationStreamBoundary::Terminal
        );
        assert_eq!(
            continuation_stream_boundary(&failed),
            ContinuationStreamBoundary::Terminal
        );
        assert_eq!(
            continuation_stream_boundary(&wait),
            ContinuationStreamBoundary::ClientWait
        );
        assert_eq!(
            continuation_stream_boundary(&checkpoint),
            ContinuationStreamBoundary::Continue
        );
        assert_eq!(
            continuation_stream_boundary(&den_tool),
            ContinuationStreamBoundary::Continue
        );
        assert_eq!(
            continuation_stream_boundary(&den_approval_wait),
            ContinuationStreamBoundary::ClientWait
        );
        assert_eq!(
            continuation_stream_boundary(&bounded_slice),
            ContinuationStreamBoundary::BoundedSlice
        );
        assert_eq!(
            continuation_stream_boundary(&delta),
            ContinuationStreamBoundary::Continue
        );
    }

    #[test]
    fn safe_tool_request_forensics_excludes_arguments() {
        let event = den_protocol::RuntimeStreamEvent::Semantic(
            den_protocol::RuntimeSemanticEvent::ToolCallRequested {
                tool_call_id: "tool-1".to_string(),
                tool_name: "web_fetch".to_string(),
                title: None,
                kind: None,
                arguments: json!({"url": "https://secret.example/token"}),
                approval_request_id: None,
                approval_required: false,
                approval_reason: None,
                run_id: None,
            },
        );
        let evidence = safe_tool_request_forensics(&event).expect("tool request evidence");
        assert_eq!(evidence["tool_name"], "web_fetch");
        assert_eq!(evidence["request_class"], "den_owned");
        assert_eq!(evidence["response_status"], "not_observed_before_timeout");
        assert!(evidence.get("arguments").is_none());
        assert!(!evidence.to_string().contains("secret.example"));
    }

    #[test]
    fn continuation_watchdog_timeout_defaults_and_clamps() {
        assert_eq!(
            continuation_watchdog_timeout_from_raw(None),
            Duration::from_secs(30)
        );
        assert_eq!(
            continuation_watchdog_timeout_from_raw(Some("1")),
            Duration::from_secs(1)
        );
        assert_eq!(
            continuation_watchdog_timeout_from_raw(Some("999999999")),
            Duration::from_mins(10)
        );
    }

    #[test]
    fn first_event_watchdog_includes_handshake_allowance() {
        assert_eq!(
            continuation_first_event_watchdog_timeout(
                Duration::from_mins(2),
                Duration::from_secs(30),
            ),
            Duration::from_secs(150)
        );
    }
}
