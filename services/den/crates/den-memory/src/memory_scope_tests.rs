use den_core::ids::HatId;
use serde_json::json;
use sqlx::sqlite::SqlitePoolOptions;
use uuid::Uuid;

use crate::{
    append_memory_record, migrate::migrate_bear_sqlite_schema, test_support::new_test_store,
    BearMemoryStore, LogicalMemoryPath, MemoryScopeType, MemorySource,
};

#[tokio::test]
async fn source_and_hat_records_have_distinct_canonical_scopes() {
    let store = new_test_store().await;
    let source_a = MemorySource::Conversation(Uuid::new_v4());
    let source_b = MemorySource::Conversation(Uuid::new_v4());
    let hat = HatId::new(Uuid::new_v4());

    let a = append_memory_record(
        &store,
        &LogicalMemoryPath::source_local(source_a, "note"),
        "note",
        "pair",
        None,
        "# A",
        &json!({}),
    )
    .await
    .expect("write source A");
    let b = append_memory_record(
        &store,
        &LogicalMemoryPath::source_local(source_b, "note"),
        "note",
        "pair",
        None,
        "# B",
        &json!({}),
    )
    .await
    .expect("write source B");
    let curated = append_memory_record(
        &store,
        &LogicalMemoryPath::hat(hat, "note"),
        "note",
        "curate",
        None,
        "# Reviewed fact",
        &json!({}),
    )
    .await
    .expect("write curated hat record");
    assert_eq!(a.scope_type, MemoryScopeType::SourceLocal);
    assert_eq!(curated.scope_type, MemoryScopeType::Hat);
    assert_ne!(a.logical_path, b.logical_path);
    assert_ne!(a.logical_path, curated.logical_path);

    let a_scope = stored_scope(&store, &a.memory_id).await;
    assert_eq!(a_scope.0, "source_local");
    assert_eq!(a_scope.1.as_deref(), Some("conversation"));
    assert_eq!(
        a_scope.2.as_deref(),
        Some(source_a.id().to_string().as_str())
    );
    assert_eq!(a_scope.3, None);
    let hat_scope = stored_scope(&store, &curated.memory_id).await;
    assert_eq!(hat_scope.0, "hat");
    assert_eq!(hat_scope.1, None);
    assert_eq!(hat_scope.2, None);
    assert_eq!(hat_scope.3.as_deref(), Some(hat.to_string().as_str()));
}

async fn stored_scope(
    store: &BearMemoryStore,
    memory_id: &str,
) -> (String, Option<String>, Option<String>, Option<String>) {
    sqlx::query_as(
        "SELECT scope_type, scope_source_kind, scope_source_id, scope_hat_id \
         FROM memory_records WHERE memory_id = ?",
    )
    .bind(memory_id)
    .fetch_one(store.pool())
    .await
    .expect("read canonical scope columns")
}

#[tokio::test]
async fn upgrades_legacy_profile_records_without_promoting_them() {
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("connect legacy sqlite");
    sqlx::query(
        "CREATE TABLE memory_records (
            memory_id TEXT PRIMARY KEY, bear_id TEXT NOT NULL, sequence_no INTEGER NOT NULL,
            scope_type TEXT NOT NULL CHECK (scope_type IN ('profile_local', 'shared')),
            scope_profile TEXT NULL, kind TEXT NOT NULL, author_profile TEXT NOT NULL,
            author_agent_id TEXT NULL, created_at TEXT NOT NULL, content_text TEXT NOT NULL,
            metadata_json TEXT NOT NULL DEFAULT '{}', supersedes_memory_id TEXT NULL,
            visibility TEXT NOT NULL DEFAULT 'normal', logical_path TEXT NULL,
            work_surface_ref TEXT NULL, valid_from TEXT NULL, invalid_at TEXT NULL,
            salience TEXT NOT NULL DEFAULT 'normal'
        )",
    )
    .execute(&pool)
    .await
    .expect("create legacy table");
    let bear_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO memory_records
         (memory_id, bear_id, sequence_no, scope_type, scope_profile, kind,
          author_profile, created_at, content_text, logical_path)
         VALUES ('legacy-1', ?, 1, 'profile_local', 'pair', 'note', 'pair',
                 '2026-01-01T00:00:00Z', '# Private legacy note', 'pair/note.md')",
    )
    .bind(bear_id.to_string())
    .execute(&pool)
    .await
    .expect("insert legacy record");

    for statement in include_str!("schema.sql")
        .split(';')
        .map(str::trim)
        .filter(|statement| !statement.is_empty())
    {
        sqlx::query(statement)
            .execute(&pool)
            .await
            .expect("apply current schema to legacy database");
    }
    migrate_bear_sqlite_schema(&pool)
        .await
        .expect("upgrade legacy schema");
    migrate_bear_sqlite_schema(&pool)
        .await
        .expect("upgrade is idempotent");
    let store = BearMemoryStore::new(bear_id, pool);
    let legacy = stored_scope(&store, "legacy-1").await;
    assert_eq!(legacy, ("profile_local".to_string(), None, None, None));
    let source = append_memory_record(
        &store,
        &LogicalMemoryPath::source_local(MemorySource::Conversation(Uuid::new_v4()), "note"),
        "note",
        "pair",
        None,
        "# New session note",
        &json!({}),
    )
    .await
    .expect("new scope must be writable after migration");
    assert_eq!(
        stored_scope(&store, &source.memory_id).await.0,
        "source_local"
    );
}
