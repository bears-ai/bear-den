use super::*;

#[test]
fn typed_origins_preserve_every_existing_profile_governance_and_armature_denial() {
    let origins = [
        (
            TurnExecutionOrigin::ChannelConversation,
            TrustProfile::Chat,
            ArmatureAvailability::Absent,
        ),
        (
            TurnExecutionOrigin::BrowserTaskSession,
            TrustProfile::Pair,
            ArmatureAvailability::Absent,
        ),
        (
            TurnExecutionOrigin::ArmatureConversation(ArmatureAvailability::Connected),
            TrustProfile::Pair,
            ArmatureAvailability::Connected,
        ),
        (
            TurnExecutionOrigin::ArmatureConversation(ArmatureAvailability::Absent),
            TrustProfile::Pair,
            ArmatureAvailability::Absent,
        ),
        (
            TurnExecutionOrigin::AuthorizedWorkRun(ArmatureAvailability::Connected),
            TrustProfile::Work,
            ArmatureAvailability::Connected,
        ),
        (
            TurnExecutionOrigin::AuthorizedWorkRun(ArmatureAvailability::Absent),
            TrustProfile::Work,
            ArmatureAvailability::Absent,
        ),
        (
            TurnExecutionOrigin::InternalCuration,
            TrustProfile::Curate,
            ArmatureAvailability::Absent,
        ),
        (
            TurnExecutionOrigin::InboundObservation,
            TrustProfile::Watch,
            ArmatureAvailability::Absent,
        ),
    ];
    for (origin, profile, armature) in origins {
        for governance in Governance::ALL {
            assert_eq!(
                EffectivePolicy::compile_for_origin(origin, governance),
                EffectivePolicy::compile(profile, governance, armature),
                "{origin:?}/{governance:?} must preserve existing denials",
            );
        }
    }
    let channel = EffectivePolicy::compile_for_origin(
        TurnExecutionOrigin::ChannelConversation,
        Governance::Interactive,
    );
    assert!(!channel
        .capabilities
        .contains(BearCapability::UseArmatureTools));
    assert!(!channel
        .capabilities
        .contains(BearCapability::OwnSessionTasks));
    let browser_tasks = EffectivePolicy::compile_for_origin(
        TurnExecutionOrigin::BrowserTaskSession,
        Governance::Interactive,
    );
    assert!(browser_tasks
        .capabilities
        .contains(BearCapability::OwnSessionTasks));
    assert!(!browser_tasks
        .capabilities
        .contains(BearCapability::UseArmatureTools));
    let disconnected = EffectivePolicy::compile_for_origin(
        TurnExecutionOrigin::ArmatureConversation(ArmatureAvailability::Absent),
        Governance::Interactive,
    );
    assert!(!disconnected
        .capabilities
        .contains(BearCapability::UseArmatureTools));
    let work = EffectivePolicy::compile_for_origin(
        TurnExecutionOrigin::AuthorizedWorkRun(ArmatureAvailability::Connected),
        Governance::Interactive,
    );
    assert!(work.capabilities.contains(BearCapability::ExecuteJob));
    assert!(!work.capabilities.contains(BearCapability::Converse));
}

#[test]
fn verified_origins_own_explicit_capability_sets() {
    use BearCapability::{
        Converse, CreateJob, CurateMemory, DispatchWork, ExecuteFocusedTask, ExecuteJob,
        ManageWorkSurfaces, OwnSessionTasks, ProposeProfileMemory, SelectSessionTask,
        UseArmatureTools, UseWorkSurfaces,
    };
    let cases = [
        (
            TurnExecutionOrigin::ChannelConversation,
            &[Converse, CreateJob, DispatchWork][..],
        ),
        (
            TurnExecutionOrigin::BrowserTaskSession,
            &[
                Converse,
                OwnSessionTasks,
                SelectSessionTask,
                ExecuteFocusedTask,
                ExecuteJob,
                CreateJob,
                DispatchWork,
                UseWorkSurfaces,
                ManageWorkSurfaces,
                ProposeProfileMemory,
            ][..],
        ),
        (
            TurnExecutionOrigin::ArmatureConversation(ArmatureAvailability::Connected),
            &[
                Converse,
                OwnSessionTasks,
                SelectSessionTask,
                ExecuteFocusedTask,
                ExecuteJob,
                CreateJob,
                DispatchWork,
                UseArmatureTools,
                UseWorkSurfaces,
                ManageWorkSurfaces,
                ProposeProfileMemory,
            ][..],
        ),
        (
            TurnExecutionOrigin::AuthorizedWorkRun(ArmatureAvailability::Connected),
            &[
                ExecuteFocusedTask,
                ExecuteJob,
                UseArmatureTools,
                UseWorkSurfaces,
                ProposeProfileMemory,
            ][..],
        ),
        (TurnExecutionOrigin::InternalCuration, &[CurateMemory][..]),
        (TurnExecutionOrigin::InboundObservation, &[][..]),
    ];
    for (origin, expected) in cases {
        assert_eq!(
            EffectivePolicy::compile_for_origin(origin, Governance::Interactive).capabilities,
            CapabilitySet::from_capabilities(expected.iter().copied()),
            "{origin:?} must keep its explicit capability set"
        );
    }
}

#[test]
fn a_shared_hat_identity_cannot_collapse_chat_pair_and_work_policy_defaults() {
    use BearCapability::{Converse, CreateJob, ExecuteJob, OwnSessionTasks, UseArmatureTools};

    // Hold governance and armature availability fixed: the policy differences
    // are not a consequence of changing a hat, client, or supervision label.
    let chat = EffectivePolicy::compile(
        TrustProfile::Chat,
        Governance::Interactive,
        ArmatureAvailability::Connected,
    );
    let pair = EffectivePolicy::compile(
        TrustProfile::Pair,
        Governance::Interactive,
        ArmatureAvailability::Connected,
    );
    let work = EffectivePolicy::compile(
        TrustProfile::Work,
        Governance::Interactive,
        ArmatureAvailability::Connected,
    );
    assert!(chat.capabilities.contains(Converse));
    assert!(chat.capabilities.contains(CreateJob));
    assert!(!chat.capabilities.contains(UseArmatureTools));
    assert!(!chat.capabilities.contains(OwnSessionTasks));
    assert!(pair.capabilities.contains(Converse));
    assert!(pair.capabilities.contains(UseArmatureTools));
    assert!(pair.capabilities.contains(OwnSessionTasks));
    assert!(work.capabilities.contains(ExecuteJob));
    assert!(work.capabilities.contains(UseArmatureTools));
    assert!(!work.capabilities.contains(Converse));
    assert!(!work.capabilities.contains(CreateJob));
}

#[test]
fn effective_capabilities_are_profile_defaults_filtered_by_runtime_context() {
    use BearCapability::{
        Converse, CreateJob, CurateMemory, DispatchWork, ExecuteFocusedTask, ManageWorkSurfaces,
        OwnSessionTasks, ProposeProfileMemory, UseArmatureTools,
    };

    let cases = [
        (
            TrustProfile::Chat,
            Governance::Interactive,
            ArmatureAvailability::Absent,
            &[Converse, CreateJob, DispatchWork][..],
            &[OwnSessionTasks, ExecuteFocusedTask][..],
        ),
        (
            TrustProfile::Pair,
            Governance::Interactive,
            ArmatureAvailability::Connected,
            &[
                Converse,
                OwnSessionTasks,
                ExecuteFocusedTask,
                UseArmatureTools,
                ManageWorkSurfaces,
            ][..],
            &[][..],
        ),
        (
            TrustProfile::Pair,
            Governance::AutonomousContinuation,
            ArmatureAvailability::Absent,
            &[Converse, OwnSessionTasks, ExecuteFocusedTask][..],
            &[UseArmatureTools, ManageWorkSurfaces][..],
        ),
        (
            TrustProfile::Pair,
            Governance::Observational,
            ArmatureAvailability::Connected,
            &[Converse][..],
            &[
                OwnSessionTasks,
                ExecuteFocusedTask,
                UseArmatureTools,
                CreateJob,
                DispatchWork,
                ManageWorkSurfaces,
                ProposeProfileMemory,
            ][..],
        ),
        (
            TrustProfile::Work,
            Governance::Interactive,
            ArmatureAvailability::Connected,
            &[ExecuteFocusedTask, UseArmatureTools][..],
            &[Converse, OwnSessionTasks, ManageWorkSurfaces][..],
        ),
        (
            TrustProfile::Curate,
            Governance::Interactive,
            ArmatureAvailability::Absent,
            &[CurateMemory][..],
            &[Converse, ExecuteFocusedTask, UseArmatureTools][..],
        ),
    ];

    for (profile, governance, armature, granted, denied) in cases {
        let policy = EffectivePolicy::compile(profile, governance, armature);
        for capability in granted {
            assert!(
                policy.capabilities.contains(*capability),
                "{profile:?}/{governance:?} should grant {capability:?}"
            );
        }
        for capability in denied {
            assert!(
                !policy.capabilities.contains(*capability),
                "{profile:?}/{governance:?} should deny {capability:?}"
            );
        }
    }
}
