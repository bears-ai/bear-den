use super::super::*;
use crate::auth_backend::Backend;
use axum::{
    body::Body,
    http::{header, Request, StatusCode},
};
use axum_login::{login_required, tower_sessions::SessionManagerLayer, AuthnBackend};
use den_cabinet::CabinetPolicy;
use http_body_util::BodyExt;
use sqlx::PgPool;
use std::sync::Arc;
use tower::ServiceExt;
use tower_sessions_sqlx_store::PostgresStore;

async fn test_login(Path(id): Path<i32>, mut session: AuthSession) -> StatusCode {
    let user = session.backend.get_user(&id).await.unwrap().unwrap();
    session.login(&user).await.unwrap();
    StatusCode::OK
}

async fn app(pool: PgPool) -> Router {
    app_on_surface(pool, crate::config::WebSurface::Cabinet).await
}

async fn app_on_surface(pool: PgPool, surface: crate::config::WebSurface) -> Router {
    let store = PostgresStore::new(pool.clone());
    store.migrate().await.unwrap();
    let mut config = crate::config::Config::test_stub();
    config.templates_dir = format!("{}/src/templates", env!("CARGO_MANIFEST_DIR"));
    config.cabinet_web_origin = (surface == crate::config::WebSurface::Cabinet)
        .then(|| "https://cabinet.example".parse().unwrap());
    let mut state = AppState::test_with_template_env(
        pool.clone(),
        crate::template_environment(&config),
        Arc::new(config),
    );
    state.surface = surface;
    Router::new()
        .merge(router().route_layer(login_required!(Backend, login_url = "/login")))
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

async fn user(pool: &PgPool, username: &str) -> i32 {
    let email = format!("{username}@example.test");
    let hash = password_auth::generate_hash("test-password");
    // Reuse the existing web-test statement/cache rather than introducing test-only SQL.
    sqlx::query_scalar!(
        "INSERT INTO users (username, email, display_name, passhash, is_admin) VALUES ($1, $2, $1, $3, $4) RETURNING id",
        username, email, hash, false
    ).fetch_one(pool).await.unwrap()
}

async fn request(app: &Router, cookie: &str, path: &str, form: Option<&str>) -> Response {
    let mut builder = Request::builder().uri(path);
    if !cookie.is_empty() {
        builder = builder.header(header::COOKIE, cookie);
    }
    let body = match form {
        Some(form) => {
            builder = builder
                .method("POST")
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded");
            Body::from(form.to_owned())
        }
        None => Body::empty(),
    };
    app.clone()
        .oneshot(builder.body(body).unwrap())
        .await
        .unwrap()
}

async fn login(app: &Router, id: i32) -> String {
    let response = request(app, "", &format!("/test-login/{id}"), None).await;
    assert_eq!(response.status(), StatusCode::OK);
    response
        .headers()
        .get(header::SET_COOKIE)
        .unwrap()
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_owned()
}

async fn text(response: Response) -> String {
    super::normalize_markup(
        String::from_utf8(
            response
                .into_body()
                .collect()
                .await
                .unwrap()
                .to_bytes()
                .to_vec(),
        )
        .unwrap(),
    )
}

async fn page(pool: &PgPool, owner: i32) -> cabinet_service::ItemView {
    cabinet_service::create_item(
        pool,
        CreateItemRequest {
            scope: ActorScope::user(UserId(owner)),
            kind: ItemKind::Document,
            title: "Shared test page".into(),
            content: "Published before text".into(),
            collection_ref: None,
            mission_ref: None,
            source_links: vec![],
        },
    )
    .await
    .unwrap()
}

#[sqlx::test(migrations = "../../migrations")]
async fn discussion_and_history_routes_require_session_and_hide_malformed_refs(pool: PgPool) {
    let app = app(pool.clone()).await;
    for path in [
        "/cabinet/cabi_unknown/discussion",
        "/cabinet/cabi_unknown/history",
    ] {
        let response = request(&app, "", path, None).await;
        assert_eq!(response.status(), StatusCode::TEMPORARY_REDIRECT);
        assert!(response
            .headers()
            .get(header::LOCATION)
            .unwrap()
            .to_str()
            .unwrap()
            .starts_with("/login"));
    }
    let response = request(
        &app,
        "",
        "/cabinet/cabi_unknown/discussion",
        Some("content=hello"),
    )
    .await;
    assert_eq!(response.status(), StatusCode::TEMPORARY_REDIRECT);
    let owner = user(&pool, "discussionlogin").await;
    let cookie = login(&app, owner).await;
    let response = request(&app, &cookie, "/cabinet/not-a-reference/discussion", None).await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[sqlx::test(migrations = "../../migrations")]
async fn discussion_posts_as_session_human_without_publishing_and_is_current_across_revisions(
    pool: PgPool,
) {
    let owner = user(&pool, "discussionauthor").await;
    let other = user(&pool, "discussionother").await;
    let page = page(&pool, owner).await;
    let reference = page.item.cabinet_ref;
    let scope = ActorScope::user(UserId(owner));
    let before_ref = page.version.version_ref().clone();
    let app = app(pool.clone()).await;
    let cookie = login(&app, owner).await;
    let path = format!("{}/discussion", item_url(&reference));
    let form = format!(
        "content={}&user_id={other}&actor_kind=bear&authored_by={other}",
        urlencoding::encode("<script>posted()</script>")
    );
    let response = request(&app, &cookie, &path, Some(&form)).await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    let entries = cabinet_service::discussion::list(&pool, &scope, &reference)
        .await
        .unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].authored_by, scope);
    assert_eq!(entries[0].content, "<script>posted()</script>");
    let versions = cabinet_service::history(
        &pool,
        HistoryRequest {
            scope: scope.clone(),
            cabinet_ref: reference.clone(),
        },
    )
    .await
    .unwrap();
    assert_eq!(versions.len(), 1);
    assert_eq!(versions[0].version_ref, before_ref);
    cabinet_service::update_item(
        &pool,
        UpdateItemRequest {
            scope: scope.clone(),
            cabinet_ref: reference.clone(),
            content: "Published current text".into(),
            base_version: before_ref.clone(),
            title: None,
        },
    )
    .await
    .unwrap();
    let response = request(&app, &cookie, &format!("{path}?version={before_ref}"), None).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers().get(header::CACHE_CONTROL).unwrap(),
        "no-store"
    );
    let html = text(response).await;
    assert!(html.contains("Current page discussion"));
    assert!(html.contains("&lt;script&gt;posted()&lt;/script&gt;"));
    assert!(!html.contains("<script>posted()"));
    let old = text(
        request(
            &app,
            &cookie,
            &format!("{}?version={before_ref}", item_url(&reference)),
            None,
        )
        .await,
    )
    .await;
    assert!(old.contains("Discuss the current page"));
    assert!(old.contains(&format!("href=\"{path}\"")));
    assert!(old.contains(&format!("?version={before_ref}\"")));
}

#[sqlx::test(migrations = "../../migrations")]
async fn invalid_discussion_preserves_draft_and_archived_page_refuses_posts(pool: PgPool) {
    let owner = user(&pool, "discussionvalidation").await;
    let page = page(&pool, owner).await;
    let scope = ActorScope::user(UserId(owner));
    let reference = page.item.cabinet_ref;
    let app = app(pool.clone()).await;
    let cookie = login(&app, owner).await;
    let path = format!("{}/discussion", item_url(&reference));
    for draft in [
        " \n\t ".to_string(),
        "x".repeat(16_001),
        "NUL\0message".into(),
    ] {
        let form = format!("content={}", urlencoding::encode(&draft));
        let response = request(&app, &cookie, &path, Some(&form)).await;
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
        let html = text(response).await;
        assert!(html.contains("role=\"alert\""));
        assert!(html.contains("draft"));
        if draft.starts_with('x') {
            assert!(html.contains(&draft));
        }
    }
    assert!(cabinet_service::discussion::list(&pool, &scope, &reference)
        .await
        .unwrap()
        .is_empty());
    cabinet_service::archive_item(&pool, &scope, &reference)
        .await
        .unwrap();
    let response = request(&app, &cookie, &path, None).await;
    assert_eq!(response.status(), StatusCode::OK);
    let html = text(response).await;
    assert!(html.contains("Restore it before adding a message"));
    assert!(!html.contains(&format!("action=\"{path}\"")));
    let response = request(
        &app,
        &cookie,
        &path,
        Some("content=Preserved+archived+draft"),
    )
    .await;
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let html = text(response).await;
    assert!(html.contains("Preserved archived draft"));
    assert!(html.contains("readonly"));
    assert!(cabinet_service::discussion::list(&pool, &scope, &reference)
        .await
        .unwrap()
        .is_empty());
}

#[sqlx::test(migrations = "../../migrations")]
async fn unreadable_discussion_and_history_are_invisible_even_with_invalid_drafts(pool: PgPool) {
    let owner = user(&pool, "discussionprivate").await;
    let outsider = user(&pool, "discussionoutsider").await;
    let page = page(&pool, owner).await;
    let scope = ActorScope::user(UserId(owner));
    let reference = page.item.cabinet_ref;
    cabinet_service::pages::configure_named(
        &pool,
        &scope,
        &reference,
        CabinetPolicy::default(),
        &["discussionprivate".into()],
        &[],
        &[],
    )
    .await
    .unwrap();
    cabinet_service::discussion::post(&pool, &scope, &reference, "PRIVATE DISCUSSION EVIDENCE")
        .await
        .unwrap();
    let app = app(pool.clone()).await;
    let cookie = login(&app, outsider).await;
    for (suffix, form) in [
        ("discussion", None),
        ("history", None),
        ("discussion", Some("content=")),
        ("discussion", Some("content=attempt")),
    ] {
        let response = request(
            &app,
            &cookie,
            &format!("{}/{suffix}", item_url(&reference)),
            form,
        )
        .await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        let html = text(response).await;
        assert!(!html.contains("PRIVATE DISCUSSION EVIDENCE"));
        assert!(!html.contains("Shared test page"));
    }
}

#[sqlx::test(migrations = "../../migrations")]
async fn history_compares_published_versions_and_keeps_pending_review_out_of_reader_scope(
    pool: PgPool,
) {
    let owner = user(&pool, "historyowner").await;
    let reader = user(&pool, "historyreader").await;
    let page = page(&pool, owner).await;
    let scope = ActorScope::user(UserId(owner));
    let reference = page.item.cabinet_ref;
    let before_ref = page.version.version_ref().clone();
    let current = cabinet_service::update_item(
        &pool,
        UpdateItemRequest {
            scope: scope.clone(),
            cabinet_ref: reference.clone(),
            content: "Published current text".into(),
            base_version: before_ref.clone(),
            title: None,
        },
    )
    .await
    .unwrap();
    let bear = den_service::bears::db::create_bear(
        &pool,
        den_service::bears::db::BearParams {
            slug: "history-bear",
            name: "History Bear",
            description: "",
            system_prompt: "",
            default_model: None,
            tools_enabled: None,
            context_profile: None,
        },
    )
    .await
    .unwrap();
    cabinet_service::pages::configure_named(
        &pool,
        &scope,
        &reference,
        CabinetPolicy {
            review_required: true,
            ..CabinetPolicy::default()
        },
        &[],
        &[],
        &["historyowner".into()],
    )
    .await
    .unwrap();
    let pending = cabinet_service::update_item(
        &pool,
        UpdateItemRequest {
            scope: ActorScope::bear(bear.into(), "work".parse().unwrap()),
            cabinet_ref: reference.clone(),
            content: "PRIVATE PENDING REVIEW CONTENT".into(),
            base_version: current.version.version_ref().clone(),
            title: None,
        },
    )
    .await
    .unwrap();
    assert_eq!(pending.version.review(), ReviewState::Pending);
    let app = app(pool.clone()).await;
    let cookie = login(&app, reader).await;
    let path = format!("{}/history", item_url(&reference));
    let response = request(&app, &cookie, &format!("{path}?before={before_ref}"), None).await;
    assert_eq!(response.status(), StatusCode::OK);
    let html = text(response).await;
    assert!(html.contains("Published before text"));
    assert!(html.contains("Published current text"));
    assert!(html.contains("Current</strong>"));
    assert!(html.contains(&format!(
        "href=\"{}?version={before_ref}\"",
        item_url(&reference)
    )));
    assert!(html.contains(&format!(
        "href=\"{}?version={}\"",
        item_url(&reference),
        current.version.version_ref()
    )));
    assert!(!html.contains(pending.version.version_ref().as_str()));
    assert!(!html.contains("PRIVATE PENDING REVIEW CONTENT"));
    let response = request(
        &app,
        &cookie,
        &format!("{path}?before={}", pending.version.version_ref()),
        None,
    )
    .await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let owner_cookie = login(&app, owner).await;
    let response = request(
        &app,
        &owner_cookie,
        &format!("{path}?before={}", pending.version.version_ref()),
        None,
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let html = text(response).await;
    assert!(html.contains("Pending review · unpublished"));
    assert!(html.contains("Only published revisions can be compared"));
    assert!(!html.contains("PRIVATE PENDING REVIEW CONTENT"));
}

#[sqlx::test(migrations = "../../migrations")]
async fn editor_preserves_invalid_and_stale_drafts_without_publishing_and_archived_form_is_readonly(
    pool: PgPool,
) {
    let owner = user(&pool, "editdraftowner").await;
    let page = page(&pool, owner).await;
    let scope = ActorScope::user(UserId(owner));
    let reference = page.item.cabinet_ref;
    let before = page.version.version_ref().clone();
    let app = app(pool.clone()).await;
    let cookie = login(&app, owner).await;
    let path = format!("{}/edit", item_url(&reference));
    let draft = "</textarea><script>draft()</script>";
    for (title, base) in [
        ("Preserved title", "invalid-base".to_string()),
        ("", before.to_string()),
    ] {
        let form = format!(
            "title={}&content={}&base_version={}",
            urlencoding::encode(title),
            urlencoding::encode(draft),
            urlencoding::encode(&base)
        );
        let response = request(&app, &cookie, &path, Some(&form)).await;
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(
            response.headers().get(header::CACHE_CONTROL).unwrap(),
            "no-store"
        );
        let html = text(response).await;
        assert!(html.contains("&lt;/textarea&gt;&lt;script&gt;draft()&lt;/script&gt;"));
        assert!(!html.contains("<script>draft()"));
        assert!(html.contains("role=\"alert\""));
    }
    let current = cabinet_service::update_item(
        &pool,
        UpdateItemRequest {
            scope: scope.clone(),
            cabinet_ref: reference.clone(),
            content: "Published newer text".into(),
            base_version: before.clone(),
            title: None,
        },
    )
    .await
    .unwrap();
    let form = format!(
        "title=Preserved+title&content={}&base_version={before}",
        urlencoding::encode(draft)
    );
    let response = request(&app, &cookie, &path, Some(&form)).await;
    assert_eq!(response.status(), StatusCode::CONFLICT);
    let html = text(response).await;
    assert!(html.contains("Preserved title"));
    assert!(html.contains("Your draft is preserved"));
    assert!(html.contains(current.version.version_ref().as_str()));
    cabinet_service::archive_item(&pool, &scope, &reference)
        .await
        .unwrap();
    let response = request(&app, &cookie, &path, None).await;
    assert_eq!(response.status(), StatusCode::OK);
    let html = text(response).await;
    assert!(html.contains("This page is archived"));
    assert!(!html.contains("method=\"post\""));
    let form = format!(
        "title=Archived+draft&content={}&base_version={}",
        urlencoding::encode(draft),
        current.version.version_ref()
    );
    let response = request(&app, &cookie, &path, Some(&form)).await;
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let html = text(response).await;
    assert!(html.contains("Archived draft"));
    assert!(html.contains("readonly"));
    assert!(!html.contains("method=\"post\""));
    let versions = cabinet_service::history(
        &pool,
        HistoryRequest {
            scope,
            cabinet_ref: reference,
        },
    )
    .await
    .unwrap();
    assert_eq!(versions.len(), 2);
}

#[sqlx::test(migrations = "../../migrations")]
async fn dedicated_cabinet_routes_use_cabinet_shell_and_current_acl_after_membership_changes(
    pool: PgPool,
) {
    let owner = user(&pool, "dedicatedowner").await;
    let reader = user(&pool, "dedicatedreader").await;
    let page = page(&pool, owner).await;
    let scope = ActorScope::user(UserId(owner));
    let reference = page.item.cabinet_ref;
    let version = page.version.version_ref().clone();
    let app = app_on_surface(pool.clone(), crate::config::WebSurface::Cabinet).await;
    let cookie = login(&app, reader).await;
    for suffix in ["", "/discussion", "/history"] {
        let response = request(
            &app,
            &cookie,
            &format!("{}{suffix}", item_url(&reference)),
            None,
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers().get(header::CACHE_CONTROL).unwrap(),
            "no-store"
        );
        let html = text(response).await;
        assert!(html.contains("aria-label=\"Cabinet navigation\""));
        assert!(html.contains("aria-label=\"Page sections\""));
        assert!(!html.contains("Den management"));
        assert!(!html.contains("href=\"/admin\""));
    }
    cabinet_service::pages::configure_named(
        &pool,
        &scope,
        &reference,
        CabinetPolicy::default(),
        &["dedicatedowner".into()],
        &[],
        &[],
    )
    .await
    .unwrap();
    for suffix in [
        format!("?version={version}"),
        format!("/history?before={version}"),
        "/discussion".into(),
        "/edit".into(),
    ] {
        let response = request(
            &app,
            &cookie,
            &format!("{}{suffix}", item_url(&reference)),
            None,
        )
        .await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        assert!(!text(response).await.contains("Shared test page"));
    }
    let response = request(
        &app,
        &cookie,
        &format!("{}/edit", item_url(&reference)),
        Some("title=Attempt&content=unposted&base_version=bad"),
    )
    .await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}
