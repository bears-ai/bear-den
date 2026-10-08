//! Canonical owner of explicit Bear membership mutations.
use den_core::{BearId, DenError, UserId};
use sqlx::PgPool;
use uuid::Uuid;

use super::{role_is_bear_admin, BEAR_ROLE_ADMIN, BEAR_ROLE_MEMBER};

pub const LAST_BEAR_ADMIN_MESSAGE: &str =
    "Grant another person Admin access before removing this admin.";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BearMembershipRole {
    Admin,
    Member,
}

impl BearMembershipRole {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Admin => BEAR_ROLE_ADMIN,
            Self::Member => BEAR_ROLE_MEMBER,
        }
    }
}

impl TryFrom<Option<&str>> for BearMembershipRole {
    type Error = DenError;

    fn try_from(role: Option<&str>) -> Result<Self, Self::Error> {
        match role.map(str::trim) {
            None | Some("") => Ok(Self::Member),
            Some(role) if role.eq_ignore_ascii_case(BEAR_ROLE_ADMIN) => Ok(Self::Admin),
            Some(role) if role.eq_ignore_ascii_case(BEAR_ROLE_MEMBER) => Ok(Self::Member),
            Some(_) => Err(DenError::ValidationError(
                "Choose Member or Admin.".to_string(),
            )),
        }
    }
}

#[derive(Clone, Copy)]
enum MembershipChange {
    Grant(BearMembershipRole),
    Revoke,
}

/// Compatibility boundary: legacy absent/blank roles mean Member; stored roles are canonical.
pub async fn grant_membership(
    pool: &PgPool,
    user_id: i32,
    bear_id: Uuid,
    role: Option<&str>,
) -> Result<(), DenError> {
    let role = BearMembershipRole::try_from(role)?;
    change_membership(pool, user_id, bear_id, MembershipChange::Grant(role)).await
}

pub async fn revoke_membership(pool: &PgPool, user_id: i32, bear_id: Uuid) -> Result<(), DenError> {
    change_membership(pool, user_id, bear_id, MembershipChange::Revoke).await
}

async fn change_membership(
    pool: &PgPool,
    user_id: i32,
    bear_id: Uuid,
    change: MembershipChange,
) -> Result<(), DenError> {
    let user_id = UserId::from(user_id);
    let bear_id = BearId::from(bear_id);
    let mut tx = pool.begin().await?;
    // A waiter must read committed membership state after acquiring the Bear lock, not an older snapshot.
    sqlx::query!("SET TRANSACTION ISOLATION LEVEL READ COMMITTED")
        .execute(&mut *tx)
        .await?;
    // Identity deletion takes an exclusive User lock before Bear locks. Take the FK-compatible
    // User lock first even for demotions/revocations, so inserts cannot reverse that lock order.
    let user = sqlx::query_scalar!(
        "SELECT id FROM users WHERE id = $1 FOR KEY SHARE",
        user_id.get()
    )
    .fetch_optional(&mut *tx)
    .await?;
    if user.is_none() {
        return Err(DenError::NotFound("user not found".to_string()));
    }
    // Serialize grants, demotions and revocations before reading fresh membership state.
    let bear = sqlx::query_scalar!(
        "SELECT id FROM bears WHERE id = $1 FOR UPDATE",
        bear_id.as_uuid()
    )
    .fetch_optional(&mut *tx)
    .await?;
    if bear.is_none() {
        return Err(DenError::NotFound("bear not found".to_string()));
    }

    if !matches!(change, MembershipChange::Grant(BearMembershipRole::Admin)) {
        let current_role = sqlx::query_scalar!(
            "SELECT role FROM user_bear WHERE user_id = $1 AND bear_id = $2",
            user_id.get(),
            bear_id.as_uuid()
        )
        .fetch_optional(&mut *tx)
        .await?;
        if role_is_bear_admin(current_role.as_ref().and_then(|role| role.as_deref())) {
            let other_admin_exists = sqlx::query_scalar!(
                r#"
                SELECT EXISTS (
                    SELECT 1 FROM user_bear
                    WHERE bear_id = $1 AND user_id <> $2
                      AND lower(btrim(coalesce(role, ''))) = 'admin'
                ) AS "exists!"
                "#,
                bear_id.as_uuid(),
                user_id.get()
            )
            .fetch_one(&mut *tx)
            .await?;
            if !other_admin_exists {
                return Err(DenError::ValidationError(
                    LAST_BEAR_ADMIN_MESSAGE.to_string(),
                ));
            }
        }
    }

    match change {
        MembershipChange::Grant(role) => {
            sqlx::query!(
                r"
                INSERT INTO user_bear (user_id, bear_id, role)
                VALUES ($1, $2, $3)
                ON CONFLICT (user_id, bear_id) DO UPDATE SET role = EXCLUDED.role
                ",
                user_id.get(),
                bear_id.as_uuid(),
                role.as_str()
            )
            .execute(&mut *tx)
            .await?;
        }
        MembershipChange::Revoke => {
            let result = sqlx::query!(
                "DELETE FROM user_bear WHERE user_id = $1 AND bear_id = $2",
                user_id.get(),
                bear_id.as_uuid()
            )
            .execute(&mut *tx)
            .await?;
            if result.rows_affected() == 0 {
                return Err(DenError::NotFound("membership not found".to_string()));
            }
        }
    }
    tx.commit().await?;
    Ok(())
}

#[cfg(test)]
mod tests;
