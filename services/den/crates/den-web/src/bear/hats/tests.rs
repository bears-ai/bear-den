use super::*;
use axum::{
    body::Body,
    http::{header, Request, StatusCode},
    routing::get,
};
use axum_login::AuthnBackend;
use den_service::bears::db::{self as bears_db, BearParams, BEAR_ROLE_ADMIN, BEAR_ROLE_MEMBER};
use den_service::work_surfaces::NewWorkSurface;
use http_body_util::BodyExt;
use sqlx::PgPool;
use std::sync::Arc;
use tower::ServiceExt;
use tower_sessions_sqlx_store::PostgresStore;

use crate::{auth_backend::Backend, config::Config};

async fn login(Path(user_id): Path<i32>, mut auth: AuthSession) -> StatusCode {
    let user = auth.backend.get_user(&user_id).await.unwrap().unwrap();
    auth.login(&user).await.unwrap();
    StatusCode::OK
}

async fn user(pool: &PgPool, bear_id: Uuid, name: &str, role: &str) -> i32 {
    let id = sqlx::query_scalar!(
        "INSERT INTO users (email, username, display_name, passhash) VALUES ($1, $2, 'Hat Tester', 'x') RETURNING id",
        format!("{name}@example.test"), name
    ).fetch_one(pool).await.unwrap();
    sqlx::query!(
        "INSERT INTO email_configs (user_id, email_address, active, verified_at) VALUES ($1, $2, true, NOW())",
        id, format!("{name}@example.test")
    ).execute(pool).await.unwrap();
    bears_db::grant_membership(pool, id, bear_id, Some(role))
        .await
        .unwrap();
    id
}

async fn cookie(app: &Router, user_id: i32) -> String {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/test-login/{user_id}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
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
        .to_string()
}

async fn request(
    app: &Router,
    cookie: &str,
    method: &str,
    uri: &str,
    body: &str,
) -> (StatusCode, String, Option<String>) {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(method)
                .uri(uri)
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
        .map(|h| h.to_str().unwrap().to_string());
    let body = response.into_body().collect().await.unwrap().to_bytes();
    (status, String::from_utf8(body.to_vec()).unwrap(), location)
}

#[sqlx::test(migrations = "../../migrations")]
async fn hat_admin_setup_and_binding_are_scoped_and_one_way(pool: PgPool) {
    let bear_id = bears_db::create_bear(
        &pool,
        BearParams {
            slug: "hatadminui",
            name: "Hat UI",
            description: "",
            system_prompt: "",
            default_model: None,
            tools_enabled: None,
            context_profile: None,
        },
    )
    .await
    .unwrap();
    let other_bear_id = bears_db::create_bear(
        &pool,
        BearParams {
            slug: "hatotherui",
            name: "Other",
            description: "",
            system_prompt: "",
            default_model: None,
            tools_enabled: None,
            context_profile: None,
        },
    )
    .await
    .unwrap();
    let admin = user(&pool, bear_id, "hatuiadmin", BEAR_ROLE_ADMIN).await;
    let member = user(&pool, bear_id, "hatuimember", BEAR_ROLE_MEMBER).await;
    let mut config = Config::test_stub();
    config.templates_dir = format!("{}/src/templates", env!("CARGO_MANIFEST_DIR"));
    config.bear_sqlite_data_dir = std::env::temp_dir()
        .join(format!("den-hat-ui-{}", Uuid::new_v4()))
        .to_string_lossy()
        .to_string();
    let config = Arc::new(config);
    let state = AppState::test_with_template_env(
        pool.clone(),
        crate::template_environment(&config),
        config,
    );
    let sessions = PostgresStore::new(pool.clone());
    sessions.migrate().await.unwrap();
    let app = Router::new()
        .merge(router())
        .nest("/bear/{bear_slug}", crate::work::docket_router())
        .route("/test-login/{user_id}", get(login))
        .with_state(state.clone())
        .layer(
            axum_login::AuthManagerLayerBuilder::new(
                Backend::new(pool.clone()),
                axum_login::tower_sessions::SessionManagerLayer::new(sessions),
            )
            .build(),
        );
    let admin_cookie = cookie(&app, admin).await;
    let member_cookie = cookie(&app, member).await;
    let path = "/bear/hatadminui/hats";
    assert_eq!(
        request(&app, &member_cookie, "GET", path, "").await.0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        request(
            &app,
            &member_cookie,
            "POST",
            path,
            "name=Injected&purpose=No"
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    let (status, body, _) = request(&app, &admin_cookie, "GET", path, "").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, _, location) = request(
        &app,
        &admin_cookie,
        "POST",
        path,
        "name=Security+review&purpose=Review+repo",
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    let detail = location.unwrap();
    let hat = hats::list_hats(&pool, BearId::new(bear_id))
        .await
        .unwrap()
        .pop()
        .unwrap();
    assert!(detail.ends_with(&hat.id.to_string()));
    let (status, body, _) = request(&app, &admin_cookie, "GET", &detail, "").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body.contains("Security review"));
    assert_eq!(
        request(
            &app,
            &member_cookie,
            "POST",
            &format!("{detail}/work"),
            "action=enable&confirmation=enable+work"
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    assert!(
        request(
            &app,
            &admin_cookie,
            "POST",
            &format!("{detail}/work"),
            "action=enable&confirmation=enable+work"
        )
        .await
        .0
        .is_client_error(),
        "cannot enable Work without surfaces"
    );
    assert_eq!(
        request(
            &app,
            &admin_cookie,
            "GET",
            &format!("/bear/hatotherui/hats/{}", hat.id),
            ""
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    assert!(hats::list_hats(&pool, BearId::new(other_bear_id))
        .await
        .unwrap()
        .is_empty());
    let (status, _, location) = request(
        &app,
        &admin_cookie,
        "POST",
        &format!("{detail}/conversations"),
        "",
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    let conversation_url = location.unwrap();
    let external = conversation_url.split("conversation_id=").nth(1).unwrap();
    let conversation = persistence::get_conversation_for_external_id(&pool, bear_id, external)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        bindings::conversation_hat(&pool, BearId::new(bear_id), conversation.id)
            .await
            .unwrap(),
        Some(hat.id)
    );
    let scope =
        hats::memory_binding::for_conversation(&pool, BearId::new(bear_id), conversation.id)
            .await
            .unwrap();
    match scope {
        hats::memory_binding::ResolvedMemoryBinding::Bound(grant) => {
            assert_eq!(grant.hat_id(), Some(hat.id));
            assert_eq!(
                grant.source(),
                den_memory::MemorySource::Conversation(conversation.id)
            );
        }
        other => panic!("new conversation lost hat binding: {other:?}"),
    }
    let other_hat = hats::create_hat(
        &pool,
        BearId::new(bear_id),
        UserId::new(admin),
        "Another",
        "Other purpose",
    )
    .await
    .unwrap();
    assert_eq!(
        request(
            &app,
            &admin_cookie,
            "POST",
            &format!("/bear/hatadminui/conversations/{}/hat", conversation.id),
            &format!("hat_id={}", other_hat.id)
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );

    let surface = work_surfaces::create_surface(
        &pool,
        admin,
        NewWorkSurface {
            name: format!("hatuirepo{}", Uuid::new_v4().simple()),
            description: None,
            upstream_url: "https://example.test/hat.git".into(),
            default_ref: "main".into(),
            default_image: None,
            allowed_outbound_hosts: vec![],
            credential: None,
        },
        "",
    )
    .await
    .unwrap();
    work_surfaces::assign_bear(&pool, surface.id, bear_id, admin)
        .await
        .unwrap();
    assert_eq!(
        request(
            &app,
            &admin_cookie,
            "POST",
            &format!("{detail}/surfaces"),
            &format!("surface_ids={}", surface.id)
        )
        .await
        .0,
        StatusCode::SEE_OTHER
    );
    assert_eq!(
        manage::allowed_surfaces(&pool, BearId::new(bear_id), hat.id)
            .await
            .unwrap(),
        vec![surface.id]
    );
    assert_eq!(
        request(
            &app,
            &admin_cookie,
            "POST",
            &format!("{detail}/work"),
            "action=enable&confirmation=enable+work"
        )
        .await
        .0,
        StatusCode::SEE_OTHER
    );
    let job_id = sqlx::query_scalar!(
        "INSERT INTO bear_jobs (bear_id, created_by_user_id, created_by_role, goal) VALUES ($1, $2, 'ui', 'Hat-bound work') RETURNING id",
        bear_id, admin
    ).fetch_one(&pool).await.unwrap();
    sqlx::query!(
        "INSERT INTO job_work_surface_assignments (job_id, work_surface_id) VALUES ($1, $2)",
        job_id,
        surface.id
    )
    .execute(&pool)
    .await
    .unwrap();
    let job_path = format!("/bear/hatadminui/jobs/{job_id}/hat");
    assert_eq!(
        request(
            &app,
            &member_cookie,
            "POST",
            &job_path,
            &format!("hat_id={}", hat.id)
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        request(
            &app,
            &admin_cookie,
            "POST",
            &job_path,
            &format!("hat_id={}", hat.id)
        )
        .await
        .0,
        StatusCode::SEE_OTHER
    );
    assert_eq!(
        bindings::job_hat(&pool, BearId::new(bear_id), job_id)
            .await
            .unwrap(),
        Some(hat.id)
    );
    assert_eq!(
        request(
            &app,
            &admin_cookie,
            "POST",
            &format!("{detail}/work"),
            "action=disable"
        )
        .await
        .0,
        StatusCode::SEE_OTHER
    );
    assert_eq!(
        bindings::eligible_job_hat(&pool, BearId::new(bear_id), job_id)
            .await
            .unwrap(),
        None
    );
    let memory = state.memory_stores.store_for_bear(bear_id).await.unwrap();
    den_memory::append_memory_record(
        &memory,
        &den_memory::LogicalMemoryPath::hat(hat.id, "reviewed-note"),
        "note",
        "curate",
        None,
        "curated hat knowledge",
        &serde_json::json!({}),
    )
    .await
    .unwrap();
    assert_eq!(
        request(
            &app,
            &admin_cookie,
            "POST",
            &format!("{detail}/work"),
            "action=enable&confirmation=enable+work"
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    assert!(
        !manage::get_hat(&pool, BearId::new(bear_id), hat.id)
            .await
            .unwrap()
            .work_enabled
    );
}
