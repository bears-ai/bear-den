use super::*;
use crate::{append_memory_record, test_support::new_test_store, LogicalMemoryPath, MemorySource};
use serde_json::json;

#[tokio::test]
async fn path_and_payload_claims_never_become_verified_proposal_columns() {
    let store = new_test_store().await;
    let hat_id = HatId::new(Uuid::new_v4());
    let source_id = Uuid::new_v4();
    let untrusted = create_memory_proposal(
        &store,
        "propose_hat",
        "normal",
        false,
        &json!({
            "source_memory_id": source_id,
            "target_hat_id": hat_id,
            "verified_hat_source": { "memory_id": source_id, "hat_id": hat_id },
            "source_paths": ["source_memory/conversation/guessed.md"],
            "summary": "Spoofed source information",
        }),
    )
    .await
    .unwrap();
    assert!(untrusted.verified_hat_source.is_none());
    assert!(get_memory_proposal(&store, &untrusted.proposal_id)
        .await
        .unwrap()
        .unwrap()
        .verified_hat_source
        .is_none());
    let columns: (Option<String>, Option<String>) = sqlx::query_as(
        "SELECT source_memory_id, target_hat_id FROM memory_proposals WHERE proposal_id = ?",
    )
    .bind(&untrusted.proposal_id)
    .fetch_one(store.pool())
    .await
    .unwrap();
    assert_eq!(columns, (None, None));
}

#[tokio::test]
async fn verified_proposals_preserve_typed_source_and_hat_and_never_copy_raw_note() {
    let store = new_test_store().await;
    let source = append_memory_record(
        &store,
        &LogicalMemoryPath::source_local(MemorySource::Conversation(Uuid::new_v4()), "note"),
        "note",
        "pair",
        None,
        "Private source must not become shared on proposal intake",
        &json!({}),
    )
    .await
    .unwrap();
    let memory_id = Uuid::parse_str(&source.memory_id).unwrap();
    let verified = VerifiedHatProposalSource {
        memory_id,
        hat_id: HatId::new(Uuid::new_v4()),
    };
    let payload = json!({"summary": "Distill a safe hat lesson", "proposed_content": "Candidate, not published"});
    let first = create_verified_hat_proposal(&store, "normal", false, &payload, verified)
        .await
        .unwrap();
    let second = create_verified_hat_proposal(&store, "normal", false, &payload, verified)
        .await
        .unwrap();
    assert_ne!(first.proposal_id, second.proposal_id);
    assert_eq!(first.verified_hat_source, Some(verified));
    assert_eq!(
        get_memory_proposal(&store, &first.proposal_id)
            .await
            .unwrap()
            .unwrap()
            .verified_hat_source,
        Some(verified)
    );
    assert_eq!(
        list_memory_proposals(&store, Some("pending"), 10)
            .await
            .unwrap()
            .into_iter()
            .filter(|row| row.verified_hat_source == Some(verified))
            .count(),
        2
    );
    assert!(
        resolve_memory_proposal(&store, &first.proposal_id, "deferred", &json!({}))
            .await
            .unwrap()
            .verified_hat_source
            == Some(verified)
    );
    let shared: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM memory_records WHERE scope_type = 'hat' OR scope_type = 'shared'",
    )
    .fetch_one(store.pool())
    .await
    .unwrap();
    assert_eq!(shared, 0);

    let other = new_test_store().await;
    assert!(
        create_verified_hat_proposal(&other, "normal", false, &payload, verified)
            .await
            .is_err()
    );
    let shared_note = append_memory_record(
        &store,
        &LogicalMemoryPath::shared_core("not-source"),
        "note",
        "curate",
        None,
        "Already shared",
        &json!({}),
    )
    .await
    .unwrap();
    assert!(create_verified_hat_proposal(
        &store,
        "normal",
        false,
        &payload,
        VerifiedHatProposalSource {
            memory_id: Uuid::parse_str(&shared_note.memory_id).unwrap(),
            ..verified
        }
    )
    .await
    .is_err());
}

#[tokio::test]
async fn corrupt_partial_verified_link_fails_closed_on_read() {
    let store = new_test_store().await;
    let proposal = create_memory_proposal(
        &store,
        "unspecified",
        "normal",
        false,
        &json!({"summary": "Source without verified hat"}),
    )
    .await
    .unwrap();
    sqlx::query("UPDATE memory_proposals SET source_memory_id = ? WHERE proposal_id = ?")
        .bind(Uuid::new_v4().to_string())
        .bind(&proposal.proposal_id)
        .execute(store.pool())
        .await
        .unwrap();
    assert!(
        get_memory_proposal(&store, &proposal.proposal_id)
            .await
            .unwrap()
            .unwrap()
            .verified_hat_source
            .is_none(),
        "older source-only proposals are not verified hat proposals"
    );
    sqlx::query("UPDATE memory_proposals SET source_memory_id = NULL, target_hat_id = ? WHERE proposal_id = ?")
        .bind(HatId::new(Uuid::new_v4()).to_string())
        .bind(&proposal.proposal_id)
        .execute(store.pool()).await.unwrap();
    assert!(get_memory_proposal(&store, &proposal.proposal_id)
        .await
        .is_err());
}
