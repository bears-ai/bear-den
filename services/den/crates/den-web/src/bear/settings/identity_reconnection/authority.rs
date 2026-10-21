//! The narrow human-only restore authority. Keep these locks through the SQLite commit.
use crate::errors::CustomError;
use den_core::ids::{BearId, UserId};
use den_service::bears::db::role_is_bear_admin;
use serde::{Deserialize, Serialize};
use sqlx::{PgPool, Postgres, Transaction};

/// Deliberately excludes email, credentials, and the global operator flag.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct AccountLabel {
    pub user_id: UserId,
    pub username: String,
    pub display_name: String,
}

pub(super) struct LockedAuthority {
    transaction: Transaction<'static, Postgres>,
    bear_id: BearId,
    pub actor: AccountLabel,
    pub destination: Option<AccountLabel>,
}

fn unavailable() -> CustomError {
    CustomError::Authorization(
        "Identity reconnection requires a currently verified Bear Admin and a verified destination Bear member. Refresh the identity list after repairing access.".into(),
    )
}

impl LockedAuthority {
    pub async fn eligible_accounts(&mut self) -> Result<Vec<AccountLabel>, CustomError> {
        let bear_id = self.bear_id;
        Ok(sqlx::query_as!(
            AccountLabel,
            r#"
        SELECT u.id AS "user_id: UserId", u.username, u.display_name
        FROM users u
        JOIN user_bear membership ON membership.user_id = u.id
        WHERE membership.bear_id = $1
          AND EXISTS (
              SELECT 1 FROM email_configs e
              WHERE e.user_id = u.id AND e.active AND e.verified_at IS NOT NULL
          )
        ORDER BY u.username, u.id
        "#,
            bear_id.as_uuid(),
        )
        .fetch_all(&mut *self.transaction)
        .await?)
    }
}

pub(super) async fn lock(
    pool: &PgPool,
    bear_id: BearId,
    actor: UserId,
    destination: Option<UserId>,
) -> Result<LockedAuthority, CustomError> {
    let mut transaction = pool.begin().await?;
    sqlx::query!("SET TRANSACTION ISOLATION LEVEL READ COMMITTED")
        .execute(&mut *transaction)
        .await?;
    let mut users = vec![actor];
    users.extend(destination);
    users.sort_unstable();
    users.dedup();
    let mut labels = Vec::with_capacity(users.len());
    // User -> email config -> Bear. SHARE also blocks non-key profile/email updates;
    // KEY SHARE alone would not freeze the eligibility shown on the review page.
    for user in &users {
        if user.get() <= 0 {
            return Err(unavailable());
        }
        let label = sqlx::query_as!(
            AccountLabel,
            r#"SELECT id AS "user_id: UserId", username, display_name
               FROM users WHERE id = $1 FOR SHARE"#,
            user.get(),
        )
        .fetch_optional(&mut *transaction)
        .await?
        .ok_or_else(unavailable)?;
        labels.push(label);
    }
    for user in &users {
        let configurations = sqlx::query!(
            r#"SELECT id, (verified_at IS NOT NULL) AS "verified!"
               FROM email_configs WHERE user_id = $1 AND active
               ORDER BY id FOR SHARE"#,
            user.get(),
        )
        .fetch_all(&mut *transaction)
        .await?;
        if !configurations
            .iter()
            .any(|configuration| configuration.verified)
        {
            return Err(unavailable());
        }
    }
    sqlx::query_scalar!(
        "SELECT id FROM bears WHERE id = $1 FOR UPDATE",
        bear_id.as_uuid()
    )
    .fetch_optional(&mut *transaction)
    .await?
    .ok_or_else(unavailable)?;
    // Separate statements after the Bear lock wait see fresh committed membership.
    for user in &users {
        let role = sqlx::query_scalar!(
            "SELECT role FROM user_bear WHERE user_id = $1 AND bear_id = $2",
            user.get(),
            bear_id.as_uuid(),
        )
        .fetch_optional(&mut *transaction)
        .await?
        .ok_or_else(unavailable)?;
        if *user == actor && !role_is_bear_admin(role.as_deref()) {
            return Err(unavailable());
        }
    }
    let actor_label = labels
        .iter()
        .find(|label| label.user_id == actor)
        .cloned()
        .ok_or_else(unavailable)?;
    let destination_label = destination
        .map(|user| {
            labels
                .iter()
                .find(|label| label.user_id == user)
                .cloned()
                .ok_or_else(unavailable)
        })
        .transpose()?;
    Ok(LockedAuthority {
        transaction,
        bear_id,
        actor: actor_label,
        destination: destination_label,
    })
}
