use super::*;
use crate::tools::{
    sqlite_memory_browse, sqlite_memory_read, sqlite_memory_search, sqlite_memory_status,
};
use crate::{
    append_memory_record, append_relation, create_entity, get_entity,
    list_record_history_for_logical_path, list_relations_for_source, EntityTrust,
    LogicalMemoryPath, MemorySource, ResolutionState,
};
use den_core::ids::HatId;
use serde_json::json;

#[tokio::test]
async fn generic_reads_filter_access_history_before_limits_and_counts() {
    let config = temp_config();
    let manager = MemoryStoreManager::new(&config);
    let bear_id = Uuid::new_v4();
    let store = manager.store_for_bear(bear_id).await.unwrap();
    let open_path = LogicalMemoryPath::profile_local("pair", "open");
    let hidden_path = LogicalMemoryPath::profile_local("pair", "restricted");
    let open = append_memory_record(
        &store,
        &open_path,
        "note",
        "pair",
        None,
        "generic memory",
        &json!({}),
    )
    .await
    .unwrap();
    let hidden = append_memory_record(
        &store,
        &hidden_path,
        "note",
        "pair",
        None,
        "restricted memory",
        &json!({}),
    )
    .await
    .unwrap();
    let person = create_entity(
        &store,
        "person",
        Some("Ryan"),
        ResolutionState::Resolved,
        EntityTrust::Asserted,
        None,
        &json!({}),
    )
    .await
    .unwrap();
    let rule = append_relation(
        &store,
        &hidden.memory_id,
        &person.entity_id,
        "audience",
        &json!({}),
        "curate",
        None,
        None,
    )
    .await
    .unwrap();
    let bear_id_text = bear_id.to_string();
    for withdrawn in [false, true] {
        if withdrawn {
            sqlx::query!(
                "UPDATE memory_access_rules SET state = 'retracted' WHERE bear_id = ? AND link_id = ?",
                bear_id_text,
                rule.link_id,
            ).execute(store.pool()).await.unwrap();
        }
        assert!(
            get_memory_record_by_id(&manager, bear_id, &hidden.memory_id)
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            get_memory_record_detail(&manager, bear_id, &hidden.memory_id)
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            list_record_history_for_logical_path(&store, &hidden_path.to_logical_path(), 1)
                .await
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            list_all_logical_paths(&manager, bear_id).await.unwrap(),
            vec![open_path.to_logical_path()]
        );
        let summaries = list_path_summaries(&manager, bear_id).await.unwrap();
        assert_eq!(summaries.len(), 1);
        assert_eq!(summaries[0].head_memory_id, open.memory_id);
        assert_eq!(summaries[0].version_count, 1);
        assert_eq!(
            list_recent_memory_records(&manager, bear_id, 1)
                .await
                .unwrap()[0]
                .memory_id,
            open.memory_id
        );
        assert_eq!(
            search_memory_records(&manager, bear_id, "memory", 1)
                .await
                .unwrap()[0]
                .memory_id,
            open.memory_id
        );
        for buckets in [
            count_records_by_kind(&manager, bear_id).await.unwrap(),
            count_records_by_profile(&manager, bear_id).await.unwrap(),
        ] {
            assert_eq!(buckets.len(), 1);
            assert_eq!(buckets[0].count, 1);
        }
        assert_eq!(head_entry_count(&manager, bear_id).await.unwrap(), 1);
        let stats = bear_memory_admin_stats(&manager, &config, bear_id)
            .await
            .unwrap();
        assert_eq!(
            stats.record_count, 2,
            "legacy-import integrity count stays physical"
        );
        assert_eq!(stats.profile_local_count, 1);
        assert_eq!(stats.distinct_paths, 1);
        assert_eq!(
            sqlite_memory_browse(&store, "pair").await.unwrap()["children"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            sqlite_memory_read(&store, &hidden_path.to_logical_path())
                .await
                .unwrap()["ok"],
            false
        );
        assert_eq!(
            sqlite_memory_search(&store, "pair", "memory", 1)
                .await
                .unwrap()["hits"][0]["memory_id"],
            open.memory_id
        );
        assert_eq!(
            sqlite_memory_status(&store, "pair").await.unwrap()["file_count"],
            1
        );
        for (target, proposed) in [
            (hidden_path.to_logical_path(), "a different claim"),
            ("core/another.md".to_owned(), hidden.content_text.as_str()),
        ] {
            let proposal = crate::create_memory_proposal(
                &store,
                "promote_to_core",
                "normal",
                false,
                &json!({"target_ref": target, "proposed_content": proposed}),
            )
            .await
            .unwrap();
            assert!(proposal.payload_json.get("freshness").is_none());
            assert!(proposal.payload_json.get("consolidation_review").is_none());
        }
    }
}

fn temp_config() -> Config {
    let mut config = Config::load();
    let dir = std::env::temp_dir().join(format!("den-admin-inspect-{}", Uuid::new_v4()));
    std::fs::create_dir_all(&dir).expect("create temp data dir");
    config.bear_sqlite_data_dir = dir.to_string_lossy().into_owned();
    config
}

#[tokio::test]
async fn admin_helpers_cover_records_history_search_and_entity_links() {
    let config = temp_config();
    let manager = MemoryStoreManager::new(&config);
    let bear_id = Uuid::new_v4();
    let store = manager.store_for_bear(bear_id).await.expect("store");

    // Two versions at one path (history) + a second path of a different kind.
    let path = LogicalMemoryPath::profile_local("pair", "note");
    let v1 = append_memory_record(
        &store,
        &path,
        "note",
        "pair",
        None,
        "first draft about Ryan",
        &json!({}),
    )
    .await
    .expect("append v1");
    let v2 = append_memory_record(
        &store,
        &path,
        "note",
        "pair",
        None,
        "second draft about Ryan",
        &json!({}),
    )
    .await
    .expect("append v2");
    let other = LogicalMemoryPath::shared_core("bear-glossary");
    append_memory_record(
        &store,
        &other,
        "glossary",
        "curate",
        None,
        "glossary entry",
        &json!({}),
    )
    .await
    .expect("append other");

    // Detail fetch includes author/visibility.
    let detail = get_memory_record_detail(&manager, bear_id, &v2.memory_id)
        .await
        .expect("detail query")
        .expect("record exists");
    assert_eq!(detail.author_profile, "pair");
    assert_eq!(detail.visibility, "normal");

    // History: both versions at the path, newest first.
    let versions = list_record_history_for_logical_path(&store, &path.to_logical_path(), 50)
        .await
        .expect("versions");
    assert_eq!(versions.len(), 2);
    assert_eq!(versions[0].memory_id, v2.memory_id, "newest first");
    assert_eq!(versions[1].memory_id, v1.memory_id);

    // Grouped counts + head count.
    let by_kind = count_records_by_kind(&manager, bear_id)
        .await
        .expect("by kind");
    assert!(by_kind.iter().any(|b| b.label == "note" && b.count == 2));
    assert!(by_kind
        .iter()
        .any(|b| b.label == "glossary" && b.count == 1));
    let by_profile = count_records_by_profile(&manager, bear_id)
        .await
        .expect("by profile");
    assert!(by_profile.iter().any(|b| b.label == "pair" && b.count == 2));
    assert!(by_profile.iter().any(|b| b.label == "core" && b.count == 1));
    assert_eq!(head_entry_count(&manager, bear_id).await.expect("heads"), 2);

    append_memory_record(
        &store,
        &LogicalMemoryPath::hat(HatId::new(Uuid::new_v4()), "hat-note"),
        "note",
        "curate",
        None,
        "hat only",
        &json!({}),
    )
    .await
    .expect("append hat record");
    append_memory_record(
        &store,
        &LogicalMemoryPath::source_local(
            MemorySource::Conversation(Uuid::new_v4()),
            "private-note",
        ),
        "note",
        "pair",
        None,
        "session only",
        &json!({}),
    )
    .await
    .expect("append session record");
    let counts = count_records_by_profile(&manager, bear_id)
        .await
        .expect("scope counts");
    assert!(counts.iter().any(|b| b.label == "core" && b.count == 1));
    assert!(counts.iter().any(|b| b.label == "hat" && b.count == 1));
    assert!(counts.iter().any(|b| b.label == "session" && b.count == 1));

    // Path summaries: one head row per path, with the version count.
    let summaries = list_path_summaries(&manager, bear_id)
        .await
        .expect("summaries");
    let note_summary = summaries
        .iter()
        .find(|s| s.logical_path == path.to_logical_path())
        .expect("note path summary");
    assert_eq!(note_summary.version_count, 2);
    assert_eq!(note_summary.head_memory_id, v2.memory_id);

    // Keyword search across all roles.
    let hits = search_memory_records(&manager, bear_id, "Ryan", 50)
        .await
        .expect("search");
    assert_eq!(hits.len(), 2, "both 'Ryan' drafts match");
    assert!(
        search_memory_records(&manager, bear_id, "nonexistent-token", 50)
            .await
            .expect("search empty")
            .is_empty()
    );

    // Entity cross-link: a record references a resolved entity.
    let entity = create_entity(
        &store,
        "person",
        Some("Ryan"),
        ResolutionState::Resolved,
        EntityTrust::Asserted,
        None,
        &json!({}),
    )
    .await
    .expect("create entity");
    append_relation(
        &store,
        &v2.memory_id,
        &entity.entity_id,
        "subject",
        &json!({ "is_primary": true }),
        "pair",
        None,
        None,
    )
    .await
    .expect("append relation");

    let rels = list_relations_for_source(&store, &v2.memory_id, 10)
        .await
        .expect("relations for source");
    assert_eq!(rels.len(), 1);
    assert_eq!(rels[0].entity_id, entity.entity_id);
    let linked = get_entity(&store, &rels[0].entity_id)
        .await
        .expect("get entity")
        .expect("entity exists");
    assert_eq!(linked.display_name.as_deref(), Some("Ryan"));
}
