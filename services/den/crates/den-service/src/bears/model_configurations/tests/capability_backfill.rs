use super::*;

#[sqlx::test(migrations = "../../migrations")]
async fn seeded_capabilities_support_configuration_writes_and_execution_without_test_patches(
    pool: PgPool,
) {
    let owner = bear(&pool, "seeded-capabilities").await;
    let supported = [
        "openai/gpt-5.5",
        "openai/gpt-5.1",
        "openai/gpt-5",
        "openai/gpt-5-mini",
        "openai/gpt-5-nano",
        "openai/o4-mini",
        "openai/o3",
        "openai/o3-mini",
        "openai/o1",
    ];
    let unsupported = [
        "openai/gpt-4.1",
        "openai/gpt-4.1-mini",
        "openai/gpt-4.1-nano",
        "openai/gpt-4o",
        "openai/gpt-4o-mini",
        "openai/o1-mini",
    ];
    assert_eq!(
        supported.len() + unsupported.len(),
        den_llm::model_registry::registry_entries().len()
    );
    for model in supported {
        let capability = validate_model_configuration(&pool, model, Some(ThinkingEffort::High))
            .await
            .unwrap();
        assert_eq!(capability.supports_reasoning_effort, Some(true));
        let config = create(&pool, owner, model, model, Some(ThinkingEffort::High))
            .await
            .unwrap();
        set_default(&pool, owner, Some(config.id)).await.unwrap();
        let resolved = resolve_primary(&pool, owner, None, None, "missing/deployment")
            .await
            .unwrap();
        assert_eq!(resolved.configuration_id, Some(config.id));
        assert_eq!(resolved.thinking_effort, Some(ThinkingEffort::High));
    }
    for model in unsupported {
        let capability = validate_model_configuration(&pool, model, None)
            .await
            .unwrap();
        assert_eq!(capability.supports_reasoning_effort, Some(false));
        assert!(
            create(&pool, owner, model, model, Some(ThinkingEffort::Low))
                .await
                .is_err()
        );
    }
    sqlx::query!(
        "INSERT INTO model_selection_options (handle, display_name) VALUES ('openai/future-unknown', 'Unknown capability')"
    ).execute(&pool).await.unwrap();
    assert_eq!(
        validate_model_configuration(&pool, "openai/future-unknown", None)
            .await
            .unwrap()
            .supports_reasoning_effort,
        None
    );
    assert!(
        create(
            &pool,
            owner,
            "Unknown",
            "openai/future-unknown",
            Some(ThinkingEffort::High)
        )
        .await
        .is_err(),
        "no provider or model-name inference for unseeded entries"
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn capability_backfill_preserves_existing_live_flags_and_unrelated_metadata(pool: PgPool) {
    let migrator = sqlx::migrate!("../../migrations");
    migrator.undo(&pool, 20261007165612).await.unwrap();
    // Reconstruct the old seed's missing flags, with examples of newer live or
    // operator overrides. None of these fields may be erased by bootstrap.
    sqlx::query!(
        "UPDATE model_selection_options SET metadata_json = metadata_json - 'supports_reasoning_effort'"
    ).execute(&pool).await.unwrap();
    sqlx::query!(
        r#"UPDATE model_selection_options
           SET metadata_json = metadata_json || '{"supports_reasoning_effort":false,"operator_note":"preserve"}'::jsonb
           WHERE handle = 'openai/gpt-5'"#
    ).execute(&pool).await.unwrap();
    sqlx::query!(
        r#"UPDATE model_selection_options
           SET metadata_json = metadata_json || '{"supports_reasoning_effort":null}'::jsonb
           WHERE handle = 'openai/gpt-5.1'"#
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query!(
        r#"UPDATE model_selection_options
           SET metadata_json = metadata_json || '{"supports_reasoning_effort":true}'::jsonb
           WHERE handle = 'openai/gpt-4o'"#
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query!(
        "INSERT INTO model_selection_options (handle, display_name) VALUES ('vendor/unseeded', 'Unseeded')"
    ).execute(&pool).await.unwrap();
    migrator.run(&pool).await.unwrap();
    for (model, expected) in [
        ("openai/gpt-5", Some(false)),
        ("openai/gpt-5.1", None),
        ("openai/gpt-4o", Some(true)),
        ("openai/gpt-5.5", Some(true)),
        ("openai/gpt-4.1", Some(false)),
        ("vendor/unseeded", None),
    ] {
        assert_eq!(
            validate_model_configuration(&pool, model, None)
                .await
                .unwrap()
                .supports_reasoning_effort,
            expected,
            "{model}"
        );
    }
    let preserved = sqlx::query!(
        r#"SELECT metadata_json->>'operator_note' AS "note!",
                  (metadata_json->>'context_window')::bigint AS "context_window!"
           FROM model_selection_options WHERE handle = 'openai/gpt-5'"#
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(preserved.note, "preserve");
    assert_eq!(preserved.context_window, 400_000);
    // Downgrade retains the additive catalog enrichment, including all later
    // live/importer/operator edits; it only removes the new schema and bridge.
    migrator.undo(&pool, 20261007165612).await.unwrap();
    assert_eq!(
        validate_model_configuration(&pool, "openai/gpt-5", None)
            .await
            .unwrap()
            .supports_reasoning_effort,
        Some(false)
    );
    assert_eq!(
        validate_model_configuration(&pool, "openai/gpt-5.5", None)
            .await
            .unwrap()
            .supports_reasoning_effort,
        Some(true)
    );
    migrator.run(&pool).await.unwrap();
}
