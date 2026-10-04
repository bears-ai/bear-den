use super::*;
use den_core::ArmatureAvailability;

#[test]
fn ordinary_origin_budgets_preserve_channel_armature_and_job_defaults() {
    for (origin, millis, steps, total, writes) in [
        (
            TurnExecutionOrigin::ChannelConversation,
            180_000,
            40,
            72,
            12,
        ),
        (
            TurnExecutionOrigin::BrowserTaskSession,
            360_000,
            80,
            112,
            24,
        ),
        (
            TurnExecutionOrigin::ArmatureConversation(ArmatureAvailability::Connected),
            360_000,
            80,
            112,
            24,
        ),
        (
            TurnExecutionOrigin::ArmatureConversation(ArmatureAvailability::Absent),
            360_000,
            80,
            112,
            24,
        ),
        (
            TurnExecutionOrigin::AuthorizedWorkRun(ArmatureAvailability::Connected),
            900_000,
            128,
            160,
            24,
        ),
        (
            TurnExecutionOrigin::AuthorizedWorkRun(ArmatureAvailability::Absent),
            900_000,
            128,
            160,
            24,
        ),
    ] {
        let defaults = NativeTurnDefaults::for_origin(origin, Governance::Interactive).unwrap();
        assert_eq!(defaults.turn_budget.max_wall_clock_ms, millis, "{origin:?}");
        assert_eq!(
            defaults.turn_budget.emergency_hard_steps, steps,
            "{origin:?}"
        );
        assert_eq!(
            defaults.turn_budget.tool_call_limits.total, total,
            "{origin:?}"
        );
        assert_eq!(
            defaults.turn_budget.tool_call_limits.write, writes,
            "{origin:?}"
        );
        assert!(defaults.include_prompt_memory);
        assert_eq!(
            defaults.context_label,
            EffectivePolicy::compile_for_origin(origin, Governance::Interactive).context_label
        );
    }
}

#[test]
fn generic_system_origins_have_no_native_budget_even_with_model_multiplier() {
    for origin in [
        TurnExecutionOrigin::InternalCuration,
        TurnExecutionOrigin::InboundObservation,
    ] {
        for governance in Governance::ALL {
            assert!(matches!(
                NativeTurnDefaults::for_origin(origin, governance),
                Err(DenError::Authorization(_))
            ));
        }
    }
}

#[test]
fn tool_budget_multiplier_preserves_existing_capacity_scaling() {
    let base = NativeTurnDefaults::for_origin(
        TurnExecutionOrigin::BrowserTaskSession,
        Governance::Interactive,
    )
    .unwrap();
    let scaled = base.with_tool_budget_multiplier(1.5);
    assert_eq!(scaled.turn_budget.tool_call_limits.total, 168);
    assert_eq!(scaled.turn_budget.emergency_hard_steps, 120);
    assert_eq!(
        scaled.turn_budget.max_wall_clock_ms,
        base.turn_budget.max_wall_clock_ms
    );
}
