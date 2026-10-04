use super::*;
use den_core::ArmatureAvailability;

#[test]
fn compaction_defaults_follow_verified_turn_origin() {
    for (origin, version, protected, groups, chars) in [
        (
            TurnExecutionOrigin::ChannelConversation,
            "chat-v1",
            3,
            10,
            16_000,
        ),
        (
            TurnExecutionOrigin::BrowserTaskSession,
            "pair-v1",
            4,
            8,
            24_000,
        ),
        (
            TurnExecutionOrigin::ArmatureConversation(ArmatureAvailability::Connected),
            "pair-v1",
            4,
            8,
            24_000,
        ),
        (
            TurnExecutionOrigin::ArmatureConversation(ArmatureAvailability::Absent),
            "pair-v1",
            4,
            8,
            24_000,
        ),
        (
            TurnExecutionOrigin::AuthorizedWorkRun(ArmatureAvailability::Connected),
            "work-v1",
            3,
            8,
            20_000,
        ),
        (
            TurnExecutionOrigin::AuthorizedWorkRun(ArmatureAvailability::Absent),
            "work-v1",
            3,
            8,
            20_000,
        ),
    ] {
        let policy = compaction_policy_for_source(CompactionSource::Turn(origin)).unwrap();
        assert_eq!(policy.policy_version, version);
        assert_eq!(policy.protected_recent_group_count, protected);
        assert_eq!(policy.max_groups_before_compaction, groups);
        assert_eq!(policy.max_transcript_chars, chars);
    }
}

#[test]
fn maintenance_is_an_explicit_operation_not_a_system_turn() {
    let policy = compaction_policy_for_source(CompactionSource::ContextMaintenance).unwrap();
    assert_eq!(policy.policy_version, "background-v1");
    assert_eq!(policy.protected_recent_group_count, 2);
    assert_eq!(policy.max_groups_before_compaction, 6);
    assert_eq!(policy.max_transcript_chars, 12_000);
    for origin in [
        TurnExecutionOrigin::InternalCuration,
        TurnExecutionOrigin::InboundObservation,
    ] {
        assert!(matches!(
            compaction_policy_for_source(CompactionSource::Turn(origin)),
            Err(DenError::Authorization(_))
        ));
    }
}
