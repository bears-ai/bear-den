use super::*;
use axum::{
    body::Body,
    http::{header, Request, StatusCode},
};
use axum_login::AuthnBackend;
use den_memory::{BearMemoryStore, LogicalMemoryPath};
use den_service::bears::db::{self as bears_db, BEAR_ROLE_ADMIN, BEAR_ROLE_MEMBER};
use http_body_util::BodyExt;
use serde_json::json;
use sqlx::PgPool;
use std::sync::Arc;
use tower::ServiceExt;
use tower_sessions_sqlx_store::PostgresStore;
use uuid::Uuid;

mod authority_locks;
mod consistency;
mod export;
mod history;
mod personal_candidates;
mod routes;
mod session;
mod templates;

struct Fixture {
    pool: PgPool,
    state: AppState,
    app: Router,
    bear_id: BearId,
    actor: UserId,
    destination: UserId,
    member: UserId,
    alice: String,
    bob: String,
    private_record: String,
    cookie: String,
}

async fn login(Path(id): Path<i32>, mut auth: AuthSession) -> StatusCode {
    let user = auth.backend.get_user(&id).await.unwrap().unwrap();
    auth.login(&user).await.unwrap();
    StatusCode::OK
}

async fn cookie(app: &Router, user: UserId) -> String {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/test-login/{}", user.get()))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    response.headers()[header::SET_COOKIE]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_string()
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

async fn fixture(pool: PgPool) -> Fixture {
    let bear_id = BearId::new(
        bears_db::create_bear(
            &pool,
            bears_db::BearParams {
                slug: "identityreview",
                name: "Identity review",
                description: "",
                system_prompt: "",
                default_model: None,
                tools_enabled: None,
                context_profile: None,
            },
        )
        .await
        .unwrap(),
    );
    let actor = account(&pool, bear_id, "identityadmin", BEAR_ROLE_ADMIN).await;
    let destination = account(&pool, bear_id, "identitydestination", BEAR_ROLE_MEMBER).await;
    let member = account(&pool, bear_id, "identitymember", BEAR_ROLE_MEMBER).await;
    let mut config = crate::config::Config::test_stub();
    config.bear_sqlite_data_dir = std::env::temp_dir()
        .join(format!("den-web-identity-{}", Uuid::new_v4()))
        .to_string_lossy()
        .into_owned();
    let config = Arc::new(config);
    let mut env = crate::template_environment(&config);
    env.add_template("base.html", "{% block content %}{% endblock %}")
        .unwrap();
    env.add_template(
        "bear/_manage.html",
        include_str!("../../../templates/bear/_manage.html"),
    )
    .unwrap();
    for (name, source) in [
        (
            "identity_reconnection.html",
            include_str!("../../../templates/bear/manage/identity_reconnection.html"),
        ),
        (
            "identity_reconnection_origins.html",
            include_str!("../../../templates/bear/manage/identity_reconnection_origins.html"),
        ),
        (
            "identity_reconnection_review.html",
            include_str!("../../../templates/bear/manage/identity_reconnection_review.html"),
        ),
        (
            "identity_reconnection_receipt.html",
            include_str!("../../../templates/bear/manage/identity_reconnection_receipt.html"),
        ),
        (
            "identity_reconnection_unavailable.html",
            include_str!("../../../templates/bear/manage/identity_reconnection_unavailable.html"),
        ),
    ] {
        env.add_template_owned(format!("bear/manage/{name}"), source.to_string())
            .unwrap();
    }
    let state = AppState::test_with_template_env(pool.clone(), env, config);
    let store = state
        .memory_stores()
        .unwrap()
        .store_for_bear(bear_id.as_uuid())
        .await
        .unwrap();
    let alice = den_memory::resolve_authenticated_user_entity(&store, UserId::new(4242))
        .await
        .unwrap();
    let bob = den_memory::resolve_authenticated_user_entity(&store, UserId::new(4343))
        .await
        .unwrap();
    core::record_export_origins(
        &store,
        "https://source.example",
        UserId::new(4242),
        &[
            core::ExportUserLabel {
                user_id: UserId::new(4242),
                username: Some("Alice<script>".into()),
                display_name: Some("Alice <private>".into()),
            },
            core::ExportUserLabel {
                user_id: UserId::new(4343),
                username: Some("Bob".into()),
                display_name: Some("Bob".into()),
            },
        ],
    )
    .await
    .unwrap();
    let record = store
        .append_record(
            &LogicalMemoryPath::shared_core("personal"),
            "note",
            "curate",
            None,
            "private-memory-canary-never-render",
            &json!({}),
            "normal",
        )
        .await
        .unwrap();
    den_memory::append_relation(
        &store,
        &record.memory_id,
        alice.entity_id(),
        "audience",
        &json!({}),
        "curate",
        None,
        None,
    )
    .await
    .unwrap();
    den_memory::quarantine_imported_authenticated_users(&store)
        .await
        .unwrap();
    let session_store = PostgresStore::new(pool.clone());
    session_store.migrate().await.unwrap();
    let app = super::super::router()
        .route("/test-login/{id}", get(login))
        .route(
            "/test-identity-unavailable",
            get(templates::unavailable_with_private_cause),
        )
        .with_state(state.clone())
        .layer(
            axum_login::AuthManagerLayerBuilder::new(
                crate::auth_backend::Backend::new(pool.clone()),
                axum_login::tower_sessions::SessionManagerLayer::new(session_store),
            )
            .build(),
        );
    let cookie = cookie(&app, actor).await;
    Fixture {
        pool,
        state,
        app,
        bear_id,
        actor,
        destination,
        member,
        alice: alice.entity_id().to_string(),
        bob: bob.entity_id().to_string(),
        private_record: record.memory_id,
        cookie,
    }
}

const LIST: &str = "/bear/identityreview/portability/identities";

async fn request(
    app: &Router,
    cookie: &str,
    method: &str,
    path: &str,
    body: &str,
) -> (StatusCode, String, Option<String>) {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(method)
                .uri(path)
                .header(header::COOKIE, cookie)
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let location = response
        .headers()
        .get(header::LOCATION)
        .map(|value| value.to_str().unwrap().to_string());
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let page = String::from_utf8(body.to_vec()).unwrap();
    assert!(!page.contains("private-memory-canary-never-render"));
    assert!(!page.contains("credential-canary-never-render"));
    (status, page, location)
}

fn input(page: &str, name: &str) -> String {
    let marker = format!("name=\"{name}\" value=\"");
    page.split_once(&marker)
        .unwrap_or_else(|| panic!("missing {name}: {page}"))
        .1
        .split('"')
        .next()
        .unwrap()
        .to_string()
}

impl Fixture {
    async fn store(&self) -> BearMemoryStore {
        self.state
            .memory_stores()
            .unwrap()
            .store_for_bear(self.bear_id.as_uuid())
            .await
            .unwrap()
    }

    async fn prepare(&self, entity: &str, destination: Option<UserId>) -> String {
        let (status, page, _) = request(&self.app, &self.cookie, "GET", LIST, "").await;
        assert_eq!(status, StatusCode::OK, "{page}");
        let mut form = format!(
            "prepare_token={}&entity_id={}&action={}",
            input(&page, "prepare_token"),
            urlencoding::encode(entity),
            if destination.is_some() {
                "reconnect"
            } else {
                "revoke"
            }
        );
        if let Some(destination) = destination {
            form.push_str(&format!("&destination_user_id={destination}"));
        }
        let (status, page, location) = request(
            &self.app,
            &self.cookie,
            "POST",
            &format!("{LIST}/prepare"),
            &form,
        )
        .await;
        assert_eq!(status, StatusCode::SEE_OTHER, "{page}");
        location.unwrap()
    }

    async fn reviewed_form(&self, location: &str, replacement: bool) -> String {
        let (status, page, _) = request(&self.app, &self.cookie, "GET", location, "").await;
        assert_eq!(status, StatusCode::OK, "{page}");
        format!(
            "nonce={}&confirmation_token={}&confirm_audience=true{}",
            input(&page, "nonce"),
            input(&page, "confirmation_token"),
            if replacement {
                "&confirm_replacement=true"
            } else {
                ""
            }
        )
    }

    async fn confirm(&self, form: &str) -> (StatusCode, String, Option<String>) {
        request(
            &self.app,
            &self.cookie,
            "POST",
            &format!("{LIST}/confirm"),
            form,
        )
        .await
    }
}
