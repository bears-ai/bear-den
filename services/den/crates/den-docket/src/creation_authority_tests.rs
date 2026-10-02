use super::*;
use crate::{
    integration_tests::{seed_user_and_bear, two_task_job},
    DocketService, PgDocketService,
};
use den_core::ArmatureAvailability;
use sqlx::PgPool;

#[test]
fn job_creation_follows_verified_origin_and_governance() {
    for origin in [
        TurnExecutionOrigin::ChannelConversation,
        TurnExecutionOrigin::BrowserTaskSession,
        TurnExecutionOrigin::ArmatureConversation(ArmatureAvailability::Connected),
        TurnExecutionOrigin::ArmatureConversation(ArmatureAvailability::Absent),
        TurnExecutionOrigin::AuthorizedWorkRun(ArmatureAvailability::Connected),
        TurnExecutionOrigin::AuthorizedWorkRun(ArmatureAvailability::Absent),
        TurnExecutionOrigin::InternalCuration,
        TurnExecutionOrigin::InboundObservation,
    ] {
        for governance in [
            Governance::Interactive,
            Governance::Grace,
            Governance::AutonomousContinuation,
            Governance::Observational,
            Governance::Frozen,
        ] {
            let authority = DocketJobCreationAuthority::NativeTurn { origin, governance };
            assert_eq!(
                authority.require_create_job().is_ok(),
                EffectivePolicy::compile_for_origin(origin, governance)
                    .capabilities
                    .contains(BearCapability::CreateJob),
                "{origin:?}/{governance:?}"
            );
        }
    }
}

#[tokio::test]
async fn denied_native_job_creation_does_not_touch_storage_or_trust_ui_audit() {
    let pool = PgPool::connect_lazy("postgres://unused:unused@localhost/unused").unwrap();
    let service = PgDocketService::from_pool(&pool);
    for (origin, governance) in [
        (
            TurnExecutionOrigin::AuthorizedWorkRun(ArmatureAvailability::Connected),
            Governance::Interactive,
        ),
        (
            TurnExecutionOrigin::InternalCuration,
            Governance::Interactive,
        ),
        (
            TurnExecutionOrigin::InboundObservation,
            Governance::Interactive,
        ),
        (
            TurnExecutionOrigin::ArmatureConversation(ArmatureAvailability::Connected),
            Governance::AutonomousContinuation,
        ),
        (TurnExecutionOrigin::ChannelConversation, Governance::Frozen),
    ] {
        let mut create = two_task_job(1, uuid::Uuid::new_v4());
        create.created_by_role = "ui".into();
        let result = service
            .create_job(
                create,
                DocketJobCreationAuthority::NativeTurn { origin, governance },
            )
            .await;
        assert!(
            matches!(result, Err(DenError::Authorization(_))),
            "{origin:?}/{governance:?}: {result:?}"
        );
    }
}

#[sqlx::test(migrations = "../../migrations")]
async fn job_creation_rechecks_membership_independently_of_creator_provenance(pool: PgPool) {
    let (user, bear_id) = seed_user_and_bear(&pool, "creation-authority").await;
    let (other_user, other_bear) = seed_user_and_bear(&pool, "creation-other").await;
    let service = PgDocketService::from_pool(&pool);
    let mut create = two_task_job(user, bear_id);
    create.created_by_role = "work".into();
    let job = service
        .create_job(
            create,
            DocketJobCreationAuthority::NativeTurn {
                origin: TurnExecutionOrigin::ChannelConversation,
                governance: Governance::Interactive,
            },
        )
        .await
        .unwrap();
    assert_eq!(job.job.created_by_role, "work");

    for (actor, bear) in [(other_user, bear_id), (user, other_bear)] {
        let mut create = two_task_job(actor, bear);
        create.created_by_role = "ui".into();
        assert!(matches!(
            service
                .create_job(create, DocketJobCreationAuthority::HumanRequest)
                .await,
            Err(DenError::Authorization(_))
        ));
    }
    sqlx::query!(
        "DELETE FROM user_bear WHERE user_id = $1 AND bear_id = $2",
        user,
        bear_id
    )
    .execute(&pool)
    .await
    .unwrap();
    assert!(matches!(
        service
            .create_job(
                two_task_job(user, bear_id),
                DocketJobCreationAuthority::HumanRequest
            )
            .await,
        Err(DenError::Authorization(_))
    ));
    let jobs = service
        .list_jobs(bear_id, crate::DocketJobListFilter::default())
        .await
        .unwrap();
    assert_eq!(jobs.len(), 1, "denied creation must not persist a Job");
}
