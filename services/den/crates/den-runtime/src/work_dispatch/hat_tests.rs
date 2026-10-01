use std::sync::Arc;

use den_core::{
    config::Config,
    ids::{BearId, UserId},
};
use den_docket::work_runs;
use den_sandbox::SandboxClient;
use den_service::{
    bears::{db, hats},
    work_surfaces::{self, NewWorkSurface},
};
use sqlx::PgPool;

use super::provision_run;

#[sqlx::test(migrations = "../../migrations")]
async fn configured_hats_fail_unbound_work_before_credentials_or_sandbox_provisioning(
    pool: PgPool,
) {
    let user = sqlx::query_scalar!(
        "INSERT INTO users (email, username) VALUES ('dispatch-hat@example.test', 'dispatchhat') RETURNING id"
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let bear = sqlx::query_scalar!(
        "INSERT INTO bears (slug, name) VALUES ('dispatchhatbear', 'Dispatch Hat') RETURNING id"
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let job = sqlx::query_scalar!(
        "INSERT INTO bear_jobs (bear_id, created_by_user_id, created_by_role, goal)
         VALUES ($1, $2, 'ui', 'Old unbound Work') RETURNING id",
        bear,
        user,
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let job_run = sqlx::query_scalar!(
        "INSERT INTO bear_job_runs (job_id) VALUES ($1) RETURNING id",
        job,
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let run_id = sqlx::query_scalar!(
        "INSERT INTO bear_work_runs (bear_id, job_id, job_run_id)
         VALUES ($1, $2, $3) RETURNING id",
        bear,
        job,
        job_run,
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    hats::create_hat(
        &pool,
        BearId::new(bear),
        UserId::new(user),
        "Work review",
        "Do not dispatch old unbound Jobs",
    )
    .await
    .unwrap();
    let run = work_runs::get_work_run(&pool, run_id)
        .await
        .unwrap()
        .unwrap();
    let config = Arc::new(Config::test_stub());
    let client = SandboxClient::new("http://127.0.0.1:1", "test");
    provision_run(&pool, &config, &client, &run).await;

    let failed = work_runs::get_work_run(&pool, run_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(failed.state, "failed");
    assert!(failed
        .error
        .as_deref()
        .unwrap()
        .contains("work_hat_ineligible"));
    assert!(failed.sandbox_id.is_none());
    assert!(failed.executing_task_id.is_none());
    assert!(failed.bearwire_session_id.is_none());
    let attempts = sqlx::query_scalar!(
        "SELECT count(*) AS \"count!\" FROM docket_execution_attempts
         WHERE binding_kind = 'work_assignment' AND binding_id = $1",
        run_id.to_string(),
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(attempts, 0);
}

#[sqlx::test(migrations = "../../migrations")]
async fn hat_bound_work_refuses_open_or_incapable_provider_before_starting_task_or_minting_token(
    pool: PgPool,
) {
    let user = sqlx::query_scalar!(
        "INSERT INTO users (email, username) VALUES ('dispatch-hat@example.test', 'dispatchhat') RETURNING id"
    ).fetch_one(&pool).await.unwrap();
    let bear = sqlx::query_scalar!(
        "INSERT INTO bears (slug, name) VALUES ('dispatchhatbear', 'Dispatch Hat') RETURNING id"
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    db::grant_membership(&pool, user, bear, Some(db::BEAR_ROLE_ADMIN))
        .await
        .unwrap();
    let surface = work_surfaces::create_surface(
        &pool,
        user,
        NewWorkSurface {
            name: "dispatch-hat-root".into(),
            description: None,
            upstream_url: "https://example.test/repo.git".into(),
            default_ref: "main".into(),
            default_image: None,
            allowed_outbound_hosts: vec!["docs.example.com".into()],
            credential: None,
        },
        "",
    )
    .await
    .unwrap();
    work_surfaces::assign_bear(&pool, surface.id, bear, user)
        .await
        .unwrap();
    let hat = hats::create_hat(
        &pool,
        BearId::new(bear),
        UserId::new(user),
        "Work review",
        "Review repository",
    )
    .await
    .unwrap();
    hats::allow_surface(&pool, BearId::new(bear), hat.id, surface.id)
        .await
        .unwrap();
    sqlx::query!(
        "UPDATE bear_hats SET work_enabled = true WHERE id = $1",
        hat.id.as_uuid()
    )
    .execute(&pool)
    .await
    .unwrap();
    let job = sqlx::query_scalar!(
        "INSERT INTO bear_jobs (bear_id, created_by_user_id, created_by_role, goal)
         VALUES ($1, $2, 'ui', 'Old unbound Work') RETURNING id",
        bear,
        user,
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    sqlx::query!(
        "INSERT INTO job_work_surface_assignments (job_id, work_surface_id) VALUES ($1, $2)",
        job,
        surface.id,
    )
    .execute(&pool)
    .await
    .unwrap();
    hats::bindings::bind_job_hat(&pool, BearId::new(bear), job, hat.id)
        .await
        .unwrap();
    let job_run = sqlx::query_scalar!(
        "INSERT INTO bear_job_runs (job_id) VALUES ($1) RETURNING id",
        job,
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let run_id = sqlx::query_scalar!(
        "INSERT INTO bear_work_runs (bear_id, job_id, job_run_id)
         VALUES ($1, $2, $3) RETURNING id",
        bear,
        job,
        job_run,
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let run = work_runs::get_work_run(&pool, run_id)
        .await
        .unwrap()
        .unwrap();
    let mut config = Config::test_stub();
    config.work_sandbox_network = "open".into();
    let client = SandboxClient::new("http://127.0.0.1:1", "test");
    provision_run(&pool, &Arc::new(config), &client, &run).await;
    let failed = work_runs::get_work_run(&pool, run_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(failed.state, "failed");
    assert!(failed
        .error
        .as_deref()
        .unwrap()
        .contains("work_hat_open_network"));
    assert!(failed.sandbox_id.is_none());
    assert!(failed.executing_task_id.is_none());
    let minted = sqlx::query_scalar!(
        "SELECT count(*) AS \"count!\" FROM docket_execution_attempts
         WHERE binding_kind = 'work_assignment' AND binding_id = $1",
        run_id.to_string(),
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(minted, 0);

    let second_job = sqlx::query_scalar!(
        "INSERT INTO bear_jobs (bear_id, created_by_user_id, created_by_role, goal)
         VALUES ($1, $2, 'ui', 'Old unbound Work') RETURNING id",
        bear,
        user,
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    sqlx::query!(
        "INSERT INTO job_work_surface_assignments (job_id, work_surface_id) VALUES ($1, $2)",
        second_job,
        surface.id,
    )
    .execute(&pool)
    .await
    .unwrap();
    hats::bindings::bind_job_hat(&pool, BearId::new(bear), second_job, hat.id)
        .await
        .unwrap();
    let second_job_run = sqlx::query_scalar!(
        "INSERT INTO bear_job_runs (job_id) VALUES ($1) RETURNING id",
        second_job,
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let second_run_id = sqlx::query_scalar!(
        "INSERT INTO bear_work_runs (bear_id, job_id, job_run_id)
         VALUES ($1, $2, $3) RETURNING id",
        bear,
        second_job,
        second_job_run,
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let second_run = work_runs::get_work_run(&pool, second_run_id)
        .await
        .unwrap()
        .unwrap();
    provision_run(&pool, &Arc::new(Config::test_stub()), &client, &second_run).await;
    let failed = work_runs::get_work_run(&pool, second_run_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(failed.state, "failed");
    assert!(failed
        .error
        .as_deref()
        .unwrap()
        .contains("work_hat_provider_capability"));
    assert!(failed.sandbox_id.is_none());
    assert!(failed.executing_task_id.is_none());
    let minted = sqlx::query_scalar!(
        "SELECT count(*) AS \"count!\" FROM docket_execution_attempts
         WHERE binding_kind = 'work_assignment' AND binding_id = $1",
        second_run_id.to_string(),
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(minted, 0);
}
