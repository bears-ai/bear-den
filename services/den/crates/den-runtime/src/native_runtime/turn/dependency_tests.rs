use super::*;
use crate::agent_loop::source_admission::tests::{step_context, test_session};
use den_core::{ArmatureAvailability, TurnExecutionOrigin};
use den_memory::personal_facts::PersonalFactId;
use den_protocol::RuntimeApprovalDecision;
use sqlx::postgres::PgPoolOptions;

const ORDINARY_ORIGINS: [TurnExecutionOrigin; 6] = [
    TurnExecutionOrigin::ChannelConversation,
    TurnExecutionOrigin::BrowserTaskSession,
    TurnExecutionOrigin::ArmatureConversation(ArmatureAvailability::Connected),
    TurnExecutionOrigin::ArmatureConversation(ArmatureAvailability::Absent),
    TurnExecutionOrigin::AuthorizedWorkRun(ArmatureAvailability::Connected),
    TurnExecutionOrigin::AuthorizedWorkRun(ArmatureAvailability::Absent),
];

struct StoredProtectedSession(AgentLoopSession);

impl StoredProtectedSession {
    fn new(origin: TurnExecutionOrigin) -> Self {
        let mut session = test_session(
            Uuid::new_v4(),
            1,
            &format!("den-conv-{}", Uuid::new_v4().simple()),
            &Uuid::new_v4().to_string(),
            &Uuid::new_v4().to_string(),
        );
        session.origin = origin;
        session.profile =
            den_core::EffectivePolicy::compile_for_origin(origin, session.governance).context_label;
        session.work_run_id =
            matches!(origin, TurnExecutionOrigin::AuthorizedWorkRun(_)).then(Uuid::new_v4);
        session.context_dependencies = ContextDependencies::personal_fact(
            session.bear_id.into(),
            PersonalFactId(Uuid::new_v4()),
        );
        session.messages.push(ChatMessage {
            role: "assistant".into(),
            content: None,
            tool_call_id: None,
            name: None,
            tool_calls: Some(vec![ChatToolCall {
                id: "protected-call".into(),
                call_type: "function".into(),
                function: crate::llm::ChatToolCallFunction {
                    name: "web_fetch".into(),
                    arguments: "{}".into(),
                },
            }]),
        });
        // A terminal stop must not admit reuse or mutate a protected session.
        session.step = session.turn_budget.emergency_hard_steps;
        SESSION_STORE.insert(session.clone());
        Self(session)
    }

    fn assert_denied<T>(&self, result: Result<T, DenError>) {
        match result {
            Err(error @ DenError::Authorization(_)) => assert_eq!(
                error.to_string(),
                self.0
                    .context_dependencies
                    .require_unrestricted_reuse()
                    .unwrap_err()
                    .to_string()
            ),
            Err(error) => panic!("expected dependency denial before I/O, got {error:?}"),
            Ok(_) => panic!("protected context was reused"),
        }
        let stored = SESSION_STORE.get(&self.0.session_key).unwrap();
        assert_eq!(stored.context_dependencies, self.0.context_dependencies);
        assert_eq!(
            format!("{stored:?}"),
            format!("{:?}", self.0),
            "denial must not mutate transcript, budget, request, or other session state"
        );
    }

    fn binding(&self) -> RoleRuntimeBinding {
        let source = match self.0.work_run_id {
            Some(run) => NativeTurnSource::WorkRun(run),
            None => NativeTurnSource::Conversation(Uuid::new_v4()),
        };
        RoleRuntimeBinding {
            binding_id: source.binding_id(self.0.bear_id.into()),
            compatibility_backend: Some("native".into()),
        }
    }
}

impl Drop for StoredProtectedSession {
    fn drop(&mut self) {
        SESSION_STORE.remove(&self.0.session_key);
    }
}

async fn closed_pool() -> PgPool {
    let pool = PgPoolOptions::new()
        .connect_lazy("postgres://postgres:postgres@127.0.0.1:1/unused")
        .unwrap();
    pool.close().await;
    pool
}

#[tokio::test]
async fn protected_continuations_are_denied_before_approval_budget_or_transcript_effects() {
    let pool = closed_pool().await;
    let config = Config::test_stub();
    let stores = MemoryStoreManager::new(&config);
    for origin in ORDINARY_ORIGINS {
        let fixture = StoredProtectedSession::new(origin);
        let session = &fixture.0;
        let binding = fixture.binding();
        for continuation in [
            RuntimeContinuation::ToolResult {
                tool_call_id: "protected-call".into(),
                approval_request_id: None,
                status: RuntimeToolResultStatus::Ok,
                content: "must not enter context".into(),
            },
            RuntimeContinuation::ToolResult {
                tool_call_id: "protected-call".into(),
                approval_request_id: Some("must-not-touch-approval".into()),
                status: RuntimeToolResultStatus::Ok,
                content: "must not enter context".into(),
            },
            RuntimeContinuation::ApprovalDecision {
                approval_request_id: "must-not-touch-approval".into(),
                tool_call_id: Some("protected-call".into()),
                decision: RuntimeApprovalDecision::Approve,
                reason: None,
            },
            RuntimeContinuation::ApprovalDecision {
                approval_request_id: "must-not-touch-approval".into(),
                tool_call_id: Some("protected-call".into()),
                decision: RuntimeApprovalDecision::Deny,
                reason: None,
            },
            RuntimeContinuation::DocketBoundedSlice,
        ] {
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
            fixture.assert_denied(result);
        }
    }
}

#[tokio::test]
async fn protected_result_recording_is_denied_before_approval_or_transcript_mutation() {
    let pool = closed_pool().await;
    for origin in ORDINARY_ORIGINS {
        let fixture = StoredProtectedSession::new(origin);
        let session = &fixture.0;
        for status in [
            RuntimeToolResultStatus::Ok,
            RuntimeToolResultStatus::Error,
            RuntimeToolResultStatus::Timeout,
        ] {
            for approval in [None, Some("must-not-touch-approval")] {
                fixture.assert_denied(
                    record_native_client_tool_result(
                        &pool,
                        &session.conversation_id,
                        &session.client_session_id,
                        "must-not-replace-request",
                        session.run_id.as_deref(),
                        "protected-call",
                        approval,
                        status.clone(),
                        "must not enter context".into(),
                    )
                    .await,
                );
            }
        }
    }
}

#[tokio::test]
async fn protected_next_step_and_direct_approved_tool_are_denied_at_shared_boundary() {
    let pool = closed_pool().await;
    let config = Config::test_stub();
    let stores = MemoryStoreManager::new(&config);
    for origin in ORDINARY_ORIGINS {
        let fixture = StoredProtectedSession::new(origin);
        let session = &fixture.0;
        let context = step_context(&pool, session);
        fixture.assert_denied(
            run_agent_step_stream(&LlmClient::new(&config), session, Some(context.clone())).await,
        );
        assert_eq!(
            format!(
                "{:?}",
                context.session_store.get(&session.session_key).unwrap()
            ),
            format!("{session:?}")
        );
        let binding = fixture.binding();
        let request = TurnContinueRequest {
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
            continuation: RuntimeContinuation::ApprovalDecision {
                approval_request_id: "must-not-touch-approval".into(),
                tool_call_id: Some("protected-call".into()),
                decision: RuntimeApprovalDecision::Approve,
                reason: None,
            },
            stream_context: crate::turn_runner::default_tool_continue_stream_context(),
        };
        let call = session.find_pending_tool_call("protected-call").unwrap();
        fixture.assert_denied(
            execute_approved_den_tool_for_session(&request, session, &call, session.profile).await,
        );
    }
}
