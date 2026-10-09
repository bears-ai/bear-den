use super::*;
use den_cabinet::{ActorScope, CreateItemRequest, ItemKind};
use den_service::{artifacts::snapshot_retirement as retirement, cabinet};

async fn saved_copy(
    pool: &PgPool,
    owner: UserId,
    bear: BearId,
) -> den_service::artifacts::ArtifactRef {
    let page = cabinet::create_item(
        pool,
        CreateItemRequest {
            scope: ActorScope::user(owner),
            kind: ItemKind::Document,
            title: "HIDDEN ACCOUNT EVIDENCE".into(),
            content: "Private account evidence".into(),
            collection_ref: None,
            mission_ref: None,
            source_links: vec![],
        },
    )
    .await
    .unwrap();
    let mut tx = pool.begin().await.unwrap();
    let reference = cabinet::snapshots::capture_in_tx(
        &mut tx,
        pool,
        &ActorScope::user(owner),
        bear,
        &page.item.cabinet_ref,
        page.version.version_ref(),
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    reference
}

#[sqlx::test(migrations = "../../migrations")]
async fn active_private_copy_blocks_shared_identity_helper_without_disclosing_or_stranding_it(
    pool: PgPool,
) {
    let target = user(&pool, "copycreator", false).await;
    let next = user(&pool, "copyadmin", false).await;
    let atlas = bear(&pool, "copy-owner-bear", "Atlas").await;
    grant(&pool, target, atlas, "member").await;
    grant(&pool, next, atlas, "admin").await;
    let reference = saved_copy(&pool, target, atlas).await;
    let preview = deletion::preview_user_deletion(&pool, target)
        .await
        .unwrap();
    assert!(preview.has_active_private_copies);
    assert!(preview.is_blocked());
    let encoded = serde_json::to_string(&preview).unwrap();
    assert!(!encoded.contains("HIDDEN ACCOUNT EVIDENCE"));
    assert!(!encoded.contains(reference.as_str()));
    assert!(matches!(
        deletion::delete_user(&pool, target).await,
        Err(UserDeletionError::ActivePrivateCopies(_))
    ));
    assert!(exists(&pool, target).await);
    assert!(sqlx::query_scalar!(
        "SELECT created_by_user_id FROM artifacts WHERE artifact_ref=$1",
        reference.as_str()
    )
    .fetch_one(&pool)
    .await
    .unwrap()
    .is_some());
    // Even an owner who lost membership cannot be deleted into an unretirable orphan.
    bears_db::revoke_membership(&pool, target.get(), atlas.as_uuid())
        .await
        .unwrap();
    assert!(matches!(
        deletion::delete_user(&pool, target).await,
        Err(UserDeletionError::ActivePrivateCopies(_))
    ));
}

#[sqlx::test(migrations = "../../migrations")]
async fn retired_copy_allows_identity_delete_and_retains_immutable_actor_receipt(pool: PgPool) {
    let target = user(&pool, "retiredcreator", false).await;
    let next = user(&pool, "retiredadmin", false).await;
    let atlas = bear(&pool, "retired-owner-bear", "Atlas").await;
    grant(&pool, target, atlas, "member").await;
    grant(&pool, next, atlas, "admin").await;
    let reference = saved_copy(&pool, target, atlas).await;
    let view = retirement::preview(&pool, target, &reference)
        .await
        .unwrap();
    retirement::retire(
        &pool,
        retirement::RetireCabinetSnapshot {
            actor: target,
            reference: reference.clone(),
            expected: view.fingerprint,
            reason: "Unused private copy".into(),
            acknowledged: true,
        },
    )
    .await
    .unwrap();
    assert!(
        !deletion::preview_user_deletion(&pool, target)
            .await
            .unwrap()
            .has_active_private_copies
    );
    deletion::delete_user(&pool, target).await.unwrap();
    assert!(!exists(&pool, target).await);
    let receipt=sqlx::query!("SELECT a.created_by_user_id,l.retention_released_by_user_id,l.retention_release_reason FROM artifacts a JOIN artifact_links l ON l.artifact_id=a.id WHERE a.artifact_ref=$1 AND l.target_kind='cabinet_snapshot'",reference.as_str()).fetch_one(&pool).await.unwrap();
    assert_eq!(receipt.created_by_user_id, None);
    assert_eq!(receipt.retention_released_by_user_id, Some(target.get()));
    assert_eq!(
        receipt.retention_release_reason.as_deref(),
        Some("Unused private copy")
    );
    let preview = den_service::bears::db::deletion::preview(&pool, next, atlas)
        .await
        .unwrap();
    assert!(preview.can_delete);
}

#[sqlx::test(migrations = "../../migrations")]
async fn capture_user_key_share_fence_prevents_identity_delete_from_missing_new_copy(pool: PgPool) {
    let target = user(&pool, "racingcreator", false).await;
    let next = user(&pool, "racingadmin", false).await;
    let atlas = bear(&pool, "racing-owner-bear", "Atlas").await;
    grant(&pool, target, atlas, "member").await;
    grant(&pool, next, atlas, "admin").await;
    let page = cabinet::create_item(
        &pool,
        CreateItemRequest {
            scope: ActorScope::user(target),
            kind: ItemKind::Document,
            title: "Racing private evidence".into(),
            content: "Captured first".into(),
            collection_ref: None,
            mission_ref: None,
            source_links: vec![],
        },
    )
    .await
    .unwrap();
    let mut writer = pool.begin().await.unwrap();
    cabinet::snapshots::capture_in_tx(
        &mut writer,
        &pool,
        &ActorScope::user(target),
        atlas,
        &page.item.cabinet_ref,
        page.version.version_ref(),
    )
    .await
    .unwrap();
    let copy_pool = pool.clone();
    let waiter = tokio::spawn(async move { deletion::delete_user(&copy_pool, target).await });
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    assert!(!waiter.is_finished());
    writer.commit().await.unwrap();
    let result = tokio::time::timeout(std::time::Duration::from_secs(5), waiter)
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(
        result,
        Err(UserDeletionError::ActivePrivateCopies(_))
    ));
    assert!(exists(&pool, target).await);
}
