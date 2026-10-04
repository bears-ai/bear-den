use super::*;

fn observation(
    class: ToolBudgetClass,
    signature: &str,
    failed: bool,
) -> ToolContinuationObservation {
    ToolContinuationObservation {
        tool_name: class.label().to_string(),
        signature: signature.to_string(),
        class,
        failed,
        grounding_probe_signal: None,
    }
}

#[test]
fn objective_orientation_resolver_prefers_work_assignment_over_task() {
    let task_ref = OrientationTaskRef::DocketTask {
        job_id: Some("job-from-task".to_string()),
        task_id: "task-1".to_string(),
        title: Some("Implement the thing".to_string()),
    };

    let resolved = resolve_objective_orientation(ObjectiveOrientationResolutionInput {
        docket_job_id: Some("job-1".to_string()),
        docket_execution_mutable: true,
        active_task_ref: Some(task_ref.clone()),
        freeform_policy: FreeformPolicy::task_definition_permitted(),
    });

    assert_eq!(
        resolved,
        ObjectiveOrientation::DocketExecution {
            job: DocketExecutionOrientation {
                job_id: "job-1".to_string(),
                active_task_ref: Some(task_ref),
                mutable: true,
            }
        }
    );
}

#[test]
fn objective_orientation_resolver_orients_to_task_before_freeform() {
    let task_ref = OrientationTaskRef::TaskListItem {
        task_list_id: "list-1".to_string(),
        item_id: "item-1".to_string(),
        title: Some("Document task orientation".to_string()),
    };

    let resolved = resolve_objective_orientation(ObjectiveOrientationResolutionInput {
        docket_job_id: None,
        docket_execution_mutable: true,
        active_task_ref: Some(task_ref.clone()),
        freeform_policy: FreeformPolicy::task_definition_permitted(),
    });

    assert_eq!(
        resolved,
        ObjectiveOrientation::Oriented {
            task: TaskOrientation {
                task_ref,
                child_policy: OrientedChildTaskPolicy {
                    max_children: DEFAULT_ORIENTED_MAX_CHILDREN,
                    max_depth_below_oriented_task: DEFAULT_ORIENTED_MAX_DEPTH,
                },
            }
        }
    );
}

#[test]
fn objective_orientation_resolver_preserves_closed_freeform_policy() {
    let resolved = resolve_objective_orientation(ObjectiveOrientationResolutionInput {
        docket_job_id: None,
        docket_execution_mutable: true,
        active_task_ref: None,
        freeform_policy: FreeformPolicy::closed(),
    });

    assert_eq!(
        resolved,
        ObjectiveOrientation::Freeform {
            policy: FreeformPolicy {
                may_define_task: false,
            }
        }
    );
}

#[test]
fn profiles_get_stricter_checkpoint_thresholds_by_level() {
    let light = AgentLoopControlProfile::for_level(AgentLoopControlLevel::Light);
    let standard = AgentLoopControlProfile::for_level(AgentLoopControlLevel::Standard);
    let careful = AgentLoopControlProfile::for_level(AgentLoopControlLevel::Careful);
    let strict = AgentLoopControlProfile::for_level(AgentLoopControlLevel::Strict);

    assert_eq!(
        light.checkpoints.exploration_without_mutation_threshold,
        Some(12)
    );
    assert_eq!(
        standard.checkpoints.exploration_without_mutation_threshold,
        Some(10)
    );
    assert!(
        standard.checkpoints.exploration_without_mutation_threshold
            > careful.checkpoints.exploration_without_mutation_threshold
    );
    assert!(
        careful.checkpoints.exploration_without_mutation_threshold
            >= strict.checkpoints.exploration_without_mutation_threshold
    );
    assert!(!light.checkpoints.require_before_broad_mutation);
    assert!(careful.checkpoints.require_before_broad_mutation);
    assert!(strict.checkpoints.require_before_broad_mutation);
}

#[test]
fn checkpoint_thinking_policy_escalates_by_level() {
    let light = AgentLoopControlProfile::for_level(AgentLoopControlLevel::Light);
    let standard = AgentLoopControlProfile::for_level(AgentLoopControlLevel::Standard);
    let careful = AgentLoopControlProfile::for_level(AgentLoopControlLevel::Careful);

    assert!(!light.thinking.enabled);
    assert_eq!(
        standard.thinking.checkpoint_turn_effort,
        Some(ThinkingEffort::Medium)
    );
    assert_eq!(
        careful.thinking.checkpoint_turn_effort,
        Some(ThinkingEffort::High)
    );
}

#[test]
fn resolver_uses_model_default_then_overrides_then_escalation() {
    let resolved = resolve_agent_loop_control(AgentLoopControlResolutionInput {
        model_handle: Some("openai/gpt-5.5"),
        model_default: None,
        bear_override: None,
        task_escalation: None,
        origin: den_core::TurnExecutionOrigin::ChannelConversation,
        governance: den_core::Governance::Observational,
        objective_orientation: None,
        pre_risk: false,
    })
    .expect("ordinary test origin");
    assert_eq!(resolved.level, AgentLoopControlLevel::Light);
    assert_eq!(resolved.source, AgentLoopControlSource::ModelDefault);

    let resolved = resolve_agent_loop_control(AgentLoopControlResolutionInput {
        model_handle: Some("openai/gpt-5.5"),
        model_default: None,
        bear_override: Some(AgentLoopControlLevel::Standard),
        task_escalation: Some(AgentLoopControlLevel::Strict),
        origin: den_core::TurnExecutionOrigin::ChannelConversation,
        governance: den_core::Governance::Observational,
        objective_orientation: None,
        pre_risk: false,
    })
    .expect("ordinary test origin");
    assert_eq!(resolved.level, AgentLoopControlLevel::Strict);
    assert_eq!(resolved.source, AgentLoopControlSource::TaskEscalation);
}

#[test]
fn checkpoint_evaluator_triggers_on_over_exploration() {
    let profile = AgentLoopControlProfile::for_level(AgentLoopControlLevel::Careful);
    let state = CheckpointState::default();
    let evaluation = evaluate_checkpoint_trigger(
        &profile,
        &state,
        &[
            observation(ToolBudgetClass::Read, "read:a", false),
            observation(ToolBudgetClass::Search, "search:b", false),
            observation(ToolBudgetClass::Read, "read:c", false),
        ],
        false,
    );

    assert_eq!(
        evaluation.trigger.as_ref().map(|trigger| trigger.reason),
        Some(CheckpointReason::OverExploration)
    );
    assert_eq!(evaluation.next_state.read_search_since_mutation, 3);
}

#[test]
fn checkpoint_state_reset_after_report_opens_fresh_observation_window() {
    let mut state = CheckpointState {
        read_search_since_mutation: 7,
        consecutive_failures: 2,
        same_signature_repeat_count: 3,
        last_signature: Some("memory_read:{path=a}".to_string()),
        last_checkpoint_reason: Some(CheckpointReason::OverExploration),
    };

    state.reset_after_checkpoint_report();

    assert_eq!(state.read_search_since_mutation, 0);
    assert_eq!(state.consecutive_failures, 0);
    assert_eq!(state.same_signature_repeat_count, 0);
    assert_eq!(state.last_signature, None);
    assert_eq!(state.last_checkpoint_reason, None);
}

#[test]
fn checkpoint_reset_allows_bounded_fresh_read_search_window() {
    let profile = AgentLoopControlProfile::for_level(AgentLoopControlLevel::Careful);
    let mut state = CheckpointState {
        read_search_since_mutation: 3,
        last_checkpoint_reason: Some(CheckpointReason::OverExploration),
        ..CheckpointState::default()
    };
    state.reset_after_checkpoint_report();

    let evaluation = evaluate_checkpoint_trigger(
        &profile,
        &state,
        &[observation(ToolBudgetClass::Read, "read:a", false)],
        false,
    );

    assert!(evaluation.trigger.is_none());
    assert_eq!(evaluation.next_state.read_search_since_mutation, 1);
}

#[test]
fn checkpoint_evaluator_clears_last_reason_when_no_trigger_fires() {
    let profile = AgentLoopControlProfile::for_level(AgentLoopControlLevel::Standard);
    let state = CheckpointState {
        last_checkpoint_reason: Some(CheckpointReason::OverExploration),
        ..CheckpointState::default()
    };

    let evaluation = evaluate_checkpoint_trigger(
        &profile,
        &state,
        &[observation(ToolBudgetClass::Write, "write:a", false)],
        false,
    );

    assert!(evaluation.trigger.is_none());
    assert_eq!(evaluation.next_state.last_checkpoint_reason, None);
}

#[test]
fn checkpoint_evaluator_resets_exploration_after_meaningful_mutation() {
    let profile = AgentLoopControlProfile::for_level(AgentLoopControlLevel::Careful);
    let state = CheckpointState {
        read_search_since_mutation: 2,
        ..CheckpointState::default()
    };
    let evaluation = evaluate_checkpoint_trigger(
        &profile,
        &state,
        &[observation(ToolBudgetClass::Write, "write:a", false)],
        false,
    );

    assert!(evaluation.trigger.is_none());
    assert_eq!(evaluation.next_state.read_search_since_mutation, 0);
}

#[test]
fn failed_grounding_probe_does_not_reset_exploration() {
    let profile = AgentLoopControlProfile::for_level(AgentLoopControlLevel::Careful);
    let state = CheckpointState {
        read_search_since_mutation: 2,
        ..CheckpointState::default()
    };
    let mut mutation = observation(ToolBudgetClass::Write, "write:a", false);
    mutation.grounding_probe_signal = Some(GroundingProbeSignalKind::Fail);

    let evaluation = evaluate_checkpoint_trigger(&profile, &state, &[mutation], false);

    assert!(evaluation.trigger.is_none());
    assert_eq!(evaluation.next_state.read_search_since_mutation, 2);
}

#[test]
fn checkpoint_evaluator_triggers_on_failures_and_low_budget() {
    let profile = AgentLoopControlProfile::for_level(AgentLoopControlLevel::Standard);
    let low_budget = evaluate_checkpoint_trigger(&profile, &CheckpointState::default(), &[], true);
    assert_eq!(
        low_budget.trigger.as_ref().map(|trigger| trigger.reason),
        Some(CheckpointReason::LowBudget)
    );

    let failures = evaluate_checkpoint_trigger(
        &profile,
        &CheckpointState::default(),
        &[
            observation(ToolBudgetClass::Read, "read:a", true),
            observation(ToolBudgetClass::Read, "read:b", true),
        ],
        false,
    );
    assert_eq!(
        failures.trigger.as_ref().map(|trigger| trigger.reason),
        Some(CheckpointReason::ConsecutiveFailure)
    );
}

#[test]
fn checkpoint_evaluator_triggers_on_same_signature_near_ko() {
    let profile = AgentLoopControlProfile::for_level(AgentLoopControlLevel::Standard);
    let evaluation = evaluate_checkpoint_trigger(
        &profile,
        &CheckpointState::default(),
        &[
            observation(ToolBudgetClass::Read, "read:a", false),
            observation(ToolBudgetClass::Read, "read:a", false),
            observation(ToolBudgetClass::Read, "read:a", false),
        ],
        false,
    );

    assert_eq!(
        evaluation.trigger.as_ref().map(|trigger| trigger.reason),
        Some(CheckpointReason::SameSignatureNearKo)
    );
}

#[test]
fn tool_budget_multiplier_preserves_profile_safety_limits() {
    let base = AgentLoopControlProfile::for_level(AgentLoopControlLevel::Strict);
    let scaled = base.with_tool_budget_multiplier(1.5);

    assert_eq!(
        scaled.budget.tool_call_limits.total,
        (f64::from(base.budget.tool_call_limits.total) * 1.5).ceil() as u32
    );
    assert_eq!(
        scaled.budget.max_wall_clock_ms,
        base.budget.max_wall_clock_ms
    );
    assert_eq!(
        scaled.budget.emergency_hard_steps,
        base.budget.emergency_hard_steps
    );
    assert_eq!(
        scaled.budget.max_consecutive_tool_failures,
        base.budget.max_consecutive_tool_failures
    );
    assert_eq!(
        scaled.ko.max_same_tool_signature_repeats,
        base.ko.max_same_tool_signature_repeats
    );
}

#[test]
fn pre_risk_checkpoint_follows_profile_policy() {
    let light = AgentLoopControlProfile::for_level(AgentLoopControlLevel::Light);
    let careful = AgentLoopControlProfile::for_level(AgentLoopControlLevel::Careful);

    assert!(pre_risk_checkpoint_trigger(&light).is_none());
    assert_eq!(
        pre_risk_checkpoint_trigger(&careful).map(|trigger| trigger.reason),
        Some(CheckpointReason::PreRiskMutation)
    );
}

#[test]
fn profile_fingerprint_is_deterministic_and_profile_sensitive() {
    let standard = AgentLoopControlProfile::for_level(AgentLoopControlLevel::Standard);
    let standard_again = AgentLoopControlProfile::for_level(AgentLoopControlLevel::Standard);
    let careful = AgentLoopControlProfile::for_level(AgentLoopControlLevel::Careful);

    let standard_fingerprint = agent_loop_control_profile_fingerprint(&standard).unwrap();

    assert_eq!(standard_fingerprint.len(), 64);
    assert_eq!(
        standard_fingerprint,
        agent_loop_control_profile_fingerprint(&standard_again).unwrap()
    );
    assert_ne!(
        standard_fingerprint,
        agent_loop_control_profile_fingerprint(&careful).unwrap()
    );
}

fn checkpoint_request() -> RuntimeCheckpointRequest {
    RuntimeCheckpointRequest {
        checkpoint_id: "ckpt-1".to_string(),
        run_id: "run-1".to_string(),
        reason: CheckpointReason::OverExploration,
        control_level: AgentLoopControlLevel::Careful,
        profile_fingerprint: Some("profile-test".to_string()),
        active_objective: Some("Patch the failing path".to_string()),
        task_context: Some(CheckpointTaskContext {
            task_list_id: Some("list-1".to_string()),
            task_list_version: Some("3".to_string()),
            active_item_id: Some("item-1".to_string()),
            active_item_title: Some("Inspect routing".to_string()),
            docket_job_id: Some("job-1".to_string()),
            docket_task_id: Some("task-1".to_string()),
        }),
        evidence_refs: vec![CheckpointEvidenceRef {
            kind: "tool_result".to_string(),
            id: "call-1".to_string(),
            summary: Some("Read routing module".to_string()),
        }],
        required_fields: vec![
            CheckpointField::ActiveObjective,
            CheckpointField::Learned,
            CheckpointField::NextAction,
        ],
    }
}

fn checkpoint_response() -> RuntimeCheckpointResponse {
    RuntimeCheckpointResponse {
        checkpoint_id: "ckpt-1".to_string(),
        active_objective: "Patch the failing path".to_string(),
        summary: None,
        learned: vec!["The relevant logic is in the route projector.".to_string()],
        remaining_uncertainty: Vec::new(),
        more_exploration_justified: false,
        next_action: CheckpointNextAction::Edit,
        task_state_change_needed: None,
        evidence_refs: Vec::new(),
        confidence: Some(CheckpointConfidence::Medium),
    }
}

#[test]
fn checkpoint_response_validation_accepts_structured_response() {
    let request = checkpoint_request();
    let response = checkpoint_response();

    assert_eq!(validate_checkpoint_response(&request, &response), Ok(()));
}

#[test]
fn checkpoint_response_validation_rejects_wrong_id_and_missing_required_field() {
    let request = checkpoint_request();
    let mut response = checkpoint_response();
    response.checkpoint_id = "other".to_string();
    assert!(matches!(
        validate_checkpoint_response(&request, &response),
        Err(CheckpointResponseValidationError::CheckpointIdMismatch { .. })
    ));

    response.checkpoint_id = request.checkpoint_id.clone();
    response.learned.clear();
    assert_eq!(
        validate_checkpoint_response(&request, &response),
        Err(CheckpointResponseValidationError::MissingRequiredField(
            CheckpointField::Learned
        ))
    );
}

#[test]
fn checkpoint_next_action_deserializes_natural_language_mutation() {
    let value = serde_json::json!(
        "Make the first meaningful mutation in den-runtime: introduce typed ToolCallWire payload structs."
    );
    let parsed: CheckpointNextAction = serde_json::from_value(value).unwrap();
    assert_eq!(parsed, CheckpointNextAction::Edit);
}

#[test]
fn checkpoint_next_action_deserializes_tool_object() {
    let value = serde_json::json!({
        "action": "call_tool",
        "tool_name": "fs_read_text_file"
    });
    let parsed: CheckpointNextAction = serde_json::from_value(value).unwrap();
    assert_eq!(
        parsed,
        CheckpointNextAction::CallTool {
            tool_name: Some("fs_read_text_file".to_string())
        }
    );
}

#[test]
fn checkpoint_next_action_serializes_to_snake_case() {
    let serialized = serde_json::to_value(CheckpointNextAction::UpdateTaskList).unwrap();
    assert_eq!(serialized, serde_json::json!("update_task_list"));
}

#[test]
fn profile_budget_can_be_overlaid_without_losing_level_policy() {
    let profile = AgentLoopControlProfile::for_level(AgentLoopControlLevel::Careful);
    let replacement_budget = budget(
        900_000,
        128,
        limits(160, 48, 32, 20, 24, 24, 6, 24),
        4,
        3,
        Some((8, 4)),
    );

    let overlaid = profile.with_budget(replacement_budget);

    assert_eq!(overlaid.budget, replacement_budget);
    assert_eq!(overlaid.ko.max_same_tool_signature_repeats, 3);
    assert_eq!(
        overlaid.checkpoints.exploration_without_mutation_threshold,
        Some(3)
    );
    assert_eq!(
        overlaid.thinking.checkpoint_turn_effort,
        Some(ThinkingEffort::High)
    );
}

fn focused_orientation() -> ObjectiveOrientation {
    ObjectiveOrientation::DocketExecution {
        job: DocketExecutionOrientation {
            job_id: "job-1".to_string(),
            active_task_ref: None,
            mutable: true,
        },
    }
}

#[test]
fn objective_orientation_gate_requires_focused_work() {
    let freeform = ObjectiveOrientation::Freeform {
        policy: FreeformPolicy::task_definition_permitted(),
    };

    assert!(!objective_orientation_allowed_for_origin(
        den_core::TurnExecutionOrigin::AuthorizedWorkRun(den_core::ArmatureAvailability::Connected),
        &freeform
    ));
    assert!(objective_orientation_allowed_for_origin(
        den_core::TurnExecutionOrigin::AuthorizedWorkRun(den_core::ArmatureAvailability::Connected),
        &focused_orientation()
    ));
    assert!(objective_orientation_allowed_for_origin(
        den_core::TurnExecutionOrigin::ArmatureConversation(
            den_core::ArmatureAvailability::Connected
        ),
        &freeform
    ));
}

#[test]
fn work_autonomous_without_focused_job_is_invalid() {
    let freeform = ObjectiveOrientation::Freeform {
        policy: FreeformPolicy::task_definition_permitted(),
    };
    let oriented = ObjectiveOrientation::Oriented {
        task: TaskOrientation {
            task_ref: OrientationTaskRef::DocketTask {
                job_id: Some("job-1".to_string()),
                task_id: "task-1".to_string(),
                title: Some("Task without focused job".to_string()),
            },
            child_policy: OrientedChildTaskPolicy::default(),
        },
    };

    assert!(!objective_orientation_allowed_for_origin(
        den_core::TurnExecutionOrigin::AuthorizedWorkRun(den_core::ArmatureAvailability::Connected),
        &freeform
    ));
    assert!(!objective_orientation_allowed_for_origin(
        den_core::TurnExecutionOrigin::AuthorizedWorkRun(den_core::ArmatureAvailability::Connected),
        &oriented
    ));
    assert!(objective_orientation_allowed_for_origin(
        den_core::TurnExecutionOrigin::AuthorizedWorkRun(den_core::ArmatureAvailability::Connected),
        &focused_orientation()
    ));
}

#[test]
fn context_defaults_are_aggressive_for_pre_release() {
    let pair_freeform = resolve_agent_loop_control(AgentLoopControlResolutionInput {
        model_handle: Some("openai/gpt-5.5"),
        model_default: None,
        bear_override: None,
        task_escalation: None,
        origin: den_core::TurnExecutionOrigin::ArmatureConversation(
            den_core::ArmatureAvailability::Connected,
        ),
        governance: den_core::Governance::Interactive,
        objective_orientation: None,
        pre_risk: false,
    })
    .expect("ordinary test origin");
    assert_eq!(pair_freeform.level, AgentLoopControlLevel::Standard);
    assert_eq!(pair_freeform.source, AgentLoopControlSource::ModelDefault);

    let focused_pair = resolve_agent_loop_control(AgentLoopControlResolutionInput {
        model_handle: Some("openai/gpt-5.5"),
        model_default: None,
        bear_override: None,
        task_escalation: None,
        origin: den_core::TurnExecutionOrigin::ArmatureConversation(
            den_core::ArmatureAvailability::Connected,
        ),
        governance: den_core::Governance::Interactive,
        objective_orientation: Some(&focused_orientation()),
        pre_risk: false,
    })
    .expect("ordinary test origin");
    assert_eq!(focused_pair.level, AgentLoopControlLevel::Careful);

    let focused_work = resolve_agent_loop_control(AgentLoopControlResolutionInput {
        model_handle: Some("openai/gpt-5.5"),
        model_default: None,
        bear_override: None,
        task_escalation: None,
        origin: den_core::TurnExecutionOrigin::AuthorizedWorkRun(
            den_core::ArmatureAvailability::Connected,
        ),
        governance: den_core::Governance::Interactive,
        objective_orientation: Some(&focused_orientation()),
        pre_risk: false,
    })
    .expect("ordinary test origin");
    assert_eq!(focused_work.level, AgentLoopControlLevel::Careful);

    let pre_risk = resolve_agent_loop_control(AgentLoopControlResolutionInput {
        model_handle: Some("openai/gpt-5.5"),
        model_default: None,
        bear_override: None,
        task_escalation: None,
        origin: den_core::TurnExecutionOrigin::ArmatureConversation(
            den_core::ArmatureAvailability::Connected,
        ),
        governance: den_core::Governance::Interactive,
        objective_orientation: Some(&focused_orientation()),
        pre_risk: true,
    })
    .expect("ordinary test origin");
    assert_eq!(pre_risk.level, AgentLoopControlLevel::Strict);
    assert_eq!(pre_risk.source, AgentLoopControlSource::PreRiskEscalation);
}

#[test]
fn escalation_never_downgrades_operator_override() {
    let resolved = resolve_agent_loop_control(AgentLoopControlResolutionInput {
        model_handle: Some("unknown-model"),
        model_default: None,
        bear_override: Some(AgentLoopControlLevel::Careful),
        task_escalation: Some(AgentLoopControlLevel::Light),
        origin: den_core::TurnExecutionOrigin::ChannelConversation,
        governance: den_core::Governance::Observational,
        objective_orientation: None,
        pre_risk: false,
    })
    .expect("ordinary test origin");
    assert_eq!(resolved.level, AgentLoopControlLevel::Careful);
    assert_eq!(resolved.source, AgentLoopControlSource::BearOverride);
}

#[test]
fn control_defaults_use_verified_origin_and_governance_not_docket_hints() {
    use den_core::ArmatureAvailability::{Absent, Connected};
    for (origin, expected) in [
        (
            TurnExecutionOrigin::ChannelConversation,
            AgentLoopControlLevel::Standard,
        ),
        (
            TurnExecutionOrigin::BrowserTaskSession,
            AgentLoopControlLevel::Careful,
        ),
        (
            TurnExecutionOrigin::ArmatureConversation(Connected),
            AgentLoopControlLevel::Careful,
        ),
        (
            TurnExecutionOrigin::ArmatureConversation(Absent),
            AgentLoopControlLevel::Careful,
        ),
        (
            TurnExecutionOrigin::AuthorizedWorkRun(Connected),
            AgentLoopControlLevel::Careful,
        ),
        (
            TurnExecutionOrigin::AuthorizedWorkRun(Absent),
            AgentLoopControlLevel::Careful,
        ),
    ] {
        let orientation = focused_orientation();
        let input = AgentLoopControlResolutionInput {
            origin,
            governance: Governance::Interactive,
            model_handle: None,
            model_default: Some(AgentLoopControlLevel::Light),
            bear_override: None,
            task_escalation: None,
            objective_orientation: Some(&orientation),
            pre_risk: false,
        };
        let control = resolve_agent_loop_control(input.clone()).unwrap();
        assert_eq!(control.level, expected, "{origin:?}");
        assert_eq!(
            control.profile,
            AgentLoopControlProfile::for_level(expected)
        );
        for governance in [Governance::Observational, Governance::Frozen] {
            let control = resolve_agent_loop_control(AgentLoopControlResolutionInput {
                governance,
                ..input.clone()
            })
            .unwrap();
            assert_eq!(
                control.level,
                AgentLoopControlLevel::Light,
                "{origin:?} {governance:?}"
            );
        }
    }
}

#[test]
fn system_origin_cannot_select_control_or_job_budget_with_overrides() {
    let orientation = focused_orientation();
    for origin in [
        TurnExecutionOrigin::InternalCuration,
        TurnExecutionOrigin::InboundObservation,
    ] {
        for governance in Governance::ALL {
            assert!(matches!(
                resolve_agent_loop_control(AgentLoopControlResolutionInput {
                    origin,
                    governance,
                    model_handle: Some("openai/test"),
                    model_default: Some(AgentLoopControlLevel::Light),
                    bear_override: Some(AgentLoopControlLevel::Light),
                    task_escalation: Some(AgentLoopControlLevel::Strict),
                    objective_orientation: Some(&orientation),
                    pre_risk: true,
                }),
                Err(DenError::Authorization(_))
            ));
            assert!(!objective_orientation_allowed_for_origin(
                origin,
                &orientation
            ));
        }
    }
}
