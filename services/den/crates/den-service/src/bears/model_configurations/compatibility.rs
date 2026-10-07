//! Model-only BearParams writes use the database's legacy routing boundary.
//! An unchanged model preserves the selected configuration and effort. Changed
//! models select/create effort-free alternatives without mutating named configs
//! or hat bindings. Both old SQL writers and this bridge share that policy.

use den_core::{ids::BearId, DenError};
use sqlx::PgConnection;

use super::{catalog::validate_on_connection, types::write_error};

pub(crate) async fn set_legacy_default(
    connection: &mut PgConnection,
    bear_id: BearId,
    requested_model: Option<&str>,
) -> Result<(), DenError> {
    // Caller owns the transaction for the Bear and canonical binding.
    let current = sqlx::query_scalar!(
        "SELECT default_model FROM bears WHERE id = $1 FOR UPDATE",
        bear_id.as_uuid(),
    )
    .fetch_optional(&mut *connection)
    .await?
    .ok_or_else(|| DenError::NotFound("Bear not found".into()))?;
    let requested = requested_model
        .map(str::trim)
        .filter(|model| !model.is_empty());
    let canonical_current = current
        .as_deref()
        .map(|model| den_llm::model_registry::resolve_model_handle(model).unwrap_or(model));
    let canonical_requested = requested
        .map(|model| den_llm::model_registry::resolve_model_handle(model).unwrap_or(model));
    if canonical_current == canonical_requested {
        return Ok(());
    }
    let selected_model = if let Some(model) = requested {
        Some(
            validate_on_connection(connection, model, None)
                .await?
                .model_handle,
        )
    } else {
        None
    };
    // This is a routing input, not an independently writable copy: the trigger
    // selects a canonical config and derives default_model before storage.
    sqlx::query!(
        "UPDATE bears SET default_model = $2, updated_at = now() WHERE id = $1",
        bear_id.as_uuid(),
        selected_model.as_ref().map(|model| model.as_str()),
    )
    .execute(connection)
    .await
    .map_err(write_error)?;
    Ok(())
}
