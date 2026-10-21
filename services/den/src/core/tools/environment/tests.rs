use super::*;
use den_memory::{EntityTrust, ResolutionState};
use uuid::Uuid;

fn fixture() -> (Config, MemoryStoreManager, PgPool, DenToolInvocationContext) {
    let mut config = Config::test_stub();
    config.bear_sqlite_data_dir = std::env::temp_dir()
        .join(format!("den-session-entities-{}", Uuid::new_v4()))
        .to_string_lossy()
        .into_owned();
    let stores = MemoryStoreManager::new(&config);
    let pool = PgPool::connect_lazy("postgres://unused:unused@localhost/unused").unwrap();
    let context = serde_json::from_value(json!({
        "bear_id": Uuid::new_v4(),
        "bear_slug": "session-entities-test",
        "binding_id": "test",
        "user_id": 1,
        "conversation_id": "conversation",
        "session_id": "session",
    }))
    .unwrap();
    (config, stores, pool, context)
}

#[tokio::test]
async fn session_human_uses_authenticated_user_not_username_and_is_bear_scoped() {
    let (config, stores, pool, mut context) = fixture();
    let store = stores.store_for_bear(context.bear_id).await.unwrap();
    let legacy = memory_store::create_entity(
        &store,
        "person",
        Some("Shared username"),
        ResolutionState::Resolved,
        EntityTrust::Asserted,
        Some("den:user:1"),
        &json!({}),
    )
    .await
    .unwrap();
    memory_store::attach_handle(
        &store,
        &legacy.entity_id,
        "den_user",
        "1",
        None,
        EntityTrust::Asserted,
    )
    .await
    .unwrap();
    memory_store::attach_handle(
        &store,
        &legacy.entity_id,
        "session_human",
        "shared-name",
        None,
        EntityTrust::Asserted,
    )
    .await
    .unwrap();
    let ops = DenEnvironmentOps {
        pool: &pool,
        config: &config,
        stores: &stores,
    };
    let role = RuntimeContextLabel::ArmatureConversation;
    let first = ops.session_entities(&context, role).await.unwrap();
    let binding = memory_store::resolve_authenticated_user_entity(&store, UserId::new(1))
        .await
        .unwrap();
    assert_eq!(
        first["human"],
        json!({"source": "den_user", "entity": binding.entity()})
    );
    assert_eq!(first["work_surface"], Value::Null);
    assert_ne!(binding.entity_id(), legacy.entity_id);

    context.username = Some("shared-name".into());
    let renamed = ops.session_entities(&context, role).await.unwrap();
    assert_eq!(renamed["human"], first["human"]);

    context.user_id = 2;
    let other_user = ops.session_entities(&context, role).await.unwrap();
    assert_eq!(other_user["human"]["source"], "den_user");
    assert_ne!(other_user["human"]["entity"]["entity_id"], legacy.entity_id);
    assert_ne!(
        other_user["human"]["entity"]["entity_id"],
        binding.entity_id()
    );
    context.username = None;
    assert_eq!(
        ops.session_entities(&context, role).await.unwrap()["human"],
        other_user["human"]
    );

    context.user_id = 1;
    context.bear_id = Uuid::new_v4();
    let other_bear = ops.session_entities(&context, role).await.unwrap();
    assert_eq!(other_bear["human"]["source"], "den_user");
    assert_ne!(
        other_bear["human"]["entity"]["entity_id"],
        binding.entity_id()
    );
    store.pool().close().await;
    stores
        .store_for_bear(context.bear_id)
        .await
        .unwrap()
        .pool()
        .close()
        .await;
    std::fs::remove_dir_all(&config.bear_sqlite_data_dir).unwrap();
}

#[tokio::test]
async fn authenticated_session_human_preserves_work_surface_lookup() {
    let (config, stores, pool, mut context) = fixture();
    let store = stores.store_for_bear(context.bear_id).await.unwrap();
    let surface = memory_store::create_entity(
        &store,
        "work_surface",
        Some("Workspace"),
        ResolutionState::Resolved,
        EntityTrust::Asserted,
        None,
        &json!({}),
    )
    .await
    .unwrap();
    memory_store::attach_handle(
        &store,
        &surface.entity_id,
        "workspace_root",
        "/workspace/project",
        None,
        EntityTrust::Asserted,
    )
    .await
    .unwrap();
    context.workspace_roots = vec!["/workspace/project".into()];
    let ops = DenEnvironmentOps {
        pool: &pool,
        config: &config,
        stores: &stores,
    };
    let result = ops
        .session_entities(&context, RuntimeContextLabel::ArmatureConversation)
        .await
        .unwrap();
    assert_eq!(result["status"], "ok");
    assert_eq!(result["human"]["source"], "den_user");
    assert_eq!(
        result["work_surface"],
        json!({"source": "workspace_root", "entity": surface})
    );
    store.pool().close().await;
    std::fs::remove_dir_all(&config.bear_sqlite_data_dir).unwrap();
}
