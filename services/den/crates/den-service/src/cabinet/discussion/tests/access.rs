use super::*;

#[sqlx::test(migrations = "../../migrations")]
async fn reads_use_current_inherited_authority_and_refs_do_not_disclose(pool: PgPool) {
    let (owner, parent) = fixture(&pool).await;
    let outsider = user(&pool).await;
    let enabled = bear(&pool, true).await;
    let disabled = bear(&pool, false).await;
    let child = cabinet::create_child(&pool, request(&owner), &parent)
        .await
        .unwrap()
        .item
        .cabinet_ref;
    assert!(list(&pool, &owner, &child).await.unwrap().is_empty());
    let entry = post(&pool, &owner, &child, "Human discussion")
        .await
        .unwrap();
    assert_eq!(
        list(&pool, &outsider, &child).await.unwrap(),
        vec![entry.clone()]
    );
    assert_eq!(
        list(&pool, &enabled, &child).await.unwrap(),
        vec![entry.clone()]
    );
    assert_eq!(
        list(&pool, &disabled, &child).await.unwrap_err(),
        CabinetError::NotFound
    );
    assert_eq!(
        post(&pool, &enabled, &child, "Bear write")
            .await
            .unwrap_err(),
        CabinetError::NotAuthorized
    );
    assert_eq!(
        post(&pool, &disabled, &child, "Bear write")
            .await
            .unwrap_err(),
        CabinetError::NotFound
    );

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
    let missing = CabinetItemRef::mint();
    for target in [&child, &missing] {
        assert_eq!(
            list(&pool, &outsider, target).await.unwrap_err(),
            CabinetError::NotFound
        );
        assert_eq!(
            post(&pool, &outsider, target, "hidden").await.unwrap_err(),
            CabinetError::NotFound
        );
    }
    assert_eq!(
        list(&pool, &enabled, &child).await.unwrap_err(),
        CabinetError::NotFound
    );
    assert_eq!(list(&pool, &owner, &child).await.unwrap(), vec![entry]);
    let nonexistent = ActorScope::user(UserId(i32::MAX));
    assert_eq!(
        list(&pool, &nonexistent, &child).await.unwrap_err(),
        CabinetError::NotFound
    );
    assert_eq!(
        post(&pool, &nonexistent, &child, "unknown actor")
            .await
            .unwrap_err(),
        CabinetError::NotFound
    );
    assert_eq!(entry_count(&pool).await, 1);
}

#[sqlx::test(migrations = "../../migrations")]
async fn invalid_content_does_not_disclose_private_or_missing_pages(pool: PgPool) {
    let (owner, reference) = fixture(&pool).await;
    let outsider = user(&pool).await;
    let unknown = ActorScope::user(UserId(i32::MAX));
    let bear = bear(&pool, true).await;
    assert_eq!(
        post(&pool, &unknown, &reference, "").await.unwrap_err(),
        CabinetError::NotFound
    );
    assert_eq!(
        post(&pool, &bear, &reference, "").await.unwrap_err(),
        CabinetError::NotAuthorized
    );
    pages::configure(
        &pool,
        &owner,
        &reference,
        CabinetPolicy::default(),
        &[user_id(&owner)],
        &[],
        &[],
    )
    .await
    .unwrap();
    let missing = CabinetItemRef::mint();
    for target in [&reference, &missing] {
        for content in [
            String::new(),
            "x".repeat(MAX_DISCUSSION_CHARS + 1),
            "NUL\0value".into(),
        ] {
            assert_eq!(
                post(&pool, &outsider, target, &content).await.unwrap_err(),
                CabinetError::NotFound
            );
        }
    }
    assert_eq!(entry_count(&pool).await, 0);
}

#[sqlx::test(migrations = "../../migrations")]
async fn moving_a_page_changes_discussion_audience_not_authorship(pool: PgPool) {
    let (owner, reference) = fixture(&pool).await;
    let writer = user(&pool).await;
    let entry = post(&pool, &writer, &reference, "Public before move")
        .await
        .unwrap();
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
    pages::organize(&pool, &owner, &reference, Some(&parent), 0, true)
        .await
        .unwrap();
    assert_eq!(
        list(&pool, &writer, &reference).await.unwrap_err(),
        CabinetError::NotFound
    );
    assert_eq!(
        post(&pool, &writer, &reference, "After move")
            .await
            .unwrap_err(),
        CabinetError::NotFound
    );
    assert_eq!(
        list(&pool, &owner, &reference).await.unwrap(),
        vec![entry.clone()]
    );
    pages::organize(&pool, &owner, &reference, None, 0, true)
        .await
        .unwrap();
    assert_eq!(list(&pool, &writer, &reference).await.unwrap(), vec![entry]);
    assert_eq!(entry_count(&pool).await, 1);
}

#[sqlx::test(migrations = "../../migrations")]
async fn bear_read_capability_is_checked_each_time(pool: PgPool) {
    let (owner, reference) = fixture(&pool).await;
    let reader = bear(&pool, true).await;
    post(&pool, &owner, &reference, "Discussion").await.unwrap();
    assert_eq!(list(&pool, &reader, &reference).await.unwrap().len(), 1);
    let Actor::Bear { bear_id, .. } = reader.actor else {
        unreachable!()
    };
    sqlx::query!(
        "UPDATE bears SET cabinet_enabled=false WHERE id=$1",
        bear_id.as_uuid()
    )
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(
        list(&pool, &reader, &reference).await.unwrap_err(),
        CabinetError::NotFound
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn archived_pages_are_read_only_and_tombstones_preserve_audit(pool: PgPool) {
    let (owner, reference) = fixture(&pool).await;
    let first = post(&pool, &owner, &reference, "Before archive")
        .await
        .unwrap();
    cabinet::archive_item(&pool, &owner, &reference)
        .await
        .unwrap();
    assert_eq!(list(&pool, &owner, &reference).await.unwrap(), vec![first]);
    assert!(matches!(
        post(&pool, &owner, &reference, "Archived").await,
        Err(CabinetError::Policy(_))
    ));
    assert_eq!(entry_count(&pool).await, 1);
    cabinet::restore_item(&pool, &owner, &reference)
        .await
        .unwrap();
    post(&pool, &owner, &reference, "Restored").await.unwrap();
    cabinet::delete_item(&pool, &owner, &reference)
        .await
        .unwrap();
    assert_eq!(
        list(&pool, &owner, &reference).await.unwrap_err(),
        CabinetError::NotFound
    );
    assert_eq!(
        post(&pool, &owner, &reference, "Deleted")
            .await
            .unwrap_err(),
        CabinetError::NotFound
    );
    assert_eq!(entry_count(&pool).await, 2);
}
