use super::super::model_configurations::{PendingConfigurationSelection, PendingModelsForm};
use super::*;
use den_core::{
    ids::{ModelConfigurationId, UserId},
    ThinkingEffort,
};
use den_service::bears::model_configurations as service;

pub(super) async fn seed_model(pool: &sqlx::PgPool, support: Option<bool>) -> String {
    let handle = format!("test/model-{}", Uuid::new_v4());
    sqlx::query!(
        "INSERT INTO model_selection_options (handle, display_name, selectable, metadata_json) VALUES ($1, 'UI test model', TRUE, $2)",
        handle,
        json!({"supports_reasoning_effort": support}),
    ).execute(pool).await.unwrap();
    handle
}

#[test]
fn pending_configuration_selection_preserves_unchanged_inherit_and_named_drafts() {
    let stored = ModelConfigurationId::new(Uuid::new_v4());
    let draft = ModelConfigurationId::new(Uuid::new_v4());
    for current in [None, Some(stored)] {
        assert_eq!(
            PendingConfigurationSelection::Unchanged.selected_id(current),
            current
        );
        assert_eq!(
            PendingConfigurationSelection::Inherit.selected_id(current),
            None
        );
        assert_eq!(
            PendingConfigurationSelection::Configuration(draft).selected_id(current),
            Some(draft)
        );
    }
    assert_eq!(
        PendingConfigurationSelection::from(None),
        PendingConfigurationSelection::Inherit
    );
    assert_eq!(
        PendingConfigurationSelection::from(Some(draft)),
        PendingConfigurationSelection::Configuration(draft)
    );
    assert_eq!(
        PendingModelsForm::default()
            .default_selection
            .selected_id(Some(stored)),
        Some(stored)
    );
}

fn escaped_html(value: &str) -> String {
    Environment::new()
        .render_str("{{ value | e }}", context! { value })
        .unwrap()
}

fn assert_input_value(page: &str, id: &str, value: &str) {
    let pattern = format!(r#"<input\b[^>]*\bid="{}"[^>]*>"#, regex::escape(id));
    let input = regex::Regex::new(&pattern).unwrap();
    let input = input
        .find(page)
        .unwrap_or_else(|| panic!("missing input {id}"));
    assert!(
        input
            .as_str()
            .contains(&format!("value=\"{}\"", escaped_html(value))),
        "input {id} did not preserve {value:?}: {}",
        input.as_str(),
    );
}

async fn post_as(app: &Router, cookie: &str, uri: &str, body: &str) -> (StatusCode, String) {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(uri)
                .header(header::COOKIE, cookie)
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(Body::from(body.to_owned()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

#[tokio::test]
async fn named_configuration_crud_is_admin_only_and_preserves_invalid_forms() {
    let _guard = TEST_DB_LOCK.lock().await;
    let Some(pool) = test_pool().await else {
        return;
    };
    let slug = fresh_slug();
    let bear_id = create_test_bear(&pool, &slug).await;
    let admin = create_bear_admin_user(&pool, bear_id).await;
    let member = create_bear_user(&pool, bear_id, BEAR_ROLE_MEMBER).await;
    let model = seed_model(&pool, Some(true)).await;
    let hat = hats::create_hat(
        &pool,
        BearId::new(bear_id),
        UserId::new(admin),
        "Engineering",
        "Code care",
    )
    .await
    .unwrap();
    let app = test_app(pool.clone()).await;
    let admin_cookie = login_cookie(&app, admin).await;
    let member_cookie = login_cookie(&app, member).await;
    let root = format!("/bear/{slug}/models");
    let config = service::create(
        &pool,
        BearId::new(bear_id),
        "Careful",
        &model,
        Some(ThinkingEffort::High),
    )
    .await
    .unwrap();
    for uri in [
        format!("{root}/configurations"),
        format!("{root}/configurations/{}", config.id),
        format!("{root}/configurations/{}/delete", config.id),
        format!("{root}/default"),
        format!("/bear/{slug}/hats/{}/model", hat.id),
        root.clone(),
        format!("{root}/provision-bifrost-key"),
    ] {
        let (status, _) = post_as(&app, &member_cookie, &uri, &format!("name=Denied&model_handle={model}&thinking_effort=model_default&configuration_id={}", config.id)).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{uri}");
    }
    let (status, page) = get_as(&app, &member_cookie, &root).await;
    assert_eq!(status, StatusCode::OK, "{page}");
    assert!(page.contains("Careful"));
    assert!(page.contains("reasoning High"));
    assert!(!page.contains("Create configuration"));
    assert!(!page.contains("Save configuration"));
    assert!(!page.contains("Delete Careful"));

    let (status, page) = post_as(
        &app,
        &admin_cookie,
        &format!("{root}/configurations/{}", config.id),
        "name=My+unsaved+name&model_handle=unknown%2Fmodel&thinking_effort=high",
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{page}");
    assert!(page.contains("not in the Den catalog"));
    assert_input_value(
        &page,
        &format!("configuration-name-{}", config.id),
        "My unsaved name",
    );
    assert_input_value(
        &page,
        &format!("configuration-model-{}", config.id),
        "unknown/model",
    );
    assert_eq!(
        service::get(&pool, BearId::new(bear_id), config.id)
            .await
            .unwrap()
            .unwrap()
            .name,
        "Careful"
    );
    let (status, page) = post_as(
        &app,
        &admin_cookie,
        &format!("{root}/configurations"),
        &format!("name=Draft+%3Cscript%3E&model_handle={model}&thinking_effort=invalid"),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(page.contains("Draft &lt;script&gt;"));
    assert!(page.contains("invalid effort"));
    assert!(!page.contains("Draft <script>"));
    assert_eq!(
        post_as(
            &app,
            &admin_cookie,
            &format!("{root}/default"),
            &format!("configuration_id={}", config.id)
        )
        .await
        .0,
        StatusCode::SEE_OTHER
    );
    let (status, page) = post_as(
        &app,
        &admin_cookie,
        &format!("{root}/configurations/{}/delete", config.id),
        "",
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{page}");
    assert!(page.contains("referenced"));
    // Saving unrelated loop/Bifrost controls must not re-write the compatibility default.
    assert_eq!(
        post_as(
            &app,
            &admin_cookie,
            &root,
            "bear_default_model=inherit&bear_loop_control=careful"
        )
        .await
        .0,
        StatusCode::SEE_OTHER
    );
    assert_eq!(
        service::default_configuration_id(&pool, BearId::new(bear_id))
            .await
            .unwrap(),
        Some(config.id)
    );
    assert_eq!(
        post_as(
            &app,
            &admin_cookie,
            &format!("{root}/default"),
            "configuration_id="
        )
        .await
        .0,
        StatusCode::SEE_OTHER
    );
    assert_eq!(
        post_as(
            &app,
            &admin_cookie,
            &format!("{root}/configurations/{}", config.id),
            &format!("name=Quick&model_handle={model}&thinking_effort=model_default")
        )
        .await
        .0,
        StatusCode::SEE_OTHER
    );
    let changed = service::get(&pool, BearId::new(bear_id), config.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(changed.name, "Quick");
    assert_eq!(changed.thinking_effort, None);
    assert_eq!(
        post_as(
            &app,
            &admin_cookie,
            &format!("{root}/configurations/{}/delete", config.id),
            ""
        )
        .await
        .0,
        StatusCode::SEE_OTHER
    );
    assert_eq!(
        post_as(
            &app,
            &admin_cookie,
            &format!("{root}/configurations"),
            &format!("name=Created&model_handle={model}&thinking_effort=medium")
        )
        .await
        .0,
        StatusCode::SEE_OTHER
    );
    assert_eq!(
        service::list(&pool, BearId::new(bear_id))
            .await
            .unwrap()
            .len(),
        1
    );
}

#[tokio::test]
async fn unsupported_effort_hat_inheritance_and_revoked_models_are_explicit() {
    let _guard = TEST_DB_LOCK.lock().await;
    let Some(pool) = test_pool().await else {
        return;
    };
    let slug = fresh_slug();
    let bear_id = create_test_bear(&pool, &slug).await;
    let admin = create_bear_admin_user(&pool, bear_id).await;
    let model = seed_model(&pool, Some(true)).await;
    let default = service::create(
        &pool,
        BearId::new(bear_id),
        "Deep",
        &model,
        Some(ThinkingEffort::High),
    )
    .await
    .unwrap();
    let quick = service::create(&pool, BearId::new(bear_id), "Quick", &model, None)
        .await
        .unwrap();
    service::set_default(&pool, BearId::new(bear_id), Some(default.id))
        .await
        .unwrap();
    let hat = hats::create_hat(
        &pool,
        BearId::new(bear_id),
        UserId::new(admin),
        "Home",
        "House care",
    )
    .await
    .unwrap();
    let app = test_app(pool.clone()).await;
    let cookie = login_cookie(&app, admin).await;
    let uri = format!("/bear/{slug}/hats/{}", hat.id);
    let (status, page) = get_as(&app, &cookie, &uri).await;
    assert_eq!(status, StatusCode::OK, "{page}");
    assert!(page.contains("Bear default:"));
    assert!(page.contains("reasoning High"));
    assert!(page.contains("Inherit Bear default (or deployment default)"));
    assert_eq!(
        post_as(
            &app,
            &cookie,
            &format!("{uri}/model"),
            &format!("configuration_id={}", quick.id)
        )
        .await
        .0,
        StatusCode::SEE_OTHER
    );
    let (_, page) = get_as(&app, &cookie, &uri).await;
    assert!(page.contains("Hat override:"));
    assert!(page.contains("reasoning Model default"));
    assert_eq!(
        post_as(&app, &cookie, &format!("{uri}/model"), "configuration_id=")
            .await
            .0,
        StatusCode::SEE_OTHER
    );
    assert_eq!(
        service::hat_configuration_id(&pool, BearId::new(bear_id), hat.id)
            .await
            .unwrap(),
        None
    );

    for support in [None, Some(false)] {
        let unsupported = seed_model(&pool, support).await;
        let (status, page) = post_as(
            &app,
            &cookie,
            &format!("/bear/{slug}/models/configurations"),
            &format!("name=Unsupported&model_handle={unsupported}&thinking_effort=high"),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{page}");
        assert!(page.contains("value=\"Unsupported\""));
        assert!(page.contains(if support.is_none() {
            "support is unknown"
        } else {
            "does not support"
        }));
    }
    sqlx::query!(
        "UPDATE model_selection_options SET selectable = FALSE WHERE handle = $1",
        model
    )
    .execute(&pool)
    .await
    .unwrap();
    for path in [format!("/bear/{slug}/models"), uri.clone()] {
        let (status, page) = get_as(&app, &cookie, &path).await;
        assert_eq!(status, StatusCode::OK, "{page}");
        assert!(page.contains("unavailable"));
        assert!(page.contains("no longer selectable"));
        assert!(
            page.contains(&format!("<code>{}</code>", escaped_html(&model))),
            "revoked model must remain inspectable on {path}",
        );
    }
    let (status, page) = post_as(
        &app,
        &cookie,
        &format!("{uri}/model"),
        &format!("configuration_id={}", quick.id),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{page}");
    assert!(page.contains(&format!("value=\"{}\" selected", quick.id)));
    assert_eq!(
        service::hat_configuration_id(&pool, BearId::new(bear_id), hat.id)
            .await
            .unwrap(),
        None
    );
    let other_id = create_test_bear(&pool, &fresh_slug()).await;
    let foreign_model = seed_model(&pool, Some(true)).await;
    let foreign = service::create(
        &pool,
        BearId::new(other_id),
        "Foreign",
        &foreign_model,
        None,
    )
    .await
    .unwrap();
    assert_eq!(
        post_as(
            &app,
            &cookie,
            &format!("{uri}/model"),
            &format!("configuration_id={}", foreign.id)
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
}

#[test]
fn hat_template_renders_inheritance_and_unavailable_override_without_grants() {
    let mut env = Environment::new();
    env.add_template("bear/_manage.html", "{% block manage %}{% endblock %}")
        .unwrap();
    env.add_template(
        "hat",
        include_str!("../../../templates/bear/manage/hat.jinja"),
    )
    .unwrap();
    let config_id = Uuid::new_v4();
    for (selection, source) in [
        (String::new(), "Bear default"),
        (config_id.to_string(), "Hat override"),
    ] {
        let page = env.get_template("hat").unwrap().render(context! {
            bear => json!({"slug": "bear"}),
            hat => json!({"id": "hat", "name": "Engineering", "purpose": "Code care", "identity_prompt": "Identity", "work_enabled": false, "auto_curate_enabled": false}),
            configurations => json!([{"configuration": {"id": config_id, "name": "Deep", "model_handle": "test/revoked"}, "effort_label": "High", "status": "unavailable"}]),
            effective_model => json!({"name": "Deep", "source_label": source, "model_handle": "test/revoked", "effort_label": "High", "status": "unavailable", "status_detail": "Model revoked"}),
            model_selection => selection, historical_hat_records => 0, grant_count => 0,
            web_grants => json!({"fetch_tool_grant_id": null, "search_tool_grant_id": null, "hosts": []}),
        }).unwrap();
        assert!(page.contains(&format!("{source}:")));
        assert!(page.contains("Model revoked"));
        assert!(page.contains("reasoning High"));
        assert!(page.contains("Inherit Bear default (or deployment default)"));
        assert!(page.contains("Web-fetch tool: not granted"));
        assert!(page.contains("Use in Jobs: off"));
        let selected = if selection.is_empty() {
            "value=\"\" selected".to_string()
        } else {
            format!("value=\"{config_id}\" selected")
        };
        assert!(page.contains(&selected));
    }
}

#[test]
fn models_template_renders_status_saved_effort_draft_and_read_only_controls() {
    let mut env = Environment::new();
    env.add_template("bear/_manage.html", "{% block manage %}{% endblock %}")
        .unwrap();
    env.add_template(
        "models",
        include_str!("../../../templates/bear/settings/models.html"),
    )
    .unwrap();
    let config_id = Uuid::new_v4();
    let views = json!([{
        "configuration": {"id": config_id, "name": "Deep <plan>", "model_handle": "test/revoked"},
        "fields": {"name": "Unsaved <draft>", "model_handle": "test/repaired", "thinking_effort": "medium"},
        "effort_label": "High", "status": "unavailable", "status_detail": "Model revoked"
    }]);
    for can_manage_bear in [false, true] {
        let page = env.get_template("models").unwrap().render(context! {
            bear => json!({"slug": "bear", "name": "Bear"}), configurations => views,
            effective_model => json!({"name": "Deep <plan>", "source_label": "Bear default", "model_handle": "test/revoked", "effort_label": "High", "status": "unavailable", "status_detail": "Model revoked"}),
            default_selection => config_id.to_string(), can_manage_bear,
            new_configuration => json!({"name": "", "model_handle": "", "thinking_effort": ""}),
            bifrost_usage => json!({"status": "missing", "error": "Not configured"}),
            bear_loop_control => "careful", bear_tool_budget_multiplier => "1.25",
            stored_bear_loop_control => "standard", stored_bear_tool_budget_multiplier => "1",
            bifrost_virtual_key_id => "", bifrost_virtual_key_name => "",
            bifrost_virtual_key_configured => false, bifrost_virtual_key_clear => false,
            field_errors => std::collections::BTreeMap::<String, String>::new(),
            model_options => Vec::<serde_json::Value>::new(),
        }).unwrap();
        assert!(page.contains("Deep &lt;plan&gt;"));
        assert!(page.contains("reasoning High"));
        assert!(page.contains("unavailable"));
        assert_eq!(
            page.contains(&format!("value=\"{config_id}\" selected")),
            can_manage_bear,
        );
        assert!(page.contains("Saved loop control: <strong>standard</strong>"));
        assert!(page.contains("saved tool budget multiplier: <strong>1</strong>"));
        assert_eq!(page.contains("Unsaved &lt;draft&gt;"), can_manage_bear);
        assert_eq!(page.contains("Create configuration"), can_manage_bear);
        assert!(!page.contains("bear_default_model"));
        assert!(!page.contains("fallback"));
        assert!(!page.contains("delegation"));
        if can_manage_bear {
            assert!(page.contains("value=\"careful\" selected"));
            assert!(page.contains("value=\"1.25\""));
            assert!(page.contains("value=\"medium\" selected"));
            assert!(page.contains("Reasoning effort"));
            assert!(page.contains("applies to model requests; loop control manages checkpoints and budgets separately"));
            assert!(!page.contains("not loop checkpoints"));
        }
    }
}
