use super::support::{assert_denied, conversation, Fixture};
use crate::{
    bears::{db, hats},
    personal_memory as service,
};
use den_core::{config::Config, ids::UserId, ArmatureAvailability, DenError, TurnExecutionOrigin};
use den_memory::{append_memory_record, LogicalMemoryPath, MemorySource, MemoryStoreManager};
use service::{
    PersonalContextMode, PersonalContextReference, PersonalContextSource, PersonalFactId,
    PublicationReceipt, RenderedPersonalContext,
};
use sqlx::{
    postgres::{PgConnectOptions, PgPoolOptions},
    PgPool,
};
use std::time::Duration;
use uuid::Uuid;

#[path = "context_freshness.rs"]
mod freshness;

const ALICE_NAME: &str = "AliceContextSentinel-7C92";
const BOB_NAME: &str = "BobContextSentinel-4A18";

fn source(fixture: &Fixture) -> PersonalContextSource {
    PersonalContextSource {
        bear_id: fixture.bear,
        user_id: fixture.owner,
        conversation_id: fixture.conversation,
        origin: TurnExecutionOrigin::ChannelConversation,
    }
}

async fn closed_pool() -> PgPool {
    let pool = PgPoolOptions::new()
        .acquire_timeout(Duration::from_millis(50))
        .connect_lazy_with(PgConnectOptions::new().host("127.0.0.1").port(1));
    pool.close().await;
    pool
}

async fn publish_name(
    fixture: &Fixture,
    source: PersonalContextSource,
    note: Uuid,
    value: &str,
    expected: Option<PersonalFactId>,
) -> PublicationReceipt {
    let candidate = service::stage_preferred_name(
        &fixture.pool,
        &fixture.stores,
        source.bear_id,
        source.user_id,
        source.conversation_id,
        note,
        service::PreferredName::new(value).unwrap(),
    )
    .await
    .unwrap();
    service::publish_preferred_name(
        &fixture.pool,
        &fixture.stores,
        source.bear_id,
        source.user_id,
        candidate.id(),
        expected,
    )
    .await
    .unwrap()
}

async fn prepare(
    fixture: &Fixture,
    source: PersonalContextSource,
) -> Result<PersonalContextReference, DenError> {
    Ok(service::prepare_personal_context(
        &fixture.pool,
        &fixture.stores,
        PersonalContextMode::OwnerApprovedPreferredName,
        source,
    )
    .await?
    .unwrap())
}

async fn render(
    fixture: &Fixture,
    source: PersonalContextSource,
    reference: &PersonalContextReference,
) -> Result<Option<RenderedPersonalContext>, DenError> {
    service::render_personal_context(
        &fixture.pool,
        &fixture.stores,
        PersonalContextMode::OwnerApprovedPreferredName,
        source,
        reference,
    )
    .await
}

async fn revalidate(
    fixture: &Fixture,
    source: PersonalContextSource,
    reference: &PersonalContextReference,
) -> Result<(), DenError> {
    service::revalidate_personal_context(
        &fixture.pool,
        &fixture.stores,
        PersonalContextMode::OwnerApprovedPreferredName,
        source,
        reference,
    )
    .await
}

#[tokio::test]
async fn disabled_and_nonhuman_context_preparation_do_not_touch_either_store() {
    let pool = closed_pool().await;
    let mut config = Config::test_stub();
    let directory =
        std::env::temp_dir().join(format!("personal-context-unused-{}", Uuid::new_v4()));
    config.bear_sqlite_data_dir = directory.to_string_lossy().into_owned();
    let stores = MemoryStoreManager::new(&config);
    let source = PersonalContextSource {
        bear_id: Uuid::new_v4().into(),
        user_id: UserId::new(1),
        conversation_id: Uuid::new_v4(),
        origin: TurnExecutionOrigin::ChannelConversation,
    };
    assert_eq!(
        PersonalContextMode::default(),
        PersonalContextMode::DefaultDisabled
    );
    for origin in [
        source.origin,
        TurnExecutionOrigin::AuthorizedWorkRun(ArmatureAvailability::Connected),
        TurnExecutionOrigin::AuthorizedWorkRun(ArmatureAvailability::Absent),
        TurnExecutionOrigin::InternalCuration,
        TurnExecutionOrigin::InboundObservation,
    ] {
        let source = PersonalContextSource { origin, ..source };
        assert!(service::prepare_personal_context(
            &pool,
            &stores,
            PersonalContextMode::DefaultDisabled,
            source,
        )
        .await
        .unwrap()
        .is_none());
        if origin != TurnExecutionOrigin::ChannelConversation {
            assert_denied(
                service::prepare_personal_context(
                    &pool,
                    &stores,
                    PersonalContextMode::OwnerApprovedPreferredName,
                    source,
                )
                .await,
            );
        }
    }
    assert!(!directory.exists());
}

#[sqlx::test(migrations = "../../migrations")]
async fn context_is_owner_bound_and_reference_mismatches_or_disabled_render_need_no_io(
    pool: PgPool,
) {
    let fixture = Fixture::new(pool).await;
    fixture.enable(fixture.owner).await;
    let alice = source(&fixture);
    let publication = publish_name(&fixture, alice, fixture.source, ALICE_NAME, None).await;
    let reference = prepare(&fixture, alice).await.unwrap();
    assert_eq!(reference.fact_id(), Some(publication.fact_id));
    assert_eq!(
        format!("{reference:?}"),
        "PersonalContextReference([redacted])"
    );
    revalidate(&fixture, alice, &reference).await.unwrap();
    let rendered = render(&fixture, alice, &reference).await.unwrap().unwrap();
    assert!(rendered.as_str().contains(ALICE_NAME));
    assert_eq!(rendered.char_count(), rendered.as_str().chars().count());
    assert_eq!(
        format!("{rendered:?}"),
        "RenderedPersonalContext([redacted])"
    );

    for user_id in [fixture.other, fixture.outsider] {
        assert_denied(prepare(&fixture, PersonalContextSource { user_id, ..alice }).await);
    }
    let bob = PersonalContextSource {
        user_id: fixture.other,
        conversation_id: conversation(
            &fixture.pool,
            fixture.bear,
            Some(fixture.other),
            Some(fixture.hat),
        )
        .await,
        ..alice
    };
    let bob_note = append_memory_record(
        &fixture.store().await,
        &LogicalMemoryPath::source_local(MemorySource::Conversation(bob.conversation_id), "note"),
        "note",
        "pair",
        None,
        "Bob's private canonical evidence, not a context payload.",
        &serde_json::json!({}),
    )
    .await
    .unwrap();
    fixture.enable(fixture.other).await;
    publish_name(
        &fixture,
        bob,
        Uuid::parse_str(&bob_note.memory_id).unwrap(),
        BOB_NAME,
        None,
    )
    .await;
    let bob_reference = prepare(&fixture, bob).await.unwrap();
    revalidate(&fixture, bob, &bob_reference).await.unwrap();
    assert_denied(revalidate(&fixture, alice, &bob_reference).await);
    assert_denied(revalidate(&fixture, bob, &reference).await);
    let bob_rendered = render(&fixture, bob, &bob_reference)
        .await
        .unwrap()
        .unwrap();
    assert!(bob_rendered.as_str().contains(BOB_NAME));
    assert!(!bob_rendered.as_str().contains(ALICE_NAME));
    assert!(!rendered.as_str().contains(BOB_NAME));

    let pool = closed_pool().await;
    let mut config = Config::test_stub();
    let directory =
        std::env::temp_dir().join(format!("personal-context-unused-{}", Uuid::new_v4()));
    config.bear_sqlite_data_dir = directory.to_string_lossy().into_owned();
    let stores = MemoryStoreManager::new(&config);
    let mismatches = [
        bob,
        PersonalContextSource {
            user_id: fixture.outsider,
            ..alice
        },
        PersonalContextSource {
            bear_id: Uuid::new_v4().into(),
            ..alice
        },
        PersonalContextSource {
            conversation_id: Uuid::new_v4(),
            ..alice
        },
        PersonalContextSource {
            origin: TurnExecutionOrigin::BrowserTaskSession,
            ..alice
        },
        PersonalContextSource {
            origin: TurnExecutionOrigin::AuthorizedWorkRun(ArmatureAvailability::Connected),
            ..alice
        },
        PersonalContextSource {
            origin: TurnExecutionOrigin::InternalCuration,
            ..alice
        },
        PersonalContextSource {
            origin: TurnExecutionOrigin::InboundObservation,
            ..alice
        },
    ];
    for source in std::iter::once(alice).chain(mismatches) {
        assert_denied(
            service::revalidate_personal_context(
                &pool,
                &stores,
                PersonalContextMode::DefaultDisabled,
                source,
                &reference,
            )
            .await,
        );
        assert!(service::render_personal_context(
            &pool,
            &stores,
            PersonalContextMode::DefaultDisabled,
            source,
            &reference,
        )
        .await
        .unwrap()
        .is_none());
        if source != alice {
            assert_denied(
                service::revalidate_personal_context(
                    &pool,
                    &stores,
                    PersonalContextMode::OwnerApprovedPreferredName,
                    source,
                    &reference,
                )
                .await,
            );
            let result = service::render_personal_context(
                &pool,
                &stores,
                PersonalContextMode::OwnerApprovedPreferredName,
                source,
                &reference,
            )
            .await;
            assert!(matches!(result, Err(DenError::Authorization(ref message))
                if message == "Personal-memory context is stale or no longer authorized."));
        }
    }
    assert!(!directory.exists());
}

#[sqlx::test(migrations = "../../migrations")]
async fn fresh_owned_conversations_across_hats_and_human_origins_share_only_the_approved_name(
    pool: PgPool,
) {
    let fixture = Fixture::new(pool).await;
    fixture.enable(fixture.owner).await;
    let original = source(&fixture);
    let publication = publish_name(&fixture, original, fixture.source, ALICE_NAME, None).await;
    let second_hat = hats::create_hat(
        &fixture.pool,
        fixture.bear,
        fixture.owner,
        "Other context hat",
        "Exercise cross-hat owner context",
    )
    .await
    .unwrap();
    for (hat, origin) in [
        (fixture.hat, TurnExecutionOrigin::ChannelConversation),
        (second_hat.id, TurnExecutionOrigin::BrowserTaskSession),
        (
            second_hat.id,
            TurnExecutionOrigin::ArmatureConversation(ArmatureAvailability::Connected),
        ),
        (
            fixture.hat,
            TurnExecutionOrigin::ArmatureConversation(ArmatureAvailability::Absent),
        ),
    ] {
        let fresh = PersonalContextSource {
            conversation_id: conversation(
                &fixture.pool,
                fixture.bear,
                Some(fixture.owner),
                Some(hat),
            )
            .await,
            origin,
            ..original
        };
        assert_ne!(fresh.conversation_id, original.conversation_id);
        let reference = prepare(&fixture, fresh).await.unwrap();
        revalidate(&fixture, fresh, &reference).await.unwrap();
        assert_eq!(reference.fact_id(), Some(publication.fact_id));
        let rendered = render(&fixture, fresh, &reference).await.unwrap().unwrap();
        assert!(rendered.as_str().contains(ALICE_NAME));
        for hidden in [
            original.conversation_id.to_string(),
            fixture.source.to_string(),
            publication.fact_id.to_string(),
            "Private evidence stays in the source".to_owned(),
        ] {
            assert!(!rendered.as_str().contains(&hidden));
        }
    }
}

#[sqlx::test(migrations = "../../migrations")]
async fn head_creation_correction_retraction_and_optout_require_new_context_references(
    pool: PgPool,
) {
    let fixture = Fixture::new(pool).await;
    fixture.enable(fixture.owner).await;
    let source = source(&fixture);
    let empty = prepare(&fixture, source).await.unwrap();
    assert_eq!(empty.fact_id(), None);
    assert!(render(&fixture, source, &empty).await.unwrap().is_none());
    assert_denied(revalidate(&fixture, source, &empty).await);
    let original = publish_name(&fixture, source, fixture.source, ALICE_NAME, None).await;
    assert_denied(render(&fixture, source, &empty).await);
    assert_denied(revalidate(&fixture, source, &empty).await);
    let first = prepare(&fixture, source).await.unwrap();
    revalidate(&fixture, source, &first).await.unwrap();
    let corrected = publish_name(
        &fixture,
        source,
        fixture.source,
        "AliciaContextSentinel-2B65",
        Some(original.fact_id),
    )
    .await;
    assert_denied(render(&fixture, source, &first).await);
    assert_denied(revalidate(&fixture, source, &first).await);
    let second = prepare(&fixture, source).await.unwrap();
    revalidate(&fixture, source, &second).await.unwrap();
    let current = render(&fixture, source, &second).await.unwrap().unwrap();
    assert!(current.as_str().contains("AliciaContextSentinel-2B65"));
    assert!(!current.as_str().contains(ALICE_NAME));
    assert!(service::retract_preferred_name(
        &fixture.pool,
        &fixture.stores,
        fixture.bear,
        fixture.owner,
        corrected.fact_id,
    )
    .await
    .unwrap());
    assert_denied(render(&fixture, source, &second).await);
    assert_denied(revalidate(&fixture, source, &second).await);
    let retracted = prepare(&fixture, source).await.unwrap();
    assert_eq!(retracted.fact_id(), Some(corrected.fact_id));
    assert_denied(revalidate(&fixture, source, &retracted).await);
    assert!(render(&fixture, source, &retracted)
        .await
        .unwrap()
        .is_none());
    let replacement = publish_name(
        &fixture,
        source,
        fixture.source,
        "AllyContextSentinel-9D31",
        Some(corrected.fact_id),
    )
    .await;
    assert_denied(render(&fixture, source, &retracted).await);
    assert_denied(revalidate(&fixture, source, &retracted).await);
    let third = prepare(&fixture, source).await.unwrap();
    revalidate(&fixture, source, &third).await.unwrap();
    service::set_capture_enabled(
        &fixture.pool,
        &fixture.stores,
        fixture.bear,
        fixture.owner,
        false,
    )
    .await
    .unwrap();
    assert_denied(render(&fixture, source, &third).await);
    assert_denied(revalidate(&fixture, source, &third).await);
    let opted_out = prepare(&fixture, source).await.unwrap();
    assert_eq!(opted_out.fact_id(), Some(replacement.fact_id));
    assert_denied(revalidate(&fixture, source, &opted_out).await);
    assert!(render(&fixture, source, &opted_out)
        .await
        .unwrap()
        .is_none());
    service::set_capture_enabled(
        &fixture.pool,
        &fixture.stores,
        fixture.bear,
        fixture.owner,
        true,
    )
    .await
    .unwrap();
    assert_denied(revalidate(&fixture, source, &opted_out).await);
    assert!(render(&fixture, source, &opted_out)
        .await
        .unwrap()
        .is_none());
}

#[sqlx::test(migrations = "../../migrations")]
async fn context_rechecks_current_conversation_and_membership_rights_on_every_read(pool: PgPool) {
    let fixture = Fixture::new(pool).await;
    fixture.enable(fixture.owner).await;
    let source = source(&fixture);
    publish_name(&fixture, source, fixture.source, ALICE_NAME, None).await;
    let reference = prepare(&fixture, source).await.unwrap();
    revalidate(&fixture, source, &reference).await.unwrap();
    for (status, owner, hat) in [
        ("archived", Some(fixture.owner), Some(fixture.hat)),
        ("active", Some(fixture.other), Some(fixture.hat)),
        ("active", Some(fixture.owner), None),
    ] {
        fixture.set_conversation_state(status, owner, hat).await;
        assert_denied(prepare(&fixture, source).await);
        assert_denied(render(&fixture, source, &reference).await);
        assert_denied(revalidate(&fixture, source, &reference).await);
    }
    fixture
        .set_conversation_state("active", Some(fixture.owner), Some(fixture.hat))
        .await;
    assert!(render(&fixture, source, &reference)
        .await
        .unwrap()
        .is_some());
    revalidate(&fixture, source, &reference).await.unwrap();
    db::revoke_membership(&fixture.pool, fixture.owner.get(), fixture.bear.as_uuid())
        .await
        .unwrap();
    assert_denied(prepare(&fixture, source).await);
    assert_denied(render(&fixture, source, &reference).await);
    assert_denied(revalidate(&fixture, source, &reference).await);
}

#[sqlx::test(migrations = "../../migrations")]
async fn context_does_not_require_the_original_publication_conversation_to_remain_active(
    pool: PgPool,
) {
    let fixture = Fixture::new(pool).await;
    fixture.enable(fixture.owner).await;
    let original = source(&fixture);
    publish_name(&fixture, original, fixture.source, ALICE_NAME, None).await;
    let fresh = PersonalContextSource {
        conversation_id: conversation(
            &fixture.pool,
            fixture.bear,
            Some(fixture.owner),
            Some(fixture.hat),
        )
        .await,
        ..original
    };
    let reference = prepare(&fixture, fresh).await.unwrap();
    fixture
        .set_conversation_state("archived", Some(fixture.owner), Some(fixture.hat))
        .await;
    assert!(render(&fixture, fresh, &reference)
        .await
        .unwrap()
        .unwrap()
        .as_str()
        .contains(ALICE_NAME));
    revalidate(&fixture, fresh, &reference).await.unwrap();
    let fresh_reference = prepare(&fixture, fresh).await.unwrap();
    revalidate(&fixture, fresh, &fresh_reference).await.unwrap();
    assert!(render(&fixture, fresh, &fresh_reference)
        .await
        .unwrap()
        .is_some());
}

#[sqlx::test(migrations = "../../migrations")]
async fn reconnected_same_person_with_capture_disabled_never_revives_an_old_context_reference(
    pool: PgPool,
) {
    use den_memory::{
        identity_reconnection::{
            commit_review, review_reconnection, review_revocation, ReplacementConsent,
        },
        personal_candidates::capture_enabled,
    };

    let fixture = Fixture::new(pool).await;
    fixture.enable(fixture.owner).await;
    let source = source(&fixture);
    let publication = publish_name(&fixture, source, fixture.source, ALICE_NAME, None).await;
    let old = prepare(&fixture, source).await.unwrap();
    revalidate(&fixture, source, &old).await.unwrap();
    let store = fixture.store().await;
    let binding = fixture.binding(fixture.owner).await;
    // This fixture supplies trusted Admin/member authorization to the memory boundary;
    // the human reconnection HTTP workflow has its own authorization coverage.
    let revoke = review_revocation(&store, fixture.other, binding.entity_id())
        .await
        .unwrap();
    commit_review(&store, &revoke, ReplacementConsent::NoReplacement)
        .await
        .unwrap();
    assert_denied(render(&fixture, source, &old).await);
    assert_denied(revalidate(&fixture, source, &old).await);
    let reconnect = review_reconnection(&store, fixture.other, binding.entity_id(), fixture.owner)
        .await
        .unwrap();
    commit_review(&store, &reconnect, ReplacementConsent::NoReplacement)
        .await
        .unwrap();
    let current_binding = fixture.binding(fixture.owner).await;
    assert_eq!(current_binding.entity_id(), binding.entity_id());
    assert!(!capture_enabled(&store, &current_binding).await.unwrap());
    assert_denied(render(&fixture, source, &old).await);
    assert_denied(revalidate(&fixture, source, &old).await);
    let fresh = prepare(&fixture, source).await.unwrap();
    revalidate(&fixture, source, &fresh).await.unwrap();
    assert_eq!(fresh.fact_id(), Some(publication.fact_id));
    assert!(render(&fixture, source, &fresh)
        .await
        .unwrap()
        .unwrap()
        .as_str()
        .contains(ALICE_NAME));
}
