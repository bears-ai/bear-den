use super::*;
use crate::{
    append_memory_record,
    library::{self, CuratedMemoryGrant},
    test_support::new_test_store,
    LogicalMemoryPath, MemoryScopeType,
};
use serde_json::json;

fn review(hat_id: HatId, source_memory_id: Uuid, expected_head: Option<Uuid>) -> ReviewedCoreEntry {
    ReviewedCoreEntry {
        hat_id,
        source_memory_id,
        kind: "finding".into(),
        reviewed_content: "A safe, newly written Bear-wide finding.".into(),
        expected_head,
        review_notes: "Reviewed and omitted private information for all Bear members and Work."
            .into(),
        reviewer: UserId::new(42),
    }
}

#[tokio::test]
async fn reviewed_hat_to_core_is_atomic_and_does_not_widen_other_raw_sources() {
    let store = new_test_store().await;
    let hat = HatId::new(Uuid::new_v4());
    let other_hat = HatId::new(Uuid::new_v4());
    let source = append_memory_record(
        &store,
        &LogicalMemoryPath::hat(hat, "finding"),
        "finding",
        "curate",
        None,
        "Private reviewed hat context — not safe to copy verbatim.",
        &json!({}),
    )
    .await
    .unwrap();
    let source_id = Uuid::parse_str(&source.memory_id).unwrap();
    assert!(candidate(&store, other_hat, source_id).await.is_err());
    assert_eq!(candidates(&store, hat, 50).await.unwrap().len(), 1);
    let old = append_memory_record(
        &store,
        &LogicalMemoryPath::shared_core("finding"),
        "finding",
        "curate",
        None,
        "Previous shared finding",
        &json!({}),
    )
    .await
    .unwrap();
    assert!(
        promote(&store, review(hat, source_id, None)).await.is_err(),
        "an old core head must not be silently replaced"
    );
    let previous_id = Uuid::parse_str(&old.memory_id).unwrap();
    let outcome = promote(&store, review(hat, source_id, Some(previous_id)))
        .await
        .unwrap();
    let shared = library::detail(
        &store,
        &CuratedMemoryGrant::new(vec![other_hat]),
        &outcome.memory_id.to_string(),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(shared.scope_type, MemoryScopeType::Shared.as_str());
    assert_eq!(
        shared.content_text,
        "A safe, newly written Bear-wide finding."
    );
    assert_eq!(shared.metadata_json["promoted_from"], source.memory_id);
    assert_eq!(shared.metadata_json["source_hat_id"], hat.to_string());
    assert_eq!(shared.metadata_json["reviewed_for_bear_and_work"], true);
    assert!(library::detail(
        &store,
        &CuratedMemoryGrant::new(vec![other_hat]),
        &source.memory_id
    )
    .await
    .unwrap()
    .is_none());
    assert!(library::detail(
        &store,
        &CuratedMemoryGrant::new(vec![other_hat]),
        &old.memory_id
    )
    .await
    .unwrap()
    .is_none());
    let promotion: (String, String) = sqlx::query_as(
        "SELECT source_memory_id, target_memory_id FROM memory_promotions WHERE promotion_id = ?",
    )
    .bind(outcome.promotion_id.to_string())
    .fetch_one(store.pool())
    .await
    .unwrap();
    assert_eq!(promotion.0, source.memory_id);
    assert_eq!(promotion.1, outcome.memory_id.to_string());
    assert!(
        promote(&store, review(hat, source_id, Some(outcome.memory_id)))
            .await
            .is_err(),
        "a prior source-to-core promotion cannot be replayed"
    );
}

#[tokio::test]
async fn inaccessible_or_non_hat_sources_cannot_be_reviewed_into_core() {
    let store = new_test_store().await;
    let hat = HatId::new(Uuid::new_v4());
    let other_hat = HatId::new(Uuid::new_v4());
    let raw = append_memory_record(
        &store,
        &LogicalMemoryPath::source_local(
            crate::MemorySource::Conversation(Uuid::new_v4()),
            "secret",
        ),
        "secret",
        "pair",
        None,
        "private source text",
        &json!({}),
    )
    .await
    .unwrap();
    let archived = append_memory_record(
        &store,
        &LogicalMemoryPath::hat(hat, "archived"),
        "archived",
        "curate",
        None,
        "old hat material",
        &json!({"lifecycle": {"status": "archived"}}),
    )
    .await
    .unwrap();
    let foreign = append_memory_record(
        &store,
        &LogicalMemoryPath::hat(other_hat, "foreign"),
        "foreign",
        "curate",
        None,
        "other hat text",
        &json!({}),
    )
    .await
    .unwrap();
    for id in [&raw.memory_id, &archived.memory_id, &foreign.memory_id] {
        assert!(candidate(&store, hat, Uuid::parse_str(id).unwrap())
            .await
            .is_err());
        assert!(
            promote(&store, review(hat, Uuid::parse_str(id).unwrap(), None))
                .await
                .is_err()
        );
    }
    assert!(candidates(&store, hat, 50).await.unwrap().is_empty());
}
