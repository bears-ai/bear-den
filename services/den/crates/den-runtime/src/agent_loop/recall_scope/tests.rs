use den_core::{config::Config, ids::HatId};
use den_memory::{append_memory_record, LogicalMemoryPath, MemorySource, MemoryStoreManager};
use den_service::recall::RecalledPassage;
use serde_json::json;
use uuid::Uuid;

use super::*;

fn passage(id: &str, path: Option<String>) -> RecalledPassage {
    RecalledPassage {
        memory_id: id.to_string(),
        logical_path: path,
        kind: Some("note".into()),
        score: 0.9,
        salience: "normal".into(),
        lifecycle_status: "active".into(),
        freshness_trend: "stable".into(),
        text: "stale index text".into(),
        conflicts_with: vec![],
    }
}

#[tokio::test]
async fn stale_or_other_source_passages_are_removed_before_prompt_render() {
    let mut config = Config::test_stub();
    config.bear_sqlite_data_dir = format!("/tmp/bears-recall-scope-{}", Uuid::new_v4());
    let stores = MemoryStoreManager::new(&config);
    let store = stores.store_for_bear(Uuid::new_v4()).await.unwrap();
    let owner = MemorySource::Conversation(Uuid::new_v4());
    let another = MemorySource::Conversation(Uuid::new_v4());
    let hat = HatId::new(Uuid::new_v4());
    let owner_record = append_memory_record(
        &store,
        &LogicalMemoryPath::source_local(owner, "note"),
        "note",
        "pair",
        None,
        "owner data",
        &json!({}),
    )
    .await
    .unwrap();
    let another_record = append_memory_record(
        &store,
        &LogicalMemoryPath::source_local(another, "note"),
        "note",
        "pair",
        None,
        "another session data",
        &json!({}),
    )
    .await
    .unwrap();
    let hat_record = append_memory_record(
        &store,
        &LogicalMemoryPath::hat(hat, "note"),
        "note",
        "curate",
        None,
        "hat data",
        &json!({}),
    )
    .await
    .unwrap();
    let legacy = append_memory_record(
        &store,
        &LogicalMemoryPath::profile_local("pair", "note"),
        "note",
        "pair",
        None,
        "legacy data",
        &json!({}),
    )
    .await
    .unwrap();
    let owner_id = owner_record.memory_id.clone();
    let hat_id = hat_record.memory_id.clone();
    let mut projection = RecallProjection {
        passages: [owner_record, another_record, hat_record, legacy]
            .into_iter()
            .map(|record| passage(&record.memory_id, record.logical_path))
            .collect(),
        diagnostic: json!({}),
    };
    retain_canonical_passages(
        &store,
        MemoryReadGrant::new(owner, Some(hat)),
        &mut projection,
    )
    .await
    .unwrap();
    assert_eq!(projection.passages.len(), 2);
    assert!(projection.passages.iter().any(|p| p.memory_id == owner_id));
    assert!(projection.passages.iter().any(|p| p.memory_id == hat_id));

    den_memory::mark_memory_record_lifecycle(&store, &hat_id, "archived", None)
        .await
        .unwrap();
    let mut stale = RecallProjection {
        passages: vec![passage(
            &hat_id,
            Some(LogicalMemoryPath::hat(hat, "note").to_logical_path()),
        )],
        diagnostic: json!({}),
    };
    retain_canonical_passages(&store, MemoryReadGrant::new(owner, Some(hat)), &mut stale)
        .await
        .unwrap();
    assert!(stale.passages.is_empty());
}
