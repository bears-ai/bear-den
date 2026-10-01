//! Bear-admin configuration operations. A hat narrows existing Bear grants;
//! these operations never provision surfaces or grant Work by themselves.

use std::collections::BTreeSet;

use den_core::{
    ids::{BearId, HatId},
    DenError,
};
use den_memory::MemoryStoreManager;
use sqlx::PgPool;
use uuid::Uuid;

use super::{identity::identity_fingerprint, list_hats, validate_short_summary, BearHat};

#[cfg(test)]
mod tests;

pub async fn get_hat(pool: &PgPool, bear_id: BearId, hat_id: HatId) -> Result<BearHat, DenError> {
    list_hats(pool, bear_id)
        .await?
        .into_iter()
        .find(|hat| hat.id == hat_id)
        .ok_or_else(|| DenError::NotFound("hat not found for this Bear".into()))
}

pub async fn update_hat(
    pool: &PgPool,
    bear_id: BearId,
    hat_id: HatId,
    name: &str,
    purpose: &str,
    identity_prompt: &str,
    confirm_work_audience: bool,
) -> Result<(), DenError> {
    let name = name.trim();
    let purpose = purpose.trim();
    let identity_prompt = identity_prompt.trim();
    if name.is_empty()
        || purpose.is_empty()
        || identity_prompt.is_empty()
        || identity_prompt.chars().count() > 4_000
    {
        return Err(DenError::ValidationError(
            "hat name, purpose, and identity text are required; identity text is limited to 4000 characters".into(),
        ));
    }
    let updated = sqlx::query!(
        "UPDATE bear_hats SET name = $3, purpose = $4, identity_prompt = $5, updated_at = NOW()
         WHERE bear_id = $1 AND id = $2 AND (NOT work_enabled OR $6)",
        bear_id.as_uuid(),
        hat_id.as_uuid(),
        name,
        purpose,
        identity_prompt,
        confirm_work_audience,
    )
    .execute(pool)
    .await
    .map_err(|err| match err {
        sqlx::Error::Database(db) if db.is_unique_violation() => {
            DenError::ValidationError("a hat with that name already exists for this Bear".into())
        }
        other => other.into(),
    })?;
    if updated.rows_affected() == 0 {
        get_hat(pool, bear_id, hat_id).await?;
        return Err(DenError::Authorization(
            "confirm the autonomous Work audience before changing this hat's identity".into(),
        ));
    }
    Ok(())
}

/// Short description shown in every bound hat's directory, including authorized
/// Work runs. Never derive it from the longer, previously private purpose text.
pub async fn set_short_summary(
    pool: &PgPool,
    bear_id: BearId,
    hat_id: HatId,
    summary: Option<&str>,
    confirm_bear_audience: bool,
) -> Result<(), DenError> {
    if !confirm_bear_audience {
        return Err(DenError::ValidationError(
            "confirm that all conversations and authorized Work can read this summary".into(),
        ));
    }
    let summary = validate_short_summary(summary)?;
    let updated = sqlx::query!(
        "UPDATE bear_hats SET short_summary = $3, updated_at = NOW() WHERE bear_id = $1 AND id = $2",
        bear_id.as_uuid(),
        hat_id.as_uuid(),
        summary,
    )
    .execute(pool)
    .await?;
    if updated.rows_affected() == 0 {
        return Err(DenError::NotFound("hat not found for this Bear".into()));
    }
    Ok(())
}

/// Bear-admin opt-in to Curate sharing verified private notes with every
/// authorized wearer of this hat, including future Job runs.
pub async fn set_auto_curate_enabled(
    pool: &PgPool,
    bear_id: BearId,
    hat_id: HatId,
    enabled: bool,
    confirm_audience: bool,
) -> Result<(), DenError> {
    if enabled && !confirm_audience {
        return Err(DenError::ValidationError(
            "confirm that Curate may read private notes and share derived knowledge with this hat's members and eligible Job runs"
                .into(),
        ));
    }
    let updated = sqlx::query!(
        "UPDATE bear_hats SET auto_curate_enabled = $3, updated_at = NOW()
         WHERE bear_id = $1 AND id = $2",
        bear_id.as_uuid(),
        hat_id.as_uuid(),
        enabled,
    )
    .execute(pool)
    .await?;
    if updated.rows_affected() == 0 {
        return Err(DenError::NotFound("hat not found for this Bear".into()));
    }
    Ok(())
}

pub async fn allowed_surfaces(
    pool: &PgPool,
    bear_id: BearId,
    hat_id: HatId,
) -> Result<Vec<Uuid>, DenError> {
    get_hat(pool, bear_id, hat_id).await?;
    Ok(sqlx::query_scalar!(
        "SELECT surface_id FROM bear_hat_work_surfaces WHERE bear_id = $1 AND hat_id = $2 ORDER BY surface_id",
        bear_id.as_uuid(), hat_id.as_uuid(),
    ).fetch_all(pool).await?)
}

/// Replace the complete allowed-surface set. A Work-enabled hat may lose grants
/// (which invalidates running Work eligibility), but may not silently gain one.
pub async fn replace_surfaces(
    pool: &PgPool,
    bear_id: BearId,
    hat_id: HatId,
    requested: &[Uuid],
) -> Result<(), DenError> {
    let requested: BTreeSet<Uuid> = requested.iter().copied().collect();
    let requested_ids: Vec<Uuid> = requested.iter().copied().collect();
    let mut tx = pool.begin().await?;
    let work_enabled = sqlx::query_scalar!(
        "SELECT work_enabled FROM bear_hats WHERE bear_id = $1 AND id = $2 FOR UPDATE",
        bear_id.as_uuid(),
        hat_id.as_uuid(),
    )
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| DenError::NotFound("hat not found for this Bear".into()))?;
    let assigned = sqlx::query_scalar!(
        "SELECT surface_id FROM work_surface_bears WHERE bear_id = $1 AND surface_id = ANY($2)",
        bear_id.as_uuid(),
        &requested_ids,
    )
    .fetch_all(&mut *tx)
    .await?;
    if assigned.len() != requested.len() {
        return Err(DenError::Authorization(
            "a selected work surface is not assigned to this Bear".into(),
        ));
    }
    let existing: BTreeSet<Uuid> = sqlx::query_scalar!(
        "SELECT surface_id FROM bear_hat_work_surfaces WHERE bear_id = $1 AND hat_id = $2",
        bear_id.as_uuid(),
        hat_id.as_uuid(),
    )
    .fetch_all(&mut *tx)
    .await?
    .into_iter()
    .collect();
    if work_enabled && !requested.is_subset(&existing) {
        return Err(DenError::Authorization(
            "disable autonomous Work and review this hat before expanding its surfaces".into(),
        ));
    }
    sqlx::query!(
        "DELETE FROM bear_hat_work_surfaces WHERE bear_id = $1 AND hat_id = $2 AND surface_id != ALL($3)",
        bear_id.as_uuid(), hat_id.as_uuid(), &requested_ids,
    ).execute(&mut *tx).await?;
    for surface in requested.difference(&existing) {
        sqlx::query!(
            "INSERT INTO bear_hat_work_surfaces (bear_id, hat_id, surface_id) VALUES ($1, $2, $3)",
            bear_id.as_uuid(),
            hat_id.as_uuid(),
            surface,
        )
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(())
}

/// Only an empty hat may enter Work without a separate memory-review flow.
/// The SQLite count includes archived and superseded rows: old curated text is
/// still part of the hat's history and must not be waved through as "empty".
pub async fn enable_work_if_empty(
    pool: &PgPool,
    stores: &MemoryStoreManager,
    bear_id: BearId,
    hat_id: HatId,
    expected_identity_sha256: &str,
) -> Result<(), DenError> {
    let mut tx = pool.begin().await?;
    let hat_state = sqlx::query!(
        "SELECT work_enabled, name, purpose, identity_prompt FROM bear_hats WHERE bear_id = $1 AND id = $2 FOR UPDATE",
        bear_id.as_uuid(),
        hat_id.as_uuid(),
    )
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| DenError::NotFound("hat not found for this Bear".into()))?;
    if hat_state.work_enabled {
        return Ok(());
    }
    if identity_fingerprint(
        &hat_state.name,
        &hat_state.purpose,
        &hat_state.identity_prompt,
    ) != expected_identity_sha256
    {
        return Err(DenError::ValidationError(
            "hat identity changed since review; refresh and inspect it again".into(),
        ));
    }
    let surfaces = sqlx::query_scalar!(
        "SELECT count(*) AS \"count!: i64\" FROM bear_hat_work_surfaces WHERE bear_id = $1 AND hat_id = $2",
        bear_id.as_uuid(), hat_id.as_uuid(),
    ).fetch_one(&mut *tx).await?;
    if surfaces == 0 {
        return Err(DenError::ValidationError(
            "select at least one Bear-assigned work surface first".into(),
        ));
    }
    let store = stores.store_for_bear(bear_id.as_uuid()).await?;
    let mut sqlite =
        store.pool().acquire().await.map_err(|err| {
            DenError::System(format!("lock Bear memory for Work enablement: {err}"))
        })?;
    // sqlx-dynamic: per-Bear SQLite has no compile-time SQLx schema. The immediate
    // transaction holds its writer lock until Postgres commits, so curation cannot
    // insert a hat record between the empty check and the Work transition.
    sqlx::query("BEGIN IMMEDIATE")
        .execute(&mut *sqlite)
        .await
        .map_err(|err| DenError::System(format!("begin hat memory review fence: {err}")))?;
    let outcome: Result<(), DenError> = async {
        let count: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM memory_records WHERE bear_id = ? AND scope_type = 'hat' AND scope_hat_id = ?"
        ).bind(bear_id.to_string()).bind(hat_id.to_string())
            .fetch_one(&mut *sqlite).await
            .map_err(|err| DenError::System(format!("count hat memory before Work enablement: {err}")))?;
        if count != 0 {
            return Err(DenError::Authorization("this hat already has reviewed memory; Work enablement requires a separate memory review".into()));
        }
        sqlx::query!(
            "UPDATE bear_hats SET work_enabled = true, updated_at = NOW() WHERE bear_id = $1 AND id = $2",
            bear_id.as_uuid(), hat_id.as_uuid(),
        ).execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(())
    }.await;
    let finish = if outcome.is_ok() {
        "COMMIT"
    } else {
        "ROLLBACK"
    };
    sqlx::query(finish)
        .execute(&mut *sqlite)
        .await
        .map_err(|err| DenError::System(format!("finish hat memory review fence: {err}")))?;
    outcome
}

pub async fn disable_work(pool: &PgPool, bear_id: BearId, hat_id: HatId) -> Result<(), DenError> {
    let updated = sqlx::query!(
        "UPDATE bear_hats SET work_enabled = false, updated_at = NOW() WHERE bear_id = $1 AND id = $2",
        bear_id.as_uuid(), hat_id.as_uuid(),
    ).execute(pool).await?;
    if updated.rows_affected() == 0 {
        return Err(DenError::NotFound("hat not found for this Bear".into()));
    }
    Ok(())
}
