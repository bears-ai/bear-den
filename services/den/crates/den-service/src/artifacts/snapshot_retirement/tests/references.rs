use super::*;

#[sqlx::test(migrations = "../../migrations")]
async fn foreign_job_associations_and_completion_receipts_remain_required(pool: PgPool) {
    let f = fixture(&pool).await;
    let foreign=sqlx::query_scalar!("INSERT INTO bears(slug,name,description) VALUES('foreign-snapshot-bear','Foreign Bear','') RETURNING id").fetch_one(&pool).await.unwrap();
    let foreign_job=sqlx::query_scalar!("INSERT INTO bear_jobs(bear_id,created_by_user_id,created_by_role,goal,lifecycle_intent) VALUES($1,$2,'ui','Foreign owner','cancelled') RETURNING id",foreign,f.peer.get()).fetch_one(&pool).await.unwrap();
    let link=sqlx::query_scalar!("INSERT INTO artifact_links(artifact_id,target_kind,target_id,role,created_by_user_id) SELECT id,'docket_job',$2,'source',$3 FROM artifacts WHERE artifact_ref=$1 RETURNING id",f.reference.as_str(),foreign_job.to_string(),f.owner.get()).fetch_one(&pool).await.unwrap();
    assert_eq!(
        preview(&pool, f.owner, &f.reference).await.unwrap().blocker,
        Some(RetirementBlocker::RequiredReferences)
    );
    sqlx::query!("DELETE FROM artifact_links WHERE id=$1", link)
        .execute(&pool)
        .await
        .unwrap();
    let task = sqlx::query_scalar!("SELECT id FROM bear_tasks WHERE job_id=$1 LIMIT 1", f.job)
        .fetch_one(&pool)
        .await
        .unwrap();
    let run = sqlx::query_scalar!("SELECT current_run_id FROM bear_jobs WHERE id=$1", f.job)
        .fetch_one(&pool)
        .await
        .unwrap()
        .unwrap();
    sqlx::query!("INSERT INTO docket_task_completion_receipts(task_id,run_id,primary_output_ref,immutable_identity,validation) VALUES($1,$2,$3,'recorded-identity','{}'::jsonb)",task,run,f.reference.as_str()).execute(&pool).await.unwrap();
    let view = preview(&pool, f.owner, &f.reference).await.unwrap();
    assert_eq!(view.blocker, Some(RetirementBlocker::RequiredReferences));
    assert!(retire(&pool, request(&f, view.fingerprint)).await.is_err());
    assert!(artifacts::json_content_for_reader(
        &pool,
        &f.reference,
        ArtifactReader::Human(f.owner)
    )
    .await
    .is_ok());
}

#[sqlx::test(migrations = "../../migrations")]
async fn checkpoint_artifact_fk_cannot_be_retired_or_cascade_removed(pool: PgPool) {
    let f = fixture(&pool).await;
    let task = sqlx::query_scalar!("SELECT id FROM bear_tasks WHERE job_id=$1 LIMIT 1", f.job)
        .fetch_one(&pool)
        .await
        .unwrap();
    let attempt=sqlx::query_scalar!("INSERT INTO docket_execution_attempts(bear_id,task_id,binding_kind,binding_id,host_kind,host_run_id,fence_epoch,authorization_key,state,settled_at) VALUES($1,$2,'client_session','checkpoint-session','pair','checkpoint-run',1,$3,'settled',NOW()) RETURNING id",f.bear.as_uuid(),task,Uuid::new_v4()).fetch_one(&pool).await.unwrap();
    sqlx::query!("INSERT INTO docket_checkpoint_directives(execution_attempt_id,fence_epoch,state,acknowledged_at,acknowledged_artifact_ref) VALUES($1,1,'acknowledged',NOW(),$2)",attempt,f.reference.as_str()).execute(&pool).await.unwrap();
    let view = preview(&pool, f.owner, &f.reference).await.unwrap();
    assert_eq!(view.blocker, Some(RetirementBlocker::RequiredReferences));
    assert!(retire(&pool, request(&f, view.fingerprint)).await.is_err());
    let deletion = crate::bears::db::deletion::preview(&pool, f.owner, f.bear)
        .await
        .unwrap();
    assert!(!deletion.can_delete);
    assert!(crate::bears::db::deletion::delete_confirmed(
        &pool,
        crate::bears::db::deletion::ConfirmBearDeletion {
            actor: f.owner,
            bear_id: f.bear,
            expected: deletion.fingerprint,
            confirm_slug: deletion.slug,
            acknowledged: true
        }
    )
    .await
    .is_err());
    assert!(db::get_bear(&pool, f.bear.as_uuid())
        .await
        .unwrap()
        .is_some());
}

#[sqlx::test(migrations = "../../migrations")]
async fn source_page_tombstone_does_not_prevent_creator_retirement_or_change_snapshot_bytes(
    pool: PgPool,
) {
    let f = fixture(&pool).await;
    let original =
        artifacts::json_content_for_reader(&pool, &f.reference, ArtifactReader::Human(f.owner))
            .await
            .unwrap();
    cabinet::archive_item(&pool, &ActorScope::user(f.owner), &f.page)
        .await
        .unwrap();
    cabinet::delete_item(&pool, &ActorScope::user(f.owner), &f.page)
        .await
        .unwrap();
    let view = preview(&pool, f.owner, &f.reference).await.unwrap();
    assert!(view.can_retire());
    retire(&pool, request(&f, view.fingerprint)).await.unwrap();
    let payload=sqlx::query_scalar!("SELECT p.payload FROM artifact_json_payloads p JOIN artifacts a ON a.id=p.artifact_id WHERE a.artifact_ref=$1",f.reference.as_str()).fetch_one(&pool).await.unwrap();
    assert_eq!(payload, original);
}

#[sqlx::test(migrations = "../../migrations")]
async fn settled_terminal_work_is_distinct_from_live_or_incomplete_terminal_work(pool: PgPool) {
    let f = fixture(&pool).await;
    let run = sqlx::query_scalar!("SELECT current_run_id FROM bear_jobs WHERE id=$1", f.job)
        .fetch_one(&pool)
        .await
        .unwrap()
        .unwrap();
    let work=sqlx::query_scalar!("INSERT INTO bear_work_runs(bear_id,job_id,job_run_id,state) VALUES($1,$2,$3,'running') RETURNING id",f.bear.as_uuid(),f.job,run).fetch_one(&pool).await.unwrap();
    assert_eq!(
        preview(&pool, f.owner, &f.reference).await.unwrap().blocker,
        Some(RetirementBlocker::JobNotSettled)
    );
    sqlx::query!(
        "UPDATE bear_work_runs SET state='blocked' WHERE id=$1",
        work
    )
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(
        preview(&pool, f.owner, &f.reference).await.unwrap().blocker,
        Some(RetirementBlocker::JobNotSettled)
    );
    sqlx::query!(
        "UPDATE bear_work_runs SET finished_at=NOW() WHERE id=$1",
        work
    )
    .execute(&pool)
    .await
    .unwrap();
    assert!(preview(&pool, f.owner, &f.reference)
        .await
        .unwrap()
        .can_retire());
}
