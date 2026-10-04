use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use den_core::{
    config::Config, execution_context::RuntimeContextLabel, resolve_agent_primary_request_profile,
    AgentPrimaryStep, DenError, ThinkingEffort,
};
use den_protocol::{RuntimeEventStream, RuntimeSemanticEvent, RuntimeStreamEvent};
use futures::{stream, Stream, StreamExt, TryStreamExt};
use sqlx::PgPool;
use tokio::time::timeout;

use crate::{
    agent_loop::{
        context::repair_tool_call_message_chain,
        evaluate_turn_context_budget,
        overflow_retry::compact_session_messages_for_overflow,
        record_context_budget_pressure_decision,
        session_store::{
            render_recently_discovered_capabilities, AgentLoopSession, AgentLoopSessionStore,
        },
        source_admission::require_ordinary_session_source,
    },
    context_budget::estimate_context_budget,
    llm::{
        bifrost_key_selection_error, byte_stream_with_idle_timeout,
        execution_fallback_model_handles, preferred_api_style_for_model, ChatCompletionRequest,
        LlmApiStyle, LlmClient,
    },
    native_runtime::{
        openai_byte_stream_to_event_stream_with_telemetry,
        responses_byte_stream_to_event_stream_with_telemetry, ObservedPromptTokensSink,
    },
    runtime_compaction::{den_error_indicates_context_overflow, CompactionMode},
};

/// Default max wait for Bifrost/upstream model to accept a streaming request and return
/// response headers. Idle providers can be cold after a long session pause; keep this
/// comfortably above the old 30s watchdog while still bounded.
const DEFAULT_NATIVE_LLM_HANDSHAKE_TIMEOUT: Duration = Duration::from_mins(2);
const MIN_NATIVE_LLM_HANDSHAKE_TIMEOUT_SECS: u64 = 15;
const MAX_NATIVE_LLM_HANDSHAKE_TIMEOUT_SECS: u64 = 900;
/// Max silence between upstream SSE byte chunks after the handshake.
const NATIVE_LLM_STREAM_IDLE_TIMEOUT: Duration = Duration::from_secs(30);

pub fn native_llm_handshake_timeout() -> Duration {
    native_llm_handshake_timeout_from_raw(
        std::env::var("BEARS_LLM_HANDSHAKE_TIMEOUT_SECS")
            .ok()
            .as_deref(),
    )
}

fn native_llm_handshake_timeout_from_raw(raw: Option<&str>) -> Duration {
    raw.and_then(|value| value.trim().parse::<u64>().ok())
        .map(|secs| {
            secs.clamp(
                MIN_NATIVE_LLM_HANDSHAKE_TIMEOUT_SECS,
                MAX_NATIVE_LLM_HANDSHAKE_TIMEOUT_SECS,
            )
        })
        .map(Duration::from_secs)
        .unwrap_or(DEFAULT_NATIVE_LLM_HANDSHAKE_TIMEOUT)
}

/// Dependencies for one-shot context-overflow recovery during an agent step.
#[derive(Clone)]
pub struct AgentStepOverflowContext {
    pub pool: PgPool,
    pub config: Arc<Config>,
    pub profile: RuntimeContextLabel,
    pub session_store: AgentLoopSessionStore,
}

/// Build a best-effort sink that folds provider-reported prompt-token usage
/// into the model registry's chars→tokens calibration (ADR-0047 §7). The
/// assembled char count mirrors the estimator's `to_body` measure so the
/// stored ratio corrects exactly what the estimator counts. Returns `None`
/// without a pool (no persistence context on this path).
fn observed_prompt_usage_sink(
    pool: Option<&PgPool>,
    request: &ChatCompletionRequest,
) -> Option<ObservedPromptTokensSink> {
    let pool = pool?.clone();
    let model = request.model.clone();
    let assembled_prompt_chars = request.to_body().to_string().chars().count() as u64;
    Some(Arc::new(move |observed_prompt_tokens: u64| {
        let pool = pool.clone();
        let model = model.clone();
        // Fire-and-forget: calibration must never fail or slow the turn.
        tokio::spawn(async move {
            match den_service::model_selection::record_model_token_calibration_sample(
                &pool,
                &model,
                assembled_prompt_chars,
                observed_prompt_tokens,
            )
            .await
            {
                Ok(stored) => {
                    tracing::debug!(
                        model = %model,
                        assembled_prompt_chars,
                        observed_prompt_tokens,
                        stored,
                        "recorded model token calibration sample"
                    );
                }
                Err(err) => {
                    tracing::debug!(
                        model = %model,
                        error = %err,
                        "model token calibration sample write failed"
                    );
                }
            }
        });
    }))
}

enum LazyAgentStepState {
    Init {
        fut: Pin<Box<dyn Future<Output = Result<RuntimeEventStream, DenError>> + Send>>,
    },
    Streaming(RuntimeEventStream),
}

struct LazyAgentStepStream {
    state: Option<LazyAgentStepState>,
}

impl LazyAgentStepStream {
    async fn connect_request_stream(
        llm: &LlmClient,
        request: &ChatCompletionRequest,
        session_key: &str,
        model: &str,
        api_style: LlmApiStyle,
        started: Instant,
        calibration_pool: Option<&PgPool>,
    ) -> Result<RuntimeEventStream, DenError> {
        let usage_sink = observed_prompt_usage_sink(calibration_pool, request);
        match api_style {
            LlmApiStyle::ChatCompletionsStream => {
                let byte_stream = llm.chat_completions_byte_stream(request).await?;
                Self::connect_byte_stream(
                    session_key.to_string(),
                    model.to_string(),
                    api_style,
                    started,
                    byte_stream,
                    request.telemetry.clone(),
                    usage_sink,
                )
            }
            LlmApiStyle::ResponsesStream => match llm.responses_byte_stream(request).await {
                Ok(byte_stream) => Self::connect_byte_stream(
                    session_key.to_string(),
                    model.to_string(),
                    api_style,
                    started,
                    byte_stream,
                    request.telemetry.clone(),
                    usage_sink,
                ),
                Err(err) if bifrost_key_selection_error(&err.to_string()) => {
                    tracing::warn!(
                        session_key = %session_key,
                        model = %model,
                        api_style = %api_style.as_str(),
                        error = %err,
                        "LLM responses stream hit Bifrost key-selection error; retrying via chat/completions stream"
                    );
                    let byte_stream = llm.chat_completions_byte_stream(request).await?;
                    Self::connect_byte_stream(
                        session_key.to_string(),
                        model.to_string(),
                        LlmApiStyle::ChatCompletionsStream,
                        started,
                        byte_stream,
                        request.telemetry.clone(),
                        usage_sink,
                    )
                }
                Err(err) => Err(err),
            },
        }
    }

    async fn retry_with_fallback_models(
        llm: &LlmClient,
        request: &ChatCompletionRequest,
        session_key: &str,
        api_style_override: Option<LlmApiStyle>,
        calibration_pool: Option<&PgPool>,
    ) -> Result<Option<RuntimeEventStream>, DenError> {
        let fallback_models = execution_fallback_model_handles(&request.model);
        if fallback_models.is_empty() {
            return Ok(None);
        }
        for fallback_model in fallback_models {
            let mut fallback_request = request.clone();
            fallback_request.model = (*fallback_model).to_string();
            let fallback_style =
                api_style_override.unwrap_or_else(|| preferred_api_style_for_model(fallback_model));
            tracing::warn!(
                session_key = %session_key,
                requested_model = %request.model,
                fallback_model,
                api_style = %fallback_style.as_str(),
                "retrying LLM stream with fallback model after Bifrost key-selection error"
            );
            match timeout(
                native_llm_handshake_timeout(),
                Self::connect_request_stream(
                    llm,
                    &fallback_request,
                    session_key,
                    fallback_model,
                    fallback_style,
                    Instant::now(),
                    calibration_pool,
                ),
            )
            .await
            {
                Ok(Ok(stream)) => {
                    tracing::info!(
                        session_key = %session_key,
                        requested_model = %request.model,
                        fallback_model,
                        api_style = %fallback_style.as_str(),
                        "LLM stream fallback model handshake succeeded"
                    );
                    return Ok(Some(stream));
                }
                Ok(Err(err)) if bifrost_key_selection_error(&err.to_string()) => {
                    tracing::warn!(
                        session_key = %session_key,
                        requested_model = %request.model,
                        fallback_model,
                        error = %err,
                        "fallback model also hit Bifrost key-selection error"
                    );
                }
                Ok(Err(err)) => return Err(err),
                Err(_) => {
                    tracing::warn!(
                        session_key = %session_key,
                        requested_model = %request.model,
                        fallback_model,
                        handshake_timeout_secs = native_llm_handshake_timeout().as_secs(),
                        "fallback model handshake timed out"
                    );
                }
            }
        }
        Ok(None)
    }

    fn new(
        llm: LlmClient,
        request: ChatCompletionRequest,
        session_key: String,
        api_style_override: Option<LlmApiStyle>,
        overflow: Option<AgentStepOverflowContext>,
    ) -> Self {
        let model = request.model.clone();
        let message_count = request.messages.len();
        let tool_count = request.tools.len();
        let fut = Self::handshake_future(
            llm,
            request,
            session_key,
            model,
            message_count,
            tool_count,
            api_style_override,
            overflow,
        );
        Self {
            state: Some(LazyAgentStepState::Init { fut }),
        }
    }

    fn handshake_future(
        llm: LlmClient,
        request: ChatCompletionRequest,
        session_key: String,
        model: String,
        message_count: usize,
        tool_count: usize,
        api_style_override: Option<LlmApiStyle>,
        overflow: Option<AgentStepOverflowContext>,
    ) -> Pin<Box<dyn Future<Output = Result<RuntimeEventStream, DenError>> + Send>> {
        Box::pin(async move {
            let started = Instant::now();
            let api_style =
                api_style_override.unwrap_or_else(|| preferred_api_style_for_model(&model));
            let handshake_timeout = native_llm_handshake_timeout();
            tracing::info!(
                session_key = %session_key,
                model = %model,
                api_style = %api_style.as_str(),
                message_count,
                tool_count,
                handshake_timeout_secs = handshake_timeout.as_secs(),
                handshake_timeout_source = "BEARS_LLM_HANDSHAKE_TIMEOUT_SECS",
                "LLM stream handshake starting"
            );
            let handshake = timeout(
                handshake_timeout,
                Self::connect_request_stream(
                    &llm,
                    &request,
                    &session_key,
                    &model,
                    api_style,
                    started,
                    overflow.as_ref().map(|ctx| &ctx.pool),
                ),
            )
            .await;
            match handshake {
                Err(_) => {
                    tracing::warn!(
                        session_key = %session_key,
                        model = %model,
                        duration_ms = started.elapsed().as_millis(),
                        handshake_timeout_secs = handshake_timeout.as_secs(),
                        api_style = %api_style.as_str(),
                        "LLM stream handshake timed out"
                    );
                    Err(DenError::System(format!(
                        "LLM stream handshake timed out after {}s (set BEARS_LLM_HANDSHAKE_TIMEOUT_SECS to tune cold/idle upstream startup tolerance)",
                        handshake_timeout.as_secs()
                    )))
                }
                Ok(Err(err)) => {
                    tracing::warn!(
                        session_key = %session_key,
                        model = %model,
                        duration_ms = started.elapsed().as_millis(),
                        error = %err,
                        api_style = %api_style.as_str(),
                        "LLM stream handshake failed"
                    );
                    if bifrost_key_selection_error(&err.to_string()) {
                        if let Some(stream) = Self::retry_with_fallback_models(
                            &llm,
                            &request,
                            &session_key,
                            api_style_override,
                            overflow.as_ref().map(|ctx| &ctx.pool),
                        )
                        .await?
                        {
                            return Ok(stream);
                        }
                    }
                    if let Some(ctx) = overflow {
                        if den_error_indicates_context_overflow(&err) {
                            return Self::recover_from_overflow_and_retry(
                                ctx,
                                llm,
                                request,
                                session_key,
                                model,
                                api_style,
                                started,
                            )
                            .await;
                        }
                    }
                    Err(err)
                }
                Ok(Ok(stream)) => Ok(stream),
            }
        })
    }

    fn connect_byte_stream(
        session_key: String,
        model: String,
        api_style: LlmApiStyle,
        started: Instant,
        byte_stream: impl Stream<Item = Result<bytes::Bytes, DenError>> + Send + Unpin + 'static,
        telemetry: Option<crate::llm::LlmRequestTelemetry>,
        usage_sink: Option<ObservedPromptTokensSink>,
    ) -> Result<RuntimeEventStream, DenError> {
        tracing::info!(
            session_key = %session_key,
            model = %model,
            api_style = %api_style.as_str(),
            duration_ms = started.elapsed().as_millis(),
            idle_timeout_secs = NATIVE_LLM_STREAM_IDLE_TIMEOUT.as_secs(),
            "LLM stream handshake connected"
        );
        let byte_stream =
            byte_stream_with_idle_timeout(byte_stream, NATIVE_LLM_STREAM_IDLE_TIMEOUT)
                .map_err(DenError::from);
        Ok(match api_style {
            LlmApiStyle::ChatCompletionsStream => {
                openai_byte_stream_to_event_stream_with_telemetry(
                    byte_stream,
                    telemetry,
                    usage_sink,
                )
            }
            LlmApiStyle::ResponsesStream => responses_byte_stream_to_event_stream_with_telemetry(
                byte_stream,
                telemetry,
                usage_sink,
            ),
        })
    }

    async fn recover_from_overflow_and_retry(
        ctx: AgentStepOverflowContext,
        llm: LlmClient,
        request: ChatCompletionRequest,
        session_key: String,
        model: String,
        api_style: LlmApiStyle,
        started: Instant,
    ) -> Result<RuntimeEventStream, DenError> {
        let session = ctx.session_store.get(&session_key).ok_or_else(|| {
            DenError::System("agent loop session not found for overflow recovery".into())
        })?;
        if session.overflow_retry_attempted {
            return Err(DenError::System(
                "LLM context overflow persists after emergency compaction retry".into(),
            ));
        }
        if CompactionMode::parse(&ctx.config.compaction_mode) != CompactionMode::Active {
            tracing::warn!(
                session_key = %session_key,
                compaction_mode = %ctx.config.compaction_mode,
                "context overflow detected but COMPACTION_MODE is not active; skipping retry"
            );
            return Err(DenError::System(
                "LLM context overflow; enable COMPACTION_MODE=active for emergency recovery".into(),
            ));
        }

        tracing::info!(
            session_key = %session_key,
            conversation_id = %session.conversation_id,
            profile = %ctx.profile.as_str(),
            "context overflow detected; running emergency compaction"
        );

        require_ordinary_session_source(&ctx.pool, (&session).into()).await?;
        let (new_messages, recovered) =
            compact_session_messages_for_overflow(&ctx.pool, &ctx.config, &session).await?;

        ctx.session_store.update(&session_key, |s| {
            s.messages.clone_from(&new_messages);
            s.overflow_retry_attempted = true;
            s.overflow_compaction_recovered = recovered;
        });

        if !recovered {
            return Err(DenError::System(
                "LLM context overflow; emergency compaction did not shrink prompt".into(),
            ));
        }

        let retry_request = ChatCompletionRequest {
            model: request.model.clone(),
            messages: repair_tool_call_message_chain(new_messages),
            tools: request.tools,
            stream: request.stream,
            tool_choice: request.tool_choice,
            temperature: request.temperature,
            max_tokens: request.max_tokens,
            thinking_effort: request.thinking_effort,
            telemetry: request.telemetry,
        };

        tracing::info!(
            session_key = %session_key,
            model = %model,
            api_style = %api_style.as_str(),
            message_count = retry_request.messages.len(),
            "retrying LLM stream after emergency compaction"
        );

        let handshake_timeout = native_llm_handshake_timeout();
        let usage_sink = observed_prompt_usage_sink(Some(&ctx.pool), &retry_request);
        let handshake = timeout(handshake_timeout, async {
            match api_style {
                LlmApiStyle::ChatCompletionsStream => {
                    let byte_stream = llm.chat_completions_byte_stream(&retry_request).await?;
                    Self::connect_byte_stream(
                        session_key.clone(),
                        model.clone(),
                        api_style,
                        started,
                        byte_stream,
                        retry_request.telemetry.clone(),
                        usage_sink,
                    )
                }
                LlmApiStyle::ResponsesStream => {
                    let byte_stream = llm.responses_byte_stream(&retry_request).await?;
                    Self::connect_byte_stream(
                        session_key.clone(),
                        model.clone(),
                        api_style,
                        started,
                        byte_stream,
                        retry_request.telemetry.clone(),
                        usage_sink,
                    )
                }
            }
        })
        .await;

        match handshake {
            Err(_) => Err(DenError::System(format!(
                "LLM stream retry timed out after {}s",
                handshake_timeout.as_secs()
            ))),
            Ok(Err(err)) => {
                tracing::warn!(
                    session_key = %session_key,
                    model = %model,
                    api_style = %api_style.as_str(),
                    duration_ms = started.elapsed().as_millis(),
                    error = %err,
                    "LLM stream retry failed after emergency compaction"
                );
                Err(err)
            }
            Ok(Ok(stream)) => Ok(stream),
        }
    }
}

impl Stream for LazyAgentStepStream {
    type Item = Result<RuntimeStreamEvent, DenError>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        loop {
            let Some(state) = self.state.take() else {
                return Poll::Ready(None);
            };
            match state {
                LazyAgentStepState::Init { mut fut } => match fut.as_mut().poll(cx) {
                    Poll::Ready(Ok(stream)) => {
                        self.state = Some(LazyAgentStepState::Streaming(stream));
                    }
                    Poll::Ready(Err(err)) => {
                        self.state = None;
                        return Poll::Ready(Some(Err(err)));
                    }
                    Poll::Pending => {
                        self.state = Some(LazyAgentStepState::Init { fut });
                        return Poll::Pending;
                    }
                },
                LazyAgentStepState::Streaming(mut stream) => {
                    match Pin::new(&mut stream).poll_next(cx) {
                        Poll::Ready(Some(item)) => {
                            self.state = Some(LazyAgentStepState::Streaming(stream));
                            return Poll::Ready(Some(item));
                        }
                        Poll::Ready(None) => return Poll::Ready(None),
                        Poll::Pending => {
                            self.state = Some(LazyAgentStepState::Streaming(stream));
                            return Poll::Pending;
                        }
                    }
                }
            }
        }
    }
}

/// Starts an agent step stream without blocking on the upstream LLM HTTP handshake.
///
/// The prompt handler must return SSE headers before Bifrost accepts the chat/completions
/// request; deferring the LLM call until the stream is polled avoids wedging client streams
/// that wait on `POST /prompt` with no timeout.
pub const RUNTIME_CHECKPOINT_TOOL_NAME: &str = "checkpoint";

fn api_compatible_thinking_effort(
    api_style: Option<LlmApiStyle>,
    has_function_tools: bool,
    thinking_effort: Option<ThinkingEffort>,
) -> Option<ThinkingEffort> {
    if api_style == Some(LlmApiStyle::ChatCompletionsStream) && has_function_tools {
        None
    } else {
        thinking_effort
    }
}

fn primary_request_profile(
    approved_model_ref: impl Into<String>,
    checkpoint_active: bool,
    supports_reasoning_effort: Option<bool>,
    configured_effort: Option<ThinkingEffort>,
) -> den_core::ModelRequestProfile {
    let step = if checkpoint_active {
        AgentPrimaryStep::Checkpoint
    } else {
        AgentPrimaryStep::OrdinaryTurn
    };
    resolve_agent_primary_request_profile(
        approved_model_ref,
        step,
        supports_reasoning_effort,
        configured_effort,
    )
}

fn primary_request_profile_for_session(
    session: &AgentLoopSession,
) -> den_core::ModelRequestProfile {
    let policy = session.agent_loop_control.profile.thinking;
    let configured_effort = policy
        .enabled
        .then_some(policy.checkpoint_turn_effort)
        .flatten();
    primary_request_profile(
        session.model_request_profile.approved_model_ref.clone(),
        session.checkpoint_state.last_checkpoint_reason.is_some(),
        session.model_request_profile.supports_reasoning_effort,
        configured_effort,
    )
}

fn compatible_thinking_effort_for_session(
    session: &AgentLoopSession,
    request_profile: &den_core::ModelRequestProfile,
    has_function_tools: bool,
) -> Option<ThinkingEffort> {
    let checkpoint_active = request_profile.agent_primary_step == AgentPrimaryStep::Checkpoint;
    let configured_effort = request_profile.thinking_effort;
    let compatible_effort =
        api_compatible_thinking_effort(session.api_style, has_function_tools, configured_effort);
    if checkpoint_active && configured_effort.is_some() && compatible_effort.is_none() {
        tracing::warn!(
            session_key = %session.session_key,
            model = %session.model,
            api_style = LlmApiStyle::ChatCompletionsStream.as_str(),
            "omitting checkpoint reasoning effort because Chat Completions with function tools is incompatible"
        );
    }
    compatible_effort
}

fn reasoning_effort_disposition_event(
    request_profile: &den_core::ModelRequestProfile,
    configured_effort: Option<ThinkingEffort>,
    compatible_effort: Option<ThinkingEffort>,
) -> Option<RuntimeStreamEvent> {
    if !matches!(
        request_profile.agent_primary_step,
        AgentPrimaryStep::Checkpoint | AgentPrimaryStep::PreRiskReview
    ) {
        return None;
    }
    let configured_effort = configured_effort?;
    let disposition = match (request_profile.supports_reasoning_effort, compatible_effort) {
        (Some(true), Some(_)) => "applied",
        (Some(false), _) => "skipped_unsupported",
        (None, _) => "skipped_unknown",
        (Some(true), None) => "skipped_api_incompatible",
    };
    Some(RuntimeStreamEvent::Semantic(
        RuntimeSemanticEvent::RunProgress {
            kind: "reasoning_effort_override".to_string(),
            text: None,
            phase: Some("agent_loop_control".to_string()),
            detail: Some(serde_json::json!({
                "disposition": disposition,
                "step": request_profile.agent_primary_step.as_str(),
                "catalog_support": request_profile.supports_reasoning_effort,
                "configured_effort": configured_effort.as_str(),
            })),
        },
    ))
}

fn checkpoint_tool_definition() -> crate::llm::LlmToolDefinition {
    crate::llm::LlmToolDefinition {
        name: RUNTIME_CHECKPOINT_TOOL_NAME.to_string(),
        description: Some(
            "Report a runtime checkpoint decision before continuing. Use this tool instead of replying with checkpoint JSON in assistant text. The fields are advisory/audit only; budgets and task tools remain authoritative.".to_string(),
        ),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "checkpoint_id": { "type": "string" },
                "active_objective": { "type": "string" },
                "summary": { "type": "string", "description": "Short prose synthesis of learned facts, uncertainty, and rationale. Keep it concise." },
                "learned": {
                    "type": "array",
                    "items": { "type": "string" }
                },
                "remaining_uncertainty": {
                    "type": "array",
                    "items": { "type": "string" }
                },
                "more_exploration_justified": { "type": "boolean" },
                "next_action": {
                    "type": "string",
                    "enum": [
                        "call_tool",
                        "edit",
                        "validate",
                        "update_task_list",
                        "sync_task_list",
                        "request_handoff",
                        "final_if_gate_allows",
                        "stop_blocked"
                    ]
                },
                "task_state_change_needed": {
                    "type": ["object", "null"],
                    "properties": {
                        "target_state": { "type": "string" },
                        "reason": { "type": "string" },
                        "evidence_refs": {
                            "type": "array",
                            "items": {
                                "type": "object",
                                "properties": {
                                    "kind": { "type": "string" },
                                    "id": { "type": "string" },
                                    "summary": { "type": ["string", "null"] }
                                },
                                "required": ["kind", "id"],
                                "additionalProperties": false
                            }
                        }
                    },
                    "required": ["target_state", "reason", "evidence_refs"],
                    "additionalProperties": false
                },
                "evidence_refs": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "kind": { "type": "string" },
                            "id": { "type": "string" },
                            "summary": { "type": ["string", "null"] }
                        },
                        "required": ["kind", "id"],
                        "additionalProperties": false
                    }
                },
                "confidence": {
                    "type": ["string", "null"],
                    "enum": ["low", "medium", "high", null]
                }
            },
            "required": ["checkpoint_id", "active_objective", "more_exploration_justified", "next_action"],
            "additionalProperties": false
        }),
    }
}

fn should_try_preflight_context_compaction(
    stop_requested: bool,
    recovered_this_step: bool,
    retry_already_attempted: bool,
    compaction_mode: &str,
) -> bool {
    stop_requested
        && !recovered_this_step
        && !retry_already_attempted
        && CompactionMode::parse(compaction_mode) == CompactionMode::Active
}

fn tools_with_checkpoint_tool(session: &AgentLoopSession) -> Vec<crate::llm::LlmToolDefinition> {
    checkpoint_tools(
        session.pending_checkpoint_request.is_some(),
        session.tools.clone(),
    )
}

fn checkpoint_tools(
    pending_checkpoint: bool,
    tools: Vec<crate::llm::LlmToolDefinition>,
) -> Vec<crate::llm::LlmToolDefinition> {
    if pending_checkpoint {
        // A pending checkpoint is an enforcement boundary, not an extra advisory tool.
        // Do not offer ordinary tools until the runtime-owned report has been handled.
        return vec![checkpoint_tool_definition()];
    }
    tools
}

fn resolved_request_profile_progress_event(
    request_profile: &den_core::ModelRequestProfile,
) -> RuntimeStreamEvent {
    RuntimeStreamEvent::Semantic(RuntimeSemanticEvent::RunProgress {
        kind: "model_request_profile_resolved".to_string(),
        text: None,
        phase: Some("model_routing".to_string()),
        detail: Some(serde_json::json!({
            "approved_model_ref": request_profile.approved_model_ref,
            "agent_primary_step": request_profile.agent_primary_step.as_str(),
            "supports_reasoning_effort": request_profile.supports_reasoning_effort,
            "thinking_effort": request_profile.thinking_effort.map(ThinkingEffort::as_str),
        })),
    })
}

fn resolved_control_progress_event(
    control: &crate::agent_loop::ResolvedAgentLoopControl,
) -> RuntimeStreamEvent {
    RuntimeStreamEvent::Semantic(RuntimeSemanticEvent::RunProgress {
        kind: "agent_loop_control_resolved".to_string(),
        text: Some(format!(
            "Applied `{}` agent-loop control policy.",
            control.level.as_str()
        )),
        phase: Some("agent_loop_control".to_string()),
        detail: Some(serde_json::json!({
            "control_level": control.level.as_str(),
            "source": control.source,
        })),
    })
}

pub async fn run_agent_step_stream(
    llm: &LlmClient,
    session: &AgentLoopSession,
    overflow: Option<AgentStepOverflowContext>,
) -> Result<RuntimeEventStream, DenError> {
    // Validate before preflight persistence or compaction (which can itself infer).
    // Pure budget-stop callers without a pool may still stop without any effect.
    if let Some(context) = overflow.as_ref() {
        require_ordinary_session_source(&context.pool, session.into()).await?;
    }
    let mut session = session.clone();
    let mut recovered_from_preflight_context_budget = false;
    let mut messages = repair_tool_call_message_chain(session.messages.clone());
    tracing::info!(
        session_key = %session.session_key,
        model = %session.model,
        message_count = messages.len(),
        tool_count = session.tools.len(),
        api_style_override = session.api_style.map(|style| style.as_str()),
        step = session.step,
        overflow_recovery = overflow.is_some(),
        "native agent step starting LLM stream"
    );
    let recently_discovered =
        render_recently_discovered_capabilities(&session.recently_discovered_capabilities);
    if !recently_discovered.is_empty() {
        let chars = recently_discovered.chars().count() as u32;
        messages.insert(
            0,
            crate::llm::ChatMessage {
                role: "system".to_string(),
                content: Some(recently_discovered),
                tool_call_id: None,
                name: None,
                tool_calls: None,
            },
        );
        session
            .budget_components
            .recently_discovered_capabilities_chars = chars;
    }
    let request_profile = primary_request_profile_for_session(&session);
    let configured_effort = session
        .agent_loop_control
        .profile
        .thinking
        .enabled
        .then_some(
            session
                .agent_loop_control
                .profile
                .thinking
                .checkpoint_turn_effort,
        )
        .flatten();
    let (request, budget, context_budget_evaluation) = loop {
        let tools = tools_with_checkpoint_tool(&session);
        let thinking_effort =
            compatible_thinking_effort_for_session(&session, &request_profile, !tools.is_empty());
        let request = ChatCompletionRequest {
            model: request_profile.approved_model_ref.clone(),
            messages,
            tools,
            stream: true,
            tool_choice: None,
            temperature: None,
            max_tokens: None,
            thinking_effort,
            telemetry: Some(session.llm_telemetry()),
        };
        let budget = estimate_context_budget(
            &request,
            &session.budget_components,
            session.model_context_window,
            session.model_max_output_tokens,
            session.model_token_calibration,
        );
        let context_budget_evaluation =
            evaluate_turn_context_budget(&session.turn_budget_state, budget);
        let budget = context_budget_evaluation
            .next_state
            .latest_context_budget
            .clone()
            .expect("context budget evaluation stores the latest report");
        if let Some(overflow) = overflow.as_ref() {
            overflow
                .session_store
                .update(&session.session_key, |stored| {
                    stored.latest_context_budget = Some(budget.clone());
                    stored.turn_budget_state = context_budget_evaluation.next_state.clone();
                });
            let _ = den_service::conversation::persistence::update_latest_context_budget(
                &overflow.pool,
                session.bear_id,
                &session.conversation_id,
                Some(&session.client_session_id),
                &budget,
            )
            .await;
            if budget.near_budget || budget.over_budget {
                if let Some(run_id) = session.run_id.as_deref() {
                    let _ = record_context_budget_pressure_decision(
                        &overflow.pool,
                        run_id,
                        None,
                        Some(session.objective_orientation.kind().to_string()),
                        &budget,
                    )
                    .await;
                }
            }
        }
        if overflow.as_ref().is_none_or(|overflow| {
            !should_try_preflight_context_compaction(
                context_budget_evaluation.stop_reason.is_some(),
                recovered_from_preflight_context_budget,
                session.overflow_retry_attempted,
                &overflow.config.compaction_mode,
            )
        }) {
            break (request, budget, context_budget_evaluation);
        }
        let Some(overflow) = overflow.as_ref() else {
            break (request, budget, context_budget_evaluation);
        };
        tracing::info!(
            session_key = %session.session_key,
            conversation_id = %session.conversation_id,
            profile = %overflow.profile.as_str(),
            "context budget exceeded before LLM call; running emergency compaction"
        );
        let (new_messages, recovered) =
            compact_session_messages_for_overflow(&overflow.pool, &overflow.config, &session)
                .await?;
        overflow
            .session_store
            .update(&session.session_key, |stored| {
                stored.messages.clone_from(&new_messages);
                stored.overflow_retry_attempted = true;
                stored.overflow_compaction_recovered = recovered;
            });
        if !recovered {
            break (request, budget, context_budget_evaluation);
        }
        session.messages = new_messages;
        session.overflow_retry_attempted = true;
        session.overflow_compaction_recovered = true;
        session.turn_budget_state = context_budget_evaluation.next_state.clone();
        messages = repair_tool_call_message_chain(session.messages.clone());
        recovered_from_preflight_context_budget = true;
    };
    if let Some(warning) = context_budget_evaluation.warning.as_ref() {
        tracing::warn!(
            session_key = %session.session_key,
            model = %budget.model,
            context_window = budget.context_window,
            estimated_input_tokens = budget.estimated_input_tokens,
            reserved_output_tokens = budget.reserved_output_tokens,
            estimated_total_tokens = budget.estimated_total_tokens,
            warning_code = warning.code,
            "context budget is near model limit"
        );
    }
    if let Some(reason) = context_budget_evaluation.stop_reason.as_ref() {
        return Err(DenError::ValidationError(reason.user_message()));
    }
    let context_budget_pressure_event = context_budget_evaluation.warning.as_ref().map(|warning| {
        RuntimeStreamEvent::Semantic(RuntimeSemanticEvent::RunProgress {
            kind: "context_budget_pressure".to_string(),
            text: Some(warning.model_message().to_string()),
            phase: Some("agent_loop_control".to_string()),
            detail: Some(serde_json::json!({
                "model": budget.model,
                "context_window": budget.context_window,
                "estimated_input_tokens": budget.estimated_input_tokens,
                "reserved_output_tokens": budget.reserved_output_tokens,
                "estimated_total_tokens": budget.estimated_total_tokens,
                "near_budget": budget.near_budget,
                "over_budget": budget.over_budget,
                "action": "prefer_checkpoint_before_more_context_growth",
            })),
        })
    });
    let reasoning_effort_disposition_event = reasoning_effort_disposition_event(
        &request_profile,
        configured_effort,
        request.thinking_effort,
    );
    let checkpoint_thinking_event = request.thinking_effort.map(|effort| {
        RuntimeStreamEvent::Semantic(RuntimeSemanticEvent::RunProgress {
            kind: "checkpoint_thinking_override_applied".to_string(),
            text: Some(format!(
                "Applied checkpoint thinking effort `{}` for this model call.",
                effort.as_str()
            )),
            phase: Some("agent_loop_control".to_string()),
            detail: Some(serde_json::json!({
                "effort": effort.as_str(),
                "reason": session.checkpoint_state.last_checkpoint_reason,
                "model": session.model,
                "control_level": session.agent_loop_control.level,
            })),
        })
    });
    let context = overflow.as_ref().ok_or_else(|| {
        DenError::Authorization("ordinary inference requires source validation dependencies".into())
    })?;
    let pool = context.pool.clone();
    let llm = llm.clone();
    let admission_session = session.clone();
    // Streaming is lazy: revalidate again at consumption, not just construction.
    let base_stream = Box::pin(
        stream::once(async move {
            require_ordinary_session_source(&pool, (&admission_session).into()).await?;
            Ok::<RuntimeEventStream, DenError>(Box::pin(LazyAgentStepStream::new(
                llm,
                request,
                admission_session.session_key,
                admission_session.api_style,
                overflow,
            )) as RuntimeEventStream)
        })
        .try_flatten(),
    ) as RuntimeEventStream;
    let prefix_events = [
        Some(resolved_request_profile_progress_event(&request_profile)),
        Some(resolved_control_progress_event(&session.agent_loop_control)),
        context_budget_pressure_event,
        reasoning_effort_disposition_event,
        checkpoint_thinking_event,
    ]
    .into_iter()
    .flatten()
    .map(Ok)
    .collect::<Vec<_>>();
    if prefix_events.is_empty() {
        Ok(base_stream)
    } else {
        Ok(Box::pin(stream::iter(prefix_events).chain(base_stream)) as RuntimeEventStream)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolved_request_profile_is_typed_and_uses_only_the_approved_model_reference() {
        let profile = den_core::ModelRequestProfile {
            approved_model_ref: "openai/approved".to_string(),
            agent_primary_step: AgentPrimaryStep::Checkpoint,
            supports_reasoning_effort: Some(true),
            thinking_effort: Some(ThinkingEffort::High),
        };

        let RuntimeStreamEvent::Semantic(RuntimeSemanticEvent::RunProgress {
            kind,
            phase,
            text,
            detail: Some(detail),
        }) = resolved_request_profile_progress_event(&profile)
        else {
            panic!("expected resolved request profile progress event");
        };

        assert_eq!(kind, "model_request_profile_resolved");
        assert_eq!(phase.as_deref(), Some("model_routing"));
        assert!(text.is_none());
        assert_eq!(detail["approved_model_ref"], "openai/approved");
        assert_eq!(detail["agent_primary_step"], "checkpoint");
        assert_eq!(detail["thinking_effort"], "high");
        assert!(detail.get("prompt").is_none());
        assert!(detail.get("transcript").is_none());
    }

    #[test]
    fn pending_checkpoint_exposes_only_the_runtime_checkpoint_tool() {
        let tools = checkpoint_tools(
            true,
            vec![crate::llm::LlmToolDefinition {
                name: "fs_read_text_file".to_string(),
                description: None,
                parameters: serde_json::json!({}),
            }],
        );

        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name, RUNTIME_CHECKPOINT_TOOL_NAME);
    }

    #[test]
    fn checkpoint_tool_schema_accepts_every_runtime_response_field() {
        let schema = checkpoint_tool_definition().parameters;
        let properties = schema["properties"].as_object().unwrap();
        let response = crate::agent_loop::RuntimeCheckpointResponse {
            checkpoint_id: "ckpt-1".to_string(),
            active_objective: "Fix checkpoint schema".to_string(),
            summary: Some("Mismatch located".to_string()),
            learned: vec!["The tool schema omitted typed response fields.".to_string()],
            remaining_uncertainty: Vec::new(),
            more_exploration_justified: false,
            next_action: crate::agent_loop::CheckpointNextAction::Edit,
            task_state_change_needed: None,
            evidence_refs: Vec::new(),
            confidence: Some(crate::agent_loop::CheckpointConfidence::High),
        };
        let production_payload = serde_json::to_value(&response).unwrap();
        for field in production_payload.as_object().unwrap().keys() {
            assert!(
                properties.contains_key(field),
                "runtime response field `{field}` is rejected by the checkpoint tool schema"
            );
        }

        serde_json::from_value::<crate::agent_loop::RuntimeCheckpointResponse>(production_payload)
            .unwrap();
    }

    #[test]
    fn resolved_control_progress_is_typed_and_transcript_free() {
        let control = crate::agent_loop::resolve_agent_loop_control(
            crate::agent_loop::AgentLoopControlResolutionInput {
                model_handle: Some("openai/test"),
                model_default: None,
                bear_override: None,
                task_escalation: None,
                origin: den_core::TurnExecutionOrigin::ChannelConversation,
                governance: den_core::Governance::AutonomousContinuation,
                objective_orientation: None,
                pre_risk: false,
            },
        )
        .expect("ordinary test origin");

        let RuntimeStreamEvent::Semantic(RuntimeSemanticEvent::RunProgress {
            kind,
            phase,
            detail: Some(detail),
            ..
        }) = resolved_control_progress_event(&control)
        else {
            panic!("expected resolved control progress event");
        };

        assert_eq!(kind, "agent_loop_control_resolved");
        assert_eq!(phase.as_deref(), Some("agent_loop_control"));
        assert_eq!(detail["control_level"], control.level.as_str());
        assert_eq!(
            detail["source"],
            serde_json::to_value(control.source).unwrap()
        );
        assert!(detail.get("prompt").is_none());
        assert!(detail.get("transcript").is_none());
    }

    #[test]
    fn shared_request_policy_limits_reasoning_effort_to_checkpoint_steps() {
        assert_eq!(
            api_compatible_thinking_effort(
                Some(LlmApiStyle::ResponsesStream),
                false,
                primary_request_profile(
                    "openai/gpt-5",
                    false,
                    Some(true),
                    Some(ThinkingEffort::High),
                )
                .thinking_effort,
            ),
            None
        );
        assert_eq!(
            api_compatible_thinking_effort(
                Some(LlmApiStyle::ResponsesStream),
                false,
                primary_request_profile(
                    "openai/gpt-5",
                    true,
                    Some(true),
                    Some(ThinkingEffort::High),
                )
                .thinking_effort,
            ),
            Some(ThinkingEffort::High)
        );
    }

    #[test]
    fn reasoning_effort_disposition_distinguishes_catalog_support() {
        for (support, expected) in [
            (Some(true), "applied"),
            (Some(false), "skipped_unsupported"),
            (None, "skipped_unknown"),
        ] {
            let profile = den_core::ModelRequestProfile {
                agent_primary_step: AgentPrimaryStep::Checkpoint,
                supports_reasoning_effort: support,
                thinking_effort: Some(ThinkingEffort::High),
                ..Default::default()
            };
            let event = reasoning_effort_disposition_event(
                &profile,
                Some(ThinkingEffort::High),
                support.and_then(|supported| supported.then_some(ThinkingEffort::High)),
            )
            .expect("configured checkpoint effort emits a disposition");
            let RuntimeStreamEvent::Semantic(RuntimeSemanticEvent::RunProgress { detail, .. }) =
                event
            else {
                panic!("expected reasoning effort progress event");
            };
            let detail = detail.expect("typed detail");
            assert_eq!(detail["disposition"], expected);
            assert_eq!(detail["step"], "checkpoint");
            assert!(detail.get("prompt").is_none());
            assert!(detail.get("transcript").is_none());
        }
    }

    #[test]
    fn reasoning_effort_is_omitted_for_chat_completions_with_function_tools() {
        let effort = Some(ThinkingEffort::Medium);

        assert_eq!(
            api_compatible_thinking_effort(Some(LlmApiStyle::ChatCompletionsStream), true, effort),
            None
        );
        assert_eq!(
            api_compatible_thinking_effort(Some(LlmApiStyle::ChatCompletionsStream), false, effort),
            effort
        );
        assert_eq!(
            api_compatible_thinking_effort(Some(LlmApiStyle::ResponsesStream), true, effort),
            effort
        );
        assert_eq!(api_compatible_thinking_effort(None, true, effort), effort);
    }

    #[test]
    fn preflight_context_compaction_only_runs_once_for_active_overflow_recovery() {
        assert!(should_try_preflight_context_compaction(
            true, false, false, "active"
        ));
        assert!(!should_try_preflight_context_compaction(
            false, false, false, "active"
        ));
        assert!(!should_try_preflight_context_compaction(
            true, true, false, "active"
        ));
        assert!(!should_try_preflight_context_compaction(
            true, false, true, "active"
        ));
        assert!(!should_try_preflight_context_compaction(
            true, false, false, "off"
        ));
    }

    #[test]
    fn native_llm_handshake_timeout_defaults_to_cold_start_tolerant_value() {
        assert_eq!(
            native_llm_handshake_timeout_from_raw(None),
            DEFAULT_NATIVE_LLM_HANDSHAKE_TIMEOUT
        );
    }

    #[test]
    fn chat_completions_with_tools_omits_incompatible_thinking_effort() {
        assert_eq!(
            api_compatible_thinking_effort(
                Some(LlmApiStyle::ChatCompletionsStream),
                true,
                Some(ThinkingEffort::Low),
            ),
            None
        );
        assert_eq!(
            api_compatible_thinking_effort(
                Some(LlmApiStyle::ChatCompletionsStream),
                false,
                Some(ThinkingEffort::Low),
            ),
            Some(ThinkingEffort::Low)
        );
        assert_eq!(
            api_compatible_thinking_effort(
                Some(LlmApiStyle::ResponsesStream),
                true,
                Some(ThinkingEffort::Low),
            ),
            Some(ThinkingEffort::Low)
        );
    }

    #[test]
    fn native_llm_handshake_timeout_parses_and_clamps_env_value() {
        assert_eq!(
            native_llm_handshake_timeout_from_raw(Some("90")),
            Duration::from_secs(90)
        );
        assert_eq!(
            native_llm_handshake_timeout_from_raw(Some("1")),
            Duration::from_secs(MIN_NATIVE_LLM_HANDSHAKE_TIMEOUT_SECS)
        );
        assert_eq!(
            native_llm_handshake_timeout_from_raw(Some("9999")),
            Duration::from_secs(MAX_NATIVE_LLM_HANDSHAKE_TIMEOUT_SECS)
        );
        assert_eq!(
            native_llm_handshake_timeout_from_raw(Some("not-a-number")),
            DEFAULT_NATIVE_LLM_HANDSHAKE_TIMEOUT
        );
    }
}
