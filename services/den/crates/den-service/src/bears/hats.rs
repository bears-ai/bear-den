//! Bear-owned hats: durable responsibility names and resource limits.
//!
//! A hat never grants a resource by itself. Its allowed surfaces must also be
//! assigned to the Bear, and a caller's own membership and runtime policy still
//! determine whether the surface can actually be used.

use den_core::{
    ids::{BearId, HatId, UserId},
    DenError,
};
use serde::Serialize;
use sqlx::PgPool;
use time::OffsetDateTime;
use uuid::Uuid;

pub mod bindings;
pub mod core_review;
pub mod curation;
pub mod identity;
pub mod legacy_review;
pub mod manage;
pub mod memory_binding;
pub mod work_review;

#[cfg(test)]
mod tests;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BearHat {
    pub id: HatId,
    pub bear_id: BearId,
    pub name: String,
    pub purpose: String,
    /// Explicitly shared, short directory description; not inferred from purpose.
    pub short_summary: Option<String>,
    pub identity_prompt: String,
    pub work_enabled: bool,
    pub created_by_user_id: UserId,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
}

struct BearHatRow {
    id: Uuid,
    bear_id: Uuid,
    name: String,
    purpose: String,
    short_summary: Option<String>,
    identity_prompt: String,
    work_enabled: bool,
    created_by_user_id: i32,
    created_at: OffsetDateTime,
    updated_at: OffsetDateTime,
}

impl From<BearHatRow> for BearHat {
    fn from(row: BearHatRow) -> Self {
        Self {
            id: HatId::new(row.id),
            bear_id: BearId::new(row.bear_id),
            name: row.name,
            purpose: row.purpose,
            short_summary: row.short_summary,
            identity_prompt: row.identity_prompt,
            work_enabled: row.work_enabled,
            created_by_user_id: UserId::new(row.created_by_user_id),
            created_at: row.created_at,
            updated_at: row.updated_at,
        }
    }
}

pub(super) fn validate_short_summary(summary: Option<&str>) -> Result<Option<&str>, DenError> {
    let Some(value) = summary else {
        return Ok(None);
    };
    let value = value.trim();
    if value.is_empty() || value.chars().count() > 160 || value.chars().any(char::is_control) {
        return Err(DenError::ValidationError(
            "short hat summary must be one line of 1–160 characters".into(),
        ));
    }
    Ok(Some(value))
}

pub async fn create_hat(
    pool: &PgPool,
    bear_id: BearId,
    created_by_user_id: UserId,
    name: &str,
    purpose: &str,
) -> Result<BearHat, DenError> {
    create_hat_with_summary(pool, bear_id, created_by_user_id, name, purpose, None).await
}

pub async fn create_hat_with_summary(
    pool: &PgPool,
    bear_id: BearId,
    created_by_user_id: UserId,
    name: &str,
    purpose: &str,
    short_summary: Option<&str>,
) -> Result<BearHat, DenError> {
    let short_summary = validate_short_summary(short_summary)?;
    let name = name.trim();
    let purpose = purpose.trim();
    if name.is_empty() || purpose.is_empty() || purpose.len() > 4_000 {
        return Err(DenError::ValidationError(
            "hat name and purpose must be present; purpose must be at most 4000 bytes".to_string(),
        ));
    }
    let row = sqlx::query_as!(
        BearHatRow,
        r#"INSERT INTO bear_hats (bear_id, name, purpose, identity_prompt, created_by_user_id, short_summary)
           VALUES ($1, $2, $3, $3, $4, $5)
           RETURNING id, bear_id, name, purpose, short_summary, identity_prompt, work_enabled, created_by_user_id, created_at, updated_at"#,
        bear_id.as_uuid(),
        name,
        purpose,
        created_by_user_id.get(),
        short_summary,
    )
    .fetch_one(pool)
    .await
    .map_err(|err| match err {
        sqlx::Error::Database(db) if db.is_unique_violation() => DenError::ValidationError(
            "a hat with that name already exists for this Bear".into(),
        ),
        other => other.into(),
    })?;
    Ok(row.into())
}

/// The configured IDE preference, if an admin has selected one. A missing
/// preference never implies that a conversation is wearing a hat.
pub async fn ide_default_hat(pool: &PgPool, bear_id: BearId) -> Result<Option<HatId>, DenError> {
    let id = sqlx::query_scalar!(
        "SELECT ide_default_hat_id FROM bears WHERE id = $1",
        bear_id.as_uuid(),
    )
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| DenError::NotFound("Bear not found".into()))?;
    Ok(id.map(HatId::new))
}

/// Select a hat owned by this Bear as its single IDE preference. The composite
/// foreign key also protects this invariant against direct database writes.
pub async fn set_ide_default_hat(
    pool: &PgPool,
    bear_id: BearId,
    hat_id: HatId,
) -> Result<(), DenError> {
    let updated = sqlx::query!(
        "UPDATE bears SET ide_default_hat_id = $2 WHERE id = $1 AND EXISTS (SELECT 1 FROM bear_hats WHERE bear_id = $1 AND id = $2)",
        bear_id.as_uuid(),
        hat_id.as_uuid(),
    )
    .execute(pool)
    .await?;
    if updated.rows_affected() == 0 {
        return Err(DenError::NotFound("hat not found for this Bear".into()));
    }
    Ok(())
}

pub async fn list_hats(pool: &PgPool, bear_id: BearId) -> Result<Vec<BearHat>, DenError> {
    let rows = sqlx::query_as!(
        BearHatRow,
        r#"SELECT id, bear_id, name, purpose, short_summary, identity_prompt, work_enabled, created_by_user_id, created_at, updated_at
           FROM bear_hats WHERE bear_id = $1 ORDER BY name, id"#,
        bear_id.as_uuid(),
    )
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().map(Into::into).collect())
}

/// A hat can only include surfaces already assigned to its Bear. Both
/// relationships are checked by the database's composite foreign keys.
pub async fn allow_surface(
    pool: &PgPool,
    bear_id: BearId,
    hat_id: HatId,
    surface_id: Uuid,
) -> Result<(), DenError> {
    let mut surfaces = manage::allowed_surfaces(pool, bear_id, hat_id).await?;
    surfaces.push(surface_id);
    manage::replace_surfaces(pool, bear_id, hat_id, &surfaces).await
}
