use super::*;

#[sqlx::test(migrations = "../../migrations")]
async fn bear_params_model_only_edits_preserve_named_configs_and_hat_bindings(pool: PgPool) {
    let owner: BearId = db::create_bear(&pool, bear_params("legacy-params", Some("gpt-5")))
        .await
        .unwrap()
        .into();
    let id = default_configuration_id(&pool, owner)
        .await
        .unwrap()
        .unwrap();
    let configured = update(
        &pool,
        owner,
        id,
        "Default",
        "gpt-5",
        Some(ThinkingEffort::High),
    )
    .await
    .unwrap();
    let hat_id = hat(&pool, owner).await;
    set_hat_override(&pool, owner, hat_id, Some(id))
        .await
        .unwrap();
    db::update_bear(
        &pool,
        owner.as_uuid(),
        bear_params("legacy-params", Some("gpt-5")),
    )
    .await
    .unwrap();
    assert_eq!(
        default_configuration_id(&pool, owner).await.unwrap(),
        Some(id)
    );
    assert_eq!(
        get(&pool, owner, id).await.unwrap(),
        Some(configured.clone())
    );

    db::update_bear(
        &pool,
        owner.as_uuid(),
        bear_params("legacy-params", Some("openai/gpt-4.1")),
    )
    .await
    .unwrap();
    let changed_id = default_configuration_id(&pool, owner)
        .await
        .unwrap()
        .unwrap();
    assert_ne!(changed_id, id);
    let changed = get(&pool, owner, changed_id).await.unwrap().unwrap();
    assert!(changed.name.starts_with("Legacy default ("));
    assert_eq!(changed.thinking_effort, None);
    assert_eq!(changed.model_handle.as_str(), "openai/gpt-4.1");
    assert_eq!(get(&pool, owner, id).await.unwrap(), Some(configured));
    assert_eq!(
        hat_configuration_id(&pool, owner, hat_id).await.unwrap(),
        Some(id)
    );
    let hat_model = resolve_primary(&pool, owner, Some(hat_id), None, "gpt-5")
        .await
        .unwrap();
    assert_eq!(hat_model.configuration_id, Some(id));
    assert_eq!(hat_model.thinking_effort, Some(ThinkingEffort::High));

    db::update_bear(
        &pool,
        owner.as_uuid(),
        bear_params("legacy-params", Some("  ")),
    )
    .await
    .unwrap();
    assert_eq!(default_configuration_id(&pool, owner).await.unwrap(), None);
    assert_eq!(
        db::get_bear(&pool, owner.as_uuid())
            .await
            .unwrap()
            .unwrap()
            .default_model,
        None
    );
    assert!(get(&pool, owner, changed_id).await.unwrap().is_some());
    assert_eq!(
        hat_configuration_id(&pool, owner, hat_id).await.unwrap(),
        Some(id)
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn legacy_sql_updates_route_without_mutating_named_configs_or_erasing_effort(pool: PgPool) {
    let owner = bear(&pool, "legacy-sql-owner").await;
    let configured = create(&pool, owner, "Default", "gpt-5", Some(ThinkingEffort::High))
        .await
        .unwrap();
    let hat_id = hat(&pool, owner).await;
    set_default(&pool, owner, Some(configured.id))
        .await
        .unwrap();
    set_hat_override(&pool, owner, hat_id, Some(configured.id))
        .await
        .unwrap();
    // Matches the current serving binary's unrelated Bear UPDATE.
    sqlx::query!(
        "UPDATE bears SET name = 'Edited Bear', default_model = default_model WHERE id = $1",
        owner.as_uuid(),
    )
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(
        default_configuration_id(&pool, owner).await.unwrap(),
        Some(configured.id)
    );
    assert_eq!(
        get(&pool, owner, configured.id).await.unwrap(),
        Some(configured.clone())
    );
    sqlx::query!(
        "UPDATE bears SET default_model = ' gpt-5 ' WHERE id = $1",
        owner.as_uuid()
    )
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(
        default_configuration_id(&pool, owner).await.unwrap(),
        Some(configured.id)
    );
    assert_eq!(list(&pool, owner).await.unwrap().len(), 1);

    sqlx::query!(
        "UPDATE bears SET default_model = 'openai/gpt-4.1' WHERE id = $1",
        owner.as_uuid()
    )
    .execute(&pool)
    .await
    .unwrap();
    let alternate_id = default_configuration_id(&pool, owner)
        .await
        .unwrap()
        .unwrap();
    assert_ne!(alternate_id, configured.id);
    let alternate = get(&pool, owner, alternate_id).await.unwrap().unwrap();
    assert_eq!(alternate.model_handle.as_str(), "openai/gpt-4.1");
    assert_eq!(alternate.thinking_effort, None);
    assert_eq!(
        get(&pool, owner, configured.id).await.unwrap(),
        Some(configured.clone())
    );
    assert_eq!(
        hat_configuration_id(&pool, owner, hat_id).await.unwrap(),
        Some(configured.id)
    );
    assert_eq!(
        resolve_primary(&pool, owner, Some(hat_id), None, "gpt-5")
            .await
            .unwrap()
            .thinking_effort,
        Some(ThinkingEffort::High)
    );

    // Returning to the old raw model is model-only intent, not permission to
    // select a named effort-bearing configuration or mutate it to remove effort.
    sqlx::query!(
        "UPDATE bears SET default_model = 'openai:gpt-5' WHERE id = $1",
        owner.as_uuid()
    )
    .execute(&pool)
    .await
    .unwrap();
    let model_only_id = default_configuration_id(&pool, owner)
        .await
        .unwrap()
        .unwrap();
    assert_ne!(model_only_id, configured.id);
    assert_eq!(
        get(&pool, owner, model_only_id)
            .await
            .unwrap()
            .unwrap()
            .thinking_effort,
        None
    );
    assert_eq!(
        get(&pool, owner, configured.id).await.unwrap(),
        Some(configured.clone())
    );
    sqlx::query!(
        "UPDATE bears SET default_model = 'openai/gpt-4.1' WHERE id = $1",
        owner.as_uuid()
    )
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(
        default_configuration_id(&pool, owner).await.unwrap(),
        Some(alternate_id)
    );
    assert_eq!(
        list(&pool, owner).await.unwrap().len(),
        3,
        "reuse an effort-free alternative"
    );
    sqlx::query!(
        "UPDATE bears SET default_model = NULL WHERE id = $1",
        owner.as_uuid()
    )
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(default_configuration_id(&pool, owner).await.unwrap(), None);
    assert_eq!(
        hat_configuration_id(&pool, owner, hat_id).await.unwrap(),
        Some(configured.id)
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn legacy_sql_inserts_create_the_canonical_default_after_the_owner_exists(pool: PgPool) {
    let id = sqlx::query_scalar!(
        "INSERT INTO bears (slug, default_model) VALUES ('legacy-insert', ' gpt-5 ') RETURNING id"
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let owner = BearId::new(id);
    let configurations = list(&pool, owner).await.unwrap();
    assert_eq!(configurations.len(), 1);
    assert_eq!(configurations[0].name, "Default");
    assert_eq!(configurations[0].model_handle.as_str(), "openai/gpt-5");
    assert_eq!(configurations[0].thinking_effort, None);
    assert_eq!(
        default_configuration_id(&pool, owner).await.unwrap(),
        Some(configurations[0].id)
    );
    assert_eq!(
        db::get_bear(&pool, id)
            .await
            .unwrap()
            .unwrap()
            .default_model
            .as_deref(),
        Some("openai/gpt-5")
    );
    let blank = sqlx::query_scalar!(
        "INSERT INTO bears (slug, default_model) VALUES ('legacy-blank', $1) RETURNING id",
        " \t\n ",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        default_configuration_id(&pool, blank.into()).await.unwrap(),
        None
    );
    assert!(list(&pool, blank.into()).await.unwrap().is_empty());
    assert_eq!(
        db::get_bear(&pool, blank)
            .await
            .unwrap()
            .unwrap()
            .default_model,
        None
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn modern_projection_refresh_is_not_misclassified_as_a_legacy_edit(pool: PgPool) {
    let owner = bear(&pool, "projection-owner").await;
    let config = create(
        &pool,
        owner,
        "Selected",
        "gpt-5",
        Some(ThinkingEffort::High),
    )
    .await
    .unwrap();
    set_default(&pool, owner, Some(config.id)).await.unwrap();
    update(
        &pool,
        owner,
        config.id,
        "Selected",
        "openai/gpt-5-mini",
        Some(ThinkingEffort::Medium),
    )
    .await
    .unwrap();
    assert_eq!(
        db::get_bear(&pool, owner.as_uuid())
            .await
            .unwrap()
            .unwrap()
            .default_model
            .as_deref(),
        Some("openai/gpt-5-mini")
    );
    assert_eq!(
        default_configuration_id(&pool, owner).await.unwrap(),
        Some(config.id)
    );
    assert_eq!(
        list(&pool, owner).await.unwrap().len(),
        1,
        "refresh must not route a second config"
    );
    assert_eq!(
        resolve_primary(&pool, owner, None, None, "gpt-5")
            .await
            .unwrap()
            .thinking_effort,
        Some(ThinkingEffort::Medium)
    );
    set_default(&pool, owner, None).await.unwrap();
    assert_eq!(
        db::get_bear(&pool, owner.as_uuid())
            .await
            .unwrap()
            .unwrap()
            .default_model,
        None
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn canonical_binding_wins_over_a_simultaneously_supplied_legacy_value(pool: PgPool) {
    let owner = bear(&pool, "canonical-pointer").await;
    let config = create(&pool, owner, "Chosen", "gpt-5", Some(ThinkingEffort::High))
        .await
        .unwrap();
    sqlx::query!(
        "UPDATE bears SET default_model_configuration_id = $2, default_model = 'missing/model' WHERE id = $1",
        owner.as_uuid(), config.id.as_uuid(),
    ).execute(&pool).await.unwrap();
    assert_eq!(
        default_configuration_id(&pool, owner).await.unwrap(),
        Some(config.id)
    );
    assert_eq!(
        db::get_bear(&pool, owner.as_uuid())
            .await
            .unwrap()
            .unwrap()
            .default_model
            .as_deref(),
        Some("openai/gpt-5")
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn invalid_legacy_model_writes_roll_back_the_entire_bear_change(pool: PgPool) {
    assert!(
        db::create_bear(&pool, bear_params("invalid-create", Some("missing/model")))
            .await
            .is_err()
    );
    assert!(!db::bear_slug_exists(&pool, "invalid-create").await.unwrap());
    let owner = bear(&pool, "valid-params").await;
    assert!(db::update_bear(
        &pool,
        owner.as_uuid(),
        bear_params("invalid-update", Some("missing/model"))
    )
    .await
    .is_err());
    assert_eq!(
        db::get_bear(&pool, owner.as_uuid())
            .await
            .unwrap()
            .unwrap()
            .slug,
        "valid-params"
    );
    assert!(sqlx::query!(
        "UPDATE bears SET slug = 'invalid-sql-update', default_model = 'missing/model' WHERE id = $1",
        owner.as_uuid(),
    ).execute(&pool).await.is_err());
    assert_eq!(
        db::get_bear(&pool, owner.as_uuid())
            .await
            .unwrap()
            .unwrap()
            .slug,
        "valid-params"
    );
    assert!(sqlx::query!(
        "INSERT INTO bears (slug, default_model) VALUES ('invalid-sql-insert', 'missing/model')"
    )
    .execute(&pool)
    .await
    .is_err());
    assert!(!db::bear_slug_exists(&pool, "invalid-sql-insert")
        .await
        .unwrap());
    sqlx::query!(
        "UPDATE model_selection_options SET selectable = false WHERE handle = 'openai/gpt-4.1'"
    )
    .execute(&pool)
    .await
    .unwrap();
    assert!(sqlx::query!(
        "UPDATE bears SET default_model = 'openai/gpt-4.1' WHERE id = $1",
        owner.as_uuid(),
    )
    .execute(&pool)
    .await
    .is_err());
    sqlx::query!(
        "INSERT INTO model_selection_options (handle, display_name) VALUES ('vendor/*', 'Routing wildcard')"
    ).execute(&pool).await.unwrap();
    assert!(sqlx::query!(
        "UPDATE bears SET default_model = 'vendor/*' WHERE id = $1",
        owner.as_uuid(),
    )
    .execute(&pool)
    .await
    .is_err());
    assert_eq!(default_configuration_id(&pool, owner).await.unwrap(), None);
    assert!(list(&pool, owner).await.unwrap().is_empty());
}
