//! Human-owned retirement of private Cabinet copies. Bytes and original citations are audit,
//! not erased content; hard Bear deletion is a separate, guarded operation.

mod fingerprint;
mod history;
mod inventory;
pub(crate) mod locks;
#[cfg(test)]
mod tests;

pub use fingerprint::InventoryFingerprint;
pub use history::{history, HistoryCursor, SnapshotHistoryPage, SnapshotSummary};
use inventory::{load, project};

use super::{ArtifactLifecycle, ArtifactRef};
use den_core::{BearId, DenError, UserId};
use serde::Serialize;
use sqlx::{PgPool, Postgres, Transaction};
use time::OffsetDateTime;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RetirementBlocker {
    RequiredReferences,
    JobAuthority,
    JobNotSettled,
    InvalidSnapshot,
}
impl RetirementBlocker {
    pub fn explanation(self) -> &'static str {
        match self {
            Self::RequiredReferences => "Other records still require this copy. Resolve them through their owning workflow before retiring it.",
            Self::JobAuthority => "The copy's creator also needs authority to edit its Job. Ask the Job owner or a Bear admin to resolve access.",
            Self::JobNotSettled => "Complete or cancel the Job and settle its Docket, Work and focused execution before retiring this copy. Archive alone is not settlement.",
            Self::InvalidSnapshot => "This record is not a simple private saved copy. Keep it and ask an operator to inspect its recorded requirements.",
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct RetirementReceipt {
    #[serde(with = "time::serde::rfc3339")]
    pub retired_at: OffsetDateTime,
    pub actor: UserId,
    pub reason: String,
}

#[derive(Debug, Serialize)]
pub struct SnapshotRetirementPreview {
    pub reference: ArtifactRef,
    pub bear_id: BearId,
    pub bear_name: String,
    pub bear_slug: String,
    pub title: String,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    pub fingerprint: InventoryFingerprint,
    pub blocker: Option<RetirementBlocker>,
    pub readable: bool,
    pub receipt: Option<RetirementReceipt>,
}
impl SnapshotRetirementPreview {
    pub fn can_retire(&self) -> bool {
        self.blocker.is_none() && self.receipt.is_none()
    }
}

pub struct RetireCabinetSnapshot {
    pub actor: UserId,
    pub reference: ArtifactRef,
    pub expected: InventoryFingerprint,
    pub reason: String,
    pub acknowledged: bool,
}

/// Also used by capture: the creator identity cannot disappear during a snapshot write.
pub async fn lock_owner(
    tx: &mut Transaction<'_, Postgres>,
    actor: UserId,
    bear: BearId,
) -> Result<(), DenError> {
    locks::owner(tx, actor, bear).await
}

pub async fn preview(
    pool: &PgPool,
    actor: UserId,
    reference: &ArtifactRef,
) -> Result<SnapshotRetirementPreview, DenError> {
    let mut tx = pool.begin().await?;
    let bear = inventory::owned_bear(&mut tx, actor, reference).await?;
    lock_owner(&mut tx, actor, bear).await?;
    locks::sources(&mut tx, bear, Some(reference)).await?;
    locks::artifact(&mut tx, reference).await?;
    let row = load(&mut tx, actor, reference).await?;
    let preview = project(row)?;
    tx.commit().await?;
    Ok(preview)
}

pub async fn retire(
    pool: &PgPool,
    request: RetireCabinetSnapshot,
) -> Result<RetirementReceipt, DenError> {
    let reason = request.reason.trim();
    if !request.acknowledged || reason.is_empty() || reason.chars().count() > 2000 {
        return Err(DenError::ValidationError(
            "Provide a reason (up to 2000 characters) and acknowledge that retirement stops downloads but does not erase audit content.".into(),
        ));
    }
    let mut tx = pool.begin().await?;
    let bear = inventory::owned_bear(&mut tx, request.actor, &request.reference).await?;
    lock_owner(&mut tx, request.actor, bear).await?;
    locks::sources(&mut tx, bear, Some(&request.reference)).await?;
    locks::artifact(&mut tx, &request.reference).await?;
    let row = load(&mut tx, request.actor, &request.reference).await?;
    if let Some(receipt) = &row.receipt {
        if row.retirement_fingerprint.as_deref() == Some(request.expected.as_str())
            && receipt.actor == request.actor
            && receipt.reason == reason
            && row.lifecycle == ArtifactLifecycle::Deleted
        {
            let receipt = receipt.clone();
            tx.commit().await?;
            return Ok(receipt);
        }
        return Err(DenError::ValidationError(
            "This copy is already retired. Review its receipt; nothing was changed.".into(),
        ));
    }
    let citation_id = row.citation_id;
    let artifact_id = row.id;
    let view = project(row)?;
    if view.fingerprint != request.expected {
        return Err(DenError::ValidationError("The copy or its requirements changed. Review a fresh preview and confirm again; nothing was changed.".into()));
    }
    if let Some(blocker) = view.blocker {
        return Err(DenError::ValidationError(blocker.explanation().into()));
    }
    let receipt = sqlx::query!(
        r#"UPDATE artifact_links SET retention_released_at=NOW(),
            retention_released_by_user_id=$2,retention_release_reason=$3,retirement_fingerprint=$4
        WHERE id=$1 AND target_kind='cabinet_snapshot' AND role='citation'
          AND retention_released_at IS NULL
        RETURNING retention_released_at AS "retired_at!""#,
        citation_id
            .ok_or_else(|| DenError::ValidationError("Snapshot citation unavailable".into()))?,
        request.actor.get(),
        reason,
        request.expected.as_str()
    )
    .fetch_one(&mut *tx)
    .await?;
    let changed = sqlx::query!(
        "UPDATE artifacts SET lifecycle='deleted',deleted_at=NOW(),updated_at=NOW() WHERE id=$1 AND lifecycle='finalized'",
        artifact_id
    ).execute(&mut *tx).await?;
    if changed.rows_affected() != 1 {
        return Err(DenError::ValidationError(
            "Snapshot state changed; nothing was changed.".into(),
        ));
    }
    tx.commit().await?;
    Ok(RetirementReceipt {
        retired_at: receipt.retired_at,
        actor: request.actor,
        reason: reason.into(),
    })
}
