//! Shared hard-delete admission. Compensation is conservative; web deletion explicitly
//! confirms loss of this Bear's eligible retired audit and settled private history.
mod inventory;
#[cfg(test)]
mod tests;

use crate::artifacts::snapshot_retirement::{locks, InventoryFingerprint};
use den_core::{BearId, DenError, UserId};
use serde::Serialize;
use sqlx::{PgPool, Postgres, Transaction};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BearDeletionBlocker {
    LiveWork,
    CabinetAttachments,
    SavedCopies,
    RequiredReferences,
    ExternalBytes,
    UnconfirmedAudit,
}
impl BearDeletionBlocker {
    pub fn explanation(self) -> &'static str {
        match self {
            Self::LiveWork => "Settle or cancel active Jobs, Docket runs, Work and focused execution before deleting this Bear.",
            Self::CabinetAttachments => "Cabinet attachments still retain this Bear's files. Detach only through an authorized page workflow after checking the file is no longer required.",
            Self::SavedCopies => "Saved copies still retain evidence. Review your copies below. Other private copies must be handled by their creator; ask the creator to review them without changing their private access.",
            Self::RequiredReferences => "Shared, external-owner or other required records still refer to this Bear's evidence. Resolve them through their owning workflow; deletion does not clear required evidence.",
            Self::ExternalBytes => "Registered external file bytes have not been removed. Complete the file's authorized cleanup before deleting its registry owner.",
            Self::UnconfirmedAudit => "This Bear has evidence or Job history. Review and explicitly confirm hard deletion in the web management workflow.",
        }
    }
}
#[derive(Debug, Serialize)]
pub struct BearDeletionPreview {
    pub bear_id: BearId,
    pub name: String,
    pub slug: String,
    pub fingerprint: InventoryFingerprint,
    pub blockers: Vec<BearDeletionBlocker>,
    pub can_delete: bool,
}
pub struct ConfirmBearDeletion {
    pub actor: UserId,
    pub bear_id: BearId,
    pub expected: InventoryFingerprint,
    pub confirm_slug: String,
    pub acknowledged: bool,
}

async fn lock_inventory(tx: &mut Transaction<'_, Postgres>, bear: BearId) -> Result<(), DenError> {
    locks::sources(tx, bear, None).await?;
    sqlx::query!(
        "SELECT id FROM artifacts WHERE bear_id=$1 ORDER BY id FOR UPDATE",
        bear.as_uuid()
    )
    .fetch_all(&mut **tx)
    .await?;
    sqlx::query!("SELECT l.id FROM artifact_links l JOIN artifacts a ON a.id=l.artifact_id WHERE a.bear_id=$1 ORDER BY l.id FOR UPDATE OF l NOWAIT", bear.as_uuid())
            .fetch_all(&mut **tx).await.map_err(locks::source_error)?;
    Ok(())
}
async fn authorize(
    tx: &mut Transaction<'_, Postgres>,
    actor: UserId,
    bear: BearId,
) -> Result<(), DenError> {
    locks::owner(tx, actor, bear).await?;
    locks::bear_lock(tx, bear).await?;
    let role = sqlx::query_scalar!(
        "SELECT role FROM user_bear WHERE user_id=$1 AND bear_id=$2",
        actor.get(),
        bear.as_uuid()
    )
    .fetch_one(&mut **tx)
    .await?;
    if !super::role_is_bear_admin(role.as_deref()) {
        return Err(DenError::Authorization("Bear Admin access required".into()));
    }
    Ok(())
}
pub async fn preview(
    pool: &PgPool,
    actor: UserId,
    bear: BearId,
) -> Result<BearDeletionPreview, DenError> {
    let mut tx = pool.begin().await?;
    authorize(&mut tx, actor, bear).await?;
    lock_inventory(&mut tx, bear).await?;
    let preview = inventory::read(&mut tx, bear, true).await?;
    tx.commit().await?;
    Ok(preview)
}
pub async fn delete_confirmed(pool: &PgPool, request: ConfirmBearDeletion) -> Result<(), DenError> {
    let mut tx = pool.begin().await?;
    authorize(&mut tx, request.actor, request.bear_id).await?;
    lock_inventory(&mut tx, request.bear_id).await?;
    let preview = inventory::read(&mut tx, request.bear_id, true).await?;
    if !request.acknowledged || request.confirm_slug.trim() != preview.slug {
        return Err(DenError::ValidationError("Type the current Bear handle and acknowledge the permanent loss of its eligible retired audit, Jobs and history.".into()));
    }
    if preview.fingerprint != request.expected {
        return Err(DenError::ValidationError("The deletion inventory changed. Review a fresh preview and confirm again; nothing was deleted.".into()));
    }
    if let Some(blocker) = preview.blockers.first() {
        return Err(DenError::ValidationError(blocker.explanation().into()));
    }
    remove(&mut tx, request.bear_id).await?;
    tx.commit().await?;
    Ok(())
}
/// Existing internal callers cannot silently purge finalized evidence or Job history.
pub async fn delete_unconfirmed(pool: &PgPool, bear: BearId) -> Result<(), DenError> {
    let mut tx = pool.begin().await?;
    locks::bear_lock(&mut tx, bear).await?;
    lock_inventory(&mut tx, bear).await?;
    let preview = inventory::read(&mut tx, bear, false).await?;
    if let Some(blocker) = preview.blockers.first() {
        return Err(DenError::ValidationError(blocker.explanation().into()));
    }
    remove(&mut tx, bear).await?;
    tx.commit().await?;
    Ok(())
}
async fn remove(tx: &mut Transaction<'_, Postgres>, bear: BearId) -> Result<(), DenError> {
    let result = sqlx::query!("DELETE FROM bears WHERE id=$1", bear.as_uuid())
        .execute(&mut **tx)
        .await;
    match result {
        Ok(result) if result.rows_affected()==1 => Ok(()),
        Ok(_) => Err(DenError::NotFound("Bear unavailable".into())),
        Err(sqlx::Error::Database(cause)) if cause.is_foreign_key_violation() || cause.is_check_violation() => {
            Err(DenError::ValidationError("Required records changed or still prevent deletion. Review the current blockers; nothing was deleted.".into()))
        }
        Err(error) => Err(error.into()),
    }
}
