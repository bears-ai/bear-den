use den_core::DenError;
use uuid::Uuid;

use super::{
    authority::{denied, write_transaction},
    read::{get_inside, withdraw_inside},
    settings::enabled,
    source::candidate_source_is_eligible,
    CandidateReviewDecision, CandidateReviewId, CandidateReviewReceipt, CandidateState,
    PersonalCandidateId,
};
use crate::{clock::now_rfc3339, AuthenticatedUserEntity, BearMemoryStore};

/// Trusted owner decision only. The service must lock membership and conversation-source
/// ownership through Defer/Resume; SQLite checks current consent, origin generation and, for
/// conversation evidence only, the exact live source note.
/// Reject/Expire close unpublished candidates without requiring their historical source.
///
/// A matching latest action returns its original immutable receipt, even after withdrawal,
/// quarantine or publication. That is historical replay, not active status or resurrection.
/// Every new decision requires an exact `expected_review` head (including `None` initially).
/// Protected candidate bodies/audit remain retained; Expire is neither fact TTL nor erasure.
pub async fn review_candidate(
    store: &BearMemoryStore,
    binding: &AuthenticatedUserEntity,
    id: PersonalCandidateId,
    decision: CandidateReviewDecision,
    expected_review: Option<CandidateReviewId>,
) -> Result<CandidateReviewReceipt, DenError> {
    let mut tx = write_transaction(store, binding).await?;
    let bear_id = store.bear_id().to_string();
    let person_entity_id = binding.entity_id();
    let candidate = get_inside(&mut tx, &bear_id, person_entity_id, id)
        .await?
        .ok_or_else(denied)?;
    if let Some(receipt) = candidate.latest_review {
        if receipt.decision == decision {
            tx.commit().await?;
            return Ok(receipt);
        }
    }
    if candidate.latest_review_id() != expected_review {
        return Err(denied());
    }
    match decision {
        CandidateReviewDecision::Reject | CandidateReviewDecision::Expire => {
            if !matches!(
                candidate.state(),
                CandidateState::Pending | CandidateState::Deferred | CandidateState::Quarantined
            ) {
                return Err(denied());
            }
        }
        CandidateReviewDecision::Defer | CandidateReviewDecision::Resume => {
            if !matches!(
                candidate.state(),
                CandidateState::Pending | CandidateState::Deferred
            ) || candidate.source_bear_id() != binding.bear_id()
                || candidate.source_user_id() != binding.user_id()
                || candidate.source_binding_generation() != binding.authority_generation
                || !enabled(
                    &mut tx,
                    &bear_id,
                    person_entity_id,
                    binding.authority_generation,
                )
                .await?
                || !candidate_source_is_eligible(&mut tx, &bear_id, candidate.source()).await?
            {
                return Err(denied());
            }
        }
    }
    let created_at = now_rfc3339()?;
    let actor = binding.user_id().get();
    if matches!(
        decision,
        CandidateReviewDecision::Reject | CandidateReviewDecision::Expire
    ) && !withdraw_inside(&mut tx, &bear_id, person_entity_id, id, actor, &created_at).await?
    {
        return Err(denied());
    }
    let sequence_no = sqlx::query_scalar!(
        r#"UPDATE bear_sequence SET next_sequence = next_sequence + 1 WHERE id = 1
               RETURNING next_sequence - 1 AS "sequence_no!: i64""#,
    )
    .fetch_one(&mut *tx)
    .await?;
    let review_id = CandidateReviewId(Uuid::new_v4());
    let review_id_text = review_id.to_string();
    let candidate_id = id.to_string();
    let decision_text = decision.as_str();
    // Insert last so a failed immutable receipt also rolls back withdrawal audit/sequence.
    // Recorded actor/host/generation are historical audit, never an authorization grant.
    sqlx::query!(
        r"
        INSERT INTO personal_memory_candidate_reviews (
            id, bear_id, person_entity_id, candidate_id, sequence_no, decision,
            recorded_on_bear_id, actor_user_id, authority_generation, created_at
        ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
        ",
        review_id_text,
        bear_id,
        person_entity_id,
        candidate_id,
        sequence_no,
        decision_text,
        bear_id,
        actor,
        binding.authority_generation,
        created_at,
    )
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(CandidateReviewReceipt {
        review_id,
        candidate_id: id,
        decision,
    })
}
