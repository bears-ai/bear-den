//! Test-only helpers: build a schema-applied per-Bear store backed by a throwaway SQLite file.

use den_core::DenError;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use uuid::Uuid;

use super::migrate::migrate_bear_sqlite_schema;
use super::records::BearMemoryStore;

const SCHEMA_SQL: &str = include_str!("schema.sql");

/// A schema-applied, migrated store over a unique temp SQLite file.
pub async fn new_test_store() -> BearMemoryStore {
    let db_path = std::env::temp_dir().join(format!("den-memory-test-{}.sqlite", Uuid::new_v4()));
    let options = SqliteConnectOptions::new()
        .filename(&db_path)
        .create_if_missing(true)
        .pragma("journal_mode", "WAL")
        .pragma("busy_timeout", "5000");
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(options)
        .await
        .expect("connect test sqlite");
    for statement in SCHEMA_SQL
        .split(';')
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        sqlx::query(statement)
            .execute(&pool)
            .await
            .expect("apply test schema");
    }
    migrate_bear_sqlite_schema(&pool)
        .await
        .expect("migrate test schema");
    BearMemoryStore::new(Uuid::new_v4(), pool)
}

/// Whole-store record and active-rule counts, plus epochs belonging to another Bear.
/// Keep SQLite fixture queries here so web tests do not describe them against Postgres.
pub async fn identity_test_counts(store: &BearMemoryStore) -> Result<(i64, i64, i64), DenError> {
    let bear_id = store.bear_id().to_string();
    let counts = sqlx::query!(
        r#"
        SELECT
            (SELECT COUNT(*) FROM memory_records) AS "records!: i64",
            (SELECT COUNT(*) FROM memory_access_rules WHERE state = 'active') AS "active_rules!: i64",
            (SELECT COUNT(*) FROM user_identity_generations WHERE bear_id <> ?) AS "foreign_generations!: i64"
        "#,
        bear_id,
    )
    .fetch_one(store.pool())
    .await?;
    Ok((
        counts.records,
        counts.active_rules,
        counts.foreign_generations,
    ))
}

/// Hold SQLite's writer lock until the returned fixture transaction is completed.
pub async fn lock_identity_test_store(
    store: &BearMemoryStore,
) -> Result<sqlx::Transaction<'static, sqlx::Sqlite>, DenError> {
    let mut tx = store.pool().begin().await?;
    sqlx::query!("UPDATE bear_sequence SET next_sequence = next_sequence WHERE id = 1")
        .execute(&mut *tx)
        .await?;
    Ok(tx)
}

/// Seed bounded, source-host receipt history without granting any identity authority.
/// Prefer an existing person id so fixtures can exercise misleading historical claims.
pub async fn inject_history_fixture(
    store: &BearMemoryStore,
    malformed: bool,
    count: u32,
) -> Result<(), DenError> {
    if count > 100 {
        return Err(DenError::ValidationError(
            "identity history fixtures are limited to 100 entries".into(),
        ));
    }
    let bear_id = store.bear_id().to_string();
    let fallback_entity_id = Uuid::new_v4().to_string();
    let mut tx = store.pool().begin().await?;
    for index in 0..count {
        let receipt_id = match (malformed, index) {
            (true, 0) => "not-a-uuid".to_owned(),
            (true, _) => format!("not-a-uuid-{}", Uuid::new_v4()),
            (false, _) => Uuid::new_v4().to_string(),
        };
        sqlx::query!(
            r"
            INSERT INTO user_identity_receipts (
                receipt_id, bear_id, recorded_bear_id, actor_user_id, entity_id,
                destination_user_id, previous_user_id, displaced_entity_id,
                prior_generation, resulting_generation, recorded_at
            ) VALUES (
                ?, ?, 'source-bear', 900,
                COALESCE((SELECT entity_id FROM authenticated_user_entities
                          WHERE bear_id = ? ORDER BY entity_id LIMIT 1), ?),
                101, 11, NULL, 1, 2, '9999-01-01'
            )
            ",
            receipt_id,
            bear_id,
            bear_id,
            fallback_entity_id,
        )
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(())
}

/// Install executable, untrusted schema for import-rejection tests, never normal store use.
/// Kept here so SQLite fixture SQL is validated against SQLite, not a caller's Postgres.
pub async fn install_untrusted_import_trigger(
    store: &BearMemoryStore,
) -> Result<(), den_core::DenError> {
    sqlx::query!(
        r"
        CREATE TRIGGER imported_authentication_rebind
        AFTER INSERT ON authenticated_user_entities
        BEGIN
            INSERT OR IGNORE INTO authenticated_user_entities (bear_id, user_id, entity_id, state)
            VALUES (NEW.bear_id, 2147483647, NEW.entity_id || ':forged', 'active');
        END
        ",
    )
    .execute(store.pool())
    .await?;
    Ok(())
}
