use super::*;

const STEPS: [AgentPrimaryStep; 8] = [
    AgentPrimaryStep::OrdinaryTurn,
    AgentPrimaryStep::Planning,
    AgentPrimaryStep::TaskSelection,
    AgentPrimaryStep::Execution,
    AgentPrimaryStep::Checkpoint,
    AgentPrimaryStep::PreRiskReview,
    AgentPrimaryStep::Summarization,
    AgentPrimaryStep::CheapProbe,
];

#[test]
fn explicit_configuration_effort_wins_for_every_loop_step() {
    for step in STEPS {
        for effort in [
            ThinkingEffort::Low,
            ThinkingEffort::Medium,
            ThinkingEffort::High,
        ] {
            let profile = resolve_agent_primary_request_profile_with_configuration(
                "openai/gpt-5",
                step,
                Some(true),
                Some(effort),
                Some(ThinkingEffort::High),
            );
            assert_eq!(profile.thinking_effort, Some(effort));
            assert_eq!(profile.approved_model_ref, "openai/gpt-5");
            assert_eq!(profile.agent_primary_step, step);
        }
    }
}

#[test]
fn model_default_preserves_only_legacy_checkpoint_effort() {
    for step in STEPS {
        let profile = resolve_agent_primary_request_profile_with_configuration(
            "openai/gpt-5",
            step,
            Some(true),
            None,
            Some(ThinkingEffort::Medium),
        );
        let expected = matches!(
            step,
            AgentPrimaryStep::Checkpoint | AgentPrimaryStep::PreRiskReview
        )
        .then_some(ThinkingEffort::Medium);
        assert_eq!(profile.thinking_effort, expected);
        assert_eq!(
            profile,
            resolve_agent_primary_request_profile(
                "openai/gpt-5",
                step,
                Some(true),
                Some(ThinkingEffort::Medium),
            )
        );
    }
}

#[test]
fn unknown_or_unsupported_capabilities_never_advertise_effort() {
    for support in [None, Some(false)] {
        for step in STEPS {
            let profile = resolve_agent_primary_request_profile_with_configuration(
                "vendor/model",
                step,
                support,
                Some(ThinkingEffort::Low),
                Some(ThinkingEffort::High),
            );
            assert_eq!(profile.thinking_effort, None);
        }
    }
}

#[test]
fn step_names_are_stable() {
    assert_eq!(AgentPrimaryStep::CheapProbe.as_str(), "cheap_probe");
}
