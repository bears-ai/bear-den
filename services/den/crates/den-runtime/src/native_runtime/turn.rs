use den_core::config::Config;
use den_core::tools::{
    arguments::DenToolChannelContext,
    capability_catalog::SessionCapabilityDescriptor,
    context::DenToolInvocationContext,
    descriptor::builtin_den_tool_descriptor_for_provider_name,
    result_compaction::{compact_client_tool_result, ClientToolResultInput, ToolResultStatus},
};
use std::sync::{Arc, LazyLock};
#[cfg(feature = "test-fixtures")]
use std::{
    collections::{HashMap, VecDeque},
    sync::Mutex,
};

use den_memory::MemoryStoreManager;
use den_protocol::{
    ContinueTurnRequest, RoleRuntimeBinding, RuntimeContinuation, RuntimeConversationBackend,
    RuntimeConversationRef, RuntimeEventStream, RuntimeHistoryPage, RuntimeHistoryRecord,
    RuntimeSemanticEvent, RuntimeStreamContinuation, RuntimeStreamEvent, RuntimeToolResultStatus,
    StartTurnRequest,
};
use den_service::{
    bears::{
        prompt_fragments::{render_turn_fragment, repository_prompt_fragment_registry},
        BearProfile,
    },
    conversation::{
        events::{
            canonical_persistence_context, canonical_persistence_enabled_for_conversation,
            persist_canonical_conversation_record, CanonicalConversationRecord,
            CanonicalToolRequestRecord, CanonicalToolResultRecord, ConversationEventProvenance,
        },
        persistence as conversation_persistence,
    },
};
use futures::{stream, StreamExt};
use sqlx::PgPool;
use uuid::Uuid;

use super::web_chat_loop::{NativeWebChatLoopRuntime, NativeWebChatLoopStream};

use crate::{
    agent_loop::{
        agent_loop_control_profile_fingerprint, agent_loop_session_key,
        assemble_native_turn_for_bear, classify_tool_budget_class, evaluate_checkpoint_trigger,
        evaluate_turn_budget, latest_grounding_probe_signal_for_tool_call,
        objective_orientation_allowed_for_stance, projected_memory_session_diagnostic,
        provider_tool_is_den_web_fetch, recalled_memory_session_diagnostic,
        record_approval_decision, record_checkpoint_request,
        record_grounding_probe_result_decision, resolve_agent_loop_control, run_agent_step_stream,
        tool_result_content_indicates_error, tool_signature_from_call,
        AgentLoopControlResolutionInput, AgentLoopSession, AgentLoopSessionStore,
        AgentStepOverflowContext, AssembleTurnContext, CheckpointArtifactInput, CheckpointField,
        CheckpointReplayPolicy, CheckpointTaskContext, CheckpointTrigger, CheckpointVisibility,
        DocketExecutionOrientation, GroundingProbeFinding, GroundingProbeResultInput,
        GroundingProbeSignalKind, NativeToolDispatchMode, ObjectiveOrientation, OrientationTaskRef,
        RuntimeCheckpointRequest, SessionTrackingStream, ToolBudgetClass,
        ToolContinuationObservation, TurnBudgetStopReason, TurnBudgetWarning,
    },
    llm::{ChatMessage, ChatToolCall, LlmClient},
    native_runtime::{
        profile::NativeCapabilityProfile,
        tools::{is_work_tool_provider_name, merge_den_and_client_tools},
    },
    turn_runner::{
        materialize_runtime_conversation_if_needed, RunRecoveryDisposition, TurnContinueRequest,
        TurnStartRequest,
    },
    turn_runs,
};
use den_core::DenError;
use den_docket::TaskListProjection;
use den_service::conversation::persistence::PersistedTranscriptRecord;

static SESSION_STORE: LazyLock<AgentLoopSessionStore> =
    LazyLock::new(AgentLoopSessionStore::default);

#[cfg(feature = "test-fixtures")]
/// A deterministic source for one native-runtime invocation in downstream
/// integration tests. `Pending` deliberately keeps the stream open after prior
/// scripts have demonstrated continuation, so the test—not a synthetic EOF—
/// decides when to settle the attempt.
#[cfg(feature = "test-fixtures")]
pub enum ScriptedRuntimeStream {
    Events(Vec<RuntimeStreamEvent>),
    Pending,
}

#[cfg(feature = "test-fixtures")]
static SCRIPTED_RUNTIME_STREAMS: LazyLock<Mutex<HashMap<String, VecDeque<ScriptedRuntimeStream>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

#[cfg(feature = "test-fixtures")]
static NEXT_SCRIPTED_RUNTIME_STREAMS: LazyLock<
    Mutex<HashMap<String, VecDeque<ScriptedRuntimeStream>>>,
> = LazyLock::new(|| Mutex::new(HashMap::new()));

#[cfg(feature = "test-fixtures")]
static SCRIPTED_RUNTIME_INVOCATIONS: LazyLock<Mutex<HashMap<String, usize>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Installs typed runtime event scripts for one Pair run. Start and
/// continuation calls consume that run's scripts in FIFO order. Intended only
/// for deterministic downstream tests.
#[cfg(feature = "test-fixtures")]
pub fn set_scripted_runtime_streams_for_run(
    run_id: impl Into<String>,
    scripts: Vec<ScriptedRuntimeStream>,
) {
    SCRIPTED_RUNTIME_STREAMS
        .lock()
        .expect("scripted runtime stream lock poisoned")
        .insert(run_id.into(), scripts.into());
}

/// Installs scripts for the next native runtime run on `session_id`. The first
/// request for that session claims them and binds the remaining sequence to its
/// generated run id, so continuations remain run-correlated. This is only for
/// focused integration tests whose run id is created inside BearWire.
#[cfg(feature = "test-fixtures")]
pub fn set_next_scripted_runtime_streams(
    session_id: impl Into<String>,
    scripts: Vec<ScriptedRuntimeStream>,
) {
    NEXT_SCRIPTED_RUNTIME_STREAMS
        .lock()
        .expect("next scripted runtime stream lock poisoned")
        .insert(session_id.into(), scripts.into());
}

/// Returns how many scripted streams have been constructed for a focused run.
#[cfg(feature = "test-fixtures")]
pub fn scripted_runtime_invocation_count(run_id: &str) -> usize {
    SCRIPTED_RUNTIME_INVOCATIONS
        .lock()
        .expect("scripted runtime invocation lock poisoned")
        .get(run_id)
        .copied()
        .unwrap_or_default()
}

#[cfg(feature = "test-fixtures")]
fn scripted_runtime_stream(
    run_id: Option<&str>,
    session_id: Option<&str>,
) -> Option<RuntimeEventStream> {
    let run_id = run_id?;
    let mut scripts = SCRIPTED_RUNTIME_STREAMS
        .lock()
        .expect("scripted runtime stream lock poisoned");
    let script = if let Some(queue) = scripts.get_mut(run_id) {
        let script = queue.pop_front()?;
        if queue.is_empty() {
            scripts.remove(run_id);
        }
        script
    } else {
        let session_id = session_id?;
        let mut next = NEXT_SCRIPTED_RUNTIME_STREAMS
            .lock()
            .expect("next scripted runtime stream lock poisoned");
        let mut queue = next.remove(session_id)?;
        let script = queue.pop_front()?;
        if !queue.is_empty() {
            scripts.insert(run_id.to_string(), queue);
        }
        script
    };
    *SCRIPTED_RUNTIME_INVOCATIONS
        .lock()
        .expect("scripted runtime invocation lock poisoned")
        .entry(run_id.to_string())
        .or_default() += 1;
    match script {
        ScriptedRuntimeStream::Events(events) => {
            Some(Box::pin(stream::iter(events.into_iter().map(Ok))) as RuntimeEventStream)
        }
        // ponytail: A permanently pending scripted stream is test-only. It
        // models a live provider after a continuation without using sleeps;
        // production streams remain provider-backed.
        ScriptedRuntimeStream::Pending => Some(Box::pin(stream::pending()) as RuntimeEventStream),
    }
}

#[cfg(not(feature = "test-fixtures"))]
fn scripted_runtime_stream(
    _run_id: Option<&str>,
    _session_id: Option<&str>,
) -> Option<RuntimeEventStream> {
    None
}

fn session_capabilities_from_client_tools(
    client_tools: Option<&serde_json::Value>,
    client_session_id: &str,
    workspace_roots: Option<&[String]>,
) -> Vec<SessionCapabilityDescriptor> {
    let surface = workspace_roots
        .filter(|roots| !roots.is_empty())
        .map(|roots| format!("workspace roots: {}", roots.join(", ")))
        .unwrap_or_else(|| "current client session".to_string());
    client_tools
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|tool| {
            let name = tool.get("name")?.as_str()?.trim();
            if name.is_empty() {
                return None;
            }
            let is_mcp = name.starts_with("mcp__");
            Some(SessionCapabilityDescriptor {
                instance_id: format!("{client_session_id}:{name}"),
                name: name.to_string(),
                summary: tool
                    .get("description")
                    .and_then(serde_json::Value::as_str)
                    .filter(|description| !description.trim().is_empty())
                    .unwrap_or("Connected client provider tool")
                    .to_string(),
                kind: "tool".to_string(),
                provider: if is_mcp { "mcp" } else { "armature" }.to_string(),
                execution_locality: if is_mcp {
                    "connected MCP provider".to_string()
                } else {
                    "armature-local client workspace".to_string()
                },
                authority: "current client connection and turn policy".to_string(),
                surface: surface.clone(),
                availability: "available".to_string(),
                tags: vec![
                    "session-bound".to_string(),
                    if is_mcp { "mcp" } else { "armature" }.to_string(),
                ],
            })
        })
        .collect()
}

fn render_host_context_for_model(prompt_context: Option<&serde_json::Value>) -> Option<String> {
    let host_context = prompt_context?.get("host_context")?;
    if let Some(body_text) = host_context
        .get("body_text")
        .and_then(serde_json::Value::as_str)
    {
        let body_text = body_text.trim();
        if !body_text.is_empty() {
            let kind = host_context
                .get("kind")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("referenced_resources");
            let delivery = host_context
                .get("delivery")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("reference_only");
            let persistence = host_context
                .get("persistence")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("not_human_message");
            return Some(format!(
                "<host_context kind=\"{kind}\" delivery=\"{delivery}\" persistence=\"{persistence}\">\n{body_text}\n</host_context>"
            ));
        }
    }

    let resources = host_context
        .get("resources")
        .and_then(serde_json::Value::as_array)?;
    if resources.is_empty() {
        return None;
    }

    let kind = host_context
        .get("kind")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("referenced_resources");
    let delivery = host_context
        .get("delivery")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("reference_only");
    let persistence = host_context
        .get("persistence")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("not_human_message");
    let mut lines = vec![
        format!(
            "<host_context kind=\"{kind}\" delivery=\"{delivery}\" persistence=\"{persistence}\">"
        ),
        "The ACP client referenced these resources. They are not human-authored instructions."
            .to_string(),
        "Use available file/content tools for authoritative contents before quoting, editing, or relying on them.".to_string(),
        String::new(),
        "Resources:".to_string(),
    ];
    for (index, resource) in resources.iter().enumerate() {
        let label = resource
            .get("label")
            .or_else(|| resource.get("name"))
            .or_else(|| resource.get("uri"))
            .and_then(serde_json::Value::as_str)
            .unwrap_or("unnamed resource");
        lines.push(format!("- resource {}: {}", index + 1, label));
        if let Some(uri) = resource.get("uri").and_then(serde_json::Value::as_str) {
            lines.push(format!("  uri: {uri}"));
        }
        if let Some(mime_type) = resource
            .get("mime_type")
            .and_then(serde_json::Value::as_str)
        {
            lines.push(format!("  mime_type: {mime_type}"));
        }
        if let Some(text_bytes) = resource
            .get("embedded_text_bytes")
            .and_then(serde_json::Value::as_u64)
        {
            lines.push(format!(
                "  embedded_text_bytes: {text_bytes} (body omitted; use tools for contents)"
            ));
        }
    }
    if let Some(omitted) = host_context
        .get("omitted_references")
        .and_then(serde_json::Value::as_u64)
    {
        if omitted > 0 {
            lines.push(format!("- omitted_references: {omitted}"));
        }
    }
    lines.push("</host_context>".to_string());
    Some(lines.join("\n"))
}

fn prompt_for_user_history(prompt: &str, prompt_context: Option<&serde_json::Value>) -> String {
    let mut text = prompt.trim().to_string();
    let resources = prompt_context
        .and_then(|context| context.pointer("/host_context/resources"))
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten();
    for resource in resources {
        let label = resource
            .get("label")
            .or_else(|| resource.get("name"))
            .or_else(|| resource.get("uri"))
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|label| !label.is_empty())
            .unwrap_or("unnamed resource");
        if !text.is_empty() {
            text.push_str("\n\n");
        }
        text.push_str("[Referenced resource: ");
        text.push_str(label);
        text.push(']');
    }
    text
}

fn prompt_for_model(prompt: &str, prompt_context: Option<&serde_json::Value>) -> String {
    let Some(host_context) = render_host_context_for_model(prompt_context) else {
        return prompt.to_string();
    };
    format!("{host_context}\n\n<user_message>\n{prompt}\n</user_message>")
}

async fn work_execution_orientation(
    pool: &PgPool,
    bear_id: Uuid,
    work_run_id: Option<Uuid>,
) -> Result<ObjectiveOrientation, DenError> {
    let Some(work_run_id) = work_run_id else {
        return Ok(ObjectiveOrientation::Freeform {
            policy: Default::default(),
        });
    };
    let row = sqlx::query_as::<_, (Uuid, Uuid)>(
        "SELECT attempts.task_id, tasks.job_id \
         FROM docket_execution_attempts attempts \
         JOIN bear_tasks tasks ON tasks.id = attempts.task_id \
         WHERE attempts.bear_id = $1 AND attempts.binding_kind = 'work_assignment' \
           AND attempts.binding_id = $2::text AND attempts.state = 'running'",
    )
    .bind(bear_id)
    .bind(work_run_id)
    .fetch_optional(pool)
    .await?;
    Ok(row.map_or_else(
        || ObjectiveOrientation::Freeform {
            policy: Default::default(),
        },
        |(task_id, job_id)| work_execution_orientation_from_ids(task_id, job_id),
    ))
}

fn work_execution_orientation_from_ids(task_id: Uuid, job_id: Uuid) -> ObjectiveOrientation {
    ObjectiveOrientation::DocketExecution {
        job: DocketExecutionOrientation {
            job_id: job_id.to_string(),
            active_task_ref: Some(OrientationTaskRef::DocketTask {
                job_id: Some(job_id.to_string()),
                task_id: task_id.to_string(),
                title: None,
            }),
            mutable: false,
        },
    }
}

pub fn native_client_run_exists(
    conversation_id: &str,
    client_session_id: &str,
    run_id: &str,
) -> bool {
    let key = agent_loop_session_key(conversation_id, client_session_id, run_id);
    SESSION_STORE.get(&key).is_some()
}

pub fn remove_native_client_run(client_session_id: &str, run_id: &str) {
    SESSION_STORE.remove_client_run(client_session_id, run_id);
}

pub fn update_native_client_session_cached_activity_plan_projection(
    conversation_id: &str,
    client_session_id: &str,
    cached_activity_plan_projection: Option<TaskListProjection>,
) {
    SESSION_STORE.update_client_sessions(conversation_id, client_session_id, |session| {
        session.cached_activity_plan_projection = cached_activity_plan_projection.clone();
    });
}

async fn persisted_tool_call_exists(
    pool: &PgPool,
    bear_id: Uuid,
    conversation_id: &str,
    tool_call_id: &str,
) -> Result<bool, DenError> {
    let exists = sqlx::query_scalar!(
        r#"
        SELECT EXISTS (
            SELECT 1
            FROM conversation_messages
            WHERE conversation_id = (
                SELECT id FROM conversations
                WHERE external_conversation_id = $1 AND bear_id = $2
                LIMIT 1
            )
              AND message_type = 'tool_call'
              AND tool_call_id = $3
        ) AS "exists!"
        "#,
        conversation_id,
        bear_id,
        tool_call_id,
    )
    .fetch_one(pool)
    .await
    .map_err(|err| DenError::Database(format!("check persisted tool_call: {err}")))?;
    Ok(exists)
}

fn native_tool_result_diagnostics(
    status: RuntimeToolResultStatus,
    status_label: ToolResultStatus,
    tool_call_id: &str,
) -> serde_json::Value {
    serde_json::json!({
        "component": "den.native_runtime",
        "phase": if matches!(status, RuntimeToolResultStatus::Error) {
            "client_tool_result_failed"
        } else {
            "client_tool_result_recorded"
        },
        "tool_call_id": tool_call_id,
        "tool_status": status_label.as_str(),
        "failure_class": if matches!(status, RuntimeToolResultStatus::Error) {
            Some("adapter_tool_error")
        } else {
            None
        },
    })
}

pub async fn record_native_client_tool_result(
    pool: &PgPool,
    conversation_id: &str,
    client_session_id: &str,
    request_id: &str,
    run_id: Option<&str>,
    tool_call_id: &str,
    approval_request_id: Option<&str>,
    status: RuntimeToolResultStatus,
    content: String,
) -> Result<(), DenError> {
    if let Some(approval_request_id) = approval_request_id {
        let approve = matches!(status, RuntimeToolResultStatus::Ok);
        record_approval_decision(
            pool,
            approval_request_id,
            approve,
            Some(if approve {
                "tool_result_delivered"
            } else {
                "tool_result_failed"
            }),
        )
        .await?;
    }

    let tool_message = ChatMessage {
        role: "tool".to_string(),
        content: Some(content),
        tool_call_id: Some(tool_call_id.to_string()),
        name: None,
        tool_calls: None,
    };
    let run_id = run_id.ok_or_else(|| {
        DenError::ValidationError("native client tool result requires run_id".to_string())
    })?;
    let session_key = agent_loop_session_key(conversation_id, client_session_id, run_id);
    SESSION_STORE.update(&session_key, |session| {
        session.request_id = Some(request_id.to_string());
        session.run_id = Some(run_id.to_string());
        session.messages.push(tool_message.clone());
    });
    let Some(session) = SESSION_STORE.get(&session_key) else {
        return Err(DenError::System(
            "native agent loop session not found".to_string(),
        ));
    };
    let matching_call = session.find_pending_tool_call(tool_call_id);
    let tool_name = matching_call
        .as_ref()
        .map(|call| call.function.name.clone());
    if canonical_persistence_enabled_for_conversation(conversation_id) {
        let persistence_context = canonical_persistence_context(
            pool.clone(),
            session.bear_id,
            session.user_id,
            conversation_id.to_string(),
            Some(client_session_id.to_string()),
            Some(request_id.to_string()),
            client_session_id.to_string(),
            false,
        );
        let provenance = ConversationEventProvenance::client_session(client_session_id.to_string());
        if let Some(call) = matching_call.as_ref() {
            if !persisted_tool_call_exists(pool, session.bear_id, conversation_id, tool_call_id)
                .await?
            {
                let args = serde_json::from_str(&call.function.arguments)
                    .unwrap_or_else(|_| serde_json::Value::String(call.function.arguments.clone()));
                persist_canonical_conversation_record(
                    &persistence_context,
                    &CanonicalConversationRecord::tool_request(
                        CanonicalToolRequestRecord::new(
                            call.function.name.clone(),
                            call.id.clone(),
                            request_id.to_string(),
                            approval_request_id.map(str::to_string),
                            args,
                            approval_request_id.is_some(),
                            if approval_request_id.is_some() {
                                Some("native runtime policy".to_string())
                            } else {
                                None
                            },
                            "native_runtime_backfill",
                        ),
                        &provenance,
                    ),
                )
                .await?;
            }
        }

        let status_label = match status {
            RuntimeToolResultStatus::Ok => ToolResultStatus::Ok,
            RuntimeToolResultStatus::Timeout => ToolResultStatus::Timeout,
            RuntimeToolResultStatus::Error => ToolResultStatus::Error,
        };
        // Preserve an adapter-reported failure in the structured error field as
        // well as the display content. Diagnostics can then report the actual
        // failure without inferring it from an opaque lifecycle phase.
        let error = if matches!(status, RuntimeToolResultStatus::Error) {
            tool_message
                .content
                .clone()
                .map(serde_json::Value::String)
                .unwrap_or(serde_json::Value::Null)
        } else {
            serde_json::Value::Null
        };
        let compacted = compact_client_tool_result(&ClientToolResultInput::new(
            tool_call_id.to_string(),
            tool_name.clone(),
            status_label,
            tool_message.content.clone(),
            serde_json::Value::Null,
            error,
        ));
        persist_canonical_conversation_record(
            &persistence_context,
            &CanonicalConversationRecord::tool_result(
                CanonicalToolResultRecord::new(
                    tool_name,
                    tool_call_id.to_string(),
                    approval_request_id.map(str::to_string),
                    status_label,
                    compacted
                        .payload
                        .get("content")
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_string),
                    compacted.payload.clone(),
                    native_tool_result_diagnostics(status, status_label, tool_call_id),
                    Some(request_id.to_string()),
                ),
                &provenance,
            ),
        )
        .await?;
    }
    Ok(())
}

fn overflow_context(
    pool: PgPool,
    config: Arc<Config>,
    profile: BearProfile,
) -> AgentStepOverflowContext {
    AgentStepOverflowContext {
        pool,
        config,
        profile,
        session_store: SESSION_STORE.clone(),
    }
}

/// Shared dependencies for internal native profile turns (no full `DenState` required).
pub struct NativeRuntimeDeps<'a> {
    pub pool: &'a PgPool,
    pub config: &'a Config,
    pub stores: &'a MemoryStoreManager,
}

fn bear_id_from_native_binding(binding: &RoleRuntimeBinding) -> Option<Uuid> {
    let rest = binding.binding_id.strip_prefix("den-native:")?;
    let bear_id_str = rest.split(':').next()?;
    Uuid::parse_str(bear_id_str).ok()
}

pub struct NativeRuntimeConversationBackend {
    pool: Option<PgPool>,
}

impl Default for NativeRuntimeConversationBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl NativeRuntimeConversationBackend {
    pub fn new() -> Self {
        Self { pool: None }
    }

    pub fn with_pool(pool: PgPool) -> Self {
        Self { pool: Some(pool) }
    }
}

#[allow(async_fn_in_trait)]
impl RuntimeConversationBackend for NativeRuntimeConversationBackend {
    async fn create_conversation(
        &self,
        binding: &RoleRuntimeBinding,
    ) -> Result<RuntimeConversationRef, DenError> {
        let id = format!("den-conv-{}", Uuid::new_v4().simple());
        if let Some(pool) = &self.pool {
            if let Some(bear_id) = bear_id_from_native_binding(binding) {
                conversation_persistence::ensure_conversation_for_external_id(
                    pool, bear_id, None, &id, None, None,
                )
                .await?;
            }
        }
        Ok(RuntimeConversationRef { id })
    }

    async fn verify_conversation_belongs_to_binding(
        &self,
        binding: &RoleRuntimeBinding,
        conversation_id: &str,
    ) -> Result<(), DenError> {
        let Some(pool) = &self.pool else {
            return Ok(());
        };
        let Some(bear_id) = bear_id_from_native_binding(binding) else {
            return Ok(());
        };
        let found = conversation_persistence::get_conversation_for_external_id(
            pool,
            bear_id,
            conversation_id,
        )
        .await?;
        if found.is_none() {
            return Err(DenError::ValidationError(format!(
                "conversation {conversation_id} does not belong to bear"
            )));
        }
        Ok(())
    }

    async fn load_history(
        &self,
        binding: &RoleRuntimeBinding,
        conversation: &RuntimeConversationRef,
    ) -> Result<RuntimeHistoryPage, DenError> {
        let Some(pool) = &self.pool else {
            return Ok(RuntimeHistoryPage {
                records: Vec::new(),
                raw_payload: None,
            });
        };
        let Some(bear_id) = bear_id_from_native_binding(binding) else {
            return Ok(RuntimeHistoryPage {
                records: Vec::new(),
                raw_payload: None,
            });
        };
        let Some(canonical) = conversation_persistence::get_conversation_for_external_id(
            pool,
            bear_id,
            &conversation.id,
        )
        .await?
        else {
            return Ok(RuntimeHistoryPage {
                records: Vec::new(),
                raw_payload: None,
            });
        };
        let rows = conversation_persistence::list_projected_messages_page(
            pool,
            canonical.id,
            None,
            100,
            conversation_persistence::ConversationHistoryProjection::ModelTranscript,
        )
        .await?;
        let mut records = Vec::new();
        for row in rows.into_iter().rev() {
            match row.to_model_transcript_record() {
                Some(PersistedTranscriptRecord::Message(message)) => {
                    records.push(RuntimeHistoryRecord::Message {
                        message_id: message.message_id,
                        role: message.role,
                        content: message.content,
                        created_at: Some(message.created_at.to_string()),
                    });
                }
                Some(PersistedTranscriptRecord::ToolCall {
                    tool_call_id,
                    tool_name,
                    arguments,
                    created_at,
                    ..
                }) => {
                    records.push(RuntimeHistoryRecord::ToolCall {
                        message_id: Some(tool_call_id.clone()),
                        tool_call_id,
                        tool_name,
                        arguments,
                        created_at: Some(created_at.to_string()),
                    });
                }
                Some(PersistedTranscriptRecord::ToolResult {
                    tool_call_id,
                    tool_name,
                    status,
                    content,
                    structured_content,
                    created_at,
                    ..
                }) => {
                    records.push(RuntimeHistoryRecord::ToolResult {
                        message_id: tool_call_id.clone(),
                        tool_call_id,
                        tool_name,
                        status,
                        content,
                        structured_content,
                        created_at: Some(created_at.to_string()),
                    });
                }
                None => {}
            }
        }
        Ok(RuntimeHistoryPage {
            records,
            raw_payload: None,
        })
    }
}

fn wrap_session_stream(
    stream: RuntimeEventStream,
    session: &AgentLoopSession,
    config: Arc<Config>,
    profile: BearProfile,
    pool: PgPool,
    bear_id: Uuid,
    user_id: Option<i32>,
    conversation_id: &str,
    client_session_id: &str,
    request_id: Option<String>,
) -> RuntimeEventStream {
    Box::pin(SessionTrackingStream::new(
        stream,
        session,
        SESSION_STORE.clone(),
        pool,
        bear_id,
        session.bear_slug.clone(),
        user_id,
        conversation_id.to_string(),
        client_session_id.to_string(),
        request_id,
        config,
        profile,
        NativeToolDispatchMode::DeferToClient,
    ))
}

struct BuildSessionInput<'a> {
    profile: NativeCapabilityProfile,
    bear_id: Uuid,
    conversation_id: &'a str,
    client_session_id: &'a str,
    human_message: Option<&'a str>,
    runtime_context: Option<&'a str>,
    session_id: Option<&'a str>,
    workspace_roots: Option<&'a [String]>,
    runtime_target: Option<&'a str>,
    conversation_selection: Option<&'a str>,
    user_id: Option<i32>,
    client_context: Option<&'a serde_json::Value>,
    client_tools: Option<&'a serde_json::Value>,
    request_id: Option<Uuid>,
    run_id: Option<&'a str>,
    checkpoint_audit_context: Option<den_protocol::CheckpointAuditContext>,
    work_run_id: Option<Uuid>,
    stream_tokens: bool,
    api_style: Option<crate::llm::LlmApiStyle>,
    supports_reasoning_effort: Option<bool>,
    technical_budget_recovery_start_payload: Option<serde_json::Value>,
    tool_messages: Vec<ChatMessage>,
}

fn native_turn_control_profile(
    agent_loop_control: crate::agent_loop::ResolvedAgentLoopControl,
    tool_budget_multiplier: f64,
) -> crate::agent_loop::ResolvedAgentLoopControl {
    let resolved_profile = agent_loop_control
        .profile
        .with_tool_budget_multiplier(tool_budget_multiplier);
    crate::agent_loop::ResolvedAgentLoopControl {
        profile: resolved_profile,
        ..agent_loop_control
    }
}

async fn build_session(
    deps: &NativeRuntimeDeps<'_>,
    input: BuildSessionInput<'_>,
) -> Result<AgentLoopSession, DenError> {
    let BuildSessionInput {
        profile,
        bear_id,
        conversation_id,
        client_session_id,
        human_message,
        runtime_context,
        session_id,
        workspace_roots,
        runtime_target,
        conversation_selection,
        user_id,
        client_context,
        client_tools,
        request_id,
        run_id,
        checkpoint_audit_context,
        work_run_id,
        stream_tokens,
        api_style,
        supports_reasoning_effort,
        technical_budget_recovery_start_payload,
        tool_messages,
    } = input;
    let llm = LlmClient::new(deps.config);
    let bear = den_service::bears::db::get_bear(deps.pool, bear_id)
        .await?
        .ok_or_else(|| DenError::NotFound("bear not found".to_string()))?;
    let include_prompt_memory = profile.include_prompt_memory && runtime_context.is_none();
    let assembled = assemble_native_turn_for_bear(
        AssembleTurnContext {
            pool: deps.pool,
            config: deps.config,
            stores: deps.stores,
            bear_id,
            profile: profile.profile,
            conversation_id,
            turn_runtime_context: runtime_context,
            human_message,
            tool_messages: &tool_messages,
            session_id,
            workspace_roots,
            runtime_target,
            conversation_selection,
            user_id,
            client_context,
            include_prompt_memory,
            key_memory_cache: None,
            native_runtime: true,
        },
        &bear,
    )
    .await?;
    let key_memory_projection_cache_key = assembled
        .key_memory_projection
        .as_ref()
        .map(|projection| projection.cache_key.clone());
    let latest_projected_memory = assembled
        .key_memory_projection
        .as_ref()
        .map(projected_memory_session_diagnostic);
    let latest_recalled_memory = Some(recalled_memory_session_diagnostic(
        assembled.recall_diagnostic.as_ref(),
    ));
    let messages = assembled.messages;
    let budget_components = assembled.budget_components;
    let cached_activity_plan_projection = assembled.cached_activity_plan_projection;
    let objective_orientation = if profile.profile == BearProfile::Work {
        work_execution_orientation(deps.pool, bear_id, work_run_id).await?
    } else {
        assembled.objective_orientation
    };
    if profile.profile == BearProfile::Work {
        if !bear.work_enabled {
            return Err(DenError::ValidationError(
                "Work stance is not enabled for this Bear".to_string(),
            ));
        }
        if !objective_orientation_allowed_for_stance(profile.profile, &objective_orientation) {
            return Err(DenError::ValidationError(
                "Work stance requires a focused Docket Job before execution can continue"
                    .to_string(),
            ));
        }
    }
    let may_define_task = match &objective_orientation {
        ObjectiveOrientation::Freeform { policy } => policy.may_define_task,
        ObjectiveOrientation::Oriented { .. } | ObjectiveOrientation::DocketExecution { .. } => {
            true
        }
    };
    let tools = merge_den_and_client_tools(
        deps.config,
        profile.profile,
        bear.work_enabled,
        bear.cabinet_enabled,
        may_define_task,
        client_tools,
        human_message,
    )?;
    let execution_id = run_id
        .map(str::to_string)
        .or_else(|| request_id.map(|id| id.to_string()))
        .unwrap_or_else(|| format!("unbound-{}", Uuid::new_v4().simple()));
    let session_key = agent_loop_session_key(conversation_id, client_session_id, &execution_id);
    let conversation_model = match conversation_persistence::get_conversation_for_external_id(
        deps.pool,
        bear.id,
        conversation_id,
    )
    .await?
    {
        Some(conversation) => {
            conversation_persistence::resolve_conversation_selected_model(
                deps.pool,
                conversation.id,
            )
            .await?
        }
        None => None,
    };
    let model = if let Some(model) = conversation_model {
        model
    } else {
        den_service::bears::db::resolve_model_for_profile(
            deps.pool,
            &bear,
            profile.profile,
            llm.default_model(),
        )
        .await?
    };
    let model = llm.resolve_model(Some(&model));
    let mut tool_budget_multiplier = bear.default_tool_budget_multiplier.unwrap_or(1.0);
    if let Some(model_multiplier) = deps.config.model_tool_budget_multipliers.get(&model) {
        tool_budget_multiplier *= *model_multiplier;
    }
    let model_option =
        den_service::model_selection::resolve_model_option(deps.pool, &model).await?;
    // Best-effort: calibration only improves the approximate estimator, so a
    // failed lookup must never fail the turn (ADR-0047 §7).
    let model_token_calibration =
        den_service::model_selection::load_model_token_calibration(deps.pool, &model)
            .await
            .unwrap_or_else(|err| {
                tracing::debug!(
                    model = %model,
                    error = %err,
                    "model token calibration lookup failed; using chars/4 fallback"
                );
                None
            });
    let bifrost_virtual_key = den_service::bears::db::bifrost_virtual_key_for_inference(
        deps.pool,
        bear.id,
        &deps.config.den_secret_encryption_key,
    )
    .await?;
    let (bear_loop_control_override, stance_loop_control_override) =
        den_service::bears::db::agent_loop_control_overrides_for_profile(
            deps.pool,
            bear.id,
            profile.profile,
        )
        .await?;
    let agent_loop_control = resolve_agent_loop_control(AgentLoopControlResolutionInput {
        model_handle: Some(&model),
        model_default: None,
        bear_override: bear_loop_control_override,
        stance_override: stance_loop_control_override,
        task_escalation: None,
        stance: Some(profile.profile),
        objective_orientation: Some(&objective_orientation),
        pre_risk: false,
    });
    let agent_loop_control =
        native_turn_control_profile(agent_loop_control, tool_budget_multiplier);
    // `work.checkout` binds the Armature session before the native loop starts.
    // Carry that authoritative binding into hosted-tool invocations instead of
    // deriving authority from a model-provided path or identifier.
    let work_run_id = if profile.profile == BearProfile::Work {
        den_docket::work_runs::get_live_work_run_by_session(deps.pool, client_session_id)
            .await?
            .map(|run| run.id)
    } else {
        None
    };
    let effective_policy = den_core::EffectivePolicy::compile(
        profile.profile,
        den_core::Governance::Interactive,
        if client_tools.is_some() {
            den_core::ArmatureAvailability::Connected
        } else {
            den_core::ArmatureAvailability::Absent
        },
    );
    let run_id = ensure_session_task_run(
        deps.pool,
        &effective_policy.capabilities,
        &cached_activity_plan_projection,
        client_session_id,
        bear_id,
        user_id,
        run_id,
    )
    .await?;
    tracing::info!(
        bear_id = %bear_id,
        profile = %profile.profile.as_str(),
        conversation_id,
        client_session_id,
        model = %model,
        agent_loop_control_level = %agent_loop_control.level.as_str(),
        agent_loop_control_source = ?agent_loop_control.source,
        "resolved native agent loop control profile"
    );
    let session = AgentLoopSession {
        session_key,
        bear_id,
        bear_slug: bear.slug.clone(),
        user_id,
        conversation_id: conversation_id.to_string(),
        client_session_id: client_session_id.to_string(),
        work_run_id,
        checkpoint_audit_context,
        workspace_roots: workspace_roots
            .map(|items| items.to_vec())
            .unwrap_or_default(),
        session_capabilities: session_capabilities_from_client_tools(
            client_tools,
            client_session_id,
            workspace_roots,
        ),
        recently_discovered_capabilities: vec![],
        request_id: request_id.map(|id| id.to_string()),
        run_id,
        technical_budget_recovery_start_payload,
        messages,
        tools,
        budget_components,
        model: model.clone(),
        model_request_profile: den_core::ModelRequestProfile {
            approved_model_ref: model,
            supports_reasoning_effort,
            ..Default::default()
        },
        model_context_window: model_option
            .as_ref()
            .and_then(|option| option.context_window),
        model_max_output_tokens: model_option
            .as_ref()
            .and_then(|option| option.max_output_tokens),
        model_token_calibration,
        bifrost_virtual_key,
        api_style,
        step: 0,
        turn_budget: agent_loop_control.profile.budget,
        turn_budget_state: Default::default(),
        agent_loop_control,
        governance: den_core::governance::Governance::Interactive,
        objective_orientation,
        checkpoint_state: Default::default(),
        pending_checkpoint_request: None,
        pending_checkpoint_task_action: None,
        pending_checkpoint_recovery_attempts: 0,
        strategy: profile.strategy,
        stream_tokens,
        key_memory_projection_cache_key,
        latest_context_budget: None,
        latest_projected_memory,
        latest_recalled_memory,
        cached_activity_plan_projection,
        profile: profile.profile,
        overflow_retry_attempted: false,
        overflow_compaction_recovered: false,
    };
    SESSION_STORE.insert(session.clone());
    Ok(session)
}

async fn ensure_session_task_run(
    pool: &PgPool,
    capabilities: &den_core::CapabilitySet,
    task_list: &Option<TaskListProjection>,
    client_session_id: &str,
    bear_id: Uuid,
    user_id: Option<i32>,
    supplied_run_id: Option<&str>,
) -> Result<Option<String>, DenError> {
    if !capabilities.contains(den_core::BearCapability::OwnSessionTasks) {
        return Ok(supplied_run_id.map(str::to_string));
    }

    let Some(task_list) = task_list else {
        return Ok(supplied_run_id.map(str::to_string));
    };
    if !session_task_needs_run(task_list, client_session_id) {
        return Ok(supplied_run_id.map(str::to_string));
    }

    if let Some(run_id) = supplied_run_id {
        if turn_runs::get_run(pool, run_id).await?.is_some() {
            return Ok(Some(run_id.to_string()));
        }
        tracing::warn!(
            run_id,
            client_session_id,
            "session task execution received an unknown run ID; creating a durable turn run instead"
        );
    }
    if let Some(run) = turn_runs::active_run_for_session(pool, client_session_id).await? {
        return Ok(Some(run.run_id));
    }

    let user_id = user_id.ok_or_else(|| {
        DenError::ValidationError(
            "session task execution requires an authenticated user to create its durable run"
                .to_string(),
        )
    })?;
    let run_id = format!("run_{}", Uuid::new_v4());
    turn_runs::create_run(pool, &run_id, client_session_id, bear_id, user_id).await?;
    Ok(Some(run_id))
}

fn session_task_needs_run(task_list: &TaskListProjection, client_session_id: &str) -> bool {
    task_list.source_client_session_id.as_deref() == Some(client_session_id)
        && task_list.current_item.is_some()
        && !matches!(
            task_list.status.as_str(),
            "blocked" | "completed" | "cancelled" | "archived"
        )
}

#[cfg(test)]
mod session_task_run_tests {
    use super::*;

    fn list(status: &str, session_id: Option<&str>, current: bool) -> TaskListProjection {
        TaskListProjection {
            id: Uuid::nil(),
            bear_id: Uuid::nil(),
            title: "t".to_string(),
            summary: String::new(),
            owner_profile: "pair".to_string(),
            visibility: "private".to_string(),
            status: status.to_string(),
            version: 1,
            source_ref: den_docket::TaskListSourceRef::local(vec![]),
            items: vec![],
            current_item: current.then(|| den_docket::TaskListItem {
                id: "task".to_string(),
                title: "task".to_string(),
                summary: None,
                status: den_docket::TaskListItemStatus::InProgress,
                blocked_reason: None,
                source_ref: den_docket::TaskListSourceRef::local(vec![]),
                sync_state: den_docket::TaskListSyncState::LocalOnly,
            }),
            source_conversation_id: None,
            source_client_session_id: session_id.map(str::to_string),
            handoff_intent_path: None,
            handoff_task_id: None,
            created_at: time::OffsetDateTime::UNIX_EPOCH,
            updated_at: time::OffsetDateTime::UNIX_EPOCH,
        }
    }

    #[test]
    fn work_execution_orientation_is_docket_execution() {
        let task_id = Uuid::new_v4();
        let job_id = Uuid::new_v4();
        assert!(matches!(
            work_execution_orientation_from_ids(task_id, job_id),
            ObjectiveOrientation::DocketExecution { job }
                if job.job_id == job_id.to_string()
                    && matches!(job.active_task_ref,
                        Some(OrientationTaskRef::DocketTask { job_id: Some(ref id), ref task_id, .. })
                        if id == &job_id.to_string() && task_id == &task_id.clone())
        ));
    }

    #[test]
    fn focused_execution_run_requires_session_connected_current_item_in_active_list() {
        assert!(session_task_needs_run(
            &list("active", Some("s"), true),
            "s"
        ));
        assert!(!session_task_needs_run(
            &list("active", Some("other"), true),
            "s"
        ));
        assert!(!session_task_needs_run(
            &list("active", Some("s"), false),
            "s"
        ));
        assert!(!session_task_needs_run(
            &list("blocked", Some("s"), true),
            "s"
        ));
    }
}

pub async fn run_native_profile_turn_collect_assistant_text(
    deps: &NativeRuntimeDeps<'_>,
    bear_id: Uuid,
    role: BearProfile,
    conversation_id: &str,
    session_id: &str,
    prompt: &str,
) -> Result<String, DenError> {
    let profile = NativeCapabilityProfile::for_profile(role);
    let session = build_session(
        deps,
        BuildSessionInput {
            profile,
            bear_id,
            conversation_id,
            client_session_id: session_id,
            human_message: Some(prompt),
            runtime_context: None,
            session_id: Some(session_id),
            workspace_roots: None,
            runtime_target: Some(conversation_id),
            conversation_selection: None,
            user_id: None,
            client_context: None,
            client_tools: None,
            request_id: None,
            run_id: None,
            checkpoint_audit_context: None,
            work_run_id: None,
            stream_tokens: false,
            api_style: None,
            supports_reasoning_effort: None,
            technical_budget_recovery_start_payload: None,
            tool_messages: Vec::new(),
        },
    )
    .await?;
    let llm = LlmClient::new(deps.config);
    let overflow = overflow_context(deps.pool.clone(), Arc::new(deps.config.clone()), role);
    let mut stream = run_agent_step_stream(&llm, &session, Some(overflow)).await?;
    let mut text = String::new();
    while let Some(item) = stream.next().await {
        if let RuntimeStreamEvent::Semantic(RuntimeSemanticEvent::AssistantTextDelta {
            text: delta,
        }) = item?
        {
            text.push_str(&delta);
        }
    }
    Ok(text)
}

pub struct NativeWebChatTurnParams<'a> {
    pub deps: &'a NativeRuntimeDeps<'a>,
    pub bear_id: Uuid,
    pub bear_slug: &'a str,
    pub chat_binding_id: &'a str,
    pub user_id: i32,
    pub username: Option<&'a str>,
    pub membership_role: Option<&'a str>,
    pub conversation_id: &'a str,
    pub session_id: &'a str,
    pub prompt: &'a str,
    pub request_id: Uuid,
    /// Concrete builtin-tool dispatcher injected by the `den` binary (the
    /// `DenToolContext` aggregate lives there, not in `den-runtime`).
    pub tool_invoker: Arc<dyn super::RuntimeToolInvoker>,
}

/// Browser web chat turn (`BearProfile::Chat`) over the native in-process loop.
pub async fn start_native_web_chat_turn_event_stream(
    params: NativeWebChatTurnParams<'_>,
) -> Result<RuntimeEventStream, DenError> {
    let assembly_started = std::time::Instant::now();
    let profile = NativeCapabilityProfile::for_profile(BearProfile::Chat);
    let session = build_session(
        params.deps,
        BuildSessionInput {
            profile,
            bear_id: params.bear_id,
            conversation_id: params.conversation_id,
            client_session_id: params.session_id,
            human_message: Some(params.prompt),
            runtime_context: None,
            session_id: Some(params.session_id),
            workspace_roots: None,
            runtime_target: Some(params.conversation_id),
            conversation_selection: Some(params.conversation_id),
            user_id: Some(params.user_id),
            client_context: None,
            client_tools: None,
            request_id: Some(params.request_id),
            run_id: None,
            checkpoint_audit_context: None,
            work_run_id: None,
            stream_tokens: true,
            api_style: None,
            supports_reasoning_effort: None,
            technical_budget_recovery_start_payload: None,
            tool_messages: Vec::new(),
        },
    )
    .await?;
    tracing::info!(
        request_id = %params.request_id,
        bear_id = %params.bear_id,
        conversation_id = %params.conversation_id,
        message_count = session.messages.len(),
        tool_count = session.tools.len(),
        assembly_ms = assembly_started.elapsed().as_millis(),
        "native web chat turn assembled"
    );
    let llm = LlmClient::new(params.deps.config);
    let config = Arc::new(params.deps.config.clone());
    let overflow = overflow_context(params.deps.pool.clone(), config.clone(), BearProfile::Chat);
    let stream = run_agent_step_stream(&llm, &session, Some(overflow)).await?;
    let runtime = NativeWebChatLoopRuntime {
        pool: params.deps.pool.clone(),
        config,
        llm,
        session_key: session.session_key.clone(),
        bear_id: params.bear_id,
        bear_slug: params.bear_slug.to_string(),
        chat_binding_id: params.chat_binding_id.to_string(),
        user_id: params.user_id,
        username: params.username.map(str::to_string),
        membership_role: params.membership_role.map(str::to_string),
        conversation_id: params.conversation_id.to_string(),
        session_id: params.session_id.to_string(),
        request_id: params.request_id.to_string(),
        session_store: SESSION_STORE.clone(),
        tool_invoker: params.tool_invoker.clone(),
    };
    let step_stream = NativeWebChatLoopStream::wrap_step_stream(&runtime, stream, &session);
    let turn_start_message_len = session.messages.len();
    Ok(Box::pin(NativeWebChatLoopStream::new(
        runtime,
        step_stream,
        turn_start_message_len,
    )))
}

pub async fn start_native_client_turn_event_stream(
    request: TurnStartRequest<'_>,
) -> Result<RuntimeEventStream, DenError> {
    start_native_profile_turn_event_stream(request, BearProfile::Pair).await
}

pub async fn start_native_profile_turn_event_stream(
    request: TurnStartRequest<'_>,
    role: BearProfile,
) -> Result<RuntimeEventStream, DenError> {
    let profile = NativeCapabilityProfile::for_profile(role);
    let runtime_conversations =
        NativeRuntimeConversationBackend::with_pool(request.sqlx_pool.clone());
    let materialized =
        materialize_runtime_conversation_if_needed(&runtime_conversations, &request).await?;
    let conversation_id = materialized.conversation_id;
    let client_session_id = request.session_id;
    let workspace_roots = request
        .workspace_roots
        .map(|roots| roots.to_vec())
        .filter(|roots| !roots.is_empty())
        .or_else(|| request.cwd.map(|cwd| vec![cwd.to_string()]));
    let prompt_for_model = prompt_for_model(request.prompt, request.prompt_context.as_ref());
    let session = build_session(
        &NativeRuntimeDeps {
            pool: request.sqlx_pool,
            config: request.config,
            stores: request.memory_stores,
        },
        BuildSessionInput {
            profile,
            bear_id: request.bear_id,
            conversation_id: &conversation_id,
            client_session_id,
            human_message: Some(prompt_for_model.as_str()),
            runtime_context: request.runtime_context,
            session_id: Some(client_session_id),
            workspace_roots: workspace_roots.as_deref(),
            runtime_target: Some(request.upstream_target),
            conversation_selection: Some(request.conversation_selection),
            user_id: Some(request.user_id),
            client_context: None,
            client_tools: request.client_tools.as_ref(),
            request_id: Some(request.request_id),
            run_id: request.run_id,
            checkpoint_audit_context: request.checkpoint_audit_context,
            work_run_id: request
                .checkpoint_audit_context
                .map(|context| context.work_run_id),
            stream_tokens: request.stream_tokens,
            api_style: request.api_style,
            supports_reasoning_effort: request.supports_reasoning_effort,
            technical_budget_recovery_start_payload: request
                .technical_budget_recovery_start_payload,
            tool_messages: Vec::new(),
        },
    )
    .await?;
    tracing::debug!(
        event = "native_turn_start",
        session_key = %session.session_key,
        conversation_id = %conversation_id,
        client_session_id = %client_session_id,
        request_id = %request.request_id,
        run_id = ?request.run_id,
        step = session.step,
        "native turn starts with a fresh session budget"
    );
    let provenance = ConversationEventProvenance::client_session(client_session_id.to_string());
    let mut content_json = provenance.as_content_json("user_prompt");
    content_json["role"] = serde_json::json!("user");
    content_json["client_session_id"] = serde_json::json!(client_session_id);
    content_json["client"] = serde_json::json!(request.client);
    content_json["request_id"] = serde_json::json!(request.request_id.to_string());
    if let Some(prompt_context) = request.prompt_context.clone() {
        content_json["prompt_context"] = prompt_context.clone();
        if let Some(host_context) = prompt_context.get("host_context") {
            content_json["host_context"] = host_context.clone();
        }
    }
    let record = CanonicalConversationRecord::visible_user_message(
        prompt_for_user_history(request.prompt, request.prompt_context.as_ref()),
        content_json,
        None,
    );
    persist_canonical_conversation_record(
        &canonical_persistence_context(
            request.sqlx_pool.clone(),
            request.bear_id,
            Some(request.user_id),
            conversation_id.clone(),
            Some(client_session_id.to_string()),
            Some(request.request_id.to_string()),
            client_session_id.to_string(),
            false,
        ),
        &record,
    )
    .await?;
    let llm = LlmClient::new(request.config);
    let config = Arc::new(request.config.clone());
    let overflow = overflow_context(request.sqlx_pool.clone(), config.clone(), role);
    let stream = match scripted_runtime_stream(session.run_id.as_deref(), Some(&client_session_id))
    {
        Some(stream) => stream,
        None => run_agent_step_stream(&llm, &session, Some(overflow)).await?,
    };
    let stream = wrap_session_stream(
        stream,
        &session,
        config,
        role,
        request.sqlx_pool.clone(),
        request.bear_id,
        Some(request.user_id),
        &conversation_id,
        client_session_id,
        Some(request.request_id.to_string()),
    );
    let _ = StartTurnRequest {
        conversation: RuntimeConversationRef {
            id: conversation_id,
        },
        binding: request.binding.clone(),
        human_message: request.prompt.to_string(),
        runtime_context: request.runtime_context.map(str::to_string),
        client_session_id: Some(client_session_id.to_string()),
        client_tools: request.client_tools.clone(),
        stream_tokens: request.stream_tokens,
    };
    Ok(stream)
}

pub async fn continue_native_profile_turn_event_stream(
    request: TurnContinueRequest<'_>,
    role: BearProfile,
) -> Result<(RuntimeStreamContinuation, RuntimeEventStream), DenError> {
    continue_native_client_turn_event_stream(request, role).await
}

fn tool_observation_from_call(
    call: &ChatToolCall,
    content: Option<&str>,
    grounding_probe_signal: Option<crate::agent_loop::GroundingProbeSignalKind>,
) -> ToolContinuationObservation {
    ToolContinuationObservation {
        tool_name: call.function.name.clone(),
        signature: tool_signature_from_call(call),
        class: classify_tool_budget_class(&call.function.name),
        failed: tool_result_content_indicates_error(content),
        grounding_probe_signal,
    }
}

async fn grounding_probe_signal_for_tool_observation(
    pool: &PgPool,
    run_id: Option<&str>,
    tool_call_id: &str,
) -> Result<Option<GroundingProbeSignalKind>, DenError> {
    let Some(run_id) = run_id else {
        return Ok(None);
    };
    latest_grounding_probe_signal_for_tool_call(pool, run_id, tool_call_id).await
}

fn tool_class_is_mutation(class: ToolBudgetClass) -> bool {
    matches!(class, ToolBudgetClass::Write | ToolBudgetClass::Destructive)
}

fn mvp_grounding_probe_signal_from_tool_result(
    status: RuntimeToolResultStatus,
    content: &str,
) -> (GroundingProbeSignalKind, GroundingProbeFinding) {
    // ponytail: MVP producer trusts tool status plus error-shaped content; upgrade to
    // read-after-write/diff probes when a mutation surface needs stronger evidence.
    if matches!(status, RuntimeToolResultStatus::Ok)
        && !tool_result_content_indicates_error(Some(content))
    {
        (
            GroundingProbeSignalKind::Pass,
            GroundingProbeFinding {
                code: "tool_result_ok".to_string(),
                message: "Mutation-like tool returned an OK result without an error marker."
                    .to_string(),
            },
        )
    } else {
        (
            GroundingProbeSignalKind::Fail,
            GroundingProbeFinding {
                code: "tool_result_failed".to_string(),
                message: "Mutation-like tool returned a failing or error-shaped result."
                    .to_string(),
            },
        )
    }
}

async fn produce_mvp_grounding_probe_signal_for_tool_result(
    pool: &PgPool,
    session: &AgentLoopSession,
    run_id: Option<&str>,
    call: &ChatToolCall,
    status: RuntimeToolResultStatus,
    content: &str,
) -> Result<Option<GroundingProbeSignalKind>, DenError> {
    let class = classify_tool_budget_class(&call.function.name);
    if !tool_class_is_mutation(class) {
        return Ok(None);
    }
    let (signal, finding) = mvp_grounding_probe_signal_from_tool_result(status, content);
    let Some(run_id) = run_id else {
        return Ok(Some(signal));
    };
    record_grounding_probe_result_decision(
        pool,
        GroundingProbeResultInput {
            run_id: run_id.to_string(),
            turn_step_id: None,
            orientation_kind: Some(session.objective_orientation.kind().to_string()),
            tool_call_id: Some(call.id.clone()),
            probe_id: format!("mvp.tool_result.{}", call.id),
            surface_kind: "tool_result".to_string(),
            signal,
            duration_ms: 0,
            findings: vec![finding],
        },
    )
    .await?;
    Ok(Some(signal))
}

fn parse_args_or_empty_object(raw: &str) -> serde_json::Value {
    serde_json::from_str(raw).unwrap_or_else(|_| serde_json::Value::Object(Default::default()))
}

fn continuation_budget_stop(
    reason: TurnBudgetStopReason,
) -> (RuntimeStreamContinuation, RuntimeEventStream) {
    let stream: RuntimeEventStream = Box::pin(stream::iter(vec![
        Ok(RuntimeStreamEvent::Semantic(
            RuntimeSemanticEvent::RunProgress {
                kind: "turn_budget_exhausted".to_string(),
                text: Some(reason.user_message()),
                phase: Some("budget".to_string()),
                detail: Some(serde_json::json!({
                    "reason": reason.persistence_reason(),
                })),
            },
        )),
        Ok(RuntimeStreamEvent::Semantic(
            RuntimeSemanticEvent::TurnCompleted { turn: None },
        )),
    ]));
    (RuntimeStreamContinuation::Deferred, stream)
}

fn reset_turn_budget_state_after_forced_stop(session: &mut AgentLoopSession) {
    session.turn_budget_state = Default::default();
}

const BUDGET_WARNING_PREFIX: &str = "Budget advisory:";
const CHECKPOINT_NUDGE_PREFIX: &str = "Runtime checkpoint required:";

fn budget_warning_runtime_event(warning: &TurnBudgetWarning) -> RuntimeStreamEvent {
    RuntimeStreamEvent::Semantic(RuntimeSemanticEvent::RunProgress {
        kind: "turn_budget_warning".to_string(),
        text: Some(warning.model_message().to_string()),
        phase: Some("budget".to_string()),
        detail: Some(serde_json::json!({
            "code": warning.code,
        })),
    })
}

fn checkpoint_trigger_runtime_event(trigger: &CheckpointTrigger) -> RuntimeStreamEvent {
    RuntimeStreamEvent::Semantic(RuntimeSemanticEvent::RunProgress {
        kind: "runtime_checkpoint_would_trigger".to_string(),
        text: Some(trigger.message.clone()),
        phase: Some("agent_loop_control".to_string()),
        detail: Some(serde_json::json!({
            "reason": trigger.reason,
            "mode": "observe_only",
        })),
    })
}

fn checkpoint_required_runtime_event(request: &RuntimeCheckpointRequest) -> RuntimeStreamEvent {
    RuntimeStreamEvent::Semantic(RuntimeSemanticEvent::RunProgress {
        kind: "runtime_checkpoint_required".to_string(),
        text: Some("A runtime checkpoint is required before continuing.".to_string()),
        phase: Some("runtime_checkpoint".to_string()),
        detail: Some(serde_json::json!({
            "checkpoint_id": request.checkpoint_id,
            "reason": request.reason,
            "control_level": request.control_level,
        })),
    })
}

fn runtime_checkpoint_request_for_trigger(
    session: &AgentLoopSession,
    trigger: &CheckpointTrigger,
) -> Option<RuntimeCheckpointRequest> {
    let run_id = session.run_id.clone()?;
    Some(RuntimeCheckpointRequest {
        checkpoint_id: format!(
            "ckpt-{}-{}",
            session.step.saturating_add(1),
            trigger.reason.as_str()
        ),
        run_id,
        reason: trigger.reason,
        control_level: session.agent_loop_control.level,
        profile_fingerprint: agent_loop_control_profile_fingerprint(
            &session.agent_loop_control.profile,
        )
        .ok(),
        active_objective: active_checkpoint_objective(session),
        task_context: checkpoint_task_context(session),
        evidence_refs: Vec::new(),
        required_fields: vec![
            CheckpointField::ActiveObjective,
            CheckpointField::MoreExplorationJustified,
            CheckpointField::NextAction,
        ],
    })
}

fn active_checkpoint_objective(session: &AgentLoopSession) -> Option<String> {
    session
        .cached_activity_plan_projection
        .as_ref()
        .and_then(|plan| {
            plan.current_item.as_ref().or_else(|| {
                plan.items.iter().find(|item| {
                    matches!(
                        item.status,
                        den_docket::TaskListItemStatus::Pending
                            | den_docket::TaskListItemStatus::InProgress
                    )
                })
            })
        })
        .map(|item| item.title.clone())
}

fn checkpoint_task_context(session: &AgentLoopSession) -> Option<CheckpointTaskContext> {
    let plan = session.cached_activity_plan_projection.as_ref()?;
    let active_item = plan.current_item.as_ref().or_else(|| {
        plan.items.iter().find(|item| {
            matches!(
                item.status,
                den_docket::TaskListItemStatus::Pending
                    | den_docket::TaskListItemStatus::InProgress
            )
        })
    });
    Some(CheckpointTaskContext {
        task_list_id: Some(plan.id.to_string()),
        task_list_version: None,
        active_item_id: active_item.map(|item| item.id.clone()),
        active_item_title: active_item.map(|item| item.title.clone()),
        docket_job_id: plan.source_ref.docket_job_id.clone(),
        docket_task_id: active_item.and_then(|item| item.source_ref.docket_task_id.clone()),
    })
}

fn agent_loop_control_observe_enabled(config: &Config) -> bool {
    matches!(
        config.agent_loop_control_mode.as_str(),
        "observe" | "enforce"
    )
}

fn agent_loop_control_enforce_enabled(config: &Config) -> bool {
    config.agent_loop_control_mode == "enforce"
}

fn checkpoint_audit_enabled_for_session(config: &Config, session: &AgentLoopSession) -> bool {
    match config.checkpoint_audit_mode.as_str() {
        "all" => true,
        "work" => session.profile == BearProfile::Work,
        _ => false,
    }
}

fn render_checkpoint_nudge(
    request: &RuntimeCheckpointRequest,
    trigger: &CheckpointTrigger,
) -> String {
    let request_json = serde_json::to_string_pretty(request)
        .unwrap_or_else(|_| "{\"error\":\"checkpoint_request_unavailable\"}".to_string());
    format!(
        "{CHECKPOINT_NUDGE_PREFIX} {}\n\nCall the `checkpoint` tool before more exploratory or risky tool use. Do not answer with checkpoint JSON in assistant text. The tool schema validates the decision fields. Use `summary` for a short free-text synthesis; keep structure only for `more_exploration_justified`, `next_action`, `task_state_change_needed`, and `evidence_refs`.\n\nAllowed `next_action` values: `call_tool`, `edit`, `validate`, `update_task_list`, `sync_task_list`, `request_handoff`, `final_if_gate_allows`, `stop_blocked`. If task state should change, choose `update_task_list`, `sync_task_list`, or `request_handoff`, then call the corresponding task tool with evidence after the checkpoint.\n\nCheckpoint request:\n```json\n{request_json}\n```",
        trigger.message
    )
}

fn apply_checkpoint_nudge(
    session: &mut AgentLoopSession,
    request: &RuntimeCheckpointRequest,
    trigger: &CheckpointTrigger,
) -> bool {
    let message = render_checkpoint_nudge(request, trigger);
    if session.messages.last().is_some_and(|message| {
        message.role == "system"
            && message
                .content
                .as_deref()
                .is_some_and(|content| content.starts_with(CHECKPOINT_NUDGE_PREFIX))
    }) {
        session.messages.pop();
    }
    session.pending_checkpoint_request = Some(request.clone());
    session.messages.push(ChatMessage {
        role: "system".to_string(),
        content: Some(message),
        tool_call_id: None,
        name: None,
        tool_calls: None,
    });
    true
}

async fn record_checkpoint_request_if_audited(
    pool: &PgPool,
    config: &Config,
    session: &AgentLoopSession,
    trigger: &CheckpointTrigger,
) {
    if !checkpoint_audit_enabled_for_session(config, session) {
        return;
    }
    let Some(request) = runtime_checkpoint_request_for_trigger(session, trigger) else {
        tracing::debug!(
            session_id = %session.client_session_id,
            reason = %trigger.reason.as_str(),
            "skipping work checkpoint artifact because run id is unavailable"
        );
        return;
    };
    if let Err(err) = record_checkpoint_request(
        pool,
        CheckpointArtifactInput {
            bear_id: session.bear_id,
            created_by_user_id: session.user_id,
            owner_profile: session.profile,
            run_id: request.run_id.clone(),
            turn_step_id: None,
            orientation_kind: Some(session.objective_orientation.kind().to_string()),
            audit_context: session.checkpoint_audit_context,
            request,
            visibility: CheckpointVisibility::AuditOnly,
            replay_policy: CheckpointReplayPolicy::None,
        },
    )
    .await
    {
        tracing::warn!(
            error = %err,
            session_id = %session.client_session_id,
            run_id = session.run_id.as_deref().unwrap_or("<none>"),
            reason = %trigger.reason.as_str(),
            "failed to record observe-only checkpoint artifact"
        );
    }
}

fn render_run_recovery_context(
    disposition: RunRecoveryDisposition,
) -> Result<Option<String>, DenError> {
    let RunRecoveryDisposition::ResumeEligible { attempts } = disposition else {
        return Ok(None);
    };
    let fragments = repository_prompt_fragment_registry()?;
    let fragment = fragments.require("runtime_run_recovery")?;
    render_turn_fragment(
        fragment,
        &serde_json::json!({
            "recovery": {
                "attempts": attempts,
            }
        }),
    )
    .map(|text| Some(text.trim().to_string()))
}

fn apply_run_recovery_context(
    session: &mut AgentLoopSession,
    disposition: RunRecoveryDisposition,
) -> Result<bool, DenError> {
    let Some(message) = render_run_recovery_context(disposition)? else {
        return Ok(false);
    };
    if session.messages.last().is_some_and(|existing| {
        existing.role == "system" && existing.content.as_deref() == Some(message.as_str())
    }) {
        return Ok(false);
    }
    session.messages.push(ChatMessage {
        role: "system".to_string(),
        content: Some(message),
        tool_call_id: None,
        name: None,
        tool_calls: None,
    });
    Ok(true)
}

fn budget_warning_requires_checkpoint(warning: &TurnBudgetWarning) -> bool {
    matches!(
        warning.code,
        "context_budget_warning"
            | "wall_clock_warning"
            | "total_tool_budget_warning"
            | "tool_class_budget_warning"
            | "emergency_hard_step_warning"
    )
}

fn budget_warning_is_model_visible(warning: &TurnBudgetWarning) -> bool {
    !matches!(warning.code, "tool_class_budget_warning")
}

fn apply_budget_warning(session: &mut AgentLoopSession, warning: &TurnBudgetWarning) -> bool {
    if !budget_warning_is_model_visible(warning) {
        return false;
    }
    if session.messages.last().is_some_and(|message| {
        message.role == "system" && message.content.as_deref() == Some(warning.model_message())
    }) {
        return false;
    }
    if session.messages.last().is_some_and(|message| {
        message.role == "system"
            && message
                .content
                .as_deref()
                .is_some_and(|content| content.starts_with(BUDGET_WARNING_PREFIX))
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
    true
}

fn call_is_den_web_fetch(call: &ChatToolCall) -> bool {
    provider_tool_is_den_web_fetch(&call.function.name)
}

fn normalize_approved_web_url(raw: &str) -> Result<String, DenError> {
    let mut url = url::Url::parse(raw.trim())
        .map_err(|err| DenError::ValidationError(format!("web_fetch url is invalid: {err}")))?;
    match url.scheme() {
        "http" | "https" => {}
        _ => {
            return Err(DenError::ValidationError(
                "web_fetch url scheme must be http or https".to_string(),
            ));
        }
    }
    url.set_fragment(None);
    Ok(url.to_string())
}

async fn record_web_fetch_url_approval(
    pool: &PgPool,
    bear_id: Uuid,
    user_id: Option<i32>,
    call: &ChatToolCall,
) -> Result<(), DenError> {
    let args = parse_args_or_empty_object(&call.function.arguments);
    let raw_url = args
        .get("url")
        .and_then(|value| value.as_str())
        .ok_or_else(|| DenError::ValidationError("web_fetch args missing url".to_string()))?;
    let normalized_url = normalize_approved_web_url(raw_url)?;
    sqlx::query!(
        r#"
        INSERT INTO bear_web_approvals (bear_id, scope_kind, scope_value, approved_by_user_id, source, expires_at)
        VALUES ($1, 'url', $2, $3, 'acp', now() + interval '1 hour')
        ON CONFLICT (bear_id, scope_kind, scope_value) WHERE revoked_at IS NULL
        DO UPDATE SET approved_by_user_id = EXCLUDED.approved_by_user_id,
                      source = EXCLUDED.source,
                      expires_at = EXCLUDED.expires_at
        "#,
        bear_id,
        normalized_url,
        user_id
    )
    .execute(pool)
    .await
    .map_err(|err| DenError::Database(format!("record web_fetch approval: {err}")))?;
    Ok(())
}

async fn execute_approved_den_tool_for_session(
    request: &TurnContinueRequest<'_>,
    session: &AgentLoopSession,
    call: &ChatToolCall,
    profile: BearProfile,
) -> Result<ChatMessage, DenError> {
    if call_is_den_web_fetch(call) {
        record_web_fetch_url_approval(request.sqlx_pool, session.bear_id, session.user_id, call)
            .await?;
    }
    let Some(invoker) = super::tool_invoker() else {
        return Err(DenError::System(
            "builtin Den tool runtime is not initialized".to_string(),
        ));
    };
    let canonical = builtin_den_tool_descriptor_for_provider_name(&call.function.name)
        .map(|descriptor| descriptor.name.to_string())
        .unwrap_or_else(|| call.function.name.clone());
    if is_work_tool_provider_name(&call.function.name) || is_work_tool_provider_name(&canonical) {
        let bear = den_service::bears::db::get_bear(request.sqlx_pool, session.bear_id)
            .await?
            .ok_or_else(|| DenError::NotFound("bear not found".to_string()))?;
        if !bear.work_enabled {
            return Err(DenError::ValidationError(
                "work is not enabled for this Bear".to_string(),
            ));
        }
    }
    let args = parse_args_or_empty_object(&call.function.arguments);
    let context = DenToolInvocationContext {
        bear_id: session.bear_id,
        bear_slug: session.bear_slug.clone(),
        binding_id: request.binding.binding_id.clone(),
        profile: Some(profile),
        user_id: session.user_id.unwrap_or_default(),
        username: None,
        membership_role: None,
        conversation_id: session.conversation_id.clone(),
        session_id: session.client_session_id.clone(),
        work_run_id: None,
        client_session_id: Some(session.client_session_id.clone()),
        conversation_selection: Some(session.conversation_id.clone()),
        runtime_target: Some(session.conversation_id.clone()),
        workspace_roots: session.workspace_roots.clone(),
        session_capabilities: session.session_capabilities.clone(),
        session_policy: None,
        activity: session
            .cached_activity_plan_projection
            .as_ref()
            .and_then(|plan| serde_json::to_value(plan).ok()),
        runtime: Some(session.session_info_runtime_snapshot()),
        context_budget: session
            .latest_context_budget
            .as_ref()
            .and_then(|budget| serde_json::to_value(budget).ok()),
        projected_memory: session.latest_projected_memory.clone(),
        recalled_memory: session.latest_recalled_memory.clone(),
        request_id: Some(request.request_id.to_string()),
        channel: DenToolChannelContext {
            family: Some("armature".to_string()),
            client: Some("bearwire".to_string()),
            protocol: Some("bearwire".to_string()),
        },
    };
    let content = match invoker
        .invoke(super::RuntimeToolInvocation {
            tool_name: canonical,
            arguments: args,
            context,
            effective_policy: den_core::EffectivePolicy::compile(
                session.profile,
                session.governance,
                den_core::ArmatureAvailability::Connected,
            ),
            origin_run_id: session
                .run_id
                .clone()
                .map(crate::turn_ids::TurnRunId::new)
                .transpose()?,
            tool_call_id: crate::turn_ids::ToolCallId::new(call.id.clone())?,
        })
        .await
    {
        Ok(value) => serde_json::to_string_pretty(&value).unwrap_or_else(|_| value.to_string()),
        Err(err) => format!("error: {err}"),
    };
    Ok(ChatMessage {
        role: "tool".to_string(),
        content: Some(content),
        tool_call_id: Some(call.id.clone()),
        name: Some(call.function.name.clone()),
        tool_calls: None,
    })
}

pub async fn continue_native_client_turn_event_stream(
    request: TurnContinueRequest<'_>,
    profile: BearProfile,
) -> Result<(RuntimeStreamContinuation, RuntimeEventStream), DenError> {
    let client_session_id = request.client_session_id;
    let conversation_id = request.conversation.id.clone();
    let run_id = request.run_id.ok_or_else(|| {
        DenError::ValidationError("native client continuation requires run_id".to_string())
    })?;
    let session_key = agent_loop_session_key(&conversation_id, client_session_id, run_id);
    let existing_session = SESSION_STORE.get(&session_key);
    let prior_session = existing_session
        .clone()
        .ok_or_else(|| DenError::System("native agent loop session not found".to_string()))?;
    tracing::debug!(
        event = "native_turn_continue",
        session_key = %session_key,
        conversation_id = %conversation_id,
        client_session_id = %client_session_id,
        request_id = %request.request_id,
        run_id = ?request.run_id,
        stored_run_id = ?prior_session.run_id,
        step = prior_session.step,
        "continuing native turn from client result"
    );
    let mut tool_messages = Vec::new();
    let mut observations = Vec::new();
    let observation_run_id = request.run_id.or(prior_session.run_id.as_deref());
    match &request.continuation {
        RuntimeContinuation::ToolResult {
            tool_call_id,
            approval_request_id,
            status,
            content,
        } => {
            if let Some(approval_request_id) = approval_request_id.as_deref() {
                let approve = matches!(status, den_protocol::RuntimeToolResultStatus::Ok);
                record_approval_decision(
                    request.sqlx_pool,
                    approval_request_id,
                    approve,
                    Some(if approve {
                        "tool_result_delivered"
                    } else {
                        "tool_result_failed"
                    }),
                )
                .await?;
            }
            let pending_call = prior_session.find_pending_tool_call(tool_call_id);
            if let Some(call) = pending_call.as_ref() {
                let produced_grounding_probe_signal =
                    produce_mvp_grounding_probe_signal_for_tool_result(
                        request.sqlx_pool,
                        &prior_session,
                        observation_run_id,
                        call,
                        status.clone(),
                        content,
                    )
                    .await?;
                let grounding_probe_signal = match produced_grounding_probe_signal {
                    Some(signal) => Some(signal),
                    None => {
                        grounding_probe_signal_for_tool_observation(
                            request.sqlx_pool,
                            observation_run_id,
                            &call.id,
                        )
                        .await?
                    }
                };
                observations.push(tool_observation_from_call(
                    call,
                    Some(content),
                    grounding_probe_signal,
                ));
            }
            tool_messages.push(ChatMessage {
                role: "tool".to_string(),
                content: Some(content.clone()),
                tool_call_id: Some(tool_call_id.clone()),
                name: pending_call.map(|call| call.function.name),
                tool_calls: None,
            });
        }
        RuntimeContinuation::DocketBoundedSlice => {}
        RuntimeContinuation::ApprovalDecision {
            approval_request_id,
            tool_call_id,
            decision,
            reason,
        } => {
            let approve = matches!(decision, den_protocol::RuntimeApprovalDecision::Approve);
            record_approval_decision(
                request.sqlx_pool,
                approval_request_id,
                approve,
                reason.as_deref(),
            )
            .await?;
            // Approval is control-plane state. Client-owned tools wait for the client
            // to execute them and send RuntimeContinuation::ToolResult; Den-owned tools
            // execute server-side immediately.
            if approve {
                if let Some(session) = existing_session.as_ref() {
                    if let Some(tool_call_id) = tool_call_id.as_deref() {
                        if let Some(call) = session.find_pending_tool_call(tool_call_id) {
                            if builtin_den_tool_descriptor_for_provider_name(&call.function.name)
                                .is_some()
                            {
                                let tool_message = execute_approved_den_tool_for_session(
                                    &request, session, &call, profile,
                                )
                                .await?;
                                let grounding_probe_signal =
                                    grounding_probe_signal_for_tool_observation(
                                        request.sqlx_pool,
                                        observation_run_id,
                                        &call.id,
                                    )
                                    .await?;
                                observations.push(tool_observation_from_call(
                                    &call,
                                    tool_message.content.as_deref(),
                                    grounding_probe_signal,
                                ));
                                tool_messages.push(tool_message);
                            }
                        }
                    }
                }
            } else {
                let content = reason.clone().unwrap_or_else(|| "denied".to_string());
                let pending_call = tool_call_id
                    .as_deref()
                    .and_then(|id| prior_session.find_pending_tool_call(id));
                if let Some(call) = pending_call.as_ref() {
                    let grounding_probe_signal = grounding_probe_signal_for_tool_observation(
                        request.sqlx_pool,
                        observation_run_id,
                        &call.id,
                    )
                    .await?;
                    observations.push(tool_observation_from_call(
                        call,
                        Some(&content),
                        grounding_probe_signal,
                    ));
                }
                tool_messages.push(ChatMessage {
                    role: "tool".to_string(),
                    content: Some(content),
                    tool_call_id: tool_call_id.clone(),
                    name: pending_call.map(|call| call.function.name),
                    tool_calls: None,
                });
            }
        }
    }
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
    let checkpoint_evaluation = evaluate_checkpoint_trigger(
        &prior_session.agent_loop_control.profile,
        &prior_session.checkpoint_state,
        &observations,
        evaluation
            .warning
            .as_ref()
            .is_some_and(budget_warning_requires_checkpoint),
    );
    let mut warning_model_context_applied = false;
    let mut recovery_context_result = Ok(false);
    SESSION_STORE.update(&session_key, |session| {
        session.request_id = Some(request.request_id.to_string());
        session.run_id = request
            .run_id
            .map(str::to_string)
            .or_else(|| session.run_id.clone());
        session.messages.extend(tool_messages.clone());
        recovery_context_result =
            apply_run_recovery_context(session, request.stream_context.run_recovery);
        session.turn_budget_state = evaluation.next_state.clone();
        session.checkpoint_state = checkpoint_evaluation.next_state.clone();
        if let Some(warning) = evaluation.warning.as_ref() {
            warning_model_context_applied = apply_budget_warning(session, warning);
        }
    });
    recovery_context_result?;
    let mut session = SESSION_STORE
        .get(&session_key)
        .ok_or_else(|| DenError::System("native agent loop session not found".to_string()))?;
    let mut required_checkpoint_event = None;
    if let Some(trigger) = checkpoint_evaluation.trigger.as_ref() {
        record_checkpoint_request_if_audited(request.sqlx_pool, request.config, &session, trigger)
            .await;
        if agent_loop_control_enforce_enabled(request.config) {
            if let Some(checkpoint_request) =
                runtime_checkpoint_request_for_trigger(&session, trigger)
            {
                SESSION_STORE.update(&session_key, |session| {
                    apply_checkpoint_nudge(session, &checkpoint_request, trigger);
                });
                required_checkpoint_event =
                    Some(checkpoint_required_runtime_event(&checkpoint_request));
                session = SESSION_STORE.get(&session_key).ok_or_else(|| {
                    DenError::System(
                        "native agent loop session not found after checkpoint nudge".to_string(),
                    )
                })?;
            }
        }
    }
    if let Some(reason) = evaluation.stop_reason {
        let reason_code = reason.persistence_reason();
        let total_tool_calls = session.turn_budget_state.tool_usage.total;
        tracing::info!(
            event = "native_turn_budget_stop",
            reason = reason_code,
            reason_detail = ?reason,
            session_key = %session_key,
            conversation_id = %conversation_id,
            client_session_id = %client_session_id,
            request_id = %request.request_id,
            run_id = ?session.run_id,
            completed_steps = session.step,
            emergency_hard_step_limit = session.turn_budget.emergency_hard_steps,
            total_tool_calls,
            total_tool_call_limit = session.turn_budget.tool_call_limits.total,
            "client continuation stopped by turn budget"
        );
        SESSION_STORE.update(&session_key, reset_turn_budget_state_after_forced_stop);
        return Ok(continuation_budget_stop(reason));
    }
    let llm = LlmClient::new(request.config);
    let config = Arc::new(request.config.clone());
    let overflow = overflow_context(request.sqlx_pool.clone(), config.clone(), profile);
    let stream = match scripted_runtime_stream(session.run_id.as_deref(), Some(&client_session_id))
    {
        Some(stream) => stream,
        None => run_agent_step_stream(&llm, &session, Some(overflow)).await?,
    };
    let mut prefix_events = Vec::new();
    if let Some(warning) = evaluation.warning.as_ref() {
        let model_context_already_has_warning = session.messages.last().is_some_and(|message| {
            message.role == "system" && message.content.as_deref() == Some(warning.model_message())
        });
        if warning_model_context_applied
            || model_context_already_has_warning
            || !budget_warning_is_model_visible(warning)
        {
            prefix_events.push(Ok(budget_warning_runtime_event(warning)));
        }
    }
    if let Some(event) = required_checkpoint_event {
        prefix_events.push(Ok(event));
    } else if agent_loop_control_observe_enabled(request.config) {
        if let Some(trigger) = checkpoint_evaluation.trigger.as_ref() {
            prefix_events.push(Ok(checkpoint_trigger_runtime_event(trigger)));
        }
    }
    let stream = if prefix_events.is_empty() {
        stream
    } else {
        Box::pin(stream::iter(prefix_events).chain(stream)) as RuntimeEventStream
    };
    let stream = wrap_session_stream(
        stream,
        &session,
        config,
        profile,
        request.sqlx_pool.clone(),
        session.bear_id,
        session.user_id,
        &conversation_id,
        client_session_id,
        Some(request.request_id.to_string()),
    );
    let _ = ContinueTurnRequest {
        conversation: request.conversation,
        turn: None,
        binding: request.binding.clone(),
        continuation: request.continuation,
    };
    Ok((RuntimeStreamContinuation::Deferred, stream))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_loop::{
        resolve_agent_loop_control, AgentLoopControlResolutionInput, FreeformPolicy,
        PostMutationVerificationWindow, StrategyProfile, ToolCallBudgetLimits, TurnBudgetPolicy,
    };

    fn sample_budget_warning(message: &str) -> TurnBudgetWarning {
        sample_budget_warning_with_code("total_tool_budget_warning", message)
    }

    fn sample_budget_warning_with_code(code: &'static str, message: &str) -> TurnBudgetWarning {
        TurnBudgetWarning {
            code,
            message: message.to_string(),
        }
    }

    fn pair_turn_budget() -> TurnBudgetPolicy {
        TurnBudgetPolicy {
            max_wall_clock_ms: 60_000,
            emergency_hard_steps: 8,
            tool_call_limits: ToolCallBudgetLimits {
                total: 8,
                read: 8,
                search: 8,
                fetch: 8,
                execute: 8,
                write: 8,
                destructive: 2,
                other: 8,
            },
            max_consecutive_tool_failures: 3,
            max_same_tool_signature_repeats: 2,
            post_mutation_verification_window: Some(PostMutationVerificationWindow {
                replenish_read: 2,
                replenish_search: 1,
            }),
        }
    }

    fn test_agent_loop_control() -> crate::agent_loop::ResolvedAgentLoopControl {
        resolve_agent_loop_control(AgentLoopControlResolutionInput {
            model_handle: Some("openai/test"),
            model_default: None,
            bear_override: None,
            stance_override: None,
            task_escalation: None,
            stance: Some(BearProfile::Pair),
            objective_orientation: None,
            pre_risk: false,
        })
    }

    fn freeform_orientation() -> crate::agent_loop::ObjectiveOrientation {
        crate::agent_loop::ObjectiveOrientation::Freeform {
            policy: FreeformPolicy::closed(),
        }
    }

    #[test]
    fn docket_bounded_slice_continuation_is_typed_and_carries_no_scheduler_input() {
        let continuation = RuntimeContinuation::DocketBoundedSlice;
        let value = serde_json::to_value(&continuation).expect("serialize continuation");

        assert_eq!(value, serde_json::json!("DocketBoundedSlice"));
    }

    #[test]
    fn native_turn_hard_budget_comes_from_resolved_control_profile() {
        let standard = resolve_agent_loop_control(AgentLoopControlResolutionInput {
            model_handle: Some("openai/test"),
            model_default: Some(den_core::AgentLoopControlLevel::Standard),
            bear_override: None,
            stance_override: None,
            task_escalation: None,
            stance: Some(BearProfile::Pair),
            objective_orientation: None,
            pre_risk: false,
        });
        let resolved = resolve_agent_loop_control(AgentLoopControlResolutionInput {
            model_handle: Some("openai/test"),
            model_default: Some(den_core::AgentLoopControlLevel::Strict),
            bear_override: None,
            stance_override: None,
            task_escalation: None,
            stance: Some(BearProfile::Pair),
            objective_orientation: None,
            pre_risk: false,
        });
        let base = resolved.profile;
        assert_ne!(
            base.budget.emergency_hard_steps,
            standard.profile.budget.emergency_hard_steps
        );
        let initialized = native_turn_control_profile(resolved, 1.5);

        assert_eq!(initialized.level, den_core::AgentLoopControlLevel::Strict);
        assert_eq!(
            initialized.profile.budget.tool_call_limits.total,
            (f64::from(base.budget.tool_call_limits.total) * 1.5).ceil() as u32
        );
        assert_eq!(
            initialized.profile.budget.max_wall_clock_ms,
            base.budget.max_wall_clock_ms
        );
        assert_eq!(
            initialized.profile.budget.emergency_hard_steps,
            base.budget.emergency_hard_steps
        );
        assert_eq!(
            initialized.profile.budget.max_consecutive_tool_failures,
            base.budget.max_consecutive_tool_failures
        );
        assert_eq!(
            initialized.profile.ko.max_same_tool_signature_repeats,
            base.ko.max_same_tool_signature_repeats
        );
        assert_eq!(
            initialized
                .profile
                .budget
                .post_mutation_verification_window
                .expect("strict profile has a verification window")
                .replenish_read,
            (f64::from(
                base.budget
                    .post_mutation_verification_window
                    .expect("strict profile has a verification window")
                    .replenish_read
            ) * 1.5)
                .ceil() as u32
        );
    }

    #[test]
    fn mvp_grounding_probe_only_targets_mutation_classes() {
        assert!(tool_class_is_mutation(classify_tool_budget_class(
            "fs_edit_file"
        )));
        assert!(tool_class_is_mutation(classify_tool_budget_class(
            "fs_delete_path"
        )));
        assert!(!tool_class_is_mutation(classify_tool_budget_class(
            "fs_read_text_file"
        )));
    }

    #[test]
    fn native_tool_failure_diagnostics_are_actionable_without_tool_arguments() {
        let diagnostics = native_tool_result_diagnostics(
            RuntimeToolResultStatus::Error,
            ToolResultStatus::Error,
            "call-123",
        );
        assert_eq!(diagnostics["phase"], "client_tool_result_failed");
        assert_eq!(diagnostics["failure_class"], "adapter_tool_error");
        assert_eq!(diagnostics["tool_status"], "error");
        assert_eq!(diagnostics["tool_call_id"], "call-123");
        assert!(diagnostics.get("arguments").is_none());
    }
    #[test]
    fn mvp_grounding_probe_signal_follows_tool_result_status_and_content() {
        let (ok_signal, ok_finding) = mvp_grounding_probe_signal_from_tool_result(
            RuntimeToolResultStatus::Ok,
            "{\"ok\":true}",
        );
        assert_eq!(ok_signal, GroundingProbeSignalKind::Pass);
        assert_eq!(ok_finding.code, "tool_result_ok");

        let (error_signal, error_finding) = mvp_grounding_probe_signal_from_tool_result(
            RuntimeToolResultStatus::Ok,
            "error: write failed",
        );
        assert_eq!(error_signal, GroundingProbeSignalKind::Fail);
        assert_eq!(error_finding.code, "tool_result_failed");

        let (status_signal, _) = mvp_grounding_probe_signal_from_tool_result(
            RuntimeToolResultStatus::Error,
            "{\"ok\":false}",
        );
        assert_eq!(status_signal, GroundingProbeSignalKind::Fail);
    }

    #[test]
    fn bear_id_from_native_binding_parses_den_native_format() {
        let bear_id = Uuid::new_v4();
        let binding = RoleRuntimeBinding {
            binding_id: format!("den-native:{bear_id}:pair"),
            compatibility_backend: Some("runtime:native".to_string()),
        };
        assert_eq!(bear_id_from_native_binding(&binding), Some(bear_id));
    }

    #[test]
    fn bear_id_from_native_binding_rejects_non_native_bindings() {
        let binding = RoleRuntimeBinding {
            binding_id: "legacy-provider-123".to_string(),
            compatibility_backend: Some("runtime:legacy".to_string()),
        };
        assert_eq!(bear_id_from_native_binding(&binding), None);
    }

    #[test]
    fn run_recovery_context_renders_neutral_fragment_once() {
        let message =
            render_run_recovery_context(RunRecoveryDisposition::ResumeEligible { attempts: 0 })
                .expect("recovery fragment renders")
                .expect("resume-eligible recovery emits context");

        assert!(message.contains("ended before final delivery"));
        assert!(!message.to_lowercase().contains("continue"));
        assert!(!message.to_lowercase().contains("try again"));
        assert!(
            render_run_recovery_context(RunRecoveryDisposition::Exhausted { attempts: 1 })
                .expect("exhausted recovery renders")
                .is_none()
        );
    }

    #[test]
    fn apply_run_recovery_context_is_idempotent_for_same_session_tail() {
        let mut session = AgentLoopSession {
            session_key: "session".to_string(),
            bear_id: Uuid::new_v4(),
            bear_slug: "test-bear".to_string(),
            user_id: Some(1),
            conversation_id: "conv".to_string(),
            client_session_id: "session".to_string(),
            work_run_id: None,
            checkpoint_audit_context: None,
            workspace_roots: vec![],
            session_capabilities: vec![],
            recently_discovered_capabilities: vec![],
            request_id: None,
            run_id: None,
            technical_budget_recovery_start_payload: None,
            messages: vec![],
            tools: vec![],
            budget_components: Default::default(),
            model: "openai/test".to_string(),
            model_request_profile: den_core::ModelRequestProfile {
                approved_model_ref: "openai/test".to_string(),
                ..Default::default()
            },
            model_context_window: None,
            model_max_output_tokens: None,
            model_token_calibration: None,
            bifrost_virtual_key: None,
            api_style: None,
            step: 0,
            turn_budget: pair_turn_budget(),
            turn_budget_state: Default::default(),
            agent_loop_control: test_agent_loop_control(),
            governance: den_core::governance::Governance::Interactive,
            objective_orientation: freeform_orientation(),
            checkpoint_state: Default::default(),
            pending_checkpoint_request: None,
            pending_checkpoint_task_action: None,
            pending_checkpoint_recovery_attempts: 0,
            strategy: StrategyProfile::plain_react(),
            stream_tokens: false,
            key_memory_projection_cache_key: None,
            latest_context_budget: None,
            latest_projected_memory: None,
            latest_recalled_memory: None,
            cached_activity_plan_projection: None,
            profile: BearProfile::Pair,
            overflow_retry_attempted: false,
            overflow_compaction_recovered: false,
        };
        let disposition = RunRecoveryDisposition::ResumeEligible { attempts: 0 };

        assert!(apply_run_recovery_context(&mut session, disposition).expect("first apply"));
        assert!(!apply_run_recovery_context(&mut session, disposition).expect("second apply"));
        assert_eq!(
            session
                .messages
                .iter()
                .filter(|message| message.role == "system")
                .count(),
            1
        );
    }

    #[test]
    fn budget_stop_completes_with_a_user_safe_status() {
        let (_continuation, stream) =
            continuation_budget_stop(TurnBudgetStopReason::WallClockLimit {
                elapsed_ms: 60_001,
                limit_ms: 60_000,
            });
        let events = futures::executor::block_on(async {
            use futures::StreamExt;
            stream.collect::<Vec<_>>().await
        });

        assert!(matches!(
            events.as_slice(),
            [
                Ok(RuntimeStreamEvent::Semantic(RuntimeSemanticEvent::RunProgress {
                    kind,
                    text: Some(text),
                    phase,
                    detail: Some(detail),
                })),
                Ok(RuntimeStreamEvent::Semantic(RuntimeSemanticEvent::TurnCompleted { turn: None })),
            ] if kind == "turn_budget_exhausted"
                && phase.as_deref() == Some("budget")
                && detail["reason"] == "wall_clock_limit"
                && text.contains("Send “continue”")
                && !text.contains("elapsed=")
        ));
    }

    #[test]
    fn pair_budget_stop_terminalizes_with_a_user_safe_status() {
        let reason = TurnBudgetStopReason::ToolClassCallLimit {
            class: ToolBudgetClass::Other,
            count: 18,
            limit: 16,
        };
        let (_continuation, stream) = continuation_budget_stop(reason);
        let events = futures::executor::block_on(async {
            use futures::StreamExt;
            stream.collect::<Vec<_>>().await
        });

        assert!(matches!(
            events.last(),
            Some(Ok(RuntimeStreamEvent::Semantic(
                RuntimeSemanticEvent::TurnCompleted { turn: None }
            )))
        ));
    }

    #[test]
    fn forced_budget_stop_resets_reusable_turn_budget_state() {
        let mut session = AgentLoopSession {
            session_key: "session".to_string(),
            bear_id: Uuid::new_v4(),
            bear_slug: "test-bear".to_string(),
            user_id: Some(1),
            conversation_id: "conv".to_string(),
            client_session_id: "session".to_string(),
            work_run_id: None,
            checkpoint_audit_context: None,
            workspace_roots: vec![],
            session_capabilities: vec![],
            recently_discovered_capabilities: vec![],
            request_id: None,
            run_id: None,
            technical_budget_recovery_start_payload: None,
            messages: vec![],
            tools: vec![],
            budget_components: Default::default(),
            model: "openai/test".to_string(),
            model_request_profile: den_core::ModelRequestProfile {
                approved_model_ref: "openai/test".to_string(),
                ..Default::default()
            },
            model_context_window: None,
            model_max_output_tokens: None,
            model_token_calibration: None,
            bifrost_virtual_key: None,
            api_style: None,
            step: 0,
            turn_budget: pair_turn_budget(),
            turn_budget_state: Default::default(),
            agent_loop_control: test_agent_loop_control(),
            governance: den_core::governance::Governance::Interactive,
            objective_orientation: freeform_orientation(),
            checkpoint_state: Default::default(),
            pending_checkpoint_request: None,
            pending_checkpoint_task_action: None,
            pending_checkpoint_recovery_attempts: 0,
            strategy: StrategyProfile::plain_react(),
            stream_tokens: false,
            key_memory_projection_cache_key: None,
            latest_context_budget: None,
            latest_projected_memory: None,
            latest_recalled_memory: None,
            cached_activity_plan_projection: None,
            profile: BearProfile::Pair,
            overflow_retry_attempted: false,
            overflow_compaction_recovered: false,
        };
        session.turn_budget_state.same_batch_signature_repeats = 3;
        session.turn_budget_state.last_batch_signature = Some("git_status:{}".to_string());
        session.turn_budget_state.consecutive_tool_failures = 2;
        session.turn_budget_state.budget_finalization_grace_used = true;

        reset_turn_budget_state_after_forced_stop(&mut session);

        assert_eq!(session.turn_budget_state.same_batch_signature_repeats, 0);
        assert_eq!(session.turn_budget_state.last_batch_signature, None);
        assert_eq!(session.turn_budget_state.consecutive_tool_failures, 0);
        assert!(!session.turn_budget_state.budget_finalization_grace_used);
        assert_eq!(session.turn_budget_state.tool_usage.total, 0);
    }

    #[test]
    fn budget_warning_checkpoint_gate_only_treats_budget_pressure_as_low_budget() {
        for code in [
            "context_budget_warning",
            "wall_clock_warning",
            "total_tool_budget_warning",
            "tool_class_budget_warning",
            "emergency_hard_step_warning",
        ] {
            assert!(
                budget_warning_requires_checkpoint(&sample_budget_warning_with_code(
                    code,
                    "Budget advisory"
                )),
                "{code} should request a low-budget checkpoint"
            );
        }

        for code in ["rule_of_ko_warning", "failure_budget_warning"] {
            assert!(
                !budget_warning_requires_checkpoint(&sample_budget_warning_with_code(
                    code,
                    "Budget advisory"
                )),
                "{code} has a dedicated checkpoint reason"
            );
        }
    }

    #[test]
    fn apply_budget_warning_reports_when_message_changes() {
        let mut session = AgentLoopSession {
            session_key: "session".to_string(),
            bear_id: Uuid::new_v4(),
            bear_slug: "test-bear".to_string(),
            user_id: Some(1),
            conversation_id: "conv".to_string(),
            client_session_id: "session".to_string(),
            work_run_id: None,
            checkpoint_audit_context: None,
            workspace_roots: vec![],
            session_capabilities: vec![],
            recently_discovered_capabilities: vec![],
            request_id: None,
            run_id: None,
            technical_budget_recovery_start_payload: None,
            messages: vec![],
            tools: vec![],
            budget_components: Default::default(),
            model: "openai/test".to_string(),
            model_request_profile: den_core::ModelRequestProfile {
                approved_model_ref: "openai/test".to_string(),
                ..Default::default()
            },
            model_context_window: None,
            model_max_output_tokens: None,
            model_token_calibration: None,
            bifrost_virtual_key: None,
            api_style: None,
            step: 0,
            turn_budget: pair_turn_budget(),
            turn_budget_state: Default::default(),
            agent_loop_control: test_agent_loop_control(),
            governance: den_core::governance::Governance::Interactive,
            objective_orientation: freeform_orientation(),
            checkpoint_state: Default::default(),
            pending_checkpoint_request: None,
            pending_checkpoint_task_action: None,
            pending_checkpoint_recovery_attempts: 0,
            strategy: StrategyProfile::plain_react(),
            stream_tokens: false,
            key_memory_projection_cache_key: None,
            latest_context_budget: None,
            latest_projected_memory: None,
            latest_recalled_memory: None,
            cached_activity_plan_projection: None,
            profile: BearProfile::Pair,
            overflow_retry_attempted: false,
            overflow_compaction_recovered: false,
        };

        let warning = sample_budget_warning("Budget advisory: close to read budget.");
        assert!(apply_budget_warning(&mut session, &warning));
        assert!(!apply_budget_warning(&mut session, &warning));

        let replacement = sample_budget_warning("Budget advisory: next read will stop the turn.");
        assert!(apply_budget_warning(&mut session, &replacement));
        assert_eq!(session.messages.len(), 1);
        assert_eq!(
            session.messages[0].content.as_deref(),
            Some(replacement.model_message())
        );
    }

    #[test]
    fn apply_budget_warning_keeps_tool_class_warning_out_of_model_context() {
        let mut session = AgentLoopSession {
            session_key: "session".to_string(),
            bear_id: Uuid::new_v4(),
            bear_slug: "test-bear".to_string(),
            user_id: Some(1),
            conversation_id: "conv".to_string(),
            client_session_id: "session".to_string(),
            work_run_id: None,
            checkpoint_audit_context: None,
            workspace_roots: vec![],
            session_capabilities: vec![],
            recently_discovered_capabilities: vec![],
            request_id: None,
            run_id: None,
            technical_budget_recovery_start_payload: None,
            messages: vec![],
            tools: vec![],
            budget_components: Default::default(),
            model: "openai/test".to_string(),
            model_request_profile: den_core::ModelRequestProfile {
                approved_model_ref: "openai/test".to_string(),
                ..Default::default()
            },
            model_context_window: None,
            model_max_output_tokens: None,
            model_token_calibration: None,
            bifrost_virtual_key: None,
            api_style: None,
            step: 0,
            turn_budget: pair_turn_budget(),
            turn_budget_state: Default::default(),
            agent_loop_control: test_agent_loop_control(),
            governance: den_core::governance::Governance::Interactive,
            objective_orientation: freeform_orientation(),
            checkpoint_state: Default::default(),
            pending_checkpoint_request: None,
            pending_checkpoint_task_action: None,
            pending_checkpoint_recovery_attempts: 0,
            strategy: StrategyProfile::plain_react(),
            stream_tokens: false,
            key_memory_projection_cache_key: None,
            latest_context_budget: None,
            latest_projected_memory: None,
            latest_recalled_memory: None,
            cached_activity_plan_projection: None,
            profile: BearProfile::Pair,
            overflow_retry_attempted: false,
            overflow_compaction_recovered: false,
        };
        let warning = sample_budget_warning_with_code(
            "tool_class_budget_warning",
            "Budget advisory: this turn is close to its read tool budget (5/6 used). Prefer a final answer over more tool calls unless one more read call is strictly necessary.",
        );

        assert!(!apply_budget_warning(&mut session, &warning));
        assert!(session.messages.is_empty());
    }

    #[test]
    fn checkpoint_nudge_renders_structured_request_without_task_state_mutation() {
        let request = RuntimeCheckpointRequest {
            checkpoint_id: "ckpt-1".to_string(),
            run_id: "run-1".to_string(),
            reason: crate::agent_loop::CheckpointReason::OverExploration,
            control_level: den_core::AgentLoopControlLevel::Careful,
            profile_fingerprint: Some("profile-test".to_string()),
            active_objective: Some("Inspect routing".to_string()),
            task_context: None,
            evidence_refs: Vec::new(),
            required_fields: vec![CheckpointField::Learned, CheckpointField::NextAction],
        };
        let trigger = CheckpointTrigger {
            reason: crate::agent_loop::CheckpointReason::OverExploration,
            message: "summarize before more reads".to_string(),
        };

        let rendered = render_checkpoint_nudge(&request, &trigger);

        assert!(rendered.starts_with(CHECKPOINT_NUDGE_PREFIX));
        assert!(rendered.contains("Call the `checkpoint` tool"));
        assert!(rendered.contains("\"checkpoint_id\": \"ckpt-1\""));
        assert!(rendered.contains("Do not answer with checkpoint JSON"));
        assert!(rendered.contains("Allowed `next_action` values"));
    }

    #[test]
    fn budget_warning_runtime_event_is_user_visible_progress_not_error() {
        let warning = sample_budget_warning("Budget advisory: next read will stop the turn.");
        let event = budget_warning_runtime_event(&warning);

        match event {
            RuntimeStreamEvent::Semantic(RuntimeSemanticEvent::RunProgress {
                kind,
                text,
                phase,
                detail: Some(detail),
            }) => {
                assert_eq!(kind, "turn_budget_warning");
                assert_eq!(text.as_deref(), Some(warning.model_message()));
                assert_eq!(phase.as_deref(), Some("budget"));
                assert_eq!(detail["code"].as_str(), Some(warning.code));
            }
            other => panic!("expected RunProgress budget warning, got {other:?}"),
        }
    }

    #[test]
    fn checkpoint_trigger_runtime_event_is_observe_only_progress() {
        let trigger = CheckpointTrigger {
            reason: crate::agent_loop::CheckpointReason::OverExploration,
            message: "summarize before more reads".to_string(),
        };
        let event = checkpoint_trigger_runtime_event(&trigger);

        match event {
            RuntimeStreamEvent::Semantic(RuntimeSemanticEvent::RunProgress {
                kind,
                phase,
                detail: Some(detail),
                ..
            }) => {
                assert_eq!(kind, "runtime_checkpoint_would_trigger");
                assert_eq!(phase.as_deref(), Some("agent_loop_control"));
                assert_eq!(detail["reason"], "over_exploration");
                assert_eq!(detail["mode"], "observe_only");
            }
            other => panic!("expected observe-only checkpoint progress, got {other:?}"),
        }
    }

    #[test]
    fn checkpoint_required_runtime_event_is_typed_progress() {
        let request = RuntimeCheckpointRequest {
            checkpoint_id: "ckpt-required".to_string(),
            run_id: "run-test".to_string(),
            reason: crate::agent_loop::CheckpointReason::OverExploration,
            control_level: den_core::AgentLoopControlLevel::Careful,
            profile_fingerprint: None,
            active_objective: None,
            task_context: None,
            evidence_refs: Vec::new(),
            required_fields: Vec::new(),
        };

        match checkpoint_required_runtime_event(&request) {
            RuntimeStreamEvent::Semantic(RuntimeSemanticEvent::RunProgress {
                kind,
                phase,
                detail: Some(detail),
                ..
            }) => {
                assert_eq!(kind, "runtime_checkpoint_required");
                assert_eq!(phase.as_deref(), Some("runtime_checkpoint"));
                assert_eq!(detail["checkpoint_id"], "ckpt-required");
                assert_eq!(detail["reason"], "over_exploration");
                assert_eq!(detail["control_level"], "careful");
            }
            other => panic!("expected required checkpoint progress, got {other:?}"),
        }
    }

    #[test]
    fn prompt_for_model_wraps_human_prompt_with_structured_host_context() {
        let prompt = prompt_for_model(
            "Please inspect this.",
            Some(&serde_json::json!({
                "format": "acp_prompt_context.v1",
                "host_context": {
                    "kind": "referenced_resources",
                    "delivery": "reference_only",
                    "persistence": "not_human_message",
                    "resources": [
                        {
                            "label": "src/lib.rs",
                            "uri": "file:///workspace/src/lib.rs",
                            "mime_type": "text/rust",
                            "embedded_text_bytes": 128
                        }
                    ]
                }
            })),
        );

        assert!(prompt.contains("<host_context kind=\"referenced_resources\""));
        assert!(prompt.contains("file:///workspace/src/lib.rs"));
        assert!(prompt.contains("embedded_text_bytes: 128 (body omitted"));
        assert!(prompt.contains("<user_message>\nPlease inspect this.\n</user_message>"));
    }

    #[tokio::test]
    async fn hard_step_continuation_returns_terminal_event_not_error() {
        let conversation_id = format!("transient-{}", Uuid::new_v4().simple());
        let client_session_id = format!("session-{}", Uuid::new_v4().simple());
        let session_key =
            agent_loop_session_key(&conversation_id, &client_session_id, "run-max-step");
        let config = Config::test_stub();
        let stores = MemoryStoreManager::new(&config);
        let pool = PgPool::connect_lazy("postgres://postgres:postgres@127.0.0.1/unused")
            .expect("lazy pool");
        SESSION_STORE.insert(AgentLoopSession {
            session_key: session_key.clone(),
            bear_id: Uuid::new_v4(),
            bear_slug: "test-bear".to_string(),
            user_id: Some(1),
            conversation_id: conversation_id.clone(),
            client_session_id: client_session_id.clone(),
            work_run_id: None,
            checkpoint_audit_context: None,
            workspace_roots: vec!["/workspace".to_string()],
            session_capabilities: vec![],
            recently_discovered_capabilities: vec![],
            request_id: None,
            run_id: Some("run-max-step".to_string()),
            technical_budget_recovery_start_payload: None,
            messages: Vec::new(),
            tools: Vec::new(),
            budget_components: Default::default(),
            model: "openai/test".to_string(),
            model_request_profile: den_core::ModelRequestProfile {
                approved_model_ref: "openai/test".to_string(),
                ..Default::default()
            },
            model_context_window: None,
            model_max_output_tokens: None,
            model_token_calibration: None,
            bifrost_virtual_key: None,
            api_style: None,
            step: 8,
            turn_budget: pair_turn_budget(),
            turn_budget_state: Default::default(),
            agent_loop_control: test_agent_loop_control(),
            governance: den_core::governance::Governance::Interactive,
            objective_orientation: freeform_orientation(),
            checkpoint_state: Default::default(),
            pending_checkpoint_request: None,
            pending_checkpoint_task_action: None,
            pending_checkpoint_recovery_attempts: 0,
            strategy: StrategyProfile::plain_react(),
            stream_tokens: false,
            key_memory_projection_cache_key: None,
            latest_context_budget: None,
            latest_projected_memory: None,
            latest_recalled_memory: None,
            cached_activity_plan_projection: None,
            profile: BearProfile::Pair,
            overflow_retry_attempted: false,
            overflow_compaction_recovered: false,
        });

        let (_continuation, mut stream) = continue_native_client_turn_event_stream(
            TurnContinueRequest {
                sqlx_pool: &pool,
                config: &config,
                memory_stores: &stores,
                request_id: Uuid::new_v4(),
                run_id: Some("run-max-step"),
                client_session_id: &client_session_id,
                conversation: RuntimeConversationRef {
                    id: conversation_id.clone(),
                },
                binding: &RoleRuntimeBinding {
                    binding_id: format!("den-native:{}:pair", Uuid::new_v4()),
                    compatibility_backend: Some("native".to_string()),
                },
                continuation: RuntimeContinuation::ToolResult {
                    tool_call_id: "call-max".to_string(),
                    approval_request_id: None,
                    status: RuntimeToolResultStatus::Ok,
                    content: "{}".to_string(),
                },
                stream_context: crate::turn_runner::default_tool_continue_stream_context(),
            },
            BearProfile::Pair,
        )
        .await
        .expect("max-step continuation should return terminal stream");

        let event = stream
            .next()
            .await
            .expect("terminal event")
            .expect("event ok");
        match event {
            RuntimeStreamEvent::Semantic(RuntimeSemanticEvent::RunProgress {
                kind,
                text: Some(text),
                phase,
                detail: Some(detail),
            }) => {
                assert_eq!(kind, "turn_budget_exhausted");
                assert_eq!(phase.as_deref(), Some("budget"));
                assert_eq!(detail["reason"], "emergency_hard_step_limit");
                assert!(text.contains("emergency continuation fuse"));
                assert!(text.contains("step=8/emergency_hard_steps=8"));
            }
            other => panic!("expected Den status for terminal stop, got {other:?}"),
        }
        SESSION_STORE.remove(&session_key);
    }

    #[tokio::test]
    async fn recorded_client_tool_result_remains_visible_in_live_continuation_transcript() {
        let conversation_id = format!("transient-{}", Uuid::new_v4().simple());
        let client_session_id = format!("session-{}", Uuid::new_v4().simple());
        let session_key =
            agent_loop_session_key(&conversation_id, &client_session_id, "run-visible-tool");
        let tool_call_id = "call-visible-tool";
        SESSION_STORE.insert(AgentLoopSession {
            session_key: session_key.clone(),
            bear_id: Uuid::new_v4(),
            bear_slug: "test-bear".to_string(),
            user_id: Some(1),
            conversation_id: conversation_id.clone(),
            client_session_id: client_session_id.clone(),
            work_run_id: None,
            checkpoint_audit_context: None,
            workspace_roots: vec!["/workspace".to_string()],
            session_capabilities: vec![],
            recently_discovered_capabilities: vec![],
            request_id: Some("request-before-tool".to_string()),
            run_id: Some("run-visible-tool".to_string()),
            technical_budget_recovery_start_payload: None,
            messages: vec![
                ChatMessage {
                    role: "user".to_string(),
                    content: Some("Inspect the plan".to_string()),
                    tool_call_id: None,
                    name: None,
                    tool_calls: None,
                },
                ChatMessage {
                    role: "assistant".to_string(),
                    content: None,
                    tool_call_id: None,
                    name: None,
                    tool_calls: Some(vec![ChatToolCall {
                        id: tool_call_id.to_string(),
                        call_type: "function".to_string(),
                        function: crate::llm::ChatToolCallFunction {
                            name: "fs_read_text_file".to_string(),
                            arguments: serde_json::json!({
                                "path": "/workspace/docs/roadmap/PLAN.md",
                                "limit": 120,
                            })
                            .to_string(),
                        },
                    }]),
                },
            ],
            tools: Vec::new(),
            budget_components: Default::default(),
            model: "openai/test".to_string(),
            model_request_profile: den_core::ModelRequestProfile {
                approved_model_ref: "openai/test".to_string(),
                ..Default::default()
            },
            model_context_window: None,
            model_max_output_tokens: None,
            model_token_calibration: None,
            bifrost_virtual_key: None,
            api_style: None,
            step: 1,
            turn_budget: pair_turn_budget(),
            turn_budget_state: Default::default(),
            agent_loop_control: test_agent_loop_control(),
            governance: den_core::governance::Governance::Interactive,
            objective_orientation: freeform_orientation(),
            checkpoint_state: Default::default(),
            pending_checkpoint_request: None,
            pending_checkpoint_task_action: None,
            pending_checkpoint_recovery_attempts: 0,
            strategy: StrategyProfile::plain_react(),
            stream_tokens: false,
            key_memory_projection_cache_key: None,
            latest_context_budget: None,
            latest_projected_memory: None,
            latest_recalled_memory: None,
            cached_activity_plan_projection: None,
            profile: BearProfile::Pair,
            overflow_retry_attempted: false,
            overflow_compaction_recovered: false,
        });
        let pool = PgPool::connect_lazy("postgres://postgres:postgres@127.0.0.1/unused")
            .expect("lazy pool");
        assert!(native_client_run_exists(
            &conversation_id,
            &client_session_id,
            "run-visible-tool"
        ));
        assert!(!native_client_run_exists(
            &conversation_id,
            &client_session_id,
            "run-successor"
        ));

        record_native_client_tool_result(
            &pool,
            &conversation_id,
            &client_session_id,
            "request-after-tool",
            Some("run-visible-tool"),
            tool_call_id,
            None,
            RuntimeToolResultStatus::Ok,
            serde_json::json!({
                "tool_name": "fs_read_text_file",
                "status": "ok",
                "structured_content": { "content": "# BEARS roadmap" }
            })
            .to_string(),
        )
        .await
        .expect("record tool result");

        let stored = SESSION_STORE.get(&session_key).expect("stored session");
        assert_eq!(stored.messages.len(), 3);
        assert_eq!(stored.messages[1].role, "assistant");
        assert_eq!(
            stored.messages[1]
                .tool_calls
                .as_ref()
                .and_then(|calls| calls.first())
                .map(|call| (call.id.as_str(), call.function.name.as_str())),
            Some((tool_call_id, "fs_read_text_file"))
        );
        assert_eq!(stored.messages[2].role, "tool");
        assert_eq!(
            stored.messages[2].tool_call_id.as_deref(),
            Some(tool_call_id)
        );
        assert!(stored.messages[2]
            .content
            .as_deref()
            .unwrap_or_default()
            .contains("BEARS roadmap"));

        let repaired = crate::agent_loop::repair_tool_call_message_chain(stored.messages);
        assert_eq!(repaired.len(), 3);
        assert_eq!(repaired[1].role, "assistant");
        assert_eq!(repaired[2].role, "tool");
        remove_native_client_run(&client_session_id, "run-visible-tool");
        assert!(SESSION_STORE.get(&session_key).is_none());
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn recorded_client_tool_result_replays_from_persisted_history(pool: PgPool) {
        let suffix = Uuid::new_v4().simple().to_string();
        let username = format!("toolhist{}", &suffix[..12]);
        let email = format!("{username}@example.test");
        let user_id: i32 = sqlx::query_scalar!(
            r#"
            INSERT INTO users (email, username, display_name, passhash)
            VALUES ($1, $2, $3, $4)
            RETURNING id
            "#,
            email,
            &username,
            "Tool History Test",
            "test-passhash"
        )
        .fetch_one(&pool)
        .await
        .expect("create user");
        let bear_id = Uuid::new_v4();
        let bear_slug = format!("tool-history-{}", &suffix[..12]);
        sqlx::query!(
            r#"
            INSERT INTO bears (id, slug, name)
            VALUES ($1, $2, $3)
            "#,
            bear_id,
            &bear_slug,
            "Tool History Bear"
        )
        .execute(&pool)
        .await
        .expect("create bear");

        let conversation_id = format!("den-conv-{}", Uuid::new_v4().simple());
        let client_session_id = format!("session-{}", Uuid::new_v4().simple());
        let request_id = Uuid::new_v4().to_string();
        let tool_call_id = "call-persisted-visible";
        let session_key = agent_loop_session_key(
            &conversation_id,
            &client_session_id,
            "run-persisted-visible",
        );
        let context = canonical_persistence_context(
            pool.clone(),
            bear_id,
            Some(user_id),
            conversation_id.clone(),
            Some(client_session_id.clone()),
            Some(request_id.clone()),
            client_session_id.clone(),
            false,
        );
        persist_canonical_conversation_record(
            &context,
            &CanonicalConversationRecord::visible_user_message(
                "Read the plan",
                serde_json::json!({ "event": "user_message", "scope_id": client_session_id }),
                None,
            ),
        )
        .await
        .expect("persist user message");

        SESSION_STORE.insert(AgentLoopSession {
            session_key: session_key.clone(),
            bear_id,
            bear_slug,
            user_id: Some(user_id),
            conversation_id: conversation_id.clone(),
            client_session_id: client_session_id.clone(),
            work_run_id: None,
            checkpoint_audit_context: None,
            workspace_roots: vec!["/workspace".to_string()],
            session_capabilities: vec![],
            recently_discovered_capabilities: vec![],
            request_id: Some(request_id.clone()),
            run_id: Some("run-persisted-visible".to_string()),
            technical_budget_recovery_start_payload: None,
            messages: vec![ChatMessage {
                role: "assistant".to_string(),
                content: None,
                tool_call_id: None,
                name: None,
                tool_calls: Some(vec![ChatToolCall {
                    id: tool_call_id.to_string(),
                    call_type: "function".to_string(),
                    function: crate::llm::ChatToolCallFunction {
                        name: "fs_read_text_file".to_string(),
                        arguments: serde_json::json!({ "path": "/workspace/docs/roadmap/PLAN.md" })
                            .to_string(),
                    },
                }]),
            }],
            tools: Vec::new(),
            budget_components: Default::default(),
            model: "openai/test".to_string(),
            model_request_profile: den_core::ModelRequestProfile {
                approved_model_ref: "openai/test".to_string(),
                ..Default::default()
            },
            model_context_window: None,
            model_max_output_tokens: None,
            model_token_calibration: None,
            bifrost_virtual_key: None,
            api_style: None,
            step: 1,
            turn_budget: pair_turn_budget(),
            turn_budget_state: Default::default(),
            agent_loop_control: test_agent_loop_control(),
            governance: den_core::governance::Governance::Interactive,
            objective_orientation: freeform_orientation(),
            checkpoint_state: Default::default(),
            pending_checkpoint_request: None,
            pending_checkpoint_task_action: None,
            pending_checkpoint_recovery_attempts: 0,
            strategy: StrategyProfile::plain_react(),
            stream_tokens: false,
            key_memory_projection_cache_key: None,
            latest_context_budget: None,
            latest_projected_memory: None,
            latest_recalled_memory: None,
            cached_activity_plan_projection: None,
            profile: BearProfile::Pair,
            overflow_retry_attempted: false,
            overflow_compaction_recovered: false,
        });

        record_native_client_tool_result(
            &pool,
            &conversation_id,
            &client_session_id,
            &request_id,
            Some("run-persisted-visible"),
            tool_call_id,
            None,
            RuntimeToolResultStatus::Ok,
            serde_json::json!({
                "tool_name": "fs_read_text_file",
                "status": "ok",
                "structured_content": { "content": "# BEARS roadmap" }
            })
            .to_string(),
        )
        .await
        .expect("record tool result");
        SESSION_STORE.remove(&session_key);

        let replayed =
            crate::agent_loop::load_transcript_messages(&pool, bear_id, &conversation_id)
                .await
                .expect("load transcript");
        assert_eq!(replayed.len(), 3, "{replayed:#?}");
        assert_eq!(replayed[1].role, "assistant");
        assert_eq!(
            replayed[1]
                .tool_calls
                .as_ref()
                .and_then(|calls| calls.first())
                .map(|call| (call.id.as_str(), call.function.name.as_str())),
            Some((tool_call_id, "fs_read_text_file"))
        );
        assert_eq!(replayed[2].role, "tool");
        assert_eq!(replayed[2].tool_call_id.as_deref(), Some(tool_call_id));
        assert!(replayed[2]
            .content
            .as_deref()
            .unwrap_or_default()
            .contains("BEARS roadmap"));
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn load_history_replays_persisted_tool_call_and_result_records(pool: PgPool) {
        let suffix = Uuid::new_v4().simple().to_string();
        let username = format!("toolhistload{}", &suffix[..8]);
        let email = format!("{username}@example.test");
        let user_id: i32 = sqlx::query_scalar!(
            r#"
            INSERT INTO users (email, username, display_name, passhash)
            VALUES ($1, $2, $3, $4)
            RETURNING id
            "#,
            email,
            &username,
            "Tool Load History Test",
            "test-passhash"
        )
        .fetch_one(&pool)
        .await
        .expect("create user");
        let bear_id = Uuid::new_v4();
        let bear_slug = format!("tool-load-history-{}", &suffix[..8]);
        sqlx::query!(
            r#"
            INSERT INTO bears (id, slug, name)
            VALUES ($1, $2, $3)
            "#,
            bear_id,
            &bear_slug,
            "Tool Load History Bear"
        )
        .execute(&pool)
        .await
        .expect("create bear");

        let conversation_id = format!("den-conv-{}", Uuid::new_v4().simple());
        let client_session_id = format!("session-{}", Uuid::new_v4().simple());
        let request_id = Uuid::new_v4().to_string();
        let tool_call_id = "call-load-history";
        let context = canonical_persistence_context(
            pool.clone(),
            bear_id,
            Some(user_id),
            conversation_id.clone(),
            Some(client_session_id.clone()),
            Some(request_id.clone()),
            client_session_id.clone(),
            false,
        );
        persist_canonical_conversation_record(
            &context,
            &CanonicalConversationRecord::visible_user_message(
                "Read the plan",
                serde_json::json!({ "event": "user_message", "scope_id": client_session_id }),
                None,
            ),
        )
        .await
        .expect("persist user message");
        let session_key =
            agent_loop_session_key(&conversation_id, &client_session_id, "run-load-history");
        SESSION_STORE.insert(AgentLoopSession {
            session_key: session_key.clone(),
            bear_id,
            bear_slug,
            user_id: Some(user_id),
            conversation_id: conversation_id.clone(),
            client_session_id: client_session_id.clone(),
            work_run_id: None,
            checkpoint_audit_context: None,
            workspace_roots: vec!["/workspace".to_string()],
            session_capabilities: vec![],
            recently_discovered_capabilities: vec![],
            request_id: Some(request_id.clone()),
            run_id: Some("run-load-history".to_string()),
            technical_budget_recovery_start_payload: None,
            messages: vec![ChatMessage {
                role: "assistant".to_string(),
                content: None,
                tool_call_id: None,
                name: None,
                tool_calls: Some(vec![ChatToolCall {
                    id: tool_call_id.to_string(),
                    call_type: "function".to_string(),
                    function: crate::llm::ChatToolCallFunction {
                        name: "fs_read_text_file".to_string(),
                        arguments: serde_json::json!({ "path": "/workspace/docs/roadmap/PLAN.md" })
                            .to_string(),
                    },
                }]),
            }],
            tools: Vec::new(),
            budget_components: Default::default(),
            model: "openai/test".to_string(),
            model_request_profile: den_core::ModelRequestProfile {
                approved_model_ref: "openai/test".to_string(),
                ..Default::default()
            },
            model_context_window: None,
            model_max_output_tokens: None,
            model_token_calibration: None,
            bifrost_virtual_key: None,
            api_style: None,
            step: 1,
            turn_budget: pair_turn_budget(),
            turn_budget_state: Default::default(),
            agent_loop_control: test_agent_loop_control(),
            governance: den_core::governance::Governance::Interactive,
            objective_orientation: freeform_orientation(),
            checkpoint_state: Default::default(),
            pending_checkpoint_request: None,
            pending_checkpoint_task_action: None,
            pending_checkpoint_recovery_attempts: 0,
            strategy: StrategyProfile::plain_react(),
            stream_tokens: false,
            key_memory_projection_cache_key: None,
            latest_context_budget: None,
            latest_projected_memory: None,
            latest_recalled_memory: None,
            cached_activity_plan_projection: None,
            profile: BearProfile::Pair,
            overflow_retry_attempted: false,
            overflow_compaction_recovered: false,
        });

        record_native_client_tool_result(
            &pool,
            &conversation_id,
            &client_session_id,
            &request_id,
            Some("run-load-history"),
            tool_call_id,
            None,
            RuntimeToolResultStatus::Ok,
            serde_json::json!({
                "tool_name": "fs_read_text_file",
                "status": "ok",
                "structured_content": { "content": "# BEARS roadmap" }
            })
            .to_string(),
        )
        .await
        .expect("record tool result");
        SESSION_STORE.remove(&session_key);

        let canonical = conversation_persistence::get_conversation_for_external_id(
            &pool,
            bear_id,
            &conversation_id,
        )
        .await
        .expect("load canonical conversation")
        .expect("canonical conversation exists");
        let rows = conversation_persistence::list_messages_page(&pool, canonical.id, None, 10)
            .await
            .expect("list persisted rows");
        assert!(
            rows.iter().any(|row| row.message_type == "tool_call"),
            "expected persisted tool_call row, got {rows:#?}"
        );
        assert!(
            rows.iter().any(|row| row.message_type == "tool_result"),
            "expected persisted tool_result row, got {rows:#?}"
        );
        let persisted_tool_result = rows
            .iter()
            .find(|row| row.message_type == "tool_result")
            .expect("persisted tool_result row");
        assert_eq!(
            persisted_tool_result.content_json["output_summary"],
            serde_json::json!("Used fs_read_text_file (ok): {\"status\":\"ok\",\"structured_content\":{\"content\":\"# BEARS roadmap\"},\"tool_name\":\"fs_read_text_file\"}")
        );
        assert!(
            rows.iter().any(|row| matches!(
                row.to_model_transcript_record(),
                Some(PersistedTranscriptRecord::ToolCall { .. })
            )),
            "expected tool_call transcript projection, got {rows:#?}"
        );
        assert!(
            rows.iter().any(|row| matches!(
                row.to_model_transcript_record(),
                Some(PersistedTranscriptRecord::ToolResult { .. })
            )),
            "expected tool_result transcript projection, got {rows:#?}"
        );

        let backend = NativeRuntimeConversationBackend::with_pool(pool);
        let binding = RoleRuntimeBinding {
            binding_id: format!("den-native:{bear_id}:pair"),
            compatibility_backend: Some("native".to_string()),
        };
        let history = backend
            .load_history(
                &binding,
                &RuntimeConversationRef {
                    id: conversation_id,
                },
            )
            .await
            .expect("load history");

        assert_eq!(history.records.len(), 3, "{history:#?}");
        assert!(matches!(
            &history.records[0],
            RuntimeHistoryRecord::Message { role, content, .. }
            if role == "user" && content == "Read the plan"
        ));
        assert!(matches!(
            &history.records[1],
            RuntimeHistoryRecord::ToolCall {
                tool_call_id: record_tool_call_id,
                tool_name,
                arguments,
                ..
            }
            if record_tool_call_id == tool_call_id
                && tool_name == "fs_read_text_file"
                && arguments.get("path").and_then(serde_json::Value::as_str) == Some("/workspace/docs/roadmap/PLAN.md")
        ));
        assert!(matches!(
            &history.records[2],
            RuntimeHistoryRecord::ToolResult {
                tool_call_id: Some(record_tool_call_id),
                tool_name: Some(tool_name),
                status: Some(status),
                content: Some(content),
                ..
            }
            if record_tool_call_id == tool_call_id
                && tool_name == "fs_read_text_file"
                && status == "ok"
                && content.contains("BEARS roadmap")
        ));
    }

    #[test]
    fn prompt_for_model_leaves_plain_prompt_unchanged_without_host_context() {
        assert_eq!(
            prompt_for_model("Please continue.", None),
            "Please continue."
        );
    }

    #[test]
    fn work_without_active_task_list_does_not_use_task_list_terminal_gate() {
        assert!(crate::runtime::turn_state::should_allow_terminal_response(
            BearProfile::Work,
            None,
            "What I changed: added one test. Remaining work: more later."
        ));
    }
}
