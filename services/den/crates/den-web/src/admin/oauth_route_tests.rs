//! Exercise the real server mount, including its Den-operator permission layer.

use axum::{
    body::Body,
    http::{header, Method, Request, StatusCode},
    Router,
};
use den_oauth::oauth::{db as oauth_db, OAuthScope};
use http_body_util::BodyExt;
use serde_json::json;
use std::{path::PathBuf, sync::Arc};
use tower::ServiceExt;
use tower_sessions_sqlx_store::PostgresStore;
use uuid::Uuid;

use crate::{config::Config, AppState};

struct TestApp {
    router: Router,
    sqlite_dir: PathBuf,
}
impl Drop for TestApp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.sqlite_dir);
    }
}

async fn app(pool: &sqlx::PgPool) -> TestApp {
    let sqlite_dir = std::env::temp_dir().join(format!("oauth-web-{}", Uuid::new_v4()));
    std::fs::create_dir(&sqlite_dir).unwrap();
    let mut config = Config::test_stub();
    config.templates_dir = format!("{}/src/templates", env!("CARGO_MANIFEST_DIR"));
    config.bear_sqlite_data_dir = sqlite_dir.to_string_lossy().into_owned();
    config.bifrost_base_url.clear();
    config.bifrost_management_url.clear();
    config.qdrant_url = None;
    let config = Arc::new(config);
    let state = AppState::test_with_template_env(
        pool.clone(),
        crate::template_environment(&config),
        config,
    );
    let store = PostgresStore::new(pool.clone());
    store.migrate().await.unwrap();
    let router = crate::server(state, store).await.unwrap();
    TestApp { router, sqlite_dir }
}

async fn user(pool: &sqlx::PgPool, operator: bool) -> i32 {
    let suffix = Uuid::new_v4().simple().to_string();
    let id = den_http::user::db::create_user(
        pool,
        &format!("{suffix}@example.test"),
        &format!("oauth{}", &suffix[..20]),
        "OAuth operator test",
        &password_auth::generate_hash("OAuth test password"),
    )
    .await
    .unwrap();
    sqlx::query!("UPDATE users SET is_admin = $1 WHERE id = $2", operator, id)
        .execute(pool)
        .await
        .unwrap();
    id
}

async fn cookie(app: &Router, pool: &sqlx::PgPool, id: i32) -> String {
    let username = den_http::user::db::get_username_by_id(pool, id)
        .await
        .unwrap()
        .unwrap();
    let response = request(
        app,
        None,
        Method::POST,
        "/login/password",
        format!(
            "username={}&password=OAuth+test+password",
            urlencoding::encode(&username)
        ),
    )
    .await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    response.headers()[header::SET_COOKIE]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_string()
}

async fn request(
    app: &Router,
    cookie: Option<&str>,
    method: Method,
    uri: &str,
    body: String,
) -> axum::response::Response {
    let mut request = Request::builder().method(method).uri(uri);
    if let Some(cookie) = cookie {
        request = request.header(header::COOKIE, cookie);
    }
    app.clone()
        .oneshot(
            request
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap()
}

async fn html(response: axum::response::Response) -> String {
    String::from_utf8(
        response
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes()
            .to_vec(),
    )
    .unwrap()
}

fn protected(response: &axum::response::Response) {
    assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    assert_eq!(response.headers()[header::REFERRER_POLICY], "no-referrer");
}

#[sqlx::test(migrations = "../../migrations")]
async fn actual_mount_denies_anonymous_and_non_operator_oauth_reads_and_mutations(
    pool: sqlx::PgPool,
) {
    let app = app(&pool).await;
    let member_id = user(&pool, false).await;
    let member = cookie(&app.router, &pool, member_id).await;
    let scope = OAuthScope::all()[0];
    let client_id = oauth_db::create_oauth_client(
        &pool,
        "protected-mount",
        None,
        "Protected client",
        &json!(["https://example.test/callback"]),
        &[scope],
        true,
    )
    .await
    .unwrap();
    let token_id = oauth_db::create_admin_access_token(
        &pool,
        "PRIVATE MOUNT TOKEN",
        client_id,
        member_id,
        &[scope],
        24,
    )
    .await
    .unwrap();
    let routes = [
        (Method::GET, "/admin/oauth_clients/".into()),
        (Method::GET, format!("/admin/oauth_clients/{client_id}")),
        (Method::GET, "/admin/oauth_tokens/".into()),
        (Method::GET, format!("/admin/oauth_tokens/{token_id}")),
        (Method::GET, "/admin/oauth_tokens/generate".into()),
        (Method::POST, "/admin/oauth_clients/add".into()),
        (
            Method::POST,
            format!("/admin/oauth_clients/{client_id}/regenerate_secret"),
        ),
        (
            Method::POST,
            format!("/admin/oauth_clients/{client_id}/toggle_trusted"),
        ),
        (
            Method::POST,
            format!("/admin/oauth_clients/{client_id}/deactivate"),
        ),
        (
            Method::POST,
            format!("/admin/oauth_tokens/{token_id}/revoke"),
        ),
        (Method::POST, "/admin/oauth_tokens/generate".into()),
    ];
    for (method, uri) in routes {
        for session in [None, Some(member.as_str())] {
            let response = request(&app.router, session, method.clone(), &uri, String::new()).await;
            assert!(
                matches!(
                    response.status(),
                    StatusCode::UNAUTHORIZED
                        | StatusCode::FORBIDDEN
                        | StatusCode::SEE_OTHER
                        | StatusCode::TEMPORARY_REDIRECT
                ),
                "{method} {uri}: {}",
                response.status()
            );
            if response.status().is_redirection() {
                assert!(response.headers()[header::LOCATION]
                    .to_str()
                    .unwrap()
                    .starts_with("/login"));
            }
            assert!(!html(response).await.contains("PRIVATE MOUNT TOKEN"));
        }
    }
    let client = oauth_db::get_oauth_client_by_id(&pool, client_id)
        .await
        .unwrap()
        .unwrap();
    assert!(client.active && !client.trusted);
    assert!(
        !oauth_db::get_oauth_token_by_id(&pool, token_id)
            .await
            .unwrap()
            .unwrap()
            .revoked
    );
    assert_eq!(
        oauth_db::list_all_access_tokens_with_context(&pool)
            .await
            .unwrap()
            .len(),
        1
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn client_credentials_are_operator_bound_one_use_and_never_in_locations(pool: sqlx::PgPool) {
    let app = app(&pool).await;
    let operator = cookie(&app.router, &pool, user(&pool, true).await).await;
    let other = cookie(&app.router, &pool, user(&pool, true).await).await;
    let scope = OAuthScope::all()[0].as_str();
    let response = request(
        &app.router,
        Some(&operator),
        Method::POST,
        "/admin/oauth_clients/add",
        format!(
            "name=One-time+client&redirect_uris=https%3A%2F%2Fexample.test%2Fcallback&scopes={}",
            urlencoding::encode(scope)
        ),
    )
    .await;
    protected(&response);
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    let location = response.headers()[header::LOCATION]
        .to_str()
        .unwrap()
        .to_string();
    assert!(!location.contains('?'));
    let client = oauth_db::list_oauth_clients(&pool).await.unwrap().remove(0);
    assert_eq!(location, format!("/admin/oauth_clients/{}", client.id));
    let index = request(
        &app.router,
        Some(&operator),
        Method::GET,
        "/admin/oauth_clients/",
        String::new(),
    )
    .await;
    assert_eq!(index.status(), StatusCode::OK);
    protected(&index);
    let index = html(index).await;
    assert!(index.contains("One-time client"));
    assert!(index.contains(&client.created_at.to_string()));
    assert_eq!(index.matches("class=\"bear-manage-nav\"").count(), 1);
    assert!(index.contains("name=\"viewport\""));
    let response = request(
        &app.router,
        Some(&other),
        Method::GET,
        &location,
        String::new(),
    )
    .await;
    assert!(!html(response).await.contains("Client Secret:</strong>"));
    let response = request(
        &app.router,
        Some(&operator),
        Method::GET,
        &location,
        String::new(),
    )
    .await;
    protected(&response);
    let page = html(response).await;
    let secret = page
        .split("<strong>Client Secret:</strong> <code>")
        .nth(1)
        .unwrap()
        .split("</code>")
        .next()
        .unwrap();
    assert!(!secret.is_empty());
    for action in ["toggle_trusted", "regenerate_secret", "deactivate"] {
        assert!(page.contains(&format!(
            "action=\"/admin/oauth_clients/{}/{action}\"",
            client.id
        )));
    }
    assert!(!page.contains("class=\"action-info\"></button>"));
    let revisit = html(
        request(
            &app.router,
            Some(&operator),
            Method::GET,
            &format!("{location}?created=true&client_secret=INJECTED_SECRET"),
            String::new(),
        )
        .await,
    )
    .await;
    assert!(!revisit.contains(secret));
    assert!(!revisit.contains("INJECTED_SECRET"));
    let response = request(
        &app.router,
        Some(&operator),
        Method::POST,
        &format!("{location}/regenerate_secret"),
        String::new(),
    )
    .await;
    protected(&response);
    assert_eq!(
        response.headers()[header::LOCATION].to_str().unwrap(),
        location
    );
    let regenerated = html(
        request(
            &app.router,
            Some(&operator),
            Method::GET,
            &location,
            String::new(),
        )
        .await,
    )
    .await;
    assert!(regenerated.contains("New Client Secret:"));
    assert!(!regenerated.contains(secret));
    let revisit = html(
        request(
            &app.router,
            Some(&operator),
            Method::GET,
            &location,
            String::new(),
        )
        .await,
    )
    .await;
    assert!(!revisit.contains("New Client Secret:"));
    let response = request(
        &app.router,
        Some(&operator),
        Method::POST,
        &format!("{location}/toggle_trusted"),
        String::new(),
    )
    .await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert!(
        oauth_db::get_oauth_client_by_id(&pool, client.id)
            .await
            .unwrap()
            .unwrap()
            .trusted
    );
    let response = request(
        &app.router,
        Some(&operator),
        Method::POST,
        &format!("{location}/deactivate"),
        String::new(),
    )
    .await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert!(oauth_db::get_oauth_client_by_id(&pool, client.id)
        .await
        .unwrap()
        .is_none());
}

#[sqlx::test(migrations = "../../migrations")]
async fn generation_validates_locally_preserves_drafts_and_uses_one_time_feedback(
    pool: sqlx::PgPool,
) {
    let app = app(&pool).await;
    let operator_id = user(&pool, true).await;
    let operator = cookie(&app.router, &pool, operator_id).await;
    let allowed = OAuthScope::all()[0];
    let disallowed = OAuthScope::all()[1].as_str();
    let client = oauth_db::create_oauth_client(
        &pool,
        "generation-local",
        None,
        "Local validation",
        &json!(["https://example.test/callback"]),
        &[allowed],
        true,
    )
    .await
    .unwrap();
    let invalid = format!(
        "client_id={client}&user_id={operator_id}&scopes={}&expires_in=24",
        urlencoding::encode(disallowed)
    );
    let response = request(
        &app.router,
        Some(&operator),
        Method::POST,
        "/admin/oauth_tokens/generate",
        invalid,
    )
    .await;
    protected(&response);
    assert_eq!(response.status(), StatusCode::OK);
    let page = html(response).await;
    crate::admin::usability_tests::assert_visible(&page, "Requested scopes exceed");
    for option in [client, operator_id] {
        assert!(page.contains(&format!("value=\"{option}\" selected")));
    }
    assert!(page.contains("value=\"24\""));
    assert!(oauth_db::list_all_access_tokens_with_context(&pool)
        .await
        .unwrap()
        .is_empty());
    for selection in [
        "client_id=missing&user_id=missing&expires_in=24",
        "client_id=2147483647&user_id=2147483647&expires_in=24",
    ] {
        let page = html(
            request(
                &app.router,
                Some(&operator),
                Method::POST,
                "/admin/oauth_tokens/generate",
                selection.into(),
            )
            .await,
        )
        .await;
        assert!(page.contains("Choose an existing active client"));
        assert!(page.contains("Choose an existing user"));
        assert!(page.contains("At least one scope"));
    }
    let response = request(
        &app.router,
        Some(&operator),
        Method::POST,
        "/admin/oauth_tokens/generate",
        format!(
            "client_id={client}&user_id={operator_id}&scopes={}&expires_in=24",
            urlencoding::encode(allowed.as_str())
        ),
    )
    .await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    protected(&response);
    assert_eq!(
        response.headers()[header::LOCATION],
        "/admin/oauth_tokens/generate"
    );
    let issued = oauth_db::list_all_access_tokens_with_context(&pool)
        .await
        .unwrap()
        .remove(0);
    let first = request(
        &app.router,
        Some(&operator),
        Method::GET,
        "/admin/oauth_tokens/generate",
        String::new(),
    )
    .await;
    protected(&first);
    assert!(html(first).await.contains(&issued.token));
    let second = html(
        request(
            &app.router,
            Some(&operator),
            Method::GET,
            "/admin/oauth_tokens/generate",
            String::new(),
        )
        .await,
    )
    .await;
    assert!(!second.contains(&issued.token));
    assert_eq!(
        oauth_db::list_all_access_tokens_with_context(&pool)
            .await
            .unwrap()
            .len(),
        1
    );
    let index = request(
        &app.router,
        Some(&operator),
        Method::GET,
        "/admin/oauth_tokens/",
        String::new(),
    )
    .await;
    protected(&index);
    assert_eq!(index.status(), StatusCode::OK);
    assert!(!html(index).await.contains(&issued.token));
    let inspected = request(
        &app.router,
        Some(&operator),
        Method::GET,
        &format!("/admin/oauth_tokens/{}", issued.token_id),
        String::new(),
    )
    .await;
    protected(&inspected);
    assert!(html(inspected).await.contains(&issued.token));
}

#[sqlx::test(migrations = "../../migrations")]
async fn actual_status_mount_projects_authentication_without_credentials(pool: sqlx::PgPool) {
    let app = app(&pool).await;
    for is_admin in [false, true] {
        let user_id = user(&pool, is_admin).await;
        let stored = den_http::user::db::get_user_by_id(&pool, user_id)
            .await
            .unwrap()
            .unwrap();
        let session = cookie(&app.router, &pool, user_id).await;
        let response = request(
            &app.router,
            Some(&session),
            Method::GET,
            "/status",
            String::new(),
        )
        .await;
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        let page = html(response).await;
        assert_eq!(page.matches("class=\"bear-manage-nav\"").count(), 1);
        assert!(page.contains("aria-label=\"Den management\""));
        assert_eq!(page.contains("href=\"/admin\""), is_admin);
        assert!(!page.contains(&stored.passhash));
        assert!(!page.contains("OAuth test password"));
    }
    let anonymous =
        html(request(&app.router, None, Method::GET, "/status", String::new()).await).await;
    assert!(!anonymous.contains("class=\"bear-manage-nav\""));
}
