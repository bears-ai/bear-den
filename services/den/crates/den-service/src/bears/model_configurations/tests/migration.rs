use super::*;

#[sqlx::test(migrations = "../../migrations")]
async fn migration_down_up_backfills_historical_defaults_and_preserves_raw_rollback_model(
    pool: PgPool,
) {
    let owner = bear(&pool, "migration-selected").await;
    let config = create(&pool, owner, "Chosen", "openai/gpt-4.1", None)
        .await
        .unwrap();
    set_default(&pool, owner, Some(config.id)).await.unwrap();
    let migrator = sqlx::migrate!("../../migrations");
    // SQLx owns execution of the real reversible scripts on this isolated
    // sqlx::test database; no hand-written runtime SQL migration substitute.
    migrator.undo(&pool, 20261007165612).await.unwrap();
    assert_eq!(
        db::get_bear(&pool, owner.as_uuid())
            .await
            .unwrap()
            .unwrap()
            .default_model
            .as_deref(),
        Some("openai/gpt-4.1")
    );
    let historical = sqlx::query_scalar!(
        "INSERT INTO bears (slug, default_model) VALUES ('historical-model', ' gpt-5 ') RETURNING id"
    ).fetch_one(&pool).await.unwrap();
    let removed = sqlx::query_scalar!(
        "INSERT INTO bears (slug, default_model) VALUES ('historical-removed', 'vendor/removed') RETURNING id"
    ).fetch_one(&pool).await.unwrap();
    let blank = sqlx::query_scalar!(
        "INSERT INTO bears (slug, default_model) VALUES ('historical-blank', $1) RETURNING id",
        " \t\n ",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let inherited = sqlx::query_scalar!(
        "INSERT INTO bears (slug) VALUES ('historical-inherited') RETURNING id"
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    migrator.run(&pool).await.unwrap();
    let configurations = list(&pool, historical.into()).await.unwrap();
    assert_eq!(configurations.len(), 1);
    assert_eq!(configurations[0].name, "Default");
    assert_eq!(configurations[0].model_handle.as_str(), "gpt-5");
    assert_eq!(configurations[0].thinking_effort, None);
    assert_eq!(
        default_configuration_id(&pool, historical.into())
            .await
            .unwrap(),
        Some(configurations[0].id)
    );
    assert_eq!(
        resolve_primary(&pool, historical.into(), None, None, "openai/gpt-4.1")
            .await
            .unwrap()
            .model_handle,
        "openai/gpt-5"
    );
    assert_eq!(
        list(&pool, removed.into()).await.unwrap()[0]
            .model_handle
            .as_str(),
        "vendor/removed"
    );
    assert!(
        resolve_primary(&pool, removed.into(), None, None, "openai/gpt-4.1")
            .await
            .is_err(),
        "migration retains removed defaults for repair, never silently replaces them"
    );
    for id in [blank, inherited] {
        assert_eq!(
            default_configuration_id(&pool, id.into()).await.unwrap(),
            None
        );
        assert!(list(&pool, id.into()).await.unwrap().is_empty());
        assert_eq!(
            db::get_bear(&pool, id)
                .await
                .unwrap()
                .unwrap()
                .default_model,
            None
        );
    }
    // Down/up is intentionally lossy for names/effort, but retains the chosen raw model.
    let rebuilt = list(&pool, owner).await.unwrap();
    assert_eq!(rebuilt.len(), 1);
    assert_eq!(rebuilt[0].name, "Default");
    assert_eq!(rebuilt[0].model_handle.as_str(), "openai/gpt-4.1");
}
