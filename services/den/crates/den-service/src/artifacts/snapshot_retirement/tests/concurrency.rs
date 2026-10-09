use super::*;
use std::time::Duration;

#[sqlx::test(migrations = "../../migrations")]
async fn reference_writer_that_started_first_is_observed_after_lock_wait(pool: PgPool) {
    let f = fixture(&pool).await;
    let view = preview(&pool, f.owner, &f.reference).await.unwrap();
    let mut writer = pool.begin().await.unwrap();
    // A new external association owns its artifact SHARE lock until commit.
    sqlx::query!("INSERT INTO artifact_links(artifact_id,target_kind,target_id,role) SELECT id,'unknown_owner','new-requirement','evidence' FROM artifacts WHERE artifact_ref=$1",f.reference.as_str()).execute(&mut *writer).await.unwrap();
    let request = request(&f, view.fingerprint);
    let copy_pool = pool.clone();
    let waiter = tokio::spawn(async move { retire(&copy_pool, request).await });
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(!waiter.is_finished());
    writer.commit().await.unwrap();
    let result = tokio::time::timeout(Duration::from_secs(5), waiter)
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(result, Err(DenError::ValidationError(_))));
    assert!(artifacts::json_content_for_reader(
        &pool,
        &f.reference,
        ArtifactReader::Human(f.owner)
    )
    .await
    .is_ok());
}

#[sqlx::test(migrations = "../../migrations")]
async fn concurrent_retire_replays_one_receipt_without_mutating_original_audit(pool: PgPool) {
    let f = fixture(&pool).await;
    let view = preview(&pool, f.owner, &f.reference).await.unwrap();
    let first = request(&f, view.fingerprint.clone());
    let second = request(&f, view.fingerprint);
    let first_pool = pool.clone();
    let second_pool = pool.clone();
    let a = tokio::spawn(async move { retire(&first_pool, first).await });
    let b = tokio::spawn(async move { retire(&second_pool, second).await });
    assert_eq!(
        a.await.unwrap().unwrap().retired_at,
        b.await.unwrap().unwrap().retired_at
    );
    let links=sqlx::query_scalar!("SELECT count(*) FROM artifact_links l JOIN artifacts a ON a.id=l.artifact_id WHERE a.artifact_ref=$1 AND l.retention_released_at IS NOT NULL",f.reference.as_str()).fetch_one(&pool).await.unwrap();
    assert_eq!(links, Some(1));
}

#[sqlx::test(migrations = "../../migrations")]
async fn membership_revoked_while_retirement_waits_for_user_lock_is_rechecked(pool: PgPool) {
    let f = fixture(&pool).await;
    let view = preview(&pool, f.owner, &f.reference).await.unwrap();
    let mut revoker = pool.begin().await.unwrap();
    sqlx::query!("SELECT id FROM users WHERE id=$1 FOR UPDATE", f.owner.get())
        .fetch_one(&mut *revoker)
        .await
        .unwrap();
    let request = request(&f, view.fingerprint);
    let copy_pool = pool.clone();
    let waiter = tokio::spawn(async move { retire(&copy_pool, request).await });
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(!waiter.is_finished());
    sqlx::query!(
        "SELECT id FROM bears WHERE id=$1 FOR UPDATE",
        f.bear.as_uuid()
    )
    .fetch_one(&mut *revoker)
    .await
    .unwrap();
    sqlx::query!(
        "DELETE FROM user_bear WHERE user_id=$1 AND bear_id=$2",
        f.owner.get(),
        f.bear.as_uuid()
    )
    .execute(&mut *revoker)
    .await
    .unwrap();
    revoker.commit().await.unwrap();
    assert!(matches!(
        tokio::time::timeout(Duration::from_secs(5), waiter)
            .await
            .unwrap()
            .unwrap(),
        Err(DenError::NotFound(_))
    ));
    assert!(sqlx::query_scalar!(r#"SELECT artifact_has_cabinet_retention(id) AS "retained!" FROM artifacts WHERE artifact_ref=$1"#,f.reference.as_str()).fetch_one(&pool).await.unwrap());
}
