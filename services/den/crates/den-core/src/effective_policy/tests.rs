use super::*;

#[test]
fn verified_origins_have_explicit_capabilities_for_every_governance_and_availability() {
    use BearCapability::{
        Converse, CreateJob, CurateMemory, DispatchWork, ExecuteFocusedTask, ExecuteJob,
        ManageWorkSurfaces, OwnSessionTasks, ProposeProfileMemory, SelectSessionTask,
        UseArmatureTools, UseWorkSurfaces,
    };

    let channel_interactive = &[Converse, CreateJob, DispatchWork][..];
    let conversation_only = &[Converse][..];
    let task_interactive = &[
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
    ][..];
    let armature_interactive = &[
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
    ][..];
    let task_continuation = &[
        Converse,
        OwnSessionTasks,
        SelectSessionTask,
        ExecuteFocusedTask,
        ExecuteJob,
        UseWorkSurfaces,
        ProposeProfileMemory,
    ][..];
    let task_read_only = &[Converse, UseWorkSurfaces][..];
    let work_interactive = &[
        ExecuteFocusedTask,
        ExecuteJob,
        UseArmatureTools,
        UseWorkSurfaces,
        ProposeProfileMemory,
    ][..];
    let work_without_armature = &[
        ExecuteFocusedTask,
        ExecuteJob,
        UseWorkSurfaces,
        ProposeProfileMemory,
    ][..];
    let work_read_only = &[UseWorkSurfaces][..];
    let curation = &[CurateMemory][..];
    let none = &[][..];

    // Each row specifies Interactive, Grace, AutonomousContinuation,
    // Observational, and Frozen expectations independently of the compiler.
    let cases = [
        (
            TurnExecutionOrigin::ChannelConversation,
            [
                channel_interactive,
                conversation_only,
                conversation_only,
                conversation_only,
                conversation_only,
            ],
        ),
        (
            TurnExecutionOrigin::BrowserTaskSession,
            [
                task_interactive,
                task_continuation,
                task_continuation,
                task_read_only,
                task_read_only,
            ],
        ),
        (
            TurnExecutionOrigin::ArmatureConversation(ArmatureAvailability::Connected),
            [
                armature_interactive,
                task_continuation,
                task_continuation,
                task_read_only,
                task_read_only,
            ],
        ),
        (
            TurnExecutionOrigin::ArmatureConversation(ArmatureAvailability::Absent),
            [
                task_interactive,
                task_continuation,
                task_continuation,
                task_read_only,
                task_read_only,
            ],
        ),
        (
            TurnExecutionOrigin::AuthorizedWorkRun(ArmatureAvailability::Connected),
            [
                work_interactive,
                work_without_armature,
                work_without_armature,
                work_read_only,
                work_read_only,
            ],
        ),
        (
            TurnExecutionOrigin::AuthorizedWorkRun(ArmatureAvailability::Absent),
            [
                work_without_armature,
                work_without_armature,
                work_without_armature,
                work_read_only,
                work_read_only,
            ],
        ),
        (
            TurnExecutionOrigin::InternalCuration,
            [curation, curation, curation, none, none],
        ),
        (TurnExecutionOrigin::InboundObservation, [none; 5]),
    ];
    let all_capabilities = [
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
        CurateMemory,
    ];

    for (origin, expectations) in cases {
        for (governance, expected) in Governance::ALL.into_iter().zip(expectations) {
            let policy = EffectivePolicy::compile_for_origin(origin, governance);
            assert_eq!(policy.governance, governance);
            assert_eq!(
                policy.capabilities,
                CapabilitySet::from_capabilities(expected.iter().copied()),
                "{origin:?}/{governance:?} must grant exactly the expected capabilities",
            );
            for capability in all_capabilities {
                if expected.contains(&capability) {
                    assert!(
                        policy.capabilities.require(capability).is_ok(),
                        "{origin:?}/{governance:?} should grant {capability:?}",
                    );
                } else {
                    assert!(
                        !policy.capabilities.contains(capability),
                        "{origin:?}/{governance:?} should deny {capability:?}",
                    );
                    assert!(
                        matches!(
                            policy.capabilities.require(capability),
                            Err(DenError::Authorization(_))
                        ),
                        "{origin:?}/{governance:?} must reject {capability:?} with authorization denial",
                    );
                }
            }
        }
    }
}

#[test]
fn a_shared_hat_cannot_collapse_verified_conversation_and_work_surfaces() {
    use BearCapability::{Converse, CreateJob, ExecuteJob, OwnSessionTasks, UseArmatureTools};

    // A shared hat is not an authority input: the verified surface determines
    // capabilities even with the same governance and connected work harness.
    let channel = EffectivePolicy::compile_for_origin(
        TurnExecutionOrigin::ChannelConversation,
        Governance::Interactive,
    );
    let armature = EffectivePolicy::compile_for_origin(
        TurnExecutionOrigin::ArmatureConversation(ArmatureAvailability::Connected),
        Governance::Interactive,
    );
    let work = EffectivePolicy::compile_for_origin(
        TurnExecutionOrigin::AuthorizedWorkRun(ArmatureAvailability::Connected),
        Governance::Interactive,
    );
    assert!(channel.capabilities.contains(Converse));
    assert!(channel.capabilities.contains(CreateJob));
    assert!(!channel.capabilities.contains(UseArmatureTools));
    assert!(!channel.capabilities.contains(OwnSessionTasks));
    assert!(armature.capabilities.contains(Converse));
    assert!(armature.capabilities.contains(UseArmatureTools));
    assert!(armature.capabilities.contains(OwnSessionTasks));
    assert!(work.capabilities.contains(ExecuteJob));
    assert!(work.capabilities.contains(UseArmatureTools));
    assert!(!work.capabilities.contains(Converse));
    assert!(!work.capabilities.contains(CreateJob));
}

#[test]
fn ordinary_verified_origins_are_allowed_in_generic_sessions() {
    let origins = [
        TurnExecutionOrigin::ChannelConversation,
        TurnExecutionOrigin::BrowserTaskSession,
        TurnExecutionOrigin::ArmatureConversation(ArmatureAvailability::Connected),
        TurnExecutionOrigin::ArmatureConversation(ArmatureAvailability::Absent),
        TurnExecutionOrigin::AuthorizedWorkRun(ArmatureAvailability::Connected),
        TurnExecutionOrigin::AuthorizedWorkRun(ArmatureAvailability::Absent),
    ];
    for origin in origins {
        assert!(
            origin.require_ordinary_session().is_ok(),
            "{origin:?} is an ordinary verified session source",
        );
    }
}

#[test]
fn system_origins_require_dedicated_operations_not_generic_sessions() {
    for origin in [
        TurnExecutionOrigin::InternalCuration,
        TurnExecutionOrigin::InboundObservation,
    ] {
        assert!(
            matches!(
                origin.require_ordinary_session(),
                Err(DenError::Authorization(_))
            ),
            "{origin:?} must be rejected at the generic session boundary",
        );
    }
}
