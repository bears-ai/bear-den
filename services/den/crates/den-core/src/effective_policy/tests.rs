use super::*;

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
