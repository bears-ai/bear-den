//! Rehearse the checked-in scripts on SQLx-owned isolated databases only.
use super::*;
use sqlx::migrate::{MigrateError, Migrator};
use std::borrow::Cow;

const RETIREMENT: i64 = 20261008193820;
const HARDENING: i64 = 20261008210012;
const ROLLBACK_BARRIER: i64 = 20261009055112;

fn migrator() -> Migrator {
    let mut migrator = sqlx::migrate!("../../migrations");
    // Each test owns a database. Expected migration errors must not leave SQLx's
    // session advisory lock attached to a connection returned to this test's pool.
    migrator.set_locking(false);
    migrator
}
async fn versions(pool: &PgPool) -> Vec<i64> {
    sqlx::query_scalar!("SELECT version FROM _sqlx_migrations WHERE success ORDER BY version")
        .fetch_all(pool)
        .await
        .unwrap()
}
async fn original_audit(pool: &PgPool, f: &Fixture) -> serde_json::Value {
    // This projection works before the receipt columns exist and ignores only
    // those additive columns; payload, hashes, original link metadata and Job remain exact.
    sqlx::query_scalar!(r#"SELECT jsonb_build_object('artifact',to_jsonb(a),'payload',p.payload,
        'links',(SELECT jsonb_agg(to_jsonb(l)-ARRAY['retention_released_at','retention_released_by_user_id','retention_release_reason','retirement_fingerprint'] ORDER BY l.id)
                 FROM artifact_links l WHERE l.artifact_id=a.id),
        'job',(SELECT to_jsonb(j) FROM bear_jobs j WHERE j.id=$2)) AS "audit!"
        FROM artifacts a JOIN artifact_json_payloads p ON p.artifact_id=a.id WHERE a.artifact_ref=$1"#,
        f.reference.as_str(),f.job).fetch_one(pool).await.unwrap()
}
async fn complete_audit(pool: &PgPool, f: &Fixture) -> serde_json::Value {
    sqlx::query_scalar!(r#"SELECT jsonb_build_object('artifact',to_jsonb(a),'payload',p.payload,
        'links',(SELECT jsonb_agg(to_jsonb(l) ORDER BY l.id) FROM artifact_links l WHERE l.artifact_id=a.id),
        'job',(SELECT to_jsonb(j) FROM bear_jobs j WHERE j.id=$2)) AS "audit!"
        FROM artifacts a JOIN artifact_json_payloads p ON p.artifact_id=a.id WHERE a.artifact_ref=$1"#,
        f.reference.as_str(),f.job).fetch_one(pool).await.unwrap()
}
async fn guards(pool: &PgPool) -> serde_json::Value {
    sqlx::query_scalar!(r#"SELECT jsonb_build_object(
        'triggers',(SELECT jsonb_agg(jsonb_build_array(c.relname,t.tgname,t.tgtype,t.tgdeferrable,t.tginitdeferred,t.tgenabled::text) ORDER BY c.relname,t.tgname)
            FROM pg_trigger t JOIN pg_class c ON c.oid=t.tgrelid JOIN pg_namespace n ON n.oid=c.relnamespace
            WHERE n.nspname='public' AND t.tgname IN ('artifacts_snapshot_immutable','artifact_links_snapshot_citation',
                'artifacts_snapshot_retirement_commit','artifact_links_snapshot_retirement_commit','artifacts_cabinet_retention')),
        'functions',(SELECT jsonb_agg(jsonb_build_array(p.proname,p.prosrc) ORDER BY p.proname)
            FROM pg_proc p JOIN pg_namespace n ON n.oid=p.pronamespace WHERE n.nspname='public'
            AND p.proname IN ('protect_snapshot_registry','protect_snapshot_citation','verify_snapshot_retirement_commit',
                'docket_job_has_foreign_requirements','docket_run_has_foreign_requirements'))
        ) AS "guards!""#).fetch_one(pool).await.unwrap()
}
fn receipt_refusal(error: MigrateError, expected_version: i64) {
    let MigrateError::ExecuteMigration(error, version) = error else {
        panic!("expected a refusal from the actual down script");
    };
    assert_eq!(version, expected_version);
    let sqlx::Error::Database(cause) = error else {
        panic!("expected the populated-receipt database guard");
    };
    assert_eq!(cause.code().as_deref(), Some("P0001"));
    assert!(cause
        .message()
        .contains("snapshot retirement receipts exist"));
}

#[sqlx::test(migrations = "../../migrations")]
async fn clean_snapshot_migrations_down_up_down_reapply_restore_guards(pool: PgPool) {
    let migrator = migrator();
    let before = versions(&pool).await;
    let before_guards = guards(&pool).await;
    migrator.undo(&pool, RETIREMENT - 1).await.unwrap();
    assert!(versions(&pool)
        .await
        .iter()
        .all(|version| *version < RETIREMENT));
    migrator.run(&pool).await.unwrap();
    assert_eq!(before, versions(&pool).await);
    assert_eq!(before_guards, guards(&pool).await);
    migrator.undo(&pool, RETIREMENT - 1).await.unwrap();
    migrator.run(&pool).await.unwrap();
    assert_eq!(before, versions(&pool).await);
    assert_eq!(before_guards, guards(&pool).await);
}

#[sqlx::test(migrations = "../../migrations")]
async fn old_active_capture_survives_snapshot_migration_up_down_reapply_unchanged(pool: PgPool) {
    let migrator = migrator();
    migrator.undo(&pool, RETIREMENT - 1).await.unwrap();
    let f = fixture(&pool).await;
    let original = original_audit(&pool, &f).await;
    migrator.run(&pool).await.unwrap();
    assert_eq!(original, original_audit(&pool, &f).await);
    let defaults = sqlx::query_scalar!(
        r#"SELECT bool_and(l.retention_released_at IS NULL
        AND l.retention_released_by_user_id IS NULL AND l.retention_release_reason IS NULL
        AND l.retirement_fingerprint IS NULL) AS "unreleased!"
        FROM artifact_links l JOIN artifacts a ON a.id=l.artifact_id WHERE a.artifact_ref=$1"#,
        f.reference.as_str()
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(defaults);
    assert!(preview(&pool, f.owner, &f.reference)
        .await
        .unwrap()
        .can_retire());
    migrator.undo(&pool, RETIREMENT - 1).await.unwrap();
    assert_eq!(original, original_audit(&pool, &f).await);
    assert!(
        sqlx::query!(
            "DELETE FROM artifacts WHERE artifact_ref=$1",
            f.reference.as_str()
        )
        .execute(&pool)
        .await
        .is_err(),
        "the pre-retirement Cabinet retainer still protects the active capture"
    );
    migrator.run(&pool).await.unwrap();
    assert_eq!(original, original_audit(&pool, &f).await);
    let view = preview(&pool, f.owner, &f.reference).await.unwrap();
    retire(&pool, request(&f, view.fingerprint)).await.unwrap();
    assert!(artifacts::json_content_for_reader(
        &pool,
        &f.reference,
        ArtifactReader::Human(f.owner)
    )
    .await
    .is_err());
}

#[sqlx::test(migrations = "../../migrations")]
async fn populated_receipt_refuses_chain_down_before_snapshot_hardening_is_removed(pool: PgPool) {
    let f = fixture(&pool).await;
    let view = preview(&pool, f.owner, &f.reference).await.unwrap();
    retire(&pool, request(&f, view.fingerprint)).await.unwrap();
    let audit = complete_audit(&pool, &f).await;
    let applied = versions(&pool).await;
    let before_guards = guards(&pool).await;
    let migrator = migrator();
    for target in [HARDENING - 1, RETIREMENT - 1] {
        receipt_refusal(
            migrator.undo(&pool, target).await.unwrap_err(),
            ROLLBACK_BARRIER,
        );
        assert_eq!(audit, complete_audit(&pool, &f).await);
        assert_eq!(
            applied,
            versions(&pool).await,
            "no earlier down may commit before refusal"
        );
        assert_eq!(
            before_guards,
            guards(&pool).await,
            "hardening remains installed after chain refusal"
        );
    }
    migrator.run(&pool).await.unwrap();
    assert_eq!(audit, complete_audit(&pool, &f).await);
}

#[sqlx::test(migrations = "../../migrations")]
async fn base_down_itself_refuses_populated_receipt_while_hardening_is_installed(pool: PgPool) {
    let f = fixture(&pool).await;
    let view = preview(&pool, f.owner, &f.reference).await.unwrap();
    retire(&pool, request(&f, view.fingerprint)).await.unwrap();
    let audit = complete_audit(&pool, &f).await;
    let applied = versions(&pool).await;
    let before_guards = guards(&pool).await;
    // Execute the unchanged base down via SQLx, without first unwinding later
    // migrations, to independently exercise its own receipt-preservation guard.
    let mut selected = migrator();
    selected.migrations = Cow::Owned(
        selected
            .iter()
            .filter(|migration| migration.version == RETIREMENT)
            .cloned()
            .collect(),
    );
    selected.set_ignore_missing(true);
    receipt_refusal(
        selected.undo(&pool, RETIREMENT - 1).await.unwrap_err(),
        RETIREMENT,
    );
    assert_eq!(audit, complete_audit(&pool, &f).await);
    assert_eq!(applied, versions(&pool).await);
    assert_eq!(before_guards, guards(&pool).await);
}
