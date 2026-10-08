use super::super::usability_tests::{assert_password_inputs, render};
use super::*;
use axum::{
    body::Body,
    http::{header, Request, StatusCode},
    routing::get,
};
use axum_login::{tower_sessions::SessionManagerLayer, AuthUser};
use http_body_util::BodyExt;
use password_auth::verify_password;
use sqlx::postgres::PgPoolOptions;
use std::sync::Arc;
use tower::ServiceExt;
use tower_sessions_sqlx_store::PostgresStore;

#[test]
fn password_validation_has_actionable_messages_but_never_serializes_secrets() {
    for (password, confirmation, field, message) in [
        ("short", "short", "password", "Use at least 8 characters."),
        (
            "long-password",
            "other-password",
            "password_check",
            "Passwords must match.",
        ),
    ] {
        let form = ChangePasswordForm {
            password: password.into(),
            password_check: confirmation.into(),
        };
        let errors = form.validate().expect_err("invalid password form");
        let safe_errors = validation_messages(&errors);
        let json = serde_json::to_string(&context! { form, errors => safe_errors }).unwrap();
        assert!(json.contains(message));
        assert!(errors.field_errors().contains_key(field));
        assert!(!json.contains(password));
        assert!(!json.contains(confirmation));
        assert!(!json.contains("params"));
    }
    let valid = ChangePasswordForm {
        password: "eight123".into(),
        password_check: "eight123".into(),
    };
    assert!(valid.validate().is_ok());
    let value = serde_json::to_value(valid).unwrap();
    assert!(value.get("password").is_none());
    assert!(value.get("password_check").is_none());
}

#[test]
fn registration_serialization_preserves_non_secret_fields_and_consent_only() {
    let form = RegisterForm {
        invite_key: "invite_123".into(),
        username: "casey".into(),
        display_name: "Casey".into(),
        email: "casey@example.test".into(),
        password: "PRIVATE REGISTRATION PASSWORD".into(),
        password_check: "PRIVATE REGISTRATION CONFIRMATION".into(),
        terms: "on".into(),
    };
    let json = serde_json::to_value(form).unwrap();
    assert_eq!(json["username"], "casey");
    assert_eq!(json["terms"], "on");
    assert!(json.get("password").is_none());
    assert!(json.get("password_check").is_none());
    assert!(!json.to_string().contains("PRIVATE"));
    assert!(validate_terms_consent("on").is_ok());
    assert!(validate_terms_consent("").is_err());
    assert!(validate_terms_consent("off").is_err());
    assert!(validate_username_format("casey123").is_ok());
    assert!(validate_username_format("casey!").is_err());
    assert!(validate_username_format("casey name").is_err());
}

#[test]
fn token_views_keep_scope_use_and_revocation_with_truthful_expiration() {
    let now = time::OffsetDateTime::UNIX_EPOCH;
    let record = armature_tokens::ArmatureTokenListRow {
        id: Uuid::new_v4(),
        name: "My editor".into(),
        scopes: serde_json::json!(["chat"]),
        bear_id: Uuid::new_v4(),
        bear_slug: "atlas".into(),
        bear_name: "Atlas".into(),
        created_at: now,
        expires_at: None,
        last_used_at: None,
        revoked_at: None,
        token_type: "Editor".into(),
        scope_labels: vec!["chat".into()],
    };
    for (expired, revoked, expected) in [
        (false, false, "Active"),
        (true, false, "Expired"),
        (true, true, "Revoked"),
    ] {
        let mut record = record.clone();
        record.expires_at = expired.then_some(now);
        record.revoked_at = revoked.then_some(now);
        let token_id = record.id;
        let html = render(
            "account/view.html",
            context! {
                user => context! { created => 0 }, armature_tokens => [AccountTokenView::at(record, now)],
            },
        );
        assert!(html.contains(expected));
        assert!(html.contains("My editor"));
        assert!(html.contains("chat"));
        assert!(html.contains("Never"));
        assert_eq!(
            html.contains(&format!("/account/armature-tokens/{token_id}/revoke")),
            !revoked
        );
    }
}

async fn test_pool() -> Option<sqlx::PgPool> {
    let Ok(url) = std::env::var("DATABASE_URL") else {
        eprintln!("skipping account route tests: DATABASE_URL is not set");
        return None;
    };
    let pool = match PgPoolOptions::new()
        .max_connections(2)
        .acquire_timeout(std::time::Duration::from_secs(5))
        .connect(&url)
        .await
    {
        Ok(pool) => pool,
        Err(error) => {
            eprintln!("skipping account route tests: cannot connect: {error}");
            return None;
        }
    };
    sqlx::migrate!("../../migrations")
        .set_ignore_missing(true)
        .run(&pool)
        .await
        .expect("migrations");
    Some(pool)
}

async fn test_login(Path(user_id): Path<i32>, mut auth_session: AuthSession) -> StatusCode {
    let user = auth_session
        .backend
        .get_user(&user_id)
        .await
        .unwrap()
        .unwrap();
    auth_session.login(&user).await.unwrap();
    StatusCode::OK
}

async fn test_app(pool: sqlx::PgPool) -> axum::Router {
    let store = PostgresStore::new(pool.clone());
    store.migrate().await.expect("session store migration");
    let mut config = crate::config::Config::test_stub();
    config.templates_dir = format!("{}/src/templates", env!("CARGO_MANIFEST_DIR"));
    let env = crate::template_environment(&config);
    let state = AppState::test_with_template_env(pool.clone(), env, Arc::new(config));
    Router::new()
        .merge(crate::user::router())
        .merge(crate::public::router())
        .route("/test-login/{user_id}", get(test_login))
        .with_state(state)
        .layer(
            axum_login::AuthManagerLayerBuilder::new(
                Backend::new(pool),
                SessionManagerLayer::new(store),
            )
            .build(),
        )
}

fn update_cookie(response: &Response, previous: &str) -> String {
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

async fn login_cookie(app: &axum::Router, id: i32) -> String {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/test-login/{id}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let cookie = update_cookie(&response, "");
    assert!(!cookie.is_empty());
    cookie
}

async fn request(app: &axum::Router, cookie: &str, path: &str, body: Option<&str>) -> Response {
    let mut builder = Request::builder().uri(path);
    if !cookie.is_empty() {
        builder = builder.header(header::COOKIE, cookie);
    }
    let body = match body {
        Some(body) => {
            builder = builder
                .method("POST")
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded");
            Body::from(body.to_string())
        }
        None => Body::empty(),
    };
    app.clone()
        .oneshot(builder.body(body).unwrap())
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

async fn auth_hash(pool: &sqlx::PgPool, id: i32) -> String {
    let user = Backend::new(pool.clone())
        .get_user(&id)
        .await
        .unwrap()
        .unwrap();
    String::from_utf8(user.session_auth_hash().to_vec()).unwrap()
}

#[tokio::test]
async fn password_routes_reject_invalid_values_persist_success_refresh_session_and_flash_once() {
    let Some(pool) = test_pool().await else {
        return;
    };
    let unique = Uuid::new_v4().simple().to_string();
    let username = format!("pw{}", &unique[..28]);
    let old_password = "old-private-password";
    let new_password = "new-private-password";
    let id = user::db::create_user(
        &pool,
        &format!("{username}@example.test"),
        &username,
        "Password test",
        &generate_hash(old_password),
    )
    .await
    .unwrap();
    let app = test_app(pool.clone()).await;
    let mut cookie = login_cookie(&app, id).await;
    let other_cookie = login_cookie(&app, id).await;
    let original_hash = auth_hash(&pool, id).await;

    let response = request(&app, &cookie, "/account/password", None).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_password_inputs(&html(response).await, "new-password");

    for (body, message, secrets) in [
        (
            "password=short&password_check=short",
            "Use at least 8 characters.",
            ["short", "short"],
        ),
        (
            "password=new-private-password&password_check=other-private-password",
            "Passwords must match.",
            [new_password, "other-private-password"],
        ),
    ] {
        let response = request(&app, &cookie, "/account/password", Some(body)).await;
        assert_eq!(response.status(), StatusCode::OK);
        cookie = update_cookie(&response, &cookie);
        let page = html(response).await;
        assert!(page.contains("Password was not changed"));
        assert!(page.contains(message));
        assert_password_inputs(&page, "new-password");
        for secret in secrets {
            assert!(!page.contains(secret));
        }
        assert!(!page.contains(&original_hash));
        assert_eq!(auth_hash(&pool, id).await, original_hash);
    }

    let response = request(
        &app,
        &cookie,
        "/account/password",
        Some("password=new-private-password&password_check=new-private-password"),
    )
    .await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert_eq!(response.headers()[header::LOCATION], "/account");
    cookie = update_cookie(&response, &cookie);
    let saved_hash = auth_hash(&pool, id).await;
    assert_ne!(saved_hash, original_hash);
    assert!(verify_password(new_password, &saved_hash).is_ok());
    assert!(verify_password(old_password, &saved_hash).is_err());
    assert!(!html(response).await.contains(new_password));

    let response = request(&app, &cookie, "/account", None).await;
    assert_eq!(
        response.status(),
        StatusCode::OK,
        "current session must stay signed in after the password change"
    );
    cookie = update_cookie(&response, &cookie);
    let page = html(response).await;
    assert!(page.contains("Password changed. This session stays signed in"));
    assert!(page.contains("role=\"status\""));
    for secret in [old_password, new_password, &saved_hash, &original_hash] {
        assert!(!page.contains(secret));
    }
    let response = request(&app, &cookie, "/account", None).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert!(!html(response).await.contains("Password changed."));
    let other_response = request(&app, &other_cookie, "/account", None).await;
    assert!(
        other_response.status().is_redirection(),
        "other sessions must not remain authenticated"
    );
    assert!(other_response.headers()[header::LOCATION]
        .to_str()
        .unwrap()
        .starts_with("/login"));

    request(&app, &cookie, "/logout", None).await;
    user::db::delete_user_by_id(&pool, id).await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn registration_route_keeps_consent_and_non_secret_draft_on_validation_failure() {
    let Some(pool) = test_pool().await else {
        return;
    };
    let app = test_app(pool).await;
    let body = "invite_key=bad&username=casey%21&display_name=Draft&email=invalid&password=PRIVATE1&password_check=PRIVATE2";
    let response = request(&app, "", "/account/register", Some(body)).await;
    assert_eq!(response.status(), StatusCode::OK);
    let page = html(response).await;
    assert!(page.contains("Account was not created"));
    assert!(page.contains("Confirm the terms acknowledgement"));
    assert!(page.contains("value=\"Draft\""));
    assert!(page.contains("Passwords must match."));
    assert!(!page.contains("PRIVATE1"));
    assert!(!page.contains("PRIVATE2"));
    assert_password_inputs(&page, "new-password");
}

#[tokio::test]
async fn sign_in_settings_and_email_routes_preserve_safe_drafts_and_field_feedback() {
    let Some(pool) = test_pool().await else {
        return;
    };
    let unique = Uuid::new_v4().simple().to_string();
    let username = format!("fb{}", &unique[..28]);
    let id = user::db::create_user(
        &pool,
        &format!("{username}@example.test"),
        &username,
        "Feedback test",
        &generate_hash("private-feedback-password"),
    )
    .await
    .unwrap();
    let app = test_app(pool.clone()).await;

    let response = request(
        &app,
        "",
        "/login/password",
        Some("username=DraftUser&password=SECRET"),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let page = html(response).await;
    assert!(page.contains("value=\"DraftUser\""));
    assert!(page.contains("Enter your password (at least 8 characters)."));
    assert!(!page.contains("SECRET"));

    let cookie = login_cookie(&app, id).await;
    let response = request(
        &app,
        &cookie,
        "/settings/edit",
        Some("display_name=x&theme=dark&week_start_day=2"),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let page = html(response).await;
    assert!(page.contains("Settings were not saved"));
    assert!(page.contains("Use 3–100 characters."));
    assert!(page.contains("value=\"x\""));
    assert!(page.contains("value=\"dark\" selected"));
    assert!(page.contains("value=\"2\" selected"));

    let response = request(
        &app,
        &cookie,
        "/settings/email/edit",
        Some("email=draft-invalid-address"),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let page = html(response).await;
    assert!(page.contains("Email was not changed"));
    assert!(page.contains("value=\"draft-invalid-address\""));
    assert!(page.contains("Enter a valid email address."));
    let _ = request(&app, &cookie, "/logout", None).await;
    user::db::delete_user_by_id(&pool, id).await.unwrap();
}

#[tokio::test]
async fn public_404_route_does_not_return_query_secrets() {
    let pool = PgPoolOptions::new()
        .connect_lazy("postgresql://unused:unused@localhost/unused")
        .unwrap();
    let app = test_app_without_sessions(pool);
    let response = request(&app, "", "/missing?token=PRIVATE-QUERY", None).await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let page = html(response).await;
    assert!(page.contains("Page not found"));
    assert!(page.contains("href=\"/\""));
    assert!(!page.contains("PRIVATE-QUERY"));
}

fn test_app_without_sessions(pool: sqlx::PgPool) -> axum::Router {
    let mut config = crate::config::Config::test_stub();
    config.templates_dir = format!("{}/src/templates", env!("CARGO_MANIFEST_DIR"));
    let env = crate::template_environment(&config);
    crate::public::router().with_state(AppState::test_with_template_env(
        pool,
        env,
        Arc::new(config),
    ))
}
