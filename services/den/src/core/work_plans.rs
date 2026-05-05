use serde::{Deserialize, Serialize};
use sqlx::{types::Json, FromRow};
use std::fmt;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::core::bears::BearAgentRole;

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
                write!(f, "blocked work plan item `{item_id}` must include blocked_reason")
            }
        }
    }
}

impl std::error::Error for WorkPlanValidationError {}

pub fn validate_work_plan_update(update: &WorkPlanUpdate) -> Result<(), WorkPlanValidationError> {
    if update.title.trim().is_empty() {
        return Err(WorkPlanValidationError::EmptyTitle);
    }

    validate_work_plan_items(&update.items)
}

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
    matches!(role, BearAgentRole::Talk | BearAgentRole::Pair | BearAgentRole::Work)
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
}
