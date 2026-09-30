use super::*;

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
