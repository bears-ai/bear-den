//! Identity deletion owns the guard because deleting a User cascades its Bear memberships.
use den_core::{BearId, DenError, UserId};
use serde::Serialize;
use sqlx::{PgConnection, PgPool};

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct UserDeletionBear {
    pub bear_id: BearId,
    pub name: String,
    pub is_admin: bool,
    pub last_admin: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct UserDeletionPreview {
    pub user_id: UserId,
    pub username: String,
    pub bears: Vec<UserDeletionBear>,
    /// Generic owner-lifecycle blocker, not a private-evidence inspection projection.
    pub has_active_private_copies: bool,
}

impl UserDeletionPreview {
    pub fn is_blocked(&self) -> bool {
        self.bears.iter().any(|bear| bear.last_admin) || self.has_active_private_copies
    }
}

#[derive(Debug, thiserror::Error)]
pub enum UserDeletionError {
    #[error("User not found")]
    NotFound,
    #[error("Grant another person Admin access to each affected Bear before deleting this user.")]
    LastBearAdmin(UserDeletionPreview),
    #[error("This account owns active privately retained saved copies. Their creator must retire eligible copies or resolve their requirements before deleting the account. No private evidence is disclosed or transferred.")]
    ActivePrivateCopies(UserDeletionPreview),
    #[error("Historical records still refer to this account. Keep the account; handing off Bear Admin access alone does not remove those references.")]
    Referenced { constraint: Option<String> },
    #[error(transparent)]
    Database(#[from] sqlx::Error),
}

impl From<UserDeletionError> for DenError {
    fn from(error: UserDeletionError) -> Self {
        match error {
            UserDeletionError::NotFound => Self::NotFound("User not found".into()),
            error @ (UserDeletionError::LastBearAdmin(_)
            | UserDeletionError::ActivePrivateCopies(_)
            | UserDeletionError::Referenced { .. }) => Self::ValidationError(error.to_string()),
            UserDeletionError::Database(error) => error.into(),
        }
    }
}

fn delete_error(error: sqlx::Error) -> UserDeletionError {
    match &error {
        sqlx::Error::Database(cause) if cause.is_foreign_key_violation() => {
            UserDeletionError::Referenced {
                constraint: cause.constraint().map(str::to_owned),
            }
        }
        _ => UserDeletionError::Database(error),
    }
}

struct DeletionMembershipRow {
    bear_id: BearId,
    name: String,
    role: Option<String>,
    other_admin_exists: bool,
}

async fn affected_bears(
    connection: &mut PgConnection,
    user_id: UserId,
) -> Result<Vec<UserDeletionBear>, sqlx::Error> {
    let rows = sqlx::query_as!(
        DeletionMembershipRow,
        r#"
        SELECT b.id AS "bear_id!: BearId", b.name, ub.role,
               EXISTS (
                   SELECT 1 FROM user_bear other
                   WHERE other.bear_id = b.id AND other.user_id <> $1
                     AND lower(btrim(coalesce(other.role, ''))) = 'admin'
               ) AS "other_admin_exists!"
        FROM user_bear ub
        JOIN bears b ON b.id = ub.bear_id
        WHERE ub.user_id = $1
        ORDER BY b.id
        "#,
        user_id.get()
    )
    .fetch_all(connection)
    .await?;
    Ok(rows
        .into_iter()
        .map(|row| {
            // Match the membership mutation boundary for the target, including Rust trim;
            // other-admin counting above retains its existing SQL normalization.
            let is_admin = row
                .role
                .as_deref()
                .is_some_and(|role| role.trim().eq_ignore_ascii_case("admin"));
            UserDeletionBear {
                bear_id: row.bear_id,
                name: row.name,
                is_admin,
                last_admin: is_admin && !row.other_admin_exists,
            }
        })
        .collect())
}

async fn has_active_private_copies(
    connection: &mut PgConnection,
    user_id: UserId,
) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar!(r#"SELECT EXISTS (SELECT 1 FROM artifacts a JOIN artifact_links l ON l.artifact_id=a.id
        WHERE a.created_by_user_id=$1 AND a.kind='cabinet_document_snapshot' AND a.visibility='same_user'
          AND a.lifecycle='finalized' AND l.target_kind='cabinet_snapshot'
          AND l.retention_released_at IS NULL) AS "blocked!""#, user_id.get())
        .fetch_one(connection).await
}

/// Advisory only. Callers must authorize disclosure; deletion always checks again under locks.
pub async fn preview_user_deletion(
    pool: &PgPool,
    user_id: UserId,
) -> Result<UserDeletionPreview, UserDeletionError> {
    let mut connection = pool.acquire().await?;
    let username = sqlx::query_scalar!("SELECT username FROM users WHERE id = $1", user_id.get())
        .fetch_optional(&mut *connection)
        .await?
        .ok_or(UserDeletionError::NotFound)?;
    let bears = affected_bears(&mut connection, user_id).await?;
    let has_active_private_copies = has_active_private_copies(&mut connection, user_id).await?;
    Ok(UserDeletionPreview {
        user_id,
        username,
        bears,
        has_active_private_copies,
    })
}

/// Lock order shared with membership changes: User first, then Bears in UUID order.
/// No memberships or historical references are explicitly removed here.
pub async fn delete_user(pool: &PgPool, user_id: UserId) -> Result<(), UserDeletionError> {
    let mut tx = pool.begin().await?;
    sqlx::query!("SET TRANSACTION ISOLATION LEVEL READ COMMITTED")
        .execute(&mut *tx)
        .await?;
    // FOR UPDATE (not NO KEY UPDATE) conflicts with the user_bear FK's KEY SHARE lock,
    // preventing new memberships from appearing after the affected Bear set is read.
    let username = sqlx::query_scalar!(
        "SELECT username FROM users WHERE id = $1 FOR UPDATE",
        user_id.get()
    )
    .fetch_optional(&mut *tx)
    .await?
    .ok_or(UserDeletionError::NotFound)?;
    sqlx::query_scalar!(
        r#"
        SELECT id FROM bears
        WHERE id IN (SELECT bear_id FROM user_bear WHERE user_id = $1)
                   OR id IN (SELECT bear_id FROM artifacts WHERE created_by_user_id=$1 AND kind='cabinet_document_snapshot')
        ORDER BY id
        FOR UPDATE
        "#,
        user_id.get()
    )
    .fetch_all(&mut *tx)
    .await?;
    // Use a separate statement after all lock waits, so READ COMMITTED sees their commits.
    let bears = affected_bears(&mut tx, user_id).await?;
    let has_active_private_copies = has_active_private_copies(&mut tx, user_id).await?;
    let preview = UserDeletionPreview {
        user_id,
        username,
        bears,
        has_active_private_copies,
    };
    if preview.bears.iter().any(|bear| bear.last_admin) {
        tx.rollback().await?;
        return Err(UserDeletionError::LastBearAdmin(preview));
    }
    if preview.has_active_private_copies {
        tx.rollback().await?;
        return Err(UserDeletionError::ActivePrivateCopies(preview));
    }
    if let Err(error) = sqlx::query!("DELETE FROM users WHERE id = $1", user_id.get())
        .execute(&mut *tx)
        .await
    {
        let error = delete_error(error);
        tx.rollback().await?;
        return Err(error);
    }
    tx.commit().await.map_err(delete_error)?;
    Ok(())
}

#[cfg(test)]
mod tests;
