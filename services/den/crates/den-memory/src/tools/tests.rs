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
