use std::fmt;

use sqlx::{PgConnection, PgPool};
use time::{format_description::well_known::Rfc3339, OffsetDateTime};
use uuid::Uuid;

use den_core::DenError;

use bearwire_protocol::{
    lifecycle::{FocusedExecutionTransition, FOCUSED_EXECUTION_TRANSITION_EVENT_TYPE},
    wire::{BearWireEvent, ResourceRef},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct BearWireEventId(Uuid);

impl BearWireEventId {
    fn new(id: Uuid) -> Self {
        Self(id)
    }
}

impl fmt::Display for BearWireEventId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "evt_{}", self.0)
    }
}

#[derive(Debug, Clone)]
pub struct BearWireEventRow {
    pub id: Uuid,
    pub sequence_no: i64,
    pub session_id: String,
    pub event_type: String,
    pub event: BearWireEvent,
    pub created_at: OffsetDateTime,
}

pub async fn append_bearwire_event_on(
    conn: &mut PgConnection,
    session_id: &str,
    bear_id: Option<Uuid>,
    user_id: Option<i32>,
    mut event: BearWireEvent,
) -> Result<BearWireEventRow, DenError> {
    sqlx::query!(
        "SELECT pg_advisory_xact_lock(hashtextextended($1, 0))",
        session_id
    )
    .execute(&mut *conn)
    .await?;

    let initial_json = serde_json::to_value(&event)
        .map_err(|err| DenError::System(format!("serialize BearWire event failed: {err}")))?;
    let row = sqlx::query!(
        r#"
        INSERT INTO bearwire_events (session_id, bear_id, user_id, event_type, event_json)
        VALUES ($1, $2, $3, $4, $5)
        RETURNING id, sequence_no, created_at
        "#,
        session_id,
        bear_id,
        user_id,
        &event.event_type,
        initial_json
    )
    .fetch_one(&mut *conn)
    .await?;

    let id = row.id;
    let sequence_no = row.sequence_no;
    let created_at = row.created_at;
    event.event_id = Some(BearWireEventId::new(id).to_string());
    event.sequence = Some(sequence_no as u64);
    event.time =
        Some(created_at.format(&Rfc3339).map_err(|err| {
            DenError::System(format!("format BearWire event time failed: {err}"))
        })?);
    if event.session_id.is_none() {
        event.session_id = Some(session_id.to_string());
    }

    let final_json = serde_json::to_value(&event)
        .map_err(|err| DenError::System(format!("serialize BearWire event failed: {err}")))?;
    sqlx::query!(
        "UPDATE bearwire_events SET event_json = $2 WHERE id = $1",
        id,
        final_json
    )
    .execute(&mut *conn)
    .await?;

    Ok(BearWireEventRow {
        id,
        sequence_no,
        session_id: session_id.to_string(),
        event_type: event.event_type.clone(),
        event,
        created_at,
    })
}

pub async fn append_focused_execution_transition_on(
    conn: &mut PgConnection,
    bear_id: Uuid,
    user_id: i32,
    mut transition: FocusedExecutionTransition,
) -> Result<BearWireEventRow, DenError> {
    sqlx::query!(
        "SELECT pg_advisory_xact_lock(hashtextextended($1, 0))",
        transition.session_id
    )
    .execute(&mut *conn)
    .await?;

    let previous = sqlx::query!(
        r#"
        SELECT event_json AS "event_json: serde_json::Value"
        FROM bearwire_events
        WHERE session_id = $1 AND event_type = $2
        ORDER BY sequence_no DESC
        LIMIT 1
        "#,
        transition.session_id,
        FOCUSED_EXECUTION_TRANSITION_EVENT_TYPE,
    )
    .fetch_optional(&mut *conn)
    .await?
    .map(|row| {
        serde_json::from_value::<BearWireEvent>(row.event_json)
            .and_then(|event| serde_json::from_value::<FocusedExecutionTransition>(event.data))
    })
    .transpose()
    .map_err(|error| {
        DenError::System(format!(
            "decode latest focused-execution transition failed: {error}"
        ))
    })?;

    transition.state_version = previous
        .as_ref()
        .map_or(1, |previous| previous.state_version.saturating_add(1));
    transition.from = previous.map(|previous| previous.to);

    let session_id = transition.session_id.clone();
    let run_id = transition.run_id.clone();
    let task_id = transition.task_id.clone();
    let attempt_id = transition.attempt_id.clone();
    let mut event =
        BearWireEvent::persistent_typed(FOCUSED_EXECUTION_TRANSITION_EVENT_TYPE, transition);
    event.bear_id = Some(bear_id.to_string());
    event.human_id = Some(user_id.to_string());
    event.session_id = Some(session_id.clone());
    event.run_id.clone_from(&run_id);
    event.subject = Some(format!("resource/session/{session_id}/focused_execution"));
    event
        .resource_refs
        .push(ResourceRef::new("session", session_id.clone()));
    if let Some(run_id) = run_id {
        event.resource_refs.push(ResourceRef::new("run", run_id));
    }
    if let Some(task_id) = task_id {
        event
            .resource_refs
            .push(ResourceRef::new("docket_task", task_id));
    }
    if let Some(attempt_id) = attempt_id {
        event
            .resource_refs
            .push(ResourceRef::new("docket_execution_attempt", attempt_id));
    }

    append_bearwire_event_on(conn, &session_id, Some(bear_id), Some(user_id), event).await
}

pub async fn append_focused_execution_transition(
    pool: &PgPool,
    bear_id: Uuid,
    user_id: i32,
    transition: FocusedExecutionTransition,
) -> Result<BearWireEventRow, DenError> {
    let mut tx = pool.begin().await?;
    let row = append_focused_execution_transition_on(&mut tx, bear_id, user_id, transition).await?;
    tx.commit().await?;
    Ok(row)
}

pub async fn append_ephemeral_bearwire_event(
    pool: &PgPool,
    session_id: &str,
    bear_id: Option<Uuid>,
    user_id: Option<i32>,
    event_type: &str,
    payload: serde_json::Value,
) -> Result<BearWireEventRow, DenError> {
    append_bearwire_event(
        pool,
        session_id,
        bear_id,
        user_id,
        BearWireEvent::ephemeral(event_type, payload),
    )
    .await
}

pub async fn append_bearwire_event(
    pool: &PgPool,
    session_id: &str,
    bear_id: Option<Uuid>,
    user_id: Option<i32>,
    event: BearWireEvent,
) -> Result<BearWireEventRow, DenError> {
    let mut tx = pool.begin().await?;
    let row = append_bearwire_event_on(&mut tx, session_id, bear_id, user_id, event).await?;
    tx.commit().await?;
    Ok(row)
}

pub async fn latest_event_sequence(
    pool: &PgPool,
    session_id: &str,
) -> Result<Option<i64>, DenError> {
    let row = sqlx::query!(
        r#"
        SELECT MAX(sequence_no) AS sequence_no
        FROM bearwire_events
        WHERE session_id = $1
        "#,
        session_id
    )
    .fetch_one(pool)
    .await?;
    Ok(row.sequence_no)
}

pub async fn latest_bearwire_event_of_type(
    pool: &PgPool,
    session_id: &str,
    event_type: &str,
) -> Result<Option<BearWireEventRow>, DenError> {
    let row = sqlx::query!(
        r#"
        SELECT id, sequence_no, session_id, event_type, event_json, created_at
        FROM bearwire_events
        WHERE session_id = $1
          AND event_type = $2
        ORDER BY sequence_no DESC
        LIMIT 1
        "#,
        session_id,
        event_type
    )
    .fetch_optional(pool)
    .await?;

    row.map(|row| {
        let event: BearWireEvent = serde_json::from_value(row.event_json)
            .map_err(|err| DenError::System(format!("decode BearWire event failed: {err}")))?;
        Ok(BearWireEventRow {
            id: row.id,
            sequence_no: row.sequence_no,
            session_id: row.session_id,
            event_type: row.event_type,
            event,
            created_at: row.created_at,
        })
    })
    .transpose()
}

pub async fn latest_bearwire_event_of_types(
    pool: &PgPool,
    session_id: &str,
    event_types: &[String],
) -> Result<Option<BearWireEventRow>, DenError> {
    let row = sqlx::query!(
        r#"
        SELECT id, sequence_no, session_id, event_type, event_json, created_at
        FROM bearwire_events
        WHERE session_id = $1
          AND event_type = ANY($2)
        ORDER BY sequence_no DESC
        LIMIT 1
        "#,
        session_id,
        event_types,
    )
    .fetch_optional(pool)
    .await?;

    row.map(|row| {
        let event: BearWireEvent = serde_json::from_value(row.event_json)
            .map_err(|err| DenError::System(format!("decode BearWire event failed: {err}")))?;
        Ok(BearWireEventRow {
            id: row.id,
            sequence_no: row.sequence_no,
            session_id: row.session_id,
            event_type: row.event_type,
            event,
            created_at: row.created_at,
        })
    })
    .transpose()
}

pub async fn list_bearwire_events_for_run(
    pool: &PgPool,
    run_id: &str,
    limit: i64,
) -> Result<Vec<BearWireEventRow>, DenError> {
    let limit = limit.clamp(1, 501);
    let rows = sqlx::query!(
        r#"
        SELECT id, sequence_no, session_id, event_type, event_json, created_at
        FROM bearwire_events
        WHERE event_json->>'run_id' = $1
        ORDER BY sequence_no ASC
        LIMIT $2
        "#,
        run_id,
        limit
    )
    .fetch_all(pool)
    .await?;

    rows.into_iter()
        .map(|row| {
            let event: BearWireEvent = serde_json::from_value(row.event_json)
                .map_err(|err| DenError::System(format!("decode BearWire event failed: {err}")))?;
            Ok(BearWireEventRow {
                id: row.id,
                sequence_no: row.sequence_no,
                session_id: row.session_id,
                event_type: row.event_type,
                event,
                created_at: row.created_at,
            })
        })
        .collect()
}

pub async fn list_bearwire_events_after(
    pool: &PgPool,
    session_id: &str,
    after_sequence: Option<i64>,
    limit: i64,
) -> Result<Vec<BearWireEventRow>, DenError> {
    let limit = limit.clamp(1, 501);
    let rows = sqlx::query!(
        r#"
        SELECT id, sequence_no, session_id, event_type, event_json, created_at
        FROM bearwire_events
        WHERE session_id = $1
          AND ($2::bigint IS NULL OR sequence_no > $2)
        ORDER BY sequence_no ASC
        LIMIT $3
        "#,
        session_id,
        after_sequence,
        limit
    )
    .fetch_all(pool)
    .await?;

    rows.into_iter()
        .map(|row| {
            let event: BearWireEvent = serde_json::from_value(row.event_json)
                .map_err(|err| DenError::System(format!("decode BearWire event failed: {err}")))?;
            Ok(BearWireEventRow {
                id: row.id,
                sequence_no: row.sequence_no,
                session_id: row.session_id,
                event_type: row.event_type,
                event,
                created_at: row.created_at,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bearwire_event_id_preserves_wire_string() {
        let id = Uuid::parse_str("67e55044-10b1-426f-9247-bb680e5fe0c8").unwrap();

        assert_eq!(
            BearWireEventId::new(id).to_string(),
            "evt_67e55044-10b1-426f-9247-bb680e5fe0c8"
        );
    }
}
