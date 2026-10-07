use super::access_tests::{app_with_runtime, conversation, login, request, seed};
use super::*;
use den_service::bears::model_configurations as models;
use sqlx::PgPool;

async fn get_model(
    app: &Router,
    cookie: &str,
    bear: Uuid,
    external_id: &str,
) -> (StatusCode, Value) {
    request(
        app,
        cookie,
        "GET",
        &format!("/v1/chat/model?bear_id={bear}&conversation_id={external_id}"),
        Value::Null,
    )
    .await
}

#[sqlx::test(migrations = "../../migrations")]
async fn chat_model_uses_current_hat_configuration_and_only_explicit_pins(pool: PgPool) {
    let (bear, [owner, _, admin]) = seed(&pool).await;
    let bear_id = BearId::new(bear);
    let default = models::create(
        &pool,
        bear_id,
        "Deep",
        "openai/gpt-5",
        Some(ThinkingEffort::High),
    )
    .await
    .unwrap();
    let override_config = models::create(
        &pool,
        bear_id,
        "Quick",
        "openai/gpt-5-mini",
        Some(ThinkingEffort::Medium),
    )
    .await
    .unwrap();
    models::set_default(&pool, bear_id, Some(default.id))
        .await
        .unwrap();
    let hat = hats::create_hat(
        &pool,
        bear_id,
        UserId::new(admin),
        "Chat hat",
        "Chat responsibility",
    )
    .await
    .unwrap();
    let external_id = "conv-model-configuration";
    let canonical = conversation(&pool, bear, Some(owner), external_id).await;
    hats::bindings::bind_conversation_hat(&pool, bear_id, canonical, hat.id)
        .await
        .unwrap();
    conversation_persistence::establish_conversation_default_model_state(
        &pool,
        canonical,
        "missing/stale-auto-cache",
        "legacy_auto_cache",
    )
    .await
    .unwrap();
    let app = app_with_runtime(
        &pool,
        crate::web_chat_runtime::unavailable_web_chat_runtime(),
    )
    .await;
    let cookie = login(&app, owner).await;

    let (status, response) = get_model(&app, &cookie, bear, external_id).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(response["source"], "bear_default");
    assert_eq!(response["effective_model"], "openai/gpt-5");
    assert_eq!(response["configuration_name"], "Deep");
    assert_eq!(response["thinking_effort"], "high");
    assert_eq!(response["configuration_id"], json!(default.id));
    assert!(response["requested_model"].is_null());
    assert!(response["selected_model"].is_null());
    assert_eq!(response["selection_mode"], "auto");

    models::set_hat_override(&pool, bear_id, hat.id, Some(override_config.id))
        .await
        .unwrap();
    let (status, response) = get_model(&app, &cookie, bear, external_id).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(response["source"], "hat_override");
    assert_eq!(response["configuration_name"], "Quick");
    assert_eq!(response["effective_model"], "openai/gpt-5-mini");
    assert_eq!(response["thinking_effort"], "medium");
    assert_eq!(response["configuration_id"], json!(override_config.id));
    assert!(response["selected_model"].is_null());
    models::update(
        &pool,
        bear_id,
        override_config.id,
        "Quick",
        "openai/gpt-5-nano",
        None,
    )
    .await
    .unwrap();
    let (_, response) = get_model(&app, &cookie, bear, external_id).await;
    assert_eq!(response["effective_model"], "openai/gpt-5-nano");
    assert!(response["thinking_effort"].is_null());

    let (status, response) = request(&app, &cookie, "PATCH", "/v1/chat/model", json!({"bear_id": bear, "conversation_id": external_id, "selection_mode": "explicit", "model": "openai/gpt-4.1"})).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(response["source"], "conversation_explicit");
    assert_eq!(response["selection_mode"], "explicit");
    assert_eq!(response["effective_model"], "openai/gpt-4.1");
    assert_eq!(response["selected_model"], "openai/gpt-4.1");
    assert!(response["configuration_id"].is_null());
    assert!(response["configuration_name"].is_null());
    assert!(response["thinking_effort"].is_null());
    let (status, response) = request(
        &app,
        &cookie,
        "PATCH",
        "/v1/chat/model",
        json!({"bear_id": bear, "conversation_id": external_id, "selection_mode": "auto"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(response["source"], "hat_override");
    assert_eq!(response["effective_model"], "openai/gpt-5-nano");
    assert!(response["thinking_effort"].is_null());

    models::set_hat_override(&pool, bear_id, hat.id, None)
        .await
        .unwrap();
    let (_, response) = get_model(&app, &cookie, bear, external_id).await;
    assert_eq!(response["source"], "bear_default");
    assert_eq!(response["thinking_effort"], "high");
    let (status, preview) = get_model(&app, &cookie, bear, "new-preview").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(preview["source"], "bear_default");
    assert_eq!(preview["configuration_id"], json!(default.id));
    assert!(
        conversation_persistence::get_conversation_for_external_id(&pool, bear, "new-preview")
            .await
            .unwrap()
            .is_none()
    );
    models::set_default(&pool, bear_id, None).await.unwrap();
    for id in [external_id, "new-preview"] {
        let (status, response) = get_model(&app, &cookie, bear, id).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(response["source"], "deployment_default");
        assert_eq!(response["effective_model"], "openai/gpt-4.1");
        assert!(response["thinking_effort"].is_null());
        assert!(response["configuration_id"].is_null());
    }
}

#[sqlx::test(migrations = "../../migrations")]
async fn chat_model_keeps_revoked_choices_manageable_without_resolving_them(pool: PgPool) {
    let (bear, [owner, other_member, admin]) = seed(&pool).await;
    let bear_id = BearId::new(bear);
    let configuration = models::create(
        &pool,
        bear_id,
        "Revoked",
        "openai/gpt-5",
        Some(ThinkingEffort::High),
    )
    .await
    .unwrap();
    models::set_default(&pool, bear_id, Some(configuration.id))
        .await
        .unwrap();
    let hat = hats::create_hat(
        &pool,
        bear_id,
        UserId::new(admin),
        "Chat",
        "Chat responsibility",
    )
    .await
    .unwrap();
    let external_id = "conv-revoked-model";
    let canonical = conversation(&pool, bear, Some(owner), external_id).await;
    hats::bindings::bind_conversation_hat(&pool, bear_id, canonical, hat.id)
        .await
        .unwrap();
    let app = app_with_runtime(
        &pool,
        crate::web_chat_runtime::unavailable_web_chat_runtime(),
    )
    .await;
    let cookie = login(&app, owner).await;
    let model = "openai/gpt-5";
    sqlx::query!(
        "UPDATE model_selection_options SET selectable = FALSE WHERE handle = $1",
        model
    )
    .execute(&pool)
    .await
    .unwrap();
    for id in [external_id, "new-preview"] {
        let (status, response) = get_model(&app, &cookie, bear, id).await;
        assert_eq!(status, StatusCode::OK);
        assert!(response["effective_model"].is_null());
        assert!(response["error"]
            .as_str()
            .unwrap()
            .contains("no longer selectable"));
        assert_eq!(response["selection_mode"], "auto");
        assert!(response["selected_model"].is_null());
        assert!(response["requested_model"].is_null());
        let options = response["model_options"].as_array().unwrap();
        assert!(options
            .iter()
            .any(|option| option["handle"] == "openai/gpt-4.1"));
        assert!(!options.iter().any(|option| option["handle"] == model));
    }
    assert!(matches!(
        den_service::model_selection::resolve_conversation_primary_model(
            &pool,
            bear_id,
            canonical,
            "openai/gpt-4.1"
        )
        .await,
        Err(DenError::ValidationError(_)),
    ));
    // A pin replaces the whole configuration, so a revoked inherited choice
    // does not block an independently valid explicit model.
    conversation_persistence::set_conversation_model_state(
        &pool,
        canonical,
        "explicit",
        Some("openai/gpt-4.1"),
        Some("openai/gpt-4.1"),
        None,
    )
    .await
    .unwrap();
    let (status, response) = get_model(&app, &cookie, bear, external_id).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(response["source"], "conversation_explicit");
    assert!(response["thinking_effort"].is_null());
    for pin in [None, Some("missing/pin"), Some("openai/gpt-5")] {
        conversation_persistence::set_conversation_model_state(
            &pool, canonical, "explicit", pin, pin, None,
        )
        .await
        .unwrap();
        let (status, response) = get_model(&app, &cookie, bear, external_id).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(response["selection_mode"], "explicit");
        assert_eq!(response["selected_model"], json!(pin));
        assert_eq!(response["requested_model"], json!(pin));
        assert_eq!(response["source"], "conversation_explicit");
        assert!(response["effective_model"].is_null());
        assert!(response["error"].is_string());
        assert!(!response["model_options"].as_array().unwrap().is_empty());
        assert!(matches!(
            den_service::model_selection::resolve_conversation_primary_model(
                &pool,
                bear_id,
                canonical,
                "openai/gpt-4.1"
            )
            .await,
            Err(DenError::ValidationError(_)),
        ));
        let (status, cleared) = request(
            &app,
            &cookie,
            "PATCH",
            "/v1/chat/model",
            json!({"bear_id": bear, "conversation_id": external_id, "selection_mode": "auto"}),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(cleared["selection_mode"], "auto");
        assert!(cleared["selected_model"].is_null());
        assert!(cleared["effective_model"].is_null());
        assert!(cleared["error"].is_string());
        assert_eq!(
            den_service::model_selection::conversation_model_pin(&pool, canonical)
                .await
                .unwrap(),
            None
        );
        let (status, repaired) = request(&app, &cookie, "PATCH", "/v1/chat/model", json!({"bear_id": bear, "conversation_id": external_id, "selection_mode": "explicit", "model": "openai/gpt-4.1"})).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(repaired["effective_model"], "openai/gpt-4.1");
        assert!(repaired["error"].is_null());
    }
    conversation_persistence::set_conversation_model_state(
        &pool,
        canonical,
        "explicit",
        Some(model),
        Some(model),
        None,
    )
    .await
    .unwrap();
    assert!(get_model(&app, &cookie, bear, external_id).await.1["error"].is_string());
    let (status, replacement) = request(&app, &cookie, "PATCH", "/v1/chat/model", json!({"bear_id": bear, "conversation_id": external_id, "selection_mode": "explicit", "model": "openai/gpt-4.1"})).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(replacement["selected_model"], "openai/gpt-4.1");
    assert!(replacement["error"].is_null());
    let other_cookie = login(&app, other_member).await;
    assert_eq!(
        get_model(&app, &other_cookie, bear, external_id).await.0,
        StatusCode::FORBIDDEN
    );
    let (status, _) = request(&app, &cookie, "PATCH", "/v1/chat/model", json!({"bear_id": bear, "conversation_id": external_id, "selection_mode": "explicit", "model": model})).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(
        den_service::model_selection::conversation_model_pin(&pool, canonical)
            .await
            .unwrap()
            .as_deref(),
        Some("openai/gpt-4.1")
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn chat_model_options_and_writes_never_use_static_fallback_as_authority(pool: PgPool) {
    let (bear, [owner, _, _]) = seed(&pool).await;
    let external_id = "conv-empty-catalog";
    let canonical = super::access_tests::bound_conversation(&pool, bear, owner, external_id).await;
    for option in den_service::model_selection::list_selectable_model_options(&pool)
        .await
        .unwrap()
    {
        let model = option.handle;
        sqlx::query!(
            "UPDATE model_selection_options SET selectable = FALSE WHERE handle = $1",
            model
        )
        .execute(&pool)
        .await
        .unwrap();
    }
    let app = app_with_runtime(
        &pool,
        crate::web_chat_runtime::unavailable_web_chat_runtime(),
    )
    .await;
    let cookie = login(&app, owner).await;
    let (status, response) = get_model(&app, &cookie, bear, external_id).await;
    assert_eq!(status, StatusCode::OK);
    assert!(response["model_options"].as_array().unwrap().is_empty());
    assert!(response["effective_model"].is_null());
    assert!(response["error"].is_string());
    let (status, _) = request(&app, &cookie, "PATCH", "/v1/chat/model", json!({"bear_id": bear, "conversation_id": external_id, "selection_mode": "explicit", "model": "openai/gpt-4.1"})).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(
        den_service::model_selection::conversation_model_pin(&pool, canonical)
            .await
            .unwrap(),
        None
    );
    let (status, response) = request(
        &app,
        &cookie,
        "PATCH",
        "/v1/chat/model",
        json!({"bear_id": bear, "conversation_id": external_id, "selection_mode": "auto"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(response["error"].is_string());
}

fn stored_model_state(mode: &str) -> conversation_persistence::ConversationModelState {
    conversation_persistence::ConversationModelState {
        conversation_id: Uuid::new_v4(),
        selection_mode: mode.into(),
        requested_model: Some("requested/pin".into()),
        selected_model: Some("selected/pin".into()),
        selected_reason: None,
        actual_last_model: None,
        actual_last_provider: None,
        fallback_count: 0,
        metadata_json: json!({}),
    }
}

#[test]
fn unavailable_model_response_preserves_only_explicit_pins_and_not_authorization_errors() {
    for mode in ["auto", "explicit"] {
        let response = ChatModelResponse::from_resolution(
            Err(DenError::ValidationError(
                "model is no longer selectable".into(),
            )),
            Some(stored_model_state(mode)),
            vec![ModelOption {
                handle: "valid/model".into(),
                label: "Valid".into(),
                context_window: None,
                max_output_tokens: None,
            }],
        )
        .unwrap();
        let response = serde_json::to_value(response).unwrap();
        assert_eq!(response["selection_mode"], mode);
        assert_eq!(
            response["requested_model"],
            if mode == "explicit" {
                json!("requested/pin")
            } else {
                Value::Null
            }
        );
        assert_eq!(
            response["selected_model"],
            if mode == "explicit" {
                json!("selected/pin")
            } else {
                Value::Null
            }
        );
        assert!(response["effective_model"].is_null());
        assert_eq!(
            response["source"],
            if mode == "explicit" {
                json!("conversation_explicit")
            } else {
                Value::Null
            }
        );
        assert!(response["thinking_effort"].is_null());
        assert!(response["configuration_id"].is_null());
        assert_eq!(response["error"], "model is no longer selectable");
        assert_eq!(response["model_options"][0]["handle"], "valid/model");
    }
    assert!(matches!(
        ChatModelResponse::from_resolution(
            Err(DenError::Authorization("not authorized".into())),
            Some(stored_model_state("explicit")),
            vec![],
        ),
        Err(CustomError::Authorization(_))
    ));
}

#[test]
fn chat_model_response_reports_each_resolved_source_and_projects_pins_only() {
    for (source, label) in [
        (PrimaryModelSource::ConversationPin, "conversation_explicit"),
        (PrimaryModelSource::HatOverride, "hat_override"),
        (PrimaryModelSource::BearDefault, "bear_default"),
        (PrimaryModelSource::DeploymentDefault, "deployment_default"),
    ] {
        let configured = matches!(
            source,
            PrimaryModelSource::HatOverride | PrimaryModelSource::BearDefault
        );
        let configuration_id = configured.then(|| ModelConfigurationId::new(Uuid::new_v4()));
        let response = ChatModelResponse::from_primary(
            ResolvedPrimaryModel {
                source,
                configuration_id,
                configuration_name: configured.then(|| "Deep".into()),
                model_handle: "test/model".into(),
                thinking_effort: configured.then_some(ThinkingEffort::High),
            },
            vec![],
        );
        let response = serde_json::to_value(response).unwrap();
        assert_eq!(response["source"], label);
        let pinned = source == PrimaryModelSource::ConversationPin;
        assert_eq!(
            response["selection_mode"],
            if pinned { "explicit" } else { "auto" }
        );

        assert_eq!(response["selected_model"].is_null(), !pinned);
        assert_eq!(response["requested_model"].is_null(), !pinned);
        assert_eq!(response["configuration_id"], json!(configuration_id));
    }
}
