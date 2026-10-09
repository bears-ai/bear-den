//! A provider-step boundary transfers requests to the outer server loop, not
//! to the client and not to terminal abandonment settlement.
use super::{
    tests::{test_session, test_tracking_stream_with_session},
    NativeToolDispatchMode, SessionTrackingStream,
};
use crate::agent_loop::pending_tool_calls;
use den_core::{RuntimeContextLabel, TurnExecutionOrigin};
use den_protocol::{RuntimeSemanticEvent, RuntimeStreamEvent};
use uuid::Uuid;

fn tracker(mode: NativeToolDispatchMode) -> SessionTrackingStream {
    let mut session = test_session("provider-step-handoff", Uuid::new_v4());
    session.origin = TurnExecutionOrigin::ChannelConversation;
    session.profile = RuntimeContextLabel::ChannelConversation;
    // No persistence/database connection is needed for this state-machine regression.
    session.conversation_id = "provider-only-handoff".into();
    let mut tracker = test_tracking_stream_with_session(&session);
    tracker.dispatch_mode = mode;
    tracker.tool_calls.insert(
        "head-call".into(),
        (
            "repository_head".into(),
            serde_json::json!({"work_surface_id":Uuid::nil()}).to_string(),
        ),
    );
    tracker.sync_assistant_tool_step_to_session();
    tracker
}

#[tokio::test]
async fn repository_head_web_step_handoff_keeps_requests_and_does_not_complete_the_turn() {
    for reason in [
        "llm_stream_ended_before_tool_results",
        "turn_ended_before_tool_results",
    ] {
        let mut tracker = tracker(NativeToolDispatchMode::ServerSideInProcess);
        assert!(tracker
            .finalize_outstanding_tools_at_stream_end(reason, "test-step-boundary", "not abandoned")
            .is_none());
        assert!(tracker.finished);
        assert!(tracker.tool_calls.is_empty());
        let session = tracker.store.get(&tracker.session_key).unwrap();
        let pending = pending_tool_calls(&session.messages);
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].id, "head-call");
        assert!(session
            .messages
            .iter()
            .all(|message| message.role != "tool"));
        assert_eq!(session.step, 1);
    }
}

#[tokio::test]
async fn repository_head_abandoned_client_deferred_server_work_still_fails() {
    let mut tracker = tracker(NativeToolDispatchMode::DeferToClient);
    let event = tracker.finalize_outstanding_tools_at_stream_end(
        "abandoned",
        "test-abandoned",
        "server work abandoned",
    );
    assert!(matches!(
        event,
        Some(RuntimeStreamEvent::Semantic(
            RuntimeSemanticEvent::TurnFailed { .. }
        ))
    ));
    assert!(tracker.finished);
}
