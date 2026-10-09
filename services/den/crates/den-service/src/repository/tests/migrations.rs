//! Run the unchanged checked-in migration pair only on SQLx-owned test databases.
use super::fixture::fixture;
use crate::{
    bears::{db, hats},
    connections,
};
use den_core::ids::{BearId, UserId};
use serde_json::Value;
use sqlx::{
    migrate::{MigrateError, Migrator},
    PgPool,
};
use std::borrow::Cow;
use uuid::Uuid;

const MEDIATION: i64 = 20261009063731;

fn migrator() -> Migrator {
    let mut selected = sqlx::migrate!("../../migrations");
    selected.migrations = Cow::Owned(
        selected
            .iter()
            .filter(|migration| migration.version == MEDIATION)
            .cloned()
            .collect(),
    );
    assert_eq!(
        selected.iter().count(),
        2,
        "exact reversible repository migration pair"
    );
    selected.set_ignore_missing(true);
    // Expected refusal must not retain a session advisory lock in the test pool.
    selected.set_locking(false);
    selected
}

async fn versions(pool: &PgPool) -> Vec<i64> {
    sqlx::query_scalar!("SELECT version FROM _sqlx_migrations WHERE success ORDER BY version")
        .fetch_all(pool)
        .await
        .unwrap()
}
async fn reference_columns(pool: &PgPool) -> i32 {
    sqlx::query_scalar!(r#"SELECT count(*)::integer AS "count!" FROM information_schema.columns
        WHERE table_schema=current_schema() AND table_name='provider_connections'
        AND column_name IN ('external_backend_binding_id','external_secret_id','external_secret_version')"#).fetch_one(pool).await.unwrap()
}
async fn guards(pool: &PgPool) -> Value {
    sqlx::query_scalar!(r#"SELECT jsonb_agg(jsonb_build_array(t.relname,c.conname,pg_get_constraintdef(c.oid))
        ORDER BY t.relname,c.conname) AS "guards!"
        FROM pg_constraint c JOIN pg_class t ON t.oid=c.conrelid JOIN pg_namespace n ON n.oid=t.relnamespace
        WHERE n.nspname=current_schema() AND (
            (t.relname='provider_connections' AND c.conname IN ('provider_connections_material','provider_connections_provider_check'))
            OR (t.relname='bear_hat_access_grants' AND c.conname IN ('bear_hat_access_grants_check','bear_hat_repository_target'))
        )"#).fetch_one(pool).await.unwrap()
}
async fn connection_audit(pool: &PgPool, id: Uuid) -> Value {
    // The same projection is valid before/after the additive reference columns.
    sqlx::query_scalar!(r#"SELECT to_jsonb(c)-ARRAY['external_backend_binding_id','external_secret_id','external_secret_version'] AS "audit!"
        FROM provider_connections c WHERE c.id=$1"#,id).fetch_one(pool).await.unwrap()
}
async fn grant_audit(pool: &PgPool, id: Uuid) -> Value {
    sqlx::query_scalar!(
        r#"SELECT to_jsonb(g) AS "audit!" FROM bear_hat_access_grants g WHERE g.id=$1"#,
        id
    )
    .fetch_one(pool)
    .await
    .unwrap()
}
fn refusal(error: MigrateError) {
    let MigrateError::ExecuteMigration(error, version) = error else {
        panic!("the real repository down script must refuse")
    };
    assert_eq!(version, MEDIATION);
    let sqlx::Error::Database(cause) = error else {
        panic!("expected the populated reference/grant guard")
    };
    assert_eq!(cause.code().as_deref(), Some("P0001"));
    assert!(cause
        .message()
        .contains("Retire external Connections and repository grant records"));
}

#[sqlx::test(migrations = "../../migrations")]
async fn clean_down_up_down_reapply_preserve_legacy_credentials_and_grants(pool: PgPool) {
    let actor=UserId::new(sqlx::query_scalar!("INSERT INTO users (username,email) VALUES ('repomigrationlegacy','repomigrationlegacy@test.invalid') RETURNING id").fetch_one(&pool).await.unwrap());
    let bear = BearId::new(
        db::create_bear(
            &pool,
            db::BearParams {
                slug: "repomigrationlegacy",
                name: "Migration legacy preservation",
                description: "",
                system_prompt: "",
                default_model: None,
                tools_enabled: None,
                context_profile: None,
            },
        )
        .await
        .unwrap(),
    );
    db::grant_membership(
        &pool,
        actor.get(),
        bear.as_uuid(),
        Some(db::BEAR_ROLE_ADMIN),
    )
    .await
    .unwrap();
    let hat = hats::create_hat(
        &pool,
        bear,
        actor,
        "Legacy permissions",
        "Preserve existing non-repository permissions",
    )
    .await
    .unwrap();
    let network = hats::access::grant(
        &pool,
        bear,
        hat.id,
        actor,
        &hats::access::HatAccessGrant::HttpsHost(
            hats::access::HttpsHost::parse("api.github.com").unwrap(),
        ),
        true,
    )
    .await
    .unwrap();
    let token = connections::create(
        &pool,
        actor,
        "Legacy token",
        connections::Material::HttpsToken("ghp_rollback_legacy_canary".into()),
        "repository-migration-test-encryption-key",
    )
    .await
    .unwrap();
    let app = connections::create(
        &pool,
        actor,
        "Legacy App",
        connections::Material::GithubApp {
            installation: 123,
            write: false,
        },
        "unused",
    )
    .await
    .unwrap();
    let token_before = connection_audit(&pool, token.0).await;
    let app_before = connection_audit(&pool, app.0).await;
    let grant_before = grant_audit(&pool, network).await;
    let before_versions = versions(&pool).await;
    let before_guards = guards(&pool).await;
    let selected = migrator();
    for _ in 0..2 {
        selected.undo(&pool, MEDIATION - 1).await.unwrap();
        assert_eq!(reference_columns(&pool).await, 0);
        let expected: Vec<_> = before_versions
            .iter()
            .copied()
            .filter(|version| *version != MEDIATION)
            .collect();
        assert_eq!(versions(&pool).await, expected);
        assert_eq!(connection_audit(&pool, token.0).await, token_before);
        assert_eq!(connection_audit(&pool, app.0).await, app_before);
        assert_eq!(grant_audit(&pool, network).await, grant_before);
        selected.run(&pool).await.unwrap();
        assert_eq!(reference_columns(&pool).await, 3);
        assert_eq!(versions(&pool).await, before_versions);
        assert_eq!(guards(&pool).await, before_guards);
        assert_eq!(connection_audit(&pool, token.0).await, token_before);
        assert_eq!(connection_audit(&pool, app.0).await, app_before);
        assert_eq!(grant_audit(&pool, network).await, grant_before);
    }
}

#[sqlx::test(migrations = "../../migrations")]
async fn populated_external_reference_and_grant_refuse_rollback_without_partial_changes(
    pool: PgPool,
) {
    let f = fixture(&pool).await;
    let selected = migrator();
    let applied = versions(&pool).await;
    let before_guards = guards(&pool).await;
    let reference = connections::external::for_owner(&pool, f.owner, f.surface)
        .await
        .unwrap()
        .credential
        .reference;
    let grant = grant_audit(&pool, f.grant).await;
    refusal(selected.undo(&pool, MEDIATION - 1).await.unwrap_err());
    assert_eq!(versions(&pool).await, applied);
    assert_eq!(guards(&pool).await, before_guards);
    assert_eq!(reference_columns(&pool).await, 3);
    assert_eq!(grant_audit(&pool, f.grant).await, grant);
    let current = connections::external::for_owner(&pool, f.owner, f.surface)
        .await
        .unwrap()
        .credential
        .reference;
    assert!(current == reference);
    selected.run(&pool).await.unwrap();
    assert_eq!(versions(&pool).await, applied);
}

#[sqlx::test(migrations = "../../migrations")]
async fn external_reference_alone_refuses_rollback(pool: PgPool) {
    let f = fixture(&pool).await;
    sqlx::query!("DELETE FROM bear_hat_access_grants WHERE id=$1", f.grant)
        .execute(&pool)
        .await
        .unwrap();
    let selected = migrator();
    let applied = versions(&pool).await;
    let before_guards = guards(&pool).await;
    refusal(selected.undo(&pool, MEDIATION - 1).await.unwrap_err());
    assert_eq!(versions(&pool).await, applied);
    assert_eq!(guards(&pool).await, before_guards);
    assert!(connections::external::for_owner(&pool, f.owner, f.surface)
        .await
        .is_ok());
}

#[sqlx::test(migrations = "../../migrations")]
async fn repository_grant_alone_including_revoked_grant_refuses_rollback(pool: PgPool) {
    let f = fixture(&pool).await;
    connections::detach(&pool, f.owner, f.surface.0)
        .await
        .unwrap();
    sqlx::query!(
        "DELETE FROM provider_connections WHERE id=$1",
        f.connection.0
    )
    .execute(&pool)
    .await
    .unwrap();
    let selected = migrator();
    let applied = versions(&pool).await;
    let before_guards = guards(&pool).await;
    for revoked in [false, true] {
        if revoked {
            hats::access::revoke(&pool, f.bear, f.hat, f.owner, f.grant)
                .await
                .unwrap();
        }
        let before = grant_audit(&pool, f.grant).await;
        refusal(selected.undo(&pool, MEDIATION - 1).await.unwrap_err());
        assert_eq!(versions(&pool).await, applied);
        assert_eq!(guards(&pool).await, before_guards);
        assert_eq!(grant_audit(&pool, f.grant).await, before);
        assert_eq!(reference_columns(&pool).await, 3);
    }
}
