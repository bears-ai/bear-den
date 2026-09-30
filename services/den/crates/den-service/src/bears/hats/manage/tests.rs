use super::*;
use den_core::{config::Config, ids::UserId};
use den_memory::{append_memory_record, LogicalMemoryPath};
use serde_json::json;

#[sqlx::test(migrations = "../../migrations")]
async fn admin_configuration_narrows_surfaces_and_requires_empty_memory_before_work(pool: PgPool) {
    let user = sqlx::query_scalar!(
        "INSERT INTO users (email, username) VALUES ('managehat@example.test', 'managehat') RETURNING id"
    ).fetch_one(&pool).await.unwrap();
    let bear = sqlx::query_scalar!(
        "INSERT INTO bears (slug, name) VALUES ('managehatbear', 'Hat management') RETURNING id"
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let other_bear = sqlx::query_scalar!(
        "INSERT INTO bears (slug, name) VALUES ('managehatother', 'Other Bear') RETURNING id"
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let bear = BearId::new(bear);
    let hat = super::super::create_hat(
        &pool,
        bear,
        UserId::new(user),
        "Review",
        "Review a repository",
    )
    .await
    .unwrap();
    let surface = Uuid::new_v4();
    let other_surface = Uuid::new_v4();
    for (id, name) in [
        (surface, "managehatrepo"),
        (other_surface, "managehatotherrepo"),
    ] {
        sqlx::query!(
            "INSERT INTO work_surfaces (id, name, kind, created_by_user_id, created_at, updated_at) VALUES ($1, $2, 'git_workspace', $3, NOW(), NOW())",
            id, name, user
        ).execute(&pool).await.unwrap();
    }
    sqlx::query!(
        "INSERT INTO work_surface_bears (surface_id, bear_id) VALUES ($1, $2)",
        surface,
        bear.as_uuid()
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query!(
        "INSERT INTO work_surface_bears (surface_id, bear_id) VALUES ($1, $2)",
        other_surface,
        other_bear
    )
    .execute(&pool)
    .await
    .unwrap();
    assert!(replace_surfaces(&pool, bear, hat.id, &[other_surface])
        .await
        .is_err());
    assert!(allowed_surfaces(&pool, bear, hat.id)
        .await
        .unwrap()
        .is_empty());
    replace_surfaces(&pool, bear, hat.id, &[surface, surface])
        .await
        .unwrap();
    assert_eq!(
        allowed_surfaces(&pool, bear, hat.id).await.unwrap(),
        vec![surface]
    );
    assert!(get_hat(&pool, BearId::new(other_bear), hat.id)
        .await
        .is_err());
    update_hat(
        &pool,
        bear,
        hat.id,
        " Security review ",
        " Check dependencies ",
        " Inspect dependency versions ",
        false,
    )
    .await
    .unwrap();
    assert_eq!(
        get_hat(&pool, bear, hat.id).await.unwrap().name,
        "Security review"
    );
    assert_eq!(
        get_hat(&pool, bear, hat.id).await.unwrap().identity_prompt,
        "Inspect dependency versions"
    );
    let mut config = Config::test_stub();
    config.bear_sqlite_data_dir = std::env::temp_dir()
        .join(format!("hat-config-{}", Uuid::new_v4()))
        .to_string_lossy()
        .to_string();
    let stores = MemoryStoreManager::new(&config);
    enable_work_if_empty(&pool, &stores, bear, hat.id)
        .await
        .unwrap();
    assert!(get_hat(&pool, bear, hat.id).await.unwrap().work_enabled);
    assert!(matches!(
        update_hat(
            &pool,
            bear,
            hat.id,
            "Security review",
            "Check dependencies",
            "Review secrets before publishing",
            false,
        )
        .await,
        Err(DenError::Authorization(_))
    ));
    update_hat(
        &pool,
        bear,
        hat.id,
        "Security review",
        "Check dependencies",
        "Review secrets before publishing",
        true,
    )
    .await
    .unwrap();
    sqlx::query!(
        "INSERT INTO work_surface_bears (surface_id, bear_id) VALUES ($1, $2)",
        other_surface,
        bear.as_uuid()
    )
    .execute(&pool)
    .await
    .unwrap();
    assert!(
        replace_surfaces(&pool, bear, hat.id, &[surface, other_surface])
            .await
            .is_err()
    );
    assert_eq!(
        allowed_surfaces(&pool, bear, hat.id).await.unwrap(),
        vec![surface]
    );
    disable_work(&pool, bear, hat.id).await.unwrap();
    let store = stores.store_for_bear(bear.as_uuid()).await.unwrap();
    append_memory_record(
        &store,
        &LogicalMemoryPath::hat(hat.id, "note"),
        "note",
        "curate",
        None,
        "historical curated text",
        &json!({}),
    )
    .await
    .unwrap();
    assert!(enable_work_if_empty(&pool, &stores, bear, hat.id)
        .await
        .is_err());
    assert!(!get_hat(&pool, bear, hat.id).await.unwrap().work_enabled);
    replace_surfaces(&pool, bear, hat.id, &[]).await.unwrap();
    assert!(allowed_surfaces(&pool, bear, hat.id)
        .await
        .unwrap()
        .is_empty());

    let used = crate::conversation::persistence::ensure_conversation_for_external_id(
        &pool,
        bear.as_uuid(),
        Some(user),
        "conv-hat-started",
        None,
        None,
    )
    .await
    .unwrap();
    crate::conversation::persistence::append_message(
        &pool,
        used.id,
        &crate::conversation::message_types::ConversationMessageWrite::user_turn(
            "Already started",
            json!({"text":"Already started"}),
            None,
        ),
    )
    .await
    .unwrap();
    assert!(
        !super::super::bindings::conversation_can_bind_hat(&pool, bear, used.id)
            .await
            .unwrap()
    );
    assert!(
        super::super::bindings::bind_conversation_hat(&pool, bear, used.id, hat.id)
            .await
            .is_err()
    );

    let connected = crate::conversation::persistence::ensure_conversation_for_external_id(
        &pool,
        bear.as_uuid(),
        Some(user),
        "conv-hat-open",
        None,
        None,
    )
    .await
    .unwrap();
    crate::client_sessions::upsert_session(
        &pool,
        crate::client_sessions::UpsertClientSession {
            user_id: user,
            bear_id: bear.as_uuid(),
            bear_slug: "managehatbear".into(),
            client_session_id: format!("hat-open-{}", Uuid::new_v4()),
            runtime_session_id: "runtime-hat-open".into(),
            conversation_id: "conv-hat-open".into(),
            resolved_conversation_id: None,
            client: "test".into(),
            cwd: None,
            current_mode: None,
        },
    )
    .await
    .unwrap();
    assert!(
        !super::super::bindings::conversation_can_bind_hat(&pool, bear, connected.id)
            .await
            .unwrap()
    );
    assert!(
        super::super::bindings::bind_conversation_hat(&pool, bear, connected.id, hat.id)
            .await
            .is_err()
    );
}
