use super::*;

#[sqlx::test(migrations = "../../migrations")]
async fn precedence_is_whole_configuration_and_pin_omits_identity_and_effort(pool: PgPool) {
    let owner = bear(&pool, "resolution-owner").await;
    let hat_id = hat(&pool, owner).await;
    let default = create(&pool, owner, "Careful", "gpt-5", Some(ThinkingEffort::High))
        .await
        .unwrap();
    let quick = create(&pool, owner, "Quick", "openai/gpt-4.1", None)
        .await
        .unwrap();
    let deployment = resolve_primary(&pool, owner, Some(hat_id), None, "gpt-5")
        .await
        .unwrap();
    assert_eq!(deployment.source, PrimaryModelSource::DeploymentDefault);
    assert_eq!(deployment.configuration_id, None);
    assert_eq!(deployment.thinking_effort, None);
    assert_eq!(deployment.model_handle, "openai/gpt-5");
    set_default(&pool, owner, Some(default.id)).await.unwrap();
    let inherited = resolve_primary(&pool, owner, Some(hat_id), None, "missing/deployment")
        .await
        .unwrap();
    assert_eq!(inherited.source, PrimaryModelSource::BearDefault);
    assert_eq!(inherited.configuration_id, Some(default.id));
    assert_eq!(inherited.configuration_name.as_deref(), Some("Careful"));
    assert_eq!(inherited.thinking_effort, Some(ThinkingEffort::High));
    set_hat_override(&pool, owner, hat_id, Some(quick.id))
        .await
        .unwrap();
    let overridden = resolve_primary(&pool, owner, Some(hat_id), None, "gpt-5")
        .await
        .unwrap();
    assert_eq!(overridden.source, PrimaryModelSource::HatOverride);
    assert_eq!(overridden.configuration_id, Some(quick.id));
    assert_eq!(overridden.model_handle, "openai/gpt-4.1");
    assert_eq!(
        overridden.thinking_effort, None,
        "do not inherit Bear effort"
    );
    let pin = resolve_primary(&pool, owner, Some(hat_id), Some("gpt-5"), "gpt-5")
        .await
        .unwrap();
    assert_eq!(pin.source, PrimaryModelSource::ConversationPin);
    assert_eq!(pin.configuration_id, None);
    assert_eq!(pin.configuration_name, None);
    assert_eq!(pin.thinking_effort, None);
    assert_eq!(
        serde_json::to_value(pin).unwrap()["source"],
        "conversation_pin"
    );
    for (source, expected) in [
        (PrimaryModelSource::HatOverride, "hat_override"),
        (PrimaryModelSource::BearDefault, "bear_default"),
        (PrimaryModelSource::DeploymentDefault, "deployment_default"),
    ] {
        assert_eq!(serde_json::to_value(source).unwrap(), expected);
    }
    set_hat_override(&pool, owner, hat_id, None).await.unwrap();
    assert_eq!(
        resolve_primary(&pool, owner, Some(hat_id), None, "gpt-5")
            .await
            .unwrap()
            .configuration_id,
        Some(default.id)
    );
    set_default(&pool, owner, None).await.unwrap();
    assert_eq!(
        resolve_primary(&pool, owner, None, None, "openai/gpt-4.1")
            .await
            .unwrap()
            .source,
        PrimaryModelSource::DeploymentDefault
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn invalid_winning_selection_never_falls_back_but_higher_precedence_can_replace_it(
    pool: PgPool,
) {
    let owner = bear(&pool, "resolution-invalid").await;
    let hat_id = hat(&pool, owner).await;
    reasoning_support(&pool, Some(true)).await;
    let default = create(&pool, owner, "Careful", "gpt-5", Some(ThinkingEffort::High))
        .await
        .unwrap();
    let quick = create(&pool, owner, "Quick", "openai/gpt-4.1", None)
        .await
        .unwrap();
    set_default(&pool, owner, Some(default.id)).await.unwrap();
    reasoning_support(&pool, None).await;
    assert!(resolve_primary(&pool, owner, None, None, "openai/gpt-4.1")
        .await
        .is_err());
    reasoning_support(&pool, Some(false)).await;
    assert!(resolve_primary(&pool, owner, None, None, "openai/gpt-4.1")
        .await
        .is_err());
    // A pin does not consult lower-layer effort, including for the same model.
    assert_eq!(
        resolve_primary(&pool, owner, Some(hat_id), Some("gpt-5"), "openai/gpt-4.1")
            .await
            .unwrap()
            .thinking_effort,
        None
    );
    set_hat_override(&pool, owner, hat_id, Some(quick.id))
        .await
        .unwrap();
    sqlx::query!(
        "UPDATE model_selection_options SET selectable = false WHERE handle = 'openai/gpt-4.1'"
    )
    .execute(&pool)
    .await
    .unwrap();
    assert!(resolve_primary(&pool, owner, Some(hat_id), None, "gpt-5")
        .await
        .is_err());
    assert!(
        resolve_primary(&pool, owner, Some(hat_id), Some("openai/gpt-4.1"), "gpt-5")
            .await
            .is_err()
    );
    assert!(
        resolve_primary(&pool, owner, Some(hat_id), Some(""), "gpt-5")
            .await
            .is_err()
    );
    sqlx::query!("DELETE FROM model_selection_options WHERE handle = 'openai/gpt-4.1'")
        .execute(&pool)
        .await
        .unwrap();
    assert!(resolve_primary(&pool, owner, Some(hat_id), None, "gpt-5")
        .await
        .is_err());
    // Invalid configurations remain visible for repair.
    assert_eq!(get(&pool, owner, quick.id).await.unwrap(), Some(quick));
    set_hat_override(&pool, owner, hat_id, None).await.unwrap();
    set_default(&pool, owner, None).await.unwrap();
    assert!(
        resolve_primary(&pool, owner, None, None, "missing/deployment")
            .await
            .is_err()
    );
    assert!(
        resolve_primary(
            &pool,
            owner,
            Some(HatId::new(Uuid::new_v4())),
            None,
            "gpt-5"
        )
        .await
        .is_err(),
        "unknown hat is not inheritance"
    );
}

#[test]
fn configuration_ids_are_uuid_transparent_and_parse_like_hat_ids() {
    let uuid = Uuid::new_v4();
    let id: ModelConfigurationId = format!(" {uuid} ").parse().unwrap();
    assert_eq!(id.as_uuid(), uuid);
    assert_eq!(Uuid::from(id), uuid);
    assert_eq!(serde_json::to_value(id).unwrap(), uuid.to_string());
    assert_eq!(
        serde_json::from_value::<ModelConfigurationId>(serde_json::to_value(id).unwrap()).unwrap(),
        id
    );
}
