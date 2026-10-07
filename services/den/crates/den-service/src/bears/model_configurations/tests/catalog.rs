use super::*;

#[test]
fn explicit_effort_requires_known_catalog_support() {
    for effort in [
        ThinkingEffort::Low,
        ThinkingEffort::Medium,
        ThinkingEffort::High,
    ] {
        assert!(validate_thinking_effort(Some(true), Some(effort)).is_ok());
        for support in [Some(false), None] {
            assert!(matches!(
                validate_thinking_effort(support, Some(effort)),
                Err(DenError::ValidationError(_))
            ));
        }
    }
    for support in [Some(true), Some(false), None] {
        assert!(validate_thinking_effort(support, None).is_ok());
    }
}

#[sqlx::test(migrations = "../../migrations")]
async fn strict_catalog_validation_normalizes_aliases_and_rejects_invalid_choices(pool: PgPool) {
    let validated = validate_model_configuration(&pool, " gpt-5 ", None)
        .await
        .unwrap();
    assert_eq!(validated.model_handle.as_str(), "openai/gpt-5");
    assert_eq!(validated.supports_reasoning_effort, Some(true));
    for model in ["", "   ", "openai/*", "missing/model", "missing/other"] {
        assert!(matches!(
            validate_model_configuration(&pool, model, None).await,
            Err(DenError::ValidationError(_))
        ));
    }
    assert!(
        validate_model_configuration(&pool, "gpt-5", Some(ThinkingEffort::High))
            .await
            .is_ok()
    );
    reasoning_support(&pool, None).await;
    assert!(
        validate_model_configuration(&pool, "gpt-5", Some(ThinkingEffort::High))
            .await
            .is_err()
    );
    reasoning_support(&pool, Some(false)).await;
    assert!(
        validate_model_configuration(&pool, "gpt-5", Some(ThinkingEffort::Low))
            .await
            .is_err()
    );
    reasoning_support(&pool, Some(true)).await;
    assert!(
        validate_model_configuration(&pool, "gpt-5", Some(ThinkingEffort::Medium))
            .await
            .is_ok()
    );

    sqlx::query!(
        "UPDATE model_selection_options SET selectable = false WHERE handle = 'openai/gpt-5'"
    )
    .execute(&pool)
    .await
    .unwrap();
    assert!(validate_model_configuration(&pool, "gpt-5", None)
        .await
        .is_err());
    sqlx::query!("DELETE FROM model_selection_options")
        .execute(&pool)
        .await
        .unwrap();
    assert!(
        validate_model_configuration(&pool, "openai/gpt-4.1", None)
            .await
            .is_err(),
        "the static registry must not resurrect an empty catalog"
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn arbitrary_catalog_models_work_without_static_metadata_guesses(pool: PgPool) {
    sqlx::query!(
        r#"INSERT INTO model_selection_options (handle, display_name, metadata_json)
           VALUES ('vendor/approved', 'Approved', '{"supports_reasoning_effort":true}'::jsonb)"#
    )
    .execute(&pool)
    .await
    .unwrap();
    let model = validate_model_configuration(&pool, "vendor/approved", Some(ThinkingEffort::High))
        .await
        .unwrap();
    assert_eq!(model.model_handle.as_str(), "vendor/approved");
    assert_eq!(model.supports_reasoning_effort, Some(true));
    // Malformed metadata must not be treated as a truthy capability.
    sqlx::query!(
        r#"UPDATE model_selection_options SET metadata_json = '{"supports_reasoning_effort":"yes"}'::jsonb
           WHERE handle = 'vendor/approved'"#
    )
    .execute(&pool).await.unwrap();
    assert!(
        validate_model_configuration(&pool, "vendor/approved", Some(ThinkingEffort::High))
            .await
            .is_err()
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn catalog_database_errors_are_not_static_fallbacks(pool: PgPool) {
    pool.close().await;
    assert!(matches!(
        validate_model_configuration(&pool, "openai/gpt-5", None).await,
        Err(DenError::DatabaseUnavailable(_))
    ));
}
