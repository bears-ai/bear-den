use super::tests::{pending_task_list_projection, test_session, test_tracking_stream_with_session};
use super::*;
use crate::runtime::task_context::RuntimeTaskSource;

#[tokio::test]
async fn final_gate_denies_restricted_governance_and_channel_even_with_forged_labels() {
    let editor = TurnExecutionOrigin::ArmatureConversation(ArmatureAvailability::Connected);
    let work = TurnExecutionOrigin::AuthorizedWorkRun(ArmatureAvailability::Absent);
    for (origin, governance) in [
        (editor, Governance::Observational),
        (editor, Governance::Frozen),
        (work, Governance::Observational),
        (work, Governance::Frozen),
        (
            TurnExecutionOrigin::ChannelConversation,
            Governance::Interactive,
        ),
    ] {
        for profile in [
            RuntimeContextLabel::ArmatureConversation,
            RuntimeContextLabel::JobRun,
        ] {
            for owner_label in ["pair", "work", "forged-owner"] {
                let mut session = test_session("focused-denial:client-test", Uuid::new_v4());
                session.origin = origin;
                session.governance = governance;
                session.profile = profile;
                let mut focused = pending_task_list_projection();
                focused.owner_profile = owner_label.into();
                session.cached_activity_plan_projection = Some(focused.clone());
                let mut stream = test_tracking_stream_with_session(&session);
                stream.assistant_text = "Remaining work; stopping here.".into();
                stream.evaluate_final_gate_or_complete(Some(focused), None);

                assert!(stream.finished, "{origin:?}/{governance:?}/{owner_label}");
                assert!(stream.pending_final_gate_continuation.is_none());
                assert!(stream.pending_pause_persistence.is_none());
                let stored = stream.store.get(&stream.session_key).unwrap();
                assert_eq!(stored.governance, governance);
                assert!(stored.messages.is_empty());
                assert!(matches!(
                    stream.pending_pause_after_tool,
                    Some(RuntimeSemanticEvent::TurnCompleted { .. })
                ));
            }
        }
    }
}

#[tokio::test]
async fn final_gate_allows_verified_editor_and_work_independent_of_metadata() {
    for origin in [
        TurnExecutionOrigin::ArmatureConversation(ArmatureAvailability::Connected),
        TurnExecutionOrigin::AuthorizedWorkRun(ArmatureAvailability::Absent),
    ] {
        for governance in [Governance::Interactive, Governance::AutonomousContinuation] {
            for profile in [
                RuntimeContextLabel::ChannelConversation,
                RuntimeContextLabel::Observation,
                RuntimeContextLabel::Curation,
            ] {
                for owner_label in ["chat", "watch", "curate", "forged-owner"] {
                    let mut session = test_session("focused-allow:client-test", Uuid::new_v4());
                    session.origin = origin;
                    session.governance = governance;
                    session.profile = profile;
                    let mut focused = pending_task_list_projection();
                    focused.owner_profile = owner_label.into();
                    session.cached_activity_plan_projection = Some(focused.clone());
                    let mut stream = test_tracking_stream_with_session(&session);
                    stream.assistant_text = "Remaining work; stopping here.".into();
                    stream.evaluate_final_gate_or_complete(Some(focused), None);

                    assert!(!stream.finished, "{origin:?}/{governance:?}/{owner_label}");
                    assert!(stream.pending_final_gate_continuation.is_some());
                    assert!(stream.pending_pause_persistence.is_none());
                    let stored = stream.store.get(&stream.session_key).unwrap();
                    assert_eq!(stored.governance, Governance::AutonomousContinuation);
                    assert_eq!(stored.profile, profile);
                    assert_eq!(
                        stored
                            .cached_activity_plan_projection
                            .unwrap()
                            .owner_profile,
                        owner_label
                    );
                    assert!(matches!(stream.pending_pause_after_tool,
                        Some(RuntimeSemanticEvent::RunProgress { ref kind, .. })
                        if kind == "autonomous_continuation_gate"));
                }
            }
        }
    }
}

#[tokio::test]
async fn final_gate_does_not_promote_cache_only_focus_for_authorized_work() {
    let mut session = test_session("focused-cache:client-test", Uuid::new_v4());
    session.origin = TurnExecutionOrigin::AuthorizedWorkRun(ArmatureAvailability::Absent);
    session.profile = RuntimeContextLabel::JobRun;
    session.cached_activity_plan_projection = Some(pending_task_list_projection());
    let mut stream = test_tracking_stream_with_session(&session);
    stream.prepare_autonomous_final_gate(RuntimeTaskContext {
        source: RuntimeTaskSource::None,
        current_task_id: None,
        cached_activity_plan_projection: session.cached_activity_plan_projection,
    });
    let current = stream
        .store
        .get(&stream.session_key)
        .unwrap()
        .cached_activity_plan_projection;
    stream.evaluate_final_gate_or_complete(current, None);
    assert!(stream.finished);
    assert!(stream.pending_final_gate_continuation.is_none());
}

#[tokio::test]
async fn final_gate_keeps_persisted_run_requirement_for_authorized_work() {
    let mut session = test_session("focused-runless:client-test", Uuid::new_v4());
    session.origin = TurnExecutionOrigin::AuthorizedWorkRun(ArmatureAvailability::Absent);
    session.profile = RuntimeContextLabel::JobRun;
    session.run_id = None;
    session.cached_activity_plan_projection = Some(pending_task_list_projection());
    let mut stream = test_tracking_stream_with_session(&session);
    stream.evaluate_final_gate_or_complete(Some(pending_task_list_projection()), None);
    assert!(stream.pending_final_gate_continuation.is_none());
    let result = stream.pending_pause_persistence.take().unwrap().await;
    assert!(matches!(result, Err(DenError::System(ref detail))
        if detail == "active task execution cannot continue without a persisted run ID"));
}

#[tokio::test]
async fn final_gate_rejects_system_origins_before_continuation() {
    for origin in [
        TurnExecutionOrigin::InboundObservation,
        TurnExecutionOrigin::InternalCuration,
    ] {
        let mut session = test_session("focused-source:client-test", Uuid::new_v4());
        session.origin = origin;
        session.profile = RuntimeContextLabel::ArmatureConversation;
        let mut stream = test_tracking_stream_with_session(&session);
        stream.evaluate_final_gate_or_complete(Some(pending_task_list_projection()), None);
        assert!(stream.pending_final_gate_continuation.is_none());
        let result = stream.pending_pause_persistence.take().unwrap().await;
        assert!(matches!(result, Err(DenError::Authorization(_))));
    }
}
