use super::*;
use den_core::{ArmatureAvailability, BearCapability, Governance, TurnExecutionOrigin};

fn supplied_context() -> DenToolInvocationContext {
    serde_json::from_value(json!({
        "bear_id": Uuid::new_v4(),
        "bear_slug": "origin-test",
        "binding_id": "den-native:untrusted:pair",
        "profile": "pair",
        "user_id": 1,
        "conversation_id": "untrusted-conversation",
        "session_id": "untrusted-session",
        "client_session_id": "claimed-client-session",
        "channel": {}
    }))
    .unwrap()
}

#[test]
fn native_workflow_capabilities_follow_verified_origin_and_governance() {
    let origins = [
        TurnExecutionOrigin::ChannelConversation,
        TurnExecutionOrigin::BrowserTaskSession,
        TurnExecutionOrigin::ArmatureConversation(ArmatureAvailability::Connected),
        TurnExecutionOrigin::ArmatureConversation(ArmatureAvailability::Absent),
        TurnExecutionOrigin::AuthorizedWorkRun(ArmatureAvailability::Connected),
        TurnExecutionOrigin::AuthorizedWorkRun(ArmatureAvailability::Absent),
        TurnExecutionOrigin::InternalCuration,
        TurnExecutionOrigin::InboundObservation,
    ];
    for origin in origins {
        for governance in [
            Governance::Interactive,
            Governance::Grace,
            Governance::AutonomousContinuation,
            Governance::Observational,
            Governance::Frozen,
        ] {
            let actual = effective_tool_policy(WorkflowAuthority { origin, governance });
            assert_eq!(
                actual,
                den_core::EffectivePolicy::compile_for_origin(origin, governance),
                "{origin:?}, {governance:?}"
            );
        }
    }
    assert!(!effective_tool_policy(WorkflowAuthority {
        origin: TurnExecutionOrigin::BrowserTaskSession,
        governance: Governance::Interactive,
    })
    .capabilities
    .contains(BearCapability::UseArmatureTools));
}

#[tokio::test]
async fn native_effects_deny_work_dispatch_and_session_tasks_before_touching_storage() {
    let pool = PgPool::connect_lazy("postgres://unused:unused@localhost/unused").unwrap();
    let context = supplied_context();
    let work = WorkflowAuthority {
        origin: TurnExecutionOrigin::AuthorizedWorkRun(ArmatureAvailability::Absent),
        governance: Governance::Interactive,
    };
    let denied_dispatch = cancel_job_run(&pool, &context, work, json!({"job_id": Uuid::new_v4()}))
        .await
        .unwrap_err();
    assert!(
        denied_dispatch.to_string().contains("DispatchWork"),
        "{denied_dispatch}"
    );
    let denied_question = append_docket_entry(
        &pool,
        &context,
        RuntimeContextLabel::ArmatureConversation,
        work,
        json!({"job_id": Uuid::new_v4(), "scope": "job_notebook", "kind": "question", "summary": "untrusted question"}),
    )
    .await
    .unwrap_err();
    assert!(
        denied_question.to_string().contains("OwnSessionTasks"),
        "{denied_question}"
    );
    let denied_session_task =
        select_current_task(&pool, &context, work, json!({"task_id": Uuid::new_v4()}))
            .await
            .unwrap_err();
    assert!(
        denied_session_task
            .to_string()
            .contains("SelectSessionTask"),
        "{denied_session_task}"
    );

    let autonomous = WorkflowAuthority {
        origin: TurnExecutionOrigin::ArmatureConversation(ArmatureAvailability::Connected),
        governance: Governance::AutonomousContinuation,
    };
    let denied_autonomous_dispatch = cancel_job_run(
        &pool,
        &context,
        autonomous,
        json!({"job_id": Uuid::new_v4()}),
    )
    .await
    .unwrap_err();
    assert!(
        denied_autonomous_dispatch
            .to_string()
            .contains("DispatchWork"),
        "{denied_autonomous_dispatch}"
    );
}
