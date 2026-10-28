use super::*;
use axum::{
    body::Body,
    http::{header, Request},
};
use axum_login::AuthnBackend;
use den_service::{
    bears::db::{self as bears_db, BEAR_ROLE_ADMIN, BEAR_ROLE_MEMBER},
    personal_memory::{CandidateState, PersonalFactState, PersonalMemoryInspection},
};
use http_body_util::BodyExt;
use sqlx::PgPool;
use std::{path::PathBuf, sync::Arc};
use tower::ServiceExt;
use tower_sessions_sqlx_store::PostgresStore;
use uuid::Uuid;

mod authority;
mod erasure;
mod lifecycle;
mod quota;
mod tokens;

const BASE: &str = "/bear/aboutyou/memory/about-you";
const OWNER_NAME: &str = "Alice<owner-only>";

struct TestDirectory(PathBuf);
impl Drop for TestDirectory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

struct Fixture {
    pool: PgPool,
    state: AppState,
    app: Router,
    bear_id: BearId,
    owner: UserId,
    other: UserId,
    admin: UserId,
    cookie: String,
    other_cookie: String,
    admin_cookie: String,
    _directory: TestDirectory,
}

async fn login(Path(id): Path<i32>, mut auth: AuthSession) -> StatusCode {
    let user = auth.backend.get_user(&id).await.unwrap().unwrap();
    auth.login(&user).await.unwrap();
    StatusCode::OK
}

async fn cookie(app: &Router, actor: UserId) -> String {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/test-login/{}", actor.get()))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    response.headers()[header::SET_COOKIE]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_owned()
}

async fn account(pool: &PgPool, bear_id: BearId, name: &str, role: &str) -> UserId {
    let id = den_http::user::db::create_user(
        pool,
        &format!("{name}@example.test"),
        name,
        name,
        "credential-canary-never-render",
    )
    .await
    .unwrap();
    den_http::user::email_settings::set_admin_email_verified(pool, id, true)
        .await
        .unwrap();
    bears_db::grant_membership(pool, id, bear_id.as_uuid(), Some(role))
        .await
        .unwrap();
    UserId::new(id)
}

async fn bear(pool: &PgPool, slug: &str) -> BearId {
    BearId::new(
        bears_db::create_bear(
            pool,
            bears_db::BearParams {
                slug,
                name: "About you Bear",
                description: "",
                system_prompt: "",
                default_model: None,
                tools_enabled: None,
                context_profile: None,
            },
        )
        .await
        .unwrap(),
    )
}

async fn fixture(pool: PgPool) -> Fixture {
    let bear_id = bear(&pool, "aboutyou").await;
    let owner = account(&pool, bear_id, "nameowner", BEAR_ROLE_MEMBER).await;
    let other = account(&pool, bear_id, "nameother", BEAR_ROLE_MEMBER).await;
    let admin = account(&pool, bear_id, "nameadmin", BEAR_ROLE_ADMIN).await;
    let directory =
        TestDirectory(std::env::temp_dir().join(format!("den-web-about-you-{}", Uuid::new_v4())));
    std::fs::create_dir(&directory.0).unwrap();
    let mut config = crate::config::Config::test_stub();
    config.bear_sqlite_data_dir = directory.0.to_str().unwrap().to_owned();
    config.templates_dir = format!("{}/src/templates", env!("CARGO_MANIFEST_DIR"));
    let config = Arc::new(config);
    let state = AppState::test_with_template_env(
        pool.clone(),
        crate::template_environment(&config),
        config,
    );
    let sessions = PostgresStore::new(pool.clone());
    sessions.migrate().await.unwrap();
    let app = super::super::memory::router()
        .route("/test-login/{id}", get(login))
        .with_state(state.clone())
        .layer(
            axum_login::AuthManagerLayerBuilder::new(
                crate::auth_backend::Backend::new(pool.clone()),
                axum_login::tower_sessions::SessionManagerLayer::new(sessions),
            )
            .build(),
        );
    let cookie = cookie(&app, owner).await;
    let other_cookie = self::cookie(&app, other).await;
    let admin_cookie = self::cookie(&app, admin).await;
    Fixture {
        pool,
        state,
        app,
        bear_id,
        owner,
        other,
        admin,
        cookie,
        other_cookie,
        admin_cookie,
        _directory: directory,
    }
}

struct Page {
    status: StatusCode,
    body: String,
    location: Option<String>,
}

async fn request(app: &Router, cookie: &str, method: &str, path: &str, form: &str) -> Page {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(method)
                .uri(path)
                .header(header::COOKIE, cookie)
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(Body::from(form.to_owned()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    assert_eq!(response.headers()[header::REFERRER_POLICY], "no-referrer");
    let status = response.status();
    let location = response
        .headers()
        .get(header::LOCATION)
        .map(|v| v.to_str().unwrap().to_owned());
    let body = String::from_utf8(
        response
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes()
            .to_vec(),
    )
    .unwrap();
    assert!(!body.contains("credential-canary-never-render"));
    Page {
        status,
        body,
        location,
    }
}

fn form(page: &str, id: &str, extra: &str) -> String {
    let marker = format!("<form id=\"{id}\"");
    let form = page
        .split_once(&marker)
        .unwrap_or_else(|| panic!("missing {id}: {page}"))
        .1
        .split("</form>")
        .next()
        .unwrap();
    let token = form
        .split_once("name=\"token\" value=\"")
        .unwrap()
        .1
        .split('"')
        .next()
        .unwrap();
    format!("token={token}&{extra}")
}

impl Fixture {
    async fn get(&self) -> Page {
        let page = request(&self.app, &self.cookie, "GET", BASE, "").await;
        assert_eq!(page.status, StatusCode::OK, "{}", page.body);
        page
    }

    async fn submit(&self, endpoint: &str, body: &str) -> Page {
        request(
            &self.app,
            &self.cookie,
            "POST",
            &format!("{BASE}/{endpoint}"),
            body,
        )
        .await
    }

    async fn save(&self, endpoint: &str, page: &Page, id: &str, extra: &str) {
        let response = self.submit(endpoint, &form(&page.body, id, extra)).await;
        assert_eq!(response.status, StatusCode::SEE_OTHER, "{}", response.body);
        assert_eq!(response.location.as_deref(), Some(BASE));
    }

    async fn inspect(&self) -> PersonalMemoryInspection {
        self.inspect_as(self.owner).await
    }

    async fn inspect_as(&self, actor: UserId) -> PersonalMemoryInspection {
        personal_memory::inspect_personal_memory(
            self.state.sqlx_pool(),
            self.state.memory_stores().unwrap(),
            self.bear_id,
            actor,
        )
        .await
        .unwrap()
    }

    async fn enable(&self) {
        let page = self.get().await;
        self.save("capture", &page, "capture", "acknowledged=true")
            .await;
        assert!(self.inspect().await.capture_enabled);
    }

    async fn stage(&self, name: &str) -> String {
        let page = self.get().await;
        self.save(
            "request",
            &page,
            "request-name",
            &format!("name={}", urlencoding::encode(name)),
        )
        .await;
        self.inspect().await.candidates[0].id().to_string()
    }

    async fn approve(&self, id: &str) {
        let page = self.get().await;
        self.save(
            "publish",
            &page,
            &format!("publish-{id}"),
            "acknowledged=true",
        )
        .await;
    }
}
