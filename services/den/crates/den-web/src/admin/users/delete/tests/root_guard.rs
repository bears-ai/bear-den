use super::*;

#[sqlx::test(migrations = "../../migrations")]
async fn identity_delete_blocks_sole_admin_and_compatibility_caller_cannot_bypass(pool: PgPool) {
    let target = user(&pool, "sole-delete-admin", false).await;
    let atlas = bear(&pool, "sole-delete-atlas", "Private Atlas").await;
    grant(&pool, target, atlas, "admin").await;
    let preview = deletion::preview_user_deletion(&pool, target)
        .await
        .unwrap();
    assert!(preview.is_blocked());
    assert_eq!(preview.bears[0].bear_id, atlas);
    assert_eq!(preview.bears[0].name, "Private Atlas");
    match deletion::delete_user(&pool, target).await {
        Err(UserDeletionError::LastBearAdmin(preview)) => {
            assert_eq!(preview.user_id, target);
            assert!(preview.bears[0].last_admin);
        }
        result => panic!("expected typed last-admin error, got {result:?}"),
    }
    assert!(matches!(
        user_db::delete_user_by_id(&pool, target.get()).await,
        Err(DenError::ValidationError(_))
    ));
    assert!(exists(&pool, target).await);
    assert_eq!(role(&pool, target, atlas).await, Some(Some("admin".into())));
    assert_eq!(admins(&pool, atlas).await, 1);
}

#[sqlx::test(migrations = "../../migrations")]
async fn multi_bear_delete_is_atomic_until_every_admin_responsibility_has_been_handed_off(
    pool: PgPool,
) {
    let target = user(&pool, "multibear-delete-target", false).await;
    let next = user(&pool, "multibear-delete-next", false).await;
    let atlas = bear(&pool, "multibear-atlas", "Atlas").await;
    let birch = bear(&pool, "multibear-birch", "Birch").await;
    let cedar = bear(&pool, "multibear-cedar", "Cedar").await;
    for bear in [atlas, birch] {
        grant(&pool, target, bear, "admin").await;
    }
    grant(&pool, target, cedar, "member").await;
    grant(&pool, next, atlas, "admin").await;
    grant(&pool, next, birch, "member").await;
    let Err(UserDeletionError::LastBearAdmin(preview)) = deletion::delete_user(&pool, target).await
    else {
        panic!("Birch's sole admin must block the whole account deletion");
    };
    assert_eq!(preview.bears.len(), 3);
    assert_eq!(
        preview
            .bears
            .iter()
            .filter(|bear| bear.last_admin)
            .map(|bear| bear.bear_id)
            .collect::<Vec<_>>(),
        vec![birch]
    );
    assert!(preview
        .bears
        .windows(2)
        .all(|pair| pair[0].bear_id.as_uuid() < pair[1].bear_id.as_uuid()));
    assert!(exists(&pool, target).await);
    for bear in [atlas, birch, cedar] {
        assert!(role(&pool, target, bear).await.is_some());
    }
    // This is an explicit call to canonical membership, not an automatic replacement selection.
    grant(&pool, next, birch, "admin").await;
    user_db::delete_user_by_id(&pool, target.get())
        .await
        .unwrap();
    assert!(!exists(&pool, target).await);
    for bear in [atlas, birch, cedar] {
        assert_eq!(role(&pool, target, bear).await, None);
    }
    for bear in [atlas, birch] {
        assert_eq!(admins(&pool, bear).await, 1);
        assert_eq!(role(&pool, next, bear).await, Some(Some("admin".into())));
    }
}

#[sqlx::test(migrations = "../../migrations")]
async fn deletion_preserves_legacy_admin_case_trim_and_null_member_semantics(pool: PgPool) {
    let target = user(&pool, "legacy-delete-target", false).await;
    let next = user(&pool, "legacy-delete-next", false).await;
    let atlas = bear(&pool, "legacy-delete-atlas", "Atlas").await;
    let birch = bear(&pool, "legacy-delete-birch", "Birch").await;
    sqlx::query!(
        "INSERT INTO user_bear (user_id, bear_id, role) VALUES ($1, $2, $3)",
        target.get(),
        atlas.as_uuid(),
        " AdMiN "
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query!(
        "INSERT INTO user_bear (user_id, bear_id, role) VALUES ($1, $2, $3)",
        next.get(),
        atlas.as_uuid(),
        Option::<&str>::None
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query!(
        "INSERT INTO user_bear (user_id, bear_id, role) VALUES ($1, $2, $3)",
        target.get(),
        birch.as_uuid(),
        Option::<&str>::None
    )
    .execute(&pool)
    .await
    .unwrap();
    let preview = deletion::preview_user_deletion(&pool, target)
        .await
        .unwrap();
    assert!(
        preview
            .bears
            .iter()
            .find(|bear| bear.bear_id == atlas)
            .unwrap()
            .last_admin
    );
    let member = preview
        .bears
        .iter()
        .find(|bear| bear.bear_id == birch)
        .unwrap();
    assert!(!member.is_admin);
    assert!(!member.last_admin);
    assert!(matches!(
        deletion::delete_user(&pool, target).await,
        Err(UserDeletionError::LastBearAdmin(_))
    ));
    // The target role uses the same Rust trim boundary as membership changes, even
    // for whitespace not stripped by PostgreSQL's btrim when counting other admins.
    sqlx::query!(
        "UPDATE user_bear SET role = $3 WHERE user_id = $1 AND bear_id = $2",
        target.get(),
        atlas.as_uuid(),
        "\tADMIN\n"
    )
    .execute(&pool)
    .await
    .unwrap();
    let preview = deletion::preview_user_deletion(&pool, target)
        .await
        .unwrap();
    assert!(
        preview
            .bears
            .iter()
            .find(|bear| bear.bear_id == atlas)
            .unwrap()
            .last_admin
    );
    assert!(matches!(
        deletion::delete_user(&pool, target).await,
        Err(UserDeletionError::LastBearAdmin(_))
    ));
    sqlx::query!(
        "UPDATE user_bear SET role = $3 WHERE user_id = $1 AND bear_id = $2",
        next.get(),
        atlas.as_uuid(),
        " ADMIN "
    )
    .execute(&pool)
    .await
    .unwrap();
    deletion::delete_user(&pool, target).await.unwrap();
    assert_eq!(admins(&pool, atlas).await, 1);
}

#[sqlx::test(migrations = "../../migrations")]
async fn historical_job_fk_is_not_force_cascaded_even_after_valid_admin_handoff(pool: PgPool) {
    let target = user(&pool, "history-delete-target", false).await;
    let next = user(&pool, "history-delete-next", false).await;
    let atlas = bear(&pool, "history-delete-atlas", "Atlas").await;
    grant(&pool, target, atlas, "admin").await;
    grant(&pool, next, atlas, "admin").await;
    let job = sqlx::query_scalar!(
        "INSERT INTO bear_jobs (bear_id, created_by_user_id, created_by_role, goal, lifecycle_intent) VALUES ($1, $2, 'ui', 'Historical job', 'archived') RETURNING id",
        atlas.as_uuid(), target.get()
    ).fetch_one(&pool).await.unwrap();
    assert!(matches!(
        deletion::delete_user(&pool, target).await,
        Err(UserDeletionError::Referenced { .. })
    ));
    assert!(exists(&pool, target).await);
    assert_eq!(role(&pool, target, atlas).await, Some(Some("admin".into())));
    assert_eq!(
        sqlx::query_scalar!(
            "SELECT created_by_user_id FROM bear_jobs WHERE id = $1",
            job
        )
        .fetch_one(&pool)
        .await
        .unwrap(),
        target.get()
    );
    assert_eq!(admins(&pool, atlas).await, 2);
}

#[sqlx::test(migrations = "../../migrations")]
async fn absent_user_is_typed_not_found_and_unassigned_user_can_be_deleted(pool: PgPool) {
    let target = user(&pool, "unassigned-delete-target", false).await;
    deletion::delete_user(&pool, target).await.unwrap();
    assert!(matches!(
        deletion::delete_user(&pool, target).await,
        Err(UserDeletionError::NotFound)
    ));
    assert!(matches!(
        deletion::preview_user_deletion(&pool, target).await,
        Err(UserDeletionError::NotFound)
    ));
}
