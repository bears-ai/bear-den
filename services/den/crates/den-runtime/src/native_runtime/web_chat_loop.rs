//! Multi-step native web chat turn: executes Den server tools in-process and continues the loop.
use den_core::config::Config;

use std::collections::VecDeque;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use den_protocol::{
    RuntimeErrorCategory, RuntimeEventStream, RuntimeSemanticEvent, RuntimeStreamEvent,
    ToolCallFinishStatus,
};
use den_service::bears::BearProfile;
use futures::Stream;
use serde_json::Value;
use sqlx::PgPool;
use uuid::Uuid;

use crate::{
    agent_loop::{
        classify_tool_budget_class, evaluate_turn_budget, pending_tool_calls,
        provider_tool_supports_unilateral_execution, run_agent_step_stream,
        spawn_persist_web_chat_interrupted_turn, spawn_persist_web_chat_turn,
        tool_call_finished_event, tool_call_finished_event_for_content,
        tool_call_finished_event_for_incomplete, tool_result_content_indicates_error,
        tool_signature_from_call, AgentLoopSessionStore, AgentStepOverflowContext,
        NativeToolDispatchMode, SessionTrackingStream, ToolContinuationObservation,
    },
    llm::{ChatMessage, ChatToolCall, LlmClient},
    runtime_compaction::enqueue_compaction_after_turn,
};
use den_core::tools::arguments::DenToolChannelContext;
use den_core::tools::context::DenToolInvocationContext;
use den_core::tools::descriptor::builtin_den_tool_descriptor_for_provider_name;
use den_core::DenError;

use crate::native_runtime::RuntimeToolInvoker;

const WEB_CHAT_KEEPALIVE_INTERVAL: Duration = Duration::from_secs(15);
const WEB_CHAT_TURN_BUDGET: Duration = Duration::from_mins(2);

#[derive(Clone)]
pub struct NativeWebChatLoopRuntime {
    pub pool: PgPool,
    pub config: Arc<Config>,
    pub llm: LlmClient,
    pub session_key: String,
    pub bear_id: Uuid,
    pub bear_slug: String,
    pub chat_binding_id: String,
    pub user_id: i32,
    pub username: Option<String>,
    pub membership_role: Option<String>,
    pub conversation_id: String,
    pub session_id: String,
    pub request_id: String,
    pub session_store: AgentLoopSessionStore,
    pub tool_invoker: Arc<dyn RuntimeToolInvoker>,
}

type ToolExecFuture = Pin<Box<dyn Future<Output = Result<ChatMessage, DenError>> + Send>>;
type NextStepFuture = Pin<Box<dyn Future<Output = Result<RuntimeEventStream, DenError>> + Send>>;

enum LoopPhase {
    Streaming(Pin<Box<dyn Stream<Item = Result<RuntimeStreamEvent, DenError>> + Send>>),
    ExecutingTools {
        calls: Vec<ChatToolCall>,
        index: usize,
        results: Vec<ChatMessage>,
        active: Option<ToolExecFuture>,
    },
    StartingNextStep(NextStepFuture),
    Finished,
}

pub struct NativeWebChatLoopStream {
    runtime: NativeWebChatLoopRuntime,
    phase: LoopPhase,
    pending_out: VecDeque<RuntimeStreamEvent>,
    saw_turn_completed: bool,
    last_outbound: Instant,
    keepalive_sleep: Pin<Box<tokio::time::Sleep>>,
    turn_timeout_sleep: Pin<Box<tokio::time::Sleep>>,
    turn_start_message_len: usize,
    transcript_persisted: bool,
}

fn turn_timeout_event() -> RuntimeStreamEvent {
    RuntimeStreamEvent::Semantic(RuntimeSemanticEvent::TurnFailed {
        turn: None,
        category: RuntimeErrorCategory::Timeout,
        message: "This reply took too long and was stopped. Try a simpler question or retry."
            .to_string(),
    })
}

fn keepalive_event() -> RuntimeStreamEvent {
    status_event("Still working…")
}

fn status_event(text: impl Into<String>) -> RuntimeStreamEvent {
    RuntimeStreamEvent::Semantic(RuntimeSemanticEvent::RunProgress {
        kind: "status".to_string(),
        text: Some(text.into()),
        phase: None,
        detail: None,
    })
}

impl NativeWebChatLoopStream {
    pub fn new(
        runtime: NativeWebChatLoopRuntime,
        initial: RuntimeEventStream,
        turn_start_message_len: usize,
    ) -> Self {
        let wall_clock_limit = runtime
            .session_store
            .get(&runtime.session_key)
            .map(|session| Duration::from_millis(session.turn_budget.max_wall_clock_ms))
            .unwrap_or(WEB_CHAT_TURN_BUDGET);
        Self {
            runtime,
            phase: LoopPhase::Streaming(initial),
            pending_out: VecDeque::from([status_event("Thinking…")]),
            saw_turn_completed: false,
            last_outbound: Instant::now(),
            keepalive_sleep: Box::pin(tokio::time::sleep(WEB_CHAT_KEEPALIVE_INTERVAL)),
            turn_timeout_sleep: Box::pin(tokio::time::sleep(wall_clock_limit)),
            turn_start_message_len,
            transcript_persisted: false,
        }
    }

    fn persist_completed_turn(&mut self) {
        if self.transcript_persisted {
            return;
        }
        self.transcript_persisted = true;
        let Some(session) = self.runtime.session_store.get(&self.runtime.session_key) else {
            return;
        };
        spawn_persist_web_chat_turn(
            self.runtime.pool.clone(),
            self.runtime.bear_id,
            self.runtime.user_id,
            self.runtime.conversation_id.clone(),
            self.runtime.session_id.clone(),
            self.runtime.request_id.clone(),
            &session.messages,
            self.turn_start_message_len,
        );
        let pool = self.runtime.pool.clone();
        let config = self.runtime.config.clone();
        let bear_id = self.runtime.bear_id;
        let conversation_id = self.runtime.conversation_id.clone();
        tokio::spawn(async move {
            enqueue_compaction_after_turn(
                &pool,
                &config,
                bear_id,
                &conversation_id,
                BearProfile::Chat,
            )
            .await;
        });
    }

    fn persist_interrupted_turn(&mut self, reason: &str) {
        if self.transcript_persisted {
            return;
        }
        self.transcript_persisted = true;
        let Some(session) = self.runtime.session_store.get(&self.runtime.session_key) else {
            return;
        };
        spawn_persist_web_chat_interrupted_turn(
            self.runtime.pool.clone(),
            self.runtime.bear_id,
            self.runtime.user_id,
            self.runtime.conversation_id.clone(),
            self.runtime.session_id.clone(),
            self.runtime.request_id.clone(),
            &session.messages,
            self.turn_start_message_len,
            reason,
        );
    }

    fn emit_incomplete_tool_outcomes(&mut self, calls: &[ChatToolCall], reason: &str) {
        for call in calls {
            self.pending_out.push_back(RuntimeStreamEvent::Semantic(
                tool_call_finished_event_for_incomplete(
                    call.id.clone(),
                    &call.function.name,
                    reason,
                ),
            ));
        }
    }

    fn touch_outbound(&mut self) {
        self.last_outbound = Instant::now();
        self.keepalive_sleep
            .as_mut()
            .reset(tokio::time::Instant::now() + WEB_CHAT_KEEPALIVE_INTERVAL);
    }

    fn maybe_keepalive(&mut self) -> Option<RuntimeStreamEvent> {
        if matches!(self.phase, LoopPhase::Finished) {
            return None;
        }
        if self.last_outbound.elapsed() < WEB_CHAT_KEEPALIVE_INTERVAL {
            return None;
        }
        self.touch_outbound();
        Some(keepalive_event())
    }

    pub(crate) fn wrap_step_stream(
        runtime: &NativeWebChatLoopRuntime,
        stream: RuntimeEventStream,
        session: &crate::agent_loop::AgentLoopSession,
    ) -> RuntimeEventStream {
        Box::pin(SessionTrackingStream::new(
            stream,
            session,
            runtime.session_store.clone(),
            runtime.pool.clone(),
            runtime.bear_id,
            runtime.bear_slug.clone(),
            Some(runtime.user_id),
            runtime.conversation_id.clone(),
            runtime.session_id.clone(),
            Some(runtime.request_id.clone()),
            runtime.config.clone(),
            BearProfile::Chat,
            NativeToolDispatchMode::ServerSideInProcess,
        ))
    }

    fn begin_tool_execution(&mut self, calls: Vec<ChatToolCall>) {
        self.pending_out.push_back(status_event("Running tools…"));
        self.phase = LoopPhase::ExecutingTools {
            calls,
            index: 0,
            results: Vec::new(),
            active: None,
        };
    }

    fn begin_next_step(&mut self) {
        let runtime = self.runtime.clone();
        self.pending_out.push_back(status_event("Continuing…"));
        self.phase = LoopPhase::StartingNextStep(Box::pin(async move {
            let session = runtime
                .session_store
                .get(&runtime.session_key)
                .ok_or_else(|| DenError::System("native web chat session not found".to_string()))?;
            let overflow = AgentStepOverflowContext {
                pool: runtime.pool.clone(),
                config: runtime.config.clone(),
                profile: BearProfile::Chat,
                session_store: runtime.session_store.clone(),
            };
            let raw = run_agent_step_stream(&runtime.llm, &session, Some(overflow)).await?;
            Ok(NativeWebChatLoopStream::wrap_step_stream(
                &runtime, raw, &session,
            ))
        }));
    }

    fn evaluate_completed_tool_batch(
        &mut self,
        calls: &[ChatToolCall],
        completed: &[ChatMessage],
    ) -> Option<RuntimeStreamEvent> {
        let Some(prior_session) = self.runtime.session_store.get(&self.runtime.session_key) else {
            return Some(RuntimeStreamEvent::Semantic(
                RuntimeSemanticEvent::TurnFailed {
                    turn: None,
                    category: RuntimeErrorCategory::Internal,
                    message: "native web chat session not found".to_string(),
                },
            ));
        };
        let observations = calls
            .iter()
            .zip(completed.iter())
            .map(|(call, message)| ToolContinuationObservation {
                tool_name: call.function.name.clone(),
                signature: tool_signature_from_call(call),
                class: classify_tool_budget_class(&call.function.name),
                failed: tool_result_content_indicates_error(message.content.as_deref()),
                grounding_probe_signal: None,
            })
            .collect::<Vec<_>>();
        let evaluation = evaluate_turn_budget(
            prior_session.turn_budget,
            prior_session.step,
            prior_session
                .turn_budget_state
                .started_at
                .elapsed()
                .as_millis() as u64,
            &prior_session.turn_budget_state,
            &observations,
        );
        self.runtime
            .session_store
            .update(&self.runtime.session_key, |session| {
                session.messages.extend(completed.iter().cloned());
                session.turn_budget_state = evaluation.next_state.clone();
                if let Some(warning) = evaluation.warning.as_ref() {
                    if session.messages.last().is_some_and(|message| {
                        message.role == "system"
                            && message.content.as_deref() == Some(warning.model_message())
                    }) {
                        return;
                    }
                    if session.messages.last().is_some_and(|message| {
                        message.role == "system"
                            && message
                                .content
                                .as_deref()
                                .is_some_and(|content| content.starts_with("Budget advisory:"))
                    }) {
                        session.messages.pop();
                    }
                    session.messages.push(ChatMessage {
                        role: "system".to_string(),
                        content: Some(warning.model_message().to_string()),
                        tool_call_id: None,
                        name: None,
                        tool_calls: None,
                    });
                }
            });
        let reason = evaluation.stop_reason?;
        self.persist_interrupted_turn(reason.persistence_reason());
        Some(RuntimeStreamEvent::Semantic(
            RuntimeSemanticEvent::TurnFailed {
                turn: None,
                category: RuntimeErrorCategory::Internal,
                message: reason.user_message(),
            },
        ))
    }

    fn poll_tool_execution(
        &mut self,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<RuntimeStreamEvent, DenError>>> {
        let LoopPhase::ExecutingTools {
            calls,
            index,
            results,
            active,
        } = &mut self.phase
        else {
            return Poll::Pending;
        };

        if *index >= calls.len() {
            let completed = std::mem::take(results);
            let completed_calls = calls.clone();
            if let Some(stop_event) =
                self.evaluate_completed_tool_batch(&completed_calls, &completed)
            {
                self.phase = LoopPhase::Finished;
                self.pending_out.push_back(stop_event);
                return Poll::Ready(self.pending_out.pop_front().map(Ok));
            }
            self.begin_next_step();
            return Poll::Ready(self.pending_out.pop_front().map(Ok));
        }

        if active.is_none() {
            let call = calls[*index].clone();
            let runtime = self.runtime.clone();
            *active = Some(Box::pin(async move {
                execute_one_web_chat_den_tool(&runtime, call).await
            }));
        }

        let Some(fut) = active.as_mut() else {
            return Poll::Pending;
        };
        match fut.as_mut().poll(cx) {
            Poll::Ready(Ok(message)) => {
                let call = calls[*index].clone();
                let content = message.content.as_deref();
                let finished = tool_call_finished_event_for_content(&call, content);
                self.pending_out
                    .push_back(RuntimeStreamEvent::Semantic(finished));
                results.push(message);
                *index += 1;
                *active = None;
                cx.waker().wake_by_ref();
                Poll::Pending
            }
            Poll::Ready(Err(error)) => {
                let call = calls[*index].clone();
                let summary = format!("{} failed: {error}", call.function.name);
                self.pending_out
                    .push_back(RuntimeStreamEvent::Semantic(tool_call_finished_event(
                        &call,
                        ToolCallFinishStatus::Error,
                        summary.clone(),
                        Some(summary),
                    )));
                results.push(ChatMessage {
                    role: "tool".to_string(),
                    content: Some(format!("error: {error}")),
                    tool_call_id: Some(call.id),
                    name: Some(call.function.name),
                    tool_calls: None,
                });
                *index += 1;
                *active = None;
                cx.waker().wake_by_ref();
                Poll::Pending
            }
            Poll::Pending => Poll::Pending,
        }
    }
}

impl Stream for NativeWebChatLoopStream {
    type Item = Result<RuntimeStreamEvent, DenError>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        if let Some(event) = self.pending_out.pop_front() {
            self.touch_outbound();
            return Poll::Ready(Some(Ok(event)));
        }
        let turn_timed_out = self.turn_timeout_sleep.as_mut().poll(cx).is_ready();
        if !turn_timed_out {
            let _ = self.keepalive_sleep.as_mut().poll(cx);
            if let Some(event) = self.maybe_keepalive() {
                return Poll::Ready(Some(Ok(event)));
            }
        }
        if turn_timed_out {
            if let LoopPhase::ExecutingTools { calls, index, .. } = &self.phase {
                let pending = calls[*index..].to_vec();
                self.emit_incomplete_tool_outcomes(&pending, "turn_timeout");
            } else if let Some(session) = self.runtime.session_store.get(&self.runtime.session_key)
            {
                let pending = pending_tool_calls(&session.messages);
                self.emit_incomplete_tool_outcomes(&pending, "turn_timeout");
            }
            self.persist_interrupted_turn("turn_timeout");
            self.pending_out.push_back(turn_timeout_event());
            self.phase = LoopPhase::Finished;
            self.touch_outbound();
            return Poll::Ready(self.pending_out.pop_front().map(Ok));
        }

        loop {
            match &mut self.phase {
                LoopPhase::Finished => return Poll::Ready(None),
                LoopPhase::ExecutingTools { .. } => return self.poll_tool_execution(cx),
                LoopPhase::StartingNextStep(future) => match future.as_mut().poll(cx) {
                    Poll::Ready(Ok(stream)) => {
                        self.saw_turn_completed = false;
                        self.phase = LoopPhase::Streaming(stream);
                    }
                    Poll::Ready(Err(error)) => {
                        self.persist_interrupted_turn("next_step_failed");
                        self.phase = LoopPhase::Finished;
                        return Poll::Ready(Some(Err(error)));
                    }
                    Poll::Pending => return Poll::Pending,
                },
                LoopPhase::Streaming(stream) => match stream.as_mut().poll_next(cx) {
                    Poll::Ready(Some(Ok(
                        event @ RuntimeStreamEvent::Semantic(RuntimeSemanticEvent::TurnCompleted {
                            ..
                        }),
                    ))) => {
                        self.saw_turn_completed = true;
                        self.touch_outbound();
                        return Poll::Ready(Some(Ok(event)));
                    }
                    Poll::Ready(Some(Ok(RuntimeStreamEvent::Semantic(
                        event @ RuntimeSemanticEvent::TurnFailed { .. },
                    )))) => {
                        self.persist_interrupted_turn("turn_failed");
                        self.phase = LoopPhase::Finished;
                        self.touch_outbound();
                        return Poll::Ready(Some(Ok(RuntimeStreamEvent::Semantic(event))));
                    }
                    Poll::Ready(Some(Ok(item))) => {
                        self.touch_outbound();
                        return Poll::Ready(Some(Ok(item)));
                    }
                    Poll::Ready(Some(Err(error))) => {
                        self.persist_interrupted_turn("stream_error");
                        self.phase = LoopPhase::Finished;
                        return Poll::Ready(Some(Err(error)));
                    }
                    Poll::Ready(None) => {
                        let session =
                            match self.runtime.session_store.get(&self.runtime.session_key) {
                                Some(session) => session,
                                None => {
                                    self.phase = LoopPhase::Finished;
                                    return Poll::Ready(Some(Err(DenError::System(
                                        "native web chat session not found".to_string(),
                                    ))));
                                }
                            };
                        let pending = pending_tool_calls(&session.messages);
                        if pending.is_empty() {
                            if !self.saw_turn_completed {
                                self.pending_out.push_back(RuntimeStreamEvent::Semantic(
                                    RuntimeSemanticEvent::TurnCompleted { turn: None },
                                ));
                            }
                            self.persist_completed_turn();
                            self.phase = LoopPhase::Finished;
                            if let Some(event) = self.pending_out.pop_front() {
                                return Poll::Ready(Some(Ok(event)));
                            }
                            return Poll::Ready(None);
                        }
                        if session.step >= session.turn_budget.emergency_hard_steps {
                            self.persist_interrupted_turn("emergency_hard_step_limit");
                            self.phase = LoopPhase::Finished;
                            return Poll::Ready(Some(Ok(RuntimeStreamEvent::Semantic(
                                RuntimeSemanticEvent::TurnFailed {
                                    turn: None,
                                    category: RuntimeErrorCategory::Internal,
                                    message: format!(
                                        "native web chat reached emergency hard step limit (step={}/emergency_hard_steps={})",
                                        session.step, session.turn_budget.emergency_hard_steps
                                    ),
                                },
                            ))));
                        }
                        self.begin_tool_execution(pending);
                    }
                    Poll::Pending => return Poll::Pending,
                },
            }
        }
    }
}

async fn execute_one_web_chat_den_tool(
    runtime: &NativeWebChatLoopRuntime,
    call: ChatToolCall,
) -> Result<ChatMessage, DenError> {
    let provider_name = call.function.name.clone();
    let canonical = builtin_den_tool_descriptor_for_provider_name(&provider_name)
        .map(|descriptor| descriptor.name.to_string())
        .unwrap_or_else(|| provider_name.clone());
    let args: Value = serde_json::from_str(&call.function.arguments)
        .unwrap_or_else(|_| Value::Object(Default::default()));
    let session_snapshot = runtime.session_store.get(&runtime.session_key);
    let content = if !provider_tool_supports_unilateral_execution(&provider_name) {
        serde_json::json!({
            "ok": false,
            "error": "This tool requires interactive approval, which web chat does not support yet."
        })
        .to_string()
    } else if builtin_den_tool_descriptor_for_provider_name(&provider_name).is_none() {
        format!("unsupported server tool: {provider_name}")
    } else {
        let tool_context = DenToolInvocationContext {
            bear_id: runtime.bear_id,
            bear_slug: runtime.bear_slug.clone(),
            binding_id: runtime.chat_binding_id.clone(),
            profile: Some(BearProfile::Chat),
            user_id: runtime.user_id,
            username: runtime.username.clone(),
            membership_role: runtime.membership_role.clone(),
            conversation_id: runtime.conversation_id.clone(),
            session_id: runtime.session_id.clone(),
            work_run_id: None,
            client_session_id: None,
            conversation_selection: Some(runtime.conversation_id.clone()),
            runtime_target: Some(runtime.conversation_id.clone()),
            workspace_roots: Vec::new(),
            session_capabilities: session_snapshot
                .as_ref()
                .map(|session| session.session_capabilities.clone())
                .unwrap_or_default(),
            session_policy: None,
            activity: session_snapshot
                .as_ref()
                .and_then(|session| session.cached_activity_plan_projection.as_ref())
                .and_then(|plan| serde_json::to_value(plan).ok()),
            runtime: session_snapshot
                .as_ref()
                .map(|session| session.session_info_runtime_snapshot()),
            context_budget: session_snapshot.as_ref().and_then(|session| {
                session
                    .latest_context_budget
                    .as_ref()
                    .and_then(|budget| serde_json::to_value(budget).ok())
            }),
            projected_memory: session_snapshot
                .as_ref()
                .and_then(|session| session.latest_projected_memory.clone()),
            recalled_memory: session_snapshot
                .as_ref()
                .and_then(|session| session.latest_recalled_memory.clone()),
            request_id: Some(runtime.request_id.clone()),
            channel: DenToolChannelContext {
                family: Some("browser_chat".to_string()),
                client: Some("den_web".to_string()),
                protocol: Some("den_chat".to_string()),
            },
        };
        match runtime
            .tool_invoker
            .invoke(crate::native_runtime::RuntimeToolInvocation {
                tool_name: canonical,
                arguments: args,
                context: tool_context,
                effective_policy: den_core::EffectivePolicy::compile(
                    den_core::TrustProfile::Chat,
                    den_core::Governance::Interactive,
                    den_core::ArmatureAvailability::Absent,
                ),
                origin_run_id: None,
                tool_call_id: crate::turn_ids::ToolCallId::new(call.id.clone())?,
            })
            .await
        {
            Ok(value) => serde_json::to_string_pretty(&value).unwrap_or_else(|_| value.to_string()),
            Err(error) => format!("error: {error}"),
        }
    };
    Ok(ChatMessage {
        role: "tool".to_string(),
        content: Some(content),
        tool_call_id: Some(call.id),
        name: Some(provider_name),
        tool_calls: None,
    })
}
