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

#[tokio::test]
async fn model_memory_read_cannot_use_another_profile_or_new_scope_path() {
    use serde_json::json;
    use uuid::Uuid;

    use crate::{config::Config, core::tools::memory_read::DenRoleMemoryStore};
    use den_memory::{append_memory_record, LogicalMemoryPath, MemorySource, MemoryStoreManager};

    let mut config = Config::test_stub();
    config.bear_sqlite_data_dir = std::env::temp_dir()
        .join(format!("den-model-memory-read-{}", Uuid::new_v4()))
        .display()
        .to_string();
    let stores = MemoryStoreManager::new(&config);
    let bear_id = Uuid::new_v4();
    let store = stores.store_for_bear(bear_id).await.expect("memory store");
    let pair = LogicalMemoryPath::profile_local("pair", "note");
    let source =
        LogicalMemoryPath::source_local(MemorySource::Conversation(Uuid::new_v4()), "note");
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
    .expect("write pair note");
    append_memory_record(
        &store,
        &source,
        "note",
        "pair",
        None,
        "session secret",
        &json!({}),
    )
    .await
    .expect("write source note");
    let pool = sqlx::PgPool::connect_lazy("postgres://unused:unused@localhost/unused")
        .expect("lazy Postgres pool");
    let adapter = DenRoleMemoryStore::new(&pool, &config, &stores);
    for path in [pair.to_logical_path(), source.to_logical_path()] {
        let result = den_core::tools::memory::memory_read(
            &adapter,
            bear_id,
            BearProfile::Work,
            json!({ "path": path }),
        )
        .await
        .expect("memory_read result");
        assert_eq!(result["ok"], false, "Work read a forbidden path: {path}");
        assert!(result.get("content").is_none());
    }
    let own = den_core::tools::memory::memory_read(
        &adapter,
        bear_id,
        BearProfile::Pair,
        json!({ "path": pair.to_logical_path() }),
    )
    .await
    .expect("Pair read");
    assert_eq!(own["content"], "pair secret");
}
