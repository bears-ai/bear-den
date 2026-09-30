use den_core::ids::HatId;
use sqlx::PgPool;
use uuid::Uuid;

use crate::{
    integration_tests::{seed_user_and_bear, two_task_job},
    PgDocketService,
};

#[sqlx::test(migrations = "../../migrations")]
async fn selected_work_hat_is_checked_and_bound_in_the_job_creation_transaction(pool: PgPool) {
    let (user, bear) = seed_user_and_bear(&pool, "hat-job-owner").await;
    let service = PgDocketService::from_pool(&pool);
    let hat = HatId::new(Uuid::new_v4());
    sqlx::query!(
        "INSERT INTO bear_hats (id, bear_id, name, purpose, identity_prompt, created_by_user_id) VALUES ($1, $2, 'Work review', 'Review code', 'Review code', $3)",
        hat.as_uuid(), bear, user,
    ).execute(&pool).await.unwrap();
    let before = sqlx::query_scalar!(
        "SELECT count(*) AS \"count!: i64\" FROM bear_jobs WHERE bear_id = $1",
        bear,
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(
        service
            .create_job_with_hat(two_task_job(user, bear), hat)
            .await
            .is_err(),
        "a disabled hat cannot produce an unbound fallback Job"
    );
    sqlx::query!(
        "UPDATE bear_hats SET work_enabled = true WHERE id = $1",
        hat.as_uuid()
    )
    .execute(&pool)
    .await
    .unwrap();
    assert!(
        service
            .create_job_with_hat(two_task_job(user, bear), hat)
            .await
            .is_err(),
        "the hat still lacks a grant for the Bear's work surface"
    );
    let foreign_bear = seed_user_and_bear(&pool, "hat-job-foreign").await.1;
    let foreign_hat = HatId::new(Uuid::new_v4());
    sqlx::query!(
        "INSERT INTO bear_hats (id, bear_id, name, purpose, identity_prompt, work_enabled, created_by_user_id) VALUES ($1, $2, 'Other hat', 'Other Bear', 'Other Bear', true, $3)",
        foreign_hat.as_uuid(), foreign_bear, user,
    ).execute(&pool).await.unwrap();
    assert!(service
        .create_job_with_hat(two_task_job(user, bear), foreign_hat)
        .await
        .is_err());
    assert_eq!(
        sqlx::query_scalar!(
            "SELECT count(*) AS \"count!: i64\" FROM bear_jobs WHERE bear_id = $1",
            bear,
        )
        .fetch_one(&pool)
        .await
        .unwrap(),
        before,
        "failed creation must not leave a Job"
    );
    sqlx::query!(
        "INSERT INTO bear_hat_work_surfaces (bear_id, hat_id, surface_id) VALUES ($1, $2, $3)",
        bear,
        hat.as_uuid(),
        bear,
    )
    .execute(&pool)
    .await
    .unwrap();
    let created = service
        .create_job_with_hat(two_task_job(user, bear), hat)
        .await
        .unwrap();
    assert!(
        created.current_run.is_some(),
        "Docket creates its initial run at Job creation"
    );
    let stored_hat = sqlx::query_scalar!(
        "SELECT hat_id FROM bear_jobs WHERE bear_id = $1 AND id = $2",
        bear,
        created.job.id,
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(stored_hat, Some(hat.as_uuid()));
}
