//! Fresh database tests exercise both production owners and the actual server mount.
use super::*;
use axum::{
    body::Body,
    http::{Method, Request},
    Router,
};
use den_core::{BearId, DenError};
use den_http::user::db as user_db;
use den_service::bears::db::{self as bears_db, BearParams};
use http_body_util::BodyExt;
use sqlx::PgPool;
use std::{path::PathBuf, sync::Arc};
use tower::ServiceExt;
use tower_sessions_sqlx_store::PostgresStore;

mod concurrency;
mod root_guard;
mod routes;

async fn user(pool: &PgPool, label: &str, operator: bool) -> UserId {
    let username: String = label.chars().filter(char::is_ascii_alphanumeric).collect();
    let password_hash = password_auth::generate_hash("Delete test password");
    // Migrations already seed this name; reuse it to prove that the name grants no privilege.
    let existing = if label == "admin" {
        user_db::get_user_by_username(pool, &username)
            .await
            .unwrap()
    } else {
        None
    };
    let id = if let Some(existing) = existing {
        user_db::set_user_passhash_by_id(pool, existing.id, &password_hash)
            .await
            .unwrap();
        existing.id
    } else {
        user_db::create_user(
            pool,
            &format!("{username}@example.test"),
            &username,
            label,
            &password_hash,
        )
        .await
        .unwrap()
    };
    sqlx::query!("UPDATE users SET is_admin = $1 WHERE id = $2", operator, id)
        .execute(pool)
        .await
        .unwrap();
    id.into()
}

async fn bear(pool: &PgPool, slug: &str, name: &str) -> BearId {
    bears_db::create_bear(
        pool,
        BearParams {
            slug,
            name,
            description: "Deletion guard test",
            system_prompt: "test",
            default_model: None,
            tools_enabled: None,
            context_profile: None,
        },
    )
    .await
    .unwrap()
    .into()
}

async fn grant(pool: &PgPool, user: UserId, bear: BearId, role: &str) {
    bears_db::grant_membership(pool, user.get(), bear.as_uuid(), Some(role))
        .await
        .unwrap();
}

async fn exists(pool: &PgPool, user: UserId) -> bool {
    user_db::get_username_by_id(pool, user.get())
        .await
        .unwrap()
        .is_some()
}

async fn role(pool: &PgPool, user: UserId, bear: BearId) -> Option<Option<String>> {
    bears_db::membership_role_for_user(pool, user.get(), bear.as_uuid())
        .await
        .unwrap()
}

async fn admins(pool: &PgPool, bear: BearId) -> i64 {
    bears_db::count_bear_admins(pool, bear.as_uuid())
        .await
        .unwrap()
}

struct TestApp {
    router: Router,
    sqlite_dir: PathBuf,
}

impl Drop for TestApp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.sqlite_dir);
    }
}

async fn app(pool: &PgPool) -> TestApp {
    let sqlite_dir = std::env::temp_dir().join(format!("delete-user-web-{}", Uuid::new_v4()));
    std::fs::create_dir(&sqlite_dir).unwrap();
    let mut config = crate::config::Config::test_stub();
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
    TestApp {
        router: crate::server(state, store).await.unwrap(),
        sqlite_dir,
    }
}

async fn request(app: &Router, cookie: &str, path: &str, form: Option<&str>) -> Response {
    let mut request = Request::builder().uri(path);
    if !cookie.is_empty() {
        request = request.header(header::COOKIE, cookie);
    }
    let body = if let Some(form) = form {
        request = request
            .method(Method::POST)
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

async fn login(app: &Router, pool: &PgPool, user: UserId) -> String {
    let username = user_db::get_username_by_id(pool, user.get())
        .await
        .unwrap()
        .unwrap();
    let form = format!(
        "username={}&password=Delete+test+password",
        urlencoding::encode(&username)
    );
    let response = request(app, "", "/login/password", Some(&form)).await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    response.headers()[header::SET_COOKIE]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_string()
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

fn token(html: &str) -> Uuid {
    let rest = html
        .split("name=\"confirmation_token\" value=\"")
        .nth(1)
        .unwrap();
    rest.split('"').next().unwrap().parse().unwrap()
}

async fn preview(app: &Router, cookie: &str, target: UserId) -> (String, Uuid) {
    let response = request(app, cookie, &format!("/admin/users/{target}/delete"), None).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    assert_eq!(response.headers()[header::REFERRER_POLICY], "no-referrer");
    let page = html(response).await;
    let token = token(&page);
    (page, token)
}

fn confirmation(token: Uuid) -> String {
    format!("confirm_delete=true&confirmation_token={token}")
}
