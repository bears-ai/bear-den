use super::*;
use den_core::{
    ids::{BearId, UserId},
    ThinkingEffort,
};
use den_memory::{
    access::AccessContext,
    scoped::{self, MemoryReadGrant},
    LogicalMemoryPath, MemorySource,
};
use den_service::bears::model_configurations as models;

pub(super) fn upload(bytes: &[u8], acknowledged: bool) -> Vec<u8> {
    let mut body = Vec::new();
    if acknowledged {
        body.extend_from_slice(b"--portable\r\nContent-Disposition: form-data; name=\"confirm_imported_knowledge\"\r\n\r\ntrue\r\n");
    }
    body.extend_from_slice(b"--portable\r\nContent-Disposition: form-data; name=\"bundle\"; filename=\"backup.bear\"\r\nContent-Type: application/zip\r\n\r\n");
    body.extend_from_slice(bytes);
    body.extend_from_slice(b"\r\n--portable--\r\n");
    body
}

#[tokio::test]
async fn bundle_roundtrip_remints_hats_and_memory_without_restoring_authority() {
    let _guard = TEST_DB_LOCK.lock().await;
    let Some(pool) = test_pool().await else {
        return;
    };
    let slug = fresh_slug();
    let bear_id = create_test_bear(&pool, &slug).await;
    let actor = create_bear_admin_user(&pool, bear_id).await;
    let original = hats::create_hat_with_summary(
        &pool,
        BearId::new(bear_id),
        UserId::new(actor),
        "Home",
        "House care",
        Some("Home responsibility"),
    )
    .await
    .unwrap();
    hats::manage::update_hat(
        &pool,
        BearId::new(bear_id),
        original.id,
        "Home",
        "House care",
        "Use the house care procedure",
        false,
    )
    .await
    .unwrap();
    hats::set_ide_default_hat(&pool, BearId::new(bear_id), original.id)
        .await
        .unwrap();
    let model = super::model_configurations::seed_model(&pool, Some(true)).await;
    let default_configuration = models::create(
        &pool,
        BearId::new(bear_id),
        "Deep",
        &model,
        Some(ThinkingEffort::High),
    )
    .await
    .unwrap();
    let hat_configuration = models::create(&pool, BearId::new(bear_id), "Quick", &model, None)
        .await
        .unwrap();
    models::set_default(&pool, BearId::new(bear_id), Some(default_configuration.id))
        .await
        .unwrap();
    models::set_hat_override(
        &pool,
        BearId::new(bear_id),
        original.id,
        Some(hat_configuration.id),
    )
    .await
    .unwrap();
    let state = test_state(pool.clone());
    let store = state.memory_stores.store_for_bear(bear_id).await.unwrap();
    let path = LogicalMemoryPath::hat(original.id, "entry");
    store
        .append_record(
            &path,
            "note",
            "curate",
            None,
            "Reviewed hat knowledge",
            &json!({}),
            "normal",
        )
        .await
        .unwrap();
    let app = test_app(pool.clone()).await;
    let cookie = login_cookie(&app, actor).await;
    let exported = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/bear/{slug}/export.bear"))
                .header(header::COOKIE, &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(exported.status(), StatusCode::OK);
    let bytes = exported.into_body().collect().await.unwrap().to_bytes();
    let (mut manifest, memory) = read_bear_bundle(&bytes).unwrap();
    assert_eq!(manifest.version, BEAR_BUNDLE_VERSION);
    assert!(manifest.bear.default_model.is_none());
    assert_eq!(
        manifest.default_model_configuration_id,
        Some(default_configuration.id)
    );
    assert_eq!(manifest.model_configurations.as_ref().unwrap().len(), 2);
    assert_eq!(
        manifest.hats[0].model_configuration_id,
        Some(hat_configuration.id)
    );
    assert_eq!(manifest.hats[0].original_id, original.id);
    assert_eq!(manifest.ide_default_hat, Some(original.id));
    manifest.hats[0].work_requested = true;
    manifest.hats[0].automatic_sharing_requested = true;
    manifest.hats[0].https_hosts = vec!["example.com".into()];
    manifest.hats[0].web_fetch_requested = true;
    let edited_bundle =
        build_bear_bundle(&serde_yml::to_string(&manifest).unwrap(), &memory).unwrap();
    let before = bears_db::list_bears_for_user(&pool, actor)
        .await
        .unwrap()
        .len();
    let review = preview(&app, &cookie, &edited_bundle).await;
    assert_eq!(
        bears_db::list_bears_for_user(&pool, actor)
            .await
            .unwrap()
            .len(),
        before
    );
    let refused = confirm(&app, &cookie, &review, false).await;
    assert_eq!(refused.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        bears_db::list_bears_for_user(&pool, actor)
            .await
            .unwrap()
            .len(),
        before
    );
    // Separate HTTP requests race through independent Session snapshots; the
    // file claim, not session remove/get, must ensure one creation.
    let (imported, replay) = tokio::join!(
        confirm(&app, &cookie, &review, true),
        confirm(&app, &cookie, &review, true),
    );
    assert_eq!(
        usize::from(imported.status() == StatusCode::SEE_OTHER)
            + usize::from(replay.status() == StatusCode::SEE_OTHER),
        1
    );
    assert_eq!(
        bears_db::list_bears_for_user(&pool, actor)
            .await
            .unwrap()
            .len(),
        before + 1
    );
    assert_eq!(
        confirm(&app, &cookie, &review, true).await.status(),
        StatusCode::BAD_REQUEST
    );
    let imported_bear = bears_db::list_bears_for_user(&pool, actor)
        .await
        .unwrap()
        .into_iter()
        .find(|row| row.bear.id != bear_id)
        .unwrap()
        .bear;
    let imported_hats = hats::list_hats(&pool, BearId::new(imported_bear.id))
        .await
        .unwrap();
    assert_eq!(imported_hats.len(), 1);
    let restored = &imported_hats[0];
    assert_ne!(restored.id, original.id);
    let restored_configurations = models::list(&pool, BearId::new(imported_bear.id))
        .await
        .unwrap();
    assert_eq!(restored_configurations.len(), 2);
    let restored_default = restored_configurations
        .iter()
        .find(|config| config.name == "Deep")
        .unwrap();
    let restored_override = restored_configurations
        .iter()
        .find(|config| config.name == "Quick")
        .unwrap();
    assert_ne!(restored_default.id, default_configuration.id);
    assert_ne!(restored_override.id, hat_configuration.id);
    assert_eq!(restored_default.model_handle.as_str(), model);
    assert_eq!(restored_default.thinking_effort, Some(ThinkingEffort::High));
    assert_eq!(restored_override.thinking_effort, None);
    assert_eq!(
        models::default_configuration_id(&pool, BearId::new(imported_bear.id))
            .await
            .unwrap(),
        Some(restored_default.id)
    );
    assert_eq!(
        models::hat_configuration_id(&pool, BearId::new(imported_bear.id), restored.id)
            .await
            .unwrap(),
        Some(restored_override.id)
    );
    assert_eq!(restored.identity_prompt, "Use the house care procedure");
    assert!(!restored.work_enabled);
    assert!(!restored.auto_curate_enabled);
    assert!(
        hats::manage::allowed_surfaces(&pool, BearId::new(imported_bear.id), restored.id)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        hats::access::web_grants_for_hat(&pool, BearId::new(imported_bear.id), restored.id)
            .await
            .unwrap()
            .fetch_tool_grant_id
            .is_none()
    );
    assert_eq!(
        hats::ide_default_hat(&pool, BearId::new(imported_bear.id))
            .await
            .unwrap(),
        Some(restored.id)
    );
    let imported_store = state
        .memory_stores
        .store_for_bear(imported_bear.id)
        .await
        .unwrap();
    let source = MemorySource::Conversation(Uuid::new_v4());
    let read = scoped::read_path(
        &imported_store,
        MemoryReadGrant::new(source, Some(restored.id)),
        &AccessContext::empty(),
        &path.to_logical_path(),
        10,
    )
    .await
    .unwrap();
    assert_eq!(read.len(), 1);
    assert_eq!(read[0].content_text, "Reviewed hat knowledge");
    assert!(scoped::read_path(
        &imported_store,
        MemoryReadGrant::new(source, Some(original.id)),
        &AccessContext::empty(),
        &path.to_logical_path(),
        10
    )
    .await
    .unwrap()
    .is_empty());

    // Legacy bundles carry only a raw default. The service bridge recreates a
    // named configuration with model-default effort, not the exported override.
    manifest.version = 2;
    manifest.bear.slug = format!("legacy-{}", Uuid::new_v4().simple());
    manifest.bear.default_model = Some(model.clone());
    manifest.model_configurations = None;
    manifest.default_model_configuration_id = None;
    manifest.hats[0].model_configuration_id = None;
    let legacy = build_bear_bundle(&serde_yml::to_string(&manifest).unwrap(), &memory).unwrap();
    let review = preview(&app, &cookie, &legacy).await;
    let response = confirm(&app, &cookie, &review, true).await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    let legacy_bear = bears_db::bear_for_user_by_slug(&pool, actor, &manifest.bear.slug)
        .await
        .unwrap()
        .unwrap();
    let legacy_id = models::default_configuration_id(&pool, BearId::new(legacy_bear.id))
        .await
        .unwrap()
        .unwrap();
    let legacy_config = models::get(&pool, BearId::new(legacy_bear.id), legacy_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(legacy_config.model_handle.as_str(), model);
    assert_eq!(legacy_config.thinking_effort, None);
}

pub(super) async fn preview(app: &axum::Router, cookie: &str, bytes: &[u8]) -> String {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/bears/import")
                .header(header::COOKIE, cookie)
                .header(
                    header::CONTENT_TYPE,
                    "multipart/form-data; boundary=portable",
                )
                .body(Body::from(upload(bytes, false)))
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
                .header(header::COOKIE, cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let page = String::from_utf8_lossy(&body);
    assert!(page.contains("No Bear has been created"));
    assert!(page.contains("SQLite memory contents are not displayed or fully inspected here"));
    assert!(page.contains("confirm_imported_knowledge"));
    review
}

pub(super) async fn confirm(
    app: &axum::Router,
    cookie: &str,
    review: &str,
    acknowledged: bool,
) -> Response {
    app.clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("{review}/confirm"))
                .header(header::COOKIE, cookie)
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(Body::from(if acknowledged {
                    "confirm_imported_knowledge=true"
                } else {
                    ""
                }))
                .unwrap(),
        )
        .await
        .unwrap()
}

#[tokio::test]
async fn review_requires_same_actor_and_session_and_cancel_is_final_without_creation() {
    let _guard = TEST_DB_LOCK.lock().await;
    let Some(pool) = test_pool().await else {
        return;
    };
    let slug = fresh_slug();
    let bear_id = create_test_bear(&pool, &slug).await;
    let actor = create_bear_admin_user(&pool, bear_id).await;
    let other = create_bear_user(&pool, bear_id, BEAR_ROLE_MEMBER).await;
    let bear = bears_db::get_bear(&pool, bear_id).await.unwrap().unwrap();
    let manifest = manifest_for_bear(&bear).unwrap();
    let bundle =
        build_bear_bundle(&serde_yml::to_string(&manifest).unwrap(), b"sqlite fixture").unwrap();
    let app = test_app(pool.clone()).await;
    let cookie = login_cookie(&app, actor).await;
    let same_actor_other_session = login_cookie(&app, actor).await;
    let other_actor = login_cookie(&app, other).await;
    let before = bears_db::list_bears_for_user(&pool, actor)
        .await
        .unwrap()
        .len();
    let review = preview(&app, &cookie, &bundle).await;
    for outsider in [&same_actor_other_session, &other_actor] {
        let page = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(&review)
                    .header(header::COOKIE, outsider)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(page.status(), StatusCode::BAD_REQUEST);
        assert_eq!(
            confirm(&app, outsider, &review, true).await.status(),
            StatusCode::BAD_REQUEST
        );
        let denied_cancel = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("{review}/cancel"))
                    .header(header::COOKIE, outsider)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(denied_cancel.status(), StatusCode::BAD_REQUEST);
    }
    let (status, owner_page) = get_as(&app, &cookie, &review).await;
    assert_eq!(status, StatusCode::OK, "{owner_page}");
    assert_eq!(
        confirm(&app, &cookie, &review, false).await.status(),
        StatusCode::BAD_REQUEST
    );
    let anonymous = app
        .clone()
        .oneshot(Request::builder().uri(&review).body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(anonymous.status(), StatusCode::UNAUTHORIZED);
    let cancelled = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("{review}/cancel"))
                .header(header::COOKIE, &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(cancelled.status(), StatusCode::SEE_OTHER);
    assert_eq!(
        confirm(&app, &cookie, &review, true).await.status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        bears_db::list_bears_for_user(&pool, actor)
            .await
            .unwrap()
            .len(),
        before
    );
}

#[tokio::test]
async fn acknowledgement_cannot_skip_the_review_get() {
    let _guard = TEST_DB_LOCK.lock().await;
    let Some(pool) = test_pool().await else {
        return;
    };
    let slug = fresh_slug();
    let bear_id = create_test_bear(&pool, &slug).await;
    let actor = create_bear_admin_user(&pool, bear_id).await;
    let bear = bears_db::get_bear(&pool, bear_id).await.unwrap().unwrap();
    let bundle = build_bear_bundle(
        &serde_yml::to_string(&manifest_for_bear(&bear).unwrap()).unwrap(),
        b"sqlite fixture",
    )
    .unwrap();
    let app = test_app(pool.clone()).await;
    let cookie = login_cookie(&app, actor).await;
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
                .body(Body::from(upload(&bundle, true)))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    let review = response.headers()[header::LOCATION]
        .to_str()
        .unwrap()
        .to_owned();
    assert_eq!(
        confirm(&app, &cookie, &review, true).await.status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        bears_db::list_bears_for_user(&pool, actor)
            .await
            .unwrap()
            .len(),
        1
    );
    let cancelled = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("{review}/cancel"))
                .header(header::COOKIE, &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(cancelled.status(), StatusCode::SEE_OTHER);
}
