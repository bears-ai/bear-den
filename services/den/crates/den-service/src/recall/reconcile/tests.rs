use super::*;
use den_core::{config::Config, ids::UserId};
use den_memory::{append_memory_record, LogicalMemoryPath, MemorySource, MemoryStoreManager};
use serde_json::json;
use sqlx::PgPool;

#[sqlx::test(migrations = "../../migrations")]
async fn configuring_hats_removes_legacy_profile_heads_without_losing_core_or_hat_heads(
    pool: PgPool,
) {
    use crate::bears::{
        db::{self, BearParams},
        hats,
    };
    let user = sqlx::query_scalar!(
        "INSERT INTO users (email, username) VALUES ('recallhat@example.test', 'recallhat') RETURNING id"
    ).fetch_one(&pool).await.unwrap();
    let bear_id = db::create_bear(
        &pool,
        BearParams {
            slug: "recallhatbear",
            name: "Recall Hat",
            description: "",
            system_prompt: "",
            default_model: None,
            tools_enabled: None,
            context_profile: None,
        },
    )
    .await
    .unwrap();
    let mut config = Config::test_stub();
    config.bear_sqlite_data_dir = std::env::temp_dir()
        .join(format!("recall-hat-cutover-{}", Uuid::new_v4()))
        .to_string_lossy()
        .to_string();
    let stores = MemoryStoreManager::new(&config);
    let store = stores.store_for_bear(bear_id).await.unwrap();
    let old = append_memory_record(
        &store,
        &LogicalMemoryPath::profile_local("pair", "note"),
        "note",
        "pair",
        None,
        "old profile text must not leave the Bear after hats exist",
        &json!({}),
    )
    .await
    .unwrap();
    let shared = append_memory_record(
        &store,
        &LogicalMemoryPath::shared_core("note"),
        "note",
        "curate",
        None,
        "reviewed Bear text",
        &json!({}),
    )
    .await
    .unwrap();
    let before = list_authorized_indexable_heads(&pool, &store)
        .await
        .unwrap();
    assert!(before.iter().any(|head| head.memory_id == old.memory_id));
    let hat = hats::create_hat(
        &pool,
        BearId::new(bear_id),
        UserId::new(user),
        "Review",
        "Review changes",
    )
    .await
    .unwrap();
    let curated = append_memory_record(
        &store,
        &LogicalMemoryPath::hat(hat.id, "note"),
        "note",
        "curate",
        None,
        "reviewed hat text",
        &json!({}),
    )
    .await
    .unwrap();
    let after = list_authorized_indexable_heads(&pool, &store)
        .await
        .unwrap();
    assert!(!after.iter().any(|head| head.memory_id == old.memory_id));
    assert!(after.iter().any(|head| head.memory_id == shared.memory_id));
    assert!(after.iter().any(|head| head.memory_id == curated.memory_id));
}

#[tokio::test]
async fn reconcile_indexes_curated_hat_heads_but_not_private_source_notes() {
    let mut config = Config::test_stub();
    config.bear_sqlite_data_dir = std::env::temp_dir()
        .join(format!("index-hat-{}", Uuid::new_v4()))
        .to_string_lossy()
        .to_string();
    let stores = MemoryStoreManager::new(&config);
    let store = stores.store_for_bear(Uuid::new_v4()).await.unwrap();
    let hat = HatId::new(Uuid::new_v4());
    let current = append_memory_record(
        &store,
        &LogicalMemoryPath::hat(hat, "note"),
        "note",
        "curate",
        None,
        "safe reviewed hat note",
        &json!({}),
    )
    .await
    .unwrap();
    let raw = append_memory_record(
        &store,
        &LogicalMemoryPath::source_local(MemorySource::Conversation(Uuid::new_v4()), "note"),
        "note",
        "pair",
        None,
        "private source note",
        &json!({}),
    )
    .await
    .unwrap();
    let _archived = append_memory_record(
        &store,
        &LogicalMemoryPath::hat(hat, "old"),
        "old",
        "curate",
        None,
        "archived note",
        &json!({"lifecycle": {"status": "archived"}}),
    )
    .await
    .unwrap();
    let gated = append_memory_record(
        &store,
        &LogicalMemoryPath::hat(hat, "restricted"),
        "restricted",
        "curate",
        None,
        "restricted hat entry must not be embedded",
        &json!({}),
    )
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO memory_access_rules (link_id, bear_id, sequence_no, src_memory_id,
            entity_id, relation, author_profile, created_at)
         VALUES (?, ?, ?, ?, ?, 'confined_to', 'curate', '2026-09-30T00:00:00Z')",
    )
    .bind(Uuid::new_v4().to_string())
    .bind(store.bear_id().to_string())
    .bind(store.next_sequence().await.unwrap())
    .bind(&gated.memory_id)
    .bind(Uuid::new_v4().to_string())
    .execute(store.pool())
    .await
    .unwrap();
    let heads = list_indexable_heads(&store).await.unwrap();
    assert!(heads.iter().any(|head| {
        head.memory_id == current.memory_id && head.scope_hat_id == Some(hat) && head.is_indexable()
    }));
    assert!(!heads.iter().any(|head| head.memory_id == raw.memory_id));
    assert!(!heads.iter().any(|head| head.memory_id == gated.memory_id));
    assert!(!heads
        .iter()
        .any(|head| head.content_text == "archived note"));
}
