use crate::core::tools::constants::*;
use den_service::bears::BearProfile;

use super::core_helpers::names_for_profile;

#[test]
fn privileged_descriptors_are_role_scoped() {
    let chat = names_for_profile(BearProfile::Chat);
    assert!(chat.contains(DEN_TASK_WRITE_INTENT));
    assert!(chat.contains(DEN_SKILL_PROPOSE));
    assert!(!chat.contains(DEN_OBSERVATION_WRITE));
    assert!(!chat.contains(DEN_RUN_WRITE_RESULT));

    let pair = names_for_profile(BearProfile::Pair);
    assert!(pair.contains(DEN_TASK_WRITE_INTENT));
    assert!(pair.contains(DEN_TASK_LISTS_UPDATE));
    assert!(pair.contains(DEN_TASK_LISTS_REQUEST_HANDOFF));
    assert!(pair.contains(DEN_SKILL_PROPOSE));
    assert!(!pair.contains(DEN_OBSERVATION_WRITE));
    assert!(!pair.contains(DEN_RUN_WRITE_RESULT));

    let curate = names_for_profile(BearProfile::Curate);
    assert!(curate.contains(DEN_TASK_APPROVE_INTENT));
    assert!(curate.contains(DEN_TASK_REJECT_INTENT));
    assert!(curate.contains(DEN_CORE_WRITE_RESULT_SUMMARY));
    assert!(curate.contains(DEN_SKILL_APPROVE_PROPOSAL));
    assert!(curate.contains(DEN_SKILL_REJECT_PROPOSAL));
    assert!(curate.contains(DEN_SKILL_PROPOSE));
    assert!(curate.contains(DEN_ENTITY_BROWSE));
    assert!(curate.contains(DEN_ENTITY_RESOLVE));
    assert!(!curate.contains(DEN_ENTITY_LINK_MEMORY));
    assert!(curate.contains(DEN_ENTITY_MERGE));
    assert!(curate.contains(DEN_ENTITY_SPLIT));
    assert!(curate.contains(DEN_ENTITY_WRITE_ACCESS_RULE));
    assert!(curate.contains(DEN_ENTITY_WRITE_ANCHOR));
    assert!(!curate.contains(DEN_TASK_WRITE_INTENT));
    assert!(!curate.contains(DEN_OBSERVATION_WRITE));
    assert!(!curate.contains(DEN_RUN_WRITE_RESULT));

    let watch = names_for_profile(BearProfile::Watch);
    assert!(watch.contains(DEN_MEMORY_STATUS));
    assert!(watch.contains(DEN_MEMORY_SEARCH));
    assert!(watch.contains(DEN_MEMORY_READ));
    assert!(watch.contains(DEN_ENTITY_BROWSE));
    assert!(watch.contains(DEN_ENTITY_RESOLVE));
    assert!(watch.contains(DEN_ENTITY_LINK_MEMORY));
    assert!(!watch.contains(DEN_ENTITY_MERGE));
    assert!(!watch.contains(DEN_ENTITY_SPLIT));
    assert!(!watch.contains(DEN_ENTITY_WRITE_ACCESS_RULE));
    assert!(!watch.contains(DEN_ENTITY_WRITE_ANCHOR));
    assert!(watch.contains(DEN_OBSERVATION_WRITE));
    assert!(watch.contains(DEN_SKILL_PROPOSE));
    assert!(!watch.contains(DEN_TASK_LISTS_LIST));
    assert!(!watch.contains(DEN_TASK_LISTS_UPDATE));
    assert!(!watch.contains(DEN_TASK_WRITE_INTENT));
    assert!(!watch.contains(DEN_RUN_WRITE_RESULT));

    let work = names_for_profile(BearProfile::Work);
    assert!(work.contains(DEN_MEMORY_STATUS));
    assert!(work.contains(DEN_MEMORY_SEARCH));
    assert!(work.contains(DEN_MEMORY_READ));
    assert!(work.contains(DEN_ENTITY_BROWSE));
    assert!(work.contains(DEN_ENTITY_RESOLVE));
    assert!(work.contains(DEN_ENTITY_LINK_MEMORY));
    assert!(!work.contains(DEN_ENTITY_MERGE));
    assert!(!work.contains(DEN_ENTITY_SPLIT));
    assert!(!work.contains(DEN_ENTITY_WRITE_ACCESS_RULE));
    assert!(!work.contains(DEN_ENTITY_WRITE_ANCHOR));
    assert!(work.contains(DEN_RUN_WRITE_RESULT));
    assert!(work.contains(DEN_TASK_LISTS_LIST));
    assert!(work.contains(DEN_TASK_LISTS_UPDATE));
    assert!(!work.contains(DEN_TASK_LISTS_REQUEST_HANDOFF));
    assert!(work.contains(DEN_SKILL_PROPOSE));
    assert!(!work.contains(DEN_TASK_WRITE_INTENT));
    assert!(!work.contains(DEN_OBSERVATION_WRITE));
}

#[sqlx::test]
async fn model_memory_read_cannot_use_another_profile_or_new_scope_path(pool: sqlx::PgPool) {
    use den_core::{
        ids::{BearId, UserId},
        tools::context::DenToolInvocationContext,
    };
    use den_memory::{append_memory_record, LogicalMemoryPath, MemoryStoreManager};
    use den_service::{
        bears::hats::{bindings::bind_conversation_hat, create_hat},
        conversation::persistence::ensure_conversation_for_external_id,
    };
    use serde_json::json;
    use uuid::Uuid;

    use crate::{config::Config, core::tools::memory_read::DenRoleMemoryStore};

    let suffix = Uuid::new_v4().simple().to_string();
    let bear_id = sqlx::query_scalar!(
        "INSERT INTO bears (slug, name) VALUES ($1, 'Memory Test Bear') RETURNING id",
        format!("memory-test-{}", &suffix[..12]),
    )
    .fetch_one(&pool)
    .await
    .expect("create Bear");
    let conversation =
        ensure_conversation_for_external_id(&pool, bear_id, None, "hat-memory-a", None, None)
            .await
            .expect("create conversation A");
    let second =
        ensure_conversation_for_external_id(&pool, bear_id, None, "hat-memory-b", None, None)
            .await
            .expect("create conversation B");
    let user_id = sqlx::query_scalar!(
        "INSERT INTO users (email, username, display_name, passhash) VALUES ($1, $2, 'Test', 'x') RETURNING id",
        format!("memory-{suffix}@example.invalid"),
        format!("memory{}", &suffix[..12]),
    ).fetch_one(&pool).await.expect("create user");
    let hat = create_hat(
        &pool,
        BearId::new(bear_id),
        UserId::new(user_id),
        "Security review",
        "Read scope test",
    )
    .await
    .expect("create hat");
    bind_conversation_hat(&pool, BearId::new(bear_id), conversation.id, hat.id)
        .await
        .expect("bind A");
    bind_conversation_hat(&pool, BearId::new(bear_id), second.id, hat.id)
        .await
        .expect("bind B");

    let mut config = Config::test_stub();
    config.bear_sqlite_data_dir = std::env::temp_dir()
        .join(format!("den-model-memory-read-{}", Uuid::new_v4()))
        .display()
        .to_string();
    let stores = MemoryStoreManager::new(&config);
    let store = stores.store_for_bear(bear_id).await.expect("memory store");
    let pair = LogicalMemoryPath::profile_local("pair", "note");
    append_memory_record(
        &store,
        &pair,
        "note",
        "pair",
        None,
        "pair secret",
        &json!({}),
    )
    .await
    .expect("write legacy pair note");
    let adapter = DenRoleMemoryStore::new(&pool, &config, &stores);
    let context: DenToolInvocationContext = serde_json::from_value(json!({
        "bear_id": bear_id, "bear_slug": "memory-test", "binding_id": "test-binding",
        "profile": "pair", "user_id": user_id, "username": null,
        "membership_role": null, "conversation_id": "hat-memory-a",
        "session_id": "test-session", "request_id": null, "channel": {}
    }))
    .expect("tool context");
    let written = den_core::tools::memory::write_memory_entry(
        &adapter,
        &context,
        BearProfile::Pair,
        json!({"kind": "note", "title": "A finding", "body": "private-source-a-token"}),
        None,
        None,
    )
    .await
    .expect("write scoped note");
    let source_path = written["path"].as_str().expect("source path").to_string();
    let own = den_core::tools::memory::memory_read(
        &adapter,
        &context,
        BearProfile::Pair,
        json!({ "path": source_path }),
    )
    .await
    .expect("read own note");
    assert!(own["content"]
        .as_str()
        .unwrap()
        .contains("private-source-a-token"));
    let legacy = den_core::tools::memory::memory_read(
        &adapter,
        &context,
        BearProfile::Pair,
        json!({ "path": pair.to_logical_path() }),
    )
    .await
    .expect("read legacy note");
    assert_eq!(legacy["ok"], false);
    let mut other_context = context.clone();
    other_context.conversation_id = "hat-memory-b".to_string();
    let other = den_core::tools::memory::memory_read(
        &adapter,
        &other_context,
        BearProfile::Pair,
        json!({ "path": source_path }),
    )
    .await
    .expect("read from B");
    assert_eq!(other["ok"], false);
    let hits = den_core::tools::memory::memory_search(
        &adapter,
        &other_context,
        BearProfile::Pair,
        json!({"query": "private-source-a-token"}),
    )
    .await
    .expect("search B");
    assert!(hits["hits"].as_array().unwrap().is_empty());
}
