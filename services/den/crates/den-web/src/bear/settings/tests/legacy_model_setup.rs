use super::model_configurations::{assert_input_value, escaped_html, post_as};
use super::*;
use crate::test_bifrost::{self, MockBifrost, PROVIDER_BODY, VIRTUAL_KEY};
use den_service::bears::model_configurations as models;
use serde_json::Value;
use sqlx::PgPool;

const MODEL: &str = "openai/gpt-6-sol";

fn safe(page: &str) {
    for secret in [
        PROVIDER_BODY,
        VIRTUAL_KEY,
        "management-token-secret-CANARY",
        "management-password-secret-CANARY",
        "global-api-key-secret-CANARY",
    ] {
        assert!(!page.contains(secret), "secret escaped: {secret}");
    }
}

#[sqlx::test(migrations = "../../migrations")]
async fn legacy_model_setup_existing_admin_edit_requires_current_bear_key_before_any_write(
    pool: PgPool,
) {
    let slug = fresh_slug();
    let bear = create_test_bear(&pool, &slug).await;
    let admin = create_bear_admin_user(&pool, bear).await;
    test_bifrost::seed_den_model(&pool, MODEL, Some(true)).await;
    test_bifrost::seed_key(&pool, bear).await;
    let gateway = MockBifrost::standard().await;
    let original = bears_db::get_bear(&pool, bear).await.unwrap().unwrap();
    let config = models::create(
        &pool,
        bear.into(),
        "Original default",
        "openai/gpt-4.1",
        None,
    )
    .await
    .unwrap();
    models::set_default(&pool, bear.into(), Some(config.id))
        .await
        .unwrap();
    let app = test_app_with_state(
        pool.clone(),
        test_state_with_config(pool.clone(), gateway.config()),
    )
    .await;
    let cookie = login_cookie(&app, admin).await;
    let path = format!("/test-admin/bears/{bear}/edit");
    for (status, code, expected) in [
        (StatusCode::OK, "model_missing", StatusCode::BAD_REQUEST),
        (
            StatusCode::UNAUTHORIZED,
            "virtual_key_rejected",
            StatusCode::CONFLICT,
        ),
        (
            StatusCode::SERVICE_UNAVAILABLE,
            "catalog_unavailable",
            StatusCode::SERVICE_UNAVAILABLE,
        ),
    ] {
        gateway.set_status(status);
        let (actual, page) = post_as(&app, &cookie, &path, &format!("slug={slug}&name=Retained+draft&description=Changed&system_prompt=Changed&default_model={MODEL}")).await;
        assert_eq!(actual, expected, "{page}");
        assert!(page.contains(code));
        assert_input_value(&page, "name_input", "Retained draft");
        assert!(page.contains(&escaped_html(MODEL)));
        safe(&page);
        let after = bears_db::get_bear(&pool, bear).await.unwrap().unwrap();
        assert_eq!(after.name, original.name);
        assert_eq!(after.system_prompt, original.system_prompt);
        assert_eq!(
            models::default_configuration_id(&pool, bear.into())
                .await
                .unwrap(),
            Some(config.id)
        );
        assert_eq!(models::list(&pool, bear.into()).await.unwrap().len(), 1);
    }
    assert_eq!(gateway.setup_calls().2, 0);
}

#[sqlx::test(migrations = "../../migrations")]
async fn legacy_model_setup_both_creation_forms_stage_before_key_and_compensate_gateway_denials(
    pool: PgPool,
) {
    let existing = create_test_bear(&pool, &fresh_slug()).await;
    let admin = create_bear_admin_user(&pool, existing).await;
    test_bifrost::seed_den_model(&pool, MODEL, Some(true)).await;
    let gateway = MockBifrost::standard().await;
    let app = test_app_with_state(
        pool.clone(),
        test_state_with_config(pool.clone(), gateway.creation_config(&pool)),
    )
    .await;
    let cookie = login_cookie(&app, admin).await;
    for route in ["/bears/new", "/test-admin/bears/new"] {
        // Local Den rejection must not create debt or even contact key provisioning.
        let before = gateway.setup_calls();
        let slug = format!("staged-model-{}", Uuid::new_v4());
        let (status, page) = post_as(&app, &cookie, route, &format!("slug={slug}&name=Local+draft&description=&system_prompt=&default_model=missing%2Fmodel")).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{page}");
        assert_input_value(&page, "slug_input", &slug);
        assert_eq!(gateway.setup_calls(), before);
        assert!(!bears_db::bear_slug_exists(&pool, &slug).await.unwrap());
        safe(&page);
        for (catalog_status, expected, code) in [
            (StatusCode::OK, StatusCode::BAD_REQUEST, "model_missing"),
            (
                StatusCode::UNAUTHORIZED,
                StatusCode::CONFLICT,
                "virtual_key_rejected",
            ),
        ] {
            gateway.set_status(catalog_status);
            let slug = format!("staged-model-{}", Uuid::new_v4());
            let (status, page) = post_as(&app, &cookie, route, &format!("slug={slug}&name=Gateway+draft&description=Keep+purpose&system_prompt=&default_model={MODEL}&grant_user_id={admin}&grant_role=admin")).await;
            assert_eq!(status, expected, "{page}");
            assert!(page.contains(code));
            assert!(page.contains("No Bear was saved"));
            assert_input_value(&page, "slug_input", &slug);
            assert_input_value(&page, "name_input", "Gateway draft");
            assert!(page.contains(&escaped_html(MODEL)));
            assert!(!bears_db::bear_slug_exists(&pool, &slug).await.unwrap());
            for id in gateway.staged_bears() {
                assert!(bears_db::get_bear(&pool, id).await.unwrap().is_none());
                assert!(models::list(&pool, id.into()).await.unwrap().is_empty());
            }
            assert_eq!(gateway.setup_calls().2, 0);
            assert!(bears_db::get_bear(&pool, existing).await.unwrap().is_some());
            safe(&page);
        }
    }
    assert_eq!(gateway.staged_bears().len(), 4);
    assert_eq!(gateway.setup_calls().1, 8); // quota before and after encrypted storage per key
    let (status, page) = get_as(&app, &cookie, "/bears/new").await;
    assert_eq!(status, StatusCode::OK);
    assert!(page.contains("availability is unverified"));
    assert!(!page.contains("<option value=\"openai"));
}

#[sqlx::test(migrations = "../../migrations")]
async fn legacy_model_setup_refused_compensation_exposes_saved_root_without_deleting_intervening_work_or_inviting_duplicates(
    pool: PgPool,
) {
    let existing = create_test_bear(&pool, &fresh_slug()).await;
    let admin = create_bear_admin_user(&pool, existing).await;
    test_bifrost::seed_den_model(&pool, MODEL, Some(true)).await;
    let gateway = MockBifrost::standard().await;
    gateway.preserve_staged_work();
    let app = test_app_with_state(
        pool.clone(),
        test_state_with_config(pool.clone(), gateway.creation_config(&pool)),
    )
    .await;
    let cookie = login_cookie(&app, admin).await;
    let slug = format!("staged-model-{}", Uuid::new_v4());
    let form = format!(
        "slug={slug}&name=Preserved+work&description=&system_prompt=&default_model={MODEL}"
    );
    let (status, page) = post_as(&app, &cookie, "/bears/new", &form).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{page}");
    assert!(page.contains("Cleanup was refused"));
    assert!(page.contains("Do not create it again"));
    assert!(page.contains(&format!("/bear/{slug}/models")));
    assert!(!page.contains("<form method=\"post\""));
    let id = gateway.staged_bears()[0];
    assert!(bears_db::get_bear(&pool, id)
        .await
        .unwrap()
        .unwrap()
        .default_model
        .is_none());
    assert!(
        den_service::conversation::persistence::get_conversation_for_external_id(
            &pool,
            id,
            "conv-intervening-work"
        )
        .await
        .unwrap()
        .is_some()
    );
    assert!(models::list(&pool, id.into()).await.unwrap().is_empty());
    let calls = gateway.setup_calls();
    post_as(&app, &cookie, "/bears/new", &form).await;
    assert_eq!(gateway.setup_calls(), calls);
    assert_eq!(gateway.staged_bears().len(), 1);
    safe(&page);
}

#[sqlx::test(migrations = "../../migrations")]
async fn legacy_model_setup_available_creation_proposals_are_saved_only_after_new_key_validation(
    pool: PgPool,
) {
    let existing = create_test_bear(&pool, &fresh_slug()).await;
    let admin = create_bear_admin_user(&pool, existing).await;
    test_bifrost::seed_den_model(&pool, MODEL, Some(true)).await;
    let gateway = MockBifrost::start(&[MODEL]).await;
    let app = test_app_with_state(
        pool.clone(),
        test_state_with_config(pool.clone(), gateway.creation_config(&pool)),
    )
    .await;
    let cookie = login_cookie(&app, admin).await;
    for route in ["/bears/new", "/test-admin/bears/new"] {
        let slug = format!("staged-model-{}", Uuid::new_v4());
        let (status, page) = post_as(&app, &cookie, route, &format!("slug={slug}&name=Verified+Bear&description=&system_prompt=&default_model={MODEL}&grant_user_id={admin}&grant_role=admin")).await;
        assert!(
            matches!(status, StatusCode::SEE_OTHER | StatusCode::OK),
            "{status}: {page}"
        );
        let id = *gateway.staged_bears().last().unwrap();
        assert_eq!(
            bears_db::get_bear(&pool, id)
                .await
                .unwrap()
                .unwrap()
                .default_model
                .as_deref(),
            Some(MODEL)
        );
        let configurations = models::list(&pool, id.into()).await.unwrap();
        assert_eq!(configurations.len(), 1);
        assert_eq!(configurations[0].model_handle.as_str(), MODEL);
        assert_eq!(
            models::default_configuration_id(&pool, id.into())
                .await
                .unwrap(),
            Some(configurations[0].id)
        );
        assert_eq!(gateway.setup_calls().2, 0);
    }
    assert_eq!(gateway.staged_bears().len(), 2);
}

#[sqlx::test(migrations = "../../migrations")]
async fn legacy_model_setup_admin_json_creation_uses_new_bear_authenticated_check_too(
    pool: PgPool,
) {
    let existing = create_test_bear(&pool, &fresh_slug()).await;
    let admin = create_bear_admin_user(&pool, existing).await;
    test_bifrost::seed_den_model(&pool, MODEL, Some(true)).await;
    let gateway = MockBifrost::standard().await;
    let app = test_app_with_state(
        pool.clone(),
        test_state_with_config(pool.clone(), gateway.creation_config(&pool)),
    )
    .await;
    let cookie = login_cookie(&app, admin).await;
    let slug = format!("staged-model-{}", Uuid::new_v4());
    let response = app.oneshot(Request::builder().method("POST").uri("/test-admin-api/bears")
        .header(header::COOKIE, cookie).header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(json!({"slug":slug,"name":"API draft","description":"Purpose","system_prompt":"","default_model":MODEL}).to_string())).unwrap()).await.unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let body: Value = serde_json::from_slice(&body).unwrap();
    assert!(body["error"].as_str().unwrap().contains("model_missing"));
    assert!(body["saved_bear_id"].is_null());
    assert!(!bears_db::bear_slug_exists(&pool, &slug).await.unwrap());
    assert_eq!(gateway.setup_calls().2, 0);
    safe(&body.to_string());
}
