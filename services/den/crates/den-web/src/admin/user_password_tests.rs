use super::*;
use crate::admin::usability_tests::render;
use crate::auth_backend::Backend;
use axum::{
    body::Body,
    http::{header, Request, StatusCode},
};
use axum_login::{login_required, permission_required, tower_sessions::SessionManagerLayer};
use http_body_util::BodyExt;
use password_auth::verify_password;
use sqlx::PgPool;
use std::sync::Arc;
use tower::ServiceExt;
use tower_sessions_sqlx_store::PostgresStore;

fn assert_fields(html: &str) {
    for name in ["password", "password_check"] {
        let position = html.find(&format!("name=\"{name}\"")).unwrap();
        let start = html[..position].rfind("<input ").unwrap();
        let end = start + html[start..].find('>').unwrap();
        let field = &html[start..=end];
        for attribute in [
            "type=\"password\"",
            "autocomplete=\"new-password\"",
            "required",
            "minlength=\"8\"",
        ] {
            assert!(field.contains(attribute), "{field}");
        }
        assert!(!field.contains("value="));
    }
}

#[test]
fn admin_password_template_renders_real_parent_masking_constraints_and_field_errors() {
    let html = render(
        "admin/users/change_password.html",
        context! {
            id => 42, target => context! { username => "casey" },
            form => context! { password => "PRIVATE PASSWORD", password_check => "PRIVATE CONFIRMATION", errors => context! {
                password => [context! { message => "Use at least 8 characters.", params => context! { value => "PRIVATE PARAMETER" } }],
                password_check => [context! { message => "Passwords must match." }],
            } },
        },
    );
    assert!(html.starts_with("<!doctype html>"));
    assert!(html.contains("/assets/css/style.css"));
    assert!(html.contains("name=\"viewport\""));
    assert!(html.contains("Change password for casey"));
    assert!(html.contains("Password was not changed"));
    assert!(html.contains("Use at least 8 characters."));
    assert!(html.contains("Passwords must match."));
    assert!(html.contains("role=\"alert\""));
    assert!(html.contains("href=\"/admin/users/42\""));
    assert!(!html.contains("PRIVATE"));
    assert_fields(&html);
    let success = render(
        "admin/users/change_password.html",
        context! {
            id => 42, target => context! { username => "casey" }, form => context! {}, password_changed => true,
        },
    );
    assert!(success.contains("Password changed for casey."));
    assert!(success.contains("role=\"status\""));
}

#[test]
fn password_form_and_feedback_serialize_only_safe_fields() {
    let form = ChangePasswordForm {
        password: "PRIVATE1".into(),
        password_check: "PRIVATE2".into(),
    };
    let errors = form.validate().unwrap_err();
    let json =
        serde_json::to_string(&context! { form, errors => password_validation_messages(&errors) })
            .unwrap();
    assert!(json.contains("Passwords must match."));
    assert!(!json.contains("PRIVATE"));
    assert!(!json.contains("params"));
    let short = ChangePasswordForm {
        password: "short".into(),
        password_check: "short".into(),
    };
    assert!(short
        .validate()
        .unwrap_err()
        .field_errors()
        .contains_key("password"));
    let valid = ChangePasswordForm {
        password: "eight123".into(),
        password_check: "eight123".into(),
    };
    assert!(valid.validate().is_ok());
    assert_eq!(serde_json::to_value(valid).unwrap(), serde_json::json!({}));
}

async fn test_login(Path(id): Path<i32>, mut auth_session: AuthSession) -> StatusCode {
    let user = auth_session.backend.get_user(&id).await.unwrap().unwrap();
    auth_session.login(&user).await.unwrap();
    StatusCode::OK
}

async fn app(pool: PgPool) -> axum::Router {
    let store = PostgresStore::new(pool.clone());
    store.migrate().await.unwrap();
    let mut config = crate::config::Config::test_stub();
    config.templates_dir = format!("{}/src/templates", env!("CARGO_MANIFEST_DIR"));
    let env = crate::template_environment(&config);
    let state = AppState::test_with_template_env(pool.clone(), env, Arc::new(config));
    Router::new()
        .nest(
            "/admin",
            router().route_layer(permission_required!(Backend, login_url = "/login", "admin")),
        )
        .merge(
            Router::new()
                .route("/session-check", get(|| async { StatusCode::OK }))
                .route_layer(login_required!(Backend, login_url = "/login")),
        )
        .route("/test-login/{id}", get(test_login))
        .with_state(state)
        .layer(
            axum_login::AuthManagerLayerBuilder::new(
                Backend::new(pool),
                SessionManagerLayer::new(store),
            )
            .build(),
        )
}

async fn create_user(pool: &PgPool, username: &str, admin: bool) -> i32 {
    let email = format!("{username}@example.test");
    let hash = generate_hash("old-private-password");
    sqlx::query_scalar!(
        "INSERT INTO users (username, email, display_name, passhash, is_admin) VALUES ($1, $2, $1, $3, $4) RETURNING id",
        username, email, hash, admin
    ).fetch_one(pool).await.unwrap()
}

fn cookie(response: &Response, previous: &str) -> String {
    response
        .headers()
        .get(header::SET_COOKIE)
        .map(|value| {
            value
                .to_str()
                .unwrap()
                .split(';')
                .next()
                .unwrap()
                .to_string()
        })
        .unwrap_or_else(|| previous.to_string())
}

async fn login(app: &axum::Router, id: i32) -> String {
    let response = request(app, "", &format!("/test-login/{id}"), None).await;
    assert_eq!(response.status(), StatusCode::OK);
    cookie(&response, "")
}

async fn request(app: &axum::Router, cookie: &str, path: &str, form: Option<&str>) -> Response {
    let mut request = Request::builder().uri(path);
    if !cookie.is_empty() {
        request = request.header(header::COOKIE, cookie);
    }
    let body = if let Some(form) = form {
        request = request
            .method("POST")
            .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded");
        Body::from(form.to_string())
    } else {
        Body::empty()
    };
    app.clone()
        .oneshot(request.body(body).unwrap())
        .await
        .unwrap()
}

async fn html(response: Response) -> String {
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

async fn hash(pool: &PgPool, id: i32) -> String {
    user_db::get_user_by_id(pool, id)
        .await
        .unwrap()
        .unwrap()
        .passhash
}

#[sqlx::test(migrations = "../../migrations")]
async fn operator_password_route_validates_persists_and_announces_once_without_secrets(
    pool: PgPool,
) {
    let operator = create_user(&pool, "passwordoperator", true).await;
    let target = create_user(&pool, "passwordtarget", false).await;
    let app = app(pool.clone()).await;
    let mut operator_cookie = login(&app, operator).await;
    let target_cookie = login(&app, target).await;
    let path = format!("/admin/users/{target}/change_password");
    let original = hash(&pool, target).await;
    for (form, message, secrets) in [
        (
            "password=short&password_check=short",
            "Use at least 8 characters.",
            ["short", "short"],
        ),
        (
            "password=new-private-password&password_check=other-private-password",
            "Passwords must match.",
            ["new-private-password", "other-private-password"],
        ),
    ] {
        let response = request(&app, &operator_cookie, &path, Some(form)).await;
        assert_eq!(response.status(), StatusCode::OK);
        operator_cookie = cookie(&response, &operator_cookie);
        let page = html(response).await;
        assert!(page.contains("Password was not changed"));
        assert!(page.contains(message));
        assert_fields(&page);
        for secret in secrets {
            assert!(!page.contains(secret));
        }
        assert!(!page.contains(&original));
        assert_eq!(hash(&pool, target).await, original);
    }
    let response = request(
        &app,
        &operator_cookie,
        &path,
        Some("password=new-private-password&password_check=new-private-password"),
    )
    .await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert_eq!(response.headers()[header::LOCATION], path);
    operator_cookie = cookie(&response, &operator_cookie);
    assert!(!html(response).await.contains("new-private-password"));
    let saved = hash(&pool, target).await;
    assert!(verify_password("new-private-password", &saved).is_ok());
    assert!(verify_password("old-private-password", &saved).is_err());
    let unrelated = request(
        &app,
        &operator_cookie,
        &format!("/admin/users/{operator}/change_password"),
        None,
    )
    .await;
    assert_eq!(unrelated.status(), StatusCode::OK);
    assert!(!html(unrelated).await.contains("Password changed for"));
    let response = request(&app, &operator_cookie, &path, None).await;
    assert_eq!(response.status(), StatusCode::OK);
    operator_cookie = cookie(&response, &operator_cookie);
    let page = html(response).await;
    assert!(page.contains("Password changed for passwordtarget."));
    assert!(page.contains("role=\"status\""));
    assert!(!page.contains("new-private-password"));
    assert!(!page.contains(&saved));
    assert!(!page.contains(&original));
    let response = request(
        &app,
        &operator_cookie,
        &format!("{path}?password_changed=true"),
        None,
    )
    .await;
    assert!(!html(response).await.contains("Password changed for"));
    assert!(request(&app, &target_cookie, "/session-check", None)
        .await
        .status()
        .is_redirection());
    assert_eq!(
        request(&app, &operator_cookie, "/session-check", None)
            .await
            .status(),
        StatusCode::OK
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn operator_self_reset_keeps_current_session_and_password_route_rejects_non_operators(
    pool: PgPool,
) {
    let operator = create_user(&pool, "selfresetoperator", true).await;
    let member = create_user(&pool, "ordinaryuser", false).await;
    let app = app(pool.clone()).await;
    let mut current = login(&app, operator).await;
    let other = login(&app, operator).await;
    let member_cookie = login(&app, member).await;
    let path = format!("/admin/users/{operator}/change_password");
    let original = hash(&pool, operator).await;
    for cookie in ["", member_cookie.as_str()] {
        let response = request(
            &app,
            cookie,
            &path,
            Some("password=forbidden-password&password_check=forbidden-password"),
        )
        .await;
        assert!(!response.status().is_success());
        assert_eq!(hash(&pool, operator).await, original);
    }
    let response = request(
        &app,
        &current,
        &path,
        Some("password=self-private-password&password_check=self-private-password"),
    )
    .await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    current = cookie(&response, &current);
    let response = request(&app, &current, &path, None).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert!(html(response)
        .await
        .contains("Password changed for selfresetoperator."));
    assert!(request(&app, &other, "/session-check", None)
        .await
        .status()
        .is_redirection());
    assert!(verify_password("self-private-password", &hash(&pool, operator).await).is_ok());
}
