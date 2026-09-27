//! Bear-owned hats: durable responsibility names and resource limits.
//!
//! A hat never grants a resource by itself. Its allowed surfaces must also be
//! assigned to the Bear, and a caller's own membership and runtime policy still
//! determine whether the surface can actually be used.

use den_core::{
    ids::{BearId, HatId, UserId},
    DenError,
};
use sqlx::PgPool;
use time::OffsetDateTime;
use uuid::Uuid;

pub mod bindings;
pub mod memory_binding;

#[cfg(test)]
mod tests;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BearHat {
    pub id: HatId,
    pub bear_id: BearId,
    pub name: String,
    pub purpose: String,
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
            work_enabled: row.work_enabled,
            created_by_user_id: UserId::new(row.created_by_user_id),
            created_at: row.created_at,
            updated_at: row.updated_at,
        }
    }
}

pub async fn create_hat(
    pool: &PgPool,
    bear_id: BearId,
    created_by_user_id: UserId,
    name: &str,
    purpose: &str,
) -> Result<BearHat, DenError> {
    let name = name.trim();
    let purpose = purpose.trim();
    if name.is_empty() || purpose.is_empty() {
        return Err(DenError::ValidationError(
            "hat name and purpose must not be empty".to_string(),
        ));
    }
    let row = sqlx::query_as!(
        BearHatRow,
        r#"INSERT INTO bear_hats (bear_id, name, purpose, created_by_user_id)
           VALUES ($1, $2, $3, $4)
           RETURNING id, bear_id, name, purpose, work_enabled, created_by_user_id, created_at, updated_at"#,
        bear_id.as_uuid(),
        name,
        purpose,
        created_by_user_id.get(),
    )
    .fetch_one(pool)
    .await?;
    Ok(row.into())
}

pub async fn list_hats(pool: &PgPool, bear_id: BearId) -> Result<Vec<BearHat>, DenError> {
    let rows = sqlx::query_as!(
        BearHatRow,
        r#"SELECT id, bear_id, name, purpose, work_enabled, created_by_user_id, created_at, updated_at
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
    sqlx::query!(
        r#"INSERT INTO bear_hat_work_surfaces (bear_id, hat_id, surface_id)
           VALUES ($1, $2, $3)"#,
        bear_id.as_uuid(),
        hat_id.as_uuid(),
        surface_id,
    )
    .execute(pool)
    .await?;
    Ok(())
}
