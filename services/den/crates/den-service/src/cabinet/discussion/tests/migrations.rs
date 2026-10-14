use super::*;
use sqlx::migrate::{MigrateError, Migrator};
use std::borrow::Cow;

const DISCUSSION_MIGRATION: i64 = 20261013130143;
const RETENTION_MIGRATION: i64 = 20261013194208;

fn migrator() -> Migrator {
    let mut migrator = sqlx::migrate!("../../migrations");
    // Rehearse both discussion scripts, preserving the downgrade write barrier.
    migrator.migrations = Cow::Owned(
        migrator
            .iter()
            .filter(|m| matches!(m.version, DISCUSSION_MIGRATION | RETENTION_MIGRATION))
            .cloned()
            .collect(),
    );
    migrator.set_ignore_missing(true);
    migrator.set_locking(false);
    migrator
}

#[sqlx::test(migrations = "../../migrations")]
async fn empty_discussion_migration_down_up_restores_append_only_guard(pool: PgPool) {
    let migrator = migrator();
    migrator
        .undo(&pool, DISCUSSION_MIGRATION - 1)
        .await
        .unwrap();
    let relation = sqlx::query_scalar!(
        r#"SELECT to_regclass('public.cabinet_discussion_entries')::text AS "relation?""#
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(relation.is_none());
    migrator.run(&pool).await.unwrap();
    let (owner, reference) = fixture(&pool).await;
    let entry = post(&pool, &owner, &reference, "After reapply")
        .await
        .unwrap();
    assert!(sqlx::query!(
        "DELETE FROM cabinet_discussion_entries WHERE discussion_ref=$1",
        entry.discussion_ref.as_str()
    )
    .execute(&pool)
    .await
    .is_err());
    assert_eq!(list(&pool, &owner, &reference).await.unwrap(), vec![entry]);
}

#[sqlx::test(migrations = "../../migrations")]
async fn populated_discussion_refuses_destructive_rollback(pool: PgPool) {
    let (owner, reference) = fixture(&pool).await;
    let entry = post(&pool, &owner, &reference, "Preserve audit across rollback")
        .await
        .unwrap();
    let error = migrator()
        .undo(&pool, DISCUSSION_MIGRATION - 1)
        .await
        .unwrap_err();
    let MigrateError::ExecuteMigration(sqlx::Error::Database(cause), version) = error else {
        panic!("expected the populated discussion rollback barrier");
    };
    assert_eq!(version, RETENTION_MIGRATION);
    assert_eq!(cause.code().as_deref(), Some("P0001"));
    assert!(cause.message().contains("Cabinet discussion entries exist"));
    assert_eq!(list(&pool, &owner, &reference).await.unwrap(), vec![entry]);
}

#[sqlx::test(migrations = "../../migrations")]
async fn discussion_truncate_cannot_erase_audit(pool: PgPool) {
    let (owner, reference) = fixture(&pool).await;
    let entry = post(&pool, &owner, &reference, "Keep this discussion")
        .await
        .unwrap();
    let error = sqlx::query!("TRUNCATE cabinet_discussion_entries")
        .execute(&pool)
        .await
        .unwrap_err();
    let sqlx::Error::Database(error) = error else {
        panic!("expected the discussion truncate barrier");
    };
    assert_eq!(error.code().as_deref(), Some("23514"));
    assert_eq!(list(&pool, &owner, &reference).await.unwrap(), vec![entry]);
}

#[sqlx::test(migrations = "../../migrations")]
async fn partial_downgrade_pauses_posts_until_retention_is_reapplied(pool: PgPool) {
    let (owner, reference) = fixture(&pool).await;
    let migrator = migrator();
    migrator.undo(&pool, DISCUSSION_MIGRATION).await.unwrap();
    assert!(
        post(&pool, &owner, &reference, "Must not race the next rollback")
            .await
            .is_err()
    );
    assert!(list(&pool, &owner, &reference).await.unwrap().is_empty());
    migrator.run(&pool).await.unwrap();
    let entry = post(&pool, &owner, &reference, "Writes resume after reapply")
        .await
        .unwrap();
    assert_eq!(list(&pool, &owner, &reference).await.unwrap(), vec![entry]);
}
