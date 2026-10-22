use super::support::{assert_denied, Fixture};
use crate::{bears::db, personal_memory as service};
use den_core::{ids::UserId, DenError};
use den_memory::personal_candidates::{self, CandidateState};
use service::{
    CandidateReviewDecision, CandidateReviewId, CandidateReviewReceipt, PersonalCandidate,
    PersonalCandidateId, PersonalFactState,
};
use sqlx::PgPool;
use uuid::Uuid;

use CandidateReviewDecision::{Defer, Expire, Reject, Resume};

impl Fixture {
    async fn stage_for_review(&self, name: &str) -> PersonalCandidateId {
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

    async fn review_as(
        &self,
        user: UserId,
        candidate: PersonalCandidateId,
        decision: CandidateReviewDecision,
        expected: Option<CandidateReviewId>,
    ) -> Result<CandidateReviewReceipt, DenError> {
        service::review_candidate(
            &self.pool,
            &self.stores,
            self.bear,
            user,
            candidate,
            decision,
            expected,
        )
        .await
    }

    async fn reviewed_candidate(&self, candidate: PersonalCandidateId) -> PersonalCandidate {
        personal_candidates::get_candidate(
            &self.store().await,
            &self.binding(self.owner).await,
            candidate,
        )
        .await
        .unwrap()
        .unwrap()
    }
}

#[sqlx::test(migrations = "../../migrations")]
async fn review_requires_current_owner_membership_not_admin_authority(pool: PgPool) {
    let fixture = Fixture::new(pool).await;
    fixture.enable(fixture.owner).await;
    fixture.enable(fixture.other).await;
    let candidate = fixture.stage_for_review("Alice").await;
    for user in [fixture.other, fixture.outsider, UserId::new(0)] {
        for id in [candidate, PersonalCandidateId(Uuid::new_v4())] {
            for decision in [Reject, Expire, Defer, Resume] {
                assert_denied(fixture.review_as(user, id, decision, None).await);
            }
        }
    }
    assert_denied(
        fixture
            .review_as(
                fixture.owner,
                PersonalCandidateId(Uuid::new_v4()),
                Reject,
                None,
            )
            .await,
    );
    let receipt = fixture
        .review_as(fixture.owner, candidate, Reject, None)
        .await
        .unwrap();
    assert_denied(
        fixture
            .review_as(fixture.other, candidate, Reject, None)
            .await,
    );
    let pending = fixture.stage_for_review("Alicia").await;
    db::revoke_membership(&fixture.pool, fixture.owner.get(), fixture.bear.as_uuid())
        .await
        .unwrap();
    assert_denied(
        fixture
            .review_as(fixture.owner, candidate, Reject, None)
            .await,
    );
    for decision in [Reject, Expire, Defer, Resume] {
        assert_denied(
            fixture
                .review_as(fixture.owner, pending, decision, None)
                .await,
        );
    }
    db::grant_membership(
        &fixture.pool,
        fixture.owner.get(),
        fixture.bear.as_uuid(),
        None,
    )
    .await
    .unwrap();
    assert_eq!(
        fixture
            .review_as(fixture.owner, candidate, Reject, None)
            .await
            .unwrap(),
        receipt,
    );
    assert_eq!(
        fixture.reviewed_candidate(pending).await.latest_review_id(),
        None,
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn review_closure_is_source_independent_but_defer_resume_require_current_source_rights(
    pool: PgPool,
) {
    let fixture = Fixture::new(pool).await;
    fixture.enable(fixture.owner).await;
    let pending = fixture.stage_for_review("Alice").await;
    let deferred = fixture.stage_for_review("Alicia").await;
    let receipt = fixture
        .review_as(fixture.owner, deferred, Defer, None)
        .await
        .unwrap();
    for (status, owner, hat) in [
        ("active", Some(fixture.other), Some(fixture.hat)),
        ("active", Some(fixture.owner), None),
        ("archived", Some(fixture.owner), Some(fixture.hat)),
    ] {
        fixture.set_conversation_state(status, owner, hat).await;
        assert_denied(fixture.review_as(fixture.owner, pending, Defer, None).await);
        assert_denied(
            fixture
                .review_as(fixture.owner, deferred, Resume, Some(receipt.review_id))
                .await,
        );
        // The service conservatively checks conversation rights even for Defer replay.
        assert_denied(
            fixture
                .review_as(fixture.owner, deferred, Defer, None)
                .await,
        );
    }
    assert_eq!(
        fixture.reviewed_candidate(pending).await.latest_review_id(),
        None,
    );
    assert_eq!(
        fixture
            .reviewed_candidate(deferred)
            .await
            .latest_review_id(),
        Some(receipt.review_id),
    );
    for (candidate, decision, expected, state) in [
        (pending, Reject, None, CandidateState::Rejected),
        (
            deferred,
            Expire,
            Some(receipt.review_id),
            CandidateState::Expired,
        ),
    ] {
        let closed = fixture
            .review_as(fixture.owner, candidate, decision, expected)
            .await
            .unwrap();
        assert_eq!(closed.candidate_id, candidate);
        assert_eq!(closed.decision, decision);
        let current = fixture.reviewed_candidate(candidate).await;
        assert_eq!(current.state(), state);
        assert_eq!(current.latest_review_id(), Some(closed.review_id));
        assert_eq!(
            fixture
                .review_as(fixture.owner, candidate, decision, None)
                .await
                .unwrap(),
            closed,
        );
    }
}

#[sqlx::test(migrations = "../../migrations")]
async fn review_cas_and_latest_decision_replay_do_not_resurrect_opted_out_candidates(pool: PgPool) {
    let fixture = Fixture::new(pool).await;
    fixture.enable(fixture.owner).await;
    let candidate = fixture.stage_for_review("Alice").await;
    let mut expected = None;
    let mut first = None;
    for (decision, state) in [
        (Defer, CandidateState::Deferred),
        (Resume, CandidateState::Pending),
        (Defer, CandidateState::Deferred),
    ] {
        assert_denied(
            fixture
                .review_as(
                    fixture.owner,
                    candidate,
                    decision,
                    Some(CandidateReviewId(Uuid::new_v4())),
                )
                .await,
        );
        let receipt = fixture
            .review_as(fixture.owner, candidate, decision, expected)
            .await
            .unwrap();
        assert_eq!(receipt.candidate_id, candidate);
        assert_eq!(receipt.decision, decision);
        first.get_or_insert(receipt.review_id);
        expected = Some(receipt.review_id);
        let current = fixture.reviewed_candidate(candidate).await;
        assert_eq!(current.state(), state);
        assert_eq!(current.latest_review_id(), expected);
        assert_eq!(
            fixture
                .review_as(fixture.owner, candidate, decision, None)
                .await
                .unwrap(),
            receipt,
        );
    }
    assert_ne!(expected, first);
    assert_denied(
        fixture
            .review_as(fixture.owner, candidate, Resume, first)
            .await,
    );
    let latest = fixture
        .review_as(fixture.owner, candidate, Defer, None)
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
    assert_eq!(
        fixture
            .review_as(fixture.owner, candidate, Defer, None)
            .await
            .unwrap(),
        latest,
    );
    assert_denied(
        fixture
            .review_as(fixture.owner, candidate, Resume, expected)
            .await,
    );
    fixture.enable(fixture.owner).await;
    assert_denied(
        fixture
            .review_as(fixture.owner, candidate, Resume, expected)
            .await,
    );
    let current = fixture.reviewed_candidate(candidate).await;
    assert_eq!(current.state(), CandidateState::Withdrawn);
    assert_eq!(current.latest_review_id(), expected);
}

#[sqlx::test(migrations = "../../migrations")]
async fn explicit_publication_accepts_deferred_candidates_without_rewriting_review(pool: PgPool) {
    let fixture = Fixture::new(pool).await;
    fixture.enable(fixture.owner).await;
    let candidate = fixture.stage_for_review("Alice").await;
    let review = fixture
        .review_as(fixture.owner, candidate, Defer, None)
        .await
        .unwrap();
    assert_eq!(
        fixture.reviewed_candidate(candidate).await.state(),
        CandidateState::Deferred,
    );
    assert!(service::current_preferred_name(
        &fixture.pool,
        &fixture.stores,
        fixture.bear,
        fixture.owner,
    )
    .await
    .unwrap()
    .is_none());
    let publication = service::publish_preferred_name(
        &fixture.pool,
        &fixture.stores,
        fixture.bear,
        fixture.owner,
        candidate,
        None,
    )
    .await
    .unwrap();
    assert_eq!(publication.candidate_id, candidate);
    assert_eq!(
        fixture
            .review_as(fixture.owner, candidate, Defer, None)
            .await
            .unwrap(),
        review,
    );
    for decision in [Reject, Expire, Resume] {
        assert_denied(
            fixture
                .review_as(fixture.owner, candidate, decision, Some(review.review_id))
                .await,
        );
    }
    let current = fixture.reviewed_candidate(candidate).await;
    assert_eq!(current.state(), CandidateState::Published);
    assert_eq!(current.latest_review_id(), Some(review.review_id));
    let head = service::current_preferred_name(
        &fixture.pool,
        &fixture.stores,
        fixture.bear,
        fixture.owner,
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(head.fact_id, publication.fact_id);
    assert_eq!(head.state, PersonalFactState::Active);
    assert_eq!(head.preferred_name.unwrap().as_str(), "Alice");
}
