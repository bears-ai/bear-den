use super::*;
use crate::{append_memory_record, test_support::new_test_store, LogicalMemoryPath};
use serde_json::json;

#[tokio::test]
async fn large_snapshot_fingerprints_full_history_while_retaining_one_page() {
    let store = new_test_store().await;
    let hat = HatId::new(Uuid::new_v4());
    let other = HatId::new(Uuid::new_v4());
    for index in 0..221 {
        append_memory_record(
            &store,
            &LogicalMemoryPath::hat(hat, &format!("note{index}")),
            "note",
            "curate",
            None,
            &format!("Historical note {index}"),
            &json!({}),
        )
        .await
        .unwrap();
    }
    append_memory_record(
        &store,
        &LogicalMemoryPath::hat(other, "not-selected"),
        "note",
        "curate",
        None,
        "Other hat",
        &json!({}),
    )
    .await
    .unwrap();
    let first = snapshot_for_hat(&store, hat, 1).await.unwrap();
    let second = snapshot_for_hat(&store, hat, 2).await.unwrap();
    let third = snapshot_for_hat(&store, hat, 3).await.unwrap();
    assert_eq!(first.total_records, 221);
    assert_eq!(hat_history_count(&store, hat).await.unwrap(), 221);
    assert_eq!(
        (
            first.records.len(),
            second.records.len(),
            third.records.len()
        ),
        (100, 100, 21)
    );
    assert_eq!(first.sha256, second.sha256);
    assert_eq!(first.sha256, third.sha256);
    assert_ne!(first.records[0].memory_id, second.records[0].memory_id);
    assert!(snapshot_for_hat(&store, hat, 0).await.is_err());
    assert!(snapshot_for_hat(&store, hat, 4).await.is_err());
    let full: Vec<HatReviewRecord> = sqlx::query_as(
        "SELECT memory_id, sequence_no, kind, content_text, metadata_json, visibility,
                invalid_at, created_at FROM memory_records
         WHERE bear_id = ? AND scope_type = 'hat' AND scope_hat_id = ?
         ORDER BY sequence_no DESC, memory_id DESC",
    )
    .bind(store.bear_id().to_string())
    .bind(hat.to_string())
    .fetch_all(store.pool())
    .await
    .unwrap();
    assert_eq!(
        first.sha256,
        format!("{:x}", Sha256::digest(serde_json::to_vec(&full).unwrap()))
    );
    sqlx::query("UPDATE memory_records SET metadata_json = ? WHERE memory_id = ?")
        .bind(json!({"lifecycle":{"status":"archived"}}).to_string())
        .bind(&third.records[0].memory_id)
        .execute(store.pool())
        .await
        .unwrap();
    let changed = snapshot_for_hat(&store, hat, 1).await.unwrap();
    assert_eq!(changed.total_records, first.total_records);
    assert_ne!(
        changed.sha256, first.sha256,
        "in-place history edits invalidate review"
    );
}
