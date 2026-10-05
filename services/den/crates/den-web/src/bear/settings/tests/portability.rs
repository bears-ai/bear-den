use super::*;
use den_core::ids::{BearId, UserId};
use den_memory::{
    access::AccessContext,
    scoped::{self, MemoryReadGrant},
    LogicalMemoryPath, MemorySource,
};

fn upload(bytes: &[u8], acknowledged: bool) -> Vec<u8> {
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
    assert_eq!(manifest.version, 2);
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
}
