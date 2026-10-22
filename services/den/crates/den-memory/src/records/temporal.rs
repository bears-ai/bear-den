use std::collections::HashMap;

use den_core::DenError;
use time::OffsetDateTime;

use super::{BearMemoryStore, MemoryRecordRow, MemoryRecordSqlRow};

/// Physical canonical watermark, including protected records.
pub async fn memory_sequence_high_water(store: &BearMemoryStore) -> Result<i64, DenError> {
    let bear_id = store.bear_id().to_string();
    Ok(sqlx::query_scalar!(
        r#"SELECT MAX(sequence_no) AS "high_water?: i64" FROM memory_records WHERE bear_id = ?"#,
        bear_id,
    )
    .fetch_one(store.pool())
    .await?
    .unwrap_or(0))
}

pub(crate) fn parse_rfc3339(value: &str) -> Option<OffsetDateTime> {
    OffsetDateTime::parse(value, &time::format_description::well_known::Rfc3339).ok()
}

/// Effective times for generic-visible records only. Input IDs are not access grants.
pub async fn effective_time_by_ids(
    store: &BearMemoryStore,
    memory_ids: &[String],
) -> Result<HashMap<String, OffsetDateTime>, DenError> {
    if memory_ids.is_empty() {
        return Ok(HashMap::new());
    }
    let bear_id = store.bear_id().to_string();
    let memory_ids = serde_json::to_string(memory_ids)?;
    let rows = sqlx::query!(
        r#"
        SELECT memory_id AS "memory_id!", COALESCE(valid_from, created_at) AS "effective_at!: String"
        FROM generic_memory_records AS memory_records
        WHERE bear_id = ? AND memory_id IN (SELECT value FROM json_each(?))
        "#,
        bear_id,
        memory_ids,
    )
    .fetch_all(store.pool())
    .await?;
    Ok(rows
        .into_iter()
        .filter_map(|row| parse_rfc3339(&row.effective_at).map(|time| (row.memory_id, time)))
        .collect())
}

/// Superseder times for generic-visible predecessors. Superseders are deliberately
/// read from the base table: a protected successor must still invalidate a public predecessor.
pub async fn superseder_times_by_superseded(
    store: &BearMemoryStore,
    memory_ids: &[String],
) -> Result<HashMap<String, Vec<OffsetDateTime>>, DenError> {
    if memory_ids.is_empty() {
        return Ok(HashMap::new());
    }
    let bear_id = store.bear_id().to_string();
    let memory_ids = serde_json::to_string(memory_ids)?;
    let rows = sqlx::query!(
        r#"
        SELECT predecessor.memory_id AS "memory_id!: String",
               COALESCE(newer.valid_from, newer.created_at) AS "effective_at!: String"
        FROM generic_memory_records predecessor
        JOIN memory_records newer ON newer.bear_id = predecessor.bear_id
          AND newer.supersedes_memory_id = predecessor.memory_id
        WHERE predecessor.bear_id = ?
          AND predecessor.memory_id IN (SELECT value FROM json_each(?))
        "#,
        bear_id,
        memory_ids,
    )
    .fetch_all(store.pool())
    .await?;
    let mut times: HashMap<String, Vec<OffsetDateTime>> = HashMap::new();
    for row in rows {
        if let Some(time) = parse_rfc3339(&row.effective_at) {
            times.entry(row.memory_id).or_default().push(time);
        }
    }
    Ok(times)
}

/// Generic-visible head as of `at`. Authorization is current, not historical.
/// Hidden successors remain part of the canonical supersession check.
pub async fn head_record_as_of(
    store: &BearMemoryStore,
    logical_path: &str,
    at: OffsetDateTime,
) -> Result<Option<MemoryRecordRow>, DenError> {
    let bear_id = store.bear_id().to_string();
    let rows = sqlx::query!(
        r#"
        SELECT memory_id AS "memory_id!", sequence_no AS "sequence_no!",
               scope_type AS "scope_type!", scope_profile, kind AS "kind!",
               content_text AS "content_text!", logical_path, work_surface_ref,
               metadata_json AS "metadata_json!", created_at AS "created_at!",
               salience AS "salience!", supersedes_memory_id, invalid_at,
               COALESCE(valid_from, created_at) AS "effective_at!: String",
               (SELECT json_group_array(COALESCE(newer.valid_from, newer.created_at))
                FROM memory_records newer
                WHERE newer.bear_id = memory_records.bear_id
                  AND newer.supersedes_memory_id = memory_records.memory_id)
                   AS "superseder_times!: String"
        FROM generic_memory_records AS memory_records
        WHERE bear_id = ? AND logical_path = ? AND visibility = 'normal'
        "#,
        bear_id,
        logical_path,
    )
    .fetch_all(store.pool())
    .await?;

    let mut best: Option<(MemoryRecordSqlRow, OffsetDateTime)> = None;
    for row in rows {
        let Some(effective) = parse_rfc3339(&row.effective_at).filter(|time| *time <= at) else {
            continue;
        };
        let superseder_times: Vec<String> = serde_json::from_str(&row.superseder_times)?;
        if superseder_times
            .iter()
            .filter_map(|time| parse_rfc3339(time))
            .any(|time| time <= at)
        {
            continue;
        }
        if best.as_ref().is_some_and(|(current, time)| {
            (*time, current.sequence_no) >= (effective, row.sequence_no)
        }) {
            continue;
        }
        best = Some((
            MemoryRecordSqlRow {
                memory_id: row.memory_id,
                sequence_no: row.sequence_no,
                scope_type: row.scope_type,
                scope_profile: row.scope_profile,
                kind: row.kind,
                content_text: row.content_text,
                logical_path: row.logical_path,
                work_surface_ref: row.work_surface_ref,
                metadata_json: row.metadata_json,
                created_at: row.created_at,
                salience: row.salience,
                supersedes_memory_id: row.supersedes_memory_id,
                invalid_at: row.invalid_at,
            },
            effective,
        ));
    }
    Ok(best.map(|(row, _)| row.into_row()))
}
