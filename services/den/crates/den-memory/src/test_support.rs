//! Test-only helpers: build a schema-applied per-Bear store backed by a throwaway SQLite file.

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
