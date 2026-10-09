use super::*;

async fn existing_link_update_race(pool: PgPool, action: Action) {
    let f = fixture(&pool).await;
    let attempt = prepare(&pool, &f, action).await;
    let link=sqlx::query_scalar!("SELECT l.id FROM artifact_links l JOIN artifacts a ON a.id=l.artifact_id WHERE a.artifact_ref=$1 AND l.target_kind='docket_job'",f.reference.as_str()).fetch_one(&pool).await.unwrap();
    let mut updater = pool.begin().await.unwrap();
    // UPDATE owns the Link tuple before its BEFORE trigger requests Artifact SHARE.
    sqlx::query!("SELECT id FROM artifact_links WHERE id=$1 FOR UPDATE", link)
        .fetch_one(&mut *updater)
        .await
        .unwrap();
    let error = tokio::time::timeout(Duration::from_secs(5), perform(&pool, attempt))
        .await
        .expect("Artifact→Link must not wait into the reverse trigger lock order")
        .unwrap_err();
    assert_busy(error);
    assert_audit_present(&pool, &f, action).await;
    let updated = tokio::time::timeout(
        Duration::from_secs(5),
        sqlx::query!(
            "UPDATE artifact_links SET metadata=metadata WHERE id=$1",
            link
        )
        .execute(&mut *updater),
    )
    .await
    .expect("the refused operation must release its Artifact lock");
    match action {
        Action::Retire => {
            assert_eq!(updated.unwrap().rows_affected(), 1);
            updater.commit().await.unwrap();
        }
        Action::DeleteBear => {
            // A retired link remains closed; it must fail its normal guard, not deadlock.
            let error = updated.unwrap_err();
            let sqlx::Error::Database(cause) = error else {
                panic!("expected closed-reference guard");
            };
            assert_eq!(cause.code().as_deref(), Some("23514"));
            assert_eq!(cause.constraint(), Some("artifact_reference_closed"));
            updater.rollback().await.unwrap();
        }
    }
    let original = sqlx::query_scalar!("SELECT metadata FROM artifact_links WHERE id=$1", link)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(original, serde_json::json!({}));
}
#[sqlx::test(migrations = "../../migrations")]
async fn retirement_refuses_existing_link_update_without_deadlock(pool: PgPool) {
    existing_link_update_race(pool, Action::Retire).await;
}
#[sqlx::test(migrations = "../../migrations")]
async fn bear_deletion_refuses_existing_link_update_without_deadlock(pool: PgPool) {
    existing_link_update_race(pool, Action::DeleteBear).await;
}
