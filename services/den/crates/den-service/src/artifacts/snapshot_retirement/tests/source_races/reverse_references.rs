use super::*;

#[derive(Clone, Copy)]
enum ReverseKind {
    CurrentRun,
    WorkRun,
}
#[derive(Clone, Copy)]
struct Foreign {
    job: Uuid,
    run: Uuid,
    work: Uuid,
}
async fn owned_run(pool: &PgPool, f: &Fixture) -> Uuid {
    sqlx::query_scalar!("SELECT current_run_id FROM bear_jobs WHERE id=$1", f.job)
        .fetch_one(pool)
        .await
        .unwrap()
        .unwrap()
}
async fn foreign(pool: &PgPool, f: &Fixture) -> Foreign {
    let bear=sqlx::query_scalar!("INSERT INTO bears(slug,name,description) VALUES('reverse-foreign-bear','FOREIGN PRIVATE BEAR','') RETURNING id").fetch_one(pool).await.unwrap();
    let job=sqlx::query_scalar!("INSERT INTO bear_jobs(bear_id,created_by_user_id,created_by_role,goal,lifecycle_intent) VALUES($1,$2,'ui','FOREIGN PRIVATE GOAL','cancelled') RETURNING id",bear,f.peer.get()).fetch_one(pool).await.unwrap();
    let run=sqlx::query_scalar!("INSERT INTO bear_job_runs(job_id,state,finished_at) VALUES($1,'completed',NOW()) RETURNING id",job).fetch_one(pool).await.unwrap();
    sqlx::query!(
        "UPDATE bear_jobs SET current_run_id=$2 WHERE id=$1",
        job,
        run
    )
    .execute(pool)
    .await
    .unwrap();
    let work=sqlx::query_scalar!("INSERT INTO bear_work_runs(bear_id,job_id,job_run_id,state,finished_at) VALUES($1,$2,$3,'succeeded',NOW()) RETURNING id",bear,job,run).fetch_one(pool).await.unwrap();
    Foreign { job, run, work }
}
async fn refer(
    tx: &mut Transaction<'_, Postgres>,
    other: Foreign,
    run: Uuid,
    kind: ReverseKind,
) -> Result<(), sqlx::Error> {
    match kind {
        ReverseKind::CurrentRun => {
            sqlx::query!(
                "UPDATE bear_jobs SET current_run_id=$2 WHERE id=$1",
                other.job,
                run
            )
            .execute(&mut **tx)
            .await?;
        }
        ReverseKind::WorkRun => {
            sqlx::query!(
                "UPDATE bear_work_runs SET job_run_id=$2 WHERE id=$1",
                other.work,
                run
            )
            .execute(&mut **tx)
            .await?;
        }
    }
    Ok(())
}
async fn foreign_rows(pool: &PgPool, other: Foreign) -> serde_json::Value {
    sqlx::query_scalar!(r#"SELECT jsonb_build_object('job',to_jsonb(j),'run',to_jsonb(r),'work',to_jsonb(w)) AS "rows!"
        FROM bear_jobs j JOIN bear_job_runs r ON r.id=$2 JOIN bear_work_runs w ON w.id=$3 WHERE j.id=$1"#,other.job,other.run,other.work).fetch_one(pool).await.unwrap()
}
async fn existing_reverse_reference(pool: PgPool, kind: ReverseKind, action: Action) {
    let f = fixture(&pool).await;
    let other = foreign(&pool, &f).await;
    let attempt = prepare(&pool, &f, action).await;
    let run = owned_run(&pool, &f).await;
    let mut writer = pool.begin().await.unwrap();
    refer(&mut writer, other, run, kind).await.unwrap();
    writer.commit().await.unwrap();
    let before = foreign_rows(&pool, other).await;
    assert!(perform(&pool, attempt).await.is_err());
    assert_audit_present(&pool, &f, action).await;
    match action {
        Action::Retire => assert_eq!(
            preview(&pool, f.owner, &f.reference).await.unwrap().blocker,
            Some(RetirementBlocker::RequiredReferences)
        ),
        Action::DeleteBear => {
            let report = removal::preview(&pool, f.owner, f.bear).await.unwrap();
            assert!(report
                .blockers
                .contains(&removal::BearDeletionBlocker::RequiredReferences));
            let encoded = serde_json::to_string(&report).unwrap();
            assert!(!encoded.contains("FOREIGN PRIVATE"));
            assert!(!encoded.contains(&other.job.to_string()));
            assert!(db::delete_bear(&pool, f.bear.as_uuid()).await.is_err());
        }
    }
    assert_eq!(
        before,
        foreign_rows(&pool, other).await,
        "no foreign SET NULL or CASCADE remediation"
    );
}
async fn new_reference_after_target_fence(pool: PgPool, kind: ReverseKind, action: Action) {
    let f = fixture(&pool).await;
    let other = foreign(&pool, &f).await;
    let before = foreign_rows(&pool, other).await;
    let attempt = prepare(&pool, &f, action).await;
    let run = owned_run(&pool, &f).await;
    let mut gate = pool.begin().await.unwrap();
    let gate_pid = backend(&mut gate).await;
    sqlx::query!(
        "SELECT id FROM artifacts WHERE artifact_ref=$1 FOR NO KEY UPDATE",
        f.reference.as_str()
    )
    .fetch_one(&mut *gate)
    .await
    .unwrap();
    let (operation_pool, operation_pid) = single_backend(&pool).await;
    let operation = tokio::spawn(async move { perform(&operation_pool, attempt).await });
    blocked_by(&pool, operation_pid, gate_pid).await;
    let (writer_pool, writer_pid) = single_backend(&pool).await;
    let writer = tokio::spawn(async move {
        let mut tx = writer_pool.begin().await.unwrap();
        match refer(&mut tx, other, run, kind).await {
            Ok(()) => tx.commit().await,
            Err(error) => {
                tx.rollback().await.unwrap();
                Err(error)
            }
        }
    });
    blocked_by(&pool, writer_pid, operation_pid).await;
    gate.commit().await.unwrap();
    tokio::time::timeout(Duration::from_secs(5), operation)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let result = tokio::time::timeout(Duration::from_secs(5), writer)
        .await
        .unwrap()
        .unwrap();
    match action {
        Action::Retire => {
            result.unwrap();
            // A later new dependency is legal only on the surviving run; it blocks future purge.
            let report = removal::preview(&pool, f.owner, f.bear).await.unwrap();
            assert!(report
                .blockers
                .contains(&removal::BearDeletionBlocker::RequiredReferences));
        }
        Action::DeleteBear => {
            let sqlx::Error::Database(cause) = result.unwrap_err() else {
                panic!("expected a target-FK refusal");
            };
            assert_eq!(cause.code().as_deref(), Some("23503"));
            assert_eq!(
                before,
                foreign_rows(&pool, other).await,
                "failed new reference must preserve its foreign row"
            );
        }
    }
}

#[sqlx::test(migrations = "../../migrations")]
async fn retirement_blocks_foreign_current_run_pointer(pool: PgPool) {
    existing_reverse_reference(pool, ReverseKind::CurrentRun, Action::Retire).await;
}
#[sqlx::test(migrations = "../../migrations")]
async fn retirement_blocks_foreign_work_job_run_pointer(pool: PgPool) {
    existing_reverse_reference(pool, ReverseKind::WorkRun, Action::Retire).await;
}
#[sqlx::test(migrations = "../../migrations")]
async fn bear_deletion_preserves_foreign_current_run_pointer(pool: PgPool) {
    existing_reverse_reference(pool, ReverseKind::CurrentRun, Action::DeleteBear).await;
}
#[sqlx::test(migrations = "../../migrations")]
async fn bear_deletion_preserves_foreign_work_job_run_pointer(pool: PgPool) {
    existing_reverse_reference(pool, ReverseKind::WorkRun, Action::DeleteBear).await;
}
#[sqlx::test(migrations = "../../migrations")]
async fn retirement_fences_new_foreign_current_run_pointer(pool: PgPool) {
    new_reference_after_target_fence(pool, ReverseKind::CurrentRun, Action::Retire).await;
}
#[sqlx::test(migrations = "../../migrations")]
async fn retirement_fences_new_foreign_work_job_run_pointer(pool: PgPool) {
    new_reference_after_target_fence(pool, ReverseKind::WorkRun, Action::Retire).await;
}
#[sqlx::test(migrations = "../../migrations")]
async fn bear_deletion_fences_new_foreign_current_run_pointer(pool: PgPool) {
    new_reference_after_target_fence(pool, ReverseKind::CurrentRun, Action::DeleteBear).await;
}
#[sqlx::test(migrations = "../../migrations")]
async fn bear_deletion_fences_new_foreign_work_job_run_pointer(pool: PgPool) {
    new_reference_after_target_fence(pool, ReverseKind::WorkRun, Action::DeleteBear).await;
}
