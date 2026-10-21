use std::sync::Arc;

use sqlx::sqlite::SqlitePoolOptions;
use tokio::sync::Barrier;

use super::*;

#[tokio::test]
async fn pending_dedup_uses_exact_normalized_name_conversation_and_note_without_allocating_again() {
    let store = new_test_store().await;
    let alice = opted_in(&store, ALICE).await;
    let source = note(&store).await;
    let first = stage(&store, &alice, source, " Alice ").await;
    let before = snapshot(&store).await;
    let duplicate = stage(&store, &alice, source, "Alice").await;
    assert_eq!(duplicate.id(), first.id());
    assert_eq!(duplicate.created_at(), first.created_at());
    assert_eq!(snapshot(&store).await, before);
    let case_distinct = stage(&store, &alice, source, "alice").await;
    let another_note = note_in(&store, source.conversation).await;
    let note_distinct = stage(&store, &alice, another_note, "Alice").await;
    let another_conversation = note(&store).await;
    let conversation_distinct = stage(&store, &alice, another_conversation, "Alice").await;
    let ids = [
        first.id(),
        case_distinct.id(),
        note_distinct.id(),
        conversation_distinct.id(),
    ];
    for (index, id) in ids.iter().enumerate() {
        assert!(!ids[..index].contains(id));
    }
    assert_eq!(list_candidates(&store, &alice, 100).await.unwrap().len(), 4);
}

#[tokio::test]
async fn dedup_is_person_and_bear_local_even_when_names_and_conversation_uuids_match() {
    let store = new_test_store().await;
    let alice = opted_in(&store, ALICE).await;
    let bob = opted_in(&store, BOB).await;
    let source = note(&store).await;
    let a = stage(&store, &alice, source, "Shared name").await;
    // At this boundary these proofs are trusted service input. This exercises the
    // person key, not a claim that Postgres authorizes Bob to Alice's conversation.
    let b = stage(&store, &bob, source, "Shared name").await;
    assert_ne!(a.id(), b.id());
    let other = BearMemoryStore::new(Uuid::new_v4(), store.pool().clone());
    let other_alice = opted_in(&other, ALICE).await;
    let other_note = note_in(&other, source.conversation).await;
    let c = stage(&other, &other_alice, other_note, "Shared name").await;
    assert_ne!(a.id(), c.id());
    assert_eq!(list_candidates(&store, &alice, 100).await.unwrap().len(), 1);
    assert_eq!(list_candidates(&store, &bob, 100).await.unwrap().len(), 1);
    assert_eq!(
        list_candidates(&other, &other_alice, 100)
            .await
            .unwrap()
            .len(),
        1
    );
    assert!(get_candidate(&other, &other_alice, a.id())
        .await
        .unwrap()
        .is_none());
    assert!(!withdraw_candidate(&other, &other_alice, a.id())
        .await
        .unwrap());
}

#[tokio::test]
async fn withdrawn_and_quarantined_history_never_deduplicates_or_resurrects() {
    let store = new_test_store().await;
    let alice = opted_in(&store, ALICE).await;
    let source = note(&store).await;
    let withdrawn = stage(&store, &alice, source, "Alice").await;
    withdraw_candidate(&store, &alice, withdrawn.id())
        .await
        .unwrap();
    let quarantined = stage(&store, &alice, source, "Alice").await;
    quarantine_candidate(&store, quarantined.id()).await;
    let sequence = next_sequence(&store).await;
    let pending = stage(&store, &alice, source, "Alice").await;
    assert_ne!(pending.id(), withdrawn.id());
    assert_ne!(pending.id(), quarantined.id());
    assert_ne!(withdrawn.id(), quarantined.id());
    assert_eq!(candidate_sequence(&store, pending.id()).await, sequence);
    assert_eq!(
        stage(&store, &alice, source, "Alice").await.id(),
        pending.id()
    );
    assert_eq!(
        state(&store, withdrawn.id()).await,
        CandidateState::Withdrawn
    );
    assert_eq!(
        state(&store, quarantined.id()).await,
        CandidateState::Quarantined
    );
    assert_eq!(list_candidates(&store, &alice, 100).await.unwrap().len(), 3);
}

#[tokio::test]
async fn partial_unique_index_rejects_duplicate_pending_rows_even_outside_the_api() {
    let store = new_test_store().await;
    let alice = opted_in(&store, ALICE).await;
    let source = note(&store).await;
    let pending = stage(&store, &alice, source, "Alice").await;
    let original = pending.id().to_string();
    let duplicate = Uuid::new_v4().to_string();
    let before = snapshot(&store).await;
    let error = sqlx::query!(
        r"INSERT INTO personal_memory_candidates (
            candidate_id, bear_id, person_entity_id, sequence_no, fact_type, value_text,
            evidence_kind, retention, source_conversation_id, source_memory_id,
            source_bear_id, source_user_id, source_binding_generation, state, created_at
        ) SELECT ?, bear_id, person_entity_id, sequence_no + 1, fact_type, value_text,
            evidence_kind, retention, source_conversation_id, source_memory_id,
            source_bear_id, source_user_id, source_binding_generation, state, created_at
          FROM personal_memory_candidates WHERE candidate_id = ?",
        duplicate,
        original,
    )
    .execute(store.pool())
    .await
    .unwrap_err();
    assert!(error.as_database_error().unwrap().is_unique_violation());
    assert_eq!(snapshot(&store).await, before);
}

async fn race(independent_pools: bool, distinct_values: bool) {
    let store = Arc::new(new_test_store().await);
    let alice = opted_in(&store, ALICE).await;
    let source = note(&store).await;
    let other_pool = if independent_pools {
        SqlitePoolOptions::new()
            .max_connections(4)
            .connect_with(store.pool().connect_options().as_ref().clone())
            .await
            .unwrap()
    } else {
        store.pool().clone()
    };
    let other = Arc::new(BearMemoryStore::new(store.bear_id(), other_pool));
    let before = next_sequence(&store).await;
    let contenders = 16;
    let barrier = Arc::new(Barrier::new(contenders));
    let mut tasks = Vec::new();
    for index in 0..contenders {
        let store = Arc::clone(if index % 2 == 0 { &store } else { &other });
        let barrier = Arc::clone(&barrier);
        let proof = source.proof(&alice);
        tasks.push(tokio::spawn(async move {
            let value = if distinct_values {
                format!("Name {index}")
            } else {
                "Alice".into()
            };
            barrier.wait().await;
            stage_preferred_name(&store, &proof, PreferredName::new(&value).unwrap())
                .await
                .unwrap()
                .id()
        }));
    }
    let mut ids = Vec::new();
    for task in tasks {
        ids.push(task.await.unwrap());
    }
    let expected = if distinct_values { contenders } else { 1 };
    let rows = list_candidates(&store, &alice, 100).await.unwrap();
    assert_eq!(rows.len(), expected);
    assert_eq!(next_sequence(&store).await, before + expected as i64);
    if distinct_values {
        let mut sequences = Vec::new();
        for id in ids {
            sequences.push(candidate_sequence(&store, id).await);
        }
        sequences.sort_unstable();
        assert_eq!(
            sequences,
            (before..before + contenders as i64).collect::<Vec<_>>()
        );
    } else {
        assert!(ids.iter().all(|id| *id == rows[0].id()));
        assert_eq!(candidate_sequence(&store, rows[0].id()).await, before);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn simultaneous_duplicates_commit_once_on_shared_and_independent_pools() {
    race(false, false).await;
    race(true, false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn independent_pool_insertions_allocate_distinct_gapless_sequences_transactionally() {
    race(true, true).await;
}
