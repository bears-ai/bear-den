use super::*;

/// One SQL shape covers misplaced paths/scopes, lifecycle/visibility suppression, and bad JSON.
pub(super) async fn alter_record(
    store: &BearMemoryStore,
    id: &str,
    path: &LogicalMemoryPath,
    visibility: &str,
    metadata: &str,
    invalid_at: Option<&str>,
) {
    let bear = store.bear_id().to_string();
    let logical = path.to_logical_path();
    let scope = path.scope_type.as_str();
    let profile = path.scope_profile.as_deref();
    let source_kind = path.source.map(|source| source.kind());
    let source_id = path.source.map(|source| source.id().to_string());
    let hat = path.hat_id.map(|hat| hat.to_string());
    assert_eq!(sqlx::query!(
        "UPDATE memory_records SET logical_path = ?, scope_type = ?, scope_profile = ?, scope_source_kind = ?, scope_source_id = ?, scope_hat_id = ?, visibility = ?, metadata_json = ?, invalid_at = ? WHERE bear_id = ? AND memory_id = ?",
        logical, scope, profile, source_kind, source_id, hat, visibility, metadata, invalid_at, bear, id,
    ).execute(store.pool()).await.unwrap().rows_affected(), 1);
}

pub(super) async fn lose_all_acl(store: &BearMemoryStore, id: &str) {
    let bear = store.bear_id().to_string();
    assert_eq!(
        sqlx::query!(
            "DELETE FROM memory_access_rules WHERE bear_id = ? AND src_memory_id = ?",
            bear,
            id,
        )
        .execute(store.pool())
        .await
        .unwrap()
        .rows_affected(),
        1
    );
}

#[derive(Clone, Copy)]
pub(super) enum Failure {
    Publication,
    Retraction,
}

pub(super) async fn fail_late(store: &BearMemoryStore, failure: Failure) {
    match failure {
        Failure::Publication => {
            sqlx::query!(
                r"CREATE TRIGGER fail_personal_fact_decision BEFORE INSERT ON personal_memory_publications
                  BEGIN SELECT RAISE(ABORT, 'injected late publication failure'); END",
            ).execute(store.pool()).await.unwrap();
        }
        Failure::Retraction => {
            sqlx::query!(
                r"CREATE TRIGGER fail_personal_fact_decision BEFORE INSERT ON personal_memory_retractions
                  BEGIN SELECT RAISE(ABORT, 'injected late retraction failure'); END",
            ).execute(store.pool()).await.unwrap();
        }
    }
}

pub(super) async fn clear_failure(store: &BearMemoryStore) {
    sqlx::query!("DROP TRIGGER IF EXISTS fail_personal_fact_decision")
        .execute(store.pool())
        .await
        .unwrap();
}
