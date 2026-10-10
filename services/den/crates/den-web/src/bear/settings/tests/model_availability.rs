use super::model_configurations::{assert_input_value, escaped_html, post_as};
use super::*;
use crate::test_bifrost::{self, MockBifrost, PROVIDER_BODY, VIRTUAL_KEY};
use den_service::bears::model_configurations as service;
use sqlx::PgPool;

const MODEL: &str = "openai/gpt-6-sol";

fn assert_safe(page: &str) {
    for private in [PROVIDER_BODY, VIRTUAL_KEY, "global-api-key-secret-CANARY"] {
        assert!(!page.contains(private));
    }
}

#[sqlx::test(migrations = "../../migrations")]
async fn model_availability_den_known_gateway_absent_cannot_save_select_or_override_but_remains_inspectable(
    pool: PgPool,
) {
    let slug = fresh_slug();
    let bear = create_test_bear(&pool, &slug).await;
    let admin = create_bear_admin_user(&pool, bear).await;
    test_bifrost::seed_den_model(&pool, MODEL, Some(true)).await;
    test_bifrost::seed_key(&pool, bear).await;
    let gateway = MockBifrost::start(&["openai/gpt-4.1"]).await;
    let state = test_state_with_config(pool.clone(), gateway.config());
    let app = test_app_with_state(pool.clone(), state).await;
    let cookie = login_cookie(&app, admin).await;
    let root = format!("/bear/{slug}/models");
    let stale = service::create(
        &pool,
        bear.into(),
        "Stored primary",
        MODEL,
        Some(den_core::ThinkingEffort::High),
    )
    .await
    .unwrap();
    let hat = hats::create_hat(
        &pool,
        bear.into(),
        den_core::ids::UserId::new(admin),
        "Test hat",
        "Gateway test",
    )
    .await
    .unwrap();
    let hat_model = format!("/bear/{slug}/hats/{}/model", hat.id);
    let (status, page) = post_as(
        &app,
        &cookie,
        &format!("{root}/configurations"),
        &format!("name=Preserved+draft&model_handle={MODEL}&thinking_effort=high"),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{page}");
    assert!(page.contains("model_missing"));
    assert_input_value(&page, "configuration-name-new", "Preserved draft");
    assert_input_value(&page, "configuration-model-new", MODEL);
    assert_safe(&page);
    assert_eq!(service::list(&pool, bear.into()).await.unwrap().len(), 1);
    let (status, page) = post_as(
        &app,
        &cookie,
        &format!("{root}/configurations/{}", stale.id),
        &format!("name=Unsaved+edit&model_handle={MODEL}&thinking_effort=medium"),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{page}");
    assert_input_value(
        &page,
        &format!("configuration-name-{}", stale.id),
        "Unsaved edit",
    );
    assert_safe(&page);
    let after = service::get(&pool, bear.into(), stale.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(after.name, "Stored primary");
    assert_eq!(after.thinking_effort, Some(den_core::ThinkingEffort::High));
    for uri in [format!("{root}/default"), hat_model.clone()] {
        let (status, page) = post_as(
            &app,
            &cookie,
            &uri,
            &format!("configuration_id={}", stale.id),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{uri}: {page}");
        assert!(page.contains("model_missing"));
        assert!(page.contains(&format!("value=\"{}\" selected disabled", stale.id)));
        assert_safe(&page);
    }
    assert_eq!(
        service::default_configuration_id(&pool, bear.into())
            .await
            .unwrap(),
        None
    );
    assert_eq!(
        service::hat_configuration_id(&pool, bear.into(), hat.id)
            .await
            .unwrap(),
        None
    );
    let (status, page) = post_as(
        &app,
        &cookie,
        &format!("/bear/{slug}/edit/configuration"),
        &format!("default_model={MODEL}"),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{page}");
    assert!(page.contains("model_missing"));
    assert!(
        page.contains(&format!(
            "value=\"{}\" selected disabled",
            escaped_html(MODEL)
        )),
        "{page}"
    );
    assert_eq!(
        service::default_configuration_id(&pool, bear.into())
            .await
            .unwrap(),
        None
    );
    assert_safe(&page);

    // Simulate a previously valid, now unavailable default/override without rewriting it.
    service::set_default(&pool, bear.into(), Some(stale.id))
        .await
        .unwrap();
    service::set_hat_override(&pool, bear.into(), hat.id, Some(stale.id))
        .await
        .unwrap();
    let (status, page) = get_as(&app, &cookie, &root).await;
    assert_eq!(status, StatusCode::OK, "{page}");
    assert!(page.contains("Stored primary"));
    assert!(page.contains("unavailable"));
    assert!(page.contains("model_missing"));
    let datalist = regex::Regex::new(r#"(?s)<datalist id="catalog-models">(.*?)</datalist>"#)
        .unwrap()
        .captures(&page)
        .unwrap();
    assert!(!datalist[1].contains(&escaped_html(MODEL)));
    assert!(datalist[1].contains(&escaped_html("openai/gpt-4.1")));
    assert!(!datalist[1].contains("Gateway label must not replace Den metadata"));
    assert_eq!(
        service::default_configuration_id(&pool, bear.into())
            .await
            .unwrap(),
        Some(stale.id)
    );
    assert_eq!(
        service::hat_configuration_id(&pool, bear.into(), hat.id)
            .await
            .unwrap(),
        Some(stale.id)
    );
    assert_safe(&page);
    // Inheritance is unavailable too; removing references must remain a repair operation.
    gateway.set_models(&[]);
    for uri in [format!("{root}/default"), hat_model] {
        assert_eq!(
            post_as(&app, &cookie, &uri, "configuration_id=").await.0,
            StatusCode::SEE_OTHER
        );
    }
    assert_eq!(
        service::default_configuration_id(&pool, bear.into())
            .await
            .unwrap(),
        None
    );
    assert_eq!(
        service::hat_configuration_id(&pool, bear.into(), hat.id)
            .await
            .unwrap(),
        None
    );
    assert_eq!(
        service::get(&pool, bear.into(), stale.id)
            .await
            .unwrap()
            .unwrap()
            .model_handle
            .as_str(),
        MODEL
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn model_availability_settings_key_rejection_and_outage_preserve_nonsecret_drafts_without_writes(
    pool: PgPool,
) {
    let slug = fresh_slug();
    let bear = create_test_bear(&pool, &slug).await;
    let admin = create_bear_admin_user(&pool, bear).await;
    test_bifrost::seed_key(&pool, bear).await;
    let gateway = MockBifrost::standard().await;
    let app = test_app_with_state(
        pool.clone(),
        test_state_with_config(pool.clone(), gateway.config()),
    )
    .await;
    let cookie = login_cookie(&app, admin).await;
    for (upstream, expected, code) in [
        (
            StatusCode::UNAUTHORIZED,
            StatusCode::CONFLICT,
            "virtual_key_rejected",
        ),
        (
            StatusCode::SERVICE_UNAVAILABLE,
            StatusCode::SERVICE_UNAVAILABLE,
            "catalog_unavailable",
        ),
    ] {
        gateway.set_status(upstream);
        let (status, page) = post_as(
            &app,
            &cookie,
            &format!("/bear/{slug}/models/configurations"),
            "name=Keep+my+draft&model_handle=openai%2Fgpt-5&thinking_effort=high",
        )
        .await;
        assert_eq!(status, expected, "{page}");
        assert!(page.contains(code));
        assert_input_value(&page, "configuration-name-new", "Keep my draft");
        assert_input_value(&page, "configuration-model-new", "openai/gpt-5");
        assert_safe(&page);
        assert!(service::list(&pool, bear.into()).await.unwrap().is_empty());
    }
}
