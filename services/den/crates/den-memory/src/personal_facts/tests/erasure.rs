//! Owner erasure regressions deliberately reuse existing fixture SQL and snapshots.

use super::*;
use crate::{
    personal_candidates::{
        capture_setting, review_candidate, set_capture_enabled_if_current,
        stage_owner_preferred_name, CandidateReviewDecision, PersonalCaptureRevision,
        VerifiedOwnerSource,
    },
    personal_erasure::{
        erase_preferred_name_memory, mark_erasure_cleanup_complete, pending_erasure_cleanups,
        PersonalErasureId, PersonalErasureReceipt,
    },
    test_support::canonical_record_contents_match,
};

mod access;
mod atomicity;
mod authority;
mod bodies;
mod cleanup;
mod guards;
mod recovery;

#[derive(Clone, Copy)]
struct Reviewed {
    request: Uuid,
    head: Option<PersonalFactId>,
    latest: Option<PersonalCandidateId>,
    capture: PersonalCaptureRevision,
}

impl Reviewed {
    async fn read(store: &BearMemoryStore, owner: &AuthenticatedUserEntity) -> Self {
        Self {
            request: Uuid::new_v4(),
            head: current_preferred_name_version(store, owner)
                .await
                .unwrap()
                .map(|version| version.fact_id),
            latest: list_candidates(store, owner, 1)
                .await
                .unwrap()
                .first()
                .map(PersonalCandidate::id),
            capture: capture_setting(store, owner).await.unwrap().revision,
        }
    }

    async fn erase(
        self,
        store: &BearMemoryStore,
        owner: &AuthenticatedUserEntity,
    ) -> Result<PersonalErasureReceipt, DenError> {
        erase_preferred_name_memory(
            store,
            owner,
            self.request,
            self.head,
            self.latest,
            self.capture,
        )
        .await
    }
}

async fn owner_candidate(
    store: &BearMemoryStore,
    owner: &AuthenticatedUserEntity,
    name: &str,
) -> (PersonalCandidate, VerifiedPersonalSource) {
    let source = VerifiedOwnerSource::for_authorized_owner_request(owner, Uuid::new_v4());
    let candidate = stage_owner_preferred_name(store, &source, PreferredName::new(name).unwrap())
        .await
        .unwrap();
    (candidate, VerifiedPersonalSource::from(&source))
}

async fn assert_erased(
    store: &BearMemoryStore,
    owner: &AuthenticatedUserEntity,
    id: PersonalCandidateId,
) {
    let candidate = get_candidate(store, owner, id).await.unwrap().unwrap();
    assert_eq!(candidate.state(), CandidateState::Erased);
    assert_eq!(candidate.preferred_name(), None);
}
