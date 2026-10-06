//! Route tests for the bear web-policy settings actions (web sources /
//! approvals) and the resources (policy) view. Ported from the retired
//! `/bears/{id}` admin UI tests when those routes became redirects; the
//! handlers now live on `/bear/{slug}/…` and require a logged-in bear admin,
//! so each test seeds a user + membership and logs in through a test-only
//! route before exercising the real router.

use super::*;
mod backend_management;
mod portability;
use axum::{
    body::Body,
    http::{header, Request, StatusCode},
    response::IntoResponse,
    routing::get,
};
use axum_login::AuthnBackend;
use den_core::RuntimeContextLabel;
use den_runtime::{
    runtime::compaction_observability::RuntimeCompactionEvent,
    runtime::compaction_store::record_runtime_compaction_event,
    runtime_conversations::RuntimeCompactionTriggerKind,
};
use http_body_util::BodyExt;
use minijinja::Environment;
use sqlx::postgres::PgPoolOptions;
use std::sync::Arc;
use tower::ServiceExt;
use tower_sessions_sqlx_store::PostgresStore;

use crate::{auth_backend::Backend, config::Config};

static TEST_DB_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

#[tokio::test]
async fn shared_management_hubs_preserve_membership_and_review_authority() {
    let _guard = TEST_DB_LOCK.lock().await;
    let Some(pool) = test_pool().await else {
        return;
    };
    let own_slug = fresh_slug();
    let other_slug = fresh_slug();
    let own_bear = create_test_bear(&pool, &own_slug).await;
    let other_bear = create_test_bear(&pool, &other_slug).await;
    let admin_id = create_bear_admin_user(&pool, own_bear).await;
    let member_id = create_bear_user(&pool, own_bear, BEAR_ROLE_MEMBER).await;
    let _other_admin_id = create_bear_admin_user(&pool, other_bear).await;
    let app = test_app(pool).await;
    let admin_cookie = login_cookie(&app, admin_id).await;
    let member_cookie = login_cookie(&app, member_id).await;
    let (status, admin_page) = get_as(&app, &admin_cookie, "/reviews").await;
    assert_eq!(status, StatusCode::OK, "{admin_page}");
    assert!(admin_page.contains(&format!("/bear/{own_slug}/memory#review-queue")));
    assert!(!admin_page.contains(&other_slug));
    let (status, member_page) = get_as(&app, &member_cookie, "/reviews").await;
    assert_eq!(status, StatusCode::OK, "{member_page}");
    assert!(member_page.contains("No review access"));
    assert!(!member_page.contains(&format!("/bear/{own_slug}/memory#review-queue")));
    let (status, _) = get_as(&app, &admin_cookie, &format!("/reviews?bear={other_slug}")).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, connections) = get_as(&app, &member_cookie, "/connections").await;
    assert_eq!(status, StatusCode::OK, "{connections}");
    assert!(connections.contains(&format!("/bear/{own_slug}/connections")));
    assert!(!connections.contains(&other_slug));
}

#[test]
fn parses_tool_budget_multiplier_form_values() {
    assert_eq!(parse_tool_budget_multiplier_form_value("").unwrap(), None);
    assert_eq!(
        parse_tool_budget_multiplier_form_value("inherit").unwrap(),
        None
    );
    assert_eq!(
        parse_tool_budget_multiplier_form_value("1.5").unwrap(),
        Some(1.5)
    );
    assert!(parse_tool_budget_multiplier_form_value("0").is_err());
    assert!(parse_tool_budget_multiplier_form_value("11").is_err());
}

async fn test_pool() -> Option<sqlx::PgPool> {
    dotenvy::dotenv().ok();
    let Ok(url) = std::env::var("DATABASE_URL") else {
        eprintln!("skipping DB-backed settings route test: DATABASE_URL is not set");
        return None;
    };
    let pool = match PgPoolOptions::new()
        .max_connections(2)
        .acquire_timeout(std::time::Duration::from_secs(5))
        .connect(&url)
        .await
    {
        Ok(pool) => pool,
        Err(err) => {
            eprintln!(
                "skipping DB-backed settings route test: could not connect to DATABASE_URL: {err}"
            );
            return None;
        }
    };
    if let Err(err) = sqlx::migrate!("../../migrations")
        .set_ignore_missing(true)
        .run(&pool)
        .await
    {
        eprintln!("skipping DB-backed settings route test: migrations failed: {err}");
        return None;
    }
    Some(pool)
}

fn test_state(pool: sqlx::PgPool) -> AppState {
    let config = Arc::new(Config::test_stub());
    let mut template_env = crate::template_environment(config.as_ref());
    template_env
            .add_template("bear/settings/policy.html", "{{ message }} {{ web_sources | length }} {{ web_approvals | length }} {{ web_fetches | length }}{% for approval in web_approvals %} {{ approval.approved_by_user_label }}{% endfor %}")
            .expect("add test template");
    template_env
        .add_template("reviews.html", include_str!("../../templates/reviews.html"))
        .expect("add reviews template");
    template_env
        .add_template(
            "connections.html",
            include_str!("../../templates/connections.html"),
        )
        .expect("add connections template");
    template_env
        .add_template("base.html", "{% block content %}{% endblock %}")
        .expect("add base template");
    template_env
        .add_template(
            "bear/_manage.html",
            include_str!("../../templates/bear/_manage.html"),
        )
        .expect("add manage template");
    template_env
        .add_template(
            "bear/settings/_bear_nav.html",
            include_str!("../../templates/bear/settings/_bear_nav.html"),
        )
        .expect("add nav template");
    template_env
        .add_template(
            "bear/settings/overview.html",
            include_str!("../../templates/bear/settings/overview.html"),
        )
        .expect("add overview template");
    template_env
        .add_template(
            "bear/manage/identity.html",
            include_str!("../../templates/bear/manage/identity.html"),
        )
        .expect("add identity template");
    template_env
        .add_template(
            "bear/manage/tools.html",
            include_str!("../../templates/bear/manage/tools.html"),
        )
        .expect("add tools template");
    for (page, source) in [
        (
            "conversations",
            include_str!("../../templates/bear/settings/conversations.html"),
        ),
        (
            "conversation",
            include_str!("../../templates/bear/settings/conversation.html"),
        ),
    ] {
        template_env
            .add_template_owned(format!("bear/settings/{page}.html"), source)
            .expect("add activity template");
    }
    for (page, source) in [
        (
            "advanced",
            include_str!("../../templates/bear/settings/advanced.html"),
        ),
        (
            "models",
            include_str!("../../templates/bear/settings/models.html"),
        ),
        (
            "context",
            include_str!("../../templates/bear/settings/context.html"),
        ),
    ] {
        template_env
            .add_template_owned(format!("bear/settings/{page}.html"), source)
            .expect("add settings template");
    }
    template_env
        .add_template("bear/settings/reflections.html", "reflections admin page")
        .expect("add inspection test template");
    AppState::test_with_template_env(pool, template_env, config)
}

/// Test-only login endpoint: establishes an axum-login session for the given
/// user id so requests carrying the returned cookie hit the real handlers as
/// that user.
async fn test_login(
    axum::extract::Path(user_id): axum::extract::Path<i32>,
    mut auth_session: AuthSession,
) -> impl IntoResponse {
    let user = auth_session
        .backend
        .get_user(&user_id)
        .await
        .expect("load login user")
        .expect("login user exists");
    auth_session.login(&user).await.expect("login");
    StatusCode::OK
}

async fn test_app(pool: sqlx::PgPool) -> axum::Router {
    let store = PostgresStore::new(pool.clone());
    store.migrate().await.expect("session store migration");
    Router::new()
        .merge(router())
        .merge(super::super::manage::router())
        .merge(crate::management_hub::router())
        .merge(crate::connections::router())
        .merge(super::super::skills::router())
        .route("/test-login/{user_id}", get(test_login))
        .with_state(test_state(pool.clone()))
        .layer(
            axum_login::AuthManagerLayerBuilder::new(
                Backend::new(pool),
                axum_login::tower_sessions::SessionManagerLayer::new(store),
            )
            .build(),
        )
}

async fn login_cookie(app: &axum::Router, user_id: i32) -> String {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/test-login/{user_id}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("login response");
    assert_eq!(response.status(), StatusCode::OK);
    response
        .headers()
        .get(header::SET_COOKIE)
        .expect("session cookie set on login")
        .to_str()
        .expect("cookie is valid string")
        .split(';')
        .next()
        .expect("cookie pair")
        .to_string()
}

async fn create_test_bear(pool: &sqlx::PgPool, slug: &str) -> Uuid {
    bears_db::create_bear(
        pool,
        bears_db::BearParams {
            slug,
            name: "Web Settings Test Bear",
            description: "",
            system_prompt: "System prompt",
            default_model: None,
            tools_enabled: None::<sqlx::types::Json<serde_json::Value>>,
            context_profile: None,
        },
    )
    .await
    .expect("create bear")
}

/// User with a verified email (the settings pages redirect unverified users)
/// and a membership on the given bear.
async fn create_bear_user(pool: &sqlx::PgPool, bear_id: Uuid, role: &str) -> i32 {
    let unique = Uuid::new_v4().simple().to_string();
    let email = format!("web-settings-{unique}@example.test");
    let username = format!("ws{}", &unique[..28]);
    let user_id = sqlx::query_scalar!(
        r#"
            INSERT INTO users (email, username, display_name, passhash)
            VALUES ($1, $2, $3, $4)
            RETURNING id
            "#,
        email,
        username,
        "Admin Display",
        "test-passhash"
    )
    .fetch_one(pool)
    .await
    .expect("create user");
    sqlx::query!(
        r#"
            INSERT INTO email_configs (user_id, email_address, active, verified_at)
            VALUES ($1, $2, true, now())
            "#,
        user_id,
        format!("web-settings-{unique}@example.test")
    )
    .execute(pool)
    .await
    .expect("verify email");
    bears_db::grant_membership(pool, user_id, bear_id, Some(role))
        .await
        .expect("grant bear membership");
    user_id
}

async fn create_bear_admin_user(pool: &sqlx::PgPool, bear_id: Uuid) -> i32 {
    create_bear_user(pool, bear_id, BEAR_ROLE_ADMIN).await
}

fn fresh_slug() -> String {
    format!("web-settings-{}", Uuid::new_v4())
}

async fn get_as(app: &axum::Router, cookie: &str, uri: &str) -> (StatusCode, String) {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(uri)
                .header(header::COOKIE, cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("settings GET response");
    let status = response.status();
    let body = response.into_body().collect().await.unwrap().to_bytes();
    (status, String::from_utf8_lossy(&body).into_owned())
}

#[tokio::test]
async fn retired_profile_routes_are_not_registered() {
    let pool = PgPoolOptions::new()
        .connect_lazy("postgres://unused:unused@localhost/unused")
        .unwrap();
    let app = router().with_state(test_state(pool));
    for (method, path) in [
        ("GET", "stances/chat"),
        ("GET", "profiles/pair"),
        ("POST", "stances/chat/model"),
        ("POST", "profiles/pair/model"),
        ("POST", "provision-missing-stances"),
        ("POST", "provision-missing-profiles"),
        ("POST", "provision-missing-roles"),
    ] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(format!("/bear/retired/{path}"))
                    .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                    .body(Body::from("model=malicious&model_custom=malicious"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND, "{method} {path}");
    }
    for path in ["stances", "profiles"] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/bear/retired/{path}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::PERMANENT_REDIRECT);
        assert_eq!(
            response.headers()[header::LOCATION],
            "/bear/retired/advanced"
        );
    }
}

#[test]
fn context_separates_bound_components_from_older_reference_snapshots() {
    let mut env = Environment::new();
    env.add_template("bear/_manage.html", "{% block manage %}{% endblock %}")
        .unwrap();
    env.add_template(
        "context",
        include_str!("../../templates/bear/settings/context.html"),
    )
    .unwrap();
    let body = env
        .get_template("context")
        .unwrap()
        .render(context! {
            bear => json!({"slug": "test", "name": "Test"}),
            compiled_bound_prompts => vec![CompiledRolePromptRow {
                role: "Bear base".into(), prompt_preview: "BOUND BASE".into(), char_count: 10,
            }],
            compiled_roles => vec![CompiledRolePromptRow {
                role: "pair".into(), prompt_preview: "OLD PAIR REFERENCE".into(), char_count: 18,
            }],
        })
        .unwrap();
    assert!(body.contains("Bear base and modes"));
    assert!(body.contains("BOUND BASE"));
    assert!(body.contains("Older stance reference snapshots"));
    assert!(body.contains("OLD PAIR REFERENCE"));
    assert!(body.contains("/bear/test/hats"));
    assert!(!body.contains("/stances/"));
    assert!(!body.contains("/profiles/"));
}

async fn historical_profile_settings(
    pool: &sqlx::PgPool,
    bear_id: Uuid,
) -> Vec<(String, Option<String>, Option<String>)> {
    bears_db::list_profile_model_settings(pool, bear_id)
        .await
        .unwrap()
        .into_iter()
        .map(|row| (row.profile, row.model, row.agent_loop_control_level))
        .collect()
}

#[tokio::test]
async fn bear_defaults_save_preserves_historical_profile_overrides_and_ignores_old_inputs() {
    let _guard = TEST_DB_LOCK.lock().await;
    let Some(pool) = test_pool().await else {
        return;
    };
    let slug = fresh_slug();
    let bear_id = create_test_bear(&pool, &slug).await;
    let admin_id = create_bear_admin_user(&pool, bear_id).await;
    for profile in RuntimeContextLabel::ALL {
        bears_db::set_profile_model_setting(&pool, bear_id, profile, Some("historical/model"))
            .await
            .unwrap();
        bears_db::set_profile_agent_loop_control_setting(
            &pool,
            bear_id,
            profile,
            Some(AgentLoopControlLevel::Strict),
        )
        .await
        .unwrap();
    }
    let before = historical_profile_settings(&pool, bear_id).await;
    let app = test_app(pool.clone()).await;
    let cookie = login_cookie(&app, admin_id).await;
    for path in [
        "stances/chat/model",
        "profiles/pair/model",
        "provision-missing-stances",
        "provision-missing-profiles",
        "provision-missing-roles",
    ] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/bear/{slug}/{path}"))
                    .header(header::COOKIE, &cookie)
                    .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                    .body(Body::from("model=inherit&model_custom=malicious"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            StatusCode::NOT_FOUND,
            "removed POST {path}"
        );
        assert_eq!(historical_profile_settings(&pool, bear_id).await, before);
        assert!(bears_db::list_bear_profile_bindings(&pool, bear_id)
            .await
            .unwrap()
            .is_empty());
    }
    for extra in ["", "&chat_model=malicious&pair_model_custom=malicious&curate_loop_control=invalid&work_model=inherit&watch_loop_control=light"] {
        let response = app.clone().oneshot(
            Request::builder().method("POST").uri(format!("/bear/{slug}/models"))
                .header(header::COOKIE, &cookie)
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(Body::from(format!("bear_default_model=inherit&bear_loop_control=careful&bear_tool_budget_multiplier=1.5{extra}"))).unwrap(),
        ).await.unwrap();
        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        assert!(response.headers()[header::LOCATION].to_str().unwrap().contains("message="));
        assert_eq!(historical_profile_settings(&pool, bear_id).await, before);
        let bear = bears_db::get_bear(&pool, bear_id).await.unwrap().unwrap();
        assert_eq!(bear.default_model, None);
        assert_eq!(bear.default_tool_budget_multiplier, Some(1.5));
        assert_eq!(bears_db::bear_agent_loop_control_setting(&pool, bear_id).await.unwrap(), Some(AgentLoopControlLevel::Careful));
    }
    let (status, body) = get_as(&app, &cookie, &format!("/bear/{slug}/models")).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body.contains("bear_default_model"));
    assert!(body.contains("Bifrost usage"));
    assert!(!body.contains("Stance defaults"));
    assert!(!body.contains("Configure stance"));
    assert!(!body.contains("name=\"pair_model\""));
    assert!(!body.contains("historical/model"));
}

#[tokio::test]
async fn admin_settings_gets_do_not_create_profile_registrations() {
    let _guard = TEST_DB_LOCK.lock().await;
    let Some(pool) = test_pool().await else {
        return;
    };
    let slug = fresh_slug();
    let bear_id = create_test_bear(&pool, &slug).await;
    let admin_id = create_bear_admin_user(&pool, bear_id).await;
    assert!(bears_db::list_bear_profile_bindings(&pool, bear_id)
        .await
        .unwrap()
        .is_empty());
    let app = test_app(pool.clone()).await;
    let cookie = login_cookie(&app, admin_id).await;
    for path in ["overview", "advanced", "context", "models"] {
        let (status, body) = get_as(&app, &cookie, &format!("/bear/{slug}/{path}")).await;
        assert_eq!(status, StatusCode::OK, "{path}: {body}");
        assert!(!body.contains("Stance bindings"));
        assert!(!body.contains("/stances/"));
        assert!(
            bears_db::list_bear_profile_bindings(&pool, bear_id)
                .await
                .unwrap()
                .is_empty(),
            "GET {path} registered profiles"
        );
    }
}

#[tokio::test]
async fn inspection_gets_require_bear_admin_but_overview_remains_member_viewable() {
    let _guard = TEST_DB_LOCK.lock().await;
    let Some(pool) = test_pool().await else {
        return;
    };
    let slug = fresh_slug();
    let bear_id = create_test_bear(&pool, &slug).await;
    let admin_id = create_bear_admin_user(&pool, bear_id).await;
    let member_id = create_bear_user(&pool, bear_id, BEAR_ROLE_MEMBER).await;
    let external_id = format!("settings-test-{}", Uuid::new_v4());
    let conversation = conversation_persistence::ensure_conversation_for_external_id(
        &pool,
        bear_id,
        Some(admin_id),
        &external_id,
        None,
        Some("private conversation title"),
    )
    .await
    .expect("persist conversation");
    let other_bear = create_test_bear(&pool, &fresh_slug()).await;
    let other_conversation = conversation_persistence::ensure_conversation_for_external_id(
        &pool,
        other_bear,
        None,
        &external_id,
        None,
        Some("other Bear's conversation"),
    )
    .await
    .expect("persist colliding conversation");
    record_runtime_compaction_event(
        &pool,
        &RuntimeCompactionEvent {
            conversation_id: external_id,
            trigger: RuntimeCompactionTriggerKind::Manual,
            policy_version: "legacy-test".to_string(),
            status: RuntimeCompactionEventStatus::Failed,
            boundary: None,
            source_group_start: None,
            source_group_end: None,
            artifact: None,
            diagnostic: Some("other Bear's private compaction diagnostic".to_string()),
        },
    )
    .await
    .expect("persist legacy compaction event");
    let app = test_app(pool.clone()).await;
    let admin_cookie = login_cookie(&app, admin_id).await;
    let member_cookie = login_cookie(&app, member_id).await;

    let (status, body) = get_as(&app, &member_cookie, &format!("/bear/{slug}/overview")).await;
    assert_eq!(status, StatusCode::OK, "member overview: {body}");
    for forbidden in [
        "private conversation title",
        "Recent activity",
        "Pending proposals",
        "Stance-local",
        "role health",
        "Health",
        "Activity over time",
        "Recall status",
        "Pending observations",
        "Week of",
        &format!("/bear/{slug}/activity"),
        &format!("/bear/{slug}/reflections"),
        &format!("/bear/{slug}/context"),
        &format!("/bear/{slug}/advanced"),
    ] {
        assert!(
            !body.contains(forbidden),
            "member overview exposed {forbidden}"
        );
    }
    assert!(body.contains(&format!("/bear/{slug}/memory")));
    let (status, body) = get_as(&app, &admin_cookie, &format!("/bear/{slug}/overview")).await;
    assert_eq!(status, StatusCode::OK, "admin overview: {body}");
    for expected in [
        "Recent activity",
        "private conversation title",
        "Conversation activity over time",
        "Week of",
        "Memory",
    ] {
        assert!(
            body.contains(expected),
            "admin overview missing {expected}: {body}"
        );
    }
    assert!(body.contains("Derived search:") || body.contains("Memory statistics unavailable."));
    assert!(body.contains(&format!("/bear/{slug}/activity")));

    let hat = hats::create_hat(
        &pool,
        BearId::new(bear_id),
        den_core::ids::UserId::new(admin_id),
        "Security review",
        "Admin-only longer purpose",
    )
    .await
    .unwrap();
    hats::manage::set_short_summary(
        &pool,
        BearId::new(bear_id),
        hat.id,
        Some("Reviews <repository>"),
        true,
    )
    .await
    .unwrap();
    let (status, body) = get_as(&app, &member_cookie, &format!("/bear/{slug}/identity")).await;
    assert_eq!(status, StatusCode::OK, "member identity: {body}");
    assert!(body.contains("Security review"));
    assert!(body.contains("Reviews &lt;repository&gt;"));
    assert!(!body.contains("Admin-only longer purpose"));
    assert!(!body.contains("<h3>Stances</h3>"));
    assert!(!body.contains(&format!("/bear/{slug}/stances/")));
    let (status, body) = get_as(&app, &admin_cookie, &format!("/bear/{slug}/identity")).await;
    assert_eq!(status, StatusCode::OK, "admin identity: {body}");
    assert!(body.contains(&format!("/bear/{slug}/hats/{}", hat.id)));
    assert!(!body.contains("<h3>Stances</h3>"));
    let (status, tools) = get_as(&app, &member_cookie, &format!("/bear/{slug}/tools")).await;
    assert_eq!(status, StatusCode::OK, "member tools: {tools}");
    assert!(tools.contains("<th>Job run</th>"));
    assert!(tools.contains("<th>Editor</th>"));
    assert!(!tools.contains("<th>pair</th>"));
    assert!(!tools.contains("not yet available in this build"));

    for path in [
        "activity".to_string(),
        "conversations".to_string(),
        format!("conversations/{}", conversation.id),
        "reflections".to_string(),
        "context".to_string(),
        "advanced".to_string(),
    ] {
        let uri = format!("/bear/{slug}/{path}");
        let (status, body) = get_as(&app, &member_cookie, &uri).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "member {uri}: {body}");
        let (status, body) = get_as(&app, &admin_cookie, &uri).await;
        assert_eq!(status, StatusCode::OK, "admin {uri}: {body}");
        if path == "activity" || path.starts_with("conversations") {
            assert!(body.contains("Legacy compaction"), "admin {uri}: {body}");
            assert!(
                !body.contains("other Bear's private compaction diagnostic"),
                "admin {uri}: {body}"
            );
            assert!(!body.contains("legacy-test"), "admin {uri}: {body}");
        }
    }
    let (status, body) = get_as(
        &app,
        &admin_cookie,
        &format!("/bear/{slug}/conversations/{}", other_conversation.id),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "cross-Bear UUID: {body}");
}

#[tokio::test]
async fn add_web_source_route_normalizes_host_and_flashes() {
    let _guard = TEST_DB_LOCK.lock().await;
    let Some(pool) = test_pool().await else {
        return;
    };
    let slug = fresh_slug();
    let bear_id = create_test_bear(&pool, &slug).await;
    let user_id = create_bear_admin_user(&pool, bear_id).await;
    let app = test_app(pool.clone()).await;
    let cookie = login_cookie(&app, user_id).await;

    let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/bear/{slug}/web-sources"))
                    .header(header::COOKIE, &cookie)
                    .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                    .body(Body::from("scope_kind=host&scope_value=Example.COM%3A8443.&policy=preferred&label=Docs&priority=10"))
                    .unwrap(),
            )
            .await
            .expect("add source response");

    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert!(response
        .headers()
        .get(header::LOCATION)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .contains("message=Web%20source%20saved"));
    let stored: String = sqlx::query_scalar!(
        "SELECT scope_value FROM bear_web_sources WHERE bear_id = $1 AND scope_kind = 'host'",
        bear_id
    )
    .fetch_one(&pool)
    .await
    .expect("stored source");
    assert_eq!(stored, "example.com:8443");
}

#[tokio::test]
async fn add_web_source_route_rejects_url_in_host_scope() {
    let _guard = TEST_DB_LOCK.lock().await;
    let Some(pool) = test_pool().await else {
        return;
    };
    let slug = fresh_slug();
    let bear_id = create_test_bear(&pool, &slug).await;
    let user_id = create_bear_admin_user(&pool, bear_id).await;
    let app = test_app(pool.clone()).await;
    let cookie = login_cookie(&app, user_id).await;

    let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/bear/{slug}/web-sources"))
                    .header(header::COOKIE, &cookie)
                    .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                    .body(Body::from("scope_kind=host&scope_value=https%3A%2F%2Fexample.com%2Fdocs&policy=preferred&label=&priority=0"))
                    .unwrap(),
            )
            .await
            .expect("validation response");

    // Invalid input flashes the normalization error back to the resources page.
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    let location = response
        .headers()
        .get(header::LOCATION)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    let location = urlencoding::decode(&location).expect("decode location");
    assert!(
        location.contains("host must be a bare hostname"),
        "unexpected redirect: {location}"
    );
    let stored: i64 = sqlx::query_scalar!(
        "SELECT COUNT(*)::bigint AS \"count!: i64\" FROM bear_web_sources WHERE bear_id = $1",
        bear_id
    )
    .fetch_one(&pool)
    .await
    .expect("source count");
    assert_eq!(stored, 0);
}

#[tokio::test]
async fn add_and_revoke_web_approval_routes_update_active_approvals() {
    let _guard = TEST_DB_LOCK.lock().await;
    let Some(pool) = test_pool().await else {
        return;
    };
    let slug = fresh_slug();
    let bear_id = create_test_bear(&pool, &slug).await;
    let user_id = create_bear_admin_user(&pool, bear_id).await;
    let app = test_app(pool.clone()).await;
    let cookie = login_cookie(&app, user_id).await;

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/bear/{slug}/web-approvals"))
                .header(header::COOKIE, &cookie)
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(Body::from("scope_kind=host&scope_value=Docs.RS"))
                .unwrap(),
        )
        .await
        .expect("add approval response");
    let status = response.status();
    if status != StatusCode::SEE_OTHER {
        let body = response.into_body().collect().await.unwrap().to_bytes();
        panic!(
            "add approval: expected 303, got {status}: {}",
            String::from_utf8_lossy(&body)
        );
    }

    let approval_id: Uuid = sqlx::query_scalar!(
        "SELECT id FROM bear_web_approvals WHERE bear_id = $1 AND scope_value = 'docs.rs' AND revoked_at IS NULL",
        bear_id
    )
    .fetch_one(&pool)
        .await
        .expect("active approval");

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/bear/{slug}/web-approvals/{approval_id}/revoke"))
                .header(header::COOKIE, &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("revoke response");
    assert_eq!(response.status(), StatusCode::SEE_OTHER);

    let active_count: i64 = sqlx::query_scalar!(
        "SELECT COUNT(*)::bigint AS \"count!: i64\" FROM bear_web_approvals WHERE bear_id = $1 AND revoked_at IS NULL",
        bear_id
    )
    .fetch_one(&pool)
    .await
    .expect("approval count");
    assert_eq!(active_count, 0);
}

#[tokio::test]
async fn resources_view_displays_approval_user_label_and_recent_fetches() {
    let _guard = TEST_DB_LOCK.lock().await;
    let Some(pool) = test_pool().await else {
        return;
    };
    let slug = fresh_slug();
    let bear_id = create_test_bear(&pool, &slug).await;
    let user_id = create_bear_admin_user(&pool, bear_id).await;
    web_policy::record_web_approval(
        &pool,
        bear_id,
        "host",
        "example.com",
        Some(user_id),
        "admin",
        None,
    )
    .await
    .expect("record approval");
    web_policy::record_web_fetch_attempt(
        &pool,
        web_policy::WebFetchAuditParams {
            bear_id,
            session_id: Some("session-1"),
            tool_call_id: Some("tool-1"),
            url: "https://example.com/",
            final_url: None,
            host: "example.com",
            execution_location: "den",
            approval_kind: "user_host",
            http_status: Some(200),
            content_type: Some("text/html"),
            bytes: Some(123),
        },
    )
    .await
    .expect("record fetch");

    let app = test_app(pool.clone()).await;
    let cookie = login_cookie(&app, user_id).await;

    let response = app
        .oneshot(
            Request::builder()
                .uri(format!("/bear/{slug}/resources?message=Saved"))
                .header(header::COOKIE, &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("resources response");
    let status = response.status();
    let body = response.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(
        status,
        StatusCode::OK,
        "resources view: {}",
        String::from_utf8_lossy(&body)
    );
    let body = String::from_utf8_lossy(&body);
    assert!(body.contains("Saved"));
    assert!(body.contains("Admin Display"));
    assert!(body.contains("0 1 1"));
}
