//! Add informational source labels to canonical SQLite before taking the bundle snapshot.
use crate::{errors::CustomError, web::AppState};
use den_core::ids::UserId;
use den_memory::identity_reconnection::{self as core, ExportUserLabel};
use uuid::Uuid;

pub(in super::super) async fn record_origins(
    state: &AppState,
    bear_id: Uuid,
    exporter: UserId,
) -> Result<(), CustomError> {
    let store = state.memory_stores()?.store_for_bear(bear_id).await?;
    let mut users: Vec<_> = core::list_identities(&store)
        .await?
        .into_iter()
        .filter_map(|identity| identity.active_user_id)
        .map(UserId::get)
        .collect();
    users.sort_unstable();
    users.dedup();
    let labels = sqlx::query!(
        "SELECT id, username, display_name FROM users WHERE id = ANY($1) ORDER BY id",
        &users,
    )
    .fetch_all(state.sqlx_pool())
    .await?
    .into_iter()
    .map(|row| ExportUserLabel {
        user_id: UserId::new(row.id),
        username: Some(row.username),
        display_name: Some(row.display_name),
    })
    .collect::<Vec<_>>();
    // No resolver call here: exporting must not allocate a person for the exporter.
    core::record_export_origins(&store, &state.config.web_server_url, exporter, &labels).await?;
    Ok(())
}
