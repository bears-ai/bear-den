//! Rights fence: User → Bear → membership → canonical conversation and hat.

use den_core::{
    ids::{BearId, UserId},
    DenError,
};
use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

pub(super) struct LockedMember {
    transaction: Transaction<'static, Postgres>,
    bear_id: BearId,
    user_id: UserId,
}

fn unavailable() -> DenError {
    DenError::Authorization("Personal-memory capture requires current Bear membership.".into())
}

pub(super) async fn lock_member(
    pool: &PgPool,
    bear_id: BearId,
    user_id: UserId,
) -> Result<LockedMember, DenError> {
    if user_id.get() <= 0 {
        return Err(unavailable());
    }
    let mut transaction = pool.begin().await?;
    sqlx::query!("SET TRANSACTION ISOLATION LEVEL READ COMMITTED")
        .execute(&mut *transaction)
        .await?;
    // Identity deletion locks User before Bear. No profile/email eligibility is
    // inferred here; callers authenticate the principal before entering this leaf.
    sqlx::query_scalar!(
        "SELECT id FROM users WHERE id = $1 FOR KEY SHARE",
        user_id.get(),
    )
    .fetch_optional(&mut *transaction)
    .await?
    .ok_or_else(unavailable)?;
    sqlx::query_scalar!(
        "SELECT id FROM bears WHERE id = $1 FOR UPDATE",
        bear_id.as_uuid(),
    )
    .fetch_optional(&mut *transaction)
    .await?
    .ok_or_else(unavailable)?;
    // A separate READ COMMITTED statement sees membership changes committed
    // while waiting for the Bear fence. Lock the row against direct mutations too.
    sqlx::query_scalar!(
        "SELECT user_id FROM user_bear WHERE user_id = $1 AND bear_id = $2 FOR SHARE",
        user_id.get(),
        bear_id.as_uuid(),
    )
    .fetch_optional(&mut *transaction)
    .await?
    .ok_or_else(unavailable)?;
    Ok(LockedMember {
        transaction,
        bear_id,
        user_id,
    })
}

impl LockedMember {
    pub(super) async fn lock_owned_conversation(
        &mut self,
        conversation_id: Uuid,
    ) -> Result<(), DenError> {
        // Hat sharing opt-in is irrelevant: staging remains private to the person.
        // Join the actual Bear-owned hat rather than trusting a non-null locator.
        sqlx::query_scalar!(
            r"SELECT c.id FROM conversations c
              JOIN bear_hats h ON h.id = c.hat_id AND h.bear_id = c.bear_id
              WHERE c.bear_id = $1 AND c.id = $2 AND c.created_by_user_id = $3
                AND c.status = 'active'
              FOR SHARE OF c, h",
            self.bear_id.as_uuid(),
            conversation_id,
            self.user_id.get(),
        )
        .fetch_optional(&mut *self.transaction)
        .await?
        .ok_or_else(|| {
            DenError::Authorization(
                "Personal-memory staging requires an owned active conversation with an existing Bear-owned hat."
                    .into(),
            )
        })?;
        Ok(())
    }
}
