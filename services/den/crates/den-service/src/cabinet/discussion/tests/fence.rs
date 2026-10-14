use super::*;
use std::time::Duration;

async fn wait_for_fenced_post(pool: &PgPool) {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let waiting = sqlx::query_scalar!(
                r#"SELECT EXISTS(SELECT 1 FROM pg_locks WHERE locktype='advisory'
                   AND database=(SELECT oid FROM pg_database WHERE datname=current_database())
                   AND classid=0 AND objid=791004812 AND NOT granted) AS "waiting!""#,
            )
            .fetch_one(pool)
            .await
            .unwrap();
            if waiting {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("post must wait behind the Cabinet write fence");
}

#[sqlx::test(migrations = "../../migrations")]
async fn queued_post_rechecks_ancestor_access_after_fenced_revocation(pool: PgPool) {
    let (owner, parent) = fixture(&pool).await;
    let writer = user(&pool).await;
    let child = cabinet::create_child(&pool, request(&owner), &parent)
        .await
        .unwrap()
        .item
        .cabinet_ref;
    pages::configure(
        &pool,
        &owner,
        &parent,
        CabinetPolicy::default(),
        &[user_id(&owner), user_id(&writer)],
        &[],
        &[],
    )
    .await
    .unwrap();
    assert!(
        pages::metadata(&pool, &writer, &child)
            .await
            .unwrap()
            .can_write
    );
    let mut tx = pool.begin().await.unwrap();
    write_fence(&mut tx).await.unwrap();
    let posting_pool = pool.clone();
    let posting_ref = child.clone();
    let task = tokio::spawn(async move {
        post(
            &posting_pool,
            &writer,
            &posting_ref,
            "Queued before revocation",
        )
        .await
    });
    wait_for_fenced_post(&pool).await;
    // The same fenced membership change as pages::configure, on the held transaction.
    sqlx::query!(
        "UPDATE cabinet_items SET user_members=$2 WHERE cabinet_ref=$1",
        parent.as_str(),
        &[user_id(&owner)]
    )
    .execute(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();
    let result = tokio::time::timeout(Duration::from_secs(5), task)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(result.unwrap_err(), CabinetError::NotFound);
    assert_eq!(entry_count(&pool).await, 0);
}

#[sqlx::test(migrations = "../../migrations")]
async fn queued_post_rechecks_new_ancestry_after_fenced_move(pool: PgPool) {
    let (owner, reference) = fixture(&pool).await;
    let writer = user(&pool).await;
    let parent = cabinet::create_item(&pool, request(&owner))
        .await
        .unwrap()
        .item
        .cabinet_ref;
    pages::configure(
        &pool,
        &owner,
        &parent,
        CabinetPolicy::default(),
        &[user_id(&owner)],
        &[],
        &[],
    )
    .await
    .unwrap();
    assert!(
        pages::metadata(&pool, &writer, &reference)
            .await
            .unwrap()
            .can_write
    );
    let mut tx = pool.begin().await.unwrap();
    write_fence(&mut tx).await.unwrap();
    let posting_pool = pool.clone();
    let posting_ref = reference.clone();
    let task = tokio::spawn(async move {
        post(&posting_pool, &writer, &posting_ref, "Queued before move").await
    });
    wait_for_fenced_post(&pool).await;
    sqlx::query!(
        "UPDATE cabinet_items SET parent_item_id=(SELECT id FROM cabinet_items WHERE cabinet_ref=$2) WHERE cabinet_ref=$1",
        reference.as_str(), parent.as_str(),
    ).execute(&mut *tx).await.unwrap();
    tx.commit().await.unwrap();
    let result = tokio::time::timeout(Duration::from_secs(5), task)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(result.unwrap_err(), CabinetError::NotFound);
    assert_eq!(entry_count(&pool).await, 0);
}

#[sqlx::test(migrations = "../../migrations")]
async fn queued_post_rechecks_lifecycle_after_fenced_delete(pool: PgPool) {
    let (owner, reference) = fixture(&pool).await;
    let original = post(&pool, &owner, &reference, "Retained before delete")
        .await
        .unwrap();
    let mut tx = pool.begin().await.unwrap();
    write_fence(&mut tx).await.unwrap();
    let posting_pool = pool.clone();
    let posting_ref = reference.clone();
    let posting_owner = owner.clone();
    let task = tokio::spawn(async move {
        post(
            &posting_pool,
            &posting_owner,
            &posting_ref,
            "Queued before delete",
        )
        .await
    });
    wait_for_fenced_post(&pool).await;
    sqlx::query!(
        "UPDATE cabinet_items SET lifecycle='deleted' WHERE cabinet_ref=$1",
        reference.as_str(),
    )
    .execute(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();
    let result = tokio::time::timeout(Duration::from_secs(5), task)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(result.unwrap_err(), CabinetError::NotFound);
    assert_eq!(
        list(&pool, &owner, &reference).await.unwrap_err(),
        CabinetError::NotFound
    );
    assert_eq!(entry_count(&pool).await, 1);
    let stored = sqlx::query_scalar!(
        "SELECT content FROM cabinet_discussion_entries WHERE discussion_ref=$1",
        original.discussion_ref.as_str(),
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(stored, original.content);
}

#[sqlx::test(migrations = "../../migrations")]
async fn queued_post_rechecks_lifecycle_after_fenced_archive(pool: PgPool) {
    let (owner, reference) = fixture(&pool).await;
    let mut tx = pool.begin().await.unwrap();
    write_fence(&mut tx).await.unwrap();
    let posting_pool = pool.clone();
    let posting_ref = reference.clone();
    let task = tokio::spawn(async move {
        post(&posting_pool, &owner, &posting_ref, "Queued before archive").await
    });
    wait_for_fenced_post(&pool).await;
    sqlx::query!(
        "UPDATE cabinet_items SET lifecycle='archived' WHERE cabinet_ref=$1",
        reference.as_str()
    )
    .execute(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();
    let result = tokio::time::timeout(Duration::from_secs(5), task)
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(result, Err(CabinetError::Policy(_))));
    assert_eq!(entry_count(&pool).await, 0);
}
