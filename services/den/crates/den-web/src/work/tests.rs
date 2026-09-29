//! Route tests for the work UI's job-creation and dispatch forms. Postgres-
//! backed (skip only without DATABASE_URL); login is seeded through a test-only
//! route, same pattern as `bear::settings::tests`.

use super::*;
use axum::{
    body::Body,
    http::{header, Request, StatusCode},
    routing::get,
};
use axum_login::AuthnBackend;
use http_body_util::BodyExt;
use sqlx::postgres::PgPoolOptions;
use std::sync::Arc;
use tower::ServiceExt;
use tower_sessions_sqlx_store::PostgresStore;

#[test]
fn job_page_exposes_accessible_journal_history_and_empty_states() {
    let template = include_str!("../templates/work/job.html");

    for expected in [
        "aria-labelledby=\"job-notebook-heading\"",
        "No notebook entries yet.",
        "aria-labelledby=\"settlement-history-heading\"",
        "No task settlements recorded yet.",
        "<time datetime=\"{{ entry.created_at }}\">",
        "<summary>Settlement evidence</summary>",
    ] {
        assert!(template.contains(expected), "missing `{expected}`");
    }
}

#[test]
fn cargo_offline_cache_miss_is_the_primary_outcome() {
    let failure = serde_json::json!({
        "code": "cargo_offline_cache_miss",
        "required_package": "serde",
    });
    let run = WorkRunRow {
        id: Uuid::nil(),
        bear_id: Uuid::nil(),
        job_id: Uuid::nil(),
        job_run_id: Uuid::nil(),
        executing_task_id: None,
        attempt: 1,
        state: "succeeded".into(),
        runner_id: None,
        lease_expires_at: None,
        cancel_requested: false,
        cancel_requested_by: None,
        cancel_reason: None,
        cancel_requested_at: None,
        git_ref: None,
        image_name: None,
        sandbox_server_url: None,
        sandbox_id: None,
        sandbox_type: None,
        sandbox_strength: None,
        work_surface: None,
        execution_target: "sandbox".into(),
        attached_client_session_id: None,
        attachment_state: None,
        attachment_warning: None,
        disconnected_at: None,
        disconnect_deadline_at: None,
        bearwire_session_id: None,
        result_summary: Some("headless turn reached a terminal run event".into()),
        result_refs: None,
        usage: None,
        error: None,
        queued_at: time::OffsetDateTime::UNIX_EPOCH,
        started_at: None,
        finished_at: None,
        updated_at: time::OffsetDateTime::UNIX_EPOCH,
    };
    assert_eq!(
        work_run_outcome(&run, &[(Uuid::nil(), "pending".into())], Some(&failure)),
        "Blocked: Rust dependencies are unavailable in the offline cache. `serde` could not be resolved. Dependency preparation was not attempted; prepare Rust dependencies, then retry Cargo.",
    );
}

#[test]
fn watchdog_failure_view_shows_only_safe_persisted_evidence() {
    let refs = serde_json::json!({
        "outcome": {
            "code": "continuation_watchdog_timeout",
            "affected_task": { "title": "Render failure details", "status": "in_progress" },
            "forensics": {
                "runtime_event_count": 1,
                "last_event_age_ms": 30007,
                "last_tool_request": {
                    "tool_name": "checkpoint",
                    "request_class": "den_owned",
                    "arguments": { "secret": "must not project" }
                }
            }
        }
    });
    let view = watchdog_failure_view(Some(&refs)).expect("watchdog view");
    assert_eq!(view.task_title.as_deref(), Some("Render failure details"));
    assert_eq!(view.tool_name.as_deref(), Some("checkpoint"));
    assert_eq!(view.request_class.as_deref(), Some("den_owned"));
    assert_eq!(view.idle_ms, Some(30007));
}

use crate::{auth_backend::Backend, config::Config};
use den_service::work_surfaces::{self, NewWorkSurface};

static TEST_DB_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

async fn test_pool() -> Option<sqlx::PgPool> {
    dotenvy::dotenv().ok();
    let Ok(url) = std::env::var("DATABASE_URL") else {
        eprintln!("skipping DB-backed work route test: DATABASE_URL is not set");
        return None;
    };
    let pool = PgPoolOptions::new()
        .max_connections(2)
        .acquire_timeout(std::time::Duration::from_secs(5))
        .connect(&url)
        .await
        .expect("DATABASE_URL is set: work route tests must connect to Postgres");
    sqlx::migrate!("../../migrations")
        .set_ignore_missing(true)
        .run(&pool)
        .await
        .expect("DATABASE_URL is set: work route migrations must succeed");
    Some(pool)
}

fn test_state(pool: sqlx::PgPool) -> AppState {
    let mut config = Config::test_stub();
    config.templates_dir = format!("{}/src/templates", env!("CARGO_MANIFEST_DIR"));
    let config = Arc::new(config);
    let template_env = crate::template_environment(config.as_ref());
    AppState::test_with_template_env(pool, template_env, config)
}

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
        .nest("/bear/{bear_slug}", docket_router())
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
        .expect("cookie str")
        .split(';')
        .next()
        .expect("cookie pair")
        .to_string()
}

/// Member user + bear (admin membership) for the work UI's scoping checks.
async fn seed_member(pool: &sqlx::PgPool) -> (i32, Uuid, String) {
    let unique = Uuid::new_v4().simple().to_string();
    let user_id = sqlx::query_scalar!(
        "INSERT INTO users (email, username, display_name, passhash)
         VALUES ($1, $2, $3, $4) RETURNING id",
        format!("work-ui-{unique}@example.test"),
        format!("wu{}", &unique[..28]),
        "Work UI Test",
        "test-passhash",
    )
    .fetch_one(pool)
    .await
    .expect("create user");
    let slug = format!("work-ui-{}", &unique[..12]);
    let bear_id = bears_db::create_bear(
        pool,
        bears_db::BearParams {
            slug: &slug,
            name: "Work UI Test Bear",
            description: "",
            system_prompt: "",
            default_model: None,
            tools_enabled: None::<sqlx::types::Json<serde_json::Value>>,
            context_profile: None,
        },
    )
    .await
    .expect("create bear");
    bears_db::grant_membership(pool, user_id, bear_id, Some("admin"))
        .await
        .expect("grant membership");
    (user_id, bear_id, slug)
}

async fn assigned_surface_id(pool: &sqlx::PgPool, user_id: i32, bear_id: Uuid) -> Uuid {
    let surface = work_surfaces::create_surface(
        pool,
        user_id,
        NewWorkSurface {
            name: format!("work-ui-{}", Uuid::new_v4().simple()),
            description: None,
            upstream_url: "https://example.test/work-ui.git".to_string(),
            default_ref: "main".to_string(),
            default_image: None,
            allowed_outbound_hosts: vec![],
            credential: None,
        },
        "test-secret-key",
    )
    .await
    .expect("create work surface");
    sqlx::query!(
        "INSERT INTO work_surface_bears (surface_id, bear_id) VALUES ($1, $2)",
        surface.id,
        bear_id,
    )
    .execute(pool)
    .await
    .expect("assign work surface");
    surface.id
}

async fn assert_job_uses_surface(pool: &sqlx::PgPool, job_id: Uuid, surface_id: Uuid) {
    let assigned: bool = sqlx::query_scalar!(
        "SELECT EXISTS(SELECT 1 FROM job_work_surface_assignments \
         WHERE job_id = $1 AND work_surface_id = $2) AS \"exists!: bool\"",
        job_id,
        surface_id,
    )
    .fetch_one(pool)
    .await
    .expect("job work-surface assignment");
    assert!(assigned);
}

#[tokio::test]
async fn create_job_form_creates_work_job_with_tasks() {
    let _guard = TEST_DB_LOCK.lock().await;
    let Some(pool) = test_pool().await else {
        return;
    };
    let (user_id, bear_id, bear_slug) = seed_member(&pool).await;
    let surface_id = assigned_surface_id(&pool, user_id, bear_id).await;
    let app = test_app(pool.clone()).await;
    let cookie = login_cookie(&app, user_id).await;

    let body = format!(
        "bear_id={bear_id}&goal=Ship+the+site&surface_id={surface_id}&commit_policy=per_task\
         &work_branch=&task_title=Update+headline&task_criteria=headline+mentions+bears\
         &task_title=&task_criteria="
    );
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/bear/{bear_slug}/jobs/new"))
                .header(header::COOKIE, &cookie)
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .expect("create job response");
    let status = response.status();
    let location = response
        .headers()
        .get(header::LOCATION)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    if status != StatusCode::SEE_OTHER {
        use http_body_util::BodyExt;
        let body = response.into_body().collect().await.unwrap().to_bytes();
        panic!(
            "create job: expected 303, got {status}: {}",
            String::from_utf8_lossy(&body)
        );
    }
    assert_eq!(
        location,
        format!(
            "/bear/{bear_slug}/jobs/{}",
            route_id(
                sqlx::query_scalar!(
                    "SELECT id FROM bear_jobs WHERE bear_id = $1 ORDER BY created_at DESC LIMIT 1",
                    bear_id
                )
                .fetch_one(&pool)
                .await
                .expect("job id")
            )
        )
    );
    let job_id: Uuid = sqlx::query_scalar!(
        "SELECT id FROM bear_jobs WHERE bear_id = $1 ORDER BY created_at DESC LIMIT 1",
        bear_id
    )
    .fetch_one(&pool)
    .await
    .expect("job id");

    let job = sqlx::query!(
        "SELECT goal, commit_policy, work_branch
             FROM bear_jobs WHERE id = $1",
        job_id
    )
    .fetch_one(&pool)
    .await
    .expect("job row");
    assert_eq!(job.goal, "Ship the site");
    assert_job_uses_surface(&pool, job_id, surface_id).await;
    assert_eq!(job.commit_policy.as_deref(), Some("per_task"));
    assert!(
        job.work_branch.is_none(),
        "blank branch stays unset until dispatch"
    );

    // Exactly one non-blank task with the criterion.
    let tasks: Vec<String> =
        sqlx::query_scalar!("SELECT title FROM bear_tasks WHERE job_id = $1", job_id)
            .fetch_all(&pool)
            .await
            .expect("tasks");
    assert_eq!(tasks, vec!["Update headline"]);

    let response = post_form(
        &app,
        &cookie,
        &format!("/bear/{bear_slug}/jobs/{}/edit", route_id(job_id)),
        format!("goal=Ship+the+updated+site&surface_id={surface_id}&commit_policy=per_job&work_branch=feature%2Fupdated"),
    )
    .await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    let job = sqlx::query!(
        "SELECT goal, commit_policy, work_branch FROM bear_jobs WHERE id = $1",
        job_id
    )
    .fetch_one(&pool)
    .await
    .expect("edited job row");
    assert_eq!(job.goal, "Ship the updated site");
    assert_job_uses_surface(&pool, job_id, surface_id).await;
    assert_eq!(job.commit_policy.as_deref(), Some("per_job"));
    assert_eq!(job.work_branch.as_deref(), Some("feature/updated"));

    let response = app
        .oneshot(
            Request::builder()
                .uri(format!("/bear/{bear_slug}/jobs/{}", route_id(job_id)))
                .header(header::COOKIE, &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("job detail response");
    assert_eq!(response.status(), StatusCode::OK);
    use http_body_util::BodyExt;
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let body = String::from_utf8_lossy(&body);
    assert!(body.contains("Dispatch topology"));
    assert!(body.contains("Isolated from your current checkout"));
    assert!(body.contains("Repository changes"));
}

#[tokio::test]
async fn work_dashboard_hides_completed_jobs_until_requested() {
    let _guard = TEST_DB_LOCK.lock().await;
    let Some(pool) = test_pool().await else {
        return;
    };
    let (user_id, bear_id, bear_slug) = seed_member(&pool).await;
    let surface_id = assigned_surface_id(&pool, user_id, bear_id).await;
    let app = test_app(pool.clone()).await;
    let cookie = login_cookie(&app, user_id).await;
    let unique = Uuid::new_v4().simple().to_string();
    let active_goal = format!("Active dashboard job {unique}");
    let completed_goal = format!("Completed dashboard job {unique}");

    for goal in [&active_goal, &completed_goal] {
        let response = post_form(
            &app,
            &cookie,
            &format!("/bear/{bear_slug}/jobs/new"),
            format!(
                "bear_id={bear_id}&goal={}&surface_id={surface_id}&commit_policy=none&work_branch=&task_title=Check&task_criteria=done",
                urlencoding::encode(goal)
            ),
        )
        .await;
        assert_eq!(response.status(), StatusCode::SEE_OTHER);
    }
    sqlx::query!(
        "UPDATE bear_task_run_state SET status = 'done'
         WHERE run_id = (SELECT current_run_id FROM bear_jobs WHERE bear_id = $1 AND goal = $2)",
        bear_id,
        &completed_goal,
    )
    .execute(&pool)
    .await
    .expect("complete dashboard job tasks");
    sqlx::query!(
        "UPDATE bear_job_criteria_state SET status = 'met'
         WHERE run_id = (SELECT current_run_id FROM bear_jobs WHERE bear_id = $1 AND goal = $2)",
        bear_id,
        &completed_goal,
    )
    .execute(&pool)
    .await
    .expect("complete dashboard job criteria");

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/bear/{bear_slug}/jobs"))
                .header(header::COOKIE, &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("dashboard response");
    let status = response.status();
    let body = String::from_utf8_lossy(&response.into_body().collect().await.unwrap().to_bytes())
        .into_owned();
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body.contains(&active_goal));
    assert!(!body.contains(&completed_goal));
    assert!(body.contains("Show completed jobs"));
    assert!(body.contains(&format!("/bear/{bear_slug}/jobs/new")));

    let response = app
        .oneshot(
            Request::builder()
                .uri(format!("/bear/{bear_slug}/jobs?completed=show"))
                .header(header::COOKIE, &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("completed dashboard response");
    let body = String::from_utf8_lossy(&response.into_body().collect().await.unwrap().to_bytes())
        .into_owned();
    assert!(body.contains(&active_goal));
    assert!(body.contains(&completed_goal));
    assert!(body.contains("Hide completed jobs"));
}

#[tokio::test]
async fn duplicate_job_copies_definition_and_resets_execution_state() {
    let _guard = TEST_DB_LOCK.lock().await;
    let Some(pool) = test_pool().await else {
        return;
    };
    let (user_id, bear_id, bear_slug) = seed_member(&pool).await;
    let surface_id = assigned_surface_id(&pool, user_id, bear_id).await;
    let app = test_app(pool.clone()).await;
    let cookie = login_cookie(&app, user_id).await;

    let response = post_form(
        &app,
        &cookie,
        &format!("/bear/{bear_slug}/jobs/new"),
        format!(
            "bear_id={bear_id}&goal=Reusable+job&surface_id={surface_id}&commit_policy=per_task\
             &work_branch=&task_title=Build+artifact&task_criteria=artifact+exists%3Btests+pass"
        ),
    )
    .await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    let source_id: Uuid = sqlx::query_scalar!(
        "SELECT id FROM bear_jobs WHERE bear_id = $1 ORDER BY created_at DESC LIMIT 1",
        bear_id
    )
    .fetch_one(&pool)
    .await
    .expect("source job");
    sqlx::query!(
        "UPDATE bear_jobs SET work_branch = 'feature/original' WHERE id = $1",
        source_id,
    )
    .execute(&pool)
    .await
    .expect("set source branch");
    let source_run_id: Uuid = sqlx::query_scalar!(
        "SELECT current_run_id FROM bear_jobs WHERE id = $1",
        source_id
    )
    .fetch_one(&pool)
    .await
    .expect("source run id")
    .expect("source has current run");

    let response = post_form(
        &app,
        &cookie,
        &format!("/bear/{bear_slug}/jobs/{}/duplicate", route_id(source_id)),
        String::new(),
    )
    .await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    let duplicate_id: Uuid = sqlx::query_scalar!(
        "SELECT id FROM bear_jobs WHERE bear_id = $1 ORDER BY created_at DESC LIMIT 1",
        bear_id
    )
    .fetch_one(&pool)
    .await
    .expect("duplicate job");
    assert_ne!(duplicate_id, source_id);

    let duplicate = sqlx::query!(
        "SELECT goal, commit_policy, work_branch, current_run_id
         FROM bear_jobs WHERE id = $1",
        duplicate_id
    )
    .fetch_one(&pool)
    .await
    .expect("duplicate job row");
    assert_eq!(duplicate.goal, "Reusable job (copy)");
    assert_job_uses_surface(&pool, duplicate_id, surface_id).await;
    assert_eq!(duplicate.commit_policy.as_deref(), Some("per_task"));
    assert!(duplicate.work_branch.is_none());
    let duplicate_run_id = duplicate.current_run_id.expect("fresh duplicate run");
    assert_ne!(duplicate_run_id, source_run_id);

    let tasks = sqlx::query!(
        "SELECT title, body, completion_criteria \
             FROM bear_tasks WHERE job_id = $1 ORDER BY sibling_order",
        duplicate_id
    )
    .fetch_all(&pool)
    .await
    .expect("duplicate tasks");
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0].title, "Build artifact");
    assert_eq!(tasks[0].body, "Build artifact");
    assert_eq!(
        tasks[0].completion_criteria,
        serde_json::json!(["artifact exists", "tests pass"])
    );
    let task_statuses: Vec<String> = sqlx::query_scalar!(
        "SELECT status FROM bear_task_run_state WHERE run_id = $1",
        duplicate_run_id
    )
    .fetch_all(&pool)
    .await
    .expect("duplicate task states");
    assert_eq!(task_statuses, vec!["pending"]);
}

#[tokio::test]
async fn task_tree_can_add_children_and_reorder_siblings() {
    let _guard = TEST_DB_LOCK.lock().await;
    let Some(pool) = test_pool().await else {
        return;
    };
    let (user_id, bear_id, bear_slug) = seed_member(&pool).await;
    let surface_id = assigned_surface_id(&pool, user_id, bear_id).await;
    let app = test_app(pool.clone()).await;
    let cookie = login_cookie(&app, user_id).await;

    let response = post_form(
        &app,
        &cookie,
        &format!("/bear/{bear_slug}/jobs/new"),
        format!(
            "bear_id={bear_id}&goal=Edit+the+tree&surface_id={surface_id}&commit_policy=none\
             &task_title=First+root&task_criteria=first+done"
        ),
    )
    .await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    let job_id: Uuid = sqlx::query_scalar!(
        "SELECT id FROM bear_jobs WHERE bear_id = $1 ORDER BY created_at DESC LIMIT 1",
        bear_id
    )
    .fetch_one(&pool)
    .await
    .expect("job id");
    let first_root_id: Uuid = sqlx::query_scalar!(
        "SELECT id FROM bear_tasks WHERE job_id = $1 AND title = 'First root'",
        job_id
    )
    .fetch_one(&pool)
    .await
    .expect("first root task");

    let response = post_form(
        &app,
        &cookie,
        &format!(
            "/bear/{bear_slug}/jobs/{}/tasks/{}/children",
            route_id(job_id),
            route_id(first_root_id)
        ),
        "title=First+child&criteria=child+done&body=".to_string(),
    )
    .await;
    if response.status() != StatusCode::SEE_OTHER {
        use http_body_util::BodyExt;
        let status = response.status();
        let body = response
            .into_body()
            .collect()
            .await
            .expect("error body")
            .to_bytes();
        panic!(
            "add child: expected 303, got {status}: {}",
            String::from_utf8_lossy(&body)
        );
    }

    let child = sqlx::query!(
        "SELECT parent_task_id, sibling_order FROM bear_tasks WHERE job_id = $1 AND title = 'First child'", job_id)
    .fetch_one(&pool)
    .await
    .expect("child task");
    assert_eq!(child.parent_task_id, Some(first_root_id));
    assert_eq!(child.sibling_order, 0);

    let response = post_form(
        &app,
        &cookie,
        &format!("/bear/{bear_slug}/jobs/{}/tasks", route_id(job_id)),
        "title=Second+root&body=&criteria=second+done".to_string(),
    )
    .await;
    if response.status() != StatusCode::SEE_OTHER {
        use http_body_util::BodyExt;
        let status = response.status();
        let body = response
            .into_body()
            .collect()
            .await
            .expect("error body")
            .to_bytes();
        panic!(
            "add root: expected 303, got {status}: {}",
            String::from_utf8_lossy(&body)
        );
    }
    let second_root_id: Uuid = sqlx::query_scalar!(
        "SELECT id FROM bear_tasks WHERE job_id = $1 AND title = 'Second root'",
        job_id
    )
    .fetch_one(&pool)
    .await
    .expect("second root task");

    let response = post_form(
        &app,
        &cookie,
        &format!(
            "/bear/{bear_slug}/jobs/{}/tasks/{}/move/up",
            route_id(job_id),
            route_id(second_root_id)
        ),
        String::new(),
    )
    .await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    let roots: Vec<Uuid> = sqlx::query_scalar!(
        "SELECT id FROM bear_tasks WHERE job_id = $1 AND parent_task_id IS NULL ORDER BY sibling_order", job_id)
    .fetch_all(&pool)
    .await
    .expect("root ordering");
    assert_eq!(roots, vec![second_root_id, first_root_id]);
}

#[tokio::test]
async fn job_lifecycle_can_extend_then_complete() {
    let _guard = TEST_DB_LOCK.lock().await;
    let Some(pool) = test_pool().await else {
        return;
    };
    let (user_id, bear_id, bear_slug) = seed_member(&pool).await;
    let app = test_app(pool.clone()).await;
    let cookie = login_cookie(&app, user_id).await;
    let surface_name = format!("lifecycle-{}", &Uuid::new_v4().simple().to_string()[..12]);
    let response = post_form(
        &app,
        &cookie,
        "/work/surfaces/new",
        format!(
            "name={surface_name}&description=&upstream_url=https%3A%2F%2Fexample.invalid%2Frepo.git\
             &default_ref=main&default_image=&credential_kind=&credential_value=&bear_id={bear_id}"
        ),
    )
    .await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    let surface_id: Uuid = sqlx::query_scalar!(
        "SELECT id FROM work_surfaces WHERE name = $1",
        &surface_name
    )
    .fetch_one(&pool)
    .await
    .expect("surface id");

    let response = post_form(
        &app,
        &cookie,
        &format!("/bear/{bear_slug}/jobs/new"),
        format!(
            "bear_id={bear_id}&goal=Lifecycle+job&surface_id={surface_id}&root=&commit_policy=none\
             &task_title=First+task&task_criteria=first+done"
        ),
    )
    .await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    let job_id: Uuid = sqlx::query_scalar!(
        "SELECT id FROM bear_jobs WHERE bear_id = $1 ORDER BY created_at DESC LIMIT 1",
        bear_id
    )
    .fetch_one(&pool)
    .await
    .expect("job id");
    let run_id = sqlx::query_scalar!("SELECT current_run_id FROM bear_jobs WHERE id = $1", job_id)
        .fetch_one(&pool)
        .await
        .expect("current run")
        .expect("job has current run");

    let response = post_form(
        &app,
        &cookie,
        &format!("/bear/{bear_slug}/jobs/{}/tasks", route_id(job_id)),
        "title=Second+task&body=&criteria=second+done".to_string(),
    )
    .await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    let task_count = sqlx::query_scalar!(
        "SELECT count(*)::bigint AS \"count!: i64\" FROM bear_tasks WHERE job_id = $1",
        job_id
    )
    .fetch_one(&pool)
    .await
    .expect("task count");
    assert_eq!(task_count, 2);
    sqlx::query("UPDATE bear_task_run_state SET status = 'done' WHERE run_id = $1")
        .bind(run_id)
        .execute(&pool)
        .await
        .expect("complete task states");
    sqlx::query("UPDATE bear_job_criteria_state SET status = 'met' WHERE run_id = $1")
        .bind(run_id)
        .execute(&pool)
        .await
        .expect("complete criterion states");

    let response = post_form(
        &app,
        &cookie,
        &format!("/bear/{bear_slug}/jobs/{}/complete", route_id(job_id)),
        String::new(),
    )
    .await;
    if response.status() != StatusCode::SEE_OTHER {
        use http_body_util::BodyExt;
        let status = response.status();
        let body = response
            .into_body()
            .collect()
            .await
            .expect("error body")
            .to_bytes();
        panic!(
            "complete job: expected 303, got {status}: {}",
            String::from_utf8_lossy(&body)
        );
    }
    assert_job_uses_surface(&pool, job_id, surface_id).await;
    let run_state: String =
        sqlx::query_scalar!("SELECT state FROM bear_job_runs WHERE id = $1", run_id)
            .fetch_one(&pool)
            .await
            .expect("run state");
    let criterion_statuses: Vec<String> = sqlx::query_scalar!(
        "SELECT status FROM bear_job_criteria_state WHERE run_id = $1",
        run_id
    )
    .fetch_all(&pool)
    .await
    .expect("criterion states");
    assert_job_uses_surface(&pool, job_id, surface_id).await;
    assert_eq!(run_state, "dispatched");
    assert!(criterion_statuses.iter().all(|status| status == "met"));
}

#[tokio::test]
async fn job_scoped_surface_creation_assigns_and_attaches_surface() {
    let _guard = TEST_DB_LOCK.lock().await;
    let Some(pool) = test_pool().await else {
        return;
    };
    let (user_id, bear_id, bear_slug) = seed_member(&pool).await;
    let initial_surface_id = assigned_surface_id(&pool, user_id, bear_id).await;
    let app = test_app(pool.clone()).await;
    let cookie = login_cookie(&app, user_id).await;
    let response = post_form(
        &app,
        &cookie,
        &format!("/bear/{bear_slug}/jobs/new"),
        format!(
            "bear_id={bear_id}&goal=Surface+job&surface_id={initial_surface_id}&commit_policy=none\
             &task_title=Use+repo&task_criteria=repo+used"
        ),
    )
    .await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    let job_id: Uuid = sqlx::query_scalar!(
        "SELECT id FROM bear_jobs WHERE bear_id = $1 ORDER BY created_at DESC LIMIT 1",
        bear_id
    )
    .fetch_one(&pool)
    .await
    .expect("job id");
    let surface_name = format!("job-surface-{}", &Uuid::new_v4().simple().to_string()[..12]);
    let response = post_form(
        &app,
        &cookie,
        "/work/surfaces/new",
        format!(
            "name={surface_name}&description=&upstream_url=https%3A%2F%2Fexample.invalid%2Frepo.git\
             &default_ref=main&default_image=&credential_kind=&credential_value=\
             &bear_id={bear_id}&return_job_id={job_id}"
        ),
    )
    .await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    let redirect = response
        .headers()
        .get(header::LOCATION)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();
    let surface_id = sqlx::query_scalar!(
        "SELECT work_surface_id FROM job_work_surface_assignments WHERE job_id = $1",
        job_id
    )
    .fetch_one(&pool)
    .await
    .expect("attached job surface");
    assert!(redirect.starts_with(&format!(
        "/work/surfaces/{}?message=",
        &surface_id.simple().to_string()[..16]
    )));
    assert!(redirect.contains("not%20ready"));
    let assignment_count = sqlx::query_scalar!(
        "SELECT count(*)::bigint AS \"count!: i64\" FROM work_surface_bears \
         WHERE surface_id = $1 AND bear_id = $2",
        surface_id,
        bear_id
    )
    .fetch_one(&pool)
    .await
    .expect("surface assignment");
    assert_eq!(assignment_count, 1);

    let response = post_form(
        &app,
        &cookie,
        &format!("/bear/{bear_slug}/jobs/{}/edit", route_id(job_id)),
        format!("goal=Surface+job&surface_id={surface_id}&commit_policy=per_task&work_branch=main"),
    )
    .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    let response = post_form(
        &app,
        &cookie,
        &format!("/bear/{bear_slug}/jobs/{}/edit", route_id(job_id)),
        format!(
            "goal=Surface+job&surface_id={surface_id}&commit_policy=per_task&work_branch=&allow_default_ref=true"
        ),
    )
    .await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    let branch: Option<String> =
        sqlx::query_scalar!("SELECT work_branch FROM bear_jobs WHERE id = $1", job_id)
            .fetch_one(&pool)
            .await
            .expect("default work branch");
    assert_eq!(branch.as_deref(), Some("main"));
}

#[tokio::test]
async fn dispatch_form_enqueues_run_with_root_and_image() {
    let _guard = TEST_DB_LOCK.lock().await;
    let Some(pool) = test_pool().await else {
        return;
    };
    let (user_id, bear_id, bear_slug) = seed_member(&pool).await;
    let surface_id = assigned_surface_id(&pool, user_id, bear_id).await;
    let app = test_app(pool.clone()).await;
    let cookie = login_cookie(&app, user_id).await;

    // Create the job through the same form, then dispatch its task.
    let body = format!(
        "bear_id={bear_id}&goal=Dispatch+me&surface_id={surface_id}&commit_policy=per_task\
         &task_title=Do+the+thing&task_criteria=thing+is+done\
         &task_title=Do+the+next+thing&task_criteria=next+thing+is+done&allow_default_ref=true"
    );
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/bear/{bear_slug}/jobs/new"))
                .header(header::COOKIE, &cookie)
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .expect("create job response");
    assert_eq!(response.status(), StatusCode::SEE_OTHER);

    let task = sqlx::query!(
        "SELECT id, job_id FROM bear_tasks WHERE bear_id = $1 AND title = 'Do the thing'",
        bear_id
    )
    .fetch_one(&pool)
    .await
    .expect("task id");
    let job_id = task.job_id.expect("task has job");

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!(
                    "/bear/{bear_slug}/jobs/{}/dispatch",
                    route_id(job_id)
                ))
                .header(header::COOKIE, &cookie)
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(Body::from("root=site&image=rust&git_ref="))
                .unwrap(),
        )
        .await
        .expect("dispatch response");
    if response.status() != StatusCode::SEE_OTHER {
        use http_body_util::BodyExt;
        let status = response.status();
        let body = response
            .into_body()
            .collect()
            .await
            .expect("error body")
            .to_bytes();
        panic!(
            "dispatch: expected 303, got {status}: {}",
            String::from_utf8_lossy(&body)
        );
    }
    let redirect = response
        .headers()
        .get(header::LOCATION)
        .and_then(|value| value.to_str().ok())
        .expect("dispatch redirect");

    let runs = sqlx::query!(
        "SELECT id, root_name, image_name, git_ref FROM bear_work_runs
         WHERE job_id = $1 AND state = 'queued' ORDER BY queued_at",
        job_id
    )
    .fetch_all(&pool)
    .await
    .expect("queued job runs");
    assert_eq!(runs.len(), 1);
    let run = &runs[0];
    assert_eq!(
        redirect,
        format!("/bear/{bear_slug}/jobs/runs/{}", route_id(run.id))
    );
    assert!(
        run.root_name.is_none(),
        "dispatch no longer accepts a root override"
    );
    assert_eq!(run.image_name.as_deref(), Some("rust"));
    assert!(run.git_ref.is_none(), "blank git_ref stays unset");
}

/// Helper: POST a form to the app with the session cookie; returns the
/// response.
async fn post_form(
    app: &axum::Router,
    cookie: &str,
    uri: &str,
    body: String,
) -> axum::response::Response {
    app.clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(uri)
                .header(header::COOKIE, cookie)
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .expect("form response")
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
        .expect("GET response");
    let status = response.status();
    let body = String::from_utf8_lossy(&response.into_body().collect().await.unwrap().to_bytes())
        .into_owned();
    (status, body)
}

#[tokio::test]
async fn jobs_and_runs_enforce_member_visibility_before_reads_and_mutations() {
    let _guard = TEST_DB_LOCK.lock().await;
    let Some(pool) = test_pool().await else {
        return;
    };
    let (owner_id, bear_id, slug) = seed_member(&pool).await;
    bears_db::grant_membership(&pool, owner_id, bear_id, Some("member"))
        .await
        .expect("demote owner to member");
    let mut other_users = Vec::new();
    for role in ["member", "admin"] {
        let unique = Uuid::new_v4().simple().to_string();
        let user_id = sqlx::query_scalar!(
            "INSERT INTO users (email, username, display_name, passhash)
             VALUES ($1, $2, $3, $4) RETURNING id",
            format!("work-ui-{unique}@example.test"),
            format!("wu{}", &unique[..28]),
            "Work UI Test",
            "test-passhash",
        )
        .fetch_one(&pool)
        .await
        .expect("create other user");
        bears_db::grant_membership(&pool, user_id, bear_id, Some(role))
            .await
            .expect("grant other membership");
        other_users.push(user_id);
    }
    let surface_id = assigned_surface_id(&pool, owner_id, bear_id).await;
    let app = test_app(pool.clone()).await;
    let owner = login_cookie(&app, owner_id).await;
    let member = login_cookie(&app, other_users[0]).await;
    let admin = login_cookie(&app, other_users[1]).await;

    let mut job_ids = Vec::new();
    for goal in ["Owner secret journal", "Shared job journal"] {
        let response = post_form(
            &app,
            &owner,
            &format!("/bear/{slug}/jobs/new"),
            format!("goal={}&surface_id={surface_id}&commit_policy=per_task&allow_default_ref=true&task_title=Check&task_criteria=done",
                urlencoding::encode(goal)),
        )
        .await;
        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        let id = sqlx::query_scalar!(
            "SELECT id FROM bear_jobs WHERE bear_id = $1 AND goal = $2",
            bear_id,
            goal,
        )
        .fetch_one(&pool)
        .await
        .expect("created job");
        job_ids.push(id);
    }
    let (private, shared) = (job_ids[0], job_ids[1]);
    sqlx::query!(
        "UPDATE bear_jobs SET visibility = 'bear_visible' WHERE id = $1",
        shared,
    )
    .execute(&pool)
    .await
    .expect("share second job");
    let policy = sqlx::query!(
        "SELECT commit_policy, work_branch FROM bear_jobs WHERE id = $1",
        private
    )
    .fetch_one(&pool)
    .await
    .expect("created policy");
    assert_eq!(
        policy.commit_policy.as_deref(),
        Some("per_task"),
        "{policy:?}"
    );
    assert_eq!(policy.work_branch.as_deref(), Some("main"), "{policy:?}");
    let private_task: Uuid = sqlx::query_scalar!(
        "SELECT id FROM bear_tasks WHERE job_id = $1 LIMIT 1",
        private,
    )
    .fetch_one(&pool)
    .await
    .expect("private task");
    let private_run = post_form(
        &app,
        &owner,
        &format!("/bear/{slug}/jobs/{}/dispatch", route_id(private)),
        "image=&git_ref=".into(),
    )
    .await;
    let dispatch_status = private_run.status();
    let dispatch_body = private_run.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(
        dispatch_status,
        StatusCode::SEE_OTHER,
        "{}",
        String::from_utf8_lossy(&dispatch_body)
    );
    let run_id: Uuid = sqlx::query_scalar!(
        "SELECT id FROM bear_work_runs WHERE job_id = $1 ORDER BY queued_at DESC LIMIT 1",
        private,
    )
    .fetch_one(&pool)
    .await
    .expect("private work run");
    sqlx::query!(
        "UPDATE bear_work_runs SET result_refs = $2::jsonb WHERE id = $1",
        run_id,
        serde_json::json!({"log_tail": "secret run log and audit"}),
    )
    .execute(&pool)
    .await
    .expect("seed sensitive log");

    let job_url = format!("/bear/{slug}/jobs/{}", route_id(private));
    let shared_url = format!("/bear/{slug}/jobs/{}", route_id(shared));
    let run_url = format!("/bear/{slug}/jobs/runs/{}", route_id(run_id));
    let (status, listing) = get_page(&app, &member, &format!("/bear/{slug}/jobs")).await;
    assert_eq!(status, StatusCode::OK, "{listing}");
    assert!(!listing.contains("Owner secret journal"));
    assert!(listing.contains("Shared job journal"));
    let (status, _) = get_page(&app, &owner, &job_url).await;
    assert_eq!(status, StatusCode::OK, "owner can inspect private job");
    let (status, _) = get_page(&app, &admin, &job_url).await;
    assert_eq!(status, StatusCode::OK, "admin can inspect private job");
    let (status, _) = get_page(&app, &member, &shared_url).await;
    assert_eq!(status, StatusCode::OK, "member can inspect BearVisible job");
    for url in [&job_url, &run_url] {
        let (status, body) = get_page(&app, &member, url).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "guessed {url}: {body}");
        assert!(!body.contains("secret run log and audit"));
    }
    let (status, _) = get_page(&app, &admin, &run_url).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "admin can inspect private run audit"
    );
    assert_eq!(get_page(&app, &owner, &run_url).await.0, StatusCode::OK);

    let response = post_form(
        &app,
        &owner,
        &format!("{shared_url}/dispatch"),
        "image=&git_ref=".into(),
    )
    .await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    let shared_run: Uuid = sqlx::query_scalar!(
        "SELECT id FROM bear_work_runs WHERE job_id = $1 ORDER BY queued_at DESC LIMIT 1",
        shared,
    )
    .fetch_one(&pool)
    .await
    .expect("shared work run");
    let shared_run_url = format!("/bear/{slug}/jobs/runs/{}", route_id(shared_run));
    assert_eq!(
        get_page(&app, &member, &shared_run_url).await.0,
        StatusCode::OK
    );
    assert_eq!(
        post_form(
            &app,
            &member,
            &format!("{shared_run_url}/cancel"),
            String::new()
        )
        .await
        .status(),
        StatusCode::SEE_OTHER,
        "member can control BearVisible work run"
    );

    for restricted in ["private_to_profile", "handoff_requested"] {
        sqlx::query!(
            "UPDATE bear_jobs SET visibility = $2 WHERE id = $1",
            private,
            restricted
        )
        .execute(&pool)
        .await
        .expect("set restricted visibility");
        assert_eq!(
            get_page(&app, &member, &job_url).await.0,
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            get_page(&app, &member, &run_url).await.0,
            StatusCode::NOT_FOUND
        );
        assert_eq!(get_page(&app, &owner, &job_url).await.0, StatusCode::OK);
        assert_eq!(get_page(&app, &admin, &job_url).await.0, StatusCode::OK);
    }
    sqlx::query!(
        "UPDATE bear_jobs SET visibility = 'same_user' WHERE id = $1",
        private
    )
    .execute(&pool)
    .await
    .expect("restore SameUser visibility");

    let (_, foreign_bear_id, foreign_slug) = seed_member(&pool).await;
    bears_db::grant_membership(&pool, other_users[0], foreign_bear_id, Some("member"))
        .await
        .expect("member belongs to foreign Bear too");
    for url in [
        format!("/bear/{foreign_slug}/jobs/{}", route_id(private)),
        format!("/bear/{foreign_slug}/jobs/runs/{}", route_id(run_id)),
    ] {
        assert_eq!(
            get_page(&app, &member, &url).await.0,
            StatusCode::NOT_FOUND,
            "cross-Bear guessed URL {url}"
        );
    }

    for suffix in [
        format!("{shared_url}/tasks/{}/children", route_id(private_task)),
        format!("{shared_url}/tasks/{}/move/up", route_id(private_task)),
        format!("{shared_url}/tasks/{}/retry", route_id(private_task)),
    ] {
        assert_eq!(
            post_form(
                &app,
                &member,
                &suffix,
                "title=No&criteria=done&reason=No".into()
            )
            .await
            .status(),
            StatusCode::NOT_FOUND,
            "task from different job: {suffix}"
        );
    }

    let rejected = [
        (
            format!("{job_url}/edit"),
            format!("goal=Hacked&surface_id={surface_id}&commit_policy=per_task"),
        ),
        (format!("{job_url}/duplicate"), String::new()),
        (format!("{job_url}/complete"), String::new()),
        (format!("{job_url}/archive"), String::new()),
        (
            format!("{job_url}/tasks"),
            "title=Hacked&criteria=done".into(),
        ),
        (format!("{job_url}/dispatch"), "image=&git_ref=".into()),
        (format!("{job_url}/cancel"), String::new()),
        (
            format!("{job_url}/tasks/{}/children", route_id(private_task)),
            "title=Hacked&criteria=done".into(),
        ),
        (
            format!("{job_url}/tasks/{}/move/up", route_id(private_task)),
            String::new(),
        ),
        (
            format!("{job_url}/tasks/{}/retry", route_id(private_task)),
            "reason=Hacked".into(),
        ),
        (format!("{run_url}/pause"), String::new()),
        (format!("{run_url}/resume"), String::new()),
        (format!("{run_url}/cancel"), String::new()),
        (format!("{run_url}/retry"), String::new()),
    ];
    for (url, body) in rejected {
        let response = post_form(&app, &member, &url, body).await;
        assert_eq!(
            response.status(),
            StatusCode::NOT_FOUND,
            "cross-member POST {url}"
        );
    }
    let private_job = sqlx::query!(
        "SELECT goal, visibility FROM bear_jobs WHERE id = $1",
        private,
    )
    .fetch_one(&pool)
    .await
    .expect("unchanged private job");
    assert_eq!(private_job.goal, "Owner secret journal");
    assert_eq!(private_job.visibility, "same_user");
    let task_count: i64 =
        sqlx::query_scalar!("SELECT count(*) FROM bear_tasks WHERE job_id = $1", private,)
            .fetch_one(&pool)
            .await
            .expect("unchanged private tasks")
            .expect("count");
    assert_eq!(task_count, 1);
    let run_count: i64 = sqlx::query_scalar!(
        "SELECT count(*) FROM bear_work_runs WHERE job_id = $1",
        private,
    )
    .fetch_one(&pool)
    .await
    .expect("unchanged private runs")
    .expect("count");
    assert_eq!(run_count, 1);

    // Filtering must precede LIMIT: the newer private job cannot consume the
    // sole slot of the member's viewer-scoped Docket listing.
    sqlx::query!(
        "UPDATE bear_jobs SET updated_at = NOW() + INTERVAL '1 hour' WHERE id = $1",
        private
    )
    .execute(&pool)
    .await
    .expect("rank private job first");
    let visible = PgDocketService::from_pool(&pool)
        .list_jobs_for_viewer(
            bear_id,
            other_users[0],
            false,
            DocketJobListFilter {
                limit: 1,
                ..DocketJobListFilter::default()
            },
        )
        .await
        .expect("viewer-scoped Docket list");
    assert_eq!(visible.len(), 1);
    assert_eq!(visible[0].id, shared);

    assert_eq!(
        post_form(
            &app,
            &owner,
            &format!("{job_url}/edit"),
            format!("goal=Owner+updated&surface_id={surface_id}&commit_policy=per_task")
        )
        .await
        .status(),
        StatusCode::SEE_OTHER,
        "owner can control private job"
    );
    assert_eq!(
        post_form(
            &app,
            &admin,
            &format!("{job_url}/tasks"),
            "title=Admin+task&criteria=done".into()
        )
        .await
        .status(),
        StatusCode::SEE_OTHER,
        "admin can control private job"
    );
    assert_eq!(
        post_form(
            &app,
            &member,
            &format!("{shared_url}/edit"),
            format!("goal=Shared+updated&surface_id={surface_id}&commit_policy=per_task")
        )
        .await
        .status(),
        StatusCode::SEE_OTHER,
        "member can control BearVisible job"
    );
}

#[test]
fn unknown_visibility_is_not_accessible_even_to_owner_or_admin() {
    let bear = BearContext {
        id: Uuid::nil(),
        slug: "test".into(),
        viewer_id: 42,
        is_admin: true,
    };
    assert!(!visible_job(42, "future_visibility", &bear));
    let member = BearContext {
        is_admin: false,
        ..bear
    };
    for visibility in [
        TaskListVisibility::SameUser,
        TaskListVisibility::PrivateToProfile,
        TaskListVisibility::HandoffRequested,
    ] {
        assert!(!visible_job(43, visibility.as_str(), &member));
    }
    assert!(visible_job(
        43,
        TaskListVisibility::BearVisible.as_str(),
        &member
    ));
    assert!(visible_job(
        42,
        TaskListVisibility::SameUser.as_str(),
        &member
    ));
}

#[test]
fn cargo_registry_network_diagnostic_is_actionable() {
    let diagnostic = run_diagnostic(
        Some("cargo test timed out while Cargo attempted to update the crates.io index"),
        None,
        "spurious network error: TLS transfer failed",
    )
    .expect("Cargo registry failure is recognized");
    assert_eq!(diagnostic.title, "Cargo dependency access failed");
    assert!(diagnostic.recovery.contains("retry the blocked task"));
}

#[test]
fn unrelated_timeout_does_not_claim_network_diagnosis() {
    assert!(run_diagnostic(Some("worker timed out"), None, "").is_none());
}

#[tokio::test]
async fn surface_management_is_owner_scoped_and_grantable() {
    let _guard = TEST_DB_LOCK.lock().await;
    let Some(pool) = test_pool().await else {
        return;
    };
    let (owner_id, _bear_id, _bear_slug) = seed_member(&pool).await;
    let (other_id, _other_bear, _other_bear_slug) = seed_member(&pool).await;
    let app = test_app(pool.clone()).await;
    let owner_cookie = login_cookie(&app, owner_id).await;
    let other_cookie = login_cookie(&app, other_id).await;

    let unique = Uuid::new_v4().simple().to_string();
    let name = format!("ui-surface-{}", &unique[..12]);
    let response = post_form(
        &app,
        &owner_cookie,
        "/work/surfaces/new",
        format!(
            "name={name}&description=&upstream_url=https%3A%2F%2Fexample.invalid%2Frepo.git\
             &default_ref=main&default_image=&credential_kind=&credential_value="
        ),
    )
    .await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    let surface = sqlx::query!(
        "SELECT id, created_by_user_id FROM work_surfaces WHERE name = $1",
        &name
    )
    .fetch_one(&pool)
    .await
    .expect("surface row");
    let surface_id = surface.id;
    assert_eq!(surface.created_by_user_id, owner_id);

    // Non-manager: manage page and mutations deny as 404.
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/work/surfaces/{surface_id}"))
                .header(header::COOKIE, &other_cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("detail response");
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let response = post_form(
        &app,
        &other_cookie,
        &format!("/work/surfaces/{surface_id}/update"),
        "description=x&upstream_url=https%3A%2F%2Fevil.invalid%2Fr.git&default_ref=main&default_image=".to_string(),
    )
    .await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    // Owner grants the other user; the grantee can now update.
    let other_username: String =
        sqlx::query_scalar!("SELECT username FROM users WHERE id = $1", other_id)
            .fetch_one(&pool)
            .await
            .expect("username");
    let response = post_form(
        &app,
        &owner_cookie,
        &format!("/work/surfaces/{surface_id}/managers/grant"),
        format!("username={other_username}&role=manager"),
    )
    .await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    let response = post_form(
        &app,
        &other_cookie,
        &format!("/work/surfaces/{surface_id}/update"),
        "description=updated+by+manager&upstream_url=https%3A%2F%2Fexample.invalid%2Frepo.git&default_ref=trunk&default_image=".to_string(),
    )
    .await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    let default_ref = sqlx::query_scalar!(
        "SELECT default_ref FROM git_work_surface_details WHERE id = $1",
        surface_id
    )
    .fetch_one(&pool)
    .await
    .expect("updated row");
    assert_eq!(default_ref, "trunk");
}

#[tokio::test]
async fn create_job_enforces_surface_assignment() {
    let _guard = TEST_DB_LOCK.lock().await;
    let Some(pool) = test_pool().await else {
        return;
    };
    let (user_id, bear_id, bear_slug) = seed_member(&pool).await;
    let app = test_app(pool.clone()).await;
    let cookie = login_cookie(&app, user_id).await;

    let unique = Uuid::new_v4().simple().to_string();
    let name = format!("job-surface-{}", &unique[..12]);
    let response = post_form(
        &app,
        &cookie,
        "/work/surfaces/new",
        format!(
            "name={name}&description=&upstream_url=https%3A%2F%2Fexample.invalid%2Frepo.git\
             &default_ref=main&default_image=&credential_kind=&credential_value="
        ),
    )
    .await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    let surface_id = sqlx::query_scalar!("SELECT id FROM work_surfaces WHERE name = $1", &name)
        .fetch_one(&pool)
        .await
        .expect("surface row");

    // The bear is not assigned: job creation with the surface is rejected.
    let job_body = format!(
        "bear_id={bear_id}&goal=Surface+gated&surface_id={surface_id}&root=&commit_policy=per_task\
         &task_title=Do+it&task_criteria=done"
    );
    let response = post_form(
        &app,
        &cookie,
        &format!("/bear/{bear_slug}/jobs/new"),
        job_body.clone(),
    )
    .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    // Assign the bear from the surface page, then the same form succeeds and
    // binds the canonical surface id.
    let response = post_form(
        &app,
        &cookie,
        &format!("/work/surfaces/{surface_id}/bears/assign"),
        format!("bear_id={bear_id}"),
    )
    .await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    let response = post_form(
        &app,
        &cookie,
        &format!("/bear/{bear_slug}/jobs/new"),
        job_body,
    )
    .await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    let job_id = sqlx::query_scalar!(
        "SELECT id FROM bear_jobs WHERE bear_id = $1 ORDER BY created_at DESC LIMIT 1",
        bear_id
    )
    .fetch_one(&pool)
    .await
    .expect("job row");
    assert_job_uses_surface(&pool, job_id, surface_id).await;
}

#[tokio::test]
async fn member_creates_a_job_bound_to_a_work_enabled_hat_atomically() {
    use den_core::ids::{BearId, UserId};
    use den_service::bears::hats;
    let _guard = TEST_DB_LOCK.lock().await;
    let Some(pool) = test_pool().await else {
        return;
    };
    let (admin, bear_id, slug) = seed_member(&pool).await;
    let surface = assigned_surface_id(&pool, admin, bear_id).await;
    let hat = hats::create_hat(
        &pool,
        BearId::new(bear_id),
        UserId::new(admin),
        "Work review",
        "Review repository changes",
    )
    .await
    .unwrap();
    hats::allow_surface(&pool, BearId::new(bear_id), hat.id, surface)
        .await
        .unwrap();
    sqlx::query!(
        "UPDATE bear_hats SET work_enabled = true WHERE id = $1",
        hat.id.as_uuid()
    )
    .execute(&pool)
    .await
    .unwrap(); // Empty test hat has passed the fixture's Work review.
    let nonce = Uuid::new_v4().simple().to_string();
    let member = sqlx::query_scalar!(
        "INSERT INTO users (email, username, display_name, passhash) VALUES ($1, $2, 'Hat Work Member', 'x') RETURNING id",
        format!("workhat-{nonce}@example.test"), format!("wh{}", &nonce[..12]),
    ).fetch_one(&pool).await.unwrap();
    bears_db::grant_membership(&pool, member, bear_id, Some(bears_db::BEAR_ROLE_MEMBER))
        .await
        .unwrap();
    let app = test_app(pool.clone()).await;
    let cookie = login_cookie(&app, member).await;
    let (page_status, page) = get_page(&app, &cookie, &format!("/bear/{slug}/jobs/new")).await;
    assert_eq!(page_status, StatusCode::OK, "{page}");
    assert!(
        page.contains("Work review"),
        "members should be offered configured Work hats"
    );
    let endpoint = format!("/bear/{slug}/jobs/new");
    let base = format!("goal=Check+dependencies+{nonce}&surface_id={surface}&commit_policy=none&task_title=Inspect+dependencies&task_criteria=List+outdated+packages");
    assert_eq!(
        post_form(&app, &cookie, &endpoint, base.clone())
            .await
            .status(),
        StatusCode::BAD_REQUEST,
        "a configured Bear must not silently fall back to an unbound Job"
    );
    let foreign = format!("{base}&hat_id={}", Uuid::new_v4());
    assert_eq!(
        post_form(&app, &cookie, &endpoint, foreign).await.status(),
        StatusCode::FORBIDDEN
    );
    let jobs_before: i64 = sqlx::query_scalar!(
        "SELECT count(*) AS \"count!: i64\" FROM bear_jobs WHERE bear_id = $1 AND created_by_user_id = $2",
        bear_id, member,
    ).fetch_one(&pool).await.unwrap();
    assert_eq!(
        jobs_before, 0,
        "failed hat selection must not leave an unbound Job"
    );
    let response = post_form(
        &app,
        &cookie,
        &endpoint,
        format!("{base}&hat_id={}", hat.id),
    )
    .await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    let job = sqlx::query!(
        "SELECT id, hat_id, current_run_id FROM bear_jobs WHERE bear_id = $1 AND created_by_user_id = $2 ORDER BY created_at DESC LIMIT 1",
        bear_id, member,
    ).fetch_one(&pool).await.unwrap();
    assert_eq!(job.hat_id, Some(hat.id.as_uuid()));
    assert!(
        job.current_run_id.is_some(),
        "hat is bound before Docket creates the initial run"
    );
    assert_eq!(
        hats::bindings::eligible_job_hat(&pool, BearId::new(bear_id), job.id)
            .await
            .unwrap(),
        Some(hat.id)
    );
}
