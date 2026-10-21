use den_core::{ids::UserId, DenError};
use sqlx::SqliteConnection;
use uuid::Uuid;

use super::types::UserIdentityOrigin;
use crate::{clock::now_rfc3339, BearMemoryStore};

/// Safe account labels supplied by the authenticated export boundary, not a full User row.
pub struct ExportUserLabel {
    pub user_id: UserId,
    pub username: Option<String>,
    pub display_name: Option<String>,
}

/// Preserve source-qualified selection metadata before an authorized private snapshot.
/// Missing/deleted accounts still get their source account ID; export does not allocate people.
/// Repeated identical exports do not grow history. All metadata is informational on import.
pub async fn record_export_origins(
    store: &BearMemoryStore,
    source_host: &str,
    exporter: UserId,
    labels: &[ExportUserLabel],
) -> Result<(), DenError> {
    if source_host.trim().is_empty() || source_host.len() > 2048 {
        return Err(DenError::ValidationError(
            "Invalid export source host label.".into(),
        ));
    }
    let bear_id = store.bear_id().to_string();
    let recorded_at = now_rfc3339()?;
    let mut tx = store.pool().begin().await?;
    // Serialize the binding snapshot with reconnection/resolution, including across pools.
    sqlx::query!("UPDATE bear_sequence SET next_sequence = next_sequence WHERE id = 1")
        .execute(&mut *tx)
        .await?;
    let identities = sqlx::query!(
        r#"SELECT entity_id, user_id AS "user_id!: i32" FROM authenticated_user_entities
           WHERE bear_id = ? AND state = 'active'"#,
        bear_id,
    )
    .fetch_all(&mut *tx)
    .await?;
    for identity in identities {
        let user_id = UserId::new(identity.user_id);
        let label = labels.iter().find(|label| label.user_id == user_id);
        let username = label.and_then(|label| label.username.as_deref());
        let display_name = label.and_then(|label| label.display_name.as_deref());
        let was_exporter = user_id == exporter;
        let origin_id = Uuid::new_v4().to_string();
        sqlx::query!(
            r"
            INSERT INTO user_identity_origins (
                origin_id, bear_id, entity_id, source_host, source_bear_id,
                source_user_id, username, display_name, was_exporter, recorded_at
            )
            SELECT ?, ?, ?, ?, ?, ?, ?, ?, ?, ?
            WHERE NOT EXISTS (
                SELECT 1 FROM user_identity_origins
                WHERE bear_id = ? AND entity_id = ? AND source_host = ? AND source_bear_id = ?
                  AND source_user_id = ? AND username IS ? AND display_name IS ? AND was_exporter = ?
            )
            ",
            origin_id,
            bear_id,
            identity.entity_id,
            source_host,
            bear_id,
            identity.user_id,
            username,
            display_name,
            was_exporter,
            recorded_at,
            bear_id,
            identity.entity_id,
            source_host,
            bear_id,
            identity.user_id,
            username,
            display_name,
            was_exporter,
        )
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(())
}

pub(super) async fn origins_for_identity(
    connection: &mut SqliteConnection,
    bear_id: &str,
    entity_id: &str,
) -> Result<Vec<UserIdentityOrigin>, DenError> {
    let rows = sqlx::query!(
        r#"
        SELECT source_host, source_bear_id, source_user_id AS "source_user_id!: i32",
               username, display_name, was_exporter AS "was_exporter!: bool", recorded_at
        FROM user_identity_origins WHERE bear_id = ? AND entity_id = ?
        ORDER BY recorded_at, origin_id
        "#,
        bear_id,
        entity_id,
    )
    .fetch_all(connection)
    .await?;
    Ok(rows
        .into_iter()
        .map(|row| UserIdentityOrigin {
            source_host: row.source_host,
            source_bear_id: row.source_bear_id,
            source_user_id: UserId::new(row.source_user_id),
            username: row.username,
            display_name: row.display_name,
            was_exporter: row.was_exporter,
            recorded_at: row.recorded_at,
        })
        .collect())
}
