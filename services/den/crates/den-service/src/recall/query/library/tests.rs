use super::*;
use den_core::ids::HatId;
use den_memory::{append_memory_record, LogicalMemoryPath, MemorySource, MemoryStoreManager};

fn stale(id: &str) -> RecalledPassage {
    RecalledPassage {
        memory_id: id.to_string(),
        logical_path: Some("source_memory/forged.md".into()),
        kind: Some("forged".into()),
        score: 0.91,
        salience: "critical".into(),
        lifecycle_status: "active".into(),
        freshness_trend: "stable".into(),
        text: "LEAK from stale vector payload".into(),
        conflicts_with: vec![],
    }
}

#[test]
fn member_vector_filter_only_admits_curated_scopes_and_selected_bear_hats() {
    let bear = Uuid::new_v4();
    let hat = HatId::new(Uuid::new_v4());
    let filter = curated_scope_filter(bear, "test-standard", &CuratedMemoryGrant::new(vec![hat]));
    assert_eq!(filter["must"][0]["match"]["value"], bear.to_string());
    assert_eq!(filter["must"][3]["should"][0]["match"]["value"], "shared");
    assert_eq!(
        filter["must"][3]["should"][1]["must"][0]["match"]["value"],
        "hat"
    );
    assert_eq!(
        filter["must"][3]["should"][1]["must"][1]["match"]["any"][0],
        hat.to_string()
    );
    assert!(!filter.to_string().contains("profile_local"));
    assert!(!filter.to_string().contains("source_local"));
    let shared_only = curated_scope_filter(bear, "test-standard", &CuratedMemoryGrant::new(vec![]));
    assert_eq!(
        shared_only["must"][3]["should"].as_array().unwrap().len(),
        1
    );
}

#[tokio::test]
async fn derived_member_hits_are_rebuilt_from_current_canonical_curated_records() {
    let mut config = Config::test_stub();
    config.bear_sqlite_data_dir = std::env::temp_dir()
        .join(format!("member-recall-{}", Uuid::new_v4()))
        .to_string_lossy()
        .to_string();
    let stores = MemoryStoreManager::new(&config);
    let store = stores.store_for_bear(Uuid::new_v4()).await.unwrap();
    let hat = HatId::new(Uuid::new_v4());
    let other_hat = HatId::new(Uuid::new_v4());
    let shared = append_memory_record(
        &store,
        &LogicalMemoryPath::shared_core("fact"),
        "note",
        "curate",
        None,
        "Canonical Bear-wide fact",
        &json!({}),
    )
    .await
    .unwrap();
    let source = append_memory_record(
        &store,
        &LogicalMemoryPath::hat(hat, "fact"),
        "note",
        "curate",
        None,
        "Canonical selected-hat fact",
        &json!({}),
    )
    .await
    .unwrap();
    let foreign = append_memory_record(
        &store,
        &LogicalMemoryPath::hat(other_hat, "fact"),
        "note",
        "curate",
        None,
        "Other hat private to that responsibility",
        &json!({}),
    )
    .await
    .unwrap();
    let raw = append_memory_record(
        &store,
        &LogicalMemoryPath::source_local(MemorySource::Conversation(Uuid::new_v4()), "raw"),
        "note",
        "pair",
        None,
        "Another conversation raw note",
        &json!({}),
    )
    .await
    .unwrap();
    let legacy = append_memory_record(
        &store,
        &LogicalMemoryPath::profile_local("pair", "legacy"),
        "note",
        "pair",
        None,
        "Legacy role-local note",
        &json!({}),
    )
    .await
    .unwrap();
    let archived = append_memory_record(
        &store,
        &LogicalMemoryPath::hat(hat, "archived"),
        "note",
        "curate",
        None,
        "Archived note",
        &json!({"lifecycle": {"status": "archived"}}),
    )
    .await
    .unwrap();
    let superseded = append_memory_record(
        &store,
        &LogicalMemoryPath::hat(hat, "version"),
        "note",
        "curate",
        None,
        "Old version",
        &json!({}),
    )
    .await
    .unwrap();
    let current = append_memory_record(
        &store,
        &LogicalMemoryPath::hat(hat, "version"),
        "note",
        "curate",
        None,
        "Current version",
        &json!({}),
    )
    .await
    .unwrap();
    sqlx::query("UPDATE memory_records SET supersedes_memory_id = ? WHERE memory_id = ?")
        .bind(&superseded.memory_id)
        .bind(&current.memory_id)
        .execute(store.pool())
        .await
        .unwrap();
    // A no-hat grant still cannot turn legacy/raw records into shared
    // memory by supplying a forged core locator in the vector payload.
    let mut no_hat = RecallProjection {
        passages: [&raw, &legacy, &foreign, &source, &shared]
            .into_iter()
            .map(|row| stale(&row.memory_id))
            .collect(),
        diagnostic: json!({}),
    };
    for passage in &mut no_hat.passages {
        passage.logical_path = Some("core/forged.md".into());
    }
    retain_curated_candidates(&store, &CuratedMemoryGrant::new(vec![]), &mut no_hat, 10)
        .await
        .unwrap();
    assert_eq!(no_hat.passages.len(), 1);
    assert_eq!(no_hat.passages[0].memory_id, shared.memory_id);
    assert_eq!(no_hat.passages[0].text, shared.content_text);

    let grant = CuratedMemoryGrant::new(vec![hat]);
    let mut projection = RecallProjection {
        passages: [
            &raw,
            &legacy,
            &foreign,
            &archived,
            &superseded,
            &source,
            &shared,
            &current,
        ]
        .into_iter()
        .map(|row| stale(&row.memory_id))
        .collect(),
        diagnostic: json!({}),
    };
    retain_curated_candidates(&store, &grant, &mut projection, 10)
        .await
        .unwrap();
    assert_eq!(projection.passages.len(), 3);
    for passage in &projection.passages {
        assert!(!passage.text.contains("LEAK"));
        assert_ne!(
            passage.logical_path.as_deref(),
            Some("source_memory/forged.md")
        );
    }
    assert!(projection
        .passages
        .iter()
        .any(|passage| passage.memory_id == shared.memory_id
            && passage.text == "Canonical Bear-wide fact"));
    assert!(projection
        .passages
        .iter()
        .any(|passage| passage.memory_id == source.memory_id
            && passage.text == "Canonical selected-hat fact"));
    assert!(projection
        .passages
        .iter()
        .any(|passage| passage.memory_id == current.memory_id));
}
