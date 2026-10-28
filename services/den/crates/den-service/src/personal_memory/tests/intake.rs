use super::support::{assert_denied, new_bear, Fixture};
use crate::{bears::db, personal_memory as service};
use den_core::{ids::UserId, DenError};
use den_memory::{
    personal_candidates::{self, CandidateSource, CandidateState},
    personal_facts,
};
use sqlx::PgPool;
use uuid::Uuid;

use super::super::intake::stage_owner_preferred_name;

async fn stage(
    fixture: &Fixture,
    actor: UserId,
    request: Uuid,
    name: &str,
) -> Result<service::PersonalCandidate, DenError> {
    stage_owner_preferred_name(
        &fixture.pool,
        &fixture.stores,
        fixture.bear,
        actor,
        request,
        service::PreferredName::new(name).unwrap(),
    )
    .await
}

#[sqlx::test(migrations = "../../migrations")]
async fn human_settings_intake_needs_opt_in_but_no_conversation_or_note(pool: PgPool) {
    let fixture = Fixture::new(pool).await;
    let bear = new_bear(&fixture.pool, "owner-settings-only").await;
    db::grant_membership(&fixture.pool, fixture.owner.get(), bear.as_uuid(), None)
        .await
        .unwrap();
    let request = Uuid::new_v4();
    assert_denied(
        stage_owner_preferred_name(
            &fixture.pool,
            &fixture.stores,
            bear,
            fixture.owner,
            request,
            service::PreferredName::new("Alice").unwrap(),
        )
        .await,
    );
    service::set_capture_enabled(&fixture.pool, &fixture.stores, bear, fixture.owner, true)
        .await
        .unwrap();
    let candidate = stage_owner_preferred_name(
        &fixture.pool,
        &fixture.stores,
        bear,
        fixture.owner,
        request,
        service::PreferredName::new("Alice").unwrap(),
    )
    .await
    .unwrap();
    assert_eq!(
        candidate.source(),
        CandidateSource::OwnerRequest {
            request_id: request
        }
    );
    assert_eq!(candidate.source_conversation_id(), None);
    assert_eq!(candidate.source_memory_id(), None);
    assert_eq!(candidate.state(), CandidateState::Pending);
    assert!(
        service::current_preferred_name(&fixture.pool, &fixture.stores, bear, fixture.owner)
            .await
            .unwrap()
            .is_none()
    );
    let store = fixture.stores.store_for_bear(bear.as_uuid()).await.unwrap();
    assert!(den_memory::list_reviewable_memory_proposals(&store)
        .await
        .unwrap()
        .is_empty());
    let approved = service::publish_preferred_name(
        &fixture.pool,
        &fixture.stores,
        bear,
        fixture.owner,
        candidate.id(),
        None,
    )
    .await
    .unwrap();
    assert_eq!(approved.candidate_id, candidate.id());
}

#[sqlx::test(migrations = "../../migrations")]
async fn owner_request_id_replays_identical_payload_and_rejects_other_actor_or_changed_name(
    pool: PgPool,
) {
    let fixture = Fixture::new(pool).await;
    fixture.enable(fixture.owner).await;
    fixture.enable(fixture.other).await;
    let request = Uuid::new_v4();
    let original = stage(&fixture, fixture.owner, request, "Alice")
        .await
        .unwrap();
    let replay = stage(&fixture, fixture.owner, request, " Alice ")
        .await
        .unwrap();
    assert_eq!(replay.id(), original.id());
    assert_eq!(replay.created_at(), original.created_at());
    assert_denied(stage(&fixture, fixture.owner, request, "Alicia").await);
    assert_denied(stage(&fixture, fixture.other, request, "Alice").await);
    assert_denied(stage(&fixture, fixture.outsider, Uuid::new_v4(), "Alice").await);
    assert_denied(
        service::publish_preferred_name(
            &fixture.pool,
            &fixture.stores,
            fixture.bear,
            fixture.other,
            original.id(),
            None,
        )
        .await,
    );
    let owner = fixture.binding(fixture.owner).await;
    assert_eq!(
        personal_candidates::list_candidates(&fixture.store().await, &owner, 100)
            .await
            .unwrap()
            .len(),
        1
    );
    db::revoke_membership(&fixture.pool, fixture.owner.get(), fixture.bear.as_uuid())
        .await
        .unwrap();
    assert_denied(stage(&fixture, fixture.owner, request, "Alice").await);
    assert_denied(
        service::publish_preferred_name(
            &fixture.pool,
            &fixture.stores,
            fixture.bear,
            fixture.owner,
            original.id(),
            None,
        )
        .await,
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn owner_publication_and_correction_are_separate_and_ignore_unrelated_conversation_authority(
    pool: PgPool,
) {
    let fixture = Fixture::new(pool).await;
    fixture.enable(fixture.owner).await;
    fixture
        .set_conversation_state("archived", Some(fixture.other), None)
        .await;
    let first = stage(&fixture, fixture.owner, Uuid::new_v4(), "Alice")
        .await
        .unwrap();
    let deferred = service::review_candidate(
        &fixture.pool,
        &fixture.stores,
        fixture.bear,
        fixture.owner,
        first.id(),
        service::CandidateReviewDecision::Defer,
        None,
    )
    .await
    .unwrap();
    service::review_candidate(
        &fixture.pool,
        &fixture.stores,
        fixture.bear,
        fixture.owner,
        first.id(),
        service::CandidateReviewDecision::Resume,
        Some(deferred.review_id),
    )
    .await
    .unwrap();
    let receipt = service::publish_preferred_name(
        &fixture.pool,
        &fixture.stores,
        fixture.bear,
        fixture.owner,
        first.id(),
        None,
    )
    .await
    .unwrap();
    let request = Uuid::new_v4();
    let correction = stage(&fixture, fixture.owner, request, "Alicia")
        .await
        .unwrap();
    let head = service::current_preferred_name(
        &fixture.pool,
        &fixture.stores,
        fixture.bear,
        fixture.owner,
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(head.fact_id, receipt.fact_id);
    assert_eq!(head.preferred_name.unwrap().as_str(), "Alice");
    assert_denied(
        service::publish_preferred_name(
            &fixture.pool,
            &fixture.stores,
            fixture.bear,
            fixture.owner,
            correction.id(),
            None,
        )
        .await,
    );
    let corrected = service::publish_preferred_name(
        &fixture.pool,
        &fixture.stores,
        fixture.bear,
        fixture.owner,
        correction.id(),
        Some(receipt.fact_id),
    )
    .await
    .unwrap();
    assert_eq!(corrected.previous_fact_id, Some(receipt.fact_id));
    assert_eq!(
        stage(&fixture, fixture.owner, request, "Alicia")
            .await
            .unwrap()
            .id(),
        correction.id()
    );
    assert_denied(stage(&fixture, fixture.owner, request, "Ally").await);
    let head = service::current_preferred_name(
        &fixture.pool,
        &fixture.stores,
        fixture.bear,
        fixture.owner,
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(head.fact_id, corrected.fact_id);
    assert_eq!(head.preferred_name.unwrap().as_str(), "Alicia");
    let pending = stage(&fixture, fixture.owner, Uuid::new_v4(), "Ally")
        .await
        .unwrap();
    service::set_capture_enabled(
        &fixture.pool,
        &fixture.stores,
        fixture.bear,
        fixture.owner,
        false,
    )
    .await
    .unwrap();
    service::set_capture_enabled(
        &fixture.pool,
        &fixture.stores,
        fixture.bear,
        fixture.owner,
        true,
    )
    .await
    .unwrap();
    assert_denied(
        service::publish_preferred_name(
            &fixture.pool,
            &fixture.stores,
            fixture.bear,
            fixture.owner,
            pending.id(),
            Some(corrected.fact_id),
        )
        .await,
    );
    assert_eq!(
        service::publish_preferred_name(
            &fixture.pool,
            &fixture.stores,
            fixture.bear,
            fixture.owner,
            correction.id(),
            None,
        )
        .await
        .unwrap(),
        corrected
    );
    assert_eq!(
        service::current_preferred_name(
            &fixture.pool,
            &fixture.stores,
            fixture.bear,
            fixture.owner,
        )
        .await
        .unwrap()
        .unwrap()
        .state,
        service::PersonalFactState::Retracted
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn imported_owner_request_stays_quarantined_after_reconnection_and_fresh_opt_in(
    pool: PgPool,
) {
    use den_memory::identity_reconnection::{
        commit_review, review_reconnection, ReplacementConsent,
    };
    let fixture = Fixture::new(pool).await;
    fixture.enable(fixture.owner).await;
    let request = Uuid::new_v4();
    let original = stage(&fixture, fixture.owner, request, "Alice")
        .await
        .unwrap();
    let store = fixture.store().await;
    let old = fixture.binding(fixture.owner).await;
    den_memory::quarantine_imported_authenticated_users(&store)
        .await
        .unwrap();
    assert_denied(
        service::publish_preferred_name(
            &fixture.pool,
            &fixture.stores,
            fixture.bear,
            fixture.owner,
            original.id(),
            None,
        )
        .await,
    );
    assert_denied(stage(&fixture, fixture.owner, request, "Alice").await);
    let review = review_reconnection(&store, fixture.other, old.entity_id(), fixture.owner)
        .await
        .unwrap();
    commit_review(
        &store,
        &review,
        ReplacementConsent::PreserveDisplacedIdentity,
    )
    .await
    .unwrap();
    let current = fixture.binding(fixture.owner).await;
    assert!(!personal_candidates::capture_enabled(&store, &current)
        .await
        .unwrap());
    fixture.enable(fixture.owner).await;
    let historical = personal_candidates::get_candidate(&store, &current, original.id())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(historical.state(), CandidateState::Quarantined);
    assert_eq!(historical.source(), original.source());
    assert_denied(stage(&fixture, fixture.owner, request, "Alice").await);
    assert_denied(
        service::publish_preferred_name(
            &fixture.pool,
            &fixture.stores,
            fixture.bear,
            fixture.owner,
            original.id(),
            None,
        )
        .await,
    );
    assert!(personal_facts::current_preferred_name(&store, &current)
        .await
        .unwrap()
        .is_none());
    let fresh = stage(&fixture, fixture.owner, Uuid::new_v4(), "Alice")
        .await
        .unwrap();
    assert_ne!(fresh.id(), original.id());
}

#[sqlx::test(migrations = "../../migrations")]
async fn conversation_evidence_intake_and_approval_keep_their_existing_guards(pool: PgPool) {
    let fixture = Fixture::new(pool).await;
    fixture.enable(fixture.owner).await;
    let candidate = fixture
        .stage_as(fixture.owner, fixture.conversation, fixture.source)
        .await
        .unwrap();
    fixture
        .set_conversation_state("archived", Some(fixture.owner), Some(fixture.hat))
        .await;
    assert_denied(
        fixture
            .stage_as(fixture.owner, fixture.conversation, fixture.source)
            .await,
    );
    assert_denied(
        service::publish_preferred_name(
            &fixture.pool,
            &fixture.stores,
            fixture.bear,
            fixture.owner,
            candidate.id(),
            None,
        )
        .await,
    );
    let direct = stage(&fixture, fixture.owner, Uuid::new_v4(), "Owner request")
        .await
        .unwrap();
    assert!(matches!(
        direct.source(),
        CandidateSource::OwnerRequest { .. }
    ));
    fixture
        .set_conversation_state("active", Some(fixture.owner), Some(fixture.hat))
        .await;
    assert_denied(
        fixture
            .stage_as(fixture.owner, fixture.conversation, Uuid::new_v4())
            .await,
    );
    service::publish_preferred_name(
        &fixture.pool,
        &fixture.stores,
        fixture.bear,
        fixture.owner,
        candidate.id(),
        None,
    )
    .await
    .unwrap();
}
