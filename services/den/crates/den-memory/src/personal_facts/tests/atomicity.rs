use super::*;
use fixtures::{clear_failure, fail_late, Failure};

#[tokio::test]
async fn late_publication_failure_rolls_back_body_relations_old_head_candidate_and_sequence() {
    for correction in [false, true] {
        let store = new_test_store().await;
        let alice = opted_in(&store, ALICE).await;
        let source = note(&store).await;
        let first = if correction {
            Some(publish(&store, &alice, source, "Original", None).await)
        } else {
            None
        };
        let candidate = stage(&store, &alice, source, "Approved correction").await;
        fail_late(&store, Failure::Publication).await;
        let before = all_state(&store).await;
        database_error(
            publish_preferred_name(
                &store,
                &source.proof(&alice),
                candidate.id(),
                first.map(|r| r.fact_id),
            )
            .await,
        );
        assert_eq!(all_state(&store).await, before);
        assert_eq!(
            candidate_state(&store, &alice, candidate.id()).await,
            CandidateState::Pending
        );
        if let Some(first) = first {
            assert_head(
                &store,
                &alice,
                first,
                PersonalFactState::Active,
                Some("Original"),
            )
            .await;
        } else {
            assert!(current_preferred_name(&store, &alice)
                .await
                .unwrap()
                .is_none());
        }
        clear_failure(&store).await;
        let receipt = publish_preferred_name(
            &store,
            &source.proof(&alice),
            candidate.id(),
            first.map(|r| r.fact_id),
        )
        .await
        .unwrap();
        assert_head(
            &store,
            &alice,
            receipt,
            PersonalFactState::Active,
            Some("Approved correction"),
        )
        .await;
    }
}

#[tokio::test]
async fn late_retraction_failure_rolls_back_explicit_removal_and_optout_with_pending_withdrawal() {
    for optout in [false, true] {
        let store = new_test_store().await;
        let alice = opted_in(&store, ALICE).await;
        let source = note(&store).await;
        let fact = publish(&store, &alice, source, "Keep until committed", None).await;
        let pending = stage(&store, &alice, source, "Pending approval").await;
        fail_late(&store, Failure::Retraction).await;
        let before = all_state(&store).await;
        if optout {
            database_error(set_capture_enabled(&store, &alice, false).await);
        } else {
            database_error(retract_preferred_name(&store, &alice, fact.fact_id).await);
        }
        assert_eq!(all_state(&store).await, before);
        assert!(capture_enabled(&store, &alice).await.unwrap());
        assert_eq!(
            candidate_state(&store, &alice, pending.id()).await,
            CandidateState::Pending
        );
        assert_head(
            &store,
            &alice,
            fact,
            PersonalFactState::Active,
            Some("Keep until committed"),
        )
        .await;
        clear_failure(&store).await;
        if optout {
            set_capture_enabled(&store, &alice, false).await.unwrap();
        } else {
            assert!(retract_preferred_name(&store, &alice, fact.fact_id)
                .await
                .unwrap());
        }
        assert_head(&store, &alice, fact, PersonalFactState::Retracted, None).await;
    }
}
