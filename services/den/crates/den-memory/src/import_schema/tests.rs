use super::*;
use crate::{test_support::new_test_store, BearMemoryStore};
use std::path::PathBuf;
use uuid::Uuid;

struct Fixture {
    path: PathBuf,
    store: BearMemoryStore,
}

impl Fixture {
    async fn new() -> Self {
        let store = new_test_store().await;
        let path = store.pool().connect_options().get_filename().to_path_buf();
        Self { path, store }
    }

    async fn validate(self) -> Result<(), DenError> {
        // Checkpoint the fixture so the validator sees the same standalone file as an import.
        self.store.pool().close().await;
        let before = std::fs::read(&self.path).unwrap();
        let result = validate_imported_memory_schema(&self.path).await;
        assert_eq!(std::fs::read(&self.path).unwrap(), before);
        std::fs::remove_file(self.path).unwrap();
        result
    }
}

#[tokio::test]
async fn canonical_schema_is_accepted_without_changing_the_file() {
    Fixture::new().await.validate().await.unwrap();
}

#[tokio::test]
async fn legacy_schema_without_authenticated_users_is_accepted() {
    let fixture = Fixture::new().await;
    sqlx::query!("DROP TABLE IF EXISTS authenticated_user_entities")
        .execute(fixture.store.pool())
        .await
        .unwrap();
    fixture.validate().await.unwrap();
}

#[tokio::test]
async fn after_insert_trigger_cannot_recreate_imported_authentication_authority() {
    let fixture = Fixture::new().await;
    crate::test_support::install_untrusted_import_trigger(&fixture.store)
        .await
        .unwrap();
    assert!(matches!(
        fixture.validate().await,
        Err(DenError::ValidationError(_))
    ));
}

#[tokio::test]
async fn view_cannot_replace_the_entities_table() {
    let fixture = Fixture::new().await;
    sqlx::query!("DROP TABLE entities")
        .execute(fixture.store.pool())
        .await
        .unwrap();
    sqlx::query!("CREATE VIEW IF NOT EXISTS entities AS SELECT 'forged' AS entity_id")
        .execute(fixture.store.pool())
        .await
        .unwrap();
    assert!(matches!(
        fixture.validate().await,
        Err(DenError::ValidationError(_))
    ));
}

#[tokio::test]
async fn memory_links_view_must_have_the_canonical_definition() {
    let fixture = Fixture::new().await;
    sqlx::query!("DROP VIEW memory_links")
        .execute(fixture.store.pool())
        .await
        .unwrap();
    sqlx::query!("CREATE VIEW IF NOT EXISTS memory_links AS SELECT 'forged' AS entity_id")
        .execute(fixture.store.pool())
        .await
        .unwrap();
    assert!(matches!(
        fixture.validate().await,
        Err(DenError::ValidationError(_))
    ));
}

#[tokio::test]
async fn additional_views_are_rejected() {
    let fixture = Fixture::new().await;
    sqlx::query!("CREATE VIEW imported_extra_view AS SELECT 1 AS value")
        .execute(fixture.store.pool())
        .await
        .unwrap();
    assert!(matches!(
        fixture.validate().await,
        Err(DenError::ValidationError(_))
    ));
}

#[tokio::test]
async fn virtual_tables_are_rejected() {
    let fixture = Fixture::new().await;
    sqlx::query!("CREATE VIRTUAL TABLE imported_search USING fts5(content)")
        .execute(fixture.store.pool())
        .await
        .unwrap();
    assert!(matches!(
        fixture.validate().await,
        Err(DenError::ValidationError(_))
    ));
}

#[tokio::test]
async fn legacy_memory_links_table_without_foreign_keys_is_accepted() {
    let fixture = Fixture::new().await;
    sqlx::query!("DROP VIEW memory_links")
        .execute(fixture.store.pool())
        .await
        .unwrap();
    sqlx::query!("CREATE TABLE IF NOT EXISTS memory_links (link_id TEXT PRIMARY KEY)")
        .execute(fixture.store.pool())
        .await
        .unwrap();
    fixture.validate().await.unwrap();
}

#[tokio::test]
async fn foreign_key_cascade_cannot_erase_rules_during_legacy_links_migration() {
    let fixture = Fixture::new().await;
    sqlx::query!("DROP VIEW memory_links")
        .execute(fixture.store.pool())
        .await
        .unwrap();
    sqlx::query!("CREATE TABLE IF NOT EXISTS memory_links (link_id TEXT PRIMARY KEY)")
        .execute(fixture.store.pool())
        .await
        .unwrap();
    sqlx::query!("DROP TABLE memory_access_rules")
        .execute(fixture.store.pool())
        .await
        .unwrap();
    sqlx::query!(
        "CREATE TABLE IF NOT EXISTS memory_access_rules (
            link_id TEXT PRIMARY KEY,
            imported_link TEXT REFERENCES memory_links(link_id) ON DELETE CASCADE
        )"
    )
    .execute(fixture.store.pool())
    .await
    .unwrap();
    // Rejected read-only, before the manager can DROP legacy memory_links and cascade.
    let result = fixture.validate().await;
    assert!(
        matches!(result, Err(DenError::ValidationError(message)) if message.contains("foreign key"))
    );
}

#[tokio::test]
async fn legacy_schema_without_generic_view_is_accepted() {
    let fixture = Fixture::new().await;
    sqlx::query!("DROP VIEW IF EXISTS generic_memory_records")
        .execute(fixture.store.pool())
        .await
        .unwrap();
    fixture.validate().await.unwrap();
}

#[tokio::test]
async fn case_variant_table_cannot_shadow_the_reserved_generic_read_view() {
    let fixture = Fixture::new().await;
    sqlx::query!("DROP VIEW IF EXISTS generic_memory_records")
        .execute(fixture.store.pool())
        .await
        .unwrap();
    sqlx::query!("CREATE TABLE IF NOT EXISTS GeNeRiC_MeMoRy_ReCoRdS (memory_id TEXT)")
        .execute(fixture.store.pool())
        .await
        .unwrap();
    assert!(matches!(
        fixture.validate().await,
        Err(DenError::ValidationError(_))
    ));
}

#[tokio::test]
async fn validation_does_not_create_a_missing_database() {
    let path = std::env::temp_dir().join(format!("den-missing-import-{}.sqlite", Uuid::new_v4()));
    assert!(matches!(
        validate_imported_memory_schema(&path).await,
        Err(DenError::ValidationError(_))
    ));
    assert!(!path.exists());
}
