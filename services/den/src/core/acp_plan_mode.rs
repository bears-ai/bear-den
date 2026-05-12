use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::{postgres::PgRow, PgPool, Row as SqlxRow};
use std::fmt;
use time::{Duration, OffsetDateTime};

pub const SUBMITTED_APPROVAL_TIMEOUT: Duration = Duration::seconds(180);
use uuid::Uuid;

use crate::errors::CustomError;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AcpPlanModeState {
    Active,
    Submitted,
    Approved,
    Rejected,
    Cancelled,
}

impl AcpPlanModeState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Submitted => "submitted",
            Self::Approved => "approved",
            Self::Rejected => "rejected",
            Self::Cancelled => "cancelled",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "active" => Some(Self::Active),
            "submitted" => Some(Self::Submitted),
            "approved" => Some(Self::Approved),
            "rejected" => Some(Self::Rejected),
            "cancelled" => Some(Self::Cancelled),
            _ => None,
        }
    }

    pub fn is_open(self) -> bool {
        matches!(self, Self::Active | Self::Submitted)
    }
}

impl fmt::Display for AcpPlanModeState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AcpPlanModeRequestedBy {
    Pair,
    User,
    System,
}

impl AcpPlanModeRequestedBy {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pair => "pair",
            Self::User => "user",
            Self::System => "system",
        }
    }
}

impl fmt::Display for AcpPlanModeRequestedBy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AcpPlanModeSessionRow {
    pub id: Uuid,
    pub user_id: i32,
    pub bear_id: Uuid,
    pub bear_slug: String,
    pub acp_session_id: String,
    pub state: String,
    pub reason: String,
    pub requested_by: String,
    pub previous_permission_mode: Option<String>,
    pub plan_artifact_path: Option<String>,
    pub plan_title: Option<String>,
    pub plan_body: Option<String>,
    pub approval_request_id: Option<String>,
    pub approved_by_user_id: Option<i32>,
    pub approved_at: Option<OffsetDateTime>,
    pub rejected_at: Option<OffsetDateTime>,
    pub closed_at: Option<OffsetDateTime>,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
}

impl AcpPlanModeSessionRow {
    pub fn parsed_state(&self) -> Result<AcpPlanModeState, CustomError> {
        AcpPlanModeState::parse(&self.state).ok_or_else(|| {
            CustomError::System(format!("unknown ACP plan mode state `{}`", self.state))
        })
    }
}

#[derive(Debug, Clone)]
pub struct EnterPlanModeParams {
    pub user_id: i32,
    pub bear_id: Uuid,
    pub bear_slug: String,
    pub acp_session_id: String,
    pub reason: String,
    pub requested_by: AcpPlanModeRequestedBy,
    pub previous_permission_mode: Option<String>,
}

#[derive(Debug, Clone)]
pub struct SubmitPlanModeParams {
    pub user_id: i32,
    pub bear_id: Uuid,
    pub acp_session_id: String,
    pub plan_mode_id: Option<Uuid>,
    pub title: String,
    pub body: String,
    pub artifact_path: String,
    pub approval_request_id: Option<String>,
}

fn clean_optional(value: Option<String>) -> Option<String> {
    value.and_then(|v| {
        let trimmed = v.trim();
        (!trimmed.is_empty()).then(|| trimmed.to_string())
    })
}

fn row_from_sql(row: &PgRow) -> AcpPlanModeSessionRow {
    AcpPlanModeSessionRow {
        id: row.get("id"),
        user_id: row.get("user_id"),
        bear_id: row.get("bear_id"),
        bear_slug: row.get("bear_slug"),
        acp_session_id: row.get("acp_session_id"),
        state: row.get("state"),
        reason: row.get("reason"),
        requested_by: row.get("requested_by"),
        previous_permission_mode: row.get("previous_permission_mode"),
        plan_artifact_path: row.get("plan_artifact_path"),
        plan_title: row.get("plan_title"),
        plan_body: row.get("plan_body"),
        approval_request_id: row.get("approval_request_id"),
        approved_by_user_id: row.get("approved_by_user_id"),
        approved_at: row.get("approved_at"),
        rejected_at: row.get("rejected_at"),
        closed_at: row.get("closed_at"),
        created_at: row.get("created_at"),
        updated_at: row.get("updated_at"),
    }
}

const SELECT_COLUMNS: &str = r#"
    id, user_id, bear_id, bear_slug, acp_session_id, state, reason, requested_by,
    previous_permission_mode, plan_artifact_path, plan_title, plan_body,
    approval_request_id, approved_by_user_id, approved_at, rejected_at, closed_at,
    created_at, updated_at
"#;

pub async fn list_for_bear(
    pool: &PgPool,
    bear_id: Uuid,
    include_closed: bool,
    limit: i64,
) -> Result<Vec<AcpPlanModeSessionRow>, CustomError> {
    let limit = limit.clamp(1, 100);
    let query = format!(
        r#"
        SELECT {SELECT_COLUMNS}
        FROM acp_plan_mode_sessions
        WHERE bear_id = $1
          AND ($2 OR state IN ('active', 'submitted'))
        ORDER BY updated_at DESC
        LIMIT $3
        "#
    );
    let rows = sqlx::query(&query)
        .bind(bear_id)
        .bind(include_closed)
        .bind(limit)
        .fetch_all(pool)
        .await?;
    Ok(rows.iter().map(row_from_sql).collect())
}

pub async fn active_for_session(
    pool: &PgPool,
    user_id: i32,
    bear_id: Uuid,
    acp_session_id: &str,
) -> Result<Option<AcpPlanModeSessionRow>, CustomError> {
    expire_stale_submitted_for_session(pool, user_id, bear_id, acp_session_id).await?;
    let query = format!(
        r#"
        SELECT {SELECT_COLUMNS}
        FROM acp_plan_mode_sessions
        WHERE user_id = $1
          AND bear_id = $2
          AND acp_session_id = $3
          AND state IN ('active', 'submitted')
        ORDER BY updated_at DESC
        LIMIT 1
        "#
    );
    let row = sqlx::query(&query)
        .bind(user_id)
        .bind(bear_id)
        .bind(acp_session_id)
        .fetch_optional(pool)
        .await?;
    Ok(row.as_ref().map(row_from_sql))
}

pub async fn get_by_id_for_bear(
    pool: &PgPool,
    user_id: i32,
    bear_id: Uuid,
    plan_mode_id: Uuid,
) -> Result<Option<AcpPlanModeSessionRow>, CustomError> {
    let query = format!(
        r#"
        SELECT {SELECT_COLUMNS}
        FROM acp_plan_mode_sessions
        WHERE id = $1 AND user_id = $2 AND bear_id = $3
        "#
    );
    let row = sqlx::query(&query)
        .bind(plan_mode_id)
        .bind(user_id)
        .bind(bear_id)
        .fetch_optional(pool)
        .await?;
    Ok(row.as_ref().map(row_from_sql))
}

pub async fn get_for_session(
    pool: &PgPool,
    user_id: i32,
    bear_id: Uuid,
    acp_session_id: &str,
    plan_mode_id: Option<Uuid>,
) -> Result<Option<AcpPlanModeSessionRow>, CustomError> {
    expire_stale_submitted_for_session(pool, user_id, bear_id, acp_session_id).await?;
    let query = if plan_mode_id.is_some() {
        format!(
            r#"
            SELECT {SELECT_COLUMNS}
            FROM acp_plan_mode_sessions
            WHERE id = $4 AND user_id = $1 AND bear_id = $2 AND acp_session_id = $3
            "#
        )
    } else {
        format!(
            r#"
            SELECT {SELECT_COLUMNS}
            FROM acp_plan_mode_sessions
            WHERE user_id = $1 AND bear_id = $2 AND acp_session_id = $3
            ORDER BY updated_at DESC
            LIMIT 1
            "#
        )
    };
    let mut q = sqlx::query(&query)
        .bind(user_id)
        .bind(bear_id)
        .bind(acp_session_id);
    if let Some(id) = plan_mode_id {
        q = q.bind(id);
    }
    let row = q.fetch_optional(pool).await?;
    Ok(row.as_ref().map(row_from_sql))
}

pub async fn enter_plan_mode(
    pool: &PgPool,
    params: EnterPlanModeParams,
) -> Result<AcpPlanModeSessionRow, CustomError> {
    if params.acp_session_id.trim().is_empty() {
        return Err(CustomError::ValidationError(
            "acp_session_id is required".to_string(),
        ));
    }
    if params.bear_slug.trim().is_empty() {
        return Err(CustomError::ValidationError(
            "bear_slug is required".to_string(),
        ));
    }

    let mut tx = pool.begin().await?;
    let existing =
        active_for_session(pool, params.user_id, params.bear_id, &params.acp_session_id).await?;
    let row = if let Some(existing) = existing {
        existing
    } else {
        let query = format!(
            r#"
            INSERT INTO acp_plan_mode_sessions (
                user_id, bear_id, bear_slug, acp_session_id, state, reason,
                requested_by, previous_permission_mode
            )
            VALUES ($1, $2, $3, $4, 'active', $5, $6, $7)
            RETURNING {SELECT_COLUMNS}
            "#
        );
        let row = sqlx::query(&query)
            .bind(params.user_id)
            .bind(params.bear_id)
            .bind(params.bear_slug.trim())
            .bind(params.acp_session_id.trim())
            .bind(params.reason.trim())
            .bind(params.requested_by.as_str())
            .bind(clean_optional(params.previous_permission_mode))
            .fetch_one(&mut *tx)
            .await?;
        row_from_sql(&row)
    };
    append_event(
        &mut tx,
        &row,
        "entered",
        json!({ "requested_by": params.requested_by.as_str(), "reason": row.reason }),
    )
    .await?;
    tx.commit().await?;
    Ok(row)
}

pub async fn submit_plan_artifact(
    pool: &PgPool,
    params: SubmitPlanModeParams,
) -> Result<AcpPlanModeSessionRow, CustomError> {
    let title = params.title.trim();
    let body = params.body.trim();
    let artifact_path = params.artifact_path.trim();
    if title.is_empty() {
        return Err(CustomError::ValidationError(
            "plan title is required".to_string(),
        ));
    }
    if body.is_empty() {
        return Err(CustomError::ValidationError(
            "plan body is required".to_string(),
        ));
    }
    if artifact_path.is_empty()
        || !artifact_path.starts_with("pair/plans/")
        || !artifact_path.ends_with(".md")
    {
        return Err(CustomError::ValidationError(
            "plan artifact path must be under pair/plans/ and end with .md".to_string(),
        ));
    }

    let mut tx = pool.begin().await?;
    let current = get_for_session(
        pool,
        params.user_id,
        params.bear_id,
        &params.acp_session_id,
        params.plan_mode_id,
    )
    .await?
    .ok_or_else(|| CustomError::NotFound("active ACP plan mode session not found".to_string()))?;
    if !current.parsed_state()?.is_open() {
        return Err(CustomError::ValidationError(
            "ACP plan mode session is already closed".to_string(),
        ));
    }

    let query = format!(
        r#"
        UPDATE acp_plan_mode_sessions
        SET state = 'submitted',
            plan_title = $5,
            plan_body = $6,
            plan_artifact_path = $7,
            approval_request_id = $8,
            updated_at = NOW()
        WHERE id = $1 AND user_id = $2 AND bear_id = $3 AND acp_session_id = $4
        RETURNING {SELECT_COLUMNS}
        "#
    );
    let row = sqlx::query(&query)
        .bind(current.id)
        .bind(params.user_id)
        .bind(params.bear_id)
        .bind(params.acp_session_id.trim())
        .bind(title)
        .bind(body)
        .bind(artifact_path)
        .bind(clean_optional(params.approval_request_id))
        .fetch_one(&mut *tx)
        .await?;
    let updated = row_from_sql(&row);
    append_event(
        &mut tx,
        &updated,
        "artifact_written",
        json!({ "artifact_path": artifact_path, "title": title }),
    )
    .await?;
    append_event(
        &mut tx,
        &updated,
        "exit_requested",
        json!({ "artifact_path": artifact_path, "title": title }),
    )
    .await?;
    tx.commit().await?;
    Ok(updated)
}

pub async fn approve_plan_mode(
    pool: &PgPool,
    user_id: i32,
    bear_id: Uuid,
    acp_session_id: &str,
    plan_mode_id: Uuid,
) -> Result<AcpPlanModeSessionRow, CustomError> {
    let current = get_by_id_for_bear(pool, user_id, bear_id, plan_mode_id)
        .await?
        .ok_or_else(|| CustomError::NotFound("ACP plan mode session not found".to_string()))?;
    expire_stale_submitted_for_session(pool, user_id, bear_id, &current.acp_session_id).await?;
    close_with_state(
        pool,
        user_id,
        bear_id,
        if acp_session_id.trim().is_empty() {
            &current.acp_session_id
        } else {
            acp_session_id
        },
        plan_mode_id,
        AcpPlanModeState::Approved,
        "approved",
    )
    .await
}

pub async fn reject_plan_mode(
    pool: &PgPool,
    user_id: i32,
    bear_id: Uuid,
    acp_session_id: &str,
    plan_mode_id: Uuid,
) -> Result<AcpPlanModeSessionRow, CustomError> {
    let current = get_by_id_for_bear(pool, user_id, bear_id, plan_mode_id)
        .await?
        .ok_or_else(|| CustomError::NotFound("ACP plan mode session not found".to_string()))?;
    close_with_state(
        pool,
        user_id,
        bear_id,
        if acp_session_id.trim().is_empty() {
            &current.acp_session_id
        } else {
            acp_session_id
        },
        plan_mode_id,
        AcpPlanModeState::Rejected,
        "rejected",
    )
    .await
}

pub async fn cancel_plan_mode(
    pool: &PgPool,
    user_id: i32,
    bear_id: Uuid,
    acp_session_id: &str,
    plan_mode_id: Option<Uuid>,
) -> Result<AcpPlanModeSessionRow, CustomError> {
    let current = if let Some(plan_mode_id) = plan_mode_id {
        get_by_id_for_bear(pool, user_id, bear_id, plan_mode_id).await?
    } else {
        get_for_session(pool, user_id, bear_id, acp_session_id, None).await?
    }
    .ok_or_else(|| CustomError::NotFound("ACP plan mode session not found".to_string()))?;
    close_with_state(
        pool,
        user_id,
        bear_id,
        &current.acp_session_id,
        current.id,
        AcpPlanModeState::Cancelled,
        "cancelled",
    )
    .await
}

pub async fn expire_stale_submitted_for_session(
    pool: &PgPool,
    user_id: i32,
    bear_id: Uuid,
    acp_session_id: &str,
) -> Result<Option<AcpPlanModeSessionRow>, CustomError> {
    let cutoff = OffsetDateTime::now_utc() - SUBMITTED_APPROVAL_TIMEOUT;
    let mut tx = pool.begin().await?;
    let query = format!(
        r#"
        UPDATE acp_plan_mode_sessions
        SET state = 'rejected',
            rejected_at = NOW(),
            closed_at = NOW(),
            updated_at = NOW()
        WHERE user_id = $1
          AND bear_id = $2
          AND acp_session_id = $3
          AND state = 'submitted'
          AND updated_at < $4
        RETURNING {SELECT_COLUMNS}
        "#
    );
    let row = sqlx::query(&query)
        .bind(user_id)
        .bind(bear_id)
        .bind(acp_session_id.trim())
        .bind(cutoff)
        .fetch_optional(&mut *tx)
        .await?;
    let Some(row) = row else {
        tx.commit().await?;
        return Ok(None);
    };
    let updated = row_from_sql(&row);
    append_event(
        &mut tx,
        &updated,
        "approval_timeout",
        json!({
            "state": "rejected",
            "timeout_seconds": SUBMITTED_APPROVAL_TIMEOUT.whole_seconds(),
        }),
    )
    .await?;
    tx.commit().await?;
    Ok(Some(updated))
}

async fn close_with_state(
    pool: &PgPool,
    user_id: i32,
    bear_id: Uuid,
    acp_session_id: &str,
    plan_mode_id: Uuid,
    state: AcpPlanModeState,
    event_type: &str,
) -> Result<AcpPlanModeSessionRow, CustomError> {
    if matches!(
        state,
        AcpPlanModeState::Active | AcpPlanModeState::Submitted
    ) {
        return Err(CustomError::System(
            "close_with_state requires a closed state".to_string(),
        ));
    }
    let mut tx = pool.begin().await?;
    let query = format!(
        r#"
        UPDATE acp_plan_mode_sessions
        SET state = $5,
            approved_by_user_id = CASE WHEN $5 = 'approved' THEN $2 ELSE approved_by_user_id END,
            approved_at = CASE WHEN $5 = 'approved' THEN NOW() ELSE approved_at END,
            rejected_at = CASE WHEN $5 = 'rejected' THEN NOW() ELSE rejected_at END,
            closed_at = NOW(),
            updated_at = NOW()
        WHERE id = $1
          AND user_id = $2
          AND bear_id = $3
          AND ($4 = '' OR acp_session_id = $4)
          AND state IN ('active', 'submitted')
        RETURNING {SELECT_COLUMNS}
        "#
    );
    let row = sqlx::query(&query)
        .bind(plan_mode_id)
        .bind(user_id)
        .bind(bear_id)
        .bind(acp_session_id.trim())
        .bind(state.as_str())
        .fetch_optional(&mut *tx)
        .await?
        .ok_or_else(|| CustomError::NotFound("open ACP plan mode session not found".to_string()))?;
    let updated = row_from_sql(&row);
    append_event(
        &mut tx,
        &updated,
        event_type,
        json!({ "state": state.as_str() }),
    )
    .await?;
    tx.commit().await?;
    Ok(updated)
}

async fn append_event(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    row: &AcpPlanModeSessionRow,
    event_type: &str,
    event_payload: Value,
) -> Result<(), CustomError> {
    sqlx::query(
        r#"
        INSERT INTO acp_plan_mode_events (
            plan_mode_id, user_id, bear_id, acp_session_id, event_type, event_payload
        )
        VALUES ($1, $2, $3, $4, $5, $6)
        "#,
    )
    .bind(row.id)
    .bind(row.user_id)
    .bind(row.bear_id)
    .bind(&row.acp_session_id)
    .bind(event_type)
    .bind(event_payload)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

pub fn render_plan_artifact_markdown(title: &str, body: &str) -> String {
    format!("# {}\n\n{}\n", title.trim(), body.trim())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_parse_round_trip() {
        for state in [
            AcpPlanModeState::Active,
            AcpPlanModeState::Submitted,
            AcpPlanModeState::Approved,
            AcpPlanModeState::Rejected,
            AcpPlanModeState::Cancelled,
        ] {
            assert_eq!(AcpPlanModeState::parse(state.as_str()), Some(state));
        }
        assert_eq!(AcpPlanModeState::parse("bogus"), None);
    }

    #[test]
    fn markdown_artifact_has_title_and_body() {
        let rendered = render_plan_artifact_markdown(" My Plan ", " - step one ");
        assert_eq!(rendered, "# My Plan\n\n- step one\n");
    }
}
