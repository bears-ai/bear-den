use super::*;
use den_core::{
    ArmatureAvailability, EffectivePolicy, Governance, RuntimeContextLabel, TurnExecutionOrigin,
};
use den_docket::{TaskListItem, TaskListItemStatus, TaskListSourceRef, TaskListSyncState};
use sqlx::{postgres::PgPoolOptions, types::time::OffsetDateTime};
use std::time::Duration;

fn unreachable_pool() -> PgPool {
    PgPoolOptions::new()
        .acquire_timeout(Duration::from_millis(20))
        .connect_lazy("postgres://unused:unused@127.0.0.1:1/unreachable")
        .expect("lazy unreachable pool")
}

fn request(policy: EffectivePolicy) -> RuntimeTaskResolveRequest {
    let bear_id = Uuid::new_v4();
    let conversation_id = "owner-conversation".to_string();
    let client_session_id = "owner-session".to_string();
    let item = TaskListItem {
        id: Uuid::new_v4().to_string(),
        title: "Cached selected task must not confer authority".into(),
        summary: None,
        status: TaskListItemStatus::Pending,
        blocked_reason: None,
        source_ref: TaskListSourceRef::local(vec![]),
        sync_state: TaskListSyncState::CheckedOut,
    };
    RuntimeTaskResolveRequest {
        bear_id,
        policy,
        user_id: Some(42),
        conversation_id: conversation_id.clone(),
        client_session_id: client_session_id.clone(),
        cached_activity_plan_projection: Some(TaskListProjection {
            id: Uuid::new_v4(),
            bear_id,
            title: "Cached session tasks".into(),
            summary: String::new(),
            owner_profile: "pair".into(),
            visibility: "private_to_profile".into(),
            status: "active".into(),
            version: 1,
            source_ref: TaskListSourceRef::local(vec![]),
            items: vec![item.clone()],
            current_item: Some(item),
            source_conversation_id: Some(conversation_id),
            source_client_session_id: Some(client_session_id),
            handoff_intent_path: None,
            handoff_task_id: None,
            created_at: OffsetDateTime::UNIX_EPOCH,
            updated_at: OffsetDateTime::UNIX_EPOCH,
        }),
    }
}

fn assert_no_projection(context: RuntimeTaskContext) {
    assert_eq!(context.source, RuntimeTaskSource::None);
    assert!(context.current_task_id.is_none());
    assert!(context.cached_activity_plan_projection.is_none());
    assert!(context.active_activity_plan().is_none());
    assert!(context.focused_orientation().is_none());
}

#[tokio::test]
async fn absent_session_task_capability_never_queries_or_borrows_cached_focus() {
    let pool = unreachable_pool();
    for origin in [
        TurnExecutionOrigin::ChannelConversation,
        TurnExecutionOrigin::AuthorizedWorkRun(ArmatureAvailability::Connected),
        TurnExecutionOrigin::AuthorizedWorkRun(ArmatureAvailability::Absent),
        TurnExecutionOrigin::InternalCuration,
        TurnExecutionOrigin::InboundObservation,
        TurnExecutionOrigin::ArmatureConversation(ArmatureAvailability::Connected),
        TurnExecutionOrigin::BrowserTaskSession,
    ] {
        for governance in [
            Governance::Interactive,
            Governance::AutonomousContinuation,
            Governance::Frozen,
            Governance::Observational,
        ] {
            let policy = EffectivePolicy::compile_for_origin(origin, governance);
            if matches!(
                origin,
                TurnExecutionOrigin::ArmatureConversation(_)
                    | TurnExecutionOrigin::BrowserTaskSession
            ) && matches!(
                governance,
                Governance::Interactive | Governance::AutonomousContinuation
            ) {
                continue;
            }
            assert!(!policy
                .capabilities
                .contains(BearCapability::OwnSessionTasks));
            for annotation in [
                policy.context_label,
                RuntimeContextLabel::ArmatureConversation,
                RuntimeContextLabel::JobRun,
            ] {
                for cached in [false, true] {
                    let mut request = request(policy.clone());
                    request.policy.context_label = annotation;
                    if !cached {
                        request.cached_activity_plan_projection = None;
                    }
                    let context = resolve_runtime_task_context(&pool, request)
                        .await
                        .unwrap_or_else(|error| {
                            panic!("{origin:?}/{governance:?} must not query the DB: {error}")
                        });
                    assert_no_projection(context);
                }
            }
        }
    }
}

#[tokio::test]
async fn editor_and_browser_session_authority_requires_durable_owner_session_lookup() {
    let pool = unreachable_pool();
    for origin in [
        TurnExecutionOrigin::ArmatureConversation(ArmatureAvailability::Connected),
        TurnExecutionOrigin::ArmatureConversation(ArmatureAvailability::Absent),
        TurnExecutionOrigin::BrowserTaskSession,
    ] {
        for governance in [Governance::Interactive, Governance::AutonomousContinuation] {
            let policy = EffectivePolicy::compile_for_origin(origin, governance);
            assert!(policy
                .capabilities
                .contains(BearCapability::OwnSessionTasks));
            let mut request = request(policy);
            // Annotation cannot revoke authority, nor can a cache mask DB failure.
            request.policy.context_label = RuntimeContextLabel::Observation;
            let result = resolve_runtime_task_context(&pool, request).await;
            assert!(
                matches!(result, Err(DenError::DatabaseUnavailable(_))),
                "{origin:?}/{governance:?} must query the existing owner session path: {result:?}"
            );
        }
    }
}

#[tokio::test]
async fn session_task_authority_without_an_owner_does_not_borrow_cached_focus() {
    let pool = unreachable_pool();
    let mut request = request(EffectivePolicy::compile_for_origin(
        TurnExecutionOrigin::BrowserTaskSession,
        Governance::Interactive,
    ));
    request.user_id = None;
    assert_no_projection(
        resolve_runtime_task_context(&pool, request)
            .await
            .expect("no owner requires no DB lookup"),
    );
}
