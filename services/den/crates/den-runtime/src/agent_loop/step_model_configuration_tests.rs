use super::*;
use den_core::{
    ids::ModelConfigurationId,
    model_request_policy::resolve_agent_primary_request_profile_with_configuration,
};
use den_service::bears::model_configurations::{PrimaryModelSource, ResolvedPrimaryModel};

fn session() -> AgentLoopSession {
    let mut session = crate::agent_loop::source_admission::tests::test_session(
        uuid::Uuid::new_v4(),
        1,
        "request-conversation",
        "request-client",
        "request-run",
    );
    session.model = "openai/gpt-5".into();
    session.model_request_profile.approved_model_ref = session.model.clone();
    session.model_request_profile.supports_reasoning_effort = Some(true);
    session.api_style = Some(LlmApiStyle::ResponsesStream);
    session
}

fn tool() -> crate::llm::LlmToolDefinition {
    crate::llm::LlmToolDefinition {
        name: "fs_read_file".into(),
        description: None,
        parameters: serde_json::json!({}),
    }
}

#[test]
fn ordinary_and_subsequent_requests_send_explicit_effort_on_both_wire_shapes() {
    let mut session = session();
    session.model_request_profile.thinking_effort = Some(ThinkingEffort::Low);
    session.agent_loop_control.profile.thinking.enabled = true;
    session
        .agent_loop_control
        .profile
        .thinking
        .checkpoint_turn_effort = Some(ThinkingEffort::High);
    for step in 0..3 {
        session.step = step;
        let profile = primary_request_profile_for_session(&session);
        let request = primary_request_for_session(&session, &profile, vec![], vec![tool()]);
        assert_eq!(request.model, "openai/gpt-5");
        assert_eq!(request.thinking_effort, Some(ThinkingEffort::Low));
        assert_eq!(request.to_body()["reasoning_effort"], "low");
        assert_eq!(request.to_responses_body()["reasoning"]["effort"], "low");
        assert_eq!(
            request.telemetry.as_ref().unwrap().run_id.as_deref(),
            Some("request-run")
        );
    }
    // The symbolic checkpoint changes, not the explicit effort.
    let profile = resolve_agent_primary_request_profile_with_configuration(
        &session.model,
        AgentPrimaryStep::Checkpoint,
        Some(true),
        session.model_request_profile.thinking_effort,
        Some(ThinkingEffort::High),
    );
    let request = primary_request_for_session(&session, &profile, vec![], vec![tool()]);
    assert_eq!(request.thinking_effort, Some(ThinkingEffort::Low));
}

#[test]
fn model_default_does_not_inherit_configured_ordinary_reasoning() {
    let session = session();
    let profile = primary_request_profile_for_session(&session);
    let request = primary_request_for_session(&session, &profile, vec![], vec![]);
    assert!(request.to_body().get("reasoning_effort").is_none());
    assert!(request.to_responses_body().get("reasoning").is_none());
    let profile = resolve_agent_primary_request_profile_with_configuration(
        &session.model,
        AgentPrimaryStep::Checkpoint,
        Some(true),
        None,
        Some(ThinkingEffort::High),
    );
    assert_eq!(
        primary_request_for_session(&session, &profile, vec![], vec![]).thinking_effort,
        Some(ThinkingEffort::High)
    );
}

#[test]
fn incompatible_tool_bridge_omits_effort_and_persists_truthful_configuration_diagnostics() {
    let mut session = session();
    session.model = "anthropic/claude-sonnet-4.5".into();
    session.model_request_profile.approved_model_ref = session.model.clone();
    session.api_style = Some(LlmApiStyle::ChatCompletionsStream);
    session.model_request_profile.thinking_effort = Some(ThinkingEffort::High);
    let profile = primary_request_profile_for_session(&session);
    let request = primary_request_for_session(&session, &profile, vec![], vec![tool()]);
    assert!(request.thinking_effort.is_none());
    assert!(request.to_body().get("reasoning_effort").is_none());
    let primary = ResolvedPrimaryModel {
        configuration_id: Some(ModelConfigurationId::new(uuid::Uuid::new_v4())),
        configuration_name: Some("Careful".into()),
        model_handle: session.model.clone(),
        thinking_effort: Some(ThinkingEffort::High),
        source: PrimaryModelSource::HatOverride,
    };
    let events = crate::runtime::bearwire_projection::wire::runtime_stream_event_to_bearwire_events(
        crate::primary_model::configuration_progress_event(
            &primary,
            &profile,
            request.thinking_effort,
            session.api_style.unwrap(),
        ),
    );
    assert_eq!(events.len(), 1);
    assert_eq!(
        events[0].scope,
        bearwire_protocol::wire::BearWireEventScope::Persistent
    );
    let detail = &events[0].data["detail"];
    assert_eq!(
        detail["configuration_id"],
        primary.configuration_id.unwrap().to_string()
    );
    assert_eq!(detail["configuration_name"], "Careful");
    assert_eq!(detail["source"], "hat_override");
    assert_eq!(detail["model"], session.model);
    assert_eq!(detail["configured_thinking_effort"], "high");
    assert!(detail["effective_request_effort"].is_null());
    assert_eq!(detail["reasoning_disposition"], "skipped_api_incompatible");
    assert!(detail.get("prompt").is_none());
    // Tool-free calls over the same transport can still carry validated effort.
    assert_eq!(
        primary_request_for_session(&session, &profile, vec![], vec![]).thinking_effort,
        Some(ThinkingEffort::High)
    );
}
