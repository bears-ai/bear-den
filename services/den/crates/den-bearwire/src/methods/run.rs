use std::{collections::BTreeMap, fmt, path::Path, time::Instant};

use axum::http::HeaderMap;
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::{types::time::OffsetDateTime, Row};
use uuid::Uuid;

use bearwire_protocol::{
    lifecycle::RunRecoveryHandoff,
    methods::{RunCancelRequest, RunRecoverRequest, RunStartRequest, RunStateRequest},
    wire::BearWireEvent,
};
use den_docket::{
    DocketExecutionAttemptRelease, DocketExecutionAttemptStart, DocketExecutionBindingKind,
    DocketExecutionHost, DocketExecutionHostKind, DocketFocusedExecutionAcquire,
    DocketFocusedExecutionBinding, DocketPairBoundedOutcome, DocketPairBoundedOutcomeReport,
    DocketPairContinuationDecision, DocketService, PgDocketService,
};
use den_http::errors::CustomError;
use den_protocol::{RoleRuntimeBinding, RuntimeContinuation};
use den_runtime::{
    agent_loop::{LedgerEvidenceRef, LoopControlDecisionKind, LoopControlLedgerInput},
    bearwire_events,
    current_task::preview_session_current_task_selection,
    native_runtime::start_native_profile_turn_event_stream,
    runtime::bearwire_projection::wire::runtime_stream_event_to_bearwire_events,
    runtime_error_ux::{log_sample, run_failure_projection, runtime_event_history_marker},
    runtime_exception_events::{
        self, NewRuntimeExceptionEvent, RuntimeExceptionContext, RuntimeExceptionSeverity,
    },
    surface_projection::bearwire_client_method_for_action,
    turn_ids::{ClientSessionId, TurnRunId},
    turn_obligations,
    turn_runner::TurnStartRequest,
    turn_runs, turn_steps,
};
use den_service::{
    bears::{
        db as bears_db, render_turn_fragment, repository_prompt_fragment_registry, BearProfile,
    },
    bifrost::BifrostCatalogEntry,
    client_sessions,
    conversation::events::{
        canonical_persistence_context, persist_canonical_conversation_record,
        CanonicalConversationRecord, ConversationEventProvenance,
    },
    DenState,
};

use crate::auth::authenticated_bear;
use crate::methods::{parse_params, DEFAULT_CLIENT};

// A focused Docket task must reach the native turn stream before `/focus` claims
// that autonomous work has started. This is intentionally bounded so an unavailable
// provider does not leave the ACP request open indefinitely.

/// BearWire-owned, durable subset of a normal start request. Reject unknown
/// fields on recovery so a future runtime-only field cannot become durable by
/// accident.
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct TechnicalBudgetRecoveryStartPayload {
    client: String,
    cwd: Option<String>,
    conversation_id: String,
    prompt: String,
    prompt_context: Option<Value>,
    client_context: Option<Value>,
    requested_mode: Option<String>,
}

/// Sanitized replayable request fields. Recovery runs these through the normal
/// `run.start` lifecycle, which re-resolves server-owned configuration and authority.
fn technical_budget_recovery_start_payload(
    client: &str,
    cwd: Option<&str>,
    conversation_id: &str,
    prompt: &str,
    prompt_context: Option<&Value>,
    client_context: Option<&Value>,
    requested_mode: Option<&str>,
) -> Value {
    json!(TechnicalBudgetRecoveryStartPayload {
        client: client.to_string(),
        cwd: cwd.map(str::to_string),
        conversation_id: conversation_id.to_string(),
        prompt: prompt.to_string(),
        prompt_context: prompt_context.cloned(),
        client_context: client_context.cloned(),
        requested_mode: requested_mode.map(str::to_string),
    })
}

#[derive(Debug, Deserialize)]
struct CanonicalClientWaitingEvent {
    permission: CanonicalClientWaitingReference,
    tool_call: CanonicalClientWaitingReference,
}

#[derive(Debug, Deserialize)]
struct CanonicalClientWaitingReference {
    id: String,
}

/// Stable, system-generated reasons for a BearWire run failure.
///
/// These values are persisted and emitted on the wire, so keep their explicit
/// string forms stable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RunFailureReason {
    DescriptorResolutionFailed,
    InitialStreamWatchdogTimeout,
    StreamError,
    StartFailed,
    ClientObligationTimeout,
    PermissionDecisionExpired,
    ServerRestartInterrupted,
    CommandOutcomeUnknown,
    #[cfg(test)]
    RuntimeInternal,
    ContinuationWatchdogTimeout,
    ContinuationStreamError,
    ContinuationRunStateConflict,
    ContinuationTechnicalBudgetSetup,
    ContinuationLoopControlLedgerPersistence,
    ContinuationStreamEndedWithoutRuntimeTerminal,
    ContinuationStartFailed,
}

impl RunFailureReason {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::DescriptorResolutionFailed => "descriptor_resolution_failed",
            Self::InitialStreamWatchdogTimeout => "initial_stream_watchdog_timeout",
            Self::StreamError => "stream_error",
            Self::StartFailed => "start_failed",
            Self::ClientObligationTimeout => "client_obligation_timeout",
            Self::PermissionDecisionExpired => "permission_decision_expired",
            Self::ServerRestartInterrupted => "server_restart_interrupted",
            Self::CommandOutcomeUnknown => "command_outcome_unknown",
            #[cfg(test)]
            Self::RuntimeInternal => "runtime_internal",
            Self::ContinuationWatchdogTimeout => "continuation_watchdog_timeout",
            Self::ContinuationStreamError => "continuation_stream_error",
            Self::ContinuationRunStateConflict => "continuation_run_state_conflict",
            Self::ContinuationTechnicalBudgetSetup => "continuation_technical_budget_setup_failed",
            Self::ContinuationLoopControlLedgerPersistence => {
                "continuation_loop_control_ledger_persistence_failed"
            }
            Self::ContinuationStreamEndedWithoutRuntimeTerminal => {
                "continuation_stream_ended_without_runtime_terminal"
            }
            Self::ContinuationStartFailed => "continuation_start_failed",
        }
    }
}

impl fmt::Display for RunFailureReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

fn den_owned_tool_call(tool_name: &str) -> bool {
    matches!(
        den_runtime::turn_waits::resolve_tool_execution_owner(tool_name),
        Ok(den_runtime::turn_waits::ToolExecutionOwner::Den)
    )
}

// Runtime status/error UX policy is surface-agnostic. Keep product copy,
// model-continuity summaries, and marker wording in den-runtime::runtime_error_ux.
// BearWire should only transport/persist those projections for this wire method.
async fn bear_display_name(pool: &sqlx::PgPool, bear_id: uuid::Uuid) -> String {
    bears_db::get_bear(pool, bear_id)
        .await
        .ok()
        .flatten()
        .map(|bear| bear.name)
        .filter(|name| !name.trim().is_empty())
        .unwrap_or_else(|| "The bear".to_string())
}

fn initial_stream_interruption_message() -> &'static str {
    "Den lost the model connection before the response finished. Your conversation and completed tool results were preserved. Send another message to retry."
}

fn retryable_stream_interruption_event(run_id: &str, detail: serde_json::Value) -> BearWireEvent {
    BearWireEvent::ephemeral(
        "run.interrupted",
        json!({
            "run_id": run_id,
            "message": initial_stream_interruption_message(),
            "retryable": true,
            "reason": "initial_stream_interrupted",
            "context": detail,
        }),
    )
}

async fn persist_visible_runtime_marker(
    pool: &sqlx::PgPool,
    session_id: &str,
    run_id: &str,
    bear_id: uuid::Uuid,
    user_id: i32,
    marker_kind: &str,
    marker_text: String,
    mut metadata: serde_json::Value,
) {
    let Ok(Some(session)) =
        client_sessions::find_for_user_bear_session_id(pool, user_id, bear_id, session_id).await
    else {
        return;
    };
    let conversation_id = session
        .resolved_conversation_id
        .clone()
        .unwrap_or_else(|| session.conversation_id.clone());
    let provenance = ConversationEventProvenance::client_session(session_id.to_string());
    metadata["source"] = json!("den.bearwire");
    metadata["event"] = json!("runtime_marker");
    metadata["scope_id"] = json!(run_id);
    metadata["request_id"] = json!(format!("{run_id}:{marker_kind}"));
    metadata["marker_kind"] = json!(marker_kind);
    metadata["run_id"] = json!(run_id);
    let _ = persist_canonical_conversation_record(
        &canonical_persistence_context(
            pool.clone(),
            bear_id,
            Some(user_id),
            conversation_id,
            Some(session_id.to_string()),
            Some(run_id.to_string()),
            provenance.scope_id,
            false,
        ),
        &CanonicalConversationRecord::visible_assistant_message(marker_text, metadata, None),
    )
    .await;
}

fn client_tool_descriptors_from_context_with_authority(
    client_context: Option<&Value>,
    authority: &den_core::client_tools::TurnAuthority,
) -> Value {
    let context = client_context.unwrap_or(&Value::Null);
    let mut descriptors = Vec::new();
    for tool in den_core::client_tools::ClientToolName::all() {
        if *tool == den_core::client_tools::ClientToolName::McpCallTool
            || !authority.allows_tool(*tool)
        {
            continue;
        }
        let descriptor = tool.descriptor();
        if !adapter_supports_tool(context, descriptor) {
            continue;
        }
        descriptors.push(den_core::client_tools::provider_tool_descriptor(*tool));
    }
    if let Some(mcp_tools) = context
        .pointer("/mcp/client_tools")
        .and_then(Value::as_array)
    {
        descriptors.extend(mcp_tools.iter().cloned());
    }
    if descriptors.is_empty() {
        descriptors.push(den_core::client_tools::provider_tool_descriptor(
            den_core::client_tools::ClientToolName::ReadTextFile,
        ));
    }
    json!(descriptors)
}

fn workspace_roots_from_client_context(client_context: Option<&Value>) -> Vec<String> {
    client_context
        .and_then(|context| context.get("workspace_roots"))
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default()
}

fn workspace_root_contains_cwd(root: &str, cwd: &str) -> bool {
    let root = Path::new(root);
    let cwd = Path::new(cwd);
    root.is_absolute()
        && cwd.is_absolute()
        && cwd
            .components()
            .collect::<Vec<_>>()
            .starts_with(&root.components().collect::<Vec<_>>())
}

pub(crate) fn normalized_workspace_roots(
    client_context: Option<&Value>,
    cwd: Option<&str>,
) -> Result<Vec<String>, CustomError> {
    let workspace_roots = workspace_roots_from_client_context(client_context);
    let Some(cwd) = cwd.map(str::trim).filter(|cwd| !cwd.is_empty()) else {
        return Ok(workspace_roots);
    };

    if workspace_roots.is_empty() {
        return Ok(vec![cwd.to_string()]);
    }

    if workspace_roots
        .iter()
        .any(|root| workspace_root_contains_cwd(root, cwd))
    {
        return Ok(workspace_roots);
    }

    Err(CustomError::ValidationError(format!(
        "cwd {cwd:?} is outside declared workspace_roots"
    )))
}

fn runtime_upstream_target(
    conversation_selection: &str,
    resolved_conversation_id: Option<&str>,
) -> String {
    resolved_conversation_id
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(conversation_selection)
        .to_string()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ResolvedRunModelSource {
    ConversationExplicit,
    ConversationAuto,
    ProfileDefault,
    BearDefault,
    SystemDefault,
}

impl ResolvedRunModelSource {
    const fn is_persisted_conversation_model(self) -> bool {
        matches!(self, Self::ConversationExplicit | Self::ConversationAuto)
    }

    const fn is_default(self) -> bool {
        matches!(
            self,
            Self::ProfileDefault | Self::BearDefault | Self::SystemDefault
        )
    }

    const fn as_str(self) -> &'static str {
        match self {
            Self::ConversationExplicit => "conversation_explicit",
            Self::ConversationAuto => "conversation_auto",
            Self::ProfileDefault => "profile_default",
            Self::BearDefault => "bear_default",
            Self::SystemDefault => "system_default",
        }
    }
}

impl fmt::Display for ResolvedRunModelSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

struct ResolvedRunModel {
    handle: String,
    provider_model_id: String,
    /// Set authoritatively by `preflight_pair_run_model` from the catalog
    /// snapshot; `resolve_pair_run_model` leaves it at a placeholder.
    api_style: den_llm::LlmApiStyle,
    supports_reasoning_effort: Option<bool>,
    source: ResolvedRunModelSource,
}

/// Placeholder used while resolving model identity; overwritten by preflight.
const RESOLVE_PLACEHOLDER_API_STYLE: den_llm::LlmApiStyle =
    den_llm::LlmApiStyle::ChatCompletionsStream;

fn provider_model_id_for_den_handle(handle: &str) -> String {
    den_llm::model_registry::provider_model_id_for_handle(handle)
        .unwrap_or_else(|| handle.trim())
        .to_string()
}

fn available_model_matches(
    model: &den_service::bifrost::BifrostModelMetadata,
    resolved: &ResolvedRunModel,
) -> bool {
    // Either of the catalog model's identifiers matching either resolved identifier is a hit.
    let model_ids = [model.handle.as_str(), model.model.as_str()];
    let resolved_ids = [
        resolved.handle.as_str(),
        resolved.provider_model_id.as_str(),
    ];
    model_ids
        .iter()
        .any(|model_id| resolved_ids.contains(model_id))
}

fn available_model_sample(models: &[den_service::bifrost::BifrostModelMetadata]) -> String {
    let mut handles = models
        .iter()
        .map(|model| model.handle.as_str())
        .take(20)
        .collect::<Vec<_>>();
    handles.sort_unstable();
    handles.join(", ")
}

fn pair_api_style_for_catalog_support(
    supports_responses_api: Option<bool>,
) -> den_llm::LlmApiStyle {
    match supports_responses_api {
        Some(false) => den_llm::LlmApiStyle::ChatCompletionsStream,
        Some(true) | None => den_llm::LlmApiStyle::ResponsesStream,
    }
}

fn ensure_pair_model_capabilities(
    entry: &BifrostCatalogEntry,
    model_handle: &str,
) -> Result<(), CustomError> {
    if entry.supports_tools == Some(false) {
        return Err(CustomError::ValidationError(format!(
            "selected model {model_handle} does not support tool calling, which is required for Pair runs"
        )));
    }
    Ok(())
}

fn unknown_capability_metadata(entry: &BifrostCatalogEntry) -> Vec<&'static str> {
    let mut missing = Vec::new();
    if entry.supports_tools.is_none() {
        missing.push("supports_tools");
    }
    if entry.supports_responses_api.is_none() {
        missing.push("supports_responses_api");
    }
    if entry.supports_vision.is_none() {
        missing.push("supports_vision");
    }
    missing
}

async fn resolve_pair_run_model(
    state: &DenState,
    bear: &den_service::bears::Bear,
    conversation_id: &str,
    stance: BearProfile,
) -> Result<ResolvedRunModel, CustomError> {
    if let Some(conversation) =
        den_service::conversation::persistence::get_conversation_for_external_id(
            &state.sqlx_pool,
            bear.id,
            conversation_id,
        )
        .await?
    {
        if let Some(model_state) =
            den_service::conversation::persistence::get_conversation_model_state(
                &state.sqlx_pool,
                conversation.id,
            )
            .await?
        {
            if model_state.selection_mode == "explicit" {
                if let Some(model) = model_state
                    .selected_model
                    .or(model_state.requested_model)
                    .map(|model| model.trim().to_string())
                    .filter(|model| !model.is_empty())
                {
                    let handle = den_llm::normalize_llm_model_handle(&model);
                    let provider_model_id = provider_model_id_for_den_handle(&handle);
                    return Ok(ResolvedRunModel {
                        api_style: RESOLVE_PLACEHOLDER_API_STYLE,
                        supports_reasoning_effort: None,
                        provider_model_id,
                        handle,
                        source: ResolvedRunModelSource::ConversationExplicit,
                    });
                }
            } else if let Some(model) = model_state
                .selected_model
                .map(|model| model.trim().to_string())
                .filter(|model| !model.is_empty())
            {
                let handle = den_llm::normalize_llm_model_handle(&model);
                let provider_model_id = provider_model_id_for_den_handle(&handle);
                return Ok(ResolvedRunModel {
                    api_style: RESOLVE_PLACEHOLDER_API_STYLE,
                    supports_reasoning_effort: None,
                    provider_model_id,
                    handle,
                    source: ResolvedRunModelSource::ConversationAuto,
                });
            }
        }
    }

    if let Some(model) = bears_db::profile_model_setting(&state.sqlx_pool, bear.id, stance).await? {
        let handle = den_llm::normalize_llm_model_handle(&model);
        let provider_model_id = provider_model_id_for_den_handle(&handle);
        return Ok(ResolvedRunModel {
            api_style: RESOLVE_PLACEHOLDER_API_STYLE,
            supports_reasoning_effort: None,
            provider_model_id,
            handle,
            source: ResolvedRunModelSource::ProfileDefault,
        });
    }

    if let Some(model) = bear
        .default_model
        .as_deref()
        .map(str::trim)
        .filter(|model| !model.is_empty())
    {
        let handle = den_llm::normalize_llm_model_handle(model);
        let provider_model_id = provider_model_id_for_den_handle(&handle);
        return Ok(ResolvedRunModel {
            api_style: RESOLVE_PLACEHOLDER_API_STYLE,
            supports_reasoning_effort: None,
            provider_model_id,
            handle,
            source: ResolvedRunModelSource::BearDefault,
        });
    }

    let handle = den_llm::normalize_llm_model_handle(&state.config.default_llm_model);
    let provider_model_id = provider_model_id_for_den_handle(&handle);
    Ok(ResolvedRunModel {
        api_style: RESOLVE_PLACEHOLDER_API_STYLE,
        supports_reasoning_effort: None,
        provider_model_id,
        handle,
        source: ResolvedRunModelSource::SystemDefault,
    })
}

async fn preflight_pair_run_model(
    state: &DenState,
    bear: &den_service::bears::Bear,
    session_id: &str,
    conversation_id: &str,
    stance: BearProfile,
) -> Result<ResolvedRunModel, CustomError> {
    let resolved = resolve_pair_run_model(state, bear, conversation_id, stance).await?;
    let snapshot = match state
        .bifrost
        .bear_catalog_snapshot(
            &state.sqlx_pool,
            bear.id,
            &state.config.den_secret_encryption_key,
        )
        .await
    {
        Ok(snapshot) => snapshot,
        Err(err) => {
            let cached_snapshot = state.bifrost.cached_bear_catalog_snapshot(bear.id);
            if resolved.source.is_persisted_conversation_model() {
                if let Some(snapshot) = cached_snapshot {
                    if let Some(entry) = snapshot.resolve(&resolved.handle) {
                        ensure_pair_model_capabilities(entry, &resolved.handle)?;
                        tracing::warn!(
                            error = %err,
                            session_id,
                            bear_id = %bear.id,
                            conversation_id,
                            model_handle = %resolved.handle,
                            model_selection_source = %resolved.source,
                            catalog_fetched_at = ?snapshot.fetched_at,
                            catalog_stale = snapshot.stale,
                            catalog_fallback = "stale_cached_snapshot",
                            "Bifrost catalog refresh failed; continuing Pair with the persisted conversation model"
                        );
                        return Ok(ResolvedRunModel {
                            api_style: pair_api_style_for_catalog_support(
                                entry.supports_responses_api,
                            ),
                            supports_reasoning_effort: entry.supports_reasoning_effort,
                            ..resolved
                        });
                    }
                }

                tracing::warn!(
                    error = %err,
                    session_id,
                    bear_id = %bear.id,
                    conversation_id,
                    model_handle = %resolved.handle,
                    model_selection_source = %resolved.source,
                    catalog_fallback = "persisted_conversation_model",
                    "Bifrost catalog refresh failed with no usable cached snapshot; continuing Pair with the persisted conversation model"
                );
                return Ok(ResolvedRunModel {
                    api_style: den_llm::LlmApiStyle::ResponsesStream,
                    supports_reasoning_effort: None,
                    ..resolved
                });
            }

            tracing::error!(
                error = %err,
                bear_id = %bear.id,
                model_handle = %resolved.handle,
                model_selection_source = %resolved.source,
                "Bear-scoped Bifrost catalog refresh before Pair preflight failed"
            );
            return Err(CustomError::System(format!(
                "Bifrost model catalog validation failed before run start: {err}"
            )));
        }
    };
    let available = snapshot.models_vec();
    let catalog_entry = snapshot.resolve(&resolved.handle).ok_or_else(|| {
        CustomError::ValidationError(format!(
            "selected model {} is not present in the Bifrost catalog; available models: {}",
            resolved.handle,
            available_model_sample(&available)
        ))
    })?;
    ensure_pair_model_capabilities(catalog_entry, &resolved.handle)?;
    let unknown_capabilities = unknown_capability_metadata(catalog_entry);
    if !unknown_capabilities.is_empty() {
        tracing::warn!(
            session_id,
            bear_id = %bear.id,
            conversation_id,
            model_handle = %resolved.handle,
            unknown_capabilities = %unknown_capabilities.join(", "),
            catalog_source = %snapshot.source,
            "Bifrost omitted optional model capability metadata; using runtime fallbacks"
        );
    }
    let resolved = ResolvedRunModel {
        api_style: pair_api_style_for_catalog_support(catalog_entry.supports_responses_api),
        supports_reasoning_effort: catalog_entry.supports_reasoning_effort,
        ..resolved
    };
    if available
        .iter()
        .any(|model| available_model_matches(model, &resolved))
    {
        tracing::info!(
            session_id,
            bear_id = %bear.id,
            conversation_id,
            model_handle = %resolved.handle,
            provider_model_id = %resolved.provider_model_id,
            model_selection_source = %resolved.source,
            api_style = %resolved.api_style.as_str(),
            catalog_stale = snapshot.stale,
            catalog_fetched_at = ?snapshot.fetched_at,
            "BearWire model preflight passed"
        );
        return Ok(resolved);
    }

    tracing::warn!(
        session_id,
        bear_id = %bear.id,
        conversation_id,
        model_handle = %resolved.handle,
        provider_model_id = %resolved.provider_model_id,
        model_selection_source = %resolved.source,
        api_style = %resolved.api_style.as_str(),
        available_models = %available_model_sample(&available),
        catalog_stale = snapshot.stale,
        catalog_fetched_at = ?snapshot.fetched_at,
        "BearWire model preflight did not find selected model in catalog snapshot; proceeding and letting Bifrost execution validate"
    );
    Ok(resolved)
}

fn adapter_supports_tool(
    client_context: &Value,
    descriptor: &den_core::client_tools::ClientToolDescriptor,
) -> bool {
    std::iter::once(descriptor.provider_name)
        .chain(descriptor.provider_aliases.iter().copied())
        .any(|name| {
            client_context
                .pointer(&format!("/adapter/direct_tools/{name}/supported"))
                .and_then(Value::as_bool)
                .or_else(|| {
                    client_context
                        .pointer(&format!("/direct_tools/{name}"))
                        .and_then(Value::as_bool)
                })
                .unwrap_or(false)
        })
}

async fn run_is_terminal(pool: &sqlx::PgPool, run_id: &str) -> bool {
    turn_runs::get_run(pool, run_id)
        .await
        .ok()
        .flatten()
        .is_some_and(|run| matches!(run.state.as_str(), "completed" | "failed" | "cancelled"))
}

fn run_progress_is_warning(detail: &Value) -> bool {
    detail.get("event_kind").and_then(Value::as_str) == Some("error")
}

fn is_replaceable_livestream_event(event: &BearWireEvent) -> bool {
    event.event_type == "run.progress"
}

pub(crate) async fn persist_run_progress(
    _pool: &sqlx::PgPool,
    session_id: &str,
    run_id: &str,
    bear_id: uuid::Uuid,
    user_id: i32,
    started_at: Instant,
    kind: &str,
    text: &str,
    detail: Value,
) {
    if run_progress_is_warning(&detail) {
        tracing::warn!(
            session_id = %session_id,
            run_id = %run_id,
            kind,
            elapsed_ms = started_at.elapsed().as_millis(),
            detail = %detail,
            "BearWire run progress"
        );
    } else {
        tracing::debug!(
            session_id = %session_id,
            run_id = %run_id,
            kind,
            elapsed_ms = started_at.elapsed().as_millis(),
            detail = %detail,
            "BearWire run progress"
        );
    }

    // ADR-0030 classifies run.progress as replaceable live status, not semantic
    // history. Keep it in operational telemetry until its caller sends the safe
    // summary to the bounded non-replay livestream channel.
    let _ = (bear_id, user_id, text);
}

/// Publish the safe, replaceable counterpart of `run.progress` to connected
/// livestream consumers. Progress details are intentionally not forwarded:
/// several producers attach tool arguments for server-side diagnostics.
pub(crate) fn publish_run_progress(
    state: &DenState,
    session_id: &str,
    run_id: &str,
    kind: &str,
    text: &str,
) {
    state.publish_bearwire_livestream(
        session_id,
        json!({
            "type": "run.progress",
            "scope": "ephemeral",
            "run_id": run_id,
            "kind": kind,
            "text": text,
        }),
    );
}

async fn incomplete_stream_pending_tools(pool: &sqlx::PgPool, run_id: &str) -> Vec<Value> {
    // Diagnostics deliberately retain only IDs and lifecycle state, never tool arguments.
    sqlx::query(
        r"
        SELECT tool_call_id, state
        FROM turn_obligations
        WHERE run_id = $1
          AND tool_call_id IS NOT NULL
          AND state IN ('requested', 'waiting_for_client')
        ORDER BY created_at ASC, id ASC
        ",
    )
    .bind(run_id)
    .fetch_all(pool)
    .await
    .map(|rows| {
        rows.into_iter()
            .map(|row| {
                json!({
                    "id": row.get::<String, _>("tool_call_id"),
                    "state": row.get::<String, _>("state"),
                })
            })
            .collect()
    })
    .unwrap_or_default()
}

fn runtime_event_has_assistant_content(event: &den_protocol::RuntimeStreamEvent) -> bool {
    matches!(
        event,
        den_protocol::RuntimeStreamEvent::Semantic(
            den_protocol::RuntimeSemanticEvent::AssistantTextDelta { .. }
        )
    )
}

fn initial_stream_eof_is_recoverable(
    terminal_or_wait_seen: bool,
    cancellation_seen: bool,
    docket_continuation_requested: bool,
) -> bool {
    // EOF is transport loss, not a run outcome. A terminal event, answerable client
    // wait, cancellation, or Docket-authorized successor already supplies the next
    // durable control boundary.
    !terminal_or_wait_seen && !cancellation_seen && !docket_continuation_requested
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RuntimeStreamBoundary {
    Continue,
    BoundedSlice,
    ClientWait,
    Terminal,
}

pub(crate) fn runtime_stream_boundary(
    event: &den_protocol::RuntimeStreamEvent,
) -> RuntimeStreamBoundary {
    use den_protocol::{RuntimeSemanticEvent, RuntimeStreamEvent};
    match event {
        RuntimeStreamEvent::Semantic(
            RuntimeSemanticEvent::TurnCompleted { .. }
            | RuntimeSemanticEvent::TurnFailed { .. }
            | RuntimeSemanticEvent::TurnCancelled { .. }
            | RuntimeSemanticEvent::Error { .. },
        ) => RuntimeStreamBoundary::Terminal,
        RuntimeStreamEvent::Semantic(RuntimeSemanticEvent::RunPaused { .. }) => {
            RuntimeStreamBoundary::ClientWait
        }
        RuntimeStreamEvent::Semantic(RuntimeSemanticEvent::BoundedSlice { .. }) => {
            RuntimeStreamBoundary::BoundedSlice
        }
        RuntimeStreamEvent::Semantic(RuntimeSemanticEvent::ToolCallRequested {
            tool_name,
            approval_required,
            approval_request_id,
            ..
        }) => {
            let approval_wait = *approval_required
                && approval_request_id
                    .as_deref()
                    .is_some_and(|id| !id.trim().is_empty());
            let den_owned = den_owned_tool_call(tool_name);
            if approval_wait || !den_owned {
                RuntimeStreamBoundary::ClientWait
            } else {
                RuntimeStreamBoundary::Continue
            }
        }
        _ => RuntimeStreamBoundary::Continue,
    }
}

fn runtime_event_is_terminal_or_wait(event: &den_protocol::RuntimeStreamEvent) -> bool {
    runtime_stream_boundary(event) != RuntimeStreamBoundary::Continue
}

pub(crate) fn runtime_event_kind(event: &den_protocol::RuntimeStreamEvent) -> &'static str {
    use den_protocol::{RuntimeSemanticEvent, RuntimeStreamEvent};
    match event {
        RuntimeStreamEvent::Semantic(RuntimeSemanticEvent::AssistantTextDelta { .. }) => {
            "assistant_text_delta"
        }
        RuntimeStreamEvent::Semantic(RuntimeSemanticEvent::ReasoningTextDelta { .. }) => {
            "reasoning_text_delta"
        }
        RuntimeStreamEvent::Semantic(RuntimeSemanticEvent::StatusText { .. }) => "status_text",
        RuntimeStreamEvent::Semantic(RuntimeSemanticEvent::RunProgress { .. }) => "run_progress",
        RuntimeStreamEvent::Semantic(RuntimeSemanticEvent::RunPaused { .. }) => "run_paused",
        RuntimeStreamEvent::Semantic(RuntimeSemanticEvent::BoundedSlice { .. }) => "bounded_slice",
        RuntimeStreamEvent::Semantic(RuntimeSemanticEvent::ToolCallRequested { .. }) => {
            "tool_call_requested"
        }
        RuntimeStreamEvent::Semantic(RuntimeSemanticEvent::ToolCallFinished { .. }) => {
            "tool_call_finished"
        }
        RuntimeStreamEvent::Semantic(RuntimeSemanticEvent::ConversationResolved { .. }) => {
            "conversation_resolved"
        }
        RuntimeStreamEvent::Semantic(RuntimeSemanticEvent::TurnCompleted { .. }) => {
            "turn_completed"
        }
        RuntimeStreamEvent::Semantic(RuntimeSemanticEvent::TurnFailed { .. }) => "turn_failed",
        RuntimeStreamEvent::Semantic(RuntimeSemanticEvent::TurnCancelled { .. }) => {
            "turn_cancelled"
        }
        RuntimeStreamEvent::Semantic(RuntimeSemanticEvent::Error { .. }) => "error",
        RuntimeStreamEvent::ProviderActivity => "provider_activity",
        RuntimeStreamEvent::UntranslatedProviderEvent { .. } => "untranslated_provider_event",
    }
}

fn canonical_client_waiting_ids(event: &BearWireEvent) -> Option<(String, String)> {
    let event = serde_json::from_value::<CanonicalClientWaitingEvent>(event.data.clone()).ok()?;
    let permission_id = event.permission.id.trim();
    let tool_call_id = event.tool_call.id.trim();
    (!permission_id.is_empty() && !tool_call_id.is_empty())
        .then(|| (permission_id.to_string(), tool_call_id.to_string()))
}

/// Make the live permission prompt from the canonical event without forwarding
/// raw tool arguments. The durable obligation remains the reconnect authority.
fn client_waiting_livestream_projection(
    event: &BearWireEvent,
    obligation: &turn_obligations::TurnObligationRow,
    expected_client_method: &str,
) -> Option<Value> {
    let (permission_id, tool_call_id) = canonical_client_waiting_ids(event)?;
    let data = event.data.as_object()?;
    let tool_call = data.get("tool_call")?.as_object()?;
    let permission = data.get("permission")?.as_object()?;
    let optional = |object: &serde_json::Map<String, Value>, key| {
        object.get(key).filter(|value| !value.is_null()).cloned()
    };

    Some(json!({
        "type": "client.waiting",
        "scope": "ephemeral",
        "run_id": obligation.run_id,
        "obligation_id": obligation.id.to_string(),
        "expected_responder_action": obligation.expected_responder_action,
        "expected_client_method": expected_client_method,
        "turn_step_id": obligation.turn_step_id.map(|id| id.to_string()),
        "tool_call": {
            "id": tool_call_id,
            "name": optional(tool_call, "name"),
            "title": optional(tool_call, "title"),
            "kind": optional(tool_call, "kind"),
            "display": optional(tool_call, "display"),
        },
        "permission": {
            "id": permission_id,
            "reason": optional(permission, "reason"),
            "title": optional(permission, "title"),
            "target": optional(permission, "target"),
        },
        "approval_required": data.get("approval_required").and_then(Value::as_bool).unwrap_or(true),
        "execution_target": optional(data, "execution_target"),
        "policy": optional(data, "policy"),
    }))
}

fn publish_answerable_client_waiting_event(
    state: &DenState,
    session_id: &str,
    run_id: &str,
    event: BearWireEvent,
    obligation: &turn_obligations::TurnObligationRow,
) {
    let Some(expected_client_method) =
        bearwire_client_method_for_action(&obligation.expected_responder_action)
    else {
        tracing::warn!(
            session_id = %session_id,
            run_id = %run_id,
            obligation_id = %obligation.id,
            expected_responder_action = %obligation.expected_responder_action,
            "refusing to publish client.waiting for obligation without BearWire client method"
        );
        return;
    };
    if obligation.expected_responder_action != "permission_decision" {
        tracing::warn!(
            session_id = %session_id,
            run_id = %run_id,
            obligation_id = %obligation.id,
            expected_responder_action = %obligation.expected_responder_action,
            "refusing to publish client.waiting for non-permission obligation"
        );
        return;
    }
    let Some(permission_id) = obligation
        .permission_id
        .as_deref()
        .map(str::trim)
        .filter(|id| !id.is_empty())
    else {
        return;
    };
    let Some(tool_call_id) = obligation
        .tool_call_id
        .as_deref()
        .map(str::trim)
        .filter(|id| !id.is_empty())
    else {
        return;
    };
    let Some((event_permission_id, event_tool_call_id)) = canonical_client_waiting_ids(&event)
    else {
        return;
    };
    if event_permission_id != permission_id || event_tool_call_id != tool_call_id {
        tracing::warn!(session_id = %session_id, run_id = %run_id, obligation_id = %obligation.id, "refusing to publish client.waiting with mismatched canonical ids");
        return;
    }

    let Some(projection) =
        client_waiting_livestream_projection(&event, obligation, expected_client_method)
    else {
        return;
    };
    // The obligation is durable. This notification only updates an active livestream.
    state.publish_bearwire_livestream(session_id, projection);
}

#[allow(clippy::too_many_arguments)]
async fn persist_tool_call_requested_transactionally(
    state: &DenState,
    session_id: &str,
    run_id: &str,
    bear_id: uuid::Uuid,
    user_id: i32,
    request_id: Uuid,
    started_at: Option<Instant>,
    tool_call_id: &str,
    tool_name: &str,
    title: &Option<String>,
    kind: &Option<String>,
    arguments: &Value,
    approval_request_id: &Option<String>,
    approval_required: bool,
    approval_reason: &Option<String>,
    event_run_id: &Option<String>,
) -> Result<(), den_core::DenError> {
    let pool = &state.sqlx_pool;
    let persisted = den_runtime::turn_waits::persist_bearwire_tool_call_wait_transactionally(
        pool,
        den_runtime::turn_waits::PersistToolCallWaitInput {
            process_epoch_id: state.process_epoch_id,
            session_id,
            run_id,
            bear_id,
            user_id,
            request_id,
            tool_call_id,
            tool_name,
            title: title.as_deref(),
            kind: kind.as_deref(),
            arguments,
            approval_request_id: approval_request_id.as_deref(),
            approval_required,
            approval_reason: approval_reason.as_deref(),
            event_run_id: event_run_id.as_deref(),
        },
    )
    .await?;

    if let (Some(started_at), Some(obligation)) = (started_at, persisted.obligation.as_ref()) {
        let (kind, text) = if persisted.effective_approval_required {
            (
                "tool_waiting_for_permission",
                "Waiting for client permission to run a local tool…",
            )
        } else {
            (
                "tool_waiting_for_result",
                "Waiting for local tool result from the armature…",
            )
        };
        persist_run_progress(
            pool,
            session_id,
            run_id,
            bear_id,
            user_id,
            started_at,
            kind,
            text,
            json!({
                "tool_call_id": tool_call_id,
                "tool_name": tool_name,
                "arguments": arguments,
                "approval_required": persisted.effective_approval_required,
                "approval_request_id": approval_request_id,
                "request_id": request_id,
                "turn_step_id": persisted.turn_step_id,
                "obligation_id": obligation.id,
                "event_sequence": persisted.event_sequence,
            }),
        )
        .await;
    }
    Ok(())
}

pub(crate) async fn persist_runtime_event_as_bearwire(
    state: &DenState,
    pool: &sqlx::PgPool,
    session_id: &str,
    run_id: &str,
    bear_id: uuid::Uuid,
    user_id: i32,
    runtime_event: den_protocol::RuntimeStreamEvent,
    request_id: Uuid,
    started_at: Option<Instant>,
) {
    if run_is_terminal(pool, run_id).await {
        tracing::debug!(
            session_id = %session_id,
            run_id = %run_id,
            event_kind = runtime_event_kind(&runtime_event),
            "skipping BearWire runtime event for terminal run"
        );
        return;
    }
    let bear_name = bear_display_name(pool, bear_id).await;
    let history_marker = runtime_event_history_marker(&bear_name, &runtime_event);
    if let den_protocol::RuntimeStreamEvent::Semantic(
        den_protocol::RuntimeSemanticEvent::ToolCallRequested {
            tool_call_id,
            tool_name,
            title,
            kind,
            arguments,
            approval_request_id,
            approval_required,
            approval_reason,
            run_id: event_run_id,
        },
    ) = &runtime_event
    {
        match persist_tool_call_requested_transactionally(
            state,
            session_id,
            run_id,
            bear_id,
            user_id,
            request_id,
            started_at,
            tool_call_id,
            tool_name,
            title,
            kind,
            arguments,
            approval_request_id,
            *approval_required,
            approval_reason,
            event_run_id,
        )
        .await
        {
            Ok(()) => return,
            Err(den_core::DenError::RunStateConflict {
                operation: "persist_client_wait",
                ..
            }) => {
                tracing::debug!(
                    session_id = %session_id,
                    run_id = %run_id,
                    tool_call_id = %tool_call_id,
                    "ignored late tool wait for terminal or missing run"
                );
                return;
            }
            Err(err) if den_runtime::turn_waits::descriptor_resolution_failed(&err) => {
                tracing::warn!(
                    error = %err,
                    session_id = %session_id,
                    run_id = %run_id,
                    tool_call_id = %tool_call_id,
                    tool_name = %tool_name,
                    "BearWire tool-call descriptor resolution failed; failing run instead of creating an ambiguous client wait"
                );
                persist_run_failed(
                    pool,
                    session_id,
                    run_id,
                    bear_id,
                    user_id,
                    RunFailureReason::DescriptorResolutionFailed,
                    err.to_string(),
                    Some(json!({
                        "tool_call_id": tool_call_id,
                        "tool_name": tool_name,
                        "request_id": request_id,
                    })),
                )
                .await;
                return;
            }
            Err(err) => {
                tracing::warn!(
                    error = %err,
                    session_id = %session_id,
                    run_id = %run_id,
                    tool_call_id = %tool_call_id,
                    "transactional BearWire tool-call persistence failed; falling back to legacy persistence path"
                );
            }
        }
    }

    let terminal_event = runtime_event_is_terminal(&runtime_event);
    let active_obligation = update_run_state_for_runtime_event(
        state,
        session_id,
        run_id,
        bear_id,
        user_id,
        &runtime_event,
        request_id,
        started_at,
    )
    .await;
    if let Some(marker) = history_marker {
        persist_visible_runtime_marker(
            pool,
            session_id,
            run_id,
            bear_id,
            user_id,
            &marker.kind,
            marker.text,
            marker.metadata,
        )
        .await;
    }
    if terminal_event {
        return;
    }
    for mut event in runtime_stream_event_to_bearwire_events(runtime_event) {
        if is_replaceable_livestream_event(&event) {
            // ADR-0030: progress is a replaceable live observation, not durable
            // semantic history. Do not let a newly added runtime producer turn it
            // back into an append-only event row.
            let kind = event
                .data
                .get("kind")
                .and_then(Value::as_str)
                .unwrap_or("status_text");
            let text = event
                .data
                .get("text")
                .and_then(Value::as_str)
                .unwrap_or_default();
            publish_run_progress(state, session_id, run_id, kind, text);
            continue;
        }
        if event.event_type == "client.waiting" {
            if let Some(obligation) = active_obligation.as_ref() {
                publish_answerable_client_waiting_event(
                    state, session_id, run_id, event, obligation,
                );
            } else {
                tracing::warn!(
                    session_id = %session_id,
                    run_id = %run_id,
                    "runtime emitted client.waiting event without persisted BearWire obligation; dropping unanswerable event"
                );
            }
            continue;
        }
        if event.event_type == "permission.requested" && active_obligation.is_none() {
            tracing::warn!(
                session_id = %session_id,
                run_id = %run_id,
                "runtime emitted permission.requested event without persisted BearWire obligation; dropping unanswerable event"
            );
            continue;
        }
        event.bear_id = Some(bear_id.to_string());
        event.human_id = Some(user_id.to_string());
        event.session_id = Some(session_id.to_string());
        if event.run_id.is_none() {
            event.run_id = Some(run_id.to_string());
        }
        if let Err(err) = bearwire_events::append_bearwire_event(
            pool,
            session_id,
            Some(bear_id),
            Some(user_id),
            event,
        )
        .await
        {
            tracing::warn!(error = %err, session_id = %session_id, "failed to persist BearWire runtime event");
        }
    }
}

pub(crate) async fn fail_run_lifecycle(
    pool: &sqlx::PgPool,
    session_id: &str,
    run_id: &str,
    bear_id: uuid::Uuid,
    user_id: i32,
    reason: RunFailureReason,
    message: String,
    context: Option<serde_json::Value>,
) {
    let bear_name = bear_display_name(pool, bear_id).await;
    let projection = run_failure_projection(reason.as_str(), &message, run_id, &bear_name, context);
    let user_message = projection.user_message.as_deref();
    let context = projection.diagnostic_context.clone();
    let diagnostic_context = context
        .as_ref()
        .map(|context| log_sample(context.to_string()));
    tracing::warn!(
        session_id,
        run_id,
        bear_id = %bear_id,
        user_id,
        reason = %reason,
        user_message = user_message,
        error_message = %log_sample(&message),
        diagnostic_context = diagnostic_context.as_deref(),
        "BearWire run failed"
    );
    let diagnostic_details = context.clone().unwrap_or_else(|| json!({}));
    if let Err(error) = runtime_exception_events::record(
        pool,
        NewRuntimeExceptionEvent {
            severity: RuntimeExceptionSeverity::Error,
            component: "den_bearwire.run".to_string(),
            event_code: format!("bearwire_run_failed_{}", reason.as_str()),
            message: message.clone(),
            details: diagnostic_details,
            context: RuntimeExceptionContext {
                session_id: Some(session_id.to_string()),
                runtime_run_id: Some(run_id.to_string()),
                bear_id: Some(bear_id),
                ..RuntimeExceptionContext::default()
            },
        },
    )
    .await
    {
        tracing::warn!(session_id, run_id, error = %error, "failed to persist runtime exception event");
    }
    let finished = turn_runs::fail_run(
        pool,
        session_id,
        run_id,
        bear_id,
        user_id,
        reason.as_str(),
        json!({
            "run_id": run_id,
            "message": message,
            "user_message": user_message,
            "reason": reason.as_str(),
            "context": context,
        }),
    )
    .await
    .unwrap_or_else(|err| {
        tracing::error!(session_id, run_id, reason = %reason, error = %err, "failed to atomically persist BearWire run failure");
        None
    });
    let Some(finished) = finished else {
        return;
    };
    den_runtime::native_runtime::remove_native_client_run(session_id, run_id);
    tracing::info!(
        session_id,
        run_id,
        reason = %reason,
        outstanding_client_tools_settled = finished.settled_obligations,
        settled_steps = finished.settled_steps,
        event_sequence = finished.event_sequence,
        "BearWire run reached failed terminal state"
    );
    record_work_run_outcome_if_bound(
        pool,
        session_id,
        run_id,
        "failed",
        Some(json!({
            "category": reason.as_str(),
            "message": message.clone(),
            "forensics": context,
        })),
    )
    .await;
    if let Ok(Some(session)) =
        client_sessions::find_for_user_bear_session_id(pool, user_id, bear_id, session_id).await
    {
        let conversation_id = session
            .resolved_conversation_id
            .clone()
            .unwrap_or_else(|| session.conversation_id.clone());
        let provenance = ConversationEventProvenance::client_session(session_id.to_string());
        let content_json = projection.content.clone();
        let marker_kind = content_json.get("kind").cloned().unwrap_or(Value::Null);
        let marker_retryable = content_json
            .get("retryable")
            .cloned()
            .unwrap_or(Value::Null);
        let _ = persist_canonical_conversation_record(
            &canonical_persistence_context(
                pool.clone(),
                bear_id,
                Some(user_id),
                conversation_id,
                Some(session_id.to_string()),
                Some(run_id.to_string()),
                provenance.scope_id,
                false,
            ),
            &CanonicalConversationRecord::model_visible_hidden_assistant_message(
                projection.model_summary.clone(),
                content_json,
                None,
            ),
        )
        .await;
        if let Some(marker) = projection.history_marker.clone() {
            persist_visible_runtime_marker(
                pool,
                session_id,
                run_id,
                bear_id,
                user_id,
                "operational_outcome",
                marker,
                json!({
                    "reason": reason.as_str(),
                    "kind": marker_kind,
                    "retryable": marker_retryable,
                }),
            )
            .await;
        }
    }
}

pub(crate) async fn persist_run_failed(
    pool: &sqlx::PgPool,
    session_id: &str,
    run_id: &str,
    bear_id: uuid::Uuid,
    user_id: i32,
    reason: RunFailureReason,
    message: String,
    context: Option<serde_json::Value>,
) {
    fail_run_lifecycle(
        pool, session_id, run_id, bear_id, user_id, reason, message, context,
    )
    .await;
}

#[derive(Debug, Clone)]
pub(crate) struct SettledRunLifecycle {
    run: Option<turn_runs::TurnRunRow>,

    cancelled_stream: bool,
    cancelled_tool_turn: bool,
    settled_obligations: u64,
    event_sequence: Option<i64>,
}

impl SettledRunLifecycle {
    fn empty() -> Self {
        Self {
            run: None,

            cancelled_stream: false,
            cancelled_tool_turn: false,
            settled_obligations: 0,
            event_sequence: None,
        }
    }

    fn settled(&self) -> bool {
        self.run.is_some() || self.cancelled_stream || self.cancelled_tool_turn
    }
}

pub(crate) async fn settle_active_run_for_session(
    state: &DenState,
    session_id: &str,
    bear_id: uuid::Uuid,
    user_id: i32,
    reason: &str,
    expected_run_id: Option<&str>,
    superseded_by_run_id: Option<&str>,
) -> Result<SettledRunLifecycle, CustomError> {
    let active_run = turn_runs::active_run_for_session(&state.sqlx_pool, session_id).await?;
    if expected_run_id.is_some()
        && active_run.as_ref().map(|run| run.run_id.as_str()) != expected_run_id
    {
        return Ok(SettledRunLifecycle::empty());
    }

    // The attached BearWire delivery must observe a terminal event before its
    // cancellation handle closes the stream. Without this projection, a successor
    // can leave the predecessor client waiting for a terminal outcome that only
    // exists in persisted event history.
    if let Some(run) = &active_run {
        state.publish_bearwire_livestream(
            session_id,
            json!({
                "type": "run.cancelled",
                "scope": "ephemeral",
                "run_id": run.run_id,
                "reason": reason,
                "superseded_by_run_id": superseded_by_run_id,
            }),
        );
    }
    let stream_cancel = active_run
        .as_ref()
        .and_then(|run| state.turn_cancellations.cancel_run(session_id, &run.run_id));
    let active_turn = state.tool_turns.cancel_active_turn(session_id);

    let cancelled_stream = stream_cancel.is_some();
    let cancelled_tool_turn = active_turn.is_some();
    let mut settled_obligations = 0;
    let mut event_sequence = None;
    if let Some(run) = &active_run {
        if let Some(finished) = turn_runs::cancel_run(
            &state.sqlx_pool,
            session_id,
            &run.run_id,
            bear_id,
            user_id,
            reason,
            json!({
                "session_id": session_id,
                "cancelled": true,
                "run_id": run.run_id,
                "reason": reason,
                "superseded_by_run_id": superseded_by_run_id,
                "cancelled_stream": cancelled_stream,
                "cancelled_tool_turn": cancelled_tool_turn,
            }),
        )
        .await?
        {
            den_runtime::native_runtime::remove_native_client_run(session_id, &run.run_id);
            settled_obligations = finished.settled_obligations;
            event_sequence = Some(finished.event_sequence);
            record_work_run_outcome_if_bound(
                &state.sqlx_pool,
                session_id,
                &run.run_id,
                "cancelled",
                None,
            )
            .await;
        }
    }
    // The Docket attempt is the execution authority; the host run above is only
    // terminal evidence. Release the stable session binding even when its host
    // controller/run was already missing so cancellation cannot leave focus wedged.
    if let Some(attempt) = PgDocketService::from_pool(&state.sqlx_pool)
        .get_live_focused_execution(
            bear_id,
            DocketFocusedExecutionBinding {
                kind: DocketExecutionBindingKind::ClientSession,
                id: session_id.to_string(),
            },
        )
        .await?
    {
        PgDocketService::from_pool(&state.sqlx_pool)
            .release_execution_attempt(den_docket::DocketExecutionAttemptRelease {
                attempt_id: attempt.id,
                fence_epoch: attempt.fence_epoch,
                recovery_key: Uuid::new_v4(),
                recovery_reason: reason.to_string(),
            })
            .await?;
        let mut transition = BearWireEvent::ephemeral(
            "docket.execution.ended",
            json!({
                "attempt_id": attempt.id,
                "task_id": attempt.task_id,
                "run_id": attempt.host.run_id,
                "binding": { "kind": "client_session", "id": session_id },
                "host": { "kind": attempt.host.kind.as_str(), "run_id": attempt.host.run_id },
                "fence_epoch": attempt.fence_epoch,
                "reason": reason,
                "superseded_by_run_id": superseded_by_run_id,
                "task_selection_preserved": true,
                "execution_authority": "absent",
            }),
        );
        transition.bear_id = Some(bear_id.to_string());
        transition.human_id = Some(user_id.to_string());
        transition.session_id = Some(session_id.to_string());
        transition.run_id = Some(attempt.host.run_id.clone());
        if let Err(err) = bearwire_events::append_bearwire_event(
            &state.sqlx_pool,
            session_id,
            Some(bear_id),
            Some(user_id),
            transition,
        )
        .await
        {
            // The released Docket attempt is authoritative. A diagnostic projection
            // failure must not make successful cancellation look unsuccessful.
            tracing::warn!(
                error = %err,
                session_id,
                attempt_id = %attempt.id,
                "failed to project Docket execution end"
            );
        }
    }
    Ok(SettledRunLifecycle {
        run: active_run,
        cancelled_stream,
        cancelled_tool_turn,
        settled_obligations,
        event_sequence,
    })
}

/// Work-run hook: when this session was bound by `work.checkout`, record the
/// terminal turn outcome and move the work run to `reporting` so the dispatch
/// worker harvests it. A no-op (one indexed lookup) for ordinary Pair sessions.
async fn record_work_run_outcome_if_bound(
    pool: &sqlx::PgPool,
    session_id: &str,
    run_id: &str,
    kind: &str,
    detail: Option<Value>,
) {
    let outcome = json!({
        "kind": kind,
        "run_id": run_id,
        "detail": detail,
    });
    match den_docket::work_runs::record_work_run_turn_outcome(pool, session_id, &outcome).await {
        Ok(Some(work_run)) => {
            tracing::info!(
                session_id,
                run_id,
                work_run_id = %work_run.id,
                outcome_kind = kind,
                "work run moved to reporting after terminal turn event"
            );
        }
        Ok(None) => {}
        Err(err) => {
            tracing::warn!(
                session_id,
                run_id,
                error = %err,
                "failed to record work run turn outcome"
            );
        }
    }
}

fn runtime_event_is_terminal(event: &den_protocol::RuntimeStreamEvent) -> bool {
    use den_protocol::{RuntimeSemanticEvent, RuntimeStreamEvent};
    matches!(
        event,
        RuntimeStreamEvent::Semantic(
            RuntimeSemanticEvent::TurnCompleted { .. }
                | RuntimeSemanticEvent::TurnFailed { .. }
                | RuntimeSemanticEvent::TurnCancelled { .. }
                | RuntimeSemanticEvent::Error { .. }
        )
    )
}

pub(crate) fn docket_bounded_slice_continuation(
    decision: Option<DocketPairContinuationDecision>,
) -> Option<RuntimeContinuation> {
    (decision == Some(DocketPairContinuationDecision::Continue))
        .then_some(RuntimeContinuation::DocketBoundedSlice)
}

pub(crate) async fn report_pair_bounded_outcome(
    state: &DenState,
    user_id: i32,
    bear_id: uuid::Uuid,
    session_id: &str,
    run_id: &str,
    outcome: DocketPairBoundedOutcome,
) -> Option<DocketPairContinuationDecision> {
    let snapshot = super::focused_execution::load_focused_execution_snapshot(
        state,
        user_id,
        bear_id,
        session_id,
        super::focused_execution::FocusedExecutionLaunchState::AlreadyRunning,
    )
    .await
    .ok()?;
    if snapshot.run_id().map(TurnRunId::as_str) != Some(run_id) {
        return None;
    }
    let attempt = snapshot.attempt.as_ref()?;
    if attempt.state != den_docket::DocketExecutionAttemptState::Running {
        return None;
    }
    let (attempt_id, fence_epoch) = (attempt.id, attempt.fence_epoch);
    match PgDocketService::from_pool(&state.sqlx_pool)
        .report_pair_bounded_outcome(DocketPairBoundedOutcomeReport {
            attempt_id,
            fence_epoch,
            outcome,
            awaiting_user_question: None,
        })
        .await
    {
        Ok(decision) => Some(decision.decision),
        Err(error) => {
            tracing::warn!(%error, %attempt_id, ?outcome, "failed to report Pair bounded outcome to Docket");
            None
        }
    }
}

pub(crate) async fn finish_runtime_terminal_event(
    state: &DenState,
    session_id: &str,
    run_id: &str,
    bear_id: uuid::Uuid,
    user_id: i32,
    event: &den_protocol::RuntimeStreamEvent,
) {
    use den_protocol::{RuntimeSemanticEvent, RuntimeStreamEvent};
    let (terminal_state, reason, outcome, detail) = match event {
        RuntimeStreamEvent::Semantic(RuntimeSemanticEvent::TurnCompleted { .. }) => (
            turn_runs::TurnRunState::Completed,
            "completed".to_string(),
            "completed",
            None,
        ),
        RuntimeStreamEvent::Semantic(RuntimeSemanticEvent::TurnFailed {
            category,
            message,
            ..
        }) => {
            let category = category.as_str();
            (
                turn_runs::TurnRunState::Failed,
                category.to_string(),
                "failed",
                Some(json!({ "category": category, "message": message })),
            )
        }
        RuntimeStreamEvent::Semantic(RuntimeSemanticEvent::TurnCancelled { .. }) => (
            turn_runs::TurnRunState::Cancelled,
            "cancelled".to_string(),
            "cancelled",
            None,
        ),
        RuntimeStreamEvent::Semantic(RuntimeSemanticEvent::Error {
            message,
            error_type,
            ..
        }) => (
            turn_runs::TurnRunState::Failed,
            error_type.clone().unwrap_or_else(|| "error".to_string()),
            "failed",
            Some(json!({ "category": error_type, "message": message })),
        ),
        _ => return,
    };
    let Some(terminal_event) = runtime_stream_event_to_bearwire_events(event.clone())
        .into_iter()
        .find(|event| {
            matches!(
                event.event_type.as_str(),
                "run.completed" | "run.failed" | "run.cancelled"
            )
        })
    else {
        tracing::error!(
            session_id,
            run_id,
            "terminal runtime event had no BearWire terminal projection"
        );
        return;
    };

    if report_pair_bounded_outcome(
        state,
        user_id,
        bear_id,
        session_id,
        run_id,
        DocketPairBoundedOutcome::Settled,
    )
    .await
    .is_some_and(|decision| decision != DocketPairContinuationDecision::Stop)
    {
        tracing::warn!(
            session_id,
            run_id,
            "unexpected Docket decision for terminal Pair outcome"
        );
    }
    let finish = match terminal_state {
        turn_runs::TurnRunState::Completed => {
            turn_runs::complete_run(
                &state.sqlx_pool,
                session_id,
                run_id,
                bear_id,
                user_id,
                Some(&reason),
                terminal_event.data,
            )
            .await
        }
        turn_runs::TurnRunState::Failed => {
            turn_runs::fail_run(
                &state.sqlx_pool,
                session_id,
                run_id,
                bear_id,
                user_id,
                &reason,
                terminal_event.data,
            )
            .await
        }
        turn_runs::TurnRunState::Cancelled => {
            turn_runs::cancel_run(
                &state.sqlx_pool,
                session_id,
                run_id,
                bear_id,
                user_id,
                &reason,
                terminal_event.data,
            )
            .await
        }
        turn_runs::TurnRunState::Accepted
        | turn_runs::TurnRunState::Running
        | turn_runs::TurnRunState::WaitingForClient
        | turn_runs::TurnRunState::Continuing => {
            unreachable!("runtime terminal event mapped to nonterminal state")
        }
    };
    match finish {
        Ok(Some(_)) => {
            den_runtime::native_runtime::remove_native_client_run(session_id, run_id);
            record_work_run_outcome_if_bound(&state.sqlx_pool, session_id, run_id, outcome, detail)
                .await;
        }
        Ok(None) => {}
        Err(err) => {
            tracing::error!(session_id, run_id, error = %err, "failed to atomically persist terminal runtime event");
        }
    }
}

async fn update_run_state_for_runtime_event(
    state: &DenState,
    session_id: &str,
    run_id: &str,
    bear_id: uuid::Uuid,
    user_id: i32,
    event: &den_protocol::RuntimeStreamEvent,
    request_id: Uuid,
    started_at: Option<Instant>,
) -> Option<turn_obligations::TurnObligationRow> {
    use den_protocol::{RuntimeSemanticEvent, RuntimeStreamEvent};
    let pool = &state.sqlx_pool;
    if runtime_event_is_terminal(event) {
        finish_runtime_terminal_event(state, session_id, run_id, bear_id, user_id, event).await;
        return None;
    }
    match event {
        RuntimeStreamEvent::Semantic(RuntimeSemanticEvent::ToolCallRequested {
            tool_call_id,
            tool_name,
            arguments,
            approval_request_id,
            approval_required,
            ..
        }) => {
            let has_permission_id = approval_request_id
                .as_deref()
                .map(str::trim)
                .is_some_and(|id| !id.is_empty());
            if *approval_required && !has_permission_id {
                tracing::warn!(
                    session_id,
                    run_id,
                    tool_call_id,
                    tool_name,
                    "runtime emitted approval_required tool call without approval_request_id; treating as tool result obligation"
                );
            }
            let effective_approval_required = *approval_required && has_permission_id;
            if den_owned_tool_call(tool_name) {
                return None;
            }
            let _ = turn_runs::transition_run(
                pool,
                run_id,
                turn_runs::TurnRunState::WaitingForClient,
                None,
            )
            .await;
            let turn_step_id = match turn_steps::ensure_active_step(pool, run_id).await {
                Ok(step) => Some(step.id),
                Err(err) => {
                    tracing::warn!(
                        error = %err,
                        session_id = %session_id,
                        run_id = %run_id,
                        tool_call_id = %tool_call_id,
                        "failed to ensure BearWire run step for tool-call obligation"
                    );
                    None
                }
            };
            let request_payload = json!({
                "tool_call_id": tool_call_id,
                "tool_name": tool_name,
                "arguments": arguments,
                "approval_required": effective_approval_required,
                "approval_request_id": approval_request_id,
                "request_id": request_id,
            });
            let obligation = if effective_approval_required {
                if let Some(permission_id) = approval_request_id.as_deref() {
                    turn_obligations::upsert_permission_decision_obligation_for_step(
                        pool,
                        run_id,
                        session_id,
                        turn_step_id,
                        permission_id,
                        Some(tool_call_id),
                        request_payload,
                    )
                    .await
                } else {
                    turn_obligations::upsert_tool_result_obligation_for_step(
                        pool,
                        run_id,
                        session_id,
                        turn_step_id,
                        tool_call_id,
                        None,
                        request_payload,
                    )
                    .await
                }
            } else {
                turn_obligations::upsert_tool_result_obligation_for_step(
                    pool,
                    run_id,
                    session_id,
                    turn_step_id,
                    tool_call_id,
                    approval_request_id.as_deref(),
                    request_payload,
                )
                .await
            };
            let obligation = match obligation {
                Ok(obligation) => Some(obligation),
                Err(err) => {
                    tracing::warn!(
                        error = %err,
                        session_id = %session_id,
                        run_id = %run_id,
                        tool_call_id = %tool_call_id,
                        "failed to persist BearWire tool-call obligation"
                    );
                    None
                }
            };
            if effective_approval_required {
                if let Err(err) = den_docket::work_runs::mark_attached_work_run_permission_required(
                    pool, session_id,
                )
                .await
                {
                    tracing::warn!(
                        error = %err,
                        session_id,
                        run_id,
                        "failed to project permission requirement onto attached work run"
                    );
                }
            }
            if let Some(started_at) = started_at {
                let (kind, text) = if effective_approval_required {
                    (
                        "tool_waiting_for_permission",
                        "Waiting for client permission to run a local tool…",
                    )
                } else {
                    (
                        "tool_waiting_for_result",
                        "Waiting for local tool result from the armature…",
                    )
                };
                persist_run_progress(
                    pool,
                    session_id,
                    run_id,
                    bear_id,
                    user_id,
                    started_at,
                    kind,
                    text,
                    json!({
                        "tool_call_id": tool_call_id,
                        "tool_name": tool_name,
                        "arguments": arguments,
                        "approval_required": effective_approval_required,
                        "approval_request_id": approval_request_id,
                        "request_id": request_id,
                    }),
                )
                .await;
            }
            obligation
        }

        _ => None,
    }
}

pub(crate) async fn run_recover_result(
    state: &DenState,
    headers: &HeaderMap,
    params: &Value,
) -> Result<Value, CustomError> {
    let (user_id, bear) = authenticated_bear(state, headers, params).await?;
    let request: RunRecoverRequest = parse_params(params)?;
    let row = turn_runs::technical_budget_recovery_snapshot(&state.sqlx_pool, &request.run_id)
        .await?
        .ok_or_else(|| {
            CustomError::NotFound("recoverable technical-budget run not found".to_string())
        })?;
    let snapshot: turn_runs::TechnicalBudgetRecoverySnapshot = serde_json::from_value(row.snapshot)
        .map_err(|_| CustomError::ValidationError("recovery snapshot is invalid".to_string()))?;
    if snapshot.version != turn_runs::TECHNICAL_BUDGET_RECOVERY_SNAPSHOT_VERSION
        || snapshot.bear_id != bear.id
        || snapshot.user_id != user_id
    {
        return Err(CustomError::NotFound(
            "recoverable technical-budget run not found".to_string(),
        ));
    }
    let payload: TechnicalBudgetRecoveryStartPayload =
        serde_json::from_value(snapshot.start_request).map_err(|_| {
            CustomError::ValidationError("recovery start payload is invalid".to_string())
        })?;
    let session = client_sessions::find_for_user_bear_session_id(
        &state.sqlx_pool,
        user_id,
        bear.id,
        &snapshot.session_id,
    )
    .await?
    .ok_or_else(|| CustomError::NotFound("client session not found".to_string()))?;
    let task_id = snapshot.selected_task_id.ok_or_else(|| {
        CustomError::ValidationError(
            "technical-budget recovery requires a selected Pair task".to_string(),
        )
    })?;
    if session.current_task_id != Some(task_id) {
        return Err(CustomError::ValidationError(
            "current Pair task changed; refusing recovery".to_string(),
        ));
    }
    preview_session_current_task_selection(
        &state.sqlx_pool,
        user_id,
        bear.id,
        &snapshot.session_id,
        task_id,
    )
    .await?;
    let recovery_lease_id = Uuid::new_v4();
    let leased = turn_runs::lease_technical_budget_recovery(
        &state.sqlx_pool,
        &request.run_id,
        recovery_lease_id,
    )
    .await?
    .ok_or_else(|| {
        CustomError::NotFound(
            "recoverable technical-budget run is missing or already leased".to_string(),
        )
    })?;
    let start_request = RunStartRequest {
        session_id: snapshot.session_id.clone(),
        prompt: payload.prompt,
        prompt_context: payload.prompt_context,
        client: Some(payload.client),
        conversation_id: Some(payload.conversation_id),
        cwd: payload.cwd,
        requested_mode: payload.requested_mode,
        supersede_active_run: true,
        client_context: payload.client_context,
    };
    let launch = run_start_with_recovery_source(
        state,
        start_request,
        user_id,
        bear.clone(),
        Some(&request.run_id),
        Some(task_id),
    )
    .await;
    let mut launched = match launch {
        Ok(launched) => launched,
        Err(error) => {
            let source_is_recoverable = turn_runs::get_run(&state.sqlx_pool, &request.run_id)
                .await?
                .is_some_and(|run| run.state == "continuing");
            if source_is_recoverable {
                turn_runs::release_technical_budget_recovery(
                    &state.sqlx_pool,
                    &request.run_id,
                    recovery_lease_id,
                )
                .await?;
            } else {
                turn_runs::complete_technical_budget_recovery(
                    &state.sqlx_pool,
                    &request.run_id,
                    recovery_lease_id,
                )
                .await?;
            }
            return Err(error);
        }
    };
    if !turn_runs::complete_technical_budget_recovery(
        &state.sqlx_pool,
        &request.run_id,
        recovery_lease_id,
    )
    .await?
    {
        return Err(CustomError::ValidationError(
            "technical-budget recovery lease changed before launch completed".to_string(),
        ));
    }
    let replacement_run_id = launched["run_id"]
        .as_str()
        .ok_or_else(|| CustomError::System("recovery launch omitted run_id".to_string()))?
        .to_string();
    den_runtime::agent_loop::record_loop_control_decision(
        &state.sqlx_pool,
        den_runtime::agent_loop::LoopControlLedgerInput {
            run_id: request.run_id.clone(),
            turn_step_id: None,
            conversation_message_id: None,
            decision_id: format!("budget-slice-recovery:{}", request.run_id),
            decision_kind: den_runtime::agent_loop::LoopControlDecisionKind::BudgetSliceRecovery,
            control_level: "standard".to_string(),
            reason: Some(leased.reason),
            orientation_kind: None,
            checkpoint_id: None,
            related_task_list_id: None,
            related_task_item_id: Some(task_id.to_string()),
            related_docket_job_id: None,
            related_docket_task_id: Some(task_id),
            evidence_refs: Vec::new(),
            decision: json!({
                "same_run": false,
                "replacement_run_id": replacement_run_id,
                "launch_state": launched["launch_state"],
            }),
        },
    )
    .await?;
    let mut event = BearWireEvent::ephemeral_typed(
        "run.recovered",
        RunRecoveryHandoff {
            run_id: request.run_id.clone(),
            replacement_run_id: replacement_run_id.clone(),
            task_id: Some(task_id.to_string()),
            reason: "technical_budget_recovery".to_string(),
            launch_state: Some(bearwire_protocol::lifecycle::RunLaunchState::Claimed),
            task_selection_preserved: true,
        },
    );
    event.bear_id = Some(bear.id.to_string());
    event.human_id = Some(user_id.to_string());
    event.session_id = Some(snapshot.session_id);
    event.run_id = Some(request.run_id.clone());
    bearwire_events::append_bearwire_event(
        &state.sqlx_pool,
        &session.client_session_id,
        Some(bear.id),
        Some(user_id),
        event,
    )
    .await?;
    launched["recovered"] = json!(true);
    launched["recovered_run_id"] = json!(request.run_id);
    Ok(launched)
}

async fn release_startup_attempt(
    pool: &sqlx::PgPool,
    attempt: Option<&den_docket::DocketExecutionAttemptRow>,
    reason: &str,
) {
    let Some(attempt) = attempt else {
        return;
    };
    if let Err(error) = PgDocketService::from_pool(pool)
        .release_execution_attempt(DocketExecutionAttemptRelease {
            attempt_id: attempt.id,
            fence_epoch: attempt.fence_epoch,
            recovery_key: Uuid::new_v4(),
            recovery_reason: reason.to_string(),
        })
        .await
    {
        tracing::warn!(
            %error,
            attempt_id = %attempt.id,
            fence_epoch = attempt.fence_epoch,
            reason,
            "failed to release focused authority after startup ended"
        );
    }
}

pub(crate) async fn run_start_result(
    state: &DenState,
    headers: &HeaderMap,
    params: &Value,
) -> Result<Value, CustomError> {
    let (user_id, bear) = authenticated_bear(state, headers, params).await?;
    let request: RunStartRequest = parse_params(params)?;
    run_start_with_recovery_source(state, request, user_id, bear, None, None).await
}

pub(crate) async fn run_start_for_focused_task(
    state: &DenState,
    request: RunStartRequest,
    user_id: i32,
    bear: den_service::bears::Bear,
    task_id: Uuid,
) -> Result<Value, CustomError> {
    run_start_with_recovery_source(state, request, user_id, bear, None, Some(task_id)).await
}

async fn run_start_with_recovery_source(
    state: &DenState,
    request: RunStartRequest,
    user_id: i32,
    bear: den_service::bears::Bear,
    recovery_source_run_id: Option<&str>,
    focused_task_id: Option<Uuid>,
) -> Result<Value, CustomError> {
    let session_id = request.session_id;
    let prompt = request.prompt;
    let prompt_context = request.prompt_context;
    let client = request.client.unwrap_or_else(|| DEFAULT_CLIENT.to_string());
    let supersede_active_run = request.supersede_active_run;
    let existing = client_sessions::find_for_user_bear_session(
        &state.sqlx_pool,
        user_id,
        &bear.slug,
        &session_id,
    )
    .await?;
    let conversation_id = request
        .conversation_id
        .or_else(|| {
            existing
                .as_ref()
                .map(|session| session.conversation_id.clone())
        })
        .unwrap_or_else(|| format!("new-acp-{client}-{}", Uuid::new_v4().simple()));
    let resolved_conversation_id = existing
        .as_ref()
        .and_then(|session| session.resolved_conversation_id.clone());
    let upstream_target =
        runtime_upstream_target(&conversation_id, resolved_conversation_id.as_deref());
    let cwd = request.cwd;
    let client_context = request.client_context;
    let workspace_roots = normalized_workspace_roots(client_context.as_ref(), cwd.as_deref())?;
    let requested_mode = request.requested_mode;
    // `TurnAuthority` is the single local authority seam for BearWire's tool
    // surface and prompt permission envelope. The concrete stance is resolved
    // below for runtime binding; BearWire sessions use Pair-equivalent tool
    // authority unless a later typed work-run authority input says otherwise.
    let turn_authority = den_core::client_tools::TurnAuthority::for_session_mode(
        den_core::BearStance::Pair,
        requested_mode.as_deref().unwrap_or("ask"),
        None,
    );
    let client_tools = client_tool_descriptors_from_context_with_authority(
        client_context.as_ref(),
        &turn_authority,
    );
    let read_only_runtime_context = turn_authority
        .read_only_runtime_context()
        .map(|authority| {
            let registry = repository_prompt_fragment_registry()?;
            let fragment = registry.require("runtime_read_only_authority")?;
            render_turn_fragment(fragment, &json!({ "authority": authority }))
        })
        .transpose()?;
    // Stance signal: a session bound to a live work run via `work.checkout`
    // runs in the Work stance; every other BearWire session stays Pair.
    let live_work_run =
        den_docket::work_runs::get_live_work_run_by_session(&state.sqlx_pool, &session_id).await?;
    let stance = if let Some(work_run) = live_work_run {
        tracing::info!(
            work_run_id = %work_run.id,
            job_id = %work_run.job_id,
            task_id = ?work_run.executing_task_id,
            session_id = %session_id,
            stance = "work",
            "run.start resolved live Work-run binding"
        );
        BearProfile::Work
    } else {
        tracing::debug!(
            session_id = %session_id,
            stance = "pair",
            "run.start found no live Work-run binding"
        );
        BearProfile::Pair
    };
    let binding_id = bears_db::profile_binding_id(&state.sqlx_pool, bear.id, stance)
        .await?
        .ok_or_else(|| {
            CustomError::System(format!(
                "Bear {} profile binding not configured",
                stance.as_str()
            ))
        })?;
    let binding = RoleRuntimeBinding {
        binding_id,
        compatibility_backend: Some("native".to_string()),
    };
    let resolved_model =
        preflight_pair_run_model(state, &bear, &session_id, &upstream_target, stance).await?;
    if resolved_model.source.is_default() {
        let conversation =
            den_service::conversation::persistence::ensure_conversation_for_external_id(
                &state.sqlx_pool,
                bear.id,
                Some(user_id),
                &upstream_target,
                Some(&session_id),
                None,
            )
            .await?;
        let established =
            den_service::conversation::persistence::establish_conversation_default_model_state(
                &state.sqlx_pool,
                conversation.id,
                &resolved_model.handle,
                "pair_default_resolved_at_preflight",
            )
            .await?;
        if established {
            tracing::info!(
                session_id,
                conversation_id = %upstream_target,
                model_handle = %resolved_model.handle,
                model_selection_source = %resolved_model.source,
                "Established default Pair model on conversation for continuity"
            );
        }
    }
    client_sessions::upsert_session(
        &state.sqlx_pool,
        client_sessions::UpsertClientSession {
            user_id,
            bear_id: bear.id,
            bear_slug: bear.slug.clone(),
            client_session_id: session_id.clone(),
            runtime_session_id: existing
                .as_ref()
                .map(|session| session.runtime_session_id.clone())
                .unwrap_or_else(|| format!("bearwire:{}:{}", bear.id, session_id)),
            conversation_id: conversation_id.clone(),
            resolved_conversation_id: resolved_conversation_id.clone(),
            client: client.clone(),
            cwd: cwd.clone(),
            current_mode: None,
        },
    )
    .await?;
    if let Some(client_context) = client_context.as_ref() {
        client_sessions::update_adapter_environment(
            &state.sqlx_pool,
            user_id,
            bear.id,
            &session_id,
            client_context,
        )
        .await?;
    }

    let run_id = TurnRunId::new(format!("run_{}", Uuid::new_v4().simple()))?;
    let session_run_id = run_id.to_string();
    let inherited_focused_task_id = if focused_task_id.is_none() {
        PgDocketService::from_pool(&state.sqlx_pool)
            .get_live_session_task_execution_attempt_for_session(bear.id, &session_id)
            .await?
            .map(|attempt| attempt.task_id)
    } else {
        None
    };
    let focused_task_id = focused_task_id.or(inherited_focused_task_id);
    let session_id = ClientSessionId::new(session_id.clone())?;
    let session_id_string = session_id.to_string();
    // `run.start` only replaces an active turn when the caller explicitly
    // identifies this as a new user action, not a retry or reconnect.
    let superseded = if supersede_active_run {
        settle_active_run_for_session(
            state,
            session_id.as_str(),
            bear.id,
            user_id,
            "superseded_by_new_run",
            None,
            Some(run_id.as_str()),
        )
        .await?
    } else {
        SettledRunLifecycle::empty()
    };
    if superseded.settled() {
        tracing::info!(
            session_id = %session_id,
            replacement_run_id = %run_id,
            previous_run_id = superseded.run.as_ref().map(|run| run.run_id.as_str()),
            cancelled_stream = superseded.cancelled_stream,
            cancelled_tool_turn = superseded.cancelled_tool_turn,
            settled_obligations = superseded.settled_obligations,
            event_sequence = superseded.event_sequence,
            "BearWire superseded active run before starting replacement"
        );
    }

    let creation_cause = if supersede_active_run {
        "explicit_supersession"
    } else if recovery_source_run_id.is_some() {
        "recovery_continuation"
    } else {
        // ponytail: The protocol does not distinguish initial user actions from
        // retries/reconnects, so do not infer a false finer-grained cause.
        "new_or_reconnect"
    };
    tracing::debug!(
        session_id = %session_id,
        run_id = %run_id,
        creation_cause,
        recovery_source_run_id,
        "BearWire creating or attaching Pair run"
    );
    let run = if supersede_active_run {
        turn_runs::create_run_with_ids(&state.sqlx_pool, &run_id, &session_id, bear.id, user_id)
            .await?
    } else {
        match turn_runs::create_or_attach_active_run_with_ids(
            &state.sqlx_pool,
            &run_id,
            &session_id,
            bear.id,
            user_id,
        )
        .await?
        {
            turn_runs::CreateOrAttachRun::Created(run) => run,
            turn_runs::CreateOrAttachRun::Attached(active_run) => {
                return Ok(json!({
                    "ok": true,
                    "accepted": true,
                    "reused": true,
                    "run_id": active_run.run_id,
                    "session_id": session_id,
                    "state": active_run.state,
                    "launch_state": "already_running",
                }));
            }
        }
    };
    let setup = async {
        let attempt = if let Some(task_id) = focused_task_id {
            Some(
                PgDocketService::from_pool(&state.sqlx_pool)
                    .acquire_focused_execution(DocketFocusedExecutionAcquire {
                        bear_id: bear.id,
                        task_id,
                        binding: DocketFocusedExecutionBinding {
                            kind: DocketExecutionBindingKind::ClientSession,
                            id: session_id.to_string(),
                        },
                        host: DocketExecutionHost {
                            kind: DocketExecutionHostKind::TurnRun,
                            run_id: session_run_id.clone(),
                        },
                        acquisition_key: Uuid::new_v5(
                            &Uuid::NAMESPACE_URL,
                            session_run_id.as_bytes(),
                        ),
                    })
                    .await?,
            )
        } else {
            None
        };
        Ok::<_, CustomError>(attempt)
    }
    .await;
    let attempt = match setup {
        Ok(attempt) => attempt,
        Err(error) => {
            fail_run_lifecycle(
                &state.sqlx_pool,
                session_id.as_str(),
                run_id.as_str(),
                bear.id,
                user_id,
                RunFailureReason::StartFailed,
                format!("focused execution startup failed: {error}"),
                Some(json!({"task_id": focused_task_id, "phase": "docket_execution_setup"})),
            )
            .await;
            return Err(error);
        }
    };
    let mut accepted = BearWireEvent::ephemeral(
        "run.accepted",
        json!({
            "run_id": run_id.as_str(),
            "session_id": session_id.as_str(),
            "creation_cause": creation_cause,
        }),
    );
    accepted.bear_id = Some(bear.id.to_string());
    accepted.human_id = Some(user_id.to_string());
    accepted.session_id = Some(session_id.to_string());
    accepted.run_id = Some(run_id.to_string());
    let accepted = bearwire_events::append_bearwire_event(
        &state.sqlx_pool,
        session_id.as_str(),
        Some(bear.id),
        Some(user_id),
        accepted,
    )
    .await?;

    let request_id = Uuid::new_v4();
    let (cancel_handle, mut cancel_rx) = state.turn_cancellations.register(
        session_id_string.clone(),
        run_id.to_string(),
        request_id,
        Some(upstream_target.clone()),
    );

    let pool = state.sqlx_pool.clone();
    let livestream_state = state.clone();
    let config = state.config.clone();
    let memory_stores = state.memory_stores.clone();
    let bear_slug = bear.slug.clone();
    let bear_id = bear.id;
    let session_for_task = session_id_string.clone();
    let conversation_for_task = conversation_id.clone();
    let upstream_target_for_task = upstream_target.clone();
    let prompt_for_task = prompt.clone();
    let read_only_runtime_context_for_task = read_only_runtime_context.clone();
    let run_id_for_task = session_run_id.clone();
    let client_tools_for_task = client_tools.clone();
    let attempt_for_task = attempt.clone();
    let api_style_for_task = resolved_model.api_style;
    let supports_reasoning_effort_for_task = resolved_model.supports_reasoning_effort;
    let run_started_at = Instant::now();
    persist_run_progress(
        &state.sqlx_pool,
        &session_id_string,
        &session_run_id,
        bear.id,
        user_id,
        run_started_at,
        "controller_claimed",
        "Execution controller claimed; preparing native context…",
        json!({ "request_id": request_id }),
    )
    .await;

    let run_for_task = run.clone();
    tokio::spawn(async move {
        let _cancel_handle = cancel_handle;
        // A focused Docket task is autonomous work. Do not acknowledge `/focus`
        // merely because Tokio accepted a spawn: wait until the continuation has
        // reached native stream startup, which proves it consumed the task turn.
        // The model's first output remains asynchronous.
        persist_run_progress(
            &pool,
            &session_for_task,
            &run_id_for_task,
            bear_id,
            user_id,
            run_started_at,
            "docket_task_turn_starting",
            "Starting Docket-controlled task turn…",
            json!({ "request_id": request_id }),
        )
        .await;
        let native_start = Instant::now();
        let checkpoint_audit_context =
            match den_docket::work_runs::get_live_work_run_by_session(&pool, &session_for_task)
                .await
            {
                Ok(work_run) => work_run.map(|work_run| den_protocol::CheckpointAuditContext {
                    work_run_id: work_run.id,
                    docket_job_id: work_run.job_id,
                }),
                Err(err) => {
                    tracing::warn!(
                        error = %err,
                        session_id = %session_for_task,
                        "failed to resolve checkpoint audit context; continuing without it"
                    );
                    None
                }
            };
        let stream_result = start_native_profile_turn_event_stream(
            TurnStartRequest {
                sqlx_pool: &pool,
                config: config.as_ref(),
                memory_stores: &memory_stores,
                request_id,
                run_id: Some(&run_id_for_task),
                checkpoint_audit_context,
                user_id,
                session_id: &session_for_task,
                bear_id,
                bear_slug: &bear_slug,
                client: &client,
                cwd: cwd.as_deref(),
                workspace_roots: Some(&workspace_roots),
                binding: &binding,
                conversation_selection: &conversation_for_task,
                upstream_target: &upstream_target_for_task,
                prompt: &prompt_for_task,
                prompt_context: prompt_context.clone(),
                client_tools: Some(client_tools_for_task.clone()),
                runtime_context: read_only_runtime_context_for_task.as_deref(),
                runtime_context_len: read_only_runtime_context_for_task
                    .as_deref()
                    .map(str::len)
                    .unwrap_or(0),
                technical_budget_recovery_start_payload: Some(
                    technical_budget_recovery_start_payload(
                        &client,
                        cwd.as_deref(),
                        &conversation_id,
                        &prompt,
                        prompt_context.as_ref(),
                        client_context.as_ref(),
                        requested_mode.as_deref(),
                    ),
                ),
                stream_tokens: true,
                api_style: Some(api_style_for_task),
                supports_reasoning_effort: supports_reasoning_effort_for_task,
            },
            stance,
        )
        .await;

        match stream_result {
            Ok(mut stream) => {
                if let Some(attempt) = attempt_for_task.as_ref() {
                    if let Err(error) = PgDocketService::from_pool(&pool)
                        .start_execution_attempt(DocketExecutionAttemptStart {
                            attempt_id: attempt.id,
                            fence_epoch: attempt.fence_epoch,
                        })
                        .await
                    {
                        persist_run_failed(
                            &pool,
                            &session_for_task,
                            &run_id_for_task,
                            bear_id,
                            user_id,
                            RunFailureReason::StartFailed,
                            format!("focused execution authority failed to start: {error}"),
                            None,
                        )
                        .await;
                        release_startup_attempt(
                            &pool,
                            attempt_for_task.as_ref(),
                            "focused_authority_start_failed",
                        )
                        .await;
                        return;
                    }
                }
                match turn_runs::transition_run(
                    &pool,
                    &run_id_for_task,
                    turn_runs::TurnRunState::Running,
                    None,
                )
                .await
                {
                    Ok(Some(_)) => {}
                    Ok(None) => {
                        den_runtime::native_runtime::remove_native_client_run(
                            &session_for_task,
                            &run_id_for_task,
                        );
                        release_startup_attempt(
                            &pool,
                            attempt_for_task.as_ref(),
                            "run_terminal_before_native_start",
                        )
                        .await;
                        return;
                    }
                    Err(error) => {
                        persist_run_failed(
                            &pool,
                            &session_for_task,
                            &run_id_for_task,
                            bear_id,
                            user_id,
                            RunFailureReason::StartFailed,
                            format!("failed to transition native execution to running: {error}"),
                            None,
                        )
                        .await;
                        release_startup_attempt(
                            &pool,
                            attempt_for_task.as_ref(),
                            "run_start_transition_failed",
                        )
                        .await;
                        return;
                    }
                }
                let mut started = BearWireEvent::ephemeral(
                    "run.started",
                    json!({
                        "run_id": run_id_for_task,
                        "session_id": session_for_task,
                    }),
                );
                started.bear_id = Some(bear_id.to_string());
                started.human_id = Some(user_id.to_string());
                started.session_id = Some(session_for_task.clone());
                started.run_id = Some(run_id_for_task.clone());
                if let Err(error) = bearwire_events::append_bearwire_event(
                    &pool,
                    &session_for_task,
                    Some(bear_id),
                    Some(user_id),
                    started,
                )
                .await
                {
                    persist_run_failed(
                        &pool,
                        &session_for_task,
                        &run_id_for_task,
                        bear_id,
                        user_id,
                        RunFailureReason::StartFailed,
                        format!("failed to persist native run start: {error}"),
                        None,
                    )
                    .await;
                    release_startup_attempt(
                        &pool,
                        attempt_for_task.as_ref(),
                        "run_started_event_failed",
                    )
                    .await;
                    return;
                }
                if attempt_for_task.is_some() {
                    match super::focused_execution::load_focused_execution_snapshot(
                        &livestream_state,
                        user_id,
                        bear_id,
                        &session_for_task,
                        super::focused_execution::FocusedExecutionLaunchState::Started,
                    )
                    .await
                    {
                        Ok(execution) => {
                            super::focused_execution::project_execution_transition(
                                &livestream_state,
                                user_id,
                                bear_id,
                                &execution,
                            )
                            .await;
                        }
                        Err(error) => tracing::warn!(
                            %error,
                            session_id = %session_for_task,
                            run_id = %run_id_for_task,
                            "failed to project native focused execution start"
                        ),
                    }
                }
                persist_run_progress(
                    &pool,
                    &session_for_task,
                    &run_id_for_task,
                    bear_id,
                    user_id,
                    run_started_at,
                    "model_stream_waiting",
                    "Context is ready; waiting for model output or tool request…",
                    json!({
                        "request_id": request_id,
                        "native_context_ms": native_start.elapsed().as_millis(),
                    }),
                )
                .await;
                let mut first_event_seen = false;
                let mut provider_activity_seen = false;
                let mut terminal_or_wait_seen = false;
                let mut docket_continuation_requested = false;
                let mut cancellation_seen = false;
                let mut runtime_event_count = 0usize;
                let mut assistant_content_seen = false;
                let mut terminal_event_seen = false;
                let mut wait_event_seen = false;
                let mut last_event_kind: Option<&'static str> = None;
                // Counts only event categories; never payload text or tool arguments.
                // ponytail: capped category map because the native enum is finite; expand
                // this only if event kinds become dynamically extensible.
                let mut runtime_event_kinds = BTreeMap::<&'static str, usize>::new();
                let idle_watchdog_timeout = crate::methods::client::continuation_watchdog_timeout();
                let handshake_timeout = den_runtime::agent_loop::native_llm_handshake_timeout();
                let first_event_watchdog_timeout =
                    crate::methods::client::continuation_first_event_watchdog_timeout(
                        handshake_timeout,
                        idle_watchdog_timeout,
                    );
                loop {
                    let watchdog_timeout = if first_event_seen || provider_activity_seen {
                        idle_watchdog_timeout
                    } else {
                        first_event_watchdog_timeout
                    };
                    tokio::select! {
                        changed = cancel_rx.changed() => {
                            if changed.is_ok() && *cancel_rx.borrow() {
                                cancellation_seen = true;
                                tracing::info!(
                                    session_id = %session_for_task,
                                    run_id = %run_id_for_task,
                                    request_id = %request_id,
                                    "BearWire background run stream observed cancellation"
                                );
                                break;
                            }
                        }
                        timed = tokio::time::timeout(watchdog_timeout, stream.next()) => {
                            let item = match timed {
                                Ok(item) => item,
                                Err(_) => {
                                    persist_run_failed(
                                        &pool,
                                        &session_for_task,
                                        &run_id_for_task,
                                        bear_id,
                                        user_id,
                                        RunFailureReason::InitialStreamWatchdogTimeout,
                                        if provider_activity_seen {
                                            format!("Provider activity stopped for {}ms before the run reached a terminal or client-wait event.", watchdog_timeout.as_millis())
                                        } else {
                                            format!("The provider did not begin streaming within the {}ms initial handshake window.", watchdog_timeout.as_millis())
                                        },
                                        Some(json!({
                                            "request_id": request_id,
                                            "provider_activity_seen": provider_activity_seen,
                                            "first_event_seen": first_event_seen,
                                            "watchdog_timeout_ms": watchdog_timeout.as_millis(),
                                        })),
                                    )
                                    .await;
                                    break;
                                }
                            };
                            let Some(item) = item else {
                                break;
                            };
                            match item {
                                Ok(den_protocol::RuntimeStreamEvent::ProviderActivity) => {
                                    provider_activity_seen = true;
                                }
                                Ok(runtime_event) => {
                                    runtime_event_count += 1;
                                    assistant_content_seen |= runtime_event_has_assistant_content(&runtime_event);
                                    let event_kind = runtime_event_kind(&runtime_event);
                                    *runtime_event_kinds.entry(event_kind).or_default() += 1;
                                    last_event_kind = Some(event_kind);
                                    match runtime_stream_boundary(&runtime_event) {
                                        RuntimeStreamBoundary::Terminal => terminal_event_seen = true,
                                        RuntimeStreamBoundary::ClientWait => wait_event_seen = true,
                                        RuntimeStreamBoundary::BoundedSlice => {
                                            if let Some(continuation) =
                                                docket_bounded_slice_continuation(
                                                    report_pair_bounded_outcome(
                                                        &livestream_state,
                                                        user_id,
                                                        bear_id,
                                                        &session_for_task,
                                                        &run_id_for_task,
                                                        DocketPairBoundedOutcome::Progress,
                                                    )
                                                    .await,
                                                )
                                            {
                                                let _ = turn_runs::transition_run(
                                                    &pool,
                                                    &run_id_for_task,
                                                    turn_runs::TurnRunState::Continuing,
                                                    Some("docket_bounded_slice"),
                                                )
                                                .await;
                                                crate::methods::client::spawn_continuation_task(
                                                    &livestream_state,
                                                    run_for_task.clone(),
                                                    binding.binding_id.clone(),
                                                    conversation_for_task.clone(),
                                                    continuation,
                                                );
                                                docket_continuation_requested = true;
                                                break;
                                            }
                                        }
                                        RuntimeStreamBoundary::Continue => {}
                                    }
                                    if runtime_event_is_terminal_or_wait(&runtime_event) {
                                        terminal_or_wait_seen = true;
                                    }
                                    if !first_event_seen {
                                        first_event_seen = true;
                                        persist_run_progress(
                                            &pool,
                                            &session_for_task,
                                            &run_id_for_task,
                                            bear_id,
                                            user_id,
                                            run_started_at,
                                            "first_runtime_event",
                                            "Received first runtime event from model/native loop.",
                                            json!({
                                                "request_id": request_id,
                                                "event_kind": runtime_event_kind(&runtime_event),
                                            }),
                                        )
                                        .await;
                                    }
                                    persist_runtime_event_as_bearwire(
                                        &livestream_state,
                                        &pool,
                                        &session_for_task,
                                        &run_id_for_task,
                                        bear_id,
                                        user_id,
                                        runtime_event,
                                        request_id,
                                        Some(run_started_at),
                                    )
                                    .await;
                                }
                                Err(err) => {
                                    persist_run_failed(
                                        &pool,
                                        &session_for_task,
                                        &run_id_for_task,
                                        bear_id,
                                        user_id,
                                        RunFailureReason::StreamError,
                                        err.to_string(),
                                        None,
                                    )
                                    .await;
                                    break;
                                }
                            }
                        }
                    }
                }
                if !docket_continuation_requested
                    && initial_stream_eof_is_recoverable(
                        terminal_or_wait_seen,
                        cancellation_seen,
                        docket_continuation_requested,
                    )
                {
                    // An EOF only ends this delivery stream. Do not settle the run or its
                    // obligations: a later client retry can reconcile from persisted events.
                    let _ = turn_runs::transition_run(
                        &pool,
                        &run_id_for_task,
                        turn_runs::TurnRunState::Running,
                        Some("initial_stream_interrupted"),
                    )
                    .await;
                    let pending_tool_calls =
                        incomplete_stream_pending_tools(&pool, &run_id_for_task).await;
                    let detail = json!({
                        "request_id": request_id,
                        "final_run_state": "running",
                        "first_event_seen": first_event_seen,
                        "provider_activity_seen": provider_activity_seen,
                        "assistant_content_seen": assistant_content_seen,
                        "runtime_event_count": runtime_event_count,
                        "terminal_event_seen": terminal_event_seen,
                        "wait_event_seen": wait_event_seen,
                        "last_event_kind": last_event_kind,
                        "pending_tool_calls": pending_tool_calls,
                        "recovery_attempted": false,
                        "recovery_outcome": "retryable_from_persisted_state",
                    });
                    tracing::warn!(
                        session_id = %session_for_task,
                        run_id = %run_id_for_task,
                        request_id = %request_id,
                        elapsed_ms = run_started_at.elapsed().as_millis(),
                        first_event_seen,
                        provider_activity_seen,
                        assistant_content_seen,
                        runtime_event_count,
                        terminal_event_seen,
                        wait_event_seen,
                        last_event_kind = ?last_event_kind,
                        pending_tool_calls = ?pending_tool_calls,
                        "BearWire runtime stream ended without a durable boundary; run remains retryable"
                    );
                    persist_run_progress(
                        &pool,
                        &session_for_task,
                        &run_id_for_task,
                        bear_id,
                        user_id,
                        run_started_at,
                        "initial_stream_interrupted",
                        "The model connection ended before the run finished. The run is preserved for retry.",
                        detail.clone(),
                    )
                    .await;
                    persist_visible_runtime_marker(
                        &pool,
                        &session_for_task,
                        &run_id_for_task,
                        bear_id,
                        user_id,
                        "initial_stream_interrupted",
                        initial_stream_interruption_message().to_string(),
                        detail.clone(),
                    )
                    .await;
                    if let Err(error) = den_runtime::agent_loop::record_loop_control_decision(
                        &pool,
                        LoopControlLedgerInput {
                            run_id: run_id_for_task.clone(),
                            turn_step_id: None,
                            conversation_message_id: None,
                            decision_id: format!("delivery-interrupted:{run_id_for_task}"),
                            decision_kind: LoopControlDecisionKind::DeliveryInterrupted,
                            control_level: "standard".to_string(),
                            reason: Some("initial_stream_interrupted".to_string()),
                            orientation_kind: None,
                            checkpoint_id: None,
                            related_task_list_id: None,
                            related_task_item_id: None,
                            related_docket_job_id: None,
                            related_docket_task_id: None,
                            evidence_refs: vec![LedgerEvidenceRef {
                                kind: "bearwire_event".to_string(),
                                id: "run.interrupted".to_string(),
                            }],
                            decision: json!({
                                "action": "terminalize_delivery",
                                "retryable": true,
                                "settles_run": false,
                            }),
                        },
                    )
                    .await
                    {
                        tracing::warn!(
                            session_id = %session_for_task,
                            run_id = %run_id_for_task,
                            error = %error,
                            "failed to record retryable delivery interruption in loop-control ledger"
                        );
                    }
                    let mut interruption_event =
                        retryable_stream_interruption_event(&run_id_for_task, detail);
                    interruption_event.bear_id = Some(bear_id.to_string());
                    interruption_event.human_id = Some(user_id.to_string());
                    interruption_event.session_id = Some(session_for_task.clone());
                    interruption_event.run_id = Some(run_id_for_task.clone());
                    if let Err(err) = bearwire_events::append_bearwire_event(
                        &pool,
                        &session_for_task,
                        Some(bear_id),
                        Some(user_id),
                        interruption_event,
                    )
                    .await
                    {
                        tracing::warn!(
                            session_id = %session_for_task,
                            run_id = %run_id_for_task,
                            error = %err,
                            "failed to persist retryable stream interruption event"
                        );
                    }
                }
            }
            Err(err) => {
                persist_run_failed(
                    &pool,
                    &session_for_task,
                    &run_id_for_task,
                    bear_id,
                    user_id,
                    RunFailureReason::StartFailed,
                    err.to_string(),
                    None,
                )
                .await;
                release_startup_attempt(
                    &pool,
                    attempt_for_task.as_ref(),
                    "native_session_start_failed",
                )
                .await;
            }
        }
    });

    Ok(json!({
        "ok": true,
        "accepted": true,
        "run_id": run_id,
        "session_id": session_id,
        "event_sequence": accepted.sequence_no,
        "state": "accepted",
        "launch_state": "claimed",
        "execution_attempt_id": attempt.as_ref().map(|attempt| attempt.id),
        "execution_attempt_state": attempt.as_ref().map(|attempt| attempt.state),
        "fence_epoch": attempt.as_ref().map(|attempt| attempt.fence_epoch),
    }))
}

async fn run_obligations_payload(
    pool: &sqlx::PgPool,
    run_id: &str,
) -> Result<Vec<Value>, CustomError> {
    let rows = sqlx::query(
        r"
        SELECT id, run_id, session_id, kind, expected_responder_action,
               tool_call_id, permission_id, state, turn_step_id, request_payload, result_payload,
               created_at, updated_at, completed_at
        FROM turn_obligations
        WHERE run_id = $1
        ORDER BY created_at ASC, id ASC
        ",
    )
    .bind(run_id)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|row| {
            let created_at: OffsetDateTime = row.get("created_at");
            let updated_at: OffsetDateTime = row.get("updated_at");
            let completed_at: Option<OffsetDateTime> = row.get("completed_at");
            json!({
                "id": row.get::<uuid::Uuid, _>("id"),
                "run_id": row.get::<String, _>("run_id"),
                "session_id": row.get::<String, _>("session_id"),
                "kind": row.get::<String, _>("kind"),
                "expected_responder_action": row.get::<String, _>("expected_responder_action"),
                "tool_call_id": row.get::<Option<String>, _>("tool_call_id"),
                "permission_id": row.get::<Option<String>, _>("permission_id"),
                "state": row.get::<String, _>("state"),
                "turn_step_id": row.get::<Option<uuid::Uuid>, _>("turn_step_id"),
                "request_payload": row.get::<Value, _>("request_payload"),
                "result_payload": row.get::<Option<Value>, _>("result_payload"),
                "created_at": created_at,
                "updated_at": updated_at,
                "completed_at": completed_at,
            })
        })
        .collect())
}

async fn run_results_payload(pool: &sqlx::PgPool, run_id: &str) -> Result<Vec<Value>, CustomError> {
    let rows = sqlx::query(
        r"
        SELECT id, run_id, obligation_kind, obligation_id, result_hash, payload_json, turn_step_id, created_at
        FROM turn_obligation_results
        WHERE run_id = $1
        ORDER BY created_at ASC, id ASC
        ",
    )
    .bind(run_id)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|row| {
            let created_at: OffsetDateTime = row.get("created_at");
            json!({
                "id": row.get::<uuid::Uuid, _>("id"),
                "run_id": row.get::<String, _>("run_id"),
                "obligation_kind": row.get::<String, _>("obligation_kind"),
                "obligation_id": row.get::<String, _>("obligation_id"),
                "result_hash": row.get::<String, _>("result_hash"),
                "payload_json": row.get::<Value, _>("payload_json"),
                "turn_step_id": row.get::<Option<uuid::Uuid>, _>("turn_step_id"),
                "created_at": created_at,
            })
        })
        .collect())
}

fn run_livestream_projection(
    run: &turn_runs::TurnRunRow,
    open_obligations: &[Value],
    events: Vec<Value>,
) -> Value {
    let mut events = events
        .into_iter()
        .filter(|event| {
            !matches!(
                event.get("event_type").and_then(Value::as_str),
                Some("run.progress" | "message.delta" | "message.reasoning.delta")
            )
        })
        .collect::<Vec<_>>();
    // ponytail: this snapshot only needs the latest durable lifecycle evidence;
    // add a typed projection enum when a subscriber endpoint consumes it directly.
    events.shrink_to_fit();
    json!({
        "kind": "livestream",
        "run_id": run.run_id,
        "state": run.state,
        "updated_at": run.updated_at,
        "waiting": !open_obligations.is_empty(),
        "open_obligations": open_obligations,
        "events": events,
    })
}

fn run_audit_projection(run: &turn_runs::TurnRunRow, events: Vec<Value>) -> Value {
    let events = events
        .into_iter()
        .filter(|event| {
            !matches!(
                event.get("event_type").and_then(Value::as_str),
                Some("message.delta" | "message.reasoning.delta")
            )
        })
        .map(|event| {
            // Audit needs ordering and lifecycle evidence, not provider or tool payloads.
            json!({
                "id": event.get("id"),
                "sequence_no": event.get("sequence_no"),
                "event_type": event.get("event_type"),
                "created_at": event.get("created_at"),
            })
        })
        .collect::<Vec<_>>();
    json!({
        "kind": "audit",
        "run_id": run.run_id,
        "state": run.state,
        "terminal_reason": run.terminal_reason,
        "created_at": run.created_at,
        "updated_at": run.updated_at,
        "completed_at": run.completed_at,
        "events": events,
    })
}

async fn run_recent_events_payload(
    pool: &sqlx::PgPool,
    session_id: &str,
    run_id: &str,
    limit: i64,
) -> Result<Vec<Value>, CustomError> {
    let rows = sqlx::query(
        r"
        SELECT id, sequence_no, event_type, event_json, created_at
        FROM bearwire_events
        WHERE session_id = $1
          AND event_json->>'run_id' = $2
        ORDER BY sequence_no DESC
        LIMIT $3
        ",
    )
    .bind(session_id)
    .bind(run_id)
    .bind(limit.clamp(1, 200))
    .fetch_all(pool)
    .await?;
    let mut events = rows
        .into_iter()
        .map(|row| {
            let created_at: OffsetDateTime = row.get("created_at");
            json!({
                "id": row.get::<uuid::Uuid, _>("id"),
                "sequence_no": row.get::<i64, _>("sequence_no"),
                "event_type": row.get::<String, _>("event_type"),
                "event": row.get::<Value, _>("event_json"),
                "created_at": created_at,
            })
        })
        .collect::<Vec<_>>();
    events.reverse();
    Ok(events)
}

pub(crate) async fn run_state_result(
    state: &DenState,
    headers: &HeaderMap,
    params: &Value,
) -> Result<Value, CustomError> {
    let (user_id, bear) = authenticated_bear(state, headers, params).await?;
    let request: RunStateRequest = parse_params(params)?;
    if request.bear_slug != bear.slug {
        return Err(CustomError::Authorization(
            "bear_slug does not match authenticated Bear".to_string(),
        ));
    }
    let run = turn_runs::get_run(&state.sqlx_pool, &request.run_id)
        .await?
        .ok_or_else(|| CustomError::NotFound("BearWire run not found".to_string()))?;
    if run.bear_id != bear.id || run.user_id != user_id {
        return Err(CustomError::Authorization(
            "run does not belong to authenticated Bear".to_string(),
        ));
    }
    if let Some(session_id) = request.session_id.as_deref() {
        if run.session_id != session_id {
            return Err(CustomError::Authorization(
                "run does not belong to requested session".to_string(),
            ));
        }
    }
    let obligations = run_obligations_payload(&state.sqlx_pool, &run.run_id).await?;
    let open_obligations = obligations
        .iter()
        .filter(|obligation| {
            obligation
                .get("state")
                .and_then(Value::as_str)
                .is_some_and(|state| matches!(state, "requested" | "waiting_for_client"))
        })
        .cloned()
        .collect::<Vec<_>>();
    let blocking_reason = turn_obligations::BlockingReason::from_expected_actions(
        open_obligations.iter().filter_map(|obligation| {
            obligation
                .get("expected_responder_action")
                .and_then(Value::as_str)
                .and_then(|action| {
                    turn_obligations::ExpectedResponderAction::try_from_storage(action).ok()
                })
        }),
    )
    .map(turn_obligations::BlockingReason::as_str);
    let results = run_results_payload(&state.sqlx_pool, &run.run_id).await?;
    let recent_events = run_recent_events_payload(
        &state.sqlx_pool,
        &run.session_id,
        &run.run_id,
        request.limit.unwrap_or(50),
    )
    .await?;
    let audit = run_audit_projection(&run, recent_events.clone());
    let livestream = run_livestream_projection(&run, &open_obligations, recent_events);
    Ok(json!({
        "kind": "run_state",
        "run": run,
        "blocking_reason": blocking_reason,
        "open_obligations": open_obligations,
        "obligations": obligations,
        "results": results,
        "recent_events": livestream["events"],
        "livestream": livestream,
        "audit": audit,
    }))
}

pub(crate) async fn run_cancel_result(
    state: &DenState,
    headers: &HeaderMap,
    params: &Value,
) -> Result<Value, CustomError> {
    let (user_id, bear) = authenticated_bear(state, headers, params).await?;
    let request: RunCancelRequest = parse_params(params)?;
    let session_id = request.session_id;
    let requested_run_id = request.run_id;
    let Some(session) = client_sessions::find_for_user_bear_session(
        &state.sqlx_pool,
        user_id,
        &bear.slug,
        &session_id,
    )
    .await?
    else {
        return Ok(json!({
            "ok": true,
            "cancelled": false,
            "session_id": session_id,
            "reason": "session_not_found",
        }));
    };

    let settled = settle_active_run_for_session(
        state,
        &session.client_session_id,
        bear.id,
        user_id,
        "client_requested",
        Some(&requested_run_id),
        None,
    )
    .await?;
    let cancelled = settled.settled();

    Ok(json!({
        "ok": true,
        "cancelled": cancelled,
        "session_id": session_id,
        "requested_run_id": requested_run_id,
        "run_id": settled.run.as_ref().map(|run| run.run_id.clone()),
        "cancelled_stream": settled.cancelled_stream,
        "cancelled_tool_turn": settled.cancelled_tool_turn,
        "settled_obligations": settled.settled_obligations,
        "event_sequence": settled.event_sequence,
        "reason": if cancelled { "client_requested" } else { "no_active_run" },
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn descriptor_names(value: &Value) -> Vec<&str> {
        value
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|item| item.get("name").and_then(Value::as_str))
            .collect()
    }

    #[test]
    fn docket_bounded_slice_hands_off_only_on_continue() {
        assert_eq!(
            docket_bounded_slice_continuation(Some(DocketPairContinuationDecision::Continue)),
            Some(RuntimeContinuation::DocketBoundedSlice)
        );
        assert_eq!(
            docket_bounded_slice_continuation(Some(DocketPairContinuationDecision::AwaitUser)),
            None
        );
        assert_eq!(
            docket_bounded_slice_continuation(Some(DocketPairContinuationDecision::Stop)),
            None
        );
        assert_eq!(docket_bounded_slice_continuation(None), None);
    }

    #[test]
    fn terminal_runtime_events_are_distinct_from_bounded_slices() {
        use den_protocol::{RuntimeSemanticEvent, RuntimeStreamEvent};

        let completed =
            RuntimeStreamEvent::Semantic(RuntimeSemanticEvent::TurnCompleted { turn: None });
        let bounded_slice = RuntimeStreamEvent::Semantic(RuntimeSemanticEvent::BoundedSlice {
            reason: "task remains actionable".to_string(),
        });

        assert!(runtime_event_is_terminal(&completed));
        assert!(!runtime_event_is_terminal(&bounded_slice));
        assert_eq!(
            runtime_stream_boundary(&completed),
            RuntimeStreamBoundary::Terminal
        );
        assert_eq!(
            runtime_stream_boundary(&bounded_slice),
            RuntimeStreamBoundary::BoundedSlice
        );
    }

    #[test]
    fn technical_budget_recovery_payload_preserves_restart_inputs_only() {
        let payload = technical_budget_recovery_start_payload(
            "test-client",
            Some("/workspace/project"),
            "conversation-1",
            "Continue the task.",
            Some(&json!({"source": "user"})),
            Some(&json!({"mcp": {"client_tools": []}})),
            Some("ask"),
        );

        assert_eq!(payload["client"], "test-client");
        assert_eq!(payload["conversation_id"], "conversation-1");
        assert_eq!(payload["requested_mode"], "ask");
        assert!(payload.get("request_id").is_none());
        assert!(payload.get("api_key").is_none());
        let decoded: TechnicalBudgetRecoveryStartPayload = serde_json::from_value(payload).unwrap();
        assert_eq!(decoded.client, "test-client");
        assert_eq!(decoded.prompt, "Continue the task.");
    }

    #[test]
    fn technical_budget_recovery_payload_rejects_unknown_fields() {
        let error = serde_json::from_value::<TechnicalBudgetRecoveryStartPayload>(json!({
            "client": "test-client",
            "cwd": null,
            "conversation_id": "conversation-1",
            "prompt": "Continue the task.",
            "prompt_context": null,
            "client_context": null,
            "requested_mode": null,
            "api_key": "must-not-be-durable"
        }))
        .unwrap_err();

        assert!(error.to_string().contains("api_key"));
    }

    #[test]
    fn only_persisted_conversation_models_may_bypass_catalog_refresh() {
        assert!(ResolvedRunModelSource::ConversationExplicit.is_persisted_conversation_model());
        assert!(ResolvedRunModelSource::ConversationAuto.is_persisted_conversation_model());
        assert!(!ResolvedRunModelSource::ProfileDefault.is_persisted_conversation_model());
        assert!(!ResolvedRunModelSource::BearDefault.is_persisted_conversation_model());
        assert!(!ResolvedRunModelSource::SystemDefault.is_persisted_conversation_model());
    }

    #[test]
    fn run_progress_warnings_are_reserved_for_runtime_error_events() {
        assert!(run_progress_is_warning(&json!({ "event_kind": "error" })));
        assert!(!run_progress_is_warning(
            &json!({ "event_kind": "turn_completed" })
        ));
        assert!(!run_progress_is_warning(&json!({})));
    }

    #[test]
    fn livestream_projection_omits_transient_history_and_keeps_open_obligations() {
        let run = turn_runs::TurnRunRow {
            id: uuid::Uuid::nil(),
            run_id: "run-test".to_string(),
            session_id: "session-test".to_string(),
            bear_id: uuid::Uuid::nil(),
            user_id: 1,
            state: "waiting_for_client".to_string(),
            terminal_reason: None,
            created_at: OffsetDateTime::UNIX_EPOCH,
            updated_at: OffsetDateTime::UNIX_EPOCH,
            completed_at: None,
        };
        let projection = run_livestream_projection(
            &run,
            &[json!({ "id": "obl-1", "state": "waiting_for_client" })],
            vec![
                json!({ "event_type": "run.progress" }),
                json!({ "event_type": "message.delta" }),
                json!({ "event_type": "tool_call.completed" }),
            ],
        );

        assert_eq!(projection["state"], "waiting_for_client");
        assert_eq!(projection["waiting"], true);
        assert_eq!(projection["open_obligations"][0]["id"], "obl-1");
        assert_eq!(projection["events"].as_array().unwrap().len(), 1);
        assert_eq!(projection["events"][0]["event_type"], "tool_call.completed");
    }

    #[test]
    fn audit_projection_keeps_timing_evidence_without_event_payloads() {
        let run = turn_runs::TurnRunRow {
            id: uuid::Uuid::nil(),
            run_id: "run-test".to_string(),
            session_id: "session-test".to_string(),
            bear_id: uuid::Uuid::nil(),
            user_id: 1,
            state: "completed".to_string(),
            terminal_reason: Some("completed".to_string()),
            created_at: OffsetDateTime::UNIX_EPOCH,
            updated_at: OffsetDateTime::UNIX_EPOCH,
            completed_at: Some(OffsetDateTime::UNIX_EPOCH),
        };
        let audit = run_audit_projection(
            &run,
            vec![
                json!({ "id": "evt-1", "sequence_no": 1, "event_type": "message.delta", "event": { "secret": "no" }, "created_at": "now" }),
                json!({ "id": "evt-2", "sequence_no": 2, "event_type": "tool_call.completed", "event": { "raw_output": "no" }, "created_at": "later" }),
            ],
        );

        assert_eq!(audit["state"], "completed");
        assert_eq!(audit["events"].as_array().unwrap().len(), 1);
        assert_eq!(audit["events"][0]["event_type"], "tool_call.completed");
        assert!(audit["events"][0].get("event").is_none());
    }

    #[test]
    fn replaceable_livestream_events_are_not_persisted() {
        assert!(is_replaceable_livestream_event(&BearWireEvent::ephemeral(
            "run.progress",
            json!({ "kind": "status_text", "text": "Thinking…" }),
        )));
        assert!(!is_replaceable_livestream_event(&BearWireEvent::ephemeral(
            "tool_call.completed",
            json!({}),
        )));
    }

    #[test]
    fn run_failure_reasons_preserve_persisted_strings() {
        assert_eq!(
            RunFailureReason::ContinuationWatchdogTimeout.as_str(),
            "continuation_watchdog_timeout"
        );
        assert_eq!(
            RunFailureReason::ContinuationStreamError.as_str(),
            "continuation_stream_error"
        );
        assert_eq!(
            RunFailureReason::ContinuationLoopControlLedgerPersistence.as_str(),
            "continuation_loop_control_ledger_persistence_failed"
        );
        assert_eq!(
            RunFailureReason::ContinuationTechnicalBudgetSetup.as_str(),
            "continuation_technical_budget_setup_failed"
        );
        assert_eq!(
            RunFailureReason::ContinuationStreamEndedWithoutRuntimeTerminal.as_str(),
            "continuation_stream_ended_without_runtime_terminal"
        );
        assert_eq!(
            RunFailureReason::ContinuationStartFailed.as_str(),
            "continuation_start_failed"
        );
    }

    #[test]
    fn supersession_cancellation_projection_is_terminal_and_names_the_successor() {
        let event = json!({
            "type": "run.cancelled",
            "scope": "ephemeral",
            "run_id": "run-old",
            "reason": "superseded_by_new_run",
            "superseded_by_run_id": "run-new",
        });

        assert_eq!(event["type"], "run.cancelled");
        assert_eq!(event["run_id"], "run-old");
        assert_eq!(event["superseded_by_run_id"], "run-new");
    }

    #[test]
    fn initial_stream_eof_policy_preserves_waiting_runs_and_retries_unbounded_runs() {
        // run.started → tool activity → client.waiting/run.paused → EOF is already a
        // durable client boundary, so the session remains available without failing it.
        assert!(!initial_stream_eof_is_recoverable(true, false, false));
        // Terminal completion remains a durable boundary too.
        assert!(!initial_stream_eof_is_recoverable(true, false, false));
        // EOF without either boundary is the recoverable path: persisted state is used
        // for later reconciliation rather than rerunning tools.
        assert!(initial_stream_eof_is_recoverable(false, false, false));
        // Explicit cancellation is never resumed.
        assert!(!initial_stream_eof_is_recoverable(false, true, false));
        // A Docket-authorized successor owns the next slice, so the ended delivery
        // stream is not reported as a retryable interruption.
        assert!(!initial_stream_eof_is_recoverable(false, false, true));
    }

    #[test]
    fn retryable_stream_interruption_ends_delivery_without_settling_the_run() {
        let event =
            retryable_stream_interruption_event("run-test", json!({ "pending_tool_calls": [] }));

        assert_eq!(event.event_type, "run.interrupted");
        assert_eq!(event.data["run_id"], "run-test");
        assert_eq!(event.data["retryable"], true);
        assert_eq!(event.data["reason"], "initial_stream_interrupted");
        assert!(event.data["message"]
            .as_str()
            .is_some_and(|message| message.contains("Send another message to retry")));
    }

    #[test]
    fn initial_stream_interruption_message_is_den_branded_and_safe() {
        let message = initial_stream_interruption_message();
        assert!(message.starts_with("Den "));
        assert!(message.contains("preserved"));
        assert!(!message.contains("BearWire"));
        assert!(!message.contains("frames="));
        assert!(!message.contains("version="));
    }

    #[test]
    fn assistant_content_tracking_excludes_status_and_tool_events() {
        use den_protocol::{RuntimeSemanticEvent, RuntimeStreamEvent};

        assert!(runtime_event_has_assistant_content(
            &RuntimeStreamEvent::Semantic(RuntimeSemanticEvent::AssistantTextDelta {
                text: "hello".to_string(),
            })
        ));
        assert!(!runtime_event_has_assistant_content(
            &RuntimeStreamEvent::Semantic(RuntimeSemanticEvent::StatusText {
                text: "working".to_string(),
            })
        ));
    }

    #[test]
    fn initial_stream_eof_is_recoverable_only_without_durable_boundary() {
        assert!(initial_stream_eof_is_recoverable(false, false, false));
        assert!(!initial_stream_eof_is_recoverable(true, false, false));
        assert!(!initial_stream_eof_is_recoverable(false, true, false));
        assert!(!initial_stream_eof_is_recoverable(false, false, true));
    }

    #[test]
    fn runtime_upstream_target_prefers_resolved_conversation_for_history() {
        assert_eq!(
            runtime_upstream_target("new-acp-zed-pending", Some("den-conv-existing")),
            "den-conv-existing"
        );
    }

    #[test]
    fn runtime_upstream_target_falls_back_to_session_selection() {
        assert_eq!(
            runtime_upstream_target("new-acp-zed-pending", None),
            "new-acp-zed-pending"
        );
        assert_eq!(
            runtime_upstream_target("conv-existing", Some("   ")),
            "conv-existing"
        );
    }

    fn available_model(handle: &str, model: &str) -> den_service::bifrost::BifrostModelMetadata {
        den_service::bifrost::BifrostModelMetadata {
            handle: handle.to_string(),
            provider: "openai".to_string(),
            model: model.to_string(),
            display_name: None,
            context_window: 0,
            max_output_tokens: None,
            enabled: true,
            supports_tools: None,
            supports_responses_api: None,
            supports_vision: None,
            supports_reasoning_effort: None,
        }
    }

    #[test]
    fn available_model_matches_den_handle_or_provider_model_id() {
        let resolved = ResolvedRunModel {
            handle: "openai/gpt-5.5".to_string(),
            provider_model_id: "gpt-5.5".to_string(),
            api_style: den_llm::LlmApiStyle::ResponsesStream,
            supports_reasoning_effort: None,
            source: ResolvedRunModelSource::ConversationExplicit,
        };

        assert!(available_model_matches(
            &available_model("openai/gpt-5.5", "gpt-5.5"),
            &resolved
        ));
        assert!(available_model_matches(
            &available_model("gpt-5.5", "gpt-5.5"),
            &resolved
        ));
        assert!(!available_model_matches(
            &available_model("openai/gpt-5.1", "gpt-5.1"),
            &resolved
        ));
    }

    #[test]
    fn pair_api_style_prefers_responses_unless_catalog_explicitly_denies_support() {
        assert_eq!(
            pair_api_style_for_catalog_support(None),
            den_llm::LlmApiStyle::ResponsesStream
        );
        assert_eq!(
            pair_api_style_for_catalog_support(Some(true)),
            den_llm::LlmApiStyle::ResponsesStream
        );
        assert_eq!(
            pair_api_style_for_catalog_support(Some(false)),
            den_llm::LlmApiStyle::ChatCompletionsStream
        );
    }

    #[test]
    fn pair_model_capabilities_allow_unknown_tool_support() {
        let entry = BifrostCatalogEntry {
            available: true,
            provider: "provider-a".to_string(),
            provider_model_id: "model-unknown-tools".to_string(),
            gateway_handle: "provider-a/model-unknown-tools".to_string(),
            display_name: None,
            context_window: 128_000,
            max_output_tokens: Some(4096),
            supports_tools: None,
            supports_responses_api: None,
            supports_vision: None,
            supports_reasoning_effort: None,
        };

        ensure_pair_model_capabilities(&entry, &entry.gateway_handle)
            .expect("unknown optional metadata must not block Pair");
    }

    #[test]
    fn pair_model_capabilities_reject_explicit_tool_denial() {
        let mut entry = BifrostCatalogEntry {
            available: true,
            provider: "provider-a".to_string(),
            provider_model_id: "model-no-tools".to_string(),
            gateway_handle: "provider-a/model-no-tools".to_string(),
            display_name: None,
            context_window: 128_000,
            max_output_tokens: Some(4096),
            supports_tools: Some(false),
            supports_responses_api: None,
            supports_vision: None,
            supports_reasoning_effort: None,
        };

        let err = ensure_pair_model_capabilities(&entry, &entry.gateway_handle)
            .expect_err("explicit tool denial must block Pair");
        assert!(err.to_string().contains("does not support tool calling"));
        entry.supports_tools = Some(true);
        ensure_pair_model_capabilities(&entry, &entry.gateway_handle)
            .expect("explicit tool support must allow Pair");
    }

    #[test]
    fn unknown_capability_metadata_lists_unknown_fields() {
        let entry = BifrostCatalogEntry {
            available: true,
            provider: "provider-a".to_string(),
            provider_model_id: "model-partial-metadata".to_string(),
            gateway_handle: "provider-a/model-partial-metadata".to_string(),
            display_name: None,
            context_window: 128_000,
            max_output_tokens: Some(4096),
            supports_tools: Some(true),
            supports_responses_api: None,
            supports_vision: None,
            supports_reasoning_effort: None,
        };

        assert_eq!(
            unknown_capability_metadata(&entry),
            vec!["supports_responses_api", "supports_vision"]
        );
    }

    #[test]
    fn client_waiting_livestream_projection_preserves_display_without_arguments() {
        let event = BearWireEvent::ephemeral(
            "client.waiting",
            json!({
                "tool_call": {
                    "id": "call-1",
                    "name": "fs_edit_file",
                    "title": "Edit the file",
                    "kind": "edit",
                    "display": { "summary": "Edit src/lib.rs" },
                    "arguments": { "path": "src/lib.rs", "secret": "do not send" }
                },
                "permission": {
                    "id": "perm-1",
                    "reason": "Writes a local file",
                    "title": "Allow edit",
                    "target": { "path": "src/lib.rs" }
                },
                "approval_required": true,
                "execution_target": "armature_local"
            }),
        );
        let obligation = turn_obligations::TurnObligationRow {
            id: Uuid::nil(),
            run_id: "run-1".to_string(),
            session_id: "session-1".to_string(),
            kind: "permission_decision".to_string(),
            expected_responder_action: "permission_decision".to_string(),
            tool_call_id: Some("call-1".to_string()),
            permission_id: Some("perm-1".to_string()),
            responder_ref_id: None,
            state: "waiting_for_client".to_string(),
            turn_step_id: None,
            request_payload: Value::Null,
            result_payload: None,
            created_at: OffsetDateTime::UNIX_EPOCH,
            updated_at: OffsetDateTime::UNIX_EPOCH,
            completed_at: None,
            lease_attempt_token_hash: None,
            claimed_at: None,
            lease_expires_at: None,
        };

        let projection =
            client_waiting_livestream_projection(&event, &obligation, "client.permission.result")
                .expect("canonical wait should project");

        assert_eq!(
            projection["tool_call"]["display"]["summary"],
            "Edit src/lib.rs"
        );
        assert_eq!(projection["permission"]["reason"], "Writes a local file");
        assert!(projection["tool_call"].get("arguments").is_none());
    }

    #[test]
    fn canonical_client_waiting_ids_require_nested_ids() {
        let event = BearWireEvent::ephemeral(
            "client.waiting",
            json!({
                "tool_call": { "id": "call-1", "name": "web_fetch" },
                "permission": { "id": "perm-1" }
            }),
        );

        assert_eq!(
            canonical_client_waiting_ids(&event),
            Some(("perm-1".to_string(), "call-1".to_string()))
        );
    }

    #[test]
    fn canonical_client_waiting_ids_require_string_ids() {
        let event = BearWireEvent::ephemeral(
            "client.waiting",
            json!({
                "tool_call": { "id": 1 },
                "permission": { "id": "perm-1" }
            }),
        );

        assert_eq!(canonical_client_waiting_ids(&event), None);
    }

    #[test]
    fn canonical_client_waiting_ids_reject_blank_ids() {
        let event = BearWireEvent::ephemeral(
            "client.waiting",
            json!({
                "tool_call": { "id": "  " },
                "permission": { "id": "perm-1" }
            }),
        );

        assert_eq!(canonical_client_waiting_ids(&event), None);
    }

    #[test]
    fn canonical_client_waiting_ids_reject_legacy_only_ids() {
        let event = BearWireEvent::ephemeral(
            "client.waiting",
            json!({
                "tool_call_id": "call-1",
                "permission_id": "perm-1"
            }),
        );

        assert_eq!(canonical_client_waiting_ids(&event), None);
    }

    #[test]
    fn bearwire_advertises_supported_direct_and_forwarded_mcp_tools() {
        let context = json!({
            "adapter": {
                "direct_tools": {
                    "fs_read_text_file": { "supported": true },
                    "fs_find_paths": { "supported": true },
                    "fs_edit_file": { "supported": true },
                    "terminal_run_command": { "supported": true },
                    "chrome_open": { "supported": true }
                }
            },
            "mcp": {
                "client_tools": [{
                    "name": "mcp__chrome_devtools_custom__click",
                    "description": "Click",
                    "parameters": { "type": "object", "properties": { "uid": { "type": "string" } }, "required": ["uid"] }
                }]
            }
        });
        let authority = den_core::client_tools::TurnAuthority::for_session_mode(
            den_core::BearStance::Pair,
            "write",
            None,
        );
        let descriptors =
            client_tool_descriptors_from_context_with_authority(Some(&context), &authority);
        let names = descriptor_names(&descriptors);
        assert!(names.contains(&"fs_read_text_file"));
        assert!(names.contains(&"fs_find_paths"));
        assert!(names.contains(&"fs_edit_file"));
        assert!(names.contains(&"run_command"));
        assert!(!names.contains(&"terminal_run_command"));
        assert!(names.contains(&"chrome_open"));
        assert!(names.contains(&"mcp__chrome_devtools_custom__click"));
        let read = descriptors
            .as_array()
            .unwrap()
            .iter()
            .find(|item| item["name"] == "fs_read_text_file")
            .unwrap();
        assert_eq!(read["parameters"]["required"], json!(["path"]));
        assert_eq!(read["parameters"]["additionalProperties"], false);
    }

    #[test]
    fn bearwire_context_compilation_includes_forwarded_mcp_tools() {
        let context = json!({
            "adapter": { "direct_tools": {} },
            "mcp": {
                "client_tools": [{
                    "name": "mcp__chrome_devtools_custom__click",
                    "description": "Click",
                    "parameters": { "type": "object", "properties": { "uid": { "type": "string" } }, "required": ["uid"] }
                }]
            }
        });
        let authority = den_core::client_tools::TurnAuthority::for_session_mode(
            den_core::BearStance::Pair,
            "ask",
            None,
        );
        let descriptors =
            client_tool_descriptors_from_context_with_authority(Some(&context), &authority);
        let names = descriptor_names(&descriptors);
        assert!(names.contains(&"mcp__chrome_devtools_custom__click"));
    }
}
