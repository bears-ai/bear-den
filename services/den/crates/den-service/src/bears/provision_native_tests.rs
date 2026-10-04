use uuid::Uuid;

use crate::bears::{
    db::{self, BearParams},
    managed_blocks::get_compiled_bear_config,
    model::RuntimeContextLabel,
    provision::initialize_bear_native,
    runtime_plan::default_runtime_plan,
};
use den_core::{config::Config, DenError};

fn memory_config() -> Config {
    let mut config = Config::test_stub();
    config.bear_sqlite_data_dir = format!("/tmp/bears-initialize-native-{}", Uuid::new_v4());
    config
}

async fn create_test_bear(
    pool: &sqlx::PgPool,
    context_profile: Option<sqlx::types::Json<serde_json::Value>>,
) -> Result<Uuid, DenError> {
    db::create_bear(
        pool,
        BearParams {
            slug: "native-initialize-bear",
            name: "Native Initialize Bear",
            description: "test",
            system_prompt: "You are a concise test bear.",
            default_model: None,
            tools_enabled: None,
            context_profile,
        },
    )
    .await
}

async fn assert_no_bindings(pool: &sqlx::PgPool, bear_id: Uuid) -> Result<(), DenError> {
    for role in RuntimeContextLabel::ALL {
        assert!(db::get_bear_profile_binding(pool, bear_id, role)
            .await?
            .is_none());
    }
    Ok(())
}

#[sqlx::test(migrations = "../../migrations")]
async fn initialize_bear_native_preserves_store_and_runtime_plan_without_bindings(
    pool: sqlx::PgPool,
) -> Result<(), Box<dyn std::error::Error>> {
    let config = memory_config();
    let stores = den_memory::MemoryStoreManager::new(&config);
    let bear_id = create_test_bear(&pool, None).await?;

    initialize_bear_native(&pool, &stores.clone(), bear_id).await?;
    assert_no_bindings(&pool, bear_id).await?;
    let bear = db::get_bear(&pool, bear_id).await?.unwrap();
    assert_eq!(bear.runtime_plan.unwrap().0, default_runtime_plan());
    assert!(
        bear.default_model.is_none(),
        "initialization must not select a model"
    );
    assert!(get_compiled_bear_config(&pool, bear_id).await?.is_none());
    assert!(std::path::Path::new(&config.bear_sqlite_data_dir)
        .join(format!("{bear_id}.sqlite"))
        .is_file());

    let store = stores.store_for_bear(bear_id).await?;
    let first_sequence = store.next_sequence().await?;
    initialize_bear_native(&pool, &stores.clone(), bear_id).await?;
    let next_store = stores.store_for_bear(bear_id).await?;
    assert_eq!(next_store.next_sequence().await?, first_sequence + 1);
    assert_eq!(
        db::get_bear(&pool, bear_id)
            .await?
            .unwrap()
            .runtime_plan
            .unwrap()
            .0,
        default_runtime_plan()
    );
    assert_no_bindings(&pool, bear_id).await?;

    Ok(())
}

#[sqlx::test(migrations = "../../migrations")]
async fn initialize_bear_native_compiles_managed_config_without_bindings(
    pool: sqlx::PgPool,
) -> Result<(), Box<dyn std::error::Error>> {
    let config = memory_config();
    let stores = den_memory::MemoryStoreManager::new(&config);
    let bear_id = create_test_bear(
        &pool,
        Some(sqlx::types::Json(serde_json::json!({
            "composition_version": 1,
            "role_contracts": {
                "chat": "CHAT", "pair": "PAIR", "curate": "CURATE",
                "work": "WORK", "watch": "WATCH"
            },
            "user_steering": "Keep answers concise",
            "bear_context": "Shared charter"
        }))),
    )
    .await?;

    initialize_bear_native(&pool, &stores, bear_id).await?;
    let compiled = get_compiled_bear_config(&pool, bear_id).await?.unwrap();
    assert!(compiled.rendered_prompts_json.0["bound_base"]
        .as_str()
        .unwrap()
        .contains("Shared charter"));
    assert!(!compiled.config_hash.is_empty());
    assert_no_bindings(&pool, bear_id).await?;

    Ok(())
}

#[sqlx::test(migrations = "../../migrations")]
async fn initialize_bear_native_retains_historical_bindings_and_existing_plan(
    pool: sqlx::PgPool,
) -> Result<(), Box<dyn std::error::Error>> {
    let config = memory_config();
    let stores = den_memory::MemoryStoreManager::new(&config);
    let bear_id = create_test_bear(&pool, None).await?;
    let existing_plan = serde_json::json!({"schema_version": 1, "historical": true});
    db::ensure_default_runtime_plan(&pool, bear_id, &existing_plan).await?;
    let historical_hash = serde_json::json!({"historical": true});
    db::mark_bear_profile_binding_ready(
        &pool,
        bear_id,
        RuntimeContextLabel::ArmatureConversation,
        "historical-binding",
        7,
        &historical_hash,
    )
    .await?;
    let before =
        db::get_bear_profile_binding(&pool, bear_id, RuntimeContextLabel::ArmatureConversation)
            .await?
            .unwrap();

    initialize_bear_native(&pool, &stores, bear_id).await?;
    let after =
        db::get_bear_profile_binding(&pool, bear_id, RuntimeContextLabel::ArmatureConversation)
            .await?
            .unwrap();
    assert_eq!(after.binding_id, before.binding_id);
    assert_eq!(after.provisioning_status, before.provisioning_status);
    assert_eq!(
        after.last_provisioned_version,
        before.last_provisioned_version
    );
    assert_eq!(after.config_hash, before.config_hash);
    assert_eq!(after.last_synced_at, before.last_synced_at);
    assert_eq!(
        after.last_provisioning_error,
        before.last_provisioning_error
    );
    assert_eq!(after.updated_at, before.updated_at);
    for role in RuntimeContextLabel::ALL {
        if role != RuntimeContextLabel::ArmatureConversation {
            assert!(db::get_bear_profile_binding(&pool, bear_id, role)
                .await?
                .is_none());
        }
    }
    assert_eq!(
        db::get_bear(&pool, bear_id)
            .await?
            .unwrap()
            .runtime_plan
            .unwrap()
            .0,
        existing_plan
    );

    Ok(())
}

#[sqlx::test(migrations = "../../migrations")]
async fn initialize_bear_native_requires_existing_bear(
    pool: sqlx::PgPool,
) -> Result<(), Box<dyn std::error::Error>> {
    let config = memory_config();
    let stores = den_memory::MemoryStoreManager::new(&config);
    let bear_id = Uuid::new_v4();
    assert!(matches!(
        initialize_bear_native(&pool, &stores, bear_id).await,
        Err(DenError::NotFound(_))
    ));
    assert!(!std::path::Path::new(&config.bear_sqlite_data_dir).exists());
    Ok(())
}
