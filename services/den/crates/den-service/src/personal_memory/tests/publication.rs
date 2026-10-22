use super::support::{assert_denied, Fixture};
use crate::{bears::db, personal_memory as service};
use den_core::{ids::UserId, DenError};
use den_memory::personal_candidates::{self, CandidateState};
use service::{
    PersonalCandidateId, PersonalFactHead, PersonalFactId, PersonalFactState, PublicationReceipt,
};
use sqlx::PgPool;
use uuid::Uuid;

impl Fixture {
    async fn stage_name(&self, name: &str) -> PersonalCandidateId {
        service::stage_preferred_name(
            &self.pool,
            &self.stores,
            self.bear,
            self.owner,
            self.conversation,
            self.source,
            service::PreferredName::new(name).unwrap(),
        )
        .await
        .unwrap()
        .id()
    }

    async fn approve_as(
        &self,
        user: UserId,
        candidate: PersonalCandidateId,
        expected: Option<PersonalFactId>,
    ) -> Result<PublicationReceipt, DenError> {
        service::publish_preferred_name(
            &self.pool,
            &self.stores,
            self.bear,
            user,
            candidate,
            expected,
        )
        .await
    }

    async fn current_as(&self, user: UserId) -> Result<Option<PersonalFactHead>, DenError> {
        service::current_preferred_name(&self.pool, &self.stores, self.bear, user).await
    }

    async fn retract_as(&self, user: UserId, expected: PersonalFactId) -> Result<bool, DenError> {
        service::retract_preferred_name(&self.pool, &self.stores, self.bear, user, expected).await
    }

    async fn capture(&self, enabled: bool) {
        service::set_capture_enabled(&self.pool, &self.stores, self.bear, self.owner, enabled)
            .await
            .unwrap();
    }
}

#[sqlx::test(migrations = "../../migrations")]
async fn only_the_owner_can_explicitly_approve_and_read_or_retract_the_fact(pool: PgPool) {
    let fixture = Fixture::new(pool).await;
    fixture.capture(true).await;
    fixture.enable(fixture.other).await;
    let candidate = fixture.stage_name("Alice").await;
    assert!(fixture.current_as(fixture.owner).await.unwrap().is_none());
    // Bear-admin membership is not ownership; unknown and other-owner IDs are indistinguishable.
    for id in [candidate, PersonalCandidateId(Uuid::new_v4())] {
        assert_denied(fixture.approve_as(fixture.other, id, None).await);
    }
    assert_denied(fixture.approve_as(fixture.outsider, candidate, None).await);
    let receipt = fixture
        .approve_as(fixture.owner, candidate, None)
        .await
        .unwrap();
    assert_eq!(receipt.candidate_id, candidate);
    assert!(fixture.current_as(fixture.other).await.unwrap().is_none());
    assert_denied(fixture.retract_as(fixture.other, receipt.fact_id).await);
    assert_denied(fixture.approve_as(fixture.other, candidate, None).await);
    let head = fixture.current_as(fixture.owner).await.unwrap().unwrap();
    assert_eq!(head.state, PersonalFactState::Active);
    assert_eq!(head.preferred_name.unwrap().as_str(), "Alice");
    let stored = personal_candidates::get_candidate(
        &fixture.store().await,
        &fixture.binding(fixture.owner).await,
        candidate,
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(stored.state(), CandidateState::Published);
}

#[sqlx::test(migrations = "../../migrations")]
async fn new_approval_requires_current_source_rights_but_receipt_read_and_retraction_do_not(
    pool: PgPool,
) {
    let fixture = Fixture::new(pool).await;
    fixture.capture(true).await;
    let original = fixture.stage_name("Alice").await;
    let receipt = fixture
        .approve_as(fixture.owner, original, None)
        .await
        .unwrap();
    let correction = fixture.stage_name("Alicia").await;
    for (status, owner, hat) in [
        ("active", Some(fixture.other), Some(fixture.hat)),
        ("active", Some(fixture.owner), None),
        ("archived", Some(fixture.owner), Some(fixture.hat)),
    ] {
        fixture.set_conversation_state(status, owner, hat).await;
        assert_denied(
            fixture
                .approve_as(fixture.owner, correction, Some(receipt.fact_id))
                .await,
        );
    }
    let head = fixture.current_as(fixture.owner).await.unwrap().unwrap();
    assert_eq!(head.fact_id, receipt.fact_id);
    assert_eq!(head.state, PersonalFactState::Active);
    assert_eq!(head.preferred_name.unwrap().as_str(), "Alice");
    assert!(fixture
        .retract_as(fixture.owner, receipt.fact_id)
        .await
        .unwrap());
    // A replay bypasses even a now-invalid source and ignores the original head CAS.
    assert_eq!(
        fixture
            .approve_as(
                fixture.owner,
                original,
                Some(PersonalFactId(Uuid::new_v4()))
            )
            .await
            .unwrap(),
        receipt,
    );
    let head = fixture.current_as(fixture.owner).await.unwrap().unwrap();
    assert_eq!(head.state, PersonalFactState::Retracted);
    assert!(head.preferred_name.is_none());
}

#[sqlx::test(migrations = "../../migrations")]
async fn correction_is_head_cas_and_replay_or_reenable_never_resurrects_a_fact(pool: PgPool) {
    let fixture = Fixture::new(pool).await;
    fixture.capture(true).await;
    let first = fixture.stage_name("Alice").await;
    assert_denied(
        fixture
            .approve_as(fixture.owner, first, Some(PersonalFactId(Uuid::new_v4())))
            .await,
    );
    let original = fixture
        .approve_as(fixture.owner, first, None)
        .await
        .unwrap();
    assert_eq!(original.previous_fact_id, None);
    let second = fixture.stage_name("Alicia").await;
    assert_denied(fixture.approve_as(fixture.owner, second, None).await);
    let corrected = fixture
        .approve_as(fixture.owner, second, Some(original.fact_id))
        .await
        .unwrap();
    assert_eq!(corrected.previous_fact_id, Some(original.fact_id));
    assert_ne!(corrected.fact_id, original.fact_id);
    assert_eq!(
        fixture
            .approve_as(fixture.owner, first, None)
            .await
            .unwrap(),
        original
    );
    assert_denied(fixture.retract_as(fixture.owner, original.fact_id).await);
    let head = fixture.current_as(fixture.owner).await.unwrap().unwrap();
    assert_eq!(head.fact_id, corrected.fact_id);
    assert_eq!(head.preferred_name.unwrap().as_str(), "Alicia");

    let pending = fixture.stage_name("Ally").await;
    fixture.capture(false).await;
    let head = fixture.current_as(fixture.owner).await.unwrap().unwrap();
    assert_eq!(head.fact_id, corrected.fact_id);
    assert_eq!(head.state, PersonalFactState::Retracted);
    assert!(head.preferred_name.is_none());
    assert!(!fixture
        .retract_as(fixture.owner, corrected.fact_id)
        .await
        .unwrap());
    assert_eq!(
        fixture
            .approve_as(fixture.owner, second, None)
            .await
            .unwrap(),
        corrected
    );
    assert_denied(
        fixture
            .approve_as(fixture.owner, pending, Some(corrected.fact_id))
            .await,
    );
    fixture.capture(true).await;
    let withdrawn = personal_candidates::get_candidate(
        &fixture.store().await,
        &fixture.binding(fixture.owner).await,
        pending,
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(withdrawn.state(), CandidateState::Withdrawn);
    assert_denied(
        fixture
            .approve_as(fixture.owner, pending, Some(corrected.fact_id))
            .await,
    );
    assert_eq!(
        fixture
            .current_as(fixture.owner)
            .await
            .unwrap()
            .unwrap()
            .state,
        PersonalFactState::Retracted
    );
    let fresh = fixture.stage_name("Ally").await;
    assert_ne!(fresh, pending);
    assert_denied(fixture.approve_as(fixture.owner, fresh, None).await);
    let replacement = fixture
        .approve_as(fixture.owner, fresh, Some(corrected.fact_id))
        .await
        .unwrap();
    assert_eq!(replacement.previous_fact_id, Some(corrected.fact_id));
}

#[sqlx::test(migrations = "../../migrations")]
async fn revoked_membership_denies_new_approval_replay_read_and_retraction(pool: PgPool) {
    let fixture = Fixture::new(pool).await;
    fixture.capture(true).await;
    let candidate = fixture.stage_name("Alice").await;
    let receipt = fixture
        .approve_as(fixture.owner, candidate, None)
        .await
        .unwrap();
    let pending = fixture.stage_name("Alicia").await;
    db::revoke_membership(&fixture.pool, fixture.owner.get(), fixture.bear.as_uuid())
        .await
        .unwrap();
    assert_denied(fixture.approve_as(fixture.owner, candidate, None).await);
    assert_denied(
        fixture
            .approve_as(fixture.owner, pending, Some(receipt.fact_id))
            .await,
    );
    assert_denied(fixture.current_as(fixture.owner).await);
    assert_denied(fixture.retract_as(fixture.owner, receipt.fact_id).await);
    db::grant_membership(
        &fixture.pool,
        fixture.owner.get(),
        fixture.bear.as_uuid(),
        None,
    )
    .await
    .unwrap();
    let head = fixture.current_as(fixture.owner).await.unwrap().unwrap();
    assert_eq!(head.fact_id, receipt.fact_id);
    assert_eq!(head.state, PersonalFactState::Active);
    fixture
        .approve_as(fixture.owner, pending, Some(receipt.fact_id))
        .await
        .unwrap();
}

#[sqlx::test(migrations = "../../migrations")]
async fn imported_fact_needs_authorized_reconnection_but_not_capture_consent(pool: PgPool) {
    use den_memory::{
        identity_reconnection::{commit_review, review_reconnection, ReplacementConsent},
        quarantine_imported_authenticated_users,
    };

    let fixture = Fixture::new(pool).await;
    fixture.capture(true).await;
    let candidate = fixture.stage_name("Alice").await;
    let receipt = fixture
        .approve_as(fixture.owner, candidate, None)
        .await
        .unwrap();
    let store = fixture.store().await;
    let original_binding = fixture.binding(fixture.owner).await;
    quarantine_imported_authenticated_users(&store)
        .await
        .unwrap();
    assert!(fixture.current_as(fixture.owner).await.unwrap().is_none());
    assert_denied(fixture.approve_as(fixture.owner, candidate, None).await);
    assert_denied(fixture.retract_as(fixture.owner, receipt.fact_id).await);
    let review = review_reconnection(
        &store,
        fixture.other,
        original_binding.entity_id(),
        fixture.owner,
    )
    .await
    .unwrap();
    commit_review(
        &store,
        &review,
        ReplacementConsent::PreserveDisplacedIdentity,
    )
    .await
    .unwrap();
    let binding = fixture.binding(fixture.owner).await;
    assert!(!personal_candidates::capture_enabled(&store, &binding)
        .await
        .unwrap());
    let head = fixture.current_as(fixture.owner).await.unwrap().unwrap();
    assert_eq!(head.fact_id, receipt.fact_id);
    assert_eq!(head.state, PersonalFactState::Active);
    assert_eq!(head.preferred_name.unwrap().as_str(), "Alice");
    assert_eq!(
        fixture
            .approve_as(fixture.owner, candidate, None)
            .await
            .unwrap(),
        receipt
    );
    assert!(fixture
        .retract_as(fixture.owner, receipt.fact_id)
        .await
        .unwrap());
}
