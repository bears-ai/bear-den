use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::{types::Json, FromRow, PgPool};
use std::fmt;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::core::bears::BearAgentRole;
use crate::errors::CustomError;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkPlanVisibility {
    PrivateToRole,
    SameUser,
    BearVisible,
    HandoffRequested,
}

impl WorkPlanVisibility {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::PrivateToRole => "private_to_role",
            Self::SameUser => "same_user",
            Self::BearVisible => "bear_visible",
            Self::HandoffRequested => "handoff_requested",
        }
    }

    pub fn parse(value: &str) -> Result<Self, CustomError> {
        match value.trim() {
            "private_to_role" => Ok(Self::PrivateToRole),
            "same_user" => Ok(Self::SameUser),
            "bear_visible" => Ok(Self::BearVisible),
            "handoff_requested" => Ok(Self::HandoffRequested),
            other => Err(CustomError::Parsing(format!(
                "unknown work plan visibility: {other}"
            ))),
        }
    }
}

impl fmt::Display for WorkPlanVisibility {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkPlanStatus {
    Active,
    Blocked,
    Completed,
    Cancelled,
    Archived,
}

impl WorkPlanStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Blocked => "blocked",
            Self::Completed => "completed",
            Self::Cancelled => "cancelled",
            Self::Archived => "archived",
        }
    }

    pub fn parse(value: &str) -> Result<Self, CustomError> {
        match value.trim() {
            "active" => Ok(Self::Active),
            "blocked" => Ok(Self::Blocked),
            "completed" => Ok(Self::Completed),
            "cancelled" => Ok(Self::Cancelled),
            "archived" => Ok(Self::Archived),
            other => Err(CustomError::Parsing(format!(
                "unknown work plan status: {other}"
            ))),
        }
    }
}

impl fmt::Display for WorkPlanStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkPlanItemStatus {
    Pending,
    InProgress,
    Blocked,
    Completed,
    Cancelled,
}

impl WorkPlanItemStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::InProgress => "in_progress",
            Self::Blocked => "blocked",
            Self::Completed => "completed",
            Self::Cancelled => "cancelled",
        }
    }
}

impl fmt::Display for WorkPlanItemStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkPlanItem {
    pub id: String,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    pub status: WorkPlanItemStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blocked_reason: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub source_refs: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkPlanUpdate {
    pub title: String,
    #[serde(default)]
    pub summary: String,
    pub visibility: WorkPlanVisibility,
    pub status: WorkPlanStatus,
    #[serde(default)]
    pub items: Vec<WorkPlanItem>,
    #[serde(default = "default_json_object")]
    pub workspace_context: serde_json::Value,
}

fn default_json_object() -> serde_json::Value {
    serde_json::json!({})
}

#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct BearWorkPlanRow {
    pub id: Uuid,
    pub bear_id: Uuid,
    pub title: String,
    pub summary: String,
    pub owner_role: String,
    pub owner_agent_id: Option<String>,
    pub created_by_user_id: Option<i32>,
    pub source_conversation_id: Option<String>,
    pub source_acp_session_id: Option<String>,
    pub source_channel: Json<serde_json::Value>,
    pub workspace_context: Json<serde_json::Value>,
    pub visibility: String,
    pub status: String,
    pub items: Json<Vec<WorkPlanItem>>,
    pub version: i32,
    pub handoff_intent_path: Option<String>,
    pub handoff_task_id: Option<String>,
    pub archived_at: Option<OffsetDateTime>,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
}

#[derive(Debug, Clone)]
pub struct WorkPlanUpsert {
    pub bear_id: Uuid,
    pub owner_role: BearAgentRole,
    pub owner_agent_id: Option<String>,
    pub created_by_user_id: Option<i32>,
    pub source_conversation_id: Option<String>,
    pub source_acp_session_id: Option<String>,
    pub source_channel: Value,
    pub plan_id: Option<Uuid>,
    pub expected_version: Option<i32>,
    pub update: WorkPlanUpdate,
}

#[derive(Debug, Clone, Default)]
pub struct WorkPlanListFilter {
    pub statuses: Option<Vec<WorkPlanStatus>>,
    pub owner_role: Option<BearAgentRole>,
    pub include_archived: bool,
}

#[derive(Debug, Clone, Default)]
pub struct WorkPlanLookup {
    pub plan_id: Option<Uuid>,
    pub source_conversation_id: Option<String>,
    pub source_acp_session_id: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct WorkPlanProjection {
    pub id: Uuid,
    pub bear_id: Uuid,
    pub title: String,
    pub summary: String,
    pub owner_role: String,
    pub visibility: String,
    pub status: String,
    pub version: i32,
    pub items: Vec<WorkPlanItem>,
    pub current_item: Option<WorkPlanItem>,
    pub source_conversation_id: Option<String>,
    pub source_acp_session_id: Option<String>,
    pub handoff_intent_path: Option<String>,
    pub handoff_task_id: Option<String>,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkPlanValidationError {
    EmptyTitle,
    EmptyItemId,
    EmptyItemTitle { item_id: String },
    MultipleInProgressItems,
    BlockedItemMissingReason { item_id: String },
}

impl fmt::Display for WorkPlanValidationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyTitle => f.write_str("work plan title must not be empty"),
            Self::EmptyItemId => f.write_str("work plan item id must not be empty"),
            Self::EmptyItemTitle { item_id } => {
                write!(f, "work plan item `{item_id}` title must not be empty")
            }
            Self::MultipleInProgressItems => {
                f.write_str("work plan may have at most one in_progress item")
            }
            Self::BlockedItemMissingReason { item_id } => {
                write!(
                    f,
                    "blocked work plan item `{item_id}` must include blocked_reason"
                )
            }
        }
    }
}

impl std::error::Error for WorkPlanValidationError {}

impl From<WorkPlanValidationError> for CustomError {
    fn from(err: WorkPlanValidationError) -> Self {
        CustomError::ValidationError(err.to_string())
    }
}

impl BearWorkPlanRow {
    pub fn parsed_owner_role(&self) -> Result<BearAgentRole, CustomError> {
        self.owner_role.parse().map_err(CustomError::Parsing)
    }

    pub fn parsed_visibility(&self) -> Result<WorkPlanVisibility, CustomError> {
        WorkPlanVisibility::parse(&self.visibility)
    }

    pub fn parsed_status(&self) -> Result<WorkPlanStatus, CustomError> {
        WorkPlanStatus::parse(&self.status)
    }

    pub fn is_visible_to(
        &self,
        viewer_role: BearAgentRole,
        user_id: i32,
    ) -> Result<bool, CustomError> {
        let owner_role = self.parsed_owner_role()?;
        let visibility = self.parsed_visibility()?;
        let same_user = self.created_by_user_id == Some(user_id);
        Ok(role_can_read_work_plan(
            viewer_role,
            owner_role,
            visibility,
            same_user,
        ))
    }

    pub fn project_for_role(
        &self,
        viewer_role: BearAgentRole,
        user_id: i32,
    ) -> Result<Option<WorkPlanProjection>, CustomError> {
        if !self.is_visible_to(viewer_role, user_id)? {
            return Ok(None);
        }
        let items = self.items.0.clone();
        let current_item = current_item(&items).cloned();
        Ok(Some(WorkPlanProjection {
            id: self.id,
            bear_id: self.bear_id,
            title: self.title.clone(),
            summary: self.summary.clone(),
            owner_role: self.owner_role.clone(),
            visibility: self.visibility.clone(),
            status: self.status.clone(),
            version: self.version,
            items,
            current_item,
            source_conversation_id: self.source_conversation_id.clone(),
            source_acp_session_id: self.source_acp_session_id.clone(),
            handoff_intent_path: self.handoff_intent_path.clone(),
            handoff_task_id: self.handoff_task_id.clone(),
            created_at: self.created_at,
            updated_at: self.updated_at,
        }))
    }
}

fn current_item(items: &[WorkPlanItem]) -> Option<&WorkPlanItem> {
    items
        .iter()
        .find(|item| item.status == WorkPlanItemStatus::InProgress)
        .or_else(|| {
            items
                .iter()
                .find(|item| item.status == WorkPlanItemStatus::Blocked)
        })
        .or_else(|| {
            items
                .iter()
                .find(|item| item.status == WorkPlanItemStatus::Pending)
        })
}

pub fn validate_work_plan_update(update: &WorkPlanUpdate) -> Result<(), WorkPlanValidationError> {
    if update.title.trim().is_empty() {
        return Err(WorkPlanValidationError::EmptyTitle);
    }

    validate_work_plan_items(&update.items)
}

pub async fn create_or_update_work_plan(
    pool: &PgPool,
    params: WorkPlanUpsert,
) -> Result<BearWorkPlanRow, CustomError> {
    validate_work_plan_update(&params.update)?;
    if !role_can_update_work_plan(params.owner_role) {
        return Err(CustomError::Authorization(format!(
            "the `{}` role cannot update work plans",
            params.owner_role
        )));
    }

    let mut tx = pool.begin().await?;
    let existing_id = if let Some(plan_id) = params.plan_id {
        Some(plan_id)
    } else {
        find_existing_plan_id(
            &mut tx,
            params.bear_id,
            params.owner_role,
            params.source_conversation_id.as_deref(),
            params.source_acp_session_id.as_deref(),
        )
        .await?
    };

    let (row, event_type) = if let Some(plan_id) = existing_id {
        let row = update_existing_plan(&mut tx, plan_id, &params).await?;
        (row, "updated")
    } else {
        let row = insert_new_plan(&mut tx, &params).await?;
        (row, "created")
    };

    append_event(
        &mut tx,
        row.id,
        row.bear_id,
        Some(params.owner_role),
        params.owner_agent_id.as_deref(),
        params.created_by_user_id,
        event_type,
        json!({
            "version": row.version,
            "status": row.status,
            "visibility": row.visibility,
        }),
    )
    .await?;
    tx.commit().await?;
    Ok(row)
}

async fn find_existing_plan_id(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    bear_id: Uuid,
    owner_role: BearAgentRole,
    source_conversation_id: Option<&str>,
    source_acp_session_id: Option<&str>,
) -> Result<Option<Uuid>, CustomError> {
    let row: Option<(Uuid,)> = sqlx::query_as(
        r#"
        SELECT id
        FROM bear_work_plans
        WHERE bear_id = $1
          AND owner_role = $2
          AND COALESCE(source_conversation_id, '') = COALESCE($3, '')
          AND COALESCE(source_acp_session_id, '') = COALESCE($4, '')
          AND status <> 'archived'
        ORDER BY updated_at DESC
        LIMIT 1
        "#,
    )
    .bind(bear_id)
    .bind(owner_role.as_str())
    .bind(source_conversation_id)
    .bind(source_acp_session_id)
    .fetch_optional(&mut **tx)
    .await?;
    Ok(row.map(|r| r.0))
}

async fn insert_new_plan(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    params: &WorkPlanUpsert,
) -> Result<BearWorkPlanRow, CustomError> {
    sqlx::query_as::<_, BearWorkPlanRow>(
        r#"
        INSERT INTO bear_work_plans (
            bear_id, title, summary, owner_role, owner_agent_id, created_by_user_id,
            source_conversation_id, source_acp_session_id, source_channel, workspace_context,
            visibility, status, items
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9::jsonb, $10::jsonb, $11, $12, $13::jsonb)
        RETURNING id, bear_id, title, summary, owner_role, owner_agent_id, created_by_user_id,
                  source_conversation_id, source_acp_session_id, source_channel, workspace_context,
                  visibility, status, items, version, handoff_intent_path, handoff_task_id,
                  archived_at, created_at, updated_at
        "#,
    )
    .bind(params.bear_id)
    .bind(params.update.title.trim())
    .bind(params.update.summary.trim())
    .bind(params.owner_role.as_str())
    .bind(params.owner_agent_id.as_deref())
    .bind(params.created_by_user_id)
    .bind(params.source_conversation_id.as_deref())
    .bind(params.source_acp_session_id.as_deref())
    .bind(&params.source_channel)
    .bind(&params.update.workspace_context)
    .bind(params.update.visibility.as_str())
    .bind(params.update.status.as_str())
    .bind(serde_json::to_value(&params.update.items)?)
    .fetch_one(&mut **tx)
    .await
    .map_err(Into::into)
}

async fn update_existing_plan(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    plan_id: Uuid,
    params: &WorkPlanUpsert,
) -> Result<BearWorkPlanRow, CustomError> {
    let row = sqlx::query_as::<_, BearWorkPlanRow>(
        r#"
        UPDATE bear_work_plans
        SET title = $4,
            summary = $5,
            visibility = $6,
            status = $7,
            items = $8::jsonb,
            workspace_context = $9::jsonb,
            owner_agent_id = COALESCE($10, owner_agent_id),
            version = version + 1,
            archived_at = CASE WHEN $7 = 'archived' THEN COALESCE(archived_at, NOW()) ELSE archived_at END,
            updated_at = NOW()
        WHERE id = $1
          AND bear_id = $2
          AND owner_role = $3
          AND ($11::integer IS NULL OR version = $11)
        RETURNING id, bear_id, title, summary, owner_role, owner_agent_id, created_by_user_id,
                  source_conversation_id, source_acp_session_id, source_channel, workspace_context,
                  visibility, status, items, version, handoff_intent_path, handoff_task_id,
                  archived_at, created_at, updated_at
        "#,
    )
    .bind(plan_id)
    .bind(params.bear_id)
    .bind(params.owner_role.as_str())
    .bind(params.update.title.trim())
    .bind(params.update.summary.trim())
    .bind(params.update.visibility.as_str())
    .bind(params.update.status.as_str())
    .bind(serde_json::to_value(&params.update.items)?)
    .bind(&params.update.workspace_context)
    .bind(params.owner_agent_id.as_deref())
    .bind(params.expected_version)
    .fetch_optional(&mut **tx)
    .await?;
    row.ok_or_else(|| {
        CustomError::ValidationError(
            "work plan was not found, is owned by another role, or version did not match"
                .to_string(),
        )
    })
}

async fn append_event(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    plan_id: Uuid,
    bear_id: Uuid,
    actor_role: Option<BearAgentRole>,
    actor_agent_id: Option<&str>,
    actor_user_id: Option<i32>,
    event_type: &str,
    event_payload: Value,
) -> Result<(), CustomError> {
    sqlx::query(
        r#"
        INSERT INTO bear_work_plan_events (
            plan_id, bear_id, actor_role, actor_agent_id, actor_user_id, event_type, event_payload
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7::jsonb)
        "#,
    )
    .bind(plan_id)
    .bind(bear_id)
    .bind(actor_role.map(|role| role.as_str()))
    .bind(actor_agent_id)
    .bind(actor_user_id)
    .bind(event_type)
    .bind(event_payload)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

pub async fn list_visible_work_plans(
    pool: &PgPool,
    bear_id: Uuid,
    viewer_role: BearAgentRole,
    user_id: i32,
    filter: WorkPlanListFilter,
) -> Result<Vec<WorkPlanProjection>, CustomError> {
    let rows = sqlx::query_as::<_, BearWorkPlanRow>(
        r#"
        SELECT id, bear_id, title, summary, owner_role, owner_agent_id, created_by_user_id,
               source_conversation_id, source_acp_session_id, source_channel, workspace_context,
               visibility, status, items, version, handoff_intent_path, handoff_task_id,
               archived_at, created_at, updated_at
        FROM bear_work_plans
        WHERE bear_id = $1
        ORDER BY updated_at DESC
        LIMIT 50
        "#,
    )
    .bind(bear_id)
    .fetch_all(pool)
    .await?;

    let mut visible = Vec::new();
    for row in rows {
        if !filter.include_archived && row.status == WorkPlanStatus::Archived.as_str() {
            continue;
        }
        if let Some(owner_role) = filter.owner_role {
            if row.owner_role != owner_role.as_str() {
                continue;
            }
        }
        if let Some(statuses) = filter.statuses.as_ref() {
            if !statuses.iter().any(|status| row.status == status.as_str()) {
                continue;
            }
        }
        if let Some(projected) = row.project_for_role(viewer_role, user_id)? {
            visible.push(projected);
        }
    }
    Ok(visible)
}

pub async fn get_visible_work_plan(
    pool: &PgPool,
    bear_id: Uuid,
    viewer_role: BearAgentRole,
    user_id: i32,
    lookup: WorkPlanLookup,
) -> Result<Option<WorkPlanProjection>, CustomError> {
    let row = if let Some(plan_id) = lookup.plan_id {
        sqlx::query_as::<_, BearWorkPlanRow>(SELECT_WORK_PLAN_BY_ID)
            .bind(bear_id)
            .bind(plan_id)
            .fetch_optional(pool)
            .await?
    } else if let Some(source_acp_session_id) = lookup.source_acp_session_id {
        sqlx::query_as::<_, BearWorkPlanRow>(SELECT_WORK_PLAN_BY_ACP_SESSION)
            .bind(bear_id)
            .bind(source_acp_session_id)
            .fetch_optional(pool)
            .await?
    } else if let Some(source_conversation_id) = lookup.source_conversation_id {
        sqlx::query_as::<_, BearWorkPlanRow>(SELECT_WORK_PLAN_BY_CONVERSATION)
            .bind(bear_id)
            .bind(source_conversation_id)
            .fetch_optional(pool)
            .await?
    } else {
        None
    };

    match row {
        Some(row) => row.project_for_role(viewer_role, user_id),
        None => Ok(None),
    }
}

pub fn render_workboard_prompt_context(plans: &[WorkPlanProjection]) -> String {
    if plans.is_empty() {
        return String::new();
    }

    let mut out = String::from(
        "\n\n<system-reminder>\nDen workboard context for this Bear. Use `den.work_plan.update` to keep live plan/status current. Use `den.work_plan.request_handoff` when channel work should become a durable task intent.\n",
    );
    for plan in plans.iter().take(5) {
        out.push_str(&format!(
            "- plan_id={} owner={} status={} visibility={} title={}",
            plan.id, plan.owner_role, plan.status, plan.visibility, plan.title
        ));
        if let Some(current) = plan.current_item.as_ref() {
            out.push_str(&format!(
                " current_item={} ({})",
                current.title,
                current.status.as_str()
            ));
        }
        if !plan.summary.trim().is_empty() {
            out.push_str(&format!(" summary={}", plan.summary.trim()));
        }
        out.push('\n');
    }
    out.push_str("</system-reminder>");
    out
}

const SELECT_WORK_PLAN_BY_ID: &str = r#"
    SELECT id, bear_id, title, summary, owner_role, owner_agent_id, created_by_user_id,
           source_conversation_id, source_acp_session_id, source_channel, workspace_context,
           visibility, status, items, version, handoff_intent_path, handoff_task_id,
           archived_at, created_at, updated_at
    FROM bear_work_plans
    WHERE bear_id = $1 AND id = $2
"#;

const SELECT_WORK_PLAN_BY_ACP_SESSION: &str = r#"
    SELECT id, bear_id, title, summary, owner_role, owner_agent_id, created_by_user_id,
           source_conversation_id, source_acp_session_id, source_channel, workspace_context,
           visibility, status, items, version, handoff_intent_path, handoff_task_id,
           archived_at, created_at, updated_at
    FROM bear_work_plans
    WHERE bear_id = $1 AND source_acp_session_id = $2
    ORDER BY updated_at DESC
    LIMIT 1
"#;

const SELECT_WORK_PLAN_BY_CONVERSATION: &str = r#"
    SELECT id, bear_id, title, summary, owner_role, owner_agent_id, created_by_user_id,
           source_conversation_id, source_acp_session_id, source_channel, workspace_context,
           visibility, status, items, version, handoff_intent_path, handoff_task_id,
           archived_at, created_at, updated_at
    FROM bear_work_plans
    WHERE bear_id = $1 AND source_conversation_id = $2
    ORDER BY updated_at DESC
    LIMIT 1
"#;

pub fn validate_work_plan_items(items: &[WorkPlanItem]) -> Result<(), WorkPlanValidationError> {
    let mut in_progress_count = 0;
    for item in items {
        if item.id.trim().is_empty() {
            return Err(WorkPlanValidationError::EmptyItemId);
        }
        if item.title.trim().is_empty() {
            return Err(WorkPlanValidationError::EmptyItemTitle {
                item_id: item.id.clone(),
            });
        }
        if item.status == WorkPlanItemStatus::InProgress {
            in_progress_count += 1;
        }
        if item.status == WorkPlanItemStatus::Blocked
            && item
                .blocked_reason
                .as_deref()
                .map(|reason| reason.trim().is_empty())
                .unwrap_or(true)
        {
            return Err(WorkPlanValidationError::BlockedItemMissingReason {
                item_id: item.id.clone(),
            });
        }
    }

    if in_progress_count > 1 {
        return Err(WorkPlanValidationError::MultipleInProgressItems);
    }
    Ok(())
}

pub fn role_can_update_work_plan(role: BearAgentRole) -> bool {
    matches!(
        role,
        BearAgentRole::Talk | BearAgentRole::Pair | BearAgentRole::Work
    )
}

pub fn role_can_request_work_handoff(role: BearAgentRole) -> bool {
    matches!(role, BearAgentRole::Talk | BearAgentRole::Pair)
}

pub fn role_can_read_work_plan(
    viewer_role: BearAgentRole,
    owner_role: BearAgentRole,
    visibility: WorkPlanVisibility,
    same_user: bool,
) -> bool {
    match visibility {
        WorkPlanVisibility::PrivateToRole => viewer_role == owner_role,
        WorkPlanVisibility::SameUser => same_user || viewer_role == owner_role,
        WorkPlanVisibility::BearVisible => true,
        WorkPlanVisibility::HandoffRequested => {
            matches!(viewer_role, BearAgentRole::Curate) || viewer_role == owner_role
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(id: &str, status: WorkPlanItemStatus) -> WorkPlanItem {
        WorkPlanItem {
            id: id.to_string(),
            title: format!("Item {id}"),
            summary: None,
            status,
            blocked_reason: None,
            source_refs: Vec::new(),
        }
    }

    #[test]
    fn validates_single_in_progress_item() {
        let items = vec![
            item("one", WorkPlanItemStatus::Completed),
            item("two", WorkPlanItemStatus::InProgress),
            item("three", WorkPlanItemStatus::Pending),
        ];
        assert!(validate_work_plan_items(&items).is_ok());
    }

    #[test]
    fn rejects_multiple_in_progress_items() {
        let items = vec![
            item("one", WorkPlanItemStatus::InProgress),
            item("two", WorkPlanItemStatus::InProgress),
        ];
        assert_eq!(
            validate_work_plan_items(&items),
            Err(WorkPlanValidationError::MultipleInProgressItems)
        );
    }

    #[test]
    fn blocked_items_need_reason() {
        let items = vec![item("one", WorkPlanItemStatus::Blocked)];
        assert_eq!(
            validate_work_plan_items(&items),
            Err(WorkPlanValidationError::BlockedItemMissingReason {
                item_id: "one".to_string()
            })
        );
    }

    #[test]
    fn visibility_preserves_role_boundaries() {
        assert!(role_can_read_work_plan(
            BearAgentRole::Pair,
            BearAgentRole::Pair,
            WorkPlanVisibility::PrivateToRole,
            false
        ));
        assert!(!role_can_read_work_plan(
            BearAgentRole::Talk,
            BearAgentRole::Pair,
            WorkPlanVisibility::PrivateToRole,
            false
        ));
        assert!(role_can_read_work_plan(
            BearAgentRole::Talk,
            BearAgentRole::Pair,
            WorkPlanVisibility::BearVisible,
            false
        ));
        assert!(role_can_read_work_plan(
            BearAgentRole::Curate,
            BearAgentRole::Pair,
            WorkPlanVisibility::HandoffRequested,
            false
        ));
        assert!(!role_can_read_work_plan(
            BearAgentRole::Work,
            BearAgentRole::Pair,
            WorkPlanVisibility::HandoffRequested,
            false
        ));
    }

    #[test]
    fn only_channel_roles_request_handoff() {
        assert!(role_can_request_work_handoff(BearAgentRole::Talk));
        assert!(role_can_request_work_handoff(BearAgentRole::Pair));
        assert!(!role_can_request_work_handoff(BearAgentRole::Work));
        assert!(!role_can_request_work_handoff(BearAgentRole::Curate));
    }

    #[test]
    fn renders_compact_prompt_context_without_raw_workspace_context() {
        let plan = WorkPlanProjection {
            id: Uuid::parse_str("00000000-0000-0000-0000-000000000123").unwrap(),
            bear_id: Uuid::parse_str("00000000-0000-0000-0000-000000000456").unwrap(),
            title: "Build task system".to_string(),
            summary: "Keep status current".to_string(),
            owner_role: "pair".to_string(),
            visibility: "bear_visible".to_string(),
            status: "active".to_string(),
            version: 1,
            items: vec![item("one", WorkPlanItemStatus::InProgress)],
            current_item: Some(item("one", WorkPlanItemStatus::InProgress)),
            source_conversation_id: None,
            source_acp_session_id: None,
            handoff_intent_path: None,
            handoff_task_id: None,
            created_at: OffsetDateTime::UNIX_EPOCH,
            updated_at: OffsetDateTime::UNIX_EPOCH,
        };

        let rendered = render_workboard_prompt_context(&[plan]);
        assert!(rendered.contains("Den workboard context"));
        assert!(rendered.contains("den.work_plan.update"));
        assert!(rendered.contains("Build task system"));
        assert!(rendered.contains("Item one"));
        assert!(!rendered.contains("workspace_context"));
    }
}
