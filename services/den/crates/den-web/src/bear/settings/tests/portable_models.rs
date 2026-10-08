use super::super::portable_models::{self as portable, PortableModelConfiguration};
use super::*;
use den_core::{
    ids::{HatId, ModelConfigurationId},
    ThinkingEffort,
};

fn configuration() -> PortableModelConfiguration {
    PortableModelConfiguration {
        original_id: ModelConfigurationId::new(Uuid::new_v4()),
        name: "Deep".into(),
        model_handle: "test/model".into(),
        thinking_effort: Some(ThinkingEffort::High),
    }
}

fn hat(configuration_id: Option<ModelConfigurationId>) -> portable_hats::PortableHat {
    portable_hats::PortableHat {
        original_id: HatId::new(Uuid::new_v4()),
        model_configuration_id: configuration_id,
        name: "Home".into(),
        purpose: "House care".into(),
        short_summary: None,
        identity_prompt: "Care for the home".into(),
        work_requested: true,
        automatic_sharing_requested: true,
        repository_names: vec![],
        https_hosts: vec![],
        web_fetch_requested: true,
        web_search_requested: true,
    }
}

#[test]
fn portable_models_require_unique_ids_names_and_internal_references() {
    let config = configuration();
    let configs = std::slice::from_ref(&config);
    assert!(portable::validate(
        Some(configs),
        Some(config.original_id),
        &[hat(Some(config.original_id))]
    )
    .is_ok());
    assert!(portable::validate(Some(&[config.clone(), config.clone()]), None, &[]).is_err());
    let mut duplicate_name = configuration();
    duplicate_name.name = " deep ".into();
    assert!(portable::validate(Some(&[config.clone(), duplicate_name]), None, &[]).is_err());
    let foreign = ModelConfigurationId::new(Uuid::new_v4());
    assert!(portable::validate(Some(configs), Some(foreign), &[]).is_err());
    assert!(portable::validate(Some(configs), None, &[hat(Some(foreign))]).is_err());
    assert!(portable::validate(None, None, &[hat(Some(config.original_id))]).is_err());
    let mut invalid = config.clone();
    invalid.name = " ".into();
    assert!(portable::validate(Some(&[invalid]), None, &[]).is_err());
    assert!(portable::validate(Some(&vec![config; 101]), None, &[]).is_err());
    assert!(portable::validate(None, None, &[hat(None)]).is_ok());
}

#[test]
fn portable_model_ids_are_remapped_and_effort_is_preserved_without_authority_fields() {
    let config = configuration();
    let imported = ModelConfigurationId::new(Uuid::new_v4());
    let mapping = std::collections::HashMap::from([(config.original_id, imported)]);
    assert_eq!(
        portable::remap(&mapping, Some(config.original_id)).unwrap(),
        Some(imported)
    );
    assert_eq!(portable::remap(&mapping, None).unwrap(), None);
    assert!(portable::remap(&mapping, Some(ModelConfigurationId::new(Uuid::new_v4()))).is_err());
    let encoded = serde_json::to_value(&config).unwrap();
    assert_eq!(encoded["thinking_effort"], "high");
    for forbidden in [
        "bear_id",
        "credential",
        "grant",
        "work_enabled",
        "identity_prompt",
    ] {
        assert!(encoded.get(forbidden).is_none());
    }
    let mut invalid = encoded;
    invalid["thinking_effort"] = json!("ultra");
    assert!(serde_json::from_value::<PortableModelConfiguration>(invalid).is_err());
}

#[tokio::test]
async fn blank_legacy_defaults_inherit_without_catalog_access() {
    let pool = PgPoolOptions::new()
        .connect_lazy("postgres://unused:unused@127.0.0.1:1/unused")
        .unwrap();
    pool.close().await;
    for legacy_default in [None, Some(""), Some(" \t\n"), Some("\u{2003}")] {
        portable::validate_catalog(&pool, None, legacy_default)
            .await
            .expect("blank legacy defaults inherit without querying the catalog");
    }
    assert!(
        portable::validate_catalog(&pool, None, Some("missing/model"))
            .await
            .is_err()
    );
}

#[tokio::test]
async fn bundle_import_rejects_destination_catalog_errors_before_setup() {
    let _guard = TEST_DB_LOCK.lock().await;
    let Some(pool) = test_pool().await else {
        return;
    };
    let source_slug = fresh_slug();
    let bear_id = create_test_bear(&pool, &source_slug).await;
    let admin = create_bear_admin_user(&pool, bear_id).await;
    let bear = bears_db::get_bear(&pool, bear_id).await.unwrap().unwrap();
    let app = test_app(pool.clone()).await;
    let cookie = login_cookie(&app, admin).await;
    let unsupported = super::model_configurations::seed_model(&pool, Some(false)).await;
    let unknown_support = super::model_configurations::seed_model(&pool, None).await;
    let revoked = super::model_configurations::seed_model(&pool, Some(true)).await;
    sqlx::query!(
        "UPDATE model_selection_options SET selectable = FALSE WHERE handle = $1",
        revoked
    )
    .execute(&pool)
    .await
    .unwrap();
    for (model, effort, expected) in [
        (
            "missing/catalog-model".to_owned(),
            None,
            "not in the Den catalog",
        ),
        (unsupported, Some(ThinkingEffort::High), "does not support"),
        (revoked, None, "no longer selectable"),
        (
            unknown_support,
            Some(ThinkingEffort::Medium),
            "support is unknown",
        ),
    ] {
        let mut manifest = manifest_for_bear(&bear).unwrap();
        manifest.bear.slug = format!("rejected-{}", Uuid::new_v4().simple());
        let mut config = configuration();
        config.model_handle = model;
        config.thinking_effort = effort;
        manifest.model_configurations = Some(vec![config]);
        let bundle =
            build_bear_bundle(&serde_yml::to_string(&manifest).unwrap(), b"not a database")
                .unwrap();
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/bears/import")
                    .header(header::COOKIE, &cookie)
                    .header(
                        header::CONTENT_TYPE,
                        "multipart/form-data; boundary=portable",
                    )
                    .body(Body::from(super::portability::upload(&bundle, true)))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        let review = response.headers()[header::LOCATION]
            .to_str()
            .unwrap()
            .to_owned();
        let reviewed = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(&review)
                    .header(header::COOKIE, &cookie)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(reviewed.status(), StatusCode::BAD_REQUEST);
        let body = reviewed.into_body().collect().await.unwrap().to_bytes();
        assert!(String::from_utf8_lossy(&body).contains(expected));
        assert_eq!(
            super::portability::confirm(&app, &cookie, &review, true)
                .await
                .status(),
            StatusCode::BAD_REQUEST
        );
        assert!(!bears_db::bear_slug_exists(&pool, &manifest.bear.slug)
            .await
            .unwrap());
    }
    let mut manifest = manifest_for_bear(&bear).unwrap();
    manifest.version = 1;
    manifest.bear.slug = format!("legacy-rejected-{}", Uuid::new_v4().simple());
    manifest.model_configurations = None;
    manifest.bear.default_model = Some("legacy/missing-model".into());
    let bundle =
        build_bear_bundle(&serde_yml::to_string(&manifest).unwrap(), b"not a database").unwrap();
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/bears/import")
                .header(header::COOKIE, &cookie)
                .header(
                    header::CONTENT_TYPE,
                    "multipart/form-data; boundary=portable",
                )
                .body(Body::from(super::portability::upload(&bundle, true)))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    let review = response.headers()[header::LOCATION]
        .to_str()
        .unwrap()
        .to_owned();
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(&review)
                .header(header::COOKIE, &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body = response.into_body().collect().await.unwrap().to_bytes();
    assert!(String::from_utf8_lossy(&body).contains("not in the Den catalog"));
    assert_eq!(
        super::portability::confirm(&app, &cookie, &review, true)
            .await
            .status(),
        StatusCode::BAD_REQUEST
    );
    assert!(!bears_db::bear_slug_exists(&pool, &manifest.bear.slug)
        .await
        .unwrap());
}

#[test]
fn backup_ui_describes_v3_model_references_and_fresh_authorization() {
    let mut env = minijinja::Environment::new();
    env.add_template("bear/_manage.html", "{% block manage %}{% endblock %}")
        .unwrap();
    env.add_filter("urlencode", |value: String| value);
    env.add_template(
        "backup",
        include_str!("../../../templates/bear/manage/portability.html"),
    )
    .unwrap();
    let page = env
        .get_template("backup")
        .unwrap()
        .render(context! {
            bear => json!({"name": "Backup Bear", "slug": "backup"}),
            can_manage_bear => true,
            reconnection_intent => Vec::<portable_hats::ReconnectionIntent>::new(),
        })
        .unwrap();
    assert!(page.contains(".bear v3 bundle"));
    assert!(page.contains("Named model configurations, Bear default and hat overrides"));
    assert!(page.contains("remapped IDs, validated against the destination catalog"));
    assert!(page.contains("No — re-authorize on the destination"));
    assert!(page.contains("Review imported hats and knowledge"));
}

fn legacy_manifest(version: u32) -> String {
    format!("format: bear\nversion: {version}\nbear:\n  slug: old\n  name: Old Bear\n  description: Legacy\n  birthdate: '2020-01-01'\n  default_model: test/legacy\nprompts:\n  system_prompt: Legacy identity\n")
}

#[test]
fn old_bundles_still_read_and_new_bundles_preserve_named_configurations() {
    for version in [1, 2] {
        let bytes =
            build_bear_bundle(&legacy_manifest(version), b"nonempty sqlite fixture").unwrap();
        let (manifest, _) = read_bear_bundle(&bytes).unwrap();
        assert!(manifest.model_configurations.is_none());
        assert!(manifest.default_model_configuration_id.is_none());
        assert_eq!(manifest.bear.default_model.as_deref(), Some("test/legacy"));
    }
    let (mut manifest, memory) =
        read_bear_bundle(&build_bear_bundle(&legacy_manifest(2), b"sqlite").unwrap()).unwrap();
    let config = configuration();
    manifest.version = BEAR_BUNDLE_VERSION;
    manifest.bear.default_model = None;
    manifest.model_configurations = Some(vec![config.clone()]);
    manifest.default_model_configuration_id = Some(config.original_id);
    manifest.hats = vec![hat(Some(config.original_id)), hat(None)];
    manifest.hats[1].name = "Engineering".into();
    let bundle = build_bear_bundle(&serde_yml::to_string(&manifest).unwrap(), &memory).unwrap();
    let (decoded, _) = read_bear_bundle(&bundle).unwrap();
    assert_eq!(
        decoded.model_configurations.unwrap()[0].thinking_effort,
        Some(ThinkingEffort::High)
    );
    assert_eq!(
        decoded.default_model_configuration_id,
        Some(config.original_id)
    );
    assert_eq!(
        decoded.hats[0].model_configuration_id,
        Some(config.original_id)
    );
    assert!(decoded.hats[1].model_configuration_id.is_none());
    manifest.default_model_configuration_id = Some(ModelConfigurationId::new(Uuid::new_v4()));
    let invalid = build_bear_bundle(&serde_yml::to_string(&manifest).unwrap(), &memory).unwrap();
    assert!(read_bear_bundle(&invalid).is_err());
}

#[tokio::test]
async fn confirmation_revalidates_catalog_after_review_without_creating_a_bear() {
    let _guard = TEST_DB_LOCK.lock().await;
    let Some(pool) = test_pool().await else {
        return;
    };
    let slug = fresh_slug();
    let bear_id = create_test_bear(&pool, &slug).await;
    let admin = create_bear_admin_user(&pool, bear_id).await;
    let bear = bears_db::get_bear(&pool, bear_id).await.unwrap().unwrap();
    let model = super::model_configurations::seed_model(&pool, Some(true)).await;
    let mut manifest = manifest_for_bear(&bear).unwrap();
    manifest.bear.slug = format!("stale-catalog-{}", Uuid::new_v4().simple());
    let mut configuration = configuration();
    configuration.model_handle = model.clone();
    manifest.model_configurations = Some(vec![configuration]);
    let bundle =
        build_bear_bundle(&serde_yml::to_string(&manifest).unwrap(), b"sqlite fixture").unwrap();
    let app = test_app(pool.clone()).await;
    let cookie = login_cookie(&app, admin).await;
    let review = super::portability::preview(&app, &cookie, &bundle).await;
    sqlx::query!(
        "UPDATE model_selection_options SET selectable = FALSE WHERE handle = $1",
        model
    )
    .execute(&pool)
    .await
    .unwrap();
    let rejected = super::portability::confirm(&app, &cookie, &review, true).await;
    assert_eq!(rejected.status(), StatusCode::BAD_REQUEST);
    let body = rejected.into_body().collect().await.unwrap().to_bytes();
    assert!(String::from_utf8_lossy(&body).contains("Nothing was imported"));
    assert!(!bears_db::bear_slug_exists(&pool, &manifest.bear.slug)
        .await
        .unwrap());
    assert_eq!(
        super::portability::confirm(&app, &cookie, &review, true)
            .await
            .status(),
        StatusCode::BAD_REQUEST
    );
}
