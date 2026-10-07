use super::*;

#[sqlx::test(migrations = "../../migrations")]
async fn crud_is_named_bear_scoped_and_roundtrips_typed_values(pool: PgPool) {
    let owner = bear(&pool, "config-owner").await;
    let other = bear(&pool, "config-other").await;
    reasoning_support(&pool, Some(true)).await;
    let configuration = create(
        &pool,
        owner,
        " Careful ",
        "gpt-5",
        Some(ThinkingEffort::High),
    )
    .await
    .unwrap();
    assert_eq!(configuration.name, "Careful");
    assert_eq!(configuration.bear_id, owner);
    assert_eq!(configuration.model_handle.as_str(), "openai/gpt-5");
    assert_eq!(configuration.thinking_effort, Some(ThinkingEffort::High));
    assert_eq!(
        get(&pool, owner, configuration.id).await.unwrap(),
        Some(configuration.clone())
    );
    assert_eq!(get(&pool, other, configuration.id).await.unwrap(), None);
    assert!(list(&pool, other).await.unwrap().is_empty());
    assert!(create(&pool, owner, "careful", "openai/gpt-4.1", None)
        .await
        .is_err());
    assert!(create(&pool, owner, " ", "openai/gpt-4.1", None)
        .await
        .is_err());
    create(&pool, other, "Careful", "openai/gpt-4.1", None)
        .await
        .unwrap();
    let alpha = create(&pool, owner, "Alpha", "openai/gpt-4.1", None)
        .await
        .unwrap();
    assert_eq!(list(&pool, owner).await.unwrap()[0].id, alpha.id);
    assert!(
        update(&pool, other, configuration.id, "Changed", "gpt-5", None)
            .await
            .is_err()
    );
    assert!(delete(&pool, other, configuration.id).await.is_err());
    assert!(
        update(&pool, owner, configuration.id, "Alpha", "gpt-5", None)
            .await
            .is_err()
    );
    let changed = update(
        &pool,
        owner,
        configuration.id,
        "Quick",
        "openai/gpt-4.1",
        None,
    )
    .await
    .unwrap();
    assert_eq!(changed.id, configuration.id);
    assert_eq!(changed.created_at, configuration.created_at);
    assert!(changed.updated_at >= configuration.updated_at);
    assert_eq!(changed.thinking_effort, None);
    delete(&pool, owner, configuration.id).await.unwrap();
    assert_eq!(get(&pool, owner, configuration.id).await.unwrap(), None);
    assert!(matches!(
        delete(&pool, owner, configuration.id).await,
        Err(DenError::NotFound(_))
    ));
}

#[sqlx::test(migrations = "../../migrations")]
async fn writes_reject_unsupported_effort_and_removed_models_without_mutating_records(
    pool: PgPool,
) {
    let owner = bear(&pool, "config-validation").await;
    reasoning_support(&pool, None).await;
    assert!(create(
        &pool,
        owner,
        "Unknown support",
        "gpt-5",
        Some(ThinkingEffort::High)
    )
    .await
    .is_err());
    assert!(
        create(&pool, owner, "Unknown model", "vendor/missing", None)
            .await
            .is_err()
    );
    let config = create(&pool, owner, "Valid", "openai/gpt-5", None)
        .await
        .unwrap();
    assert!(update(
        &pool,
        owner,
        config.id,
        "Invalid",
        "gpt-5",
        Some(ThinkingEffort::Low)
    )
    .await
    .is_err());
    assert_eq!(
        get(&pool, owner, config.id).await.unwrap(),
        Some(config.clone())
    );
    sqlx::query!("DELETE FROM model_selection_options WHERE handle = 'openai/gpt-5'")
        .execute(&pool)
        .await
        .unwrap();
    assert!(set_default(&pool, owner, Some(config.id)).await.is_err());
    assert!(update(&pool, owner, config.id, "Invalid", "gpt-5", None)
        .await
        .is_err());
    update(&pool, owner, config.id, "Repaired", "openai/gpt-4.1", None)
        .await
        .unwrap();
}

#[sqlx::test(migrations = "../../migrations")]
async fn bindings_restrict_deletion_and_database_rejects_cross_bear_references(pool: PgPool) {
    let owner = bear(&pool, "binding-owner").await;
    let other = bear(&pool, "binding-other").await;
    let hat_id = hat(&pool, owner).await;
    let config = create(&pool, owner, "Shared", "openai/gpt-4.1", None)
        .await
        .unwrap();
    let foreign = create(&pool, other, "Foreign", "openai/gpt-5", None)
        .await
        .unwrap();
    assert_eq!(default_configuration_id(&pool, owner).await.unwrap(), None);
    assert_eq!(
        hat_configuration_id(&pool, owner, hat_id).await.unwrap(),
        None
    );
    assert!(set_default(&pool, owner, Some(foreign.id)).await.is_err());
    assert!(set_hat_override(&pool, owner, hat_id, Some(foreign.id))
        .await
        .is_err());
    assert!(set_hat_override(&pool, other, hat_id, None).await.is_err());
    assert!(hat_configuration_id(&pool, other, hat_id).await.is_err());
    assert!(sqlx::query!(
        "UPDATE bears SET default_model_configuration_id = $2 WHERE id = $1",
        owner.as_uuid(),
        foreign.id.as_uuid(),
    )
    .execute(&pool)
    .await
    .is_err());
    assert!(sqlx::query!(
        "UPDATE bear_hats SET model_configuration_id = $2 WHERE id = $1",
        hat_id.as_uuid(),
        foreign.id.as_uuid(),
    )
    .execute(&pool)
    .await
    .is_err());
    set_default(&pool, owner, Some(config.id)).await.unwrap();
    assert!(matches!(
        delete(&pool, owner, config.id).await,
        Err(DenError::ValidationError(_))
    ));
    set_hat_override(&pool, owner, hat_id, Some(config.id))
        .await
        .unwrap();
    set_default(&pool, owner, None).await.unwrap();
    assert_eq!(default_configuration_id(&pool, owner).await.unwrap(), None);
    assert_eq!(
        hat_configuration_id(&pool, owner, hat_id).await.unwrap(),
        Some(config.id)
    );
    assert!(delete(&pool, owner, config.id).await.is_err());
    set_hat_override(&pool, owner, hat_id, None).await.unwrap();
    delete(&pool, owner, config.id).await.unwrap();
    assert!(default_configuration_id(&pool, BearId::new(Uuid::new_v4()))
        .await
        .is_err());
    assert!(set_default(&pool, BearId::new(Uuid::new_v4()), None)
        .await
        .is_err());
    assert!(set_default(
        &pool,
        owner,
        Some(ModelConfigurationId::new(Uuid::new_v4()))
    )
    .await
    .is_err());
}

#[sqlx::test(migrations = "../../migrations")]
async fn deleting_a_bear_cascades_its_own_configurations_and_hats(pool: PgPool) {
    let owner = bear(&pool, "cascade-owner").await;
    let hat_id = hat(&pool, owner).await;
    let config = create(&pool, owner, "Bound", "openai/gpt-5", None)
        .await
        .unwrap();
    set_default(&pool, owner, Some(config.id)).await.unwrap();
    set_hat_override(&pool, owner, hat_id, Some(config.id))
        .await
        .unwrap();
    db::delete_bear(&pool, owner.as_uuid()).await.unwrap();
    assert_eq!(get(&pool, owner, config.id).await.unwrap(), None);
}
