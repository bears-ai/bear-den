use super::*;
use crate::agent_loop::{FreeformPolicy, StrategyProfile, TurnBudgetState};
use den_core::TurnExecutionOrigin;
use den_protocol::RuntimeApprovalDecision;
use sqlx::postgres::PgPoolOptions;
use std::time::Duration;

const INTERNAL_ORIGINS: [TurnExecutionOrigin; 2] = [
    TurnExecutionOrigin::InternalCuration,
    TurnExecutionOrigin::InboundObservation,
];

fn unreachable_pool() -> PgPool {
    PgPoolOptions::new()
        .acquire_timeout(Duration::from_millis(100))
        .connect_lazy("postgres://postgres:postgres@127.0.0.1:1/unused")
        .expect("lazy unreachable pool")
}

fn assert_internal_denial<T>(result: Result<T, DenError>) {
    match result {
        Err(DenError::Authorization(message)) => assert_eq!(
            message,
            "system execution requires a dedicated source-verified operation"
        ),
        Err(error) => panic!("expected pre-DB internal-origin denial, got {error:?}"),
        Ok(_) => panic!("generic runtime route accepted an internal origin"),
    }
}

struct StoredInternalSession(AgentLoopSession);

impl StoredInternalSession {
    fn new(origin: TurnExecutionOrigin) -> Self {
        let conversation_id = format!("den-conv-{}", Uuid::new_v4().simple());
        let client_session_id = Uuid::new_v4().to_string();
        let run_id = Uuid::new_v4().to_string();
        let profile = den_core::EffectivePolicy::compile_for_origin(
            origin,
            den_core::Governance::Interactive,
        )
        .trust_profile;
        let agent_loop_control = resolve_agent_loop_control(AgentLoopControlResolutionInput {
            model_handle: Some("openai/test"),
            model_default: None,
            bear_override: None,
            stance_override: None,
            task_escalation: None,
            stance: Some(profile),
            objective_orientation: None,
            pre_risk: false,
        });
        let session = AgentLoopSession {
            session_key: agent_loop_session_key(&conversation_id, &client_session_id, &run_id),
            bear_id: Uuid::new_v4(),
            bear_slug: "internal-route-test".into(),
            user_id: Some(1),
            conversation_id,
            client_session_id,
            work_run_id: None,
            origin,
            checkpoint_audit_context: None,
            workspace_roots: vec![],
            session_capabilities: vec![],
            recently_discovered_capabilities: vec![],
            request_id: Some(Uuid::new_v4().to_string()),
            run_id: Some(run_id),
            technical_budget_recovery_start_payload: None,
            messages: vec![
                ChatMessage {
                    role: "user".into(),
                    content: Some("Internal operation transcript".into()),
                    tool_call_id: None,
                    name: None,
                    tool_calls: None,
                },
                ChatMessage {
                    role: "assistant".into(),
                    content: None,
                    tool_call_id: None,
                    name: None,
                    tool_calls: Some(vec![ChatToolCall {
                        id: Uuid::new_v4().to_string(),
                        call_type: "function".into(),
                        function: crate::llm::ChatToolCallFunction {
                            name: "fs_read_text_file".into(),
                            arguments: "{}".into(),
                        },
                    }]),
                },
            ],
            tools: vec![],
            budget_components: Default::default(),
            model: "openai/test".into(),
            model_request_profile: den_core::ModelRequestProfile {
                approved_model_ref: "openai/test".into(),
                ..Default::default()
            },
            model_context_window: None,
            model_max_output_tokens: None,
            model_token_calibration: None,
            bifrost_virtual_key: None,
            api_style: None,
            step: 2,
            turn_budget: agent_loop_control.profile.budget,
            turn_budget_state: TurnBudgetState {
                consecutive_tool_failures: 1,
                last_batch_signature: Some("original-batch".into()),
                same_batch_signature_repeats: 1,
                ..Default::default()
            },
            agent_loop_control,
            governance: den_core::Governance::Interactive,
            objective_orientation: ObjectiveOrientation::Freeform {
                policy: FreeformPolicy::closed(),
            },
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
            profile,
            overflow_retry_attempted: false,
            overflow_compaction_recovered: false,
        };
        SESSION_STORE.insert(session.clone());
        Self(session)
    }

    fn tool_call_id(&self) -> String {
        self.0.messages[1].tool_calls.as_ref().unwrap()[0]
            .id
            .clone()
    }

    fn assert_unchanged(&self) {
        let stored = SESSION_STORE
            .get(&self.0.session_key)
            .expect("denied operation must retain the stored session");
        assert_eq!(
            serde_json::to_value(&stored.messages).unwrap(),
            serde_json::to_value(&self.0.messages).unwrap(),
            "denial must not append or replace transcript messages"
        );
        assert_eq!(stored.turn_budget, self.0.turn_budget);
        assert_eq!(stored.turn_budget_state, self.0.turn_budget_state);
        assert_eq!(stored.step, self.0.step);
        assert_eq!(stored.request_id, self.0.request_id);
        assert_eq!(stored.run_id, self.0.run_id);
        assert_eq!(
            format!("{stored:?}"),
            format!("{:?}", self.0),
            "denial must leave all other session state unchanged"
        );
    }
}

impl Drop for StoredInternalSession {
    fn drop(&mut self) {
        SESSION_STORE.remove(&self.0.session_key);
    }
}

#[tokio::test]
async fn internal_origins_cannot_start_generic_turns_before_materialization() {
    let pool = unreachable_pool();
    let config = Config::test_stub();
    let stores = MemoryStoreManager::new(&config);
    for origin in INTERNAL_ORIGINS {
        let session_id = Uuid::new_v4().to_string();
        let run_id = Uuid::new_v4().to_string();
        let selection = format!("new-{}", Uuid::new_v4().simple());
        let bear_id = Uuid::new_v4();
        let binding = RoleRuntimeBinding {
            binding_id: NativeTurnSource::Conversation(Uuid::new_v4())
                .binding_id(BearId::new(bear_id)),
            compatibility_backend: Some("native".into()),
        };
        // A pending new conversation forces materialization to reach the pool
        // if the origin guard is ever moved below it.
        let result = start_native_turn_event_stream(
            TurnStartRequest {
                sqlx_pool: &pool,
                config: &config,
                memory_stores: &stores,
                request_id: Uuid::new_v4(),
                run_id: Some(&run_id),
                checkpoint_audit_context: None,
                user_id: 1,
                session_id: &session_id,
                bear_id,
                bear_slug: "internal-route-test",
                client: "bear-armature",
                cwd: None,
                workspace_roots: None,
                binding: &binding,
                conversation_selection: &selection,
                upstream_target: &selection,
                prompt: "Do not start a generic internal turn",
                prompt_context: None,
                client_tools: None,
                runtime_context: None,
                runtime_context_len: 0,
                technical_budget_recovery_start_payload: None,
                stream_tokens: false,
                api_style: None,
                supports_reasoning_effort: None,
            },
            origin,
        )
        .await;
        assert_internal_denial(result);
        assert_eq!(pool.size(), 0);
    }
}

#[tokio::test]
async fn internal_origins_cannot_continue_or_mutate_stored_sessions() {
    let pool = unreachable_pool();
    let config = Config::test_stub();
    let stores = MemoryStoreManager::new(&config);
    for origin in INTERNAL_ORIGINS {
        let fixture = StoredInternalSession::new(origin);
        let session = &fixture.0;
        let binding = RoleRuntimeBinding {
            binding_id: NativeTurnSource::Conversation(Uuid::new_v4())
                .binding_id(BearId::new(session.bear_id)),
            compatibility_backend: Some("native".into()),
        };
        let tool_call_id = fixture.tool_call_id();
        let approval_request_id = Uuid::new_v4().to_string();
        let continuations = [
            RuntimeContinuation::ToolResult {
                tool_call_id: tool_call_id.clone(),
                approval_request_id: None,
                status: RuntimeToolResultStatus::Ok,
                content: "must not be appended".into(),
            },
            RuntimeContinuation::ToolResult {
                tool_call_id: tool_call_id.clone(),
                approval_request_id: Some(approval_request_id.clone()),
                status: RuntimeToolResultStatus::Ok,
                content: "must not record approval or append".into(),
            },
            RuntimeContinuation::ApprovalDecision {
                approval_request_id: approval_request_id.clone(),
                tool_call_id: Some(tool_call_id.clone()),
                decision: RuntimeApprovalDecision::Approve,
                reason: None,
            },
            RuntimeContinuation::ApprovalDecision {
                approval_request_id,
                tool_call_id: Some(tool_call_id),
                decision: RuntimeApprovalDecision::Deny,
                reason: Some("must not be appended".into()),
            },
            RuntimeContinuation::DocketBoundedSlice,
        ];
        for continuation in continuations {
            let result = continue_native_client_turn_event_stream(TurnContinueRequest {
                sqlx_pool: &pool,
                config: &config,
                memory_stores: &stores,
                request_id: Uuid::new_v4(),
                run_id: session.run_id.as_deref(),
                client_session_id: &session.client_session_id,
                conversation: RuntimeConversationRef {
                    id: session.conversation_id.clone(),
                },
                binding: &binding,
                continuation,
                stream_context: crate::turn_runner::default_tool_continue_stream_context(),
            })
            .await;
            assert_internal_denial(result);
            fixture.assert_unchanged();
            assert_eq!(pool.size(), 0);
        }
    }
}

#[tokio::test]
async fn internal_tool_results_are_denied_before_approval_persistence_or_transcript_mutation() {
    let pool = unreachable_pool();
    for origin in INTERNAL_ORIGINS {
        let fixture = StoredInternalSession::new(origin);
        let session = &fixture.0;
        for status in [RuntimeToolResultStatus::Ok, RuntimeToolResultStatus::Error] {
            let result = record_native_client_tool_result(
                &pool,
                &session.conversation_id,
                &session.client_session_id,
                &Uuid::new_v4().to_string(),
                session.run_id.as_deref(),
                &fixture.tool_call_id(),
                Some(&Uuid::new_v4().to_string()),
                status,
                "must not record approval or append".into(),
            )
            .await;
            assert_internal_denial(result);
            fixture.assert_unchanged();
            assert_eq!(pool.size(), 0);
        }
    }
}
