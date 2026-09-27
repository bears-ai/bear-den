use den_core::BearProfile;
use serde_json::json;
use uuid::Uuid;

use super::*;
use crate::{append_memory_record, test_support::new_test_store, MemorySource};

#[tokio::test]
async fn model_direct_read_enforces_profile_boundary_even_when_path_is_known() {
    let store = new_test_store().await;
    let pair = LogicalMemoryPath::profile_local("pair", "note");
    let chat = LogicalMemoryPath::profile_local("chat", "note");
    let core = LogicalMemoryPath::shared_core("note");
    let source =
        LogicalMemoryPath::source_local(MemorySource::Conversation(Uuid::new_v4()), "note");
    let hat = LogicalMemoryPath::hat(den_core::ids::HatId::new(Uuid::new_v4()), "note");
    for (path, content) in [
        (&pair, "pair private"),
        (&chat, "chat private"),
        (&core, "core shared"),
        (&source, "source private"),
        (&hat, "hat reviewed"),
    ] {
        append_memory_record(&store, path, "note", "pair", None, content, &json!({}))
            .await
            .expect("store fixture");
    }

    let pair_path = pair.to_logical_path();
    let chat_path = chat.to_logical_path();
    let core_path = core.to_logical_path();
    let source_path = source.to_logical_path();
    let hat_path = hat.to_logical_path();
    assert_eq!(
        sqlite_memory_read_for_profile(&store, BearProfile::Pair, &pair_path)
            .await
            .unwrap()["content"],
        "pair private"
    );
    assert_eq!(
        sqlite_memory_read_for_profile(&store, BearProfile::Pair, &core_path)
            .await
            .unwrap()["content"],
        "core shared"
    );
    assert_eq!(
        sqlite_memory_read_for_profile(&store, BearProfile::Curate, &chat_path)
            .await
            .unwrap()["content"],
        "chat private"
    );
    for (profile, path) in [
        (BearProfile::Pair, chat_path.as_str()),
        (BearProfile::Work, pair_path.as_str()),
        (BearProfile::Chat, pair_path.as_str()),
        (BearProfile::Pair, source_path.as_str()),
        (BearProfile::Curate, hat_path.as_str()),
    ] {
        let result = sqlite_memory_read_for_profile(&store, profile, path)
            .await
            .expect("read must fail closed");
        assert_eq!(
            result["ok"], false,
            "unexpected read of {path} as {profile}"
        );
        assert!(result.get("content").is_none());
    }
}

#[tokio::test]
async fn model_direct_read_filters_each_row_when_paths_collide() {
    let store = new_test_store().await;
    let path = LogicalMemoryPath::profile_local("pair", "note");
    let logical_path = path.to_logical_path();
    append_memory_record(&store, &path, "note", "pair", None, "owned", &json!({}))
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO memory_records (memory_id, bear_id, sequence_no, scope_type,
            scope_profile, kind, author_profile, created_at, content_text, logical_path)
         VALUES (?, ?, 900, 'profile_local', 'chat', 'note', 'chat',
                 '2026-01-01T00:00:00Z', 'chat secret', ?)",
    )
    .bind(Uuid::new_v4().to_string())
    .bind(store.bear_id().to_string())
    .bind(&logical_path)
    .execute(store.pool())
    .await
    .unwrap();
    let result = sqlite_memory_read_for_profile(&store, BearProfile::Pair, &logical_path)
        .await
        .unwrap();
    assert_eq!(result["record_count"], 1);
    assert_eq!(result["content"], "owned");
}

#[tokio::test]
async fn source_entry_uses_canonical_owner_without_promoting_legacy_notes() {
    use crate::{
        scoped::{self, MemoryReadGrant},
        AccessContext,
    };

    let store = new_test_store().await;
    let source = MemorySource::Conversation(Uuid::new_v4());
    let tags = vec!["review".to_string()];
    let entry = SqliteMemoryEntryWrite {
        kind: "note",
        title: "Finding",
        body: "Only this conversation has the raw finding",
        tags: &tags,
        refs: None,
        lifecycle: Some(json!({ "status": "active" })),
        source: Some(json!({ "origin": "test" })),
        author: Some("human".to_string()),
    };
    let written = write_semantic_entry(
        &store,
        LogicalMemoryPath::source_local(source, "note"),
        "pair",
        entry,
    )
    .await
    .expect("write source-local memory");
    let path = written["path"].as_str().expect("logical path");
    let own = scoped::read_path(
        &store,
        MemoryReadGrant::new(source, None),
        &AccessContext::empty(),
        path,
        10,
    )
    .await
    .expect("read own source memory");
    assert_eq!(own.len(), 1);
    assert_eq!(own[0].metadata_json["source"]["origin"], "test");
    assert_eq!(own[0].metadata_json["tags"], json!(["review"]));
    assert!(scoped::read_path(
        &store,
        MemoryReadGrant::new(MemorySource::Conversation(Uuid::new_v4()), None),
        &AccessContext::empty(),
        path,
        10,
    )
    .await
    .unwrap()
    .is_empty());
    assert_eq!(
        sqlite_memory_read_for_profile(&store, BearProfile::Pair, path)
            .await
            .unwrap()["ok"],
        false
    );
}
