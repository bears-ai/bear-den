use super::*;

#[derive(Clone, Copy)]
enum ProofKind {
    Task,
    Criterion,
}
#[derive(Clone, Copy)]
struct ProofRows {
    run: Uuid,
    task: Uuid,
    criterion: Uuid,
}
async fn proof_rows(pool: &PgPool, f: &Fixture) -> ProofRows {
    let run = sqlx::query_scalar!("SELECT current_run_id FROM bear_jobs WHERE id=$1", f.job)
        .fetch_one(pool)
        .await
        .unwrap()
        .unwrap();
    let task = sqlx::query_scalar!(
        "SELECT id FROM bear_tasks WHERE job_id=$1 ORDER BY id LIMIT 1",
        f.job
    )
    .fetch_one(pool)
    .await
    .unwrap();
    let criterion=sqlx::query_scalar!("INSERT INTO bear_job_criteria(job_id,kind,description) VALUES($1,'narrative','Completed proof') RETURNING id",f.job).fetch_one(pool).await.unwrap();
    sqlx::query!(
        "INSERT INTO bear_job_criteria_state(run_id,criterion_id,status) VALUES($1,$2,'met')",
        run,
        criterion
    )
    .execute(pool)
    .await
    .unwrap();
    ProofRows {
        run,
        task,
        criterion,
    }
}
async fn invalidate(
    tx: &mut Transaction<'_, Postgres>,
    rows: ProofRows,
    kind: ProofKind,
) -> Result<u64, sqlx::Error> {
    let changed = match kind {
        ProofKind::Task => {
            sqlx::query!(
                "UPDATE bear_task_run_state SET status='pending' WHERE run_id=$1 AND task_id=$2",
                rows.run,
                rows.task
            )
            .execute(&mut **tx)
            .await?
        }
        ProofKind::Criterion => sqlx::query!(
            "UPDATE bear_job_criteria_state SET status='unmet' WHERE run_id=$1 AND criterion_id=$2",
            rows.run,
            rows.criterion
        )
        .execute(&mut **tx)
        .await?,
    };
    Ok(changed.rows_affected())
}
async fn invalid_writer_first(pool: PgPool, kind: ProofKind, action: Action) {
    let f = fixture(&pool).await;
    let rows = proof_rows(&pool, &f).await;
    let attempt = prepare(&pool, &f, action).await;
    let mut writer = pool.begin().await.unwrap();
    assert_eq!(invalidate(&mut writer, rows, kind).await.unwrap(), 1);
    // The writer owns only an existing proof row, not its unchanged parent FK.
    let error = tokio::time::timeout(Duration::from_secs(5), perform(&pool, attempt))
        .await
        .expect("proof contention must refuse without waiting")
        .unwrap_err();
    assert_busy(error);
    assert_audit_present(&pool, &f, action).await;
    writer.commit().await.unwrap();
    match action {
        Action::Retire => assert_eq!(
            preview(&pool, f.owner, &f.reference).await.unwrap().blocker,
            Some(RetirementBlocker::JobNotSettled)
        ),
        Action::DeleteBear => assert!(removal::preview(&pool, f.owner, f.bear)
            .await
            .unwrap()
            .blockers
            .contains(&removal::BearDeletionBlocker::LiveWork)),
    }
}

async fn source_fence_first(pool: PgPool, kind: ProofKind, action: Action) {
    let f = fixture(&pool).await;
    let rows = proof_rows(&pool, &f).await;
    let attempt = prepare(&pool, &f, action).await;
    let mut gate = pool.begin().await.unwrap();
    let gate_pid = backend(&mut gate).await;
    // Pause the real operation after its source fences but before it reads eligibility.
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
        let changed = invalidate(&mut tx, rows, kind).await.unwrap();
        tx.commit().await.unwrap();
        changed
    });
    // done→pending / met→unmet cannot pass the already-acquired proof fence.
    blocked_by(&pool, writer_pid, operation_pid).await;
    assert!(!writer.is_finished());
    gate.commit().await.unwrap();
    tokio::time::timeout(Duration::from_secs(5), operation)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let changed = tokio::time::timeout(Duration::from_secs(5), writer)
        .await
        .unwrap()
        .unwrap();
    match action {
        Action::Retire => {
            assert_eq!(changed, 1, "proof may change only after retirement commits");
            assert!(artifacts::json_content_for_reader(
                &pool,
                &f.reference,
                ArtifactReader::Human(f.owner)
            )
            .await
            .is_err());
            assert_audit_present(&pool, &f, Action::DeleteBear).await;
        }
        Action::DeleteBear => {
            assert_eq!(
                changed, 0,
                "the waiting updater cannot recreate a cascade-deleted proof row"
            );
            assert!(db::get_bear(&pool, f.bear.as_uuid())
                .await
                .unwrap()
                .is_none());
        }
    }
}

#[sqlx::test(migrations = "../../migrations")]
async fn retirement_refuses_uncommitted_done_to_pending(pool: PgPool) {
    invalid_writer_first(pool, ProofKind::Task, Action::Retire).await;
}
#[sqlx::test(migrations = "../../migrations")]
async fn retirement_refuses_uncommitted_met_to_unmet(pool: PgPool) {
    invalid_writer_first(pool, ProofKind::Criterion, Action::Retire).await;
}
#[sqlx::test(migrations = "../../migrations")]
async fn bear_deletion_refuses_uncommitted_done_to_pending(pool: PgPool) {
    invalid_writer_first(pool, ProofKind::Task, Action::DeleteBear).await;
}
#[sqlx::test(migrations = "../../migrations")]
async fn bear_deletion_refuses_uncommitted_met_to_unmet(pool: PgPool) {
    invalid_writer_first(pool, ProofKind::Criterion, Action::DeleteBear).await;
}
#[sqlx::test(migrations = "../../migrations")]
async fn retirement_fences_done_to_pending_until_commit(pool: PgPool) {
    source_fence_first(pool, ProofKind::Task, Action::Retire).await;
}
#[sqlx::test(migrations = "../../migrations")]
async fn retirement_fences_met_to_unmet_until_commit(pool: PgPool) {
    source_fence_first(pool, ProofKind::Criterion, Action::Retire).await;
}
#[sqlx::test(migrations = "../../migrations")]
async fn bear_deletion_fences_done_to_pending_until_commit(pool: PgPool) {
    source_fence_first(pool, ProofKind::Task, Action::DeleteBear).await;
}
#[sqlx::test(migrations = "../../migrations")]
async fn bear_deletion_fences_met_to_unmet_until_commit(pool: PgPool) {
    source_fence_first(pool, ProofKind::Criterion, Action::DeleteBear).await;
}
