use super::*;

#[test]
fn only_an_active_retained_head_can_reach_private_context_revalidation() {
    use service::{context::require_active_context_head, PersonalFactState};

    assert_denied(require_active_context_head(None));
    let fact_id = PersonalFactId(Uuid::new_v4());
    for state in [PersonalFactState::Retracted, PersonalFactState::Suppressed] {
        assert_denied(require_active_context_head(Some((fact_id, state))));
    }
    require_active_context_head(Some((fact_id, PersonalFactState::Active))).unwrap();
}

#[sqlx::test(migrations = "../../migrations")]
async fn revalidation_preconditions_deny_before_io_without_allocating_stores(pool: PgPool) {
    let fixture = Fixture::new(pool).await;
    let original = source(&fixture);
    let empty = prepare(&fixture, original).await.unwrap();
    assert_eq!(empty.fact_id(), None);
    fixture.enable(fixture.owner).await;
    let publication = publish_name(&fixture, original, fixture.source, ALICE_NAME, None).await;
    let active = prepare(&fixture, original).await.unwrap();
    revalidate(&fixture, original, &active).await.unwrap();
    assert!(service::retract_preferred_name(
        &fixture.pool,
        &fixture.stores,
        fixture.bear,
        fixture.owner,
        publication.fact_id,
    )
    .await
    .unwrap());
    let retracted = prepare(&fixture, original).await.unwrap();
    assert_eq!(retracted.fact_id(), Some(publication.fact_id));

    let pool = closed_pool().await;
    let directory =
        std::env::temp_dir().join(format!("personal-freshness-unused-{}", Uuid::new_v4()));
    let mut config = Config::test_stub();
    config.bear_sqlite_data_dir = directory.to_string_lossy().into_owned();
    let stores = MemoryStoreManager::new(&config);
    for reference in [&empty, &retracted] {
        for mode in [
            PersonalContextMode::DefaultDisabled,
            PersonalContextMode::OwnerApprovedPreferredName,
        ] {
            assert_denied(
                service::revalidate_personal_context(&pool, &stores, mode, original, reference)
                    .await,
            );
        }
    }
    for changed in [
        original,
        PersonalContextSource {
            user_id: fixture.other,
            ..original
        },
        PersonalContextSource {
            bear_id: Uuid::new_v4().into(),
            ..original
        },
        PersonalContextSource {
            conversation_id: Uuid::new_v4(),
            ..original
        },
        PersonalContextSource {
            origin: TurnExecutionOrigin::BrowserTaskSession,
            ..original
        },
        PersonalContextSource {
            origin: TurnExecutionOrigin::ArmatureConversation(ArmatureAvailability::Connected),
            ..original
        },
        PersonalContextSource {
            origin: TurnExecutionOrigin::AuthorizedWorkRun(ArmatureAvailability::Connected),
            ..original
        },
        PersonalContextSource {
            origin: TurnExecutionOrigin::AuthorizedWorkRun(ArmatureAvailability::Absent),
            ..original
        },
        PersonalContextSource {
            origin: TurnExecutionOrigin::InternalCuration,
            ..original
        },
        PersonalContextSource {
            origin: TurnExecutionOrigin::InboundObservation,
            ..original
        },
    ] {
        assert_denied(
            service::revalidate_personal_context(
                &pool,
                &stores,
                PersonalContextMode::DefaultDisabled,
                changed,
                &active,
            )
            .await,
        );
        if changed != original {
            assert_denied(
                service::revalidate_personal_context(
                    &pool,
                    &stores,
                    PersonalContextMode::OwnerApprovedPreferredName,
                    changed,
                    &active,
                )
                .await,
            );
        }
    }
    assert_eq!(active.fact_id(), Some(publication.fact_id));
    assert_eq!(empty.fact_id(), None);
    assert!(!directory.exists());
}
