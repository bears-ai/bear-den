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
    let refused = app
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
                .body(Body::from(upload(&edited_bundle, false)))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(refused.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        bears_db::list_bears_for_user(&pool, actor)
            .await
            .unwrap()
            .len(),
        before
    );
    let imported = app
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
                .body(Body::from(upload(&edited_bundle, true)))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(imported.status(), StatusCode::SEE_OTHER);
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
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/bears/import")
                .header(header::COOKIE, &cookie)
                .header(
                    header::CONTENT_TYPE,
                    "multipart/form-data; boundary=portable",
                )
                .body(Body::from(upload(&legacy, true)))
                .unwrap(),
        )
        .await
        .unwrap();
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
