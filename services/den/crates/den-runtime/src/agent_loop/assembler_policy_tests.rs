use super::*;
use den_core::{ArmatureAvailability, BearCapability, Governance, TurnExecutionOrigin};

#[tokio::test]
async fn assembly_task_authority_uses_origin_and_governance_not_session_hints() {
    let pool = PgPool::connect_lazy("postgres://unused:unused@localhost/unused").unwrap();
    let config = Config::test_stub();
    let stores = MemoryStoreManager::new(&config);
    let mut ctx = AssembleTurnContext {
        pool: &pool,
        config: &config,
        stores: &stores,
        bear_id: Uuid::nil(),
        origin: TurnExecutionOrigin::ChannelConversation,
        governance: Governance::Interactive,
        conversation_id: "untrusted-conversation",
        turn_runtime_context: None,
        human_message: None,
        tool_messages: &[],
        session_id: Some("claimed-client"),
        workspace_roots: None,
        runtime_target: None,
        conversation_selection: None,
        user_id: Some(1),
        client_context: None,
        include_prompt_memory: false,
        key_memory_cache: None,
        native_runtime: true,
    };
    for origin in [
        TurnExecutionOrigin::InternalCuration,
        TurnExecutionOrigin::InboundObservation,
    ] {
        ctx.origin = origin;
        assert!(matches!(
            resolve_memory_projection_scope(&ctx).await,
            Err(DenError::Authorization(_))
        ));
    }
    for (origin, owns_tasks) in [
        (TurnExecutionOrigin::ChannelConversation, false),
        (TurnExecutionOrigin::BrowserTaskSession, true),
        (
            TurnExecutionOrigin::ArmatureConversation(ArmatureAvailability::Connected),
            true,
        ),
        (
            TurnExecutionOrigin::ArmatureConversation(ArmatureAvailability::Absent),
            true,
        ),
        (
            TurnExecutionOrigin::AuthorizedWorkRun(ArmatureAvailability::Connected),
            false,
        ),
        (TurnExecutionOrigin::InternalCuration, false),
        (TurnExecutionOrigin::InboundObservation, false),
    ] {
        ctx.origin = origin;
        for governance in [
            Governance::Interactive,
            Governance::AutonomousContinuation,
            Governance::Observational,
            Governance::Frozen,
        ] {
            ctx.governance = governance;
            let expected =
                owns_tasks && !matches!(governance, Governance::Observational | Governance::Frozen);
            for session_id in [Some("claimed-client"), None] {
                ctx.session_id = session_id;
                assert_eq!(
                    ctx.policy()
                        .capabilities
                        .contains(BearCapability::OwnSessionTasks),
                    expected,
                    "{origin:?}/{governance:?}/{session_id:?}"
                );
            }
        }
    }
}
