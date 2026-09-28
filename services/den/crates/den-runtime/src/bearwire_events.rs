use std::fmt;

use sqlx::{PgConnection, PgPool};
use time::{format_description::well_known::Rfc3339, OffsetDateTime};
use uuid::Uuid;

use den_core::DenError;

use bearwire_protocol::{
    lifecycle::{
        FocusedExecutionTransition, FocusedExecutionTransitionRecord,
        FOCUSED_EXECUTION_TRANSITION_EVENT_TYPE,
    },
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

pub async fn list_focused_execution_transition_records(
    pool: &PgPool,
    session_id: &str,
    limit: i64,
) -> Result<(Vec<FocusedExecutionTransitionRecord>, bool), DenError> {
    let limit = limit.clamp(1, 100);
    let rows = sqlx::query!(
        r#"
        SELECT event_json AS "event_json: serde_json::Value"
        FROM bearwire_events
        WHERE session_id = $1 AND event_type = $2
        ORDER BY sequence_no DESC
        LIMIT $3
        "#,
        session_id,
        FOCUSED_EXECUTION_TRANSITION_EVENT_TYPE,
        limit + 1,
    )
    .fetch_all(pool)
    .await?;
    decode_focused_execution_transition_records(rows.into_iter().map(|row| row.event_json), limit)
}

pub async fn list_focused_execution_transition_records_for_viewer(
    pool: &PgPool,
    bear_id: Uuid,
    user_id: i32,
    session_id: &str,
    limit: i64,
) -> Result<(Vec<FocusedExecutionTransitionRecord>, bool), DenError> {
    let limit = limit.clamp(1, 100);
    let rows = sqlx::query!(
        r#"
        SELECT event_json AS "event_json: serde_json::Value"
        FROM bearwire_events
        WHERE bear_id = $1 AND user_id = $2 AND session_id = $3 AND event_type = $4
        ORDER BY sequence_no DESC
        LIMIT $5
        "#,
        bear_id,
        user_id,
        session_id,
        FOCUSED_EXECUTION_TRANSITION_EVENT_TYPE,
        limit + 1,
    )
    .fetch_all(pool)
    .await?;
    decode_focused_execution_transition_records(rows.into_iter().map(|row| row.event_json), limit)
}

fn decode_focused_execution_transition_records(
    event_jsons: impl IntoIterator<Item = serde_json::Value>,
    limit: i64,
) -> Result<(Vec<FocusedExecutionTransitionRecord>, bool), DenError> {
    let event_jsons = event_jsons.into_iter().collect::<Vec<_>>();
    let history_truncated = event_jsons.len() > limit as usize;
    let mut records = event_jsons
        .into_iter()
        .take(limit as usize)
        .map(|event_json| {
            let event: BearWireEvent = serde_json::from_value(event_json).map_err(|error| {
                DenError::System(format!(
                    "decode focused-execution diagnostic event failed: {error}"
                ))
            })?;
            let sequence = event.sequence.ok_or_else(|| {
                DenError::System("focused-execution diagnostic event omitted sequence".to_string())
            })?;
            let time = event.time.ok_or_else(|| {
                DenError::System("focused-execution diagnostic event omitted time".to_string())
            })?;
            let transition: FocusedExecutionTransition = serde_json::from_value(event.data)
                .map_err(|error| {
                    DenError::System(format!(
                        "decode focused-execution diagnostic transition failed: {error}"
                    ))
                })?;
            Ok(FocusedExecutionTransitionRecord {
                sequence,
                time,
                transition,
            })
        })
        .collect::<Result<Vec<_>, DenError>>()?;
    records.reverse();
    Ok((records, history_truncated))
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

pub async fn list_bearwire_events_after_for_user(
    pool: &PgPool,
    bear_id: Uuid,
    user_id: i32,
    session_id: &str,
    after_sequence: Option<i64>,
    limit: i64,
) -> Result<Vec<BearWireEventRow>, DenError> {
    let limit = limit.clamp(1, 501);
    let rows = sqlx::query!(
        r#"
        SELECT id, sequence_no, session_id, event_type, event_json, created_at
        FROM bearwire_events
        WHERE bear_id = $1 AND user_id = $2 AND session_id = $3
          AND ($4::bigint IS NULL OR sequence_no > $4)
        ORDER BY sequence_no ASC
        LIMIT $5
        "#,
        bear_id,
        user_id,
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

    #[sqlx::test(migrations = "../../migrations")]
    async fn scoped_event_page_filters_identity_before_limit_and_cursor(pool: PgPool) {
        use den_service::bears::db::{self, BearParams};

        let suffix = Uuid::new_v4().simple().to_string();
        let owner = sqlx::query_scalar!(
            "INSERT INTO users (email, username, display_name, passhash) VALUES ($1, $2, $3, $4) RETURNING id",
            format!("owner-{suffix}@example.test"),
            format!("o{}", &suffix[..16]),
            "Owner",
            "unused",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        let other = sqlx::query_scalar!(
            "INSERT INTO users (email, username, display_name, passhash) VALUES ($1, $2, $3, $4) RETURNING id",
            format!("other-{suffix}@example.test"),
            format!("x{}", &suffix[..16]),
            "Other",
            "unused",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        async fn create_bear(pool: &PgPool, slug: &str) -> Uuid {
            db::create_bear(
                pool,
                BearParams {
                    slug,
                    name: "Event page test Bear",
                    description: "test",
                    system_prompt: "test",
                    default_model: None,
                    tools_enabled: None,
                    context_profile: None,
                },
            )
            .await
            .unwrap()
        }
        let bear_id = create_bear(&pool, &format!("events-{suffix}")).await;
        let other_bear_id = create_bear(&pool, &format!("other-events-{suffix}")).await;
        let session = format!("shared-{suffix}");
        let owner_first = append_ephemeral_bearwire_event(
            &pool,
            &session,
            Some(bear_id),
            Some(owner),
            "owner.first",
            serde_json::json!({"private": "owner"}),
        )
        .await
        .unwrap();
        append_ephemeral_bearwire_event(
            &pool,
            &session,
            Some(bear_id),
            Some(other),
            "other.first",
            serde_json::json!({"private": "other"}),
        )
        .await
        .unwrap();
        append_ephemeral_bearwire_event(
            &pool,
            &session,
            Some(other_bear_id),
            Some(owner),
            "other_bear.first",
            serde_json::json!({"private": "other bear"}),
        )
        .await
        .unwrap();
        append_ephemeral_bearwire_event(
            &pool,
            &session,
            Some(bear_id),
            None,
            "legacy.first",
            serde_json::json!({"private": "legacy"}),
        )
        .await
        .unwrap();
        let owner_second = append_ephemeral_bearwire_event(
            &pool,
            &session,
            Some(bear_id),
            Some(owner),
            "owner.second",
            serde_json::json!({"private": "owner second"}),
        )
        .await
        .unwrap();

        let first = list_bearwire_events_after_for_user(&pool, bear_id, owner, &session, None, 1)
            .await
            .unwrap();
        assert_eq!(
            first.iter().map(|row| row.sequence_no).collect::<Vec<_>>(),
            vec![owner_first.sequence_no]
        );
        let second = list_bearwire_events_after_for_user(
            &pool,
            bear_id,
            owner,
            &session,
            Some(owner_first.sequence_no),
            1,
        )
        .await
        .unwrap();
        assert_eq!(
            second.iter().map(|row| row.sequence_no).collect::<Vec<_>>(),
            vec![owner_second.sequence_no]
        );
        assert!(list_bearwire_events_after_for_user(
            &pool,
            bear_id,
            owner,
            &session,
            Some(owner_second.sequence_no),
            1,
        )
        .await
        .unwrap()
        .is_empty());
        assert_eq!(
            list_bearwire_events_after_for_user(&pool, bear_id, other, &session, None, 10)
                .await
                .unwrap()
                .iter()
                .map(|row| row.event_type.as_str())
                .collect::<Vec<_>>(),
            vec!["other.first"]
        );
        assert_eq!(
            list_bearwire_events_after_for_user(&pool, other_bear_id, owner, &session, None, 10)
                .await
                .unwrap()
                .iter()
                .map(|row| row.event_type.as_str())
                .collect::<Vec<_>>(),
            vec!["other_bear.first"]
        );
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn focused_execution_diagnostics_filter_viewer_before_pagination(pool: PgPool) {
        use den_service::bears::db::{self, BearParams, BEAR_ROLE_MEMBER};

        let suffix = Uuid::new_v4().simple().to_string();
        async fn user(pool: &PgPool, name: &str) -> i32 {
            sqlx::query_scalar!(
                "INSERT INTO users (email, username, display_name, passhash) VALUES ($1, $2, $3, $4) RETURNING id",
                format!("{name}@example.test"),
                name,
                name,
                "unused",
            )
            .fetch_one(pool)
            .await
            .unwrap()
        }
        async fn bear(pool: &PgPool, slug: &str) -> Uuid {
            db::create_bear(
                pool,
                BearParams {
                    slug,
                    name: "Diagnostics test Bear",
                    description: "test",
                    system_prompt: "test",
                    default_model: None,
                    tools_enabled: None,
                    context_profile: None,
                },
            )
            .await
            .unwrap()
        }
        async fn transition(
            pool: &PgPool,
            bear_id: Option<Uuid>,
            user_id: Option<i32>,
            session_id: &str,
            label: &str,
        ) -> u64 {
            let event = BearWireEvent::persistent_typed(
                FOCUSED_EXECUTION_TRANSITION_EVENT_TYPE,
                FocusedExecutionTransition {
                    state_version: 1,
                    from: None,
                    to: bearwire_protocol::lifecycle::FocusedExecutionState::Selected,
                    reason:
                        bearwire_protocol::lifecycle::FocusedExecutionTransitionReason::Reconciled,
                    correlation_id: label.to_string(),
                    causation_id: None,
                    session_id: session_id.to_string(),
                    task_id: None,
                    run_id: None,
                    attempt_id: None,
                    fence_epoch: None,
                    open_obligations: 0,
                    task_selection_preserved: true,
                },
            );
            append_bearwire_event(pool, session_id, bear_id, user_id, event)
                .await
                .unwrap()
                .sequence_no as u64
        }

        let owner = user(&pool, &format!("o{}", &suffix[..16])).await;
        let other = user(&pool, &format!("x{}", &suffix[..16])).await;
        let bear_id = bear(&pool, &format!("diagnostics-{suffix}")).await;
        let other_bear_id = bear(&pool, &format!("other-diagnostics-{suffix}")).await;
        for (user_id, bear_id) in [(owner, bear_id), (other, bear_id), (owner, other_bear_id)] {
            db::grant_membership(&pool, user_id, bear_id, Some(BEAR_ROLE_MEMBER))
                .await
                .unwrap();
        }
        let session = format!("shared-{suffix}");
        let first = transition(&pool, Some(bear_id), Some(owner), &session, "owner.first").await;
        transition(&pool, Some(bear_id), Some(other), &session, "other").await;
        transition(
            &pool,
            Some(other_bear_id),
            Some(owner),
            &session,
            "other.bear",
        )
        .await;
        transition(&pool, Some(bear_id), None, &session, "no.user").await;
        transition(&pool, None, Some(owner), &session, "no.bear").await;
        let second = transition(&pool, Some(bear_id), Some(owner), &session, "owner.second").await;
        transition(&pool, Some(bear_id), Some(other), &session, "other.latest").await;
        let third = transition(&pool, Some(bear_id), Some(owner), &session, "owner.third").await;
        transition(
            &pool,
            Some(bear_id),
            Some(owner),
            "different-session",
            "different.session",
        )
        .await;

        let (records, truncated) = list_focused_execution_transition_records_for_viewer(
            &pool, bear_id, owner, &session, 1,
        )
        .await
        .unwrap();
        assert!(truncated);
        assert_eq!(
            records
                .iter()
                .map(|record| record.sequence)
                .collect::<Vec<_>>(),
            vec![third]
        );
        let (records, truncated) = list_focused_execution_transition_records_for_viewer(
            &pool, bear_id, owner, &session, 2,
        )
        .await
        .unwrap();
        assert!(truncated);
        assert_eq!(
            records
                .iter()
                .map(|record| record.sequence)
                .collect::<Vec<_>>(),
            vec![second, third]
        );
        let (records, truncated) = list_focused_execution_transition_records_for_viewer(
            &pool, bear_id, owner, &session, 3,
        )
        .await
        .unwrap();
        assert!(!truncated);
        assert_eq!(
            records
                .iter()
                .map(|record| record.sequence)
                .collect::<Vec<_>>(),
            vec![first, second, third]
        );
        for (viewer_bear, viewer_user, expected) in [
            (bear_id, other, vec!["other", "other.latest"]),
            (other_bear_id, owner, vec!["other.bear"]),
        ] {
            let (records, truncated) = list_focused_execution_transition_records_for_viewer(
                &pool,
                viewer_bear,
                viewer_user,
                &session,
                10,
            )
            .await
            .unwrap();
            assert!(!truncated);
            assert_eq!(
                records
                    .iter()
                    .map(|record| record.transition.correlation_id.as_str())
                    .collect::<Vec<_>>(),
                expected
            );
        }
        let (records, truncated) = list_focused_execution_transition_records_for_viewer(
            &pool,
            other_bear_id,
            other,
            &session,
            10,
        )
        .await
        .unwrap();
        assert!(records.is_empty());
        assert!(!truncated);
    }

    #[test]
    fn bearwire_event_id_preserves_wire_string() {
        let id = Uuid::parse_str("67e55044-10b1-426f-9247-bb680e5fe0c8").unwrap();

        assert_eq!(
            BearWireEventId::new(id).to_string(),
            "evt_67e55044-10b1-426f-9247-bb680e5fe0c8"
        );
    }
}
