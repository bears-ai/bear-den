use super::*;
use axum::{
    body::Body,
    http::{header, Request, StatusCode},
    routing::get,
};
use axum_login::AuthnBackend;
use den_core::ids::UserId;
use den_memory::{append_memory_record, LogicalMemoryPath, MemorySource};
use http_body_util::BodyExt;
use sqlx::postgres::PgPoolOptions;
use std::{path::PathBuf, sync::Arc};
use tower::ServiceExt;
use tower_sessions_sqlx_store::PostgresStore;

use crate::{auth_backend::Backend, config::Config};

static TEST_DB_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

#[path = "feedback_tests.rs"]
mod feedback_tests;

struct TestSqliteDir(PathBuf);

impl TestSqliteDir {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("den-web-memory-routes-{}", Uuid::new_v4()));
        std::fs::create_dir(&path).expect("create isolated SQLite directory");
        Self(path)
    }
}

impl Drop for TestSqliteDir {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).expect("remove isolated SQLite directory");
    }
}

async fn test_pool() -> Option<sqlx::PgPool> {
    dotenvy::dotenv().ok();
    let Ok(url) = std::env::var("DATABASE_URL") else {
        eprintln!("skipping DB-backed memory route test: DATABASE_URL is not set");
        return None;
    };
    let pool = PgPoolOptions::new()
        .max_connections(2)
        .acquire_timeout(std::time::Duration::from_secs(5))
        .connect(&url)
        .await
        .expect("DATABASE_URL is set but memory route test could not connect");
    sqlx::migrate!("../../migrations")
        .set_ignore_missing(true)
        .run(&pool)
        .await
        .expect("migrate Postgres for memory route test");
    Some(pool)
}

async fn test_login(
    axum::extract::Path(user_id): axum::extract::Path<i32>,
    mut auth_session: crate::auth_backend::AuthSession,
) -> StatusCode {
    let user = auth_session
        .backend
        .get_user(&user_id)
        .await
        .expect("load login user")
        .expect("login user exists");
    auth_session.login(&user).await.expect("login");
    StatusCode::OK
}

async fn test_app(
    pool: sqlx::PgPool,
    sqlite_dir: &TestSqliteDir,
) -> (axum::Router, MemoryStoreManager) {
    let mut config = Config::test_stub();
    config.templates_dir = format!("{}/src/templates", env!("CARGO_MANIFEST_DIR"));
    config.bear_sqlite_data_dir = sqlite_dir.0.to_str().expect("UTF-8 temp path").to_string();
    // A configured semantic stack must not make the member request call Qdrant.
    config.qdrant_url = Some("http://127.0.0.1:1".to_string());
    config.llm_api_url = "http://127.0.0.1:1".to_string();
    let config = Arc::new(config);
    let state = AppState::test_with_template_env(
        pool.clone(),
        crate::template_environment(&config),
        config,
    );
    let manager = state.memory_stores().unwrap().clone();
    let session_store = PostgresStore::new(pool.clone());
    session_store
        .migrate()
        .await
        .expect("session store migration");
    let app = Router::new()
        .merge(router())
        .route("/test-login/{user_id}", get(test_login))
        .with_state(state)
        .layer(
            axum_login::AuthManagerLayerBuilder::new(
                Backend::new(pool),
                axum_login::tower_sessions::SessionManagerLayer::new(session_store),
            )
            .build(),
        );
    (app, manager)
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
        .expect("session cookie")
        .to_str()
        .expect("cookie text")
        .split(';')
        .next()
        .expect("cookie pair")
        .to_string()
}

async fn get_page(app: &axum::Router, cookie: &str, uri: &str) -> (StatusCode, String) {
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
        .expect("route response");
    let status = response.status();
    let body = response
        .into_body()
        .collect()
        .await
        .expect("response body")
        .to_bytes();
    (
        status,
        String::from_utf8(body.to_vec()).expect("UTF-8 response"),
    )
}

async fn seed_user(pool: &sqlx::PgPool, bear_id: Uuid, role: &str) -> i32 {
    let unique = Uuid::new_v4().simple().to_string();
    let email = format!("web-memory-{unique}@example.test");
    let user_id = sqlx::query_scalar!(
        "INSERT INTO users (email, username, display_name, passhash)
         VALUES ($1, $2, $3, $4) RETURNING id",
        email,
        format!("wm{}", &unique[..28]),
        "Memory Route Test",
        "test-passhash",
    )
    .fetch_one(pool)
    .await
    .expect("create user");
    sqlx::query!(
        "INSERT INTO email_configs (user_id, email_address, active, verified_at)
         VALUES ($1, $2, true, now())",
        user_id,
        email,
    )
    .execute(pool)
    .await
    .expect("verify email");
    bears_db::grant_membership(pool, user_id, bear_id, Some(role))
        .await
        .expect("grant bear membership");
    user_id
}

async fn add_record(
    store: &store::BearMemoryStore,
    path: &LogicalMemoryPath,
    text: &str,
) -> MemoryRecordRow {
    append_memory_record(store, path, "note", "curate", None, text, &json!({}))
        .await
        .expect("append SQLite record")
}

fn assert_ids(body: &str, present: &[&MemoryRecordRow], absent: &[&MemoryRecordRow]) {
    for row in present {
        assert!(
            body.contains(&row.memory_id),
            "missing record {} in {body}",
            row.memory_id
        );
    }
    for row in absent {
        assert!(
            !body.contains(&row.memory_id),
            "leaked record {}",
            row.memory_id
        );
    }
}

fn stale_semantic_passage(memory_id: &str) -> den_service::recall::query::RecalledPassage {
    den_service::recall::query::RecalledPassage {
        memory_id: memory_id.into(),
        logical_path: Some("core/STALE_VECTOR_PATH.md".into()),
        kind: Some("STALE_VECTOR_KIND".into()),
        score: 0.75,
        salience: "normal".into(),
        lifecycle_status: "active".into(),
        freshness_trend: "stable".into(),
        text: "STALE_VECTOR_PAYLOAD_CANARY".into(),
        conflicts_with: vec![],
    }
}

fn render_admin_semantic_results(results: Vec<RecordListItem>) -> String {
    crate::admin::usability_tests::render(
        "bear/memory/search.html",
        context! {
            can_manage_bear => true, semantic_available => true, mode_used => "semantic",
            q => "canonical", result_count => results.len(), results,
        },
    )
}

#[test]
fn admin_semantic_results_require_current_canonical_content_not_vector_payloads() {
    let passage = stale_semantic_passage("canonical-id");
    let head = MemoryRecordRow {
        memory_id: passage.memory_id.clone(),
        sequence_no: 42,
        scope_type: store::MemoryScopeType::Shared,
        scope_profile: None,
        kind: "canonical-note".into(),
        content_text: "Public canonical text replaces the stale vector body".into(),
        logical_path: Some("core/canonical.md".into()),
        work_surface_ref: None,
        metadata_json: json!({}),
        created_at: "2026-01-01T00:00:00Z".into(),
        salience: "normal".into(),
        supersedes_memory_id: None,
        invalid_at: None,
        lifecycle_status: "active".into(),
        freshness_trend: "stable".into(),
    };
    let item = admin_semantic_result(&passage, Some(head.clone())).unwrap();
    assert_eq!(item.memory_id, head.memory_id);
    assert_eq!(item.kind, head.kind);
    assert_eq!(item.scope_label, "core");
    assert_eq!(item.logical_path, head.logical_path);
    assert_eq!(item.created_at, head.created_at);
    assert_eq!(item.sequence_no, head.sequence_no);
    assert_eq!(item.score, Some(passage.score));
    let html = render_admin_semantic_results(vec![item]);
    assert!(html.contains(&head.content_text));
    for stale in [
        passage.text.as_str(),
        "STALE_VECTOR_PATH",
        "STALE_VECTOR_KIND",
    ] {
        assert!(!html.contains(stale), "rendered derived payload: {stale}");
    }

    // Denied/removed canonical reads and missing heads cannot fall back to vector text.
    let mut rejected = vec![None];
    let mut different_head = head.clone();
    different_head.memory_id = "replacement-id".into();
    rejected.push(Some(different_head));
    let mut invalid = head.clone();
    invalid.invalid_at = Some("2026-01-02T00:00:00Z".into());
    rejected.push(Some(invalid));
    for status in ["archived", "archive-candidate", "superseded"] {
        let mut historical = head.clone();
        historical.lifecycle_status = status.into();
        rejected.push(Some(historical));
    }
    for canonical in rejected {
        let result = admin_semantic_result(&passage, canonical);
        assert!(result.is_none());
        let html = render_admin_semantic_results(result.into_iter().collect());
        assert!(!html.contains(&passage.text));
        assert!(!html.contains(&head.content_text));
        assert!(!html.contains(&passage.memory_id));
    }
}

#[tokio::test]
async fn memory_routes_enforce_curated_member_and_admin_inspection_boundaries() {
    let _guard = TEST_DB_LOCK.lock().await;
    let Some(pool) = test_pool().await else {
        return;
    };
    let sqlite_dir = TestSqliteDir::new();
    let slug = format!("web-memory-{}", Uuid::new_v4());
    let bear_id = bears_db::create_bear(
        &pool,
        bears_db::BearParams {
            slug: &slug,
            name: "Memory Route Bear",
            description: "",
            system_prompt: "",
            default_model: None,
            tools_enabled: None::<sqlx::types::Json<serde_json::Value>>,
            context_profile: None,
        },
    )
    .await
    .expect("create bear");
    let admin_id = seed_user(&pool, bear_id, bears_db::BEAR_ROLE_ADMIN).await;
    let member_id = seed_user(&pool, bear_id, bears_db::BEAR_ROLE_MEMBER).await;
    let hat = hats::create_hat(
        &pool,
        BearId::new(bear_id),
        UserId::new(admin_id),
        "Memory route hat",
        "Curated route visibility",
    )
    .await
    .expect("create Bear-owned hat");
    let (app, manager) = test_app(pool.clone(), &sqlite_dir).await;
    let memory = manager
        .store_for_bear(bear_id)
        .await
        .expect("Bear SQLite store");
    let shared_path = LogicalMemoryPath::shared_core("route-shared");
    let shared_old = add_record(&memory, &shared_path, "boundaryneedle shared old").await;
    let shared = add_record(&memory, &shared_path, "boundaryneedle shared current").await;
    sqlx::query("UPDATE memory_records SET supersedes_memory_id = ? WHERE memory_id = ?")
        .bind(&shared_old.memory_id)
        .bind(&shared.memory_id)
        .execute(memory.pool())
        .await
        .expect("link shared versions");
    let hat_record = add_record(
        &memory,
        &LogicalMemoryPath::hat(hat.id, "route-hat"),
        "boundaryneedle curated hat",
    )
    .await;
    // Presentation must use the canonical hat ID, not a locator that claims core.
    sqlx::query(
        "UPDATE memory_records SET logical_path = 'core/pretend-core.md' WHERE memory_id = ?",
    )
    .bind(&hat_record.memory_id)
    .execute(memory.pool())
    .await
    .expect("set misleading hat locator");
    let source = add_record(
        &memory,
        &LogicalMemoryPath::source_local(
            MemorySource::Conversation(Uuid::new_v4()),
            "route-source",
        ),
        "boundaryneedle private source",
    )
    .await;
    let legacy = add_record(
        &memory,
        &LogicalMemoryPath::profile_local("pair", "route-legacy"),
        "boundaryneedle private legacy",
    )
    .await;
    let collision = add_record(
        &memory,
        &LogicalMemoryPath::source_local(MemorySource::Conversation(Uuid::new_v4()), "collision"),
        "boundaryneedle colliding private source",
    )
    .await;
    sqlx::query("UPDATE memory_records SET logical_path = ? WHERE memory_id = ?")
        .bind(shared.logical_path.as_deref().unwrap())
        .bind(&collision.memory_id)
        .execute(memory.pool())
        .await
        .expect("collide raw and shared logical paths");

    let member = login_cookie(&app, member_id).await;
    let admin = login_cookie(&app, admin_id).await;
    assert_ne!(member, admin, "users must have distinct login sessions");
    assert_eq!(
        bears_db::membership_role_for_user(&pool, member_id, bear_id)
            .await
            .unwrap(),
        Some(Some(bears_db::BEAR_ROLE_MEMBER.to_string())),
    );
    let base = format!("/bear/{slug}/memory");
    let curated = [&shared, &hat_record];
    let raw = [&source, &legacy, &collision];

    for route in [base.clone(), format!("{base}/recent")] {
        let (status, body) = get_page(&app, &member, &route).await;
        assert_eq!(status, StatusCode::OK, "{route}: {body}");
        let heading = if route == base {
            "Recent shared entries"
        } else {
            "Recent additions"
        };
        assert!(body.contains(heading), "{route} rendered the wrong page");
        assert!(
            !body.contains("Memory admin inspection"),
            "member received admin dashboard"
        );

        assert_ids(&body, &curated, &raw);
        assert!(body.contains("Hat: Memory route hat"), "{route}: {body}");
        assert!(body.contains("Bear-wide"), "{route}: {body}");
        assert!(!body.contains("core/pretend-core.md"), "{route}: {body}");
        assert!(
            !body.contains(&shared_old.memory_id),
            "superseded row in {route}"
        );
    }
    let (status, body) = get_page(&app, &member, &format!("{base}/browse")).await;
    assert_eq!(status, StatusCode::OK, "{body}");

    assert_ids(&body, &curated, &raw);
    assert!(body.contains("Hat: Memory route hat"), "{body}");
    assert!(body.contains("Bear-wide"), "{body}");
    assert!(!body.contains("core/pretend-core.md"), "{body}");
    assert!(!body.contains("route-source"));
    assert!(!body.contains("route-legacy"));

    for mode in ["keyword", "semantic"] {
        let route = format!("{base}/search?q=boundaryneedle&mode={mode}");
        let (status, body) = get_page(&app, &member, &route).await;
        assert_eq!(status, StatusCode::OK, "{route}: {body}");

        assert_ids(&body, &curated, &raw);
        assert!(body.contains("Hat: Memory route hat"), "{route}: {body}");
        assert!(body.contains("Bear-wide"), "{route}: {body}");
        assert!(!body.contains("core/pretend-core.md"), "{route}: {body}");
        assert!(!body.contains(&shared_old.memory_id));
        if mode == "semantic" {
            crate::admin::usability_tests::assert_visible(
                &body,
                "Semantic search failed; showing keyword results.",
            );
            assert!(body.contains("(keyword)"), "{body}");
            assert!(!body.contains("score "), "raw Qdrant passage in {body}");
        }
    }
    // A private row cannot be fetched by guessing its ID, even when its path
    // collides with a shared entry; the member history must not inherit it.
    for row in raw {
        let route = format!("{base}/records/{}", row.memory_id);
        let (status, _) = get_page(&app, &member, &route).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{route}");
        let (status, body) = get_page(&app, &admin, &route).await;
        assert_eq!(status, StatusCode::OK, "{route}: {body}");
        assert!(body.contains(&row.content_text), "{route}: {body}");
    }
    let (status, body) = get_page(
        &app,
        &member,
        &format!("{base}/records/{}", hat_record.memory_id),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body.contains("Hat: Memory route hat"), "{body}");
    assert!(!body.contains("core/pretend-core.md"), "{body}");
    let (status, body) = get_page(
        &app,
        &member,
        &format!("{base}/records/{}", shared.memory_id),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(
        body.contains(&shared_old.memory_id),
        "visible shared history: {body}"
    );
    assert!(
        !body.contains(&collision.memory_id),
        "raw history leaked: {body}"
    );
    assert!(
        !body.contains(&collision.content_text),
        "raw content leaked: {body}"
    );
    let (status, body) = get_page(
        &app,
        &admin,
        &format!("{base}/records/{}", shared.memory_id),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(
        body.contains(&collision.memory_id),
        "admin raw history missing: {body}"
    );

    let guessed = Uuid::new_v4();
    for route in [
        format!("/bear/{slug}/entities"),
        format!("/bear/{slug}/entities/{guessed}"),
        format!("{base}/proposals/{guessed}"),
        format!("{base}/reflection/{guessed}"),
        format!("{base}/reflection/{guessed}/evidence"),
    ] {
        let (status, _) = get_page(&app, &member, &route).await;
        assert_eq!(
            status,
            StatusCode::FORBIDDEN,
            "{route} must require Bear admin"
        );
    }
    for route in [base.clone(), format!("{base}/recent")] {
        let (status, body) = get_page(&app, &admin, &route).await;
        assert_eq!(status, StatusCode::OK, "{route}: {body}");
        for row in curated.into_iter().chain(raw) {
            assert!(
                body.contains(&row.memory_id),
                "admin cannot inspect {} in {route}",
                row.memory_id
            );
        }
        assert!(
            body.contains(&shared_old.memory_id),
            "admin cannot inspect history in {route}"
        );
    }

    // Admin membership is not authority to publish access-bearing canonical memory,
    // even if a stale semantic candidate claims it is readable.
    let denied_path = LogicalMemoryPath::shared_core("semantic-denied");
    let denied = add_record(&memory, &denied_path, "DENIED_CANONICAL_BODY_CANARY").await;
    let audience = store::create_entity(
        &memory,
        "person",
        Some("Restricted audience"),
        store::ResolutionState::Resolved,
        store::EntityTrust::Asserted,
        None,
        &json!({}),
    )
    .await
    .unwrap();
    store::append_relation(
        &memory,
        &denied.memory_id,
        &audience.entity_id,
        "audience",
        &json!({}),
        "curate",
        None,
        None,
    )
    .await
    .unwrap();
    let missing_id = Uuid::new_v4().to_string();
    for memory_id in [&denied.memory_id, &missing_id] {
        let canonical = store::get_memory_record_by_id(&manager, bear_id, memory_id)
            .await
            .unwrap();
        assert!(canonical.is_none());
        let (status, _) = get_page(&app, &admin, &format!("{base}/records/{memory_id}")).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        let passage = stale_semantic_passage(memory_id);
        let result = admin_semantic_result(&passage, canonical);
        let html = render_admin_semantic_results(result.into_iter().collect());
        assert!(!html.contains(&passage.text));
        assert!(!html.contains(&denied.content_text));
    }
    assert!(
        store::head_record_for_logical_path(&memory, &denied_path.to_logical_path())
            .await
            .unwrap()
            .is_none()
    );
    let (status, body) = get_page(
        &app,
        &admin,
        &format!("{base}/search?q=DENIED_CANONICAL&mode=keyword"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(!body.contains(&denied.memory_id));
    assert!(!body.contains(&denied.content_text));
}

#[tokio::test]
async fn dashboard_separates_own_notes_from_shared_library_for_members_and_admins() {
    let _guard = TEST_DB_LOCK.lock().await;
    let Some(pool) = test_pool().await else {
        return;
    };
    let sqlite_dir = TestSqliteDir::new();
    let slug = format!("own-notes-{}", Uuid::new_v4());
    let bear_id = bears_db::create_bear(
        &pool,
        bears_db::BearParams {
            slug: &slug,
            name: "Own Notes Bear",
            description: "",
            system_prompt: "",
            default_model: None,
            tools_enabled: None::<sqlx::types::Json<serde_json::Value>>,
            context_profile: None,
        },
    )
    .await
    .unwrap();
    let owner = seed_user(&pool, bear_id, bears_db::BEAR_ROLE_MEMBER).await;
    let other = seed_user(&pool, bear_id, bears_db::BEAR_ROLE_MEMBER).await;
    let admin = seed_user(&pool, bear_id, bears_db::BEAR_ROLE_ADMIN).await;
    let hat = hats::create_hat(
        &pool,
        BearId::new(bear_id),
        UserId::new(admin),
        "Note hat",
        "Private notes",
    )
    .await
    .unwrap();
    let mut conversations = Vec::new();
    for (creator, name) in [
        (Some(owner), "owner-one"),
        (Some(owner), "owner-two"),
        (Some(other), "other-owner"),
        (None, "ownerless"),
        (Some(admin), "admin-own"),
    ] {
        let external = format!("conv-{}", Uuid::new_v4().simple());
        let row = den_service::conversation::persistence::ensure_conversation_for_external_id(
            &pool, bear_id, creator, &external, None, None,
        )
        .await
        .unwrap();
        hats::bindings::bind_conversation_hat(&pool, BearId::new(bear_id), row.id, hat.id)
            .await
            .unwrap();
        conversations.push((name, row.id, external));
    }
    let (app, manager) = test_app(pool.clone(), &sqlite_dir).await;
    let store = manager.store_for_bear(bear_id).await.unwrap();
    let mut notes = Vec::new();
    for (label, id, _) in &conversations {
        let note = add_record(
            &store,
            &LogicalMemoryPath::source_local(MemorySource::Conversation(*id), label),
            &format!("secret-{label}"),
        )
        .await;
        notes.push(note);
    }
    let legacy = add_record(
        &store,
        &LogicalMemoryPath::profile_local("pair", "legacy"),
        "secret-legacy",
    )
    .await;
    sqlx::query("UPDATE memory_records SET logical_path = ? WHERE memory_id = ?")
        .bind(&notes[0].logical_path)
        .bind(&legacy.memory_id)
        .execute(store.pool())
        .await
        .unwrap();
    let (status, member_body) = get_page(
        &app,
        &login_cookie(&app, owner).await,
        &format!("/bear/{slug}/memory"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{member_body}");
    assert!(member_body.contains("Your conversation notes"));
    for label in ["secret-owner-one", "secret-owner-two"] {
        assert!(member_body.contains(label));
    }
    for label in [
        "secret-other-owner",
        "secret-ownerless",
        "secret-admin-own",
        "secret-legacy",
    ] {
        assert!(!member_body.contains(label), "private note leaked: {label}");
    }
    assert!(member_body.contains(&format!("conversation_id={}", conversations[0].2)));
    let (status, other_body) = get_page(
        &app,
        &login_cookie(&app, other).await,
        &format!("/bear/{slug}/memory"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{other_body}");
    assert!(other_body.contains("secret-other-owner"));
    for label in [
        "secret-owner-one",
        "secret-owner-two",
        "secret-ownerless",
        "secret-admin-own",
    ] {
        assert!(!other_body.contains(label), "private note leaked: {label}");
    }
    let (status, admin_body) = get_page(
        &app,
        &login_cookie(&app, admin).await,
        &format!("/bear/{slug}/memory"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{admin_body}");
    let own_section = admin_body
        .split("Your conversation notes")
        .nth(1)
        .unwrap()
        .split("Review queue")
        .next()
        .unwrap();
    assert!(own_section.contains("secret-admin-own"));
    assert!(!own_section.contains("secret-owner-one"));
    assert!(!own_section.contains("secret-other-owner"));
    let (status, feed) = get_page(
        &app,
        &login_cookie(&app, owner).await,
        &format!("/bear/{slug}/memory/recent"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{feed}");
    assert_ids(
        &feed,
        &[],
        &[&notes[0], &notes[1], &notes[2], &notes[3], &notes[4]],
    );
}

/// Axum panics on path conflicts at merge time. Merging the memory router alongside the
/// settings and management routers guards against regressions like the old
/// `/memory/browse` redirect colliding with the real browse page.
#[test]
fn bear_routers_merge_without_conflict() {
    // Mirrors `lib.rs`: `management::router()` already merges `settings::router()`.
    let _router: Router<AppState> = Router::new()
        .merge(router())
        .merge(crate::bear::management::router());
}

/// Compile every new memory/entity template via the path loader to catch MiniJinja
/// syntax errors at test time rather than first render in production.
#[test]
fn memory_templates_compile() {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/src/templates");
    let mut env = minijinja::Environment::new();
    env.set_loader(minijinja::path_loader(dir));
    for name in [
        "bear/memory/_memory_nav.html",
        "bear/memory/dashboard.html",
        "bear/memory/member_dashboard.html",
        "bear/memory/_own_notes.html",
        "bear/memory/member_record.html",
        "bear/memory/recent.html",
        "bear/memory/search.html",
        "bear/memory/browse.html",
        "bear/memory/record.html",
        "bear/memory/reflection_run.html",
        "bear/memory/reflection_evidence.html",
        "bear/memory/entities.html",
        "bear/memory/entity.html",
        "bear/memory_proposal.html",
    ] {
        env.get_template(name)
            .unwrap_or_else(|e| panic!("template {name} failed to compile: {e}"));
    }
}
