//! Bear entity layer store (ADR-0042 §2, §6, §11): the Bear's portable awareness and
//! resolution of every entity it knows. Append-only discipline with first-class
//! merge/split; reads follow the `superseded_by_entity_id` forward pointer to the live
//! survivor so no history is lost.

use serde::Serialize;
use serde_json::Value;
use uuid::Uuid;

use den_core::DenError;

use super::authenticated_user::{ensure_entity_mutable, ensure_handle_mutable};
use super::clock::now_rfc3339;
use super::descriptors::{self, EntityTrust, ResolutionState};
use super::records::BearMemoryStore;

#[derive(Debug, Clone, Serialize)]
pub struct EntityRow {
    pub entity_id: String,
    pub sequence_no: i64,
    pub entity_type: String,
    pub display_name: Option<String>,
    pub resolution: ResolutionState,
    pub trust: EntityTrust,
    pub canonical_ref: Option<String>,
    pub superseded_by_entity_id: Option<String>,
    pub metadata_json: Value,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct EntityHandleRow {
    pub handle_id: String,
    pub entity_id: String,
    pub handle_type: String,
    pub handle_value: String,
    pub source: Option<String>,
    pub trust: EntityTrust,
    pub state: String,
    pub created_at: String,
}

/// Create a new entity. The `entity_type` must be a registered descriptor; resolution and
/// trust are chosen by the caller (a resolver), not inferred here.
#[allow(clippy::too_many_arguments)]
pub async fn create_entity(
    store: &BearMemoryStore,
    entity_type: &str,
    display_name: Option<&str>,
    resolution: ResolutionState,
    trust: EntityTrust,
    canonical_ref: Option<&str>,
    metadata_json: &Value,
) -> Result<EntityRow, DenError> {
    if descriptors::entity_type(entity_type).is_none() {
        return Err(DenError::ValidationError(format!(
            "unknown entity type: {entity_type}"
        )));
    }
    let entity_id = Uuid::new_v4().to_string();
    let sequence_no = store.next_sequence().await?;
    let created_at = now_rfc3339()?;
    sqlx::query(
        r"
        INSERT INTO entities (
            entity_id, bear_id, sequence_no, type, display_name, resolution, trust,
            canonical_ref, superseded_by_entity_id, metadata_json, created_at
        ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, NULL, ?, ?)
        ",
    )
    .bind(&entity_id)
    .bind(store.bear_id().to_string())
    .bind(sequence_no)
    .bind(entity_type)
    .bind(display_name)
    .bind(resolution.as_str())
    .bind(trust.as_str())
    .bind(canonical_ref)
    .bind(metadata_json.to_string())
    .bind(&created_at)
    .execute(store.pool())
    .await
    .map_err(|e| DenError::System(format!("append entity failed: {e}")))?;
    Ok(EntityRow {
        entity_id,
        sequence_no,
        entity_type: entity_type.to_string(),
        display_name: display_name.map(str::to_string),
        resolution,
        trust,
        canonical_ref: canonical_ref.map(str::to_string),
        superseded_by_entity_id: None,
        metadata_json: metadata_json.clone(),
        created_at,
    })
}

/// Fetch a single entity by id without following the forward pointer.
pub async fn get_entity(
    store: &BearMemoryStore,
    entity_id: &str,
) -> Result<Option<EntityRow>, DenError> {
    let row = sqlx::query_as::<_, EntitySqlRow>(
        r"
        SELECT entity_id, sequence_no, type, display_name, resolution, trust,
               canonical_ref, superseded_by_entity_id, metadata_json, created_at
        FROM entities
        WHERE bear_id = ? AND entity_id = ?
        ",
    )
    .bind(store.bear_id().to_string())
    .bind(entity_id)
    .fetch_optional(store.pool())
    .await
    .map_err(|e| DenError::System(format!("get entity failed: {e}")))?;
    Ok(row.map(EntitySqlRow::into_row))
}

/// Resolve to the live survivor by following `superseded_by_entity_id` (merge forward
/// pointer). Bounded to guard against accidental cycles.
pub async fn resolve_live_entity(
    store: &BearMemoryStore,
    entity_id: &str,
) -> Result<Option<EntityRow>, DenError> {
    let mut current = get_entity(store, entity_id).await?;
    let mut hops = 0;
    while let Some(row) = &current {
        match &row.superseded_by_entity_id {
            Some(next) if hops < 32 => {
                hops += 1;
                let next = next.clone();
                current = get_entity(store, &next).await?;
            }
            _ => break,
        }
    }
    Ok(current)
}

pub async fn list_entities(
    store: &BearMemoryStore,
    type_filter: Option<&str>,
    limit: i64,
) -> Result<Vec<EntityRow>, DenError> {
    let rows = if let Some(t) = type_filter {
        sqlx::query_as::<_, EntitySqlRow>(
            r"
            SELECT entity_id, sequence_no, type, display_name, resolution, trust,
                   canonical_ref, superseded_by_entity_id, metadata_json, created_at
            FROM entities
            WHERE bear_id = ? AND type = ?
            ORDER BY sequence_no DESC
            LIMIT ?
            ",
        )
        .bind(store.bear_id().to_string())
        .bind(t)
        .bind(limit)
        .fetch_all(store.pool())
        .await
    } else {
        sqlx::query_as::<_, EntitySqlRow>(
            r"
            SELECT entity_id, sequence_no, type, display_name, resolution, trust,
                   canonical_ref, superseded_by_entity_id, metadata_json, created_at
            FROM entities
            WHERE bear_id = ?
            ORDER BY sequence_no DESC
            LIMIT ?
            ",
        )
        .bind(store.bear_id().to_string())
        .bind(limit)
        .fetch_all(store.pool())
        .await
    }
    .map_err(|e| DenError::System(format!("list entities failed: {e}")))?;
    Ok(rows.into_iter().map(EntitySqlRow::into_row).collect())
}

/// Attach a handle/alias to an entity. Idempotent on (entity, type, value) for active handles.
pub async fn attach_handle(
    store: &BearMemoryStore,
    entity_id: &str,
    handle_type: &str,
    handle_value: &str,
    source: Option<&str>,
    trust: EntityTrust,
) -> Result<EntityHandleRow, DenError> {
    ensure_entity_mutable(store, entity_id).await?;
    if let Some(existing) = sqlx::query_as::<_, EntityHandleSqlRow>(
        r"
        SELECT handle_id, entity_id, handle_type, handle_value, source, trust, state, created_at
        FROM entity_handles
        WHERE bear_id = ? AND entity_id = ? AND handle_type = ? AND handle_value = ?
          AND state = 'active'
        LIMIT 1
        ",
    )
    .bind(store.bear_id().to_string())
    .bind(entity_id)
    .bind(handle_type)
    .bind(handle_value)
    .fetch_optional(store.pool())
    .await
    .map_err(|e| DenError::System(format!("handle lookup failed: {e}")))?
    {
        return Ok(existing.into_row());
    }

    let handle_id = Uuid::new_v4().to_string();
    let created_at = now_rfc3339()?;
    sqlx::query(
        r"
        INSERT INTO entity_handles (
            handle_id, bear_id, entity_id, handle_type, handle_value, source, trust, state, created_at
        ) VALUES (?, ?, ?, ?, ?, ?, ?, 'active', ?)
        ",
    )
    .bind(&handle_id)
    .bind(store.bear_id().to_string())
    .bind(entity_id)
    .bind(handle_type)
    .bind(handle_value)
    .bind(source)
    .bind(trust.as_str())
    .bind(&created_at)
    .execute(store.pool())
    .await
    .map_err(|e| DenError::System(format!("attach handle failed: {e}")))?;
    Ok(EntityHandleRow {
        handle_id,
        entity_id: entity_id.to_string(),
        handle_type: handle_type.to_string(),
        handle_value: handle_value.to_string(),
        source: source.map(str::to_string),
        trust,
        state: "active".to_string(),
        created_at,
    })
}

pub async fn detach_handle(store: &BearMemoryStore, handle_id: &str) -> Result<(), DenError> {
    ensure_handle_mutable(store, handle_id).await?;
    sqlx::query("UPDATE entity_handles SET state = 'detached' WHERE bear_id = ? AND handle_id = ?")
        .bind(store.bear_id().to_string())
        .bind(handle_id)
        .execute(store.pool())
        .await
        .map_err(|e| DenError::System(format!("detach handle failed: {e}")))?;
    Ok(())
}

pub async fn list_handles(
    store: &BearMemoryStore,
    entity_id: &str,
) -> Result<Vec<EntityHandleRow>, DenError> {
    let rows = sqlx::query_as::<_, EntityHandleSqlRow>(
        r"
        SELECT handle_id, entity_id, handle_type, handle_value, source, trust, state, created_at
        FROM entity_handles
        WHERE bear_id = ? AND entity_id = ? AND state = 'active'
        ORDER BY created_at ASC
        ",
    )
    .bind(store.bear_id().to_string())
    .bind(entity_id)
    .fetch_all(store.pool())
    .await
    .map_err(|e| DenError::System(format!("list handles failed: {e}")))?;
    Ok(rows.into_iter().map(EntityHandleSqlRow::into_row).collect())
}

/// Find the live entity carrying an active handle of the given type/value. Returns `None`
/// if no active handle matches; follows the merge forward pointer to the survivor.
pub async fn find_entity_by_handle(
    store: &BearMemoryStore,
    handle_type: &str,
    handle_value: &str,
) -> Result<Option<EntityRow>, DenError> {
    let entity_id = sqlx::query_scalar::<_, String>(
        r"
        SELECT entity_id FROM entity_handles
        WHERE bear_id = ? AND handle_type = ? AND handle_value = ? AND state = 'active'
        ORDER BY created_at ASC
        LIMIT 1
        ",
    )
    .bind(store.bear_id().to_string())
    .bind(handle_type)
    .bind(handle_value)
    .fetch_optional(store.pool())
    .await
    .map_err(|e| DenError::System(format!("find entity by handle failed: {e}")))?;
    match entity_id {
        Some(id) => resolve_live_entity(store, &id).await,
        None => Ok(None),
    }
}

/// Transition an entity's resolution state (and optionally raise trust).
pub async fn set_resolution(
    store: &BearMemoryStore,
    entity_id: &str,
    resolution: ResolutionState,
    trust: Option<EntityTrust>,
) -> Result<(), DenError> {
    ensure_entity_mutable(store, entity_id).await?;
    if let Some(trust) = trust {
        sqlx::query(
            "UPDATE entities SET resolution = ?, trust = ? WHERE bear_id = ? AND entity_id = ?",
        )
        .bind(resolution.as_str())
        .bind(trust.as_str())
        .bind(store.bear_id().to_string())
        .bind(entity_id)
        .execute(store.pool())
        .await
        .map_err(|e| DenError::System(format!("set resolution failed: {e}")))?;
    } else {
        sqlx::query("UPDATE entities SET resolution = ? WHERE bear_id = ? AND entity_id = ?")
            .bind(resolution.as_str())
            .bind(store.bear_id().to_string())
            .bind(entity_id)
            .execute(store.pool())
            .await
            .map_err(|e| DenError::System(format!("set resolution failed: {e}")))?;
    }
    Ok(())
}

pub async fn set_canonical_ref(
    store: &BearMemoryStore,
    entity_id: &str,
    canonical_ref: Option<&str>,
) -> Result<(), DenError> {
    ensure_entity_mutable(store, entity_id).await?;
    sqlx::query("UPDATE entities SET canonical_ref = ? WHERE bear_id = ? AND entity_id = ?")
        .bind(canonical_ref)
        .bind(store.bear_id().to_string())
        .bind(entity_id)
        .execute(store.pool())
        .await
        .map_err(|e| DenError::System(format!("set canonical_ref failed: {e}")))?;
    Ok(())
}

/// Merge `loser` into `survivor` (ADR-0042 §11): the loser gets `resolution = merged` and a
/// forward pointer to the survivor; its active handles re-home to the survivor so handle
/// lookups resolve correctly. Never hard-deletes — the operation is auditable from the rows.
pub async fn merge_entities(
    store: &BearMemoryStore,
    survivor_entity_id: &str,
    loser_entity_id: &str,
) -> Result<EntityRow, DenError> {
    if survivor_entity_id == loser_entity_id {
        return Err(DenError::ValidationError(
            "cannot merge an entity into itself".to_string(),
        ));
    }
    ensure_entity_mutable(store, survivor_entity_id).await?;
    ensure_entity_mutable(store, loser_entity_id).await?;
    let survivor = resolve_live_entity(store, survivor_entity_id)
        .await?
        .ok_or_else(|| DenError::NotFound(format!("survivor entity {survivor_entity_id}")))?;
    ensure_entity_mutable(store, &survivor.entity_id).await?;
    if get_entity(store, loser_entity_id).await?.is_none() {
        return Err(DenError::NotFound(format!(
            "loser entity {loser_entity_id}"
        )));
    }

    sqlx::query(
        r"
        UPDATE entities
        SET resolution = 'merged', superseded_by_entity_id = ?
        WHERE bear_id = ? AND entity_id = ?
        ",
    )
    .bind(&survivor.entity_id)
    .bind(store.bear_id().to_string())
    .bind(loser_entity_id)
    .execute(store.pool())
    .await
    .map_err(|e| DenError::System(format!("merge entity update failed: {e}")))?;

    sqlx::query(
        r"
        UPDATE entity_handles
        SET entity_id = ?
        WHERE bear_id = ? AND entity_id = ? AND state = 'active'
        ",
    )
    .bind(&survivor.entity_id)
    .bind(store.bear_id().to_string())
    .bind(loser_entity_id)
    .execute(store.pool())
    .await
    .map_err(|e| DenError::System(format!("merge handle re-home failed: {e}")))?;

    Ok(survivor)
}

/// Split a wrong merge (ADR-0042 §11): create a fresh entity and re-home the offending
/// handles onto it. Returns the new entity. Relation re-homing by `source_handle` is a
/// relation-layer concern (Phase 3) and is handled by the caller.
pub async fn split_entity(
    store: &BearMemoryStore,
    new_entity_type: &str,
    display_name: Option<&str>,
    handle_ids_to_move: &[String],
    resolution: ResolutionState,
    trust: EntityTrust,
) -> Result<EntityRow, DenError> {
    // Validate the entire move before creating an entity or re-homing any handles.
    for handle_id in handle_ids_to_move {
        ensure_handle_mutable(store, handle_id).await?;
    }
    let new_entity = create_entity(
        store,
        new_entity_type,
        display_name,
        resolution,
        trust,
        None,
        &Value::Object(Default::default()),
    )
    .await?;
    for handle_id in handle_ids_to_move {
        sqlx::query("UPDATE entity_handles SET entity_id = ? WHERE bear_id = ? AND handle_id = ?")
            .bind(&new_entity.entity_id)
            .bind(store.bear_id().to_string())
            .bind(handle_id)
            .execute(store.pool())
            .await
            .map_err(|e| DenError::System(format!("split handle re-home failed: {e}")))?;
    }
    Ok(new_entity)
}

#[derive(sqlx::FromRow)]
struct EntitySqlRow {
    entity_id: String,
    sequence_no: i64,
    r#type: String,
    display_name: Option<String>,
    resolution: String,
    trust: String,
    canonical_ref: Option<String>,
    superseded_by_entity_id: Option<String>,
    metadata_json: String,
    created_at: String,
}

impl EntitySqlRow {
    fn into_row(self) -> EntityRow {
        EntityRow {
            entity_id: self.entity_id,
            sequence_no: self.sequence_no,
            entity_type: self.r#type,
            display_name: self.display_name,
            resolution: ResolutionState::parse(&self.resolution)
                .unwrap_or(ResolutionState::Observed),
            trust: EntityTrust::parse(&self.trust).unwrap_or(EntityTrust::Inferred),
            canonical_ref: self.canonical_ref,
            superseded_by_entity_id: self.superseded_by_entity_id,
            metadata_json: serde_json::from_str(&self.metadata_json)
                .unwrap_or_else(|_| Value::Object(Default::default())),
            created_at: self.created_at,
        }
    }
}

#[derive(sqlx::FromRow)]
struct EntityHandleSqlRow {
    handle_id: String,
    entity_id: String,
    handle_type: String,
    handle_value: String,
    source: Option<String>,
    trust: String,
    state: String,
    created_at: String,
}

impl EntityHandleSqlRow {
    fn into_row(self) -> EntityHandleRow {
        EntityHandleRow {
            handle_id: self.handle_id,
            entity_id: self.entity_id,
            handle_type: self.handle_type,
            handle_value: self.handle_value,
            source: self.source,
            trust: EntityTrust::parse(&self.trust).unwrap_or(EntityTrust::Inferred),
            state: self.state,
            created_at: self.created_at,
        }
    }
}

#[cfg(test)]
mod tests;
