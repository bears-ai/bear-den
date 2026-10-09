use super::{assert_denied, fixture::fixture};
use crate::{bears::hats, connections, repository::RepositoryHeadPolicy};
use den_core::{ArmatureAvailability, EffectivePolicy, Governance, TurnExecutionOrigin};
use den_repository::RepositoryAuthorizer;
use sqlx::PgPool;
use uuid::Uuid;

#[sqlx::test(migrations = "../../migrations")]
async fn work_requires_exact_live_job_creator_run_assignment_and_hat(pool: PgPool) {
    let mut f = fixture(&pool).await;
    sqlx::query!(
        "UPDATE bear_hats SET work_enabled=true WHERE id=$1",
        f.hat.as_uuid()
    )
    .execute(&pool)
    .await
    .unwrap();
    let job = sqlx::query_scalar!("INSERT INTO bear_jobs (bear_id,created_by_user_id,created_by_role,goal) VALUES ($1,$2,'ui','Read upstream head') RETURNING id", f.bear.as_uuid(), f.owner.get()).fetch_one(&pool).await.unwrap();
    sqlx::query!(
        "INSERT INTO job_work_surface_assignments (job_id,work_surface_id) VALUES ($1,$2)",
        job,
        f.surface.0
    )
    .execute(&pool)
    .await
    .unwrap();
    hats::bindings::bind_job_hat(&pool, f.bear, job, f.hat)
        .await
        .unwrap();
    let job_run = sqlx::query_scalar!(
        "INSERT INTO bear_job_runs (job_id) VALUES ($1) RETURNING id",
        job
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    sqlx::query!(
        "UPDATE bear_jobs SET current_run_id=$2 WHERE id=$1",
        job,
        job_run
    )
    .execute(&pool)
    .await
    .unwrap();
    let work = sqlx::query_scalar!("INSERT INTO bear_work_runs (bear_id,job_id,job_run_id,state,bearwire_session_id,lease_expires_at) VALUES ($1,$2,$3,'running',$4,now()+interval '60 seconds') RETURNING id", f.bear.as_uuid(), job, job_run, &f.context.session_id).fetch_one(&pool).await.unwrap();
    let origin = TurnExecutionOrigin::AuthorizedWorkRun(ArmatureAvailability::Connected);
    f.context.profile =
        Some(EffectivePolicy::compile_for_origin(origin, Governance::Interactive).context_label);
    f.context.work_run_id = Some(work);
    f.context.binding_id = hats::turn_binding::NativeTurnSource::WorkRun(work).binding_id(f.bear);
    assert!(RepositoryHeadPolicy {
        pool: &pool,
        context: &f.context,
        origin,
        governance: Governance::Interactive
    }
    .authorize(f.surface)
    .await
    .is_ok());
    let other_job = sqlx::query_scalar!(
        "INSERT INTO bear_jobs (bear_id,created_by_user_id,created_by_role,goal) VALUES ($1,$2,'ui','Other Job') RETURNING id",
        f.bear.as_uuid(), f.owner.get(),
    ).fetch_one(&pool).await.unwrap();
    let foreign_run = sqlx::query_scalar!(
        "INSERT INTO bear_job_runs (job_id) VALUES ($1) RETURNING id",
        other_job
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    // Both scalar IDs agree but the run belongs to another Job; equality is not ownership.
    sqlx::query!(
        "UPDATE bear_jobs SET current_run_id=$2 WHERE id=$1",
        job,
        foreign_run
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query!(
        "UPDATE bear_work_runs SET job_run_id=$2 WHERE id=$1",
        work,
        foreign_run
    )
    .execute(&pool)
    .await
    .unwrap();
    assert_denied(&pool, &f.context, f.surface, origin).await;
    sqlx::query!(
        "UPDATE bear_work_runs SET job_run_id=$2 WHERE id=$1",
        work,
        job_run
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query!(
        "UPDATE bear_jobs SET current_run_id=$2 WHERE id=$1",
        job,
        job_run
    )
    .execute(&pool)
    .await
    .unwrap();
    let mut substitution = f.context.clone();
    substitution.work_run_id = Some(Uuid::new_v4());
    assert_denied(&pool, &substitution, f.surface, origin).await;
    substitution = f.context.clone();
    substitution.user_id = f.other.get();
    assert_denied(&pool, &substitution, f.surface, origin).await;
    assert!(connections::detach(&pool, f.owner, f.surface.0)
        .await
        .is_err());
    assert!(
        connections::attach(&pool, f.owner, f.connection, f.surface.0)
            .await
            .is_err()
    );
    sqlx::query!("UPDATE bear_work_runs SET state='paused' WHERE id=$1", work)
        .execute(&pool)
        .await
        .unwrap();
    assert!(connections::detach(&pool, f.owner, f.surface.0)
        .await
        .is_err());
    assert!(
        connections::attach(&pool, f.owner, f.connection, f.surface.0)
            .await
            .is_err()
    );
    let attached = sqlx::query_scalar!(
        "SELECT connection_id FROM git_work_surface_details WHERE id=$1",
        f.surface.0
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(attached, Some(f.connection.0));
    sqlx::query!(
        "UPDATE bear_work_runs SET state='running' WHERE id=$1",
        work
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query!(
        "UPDATE bear_work_runs SET cancel_requested=true WHERE id=$1",
        work
    )
    .execute(&pool)
    .await
    .unwrap();
    assert_denied(&pool, &f.context, f.surface, origin).await;
    sqlx::query!(
        "UPDATE bear_work_runs SET cancel_requested=false WHERE id=$1",
        work
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query!(
        "UPDATE bear_work_runs SET lease_expires_at=now()-interval '1 second' WHERE id=$1",
        work
    )
    .execute(&pool)
    .await
    .unwrap();
    assert_denied(&pool, &f.context, f.surface, origin).await;
    sqlx::query!(
        "UPDATE bear_work_runs SET lease_expires_at=now()+interval '60 seconds' WHERE id=$1",
        work
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query!(
        "DELETE FROM job_work_surface_assignments WHERE job_id=$1",
        job
    )
    .execute(&pool)
    .await
    .unwrap();
    assert_denied(&pool, &f.context, f.surface, origin).await;
}
