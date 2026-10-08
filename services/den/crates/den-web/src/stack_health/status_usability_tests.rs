use crate::admin::usability_tests::{assert_visible, render};
use minijinja::context;

#[tokio::test]
async fn production_status_projection_preserves_the_session_rail_but_redacts_credentials() {
    use crate::{
        auth_backend::SessionUser,
        config::Config,
        stack_health::{CheckState, HealthCheck, StackHealthReport},
        status::{ModelRegistryStatus, StatusPayload},
        AppState,
    };
    use http_body_util::BodyExt;
    let mut config = Config::test_stub();
    config.templates_dir = format!("{}/src/templates", env!("CARGO_MANIFEST_DIR"));
    config.github_packages_token = "REGISTRY-SECRET".into();
    config.llm_api_key = "MODEL-SECRET".into();
    config.qdrant_url =
        Some("https://operator:URL-SECRET@example.test?api_key=QUERY-SECRET".into());
    let payload = StatusPayload {
        health: StackHealthReport { ok: false, checked_at: "recorded time".into(), checks: vec![HealthCheck {
            id: "test", label: "Recall", state: CheckState::Warn,
            detail: "request to https://operator:URL-SECRET@example.test?api_key=QUERY-SECRET failed; MODEL-SECRET".into(),
        }] },
        den_version: crate::build_info::snapshot(), ghcr_den: None, ghcr_config_note: None,
        ghcr_error: Some("HTTP 403 REGISTRY-SECRET".into()),
        model_registry: ModelRegistryStatus { report: den_llm::model_registry::gateway_compatibility_report(Vec::<String>::new()), gateway_error: None },
    };
    let mut sanitized = payload.clone();
    super::feedback::sanitize(&mut sanitized, &config);
    let json = serde_json::to_string(&sanitized).unwrap();
    for secret in [
        "REGISTRY-SECRET",
        "MODEL-SECRET",
        "URL-SECRET",
        "QUERY-SECRET",
    ] {
        assert!(!json.contains(secret));
    }
    assert!(json.contains("HTTP 403"));
    let pool = sqlx::postgres::PgPoolOptions::new()
        .connect_lazy("postgres://unused/unused")
        .unwrap();
    let config = std::sync::Arc::new(config);
    let state =
        AppState::test_with_template_env(pool, crate::template_environment(&config), config);
    let user = SessionUser::from(den_http::user::db::UserAuth {
        id: 7,
        username: "Trusted status user".into(),
        passhash: "PRIVATE PASSHASH".into(),
        is_admin: false,
        theme: "system".into(),
    });
    for session in [None, Some(&user)] {
        let response = super::render_page(&state, session, payload.clone()).unwrap();
        assert_eq!(
            response.headers()[axum::http::header::CACHE_CONTROL],
            "no-store"
        );
        let page = String::from_utf8(
            response
                .into_body()
                .collect()
                .await
                .unwrap()
                .to_bytes()
                .to_vec(),
        )
        .unwrap();
        assert_eq!(
            page.matches("class=\"bear-manage-nav\"").count(),
            usize::from(session.is_some())
        );
        assert!(!page.contains("PRIVATE PASSHASH"));
        for secret in [
            "REGISTRY-SECRET",
            "MODEL-SECRET",
            "URL-SECRET",
            "QUERY-SECRET",
        ] {
            assert!(!page.contains(secret));
        }
        assert_visible(&page, "HTTP 403");
    }
}

#[test]
fn optional_registry_setup_is_local_help_but_real_failures_stay_visible() {
    let configured_later = render(
        "status.html",
        context! {
            overall_ok => false, ghcr_note => "Set GITHUB_PACKAGES_TOKEN and GHCR_PACKAGES_OWNER",
            rows => [context! { state => "fail", label => "Gateway", detail => "REAL GATEWAY REPAIR" }],
        },
    );
    assert_visible(&configured_later, "REAL GATEWAY REPAIR");
    assert!(
        configured_later.find("Deployed vs GHCR").unwrap()
            < configured_later.find("Help: optional GHCR").unwrap()
    );
    assert!(
        configured_later.find("Help: optional GHCR").unwrap()
            < configured_later.find("GITHUB_PACKAGES_TOKEN").unwrap()
    );
    let failed = render(
        "status.html",
        context! {
            ghcr_error => "HTTP 403 ACTUAL REGISTRY FAILURE <unsafe>",
            model_registry => context! {
                            report => den_llm::model_registry::gateway_compatibility_report(Vec::<String>::new()),
                            gateway_error => "ACTUAL MODEL CATALOG FAILURE",
                        },
        },
    );
    assert_visible(&failed, "ACTUAL REGISTRY FAILURE");
    assert_visible(&failed, "ACTUAL MODEL CATALOG FAILURE");
    assert!(failed.contains("&lt;unsafe&gt;"));
    assert!(!failed.contains("Help: optional GHCR comparison"));
}
