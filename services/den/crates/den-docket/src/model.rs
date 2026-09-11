//! Docket domain types and compatibility projection shapes.
//!
//! ADR-0034 relational jobs/tasks are the canonical storage model. The
//! `TaskList*` and task-list projection types remain as API/projection shapes
//! for existing tool contracts; they should not imply an active
//! `bear_task_lists` persistence path.

use serde::{Deserialize, Serialize};
use sqlx::types::Json;
use sqlx::FromRow;
use std::fmt::{self, Write as _};
use time::OffsetDateTime;
use uuid::Uuid;

use den_core::{BearProfile, DenError};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskListVisibility {
    PrivateToProfile,
    SameUser,
    BearVisible,
    HandoffRequested,
}

impl TaskListVisibility {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::PrivateToProfile => "private_to_profile",
            Self::SameUser => "same_user",
            Self::BearVisible => "bear_visible",
            Self::HandoffRequested => "handoff_requested",
        }
    }

    pub fn parse(value: &str) -> Result<Self, DenError> {
        match value.trim() {
            "private_to_profile" => Ok(Self::PrivateToProfile),
            "same_user" => Ok(Self::SameUser),
            "bear_visible" => Ok(Self::BearVisible),
            "handoff_requested" => Ok(Self::HandoffRequested),
            other => Err(DenError::Parsing(format!(
                "unknown task-list visibility: {other}"
            ))),
        }
    }
}

impl fmt::Display for TaskListVisibility {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskListStatus {
    Active,
    Blocked,
    Completed,
    Cancelled,
    Archived,
}

impl TaskListStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Blocked => "blocked",
            Self::Completed => "completed",
            Self::Cancelled => "cancelled",
            Self::Archived => "archived",
        }
    }

    pub fn parse(value: &str) -> Result<Self, DenError> {
        match value.trim() {
            "active" => Ok(Self::Active),
            "blocked" => Ok(Self::Blocked),
            "completed" => Ok(Self::Completed),
            "cancelled" => Ok(Self::Cancelled),
            "archived" => Ok(Self::Archived),
            other => Err(DenError::Parsing(format!(
                "unknown task-list status: {other}"
            ))),
        }
    }
}

impl fmt::Display for TaskListStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskListItemStatus {
    Pending,
    InProgress,
    Blocked,
    Completed,
    Cancelled,
}

impl TaskListItemStatus {
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

impl fmt::Display for TaskListItemStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskListUpdateItem {
    #[serde(default)]
    pub id: String,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    pub status: TaskListItemStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blocked_reason: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub source_refs: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct TaskListUpdate {
    pub title: String,
    pub summary: String,
    pub visibility: TaskListVisibility,
    pub status: TaskListStatus,
    pub items: Vec<TaskListUpdateItem>,
    pub workspace_context: serde_json::Value,
}

impl<'de> Deserialize<'de> for TaskListUpdate {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct RawTaskListUpdate {
            title: String,
            #[serde(default)]
            summary: String,
            visibility: TaskListVisibility,
            status: TaskListStatus,
            #[serde(default)]
            items: Vec<TaskListUpdateItem>,
            #[serde(default = "default_json_object")]
            workspace_context: serde_json::Value,
        }

        let mut raw = RawTaskListUpdate::deserialize(deserializer)?;
        normalize_task_list_item_ids(&mut raw.items);
        Ok(Self {
            title: raw.title,
            summary: raw.summary,
            visibility: raw.visibility,
            status: raw.status,
            items: raw.items,
            workspace_context: raw.workspace_context,
        })
    }
}

fn default_json_object() -> serde_json::Value {
    serde_json::json!({})
}

pub fn normalize_task_list_item_ids(items: &mut [TaskListUpdateItem]) {
    let mut generated_ids = std::collections::HashSet::new();
    for item in items {
        let trimmed = item.id.trim();
        if trimmed.is_empty() {
            let mut generated = generated_task_list_item_id(item, None);
            if !generated_ids.insert(generated.clone()) {
                let mut ordinal = 2_u32;
                loop {
                    generated = generated_task_list_item_id(item, Some(ordinal));
                    if generated_ids.insert(generated.clone()) {
                        break;
                    }
                    ordinal = ordinal.saturating_add(1);
                }
            }
            item.id = generated;
        } else if trimmed.len() != item.id.len() {
            item.id = trimmed.to_string();
        }
    }
}

fn generated_task_list_item_id(item: &TaskListUpdateItem, ordinal: Option<u32>) -> String {
    let mut seed = format!(
        "{}\n{}\n{}",
        item.title.trim(),
        item.summary.as_deref().unwrap_or("").trim(),
        item.status.as_str()
    );
    if let Some(ordinal) = ordinal {
        let _ = write!(seed, "\n{ordinal}");
    }
    let prefix = slug_prefix(&item.title).unwrap_or_else(|| "item".to_string());
    format!("{}_{:06x}", prefix, fnv1a64(seed.as_bytes()) & 0x00ff_ffff)
}

fn slug_prefix(value: &str) -> Option<String> {
    let mut slug = String::new();
    let mut last_was_separator = false;
    for ch in value.trim().chars().flat_map(char::to_lowercase) {
        if ch.is_ascii_alphanumeric() {
            slug.push(ch);
            last_was_separator = false;
        } else if !last_was_separator && !slug.is_empty() {
            slug.push('_');
            last_was_separator = true;
        }
        if slug.len() >= 32 {
            break;
        }
    }
    while slug.ends_with('_') {
        slug.pop();
    }
    (!slug.is_empty()).then_some(slug)
}

fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

#[derive(Debug, Clone, Serialize)]
pub struct TaskListLocalProjection {
    pub id: Uuid,
    pub bear_id: Uuid,
    pub title: String,
    pub summary: String,
    pub owner_profile: String,
    pub visibility: String,
    pub status: String,
    pub version: i32,
    pub items: Vec<TaskListUpdateItem>,
    pub current_item: Option<TaskListUpdateItem>,
    pub source_conversation_id: Option<String>,
    pub source_client_session_id: Option<String>,
    pub handoff_intent_path: Option<String>,
    pub handoff_task_id: Option<String>,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TaskListSyncState {
    LocalOnly,
    CheckedOut,
    Clean,
    Dirty,
    Syncing,
    Synced,
    Conflict,
    ReviewRequired,
}

impl TaskListSyncState {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::LocalOnly => "local_only",
            Self::CheckedOut => "checked_out",
            Self::Clean => "clean",
            Self::Dirty => "dirty",
            Self::Syncing => "syncing",
            Self::Synced => "synced",
            Self::Conflict => "conflict",
            Self::ReviewRequired => "review_required",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TaskListSourceRef {
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub docket_job_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub docket_task_id: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub refs: Vec<String>,
}

impl TaskListSourceRef {
    pub fn local(refs: Vec<String>) -> Self {
        Self {
            kind: "local".to_string(),
            docket_job_id: None,
            docket_task_id: None,
            refs,
        }
    }

    pub fn docket_job(job_id: String, refs: Vec<String>) -> Self {
        Self {
            kind: "docket_job".to_string(),
            docket_job_id: Some(job_id),
            docket_task_id: None,
            refs,
        }
    }

    pub fn docket_task(job_id: Option<String>, task_id: String, refs: Vec<String>) -> Self {
        Self {
            kind: "docket_task".to_string(),
            docket_job_id: job_id,
            docket_task_id: Some(task_id),
            refs,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskListItem {
    pub id: String,
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    pub status: TaskListItemStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub blocked_reason: Option<String>,
    pub source_ref: TaskListSourceRef,
    pub sync_state: TaskListSyncState,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskListProjection {
    pub id: Uuid,
    pub bear_id: Uuid,
    pub title: String,
    pub summary: String,
    pub owner_profile: String,
    pub visibility: String,
    pub status: String,
    pub version: i32,
    pub source_ref: TaskListSourceRef,
    pub items: Vec<TaskListItem>,
    pub current_item: Option<TaskListItem>,
    pub source_conversation_id: Option<String>,
    pub source_client_session_id: Option<String>,
    pub handoff_intent_path: Option<String>,
    pub handoff_task_id: Option<String>,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaskListValidationError {
    EmptyTitle,
    EmptyItemId,
    EmptyItemTitle { item_id: String },
    MultipleInProgressItems,
    BlockedItemMissingReason { item_id: String },
}

impl fmt::Display for TaskListValidationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyTitle => f.write_str("task-list title must not be empty"),
            Self::EmptyItemId => f.write_str("task-list item id must not be empty"),
            Self::EmptyItemTitle { item_id } => {
                write!(f, "task-list item `{item_id}` title must not be empty")
            }
            Self::MultipleInProgressItems => {
                f.write_str("task list may have at most one in_progress item")
            }
            Self::BlockedItemMissingReason { item_id } => {
                write!(
                    f,
                    "blocked task-list item `{item_id}` must include blocked_reason"
                )
            }
        }
    }
}

impl std::error::Error for TaskListValidationError {}

impl From<TaskListValidationError> for DenError {
    fn from(err: TaskListValidationError) -> Self {
        DenError::ValidationError(err.to_string())
    }
}

fn parse_docket_source_refs(refs: &[String]) -> Option<TaskListSourceRef> {
    let mut job_id = None;
    let mut task_id = None;
    for raw in refs {
        let value = raw.trim();
        if let Some(rest) = value.strip_prefix("docket_job:") {
            if !rest.trim().is_empty() {
                job_id = Some(rest.trim().to_string());
            }
        } else if let Some(rest) = value.strip_prefix("docket_task:") {
            if !rest.trim().is_empty() {
                task_id = Some(rest.trim().to_string());
            }
        }
    }
    task_id.map(|task_id| TaskListSourceRef::docket_task(job_id, task_id, refs.to_vec()))
}

pub fn task_list_item_from_update_item(item: &TaskListUpdateItem) -> TaskListItem {
    let source_ref = parse_docket_source_refs(&item.source_refs)
        .unwrap_or_else(|| TaskListSourceRef::local(item.source_refs.clone()));
    let sync_state = if source_ref.kind == "docket_task" {
        TaskListSyncState::CheckedOut
    } else {
        TaskListSyncState::LocalOnly
    };
    TaskListItem {
        id: item.id.clone(),
        title: item.title.clone(),
        summary: item.summary.clone(),
        status: item.status,
        blocked_reason: item.blocked_reason.clone(),
        source_ref,
        sync_state,
    }
}

pub fn task_list_projection_from_local(plan: &TaskListLocalProjection) -> TaskListProjection {
    let items = plan
        .items
        .iter()
        .map(task_list_item_from_update_item)
        .collect::<Vec<_>>();
    let current_item = plan
        .current_item
        .as_ref()
        .map(task_list_item_from_update_item);
    TaskListProjection {
        id: plan.id,
        bear_id: plan.bear_id,
        title: plan.title.clone(),
        summary: plan.summary.clone(),
        owner_profile: plan.owner_profile.clone(),
        visibility: plan.visibility.clone(),
        status: plan.status.clone(),
        version: plan.version,
        source_ref: TaskListSourceRef::local(vec![format!("task_list:{}", plan.id)]),
        items,
        current_item,
        source_conversation_id: plan.source_conversation_id.clone(),
        source_client_session_id: plan.source_client_session_id.clone(),
        handoff_intent_path: plan.handoff_intent_path.clone(),
        handoff_task_id: plan.handoff_task_id.clone(),
        created_at: plan.created_at,
        updated_at: plan.updated_at,
    }
}

impl TaskListLocalProjection {
    pub fn to_task_list_projection(&self) -> TaskListProjection {
        task_list_projection_from_local(self)
    }
}

#[derive(Debug, Clone)]
pub enum TaskListCheckoutSource {
    DocketJob {
        job_id: Uuid,
        parent_task_id: Option<Uuid>,
    },
    LocalProjection(Box<TaskListProjection>),
}

#[derive(Debug, Clone)]
pub struct TaskListCheckoutRequest {
    pub source: TaskListCheckoutSource,
    /// Pair session that explicitly claims durable tasks from this checkout.
    /// None preserves read-only projection behavior for non-Pair callers.
    pub pair_session_id: Option<Uuid>,
}

#[derive(Debug, Clone)]
pub struct TaskListSyncRequest {
    pub task_list: TaskListProjection,
}

#[derive(Debug, Clone, Serialize)]
pub struct TaskListSyncOutcome {
    pub task_list: TaskListProjection,
    pub applied: bool,
    pub review_required: bool,
    pub conflicts: Vec<String>,
    pub message: String,
}

impl TaskListSyncOutcome {
    pub fn applied(task_list: TaskListProjection, message: impl Into<String>) -> Self {
        Self {
            task_list,
            applied: true,
            review_required: false,
            conflicts: Vec::new(),
            message: message.into(),
        }
    }

    pub fn conflicts(
        task_list: TaskListProjection,
        conflicts: Vec<String>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            task_list,
            applied: false,
            review_required: false,
            conflicts,
            message: message.into(),
        }
    }

    pub fn review_required(task_list: TaskListProjection, message: impl Into<String>) -> Self {
        Self {
            task_list,
            applied: false,
            review_required: true,
            conflicts: Vec::new(),
            message: message.into(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct TaskListHandoffRequest {
    pub task_list: TaskListProjection,
    pub item_ids: Vec<String>,
    pub title: String,
    pub summary: String,
    pub requested_outcome: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct TaskListHandoffOutcome {
    pub accepted: bool,
    pub review_required: bool,
    pub message: String,
    pub task_list_id: Uuid,
    pub item_ids: Vec<String>,
}

impl TaskListHandoffOutcome {
    pub fn review_required(request: &TaskListHandoffRequest, message: impl Into<String>) -> Self {
        Self {
            accepted: true,
            review_required: true,
            message: message.into(),
            task_list_id: request.task_list.id,
            item_ids: request.item_ids.clone(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DocketJobStatus {
    Draft,
    Ready,
    Running,
    Stalled,
    Blocked,
    Completed,
    Cancelled,
    Archived,
}

impl DocketJobStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Draft => "draft",
            Self::Ready => "ready",
            Self::Running => "running",
            Self::Stalled => "stalled",
            Self::Blocked => "blocked",
            Self::Completed => "completed",
            Self::Cancelled => "cancelled",
            Self::Archived => "archived",
        }
    }
}

impl fmt::Display for DocketJobStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Explicit caller intent when a new job has the same normalized goal and
/// work surface as an active job. This is deliberately not a fuzzy goal
/// similarity heuristic.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum DocketJobOverlapResolution {
    #[default]
    Reject,
    Independent,
    Supersede,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DocketCommitPolicy {
    None,
    PerTask,
    PerJob,
}

impl DocketCommitPolicy {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::PerTask => "per_task",
            Self::PerJob => "per_job",
        }
    }

    /// Source-changing jobs publish one coherent result unless the caller
    /// explicitly chooses another policy.
    pub fn for_new_job(policy: Option<Self>) -> Self {
        policy.unwrap_or(Self::PerJob)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DocketCriterionKind {
    Narrative,
    Command,
    CheckRef,
}

impl DocketCriterionKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Narrative => "narrative",
            Self::Command => "command",
            Self::CheckRef => "check_ref",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DocketRunTrigger {
    Manual,
    Scheduled,
    Event,
}

impl DocketRunTrigger {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Manual => "manual",
            Self::Scheduled => "scheduled",
            Self::Event => "event",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DocketRunState {
    Dispatched,
    Running,
    Paused,
    Completed,
    Failed,
    Cancelled,
}

impl DocketRunState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Dispatched => "dispatched",
            Self::Running => "running",
            Self::Paused => "paused",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DocketTaskKind {
    Execution,
    Investigation,
    Decision,
}

impl DocketTaskKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Execution => "execution",
            Self::Investigation => "investigation",
            Self::Decision => "decision",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DocketTaskScope {
    Template,
    Run,
}

impl DocketTaskScope {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Template => "template",
            Self::Run => "run",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DocketTaskDifficulty {
    Trivial,
    Moderate,
    Hard,
    Unknown,
}

impl DocketTaskDifficulty {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Trivial => "trivial",
            Self::Moderate => "moderate",
            Self::Hard => "hard",
            Self::Unknown => "unknown",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DocketEffortHint {
    Low,
    Medium,
    High,
}

impl DocketEffortHint {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum RoutingStrategy {
    Inline,
    Scoped,
    Delegated,
    #[default]
    Auto,
}

impl RoutingStrategy {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Inline => "inline",
            Self::Scoped => "scoped",
            Self::Delegated => "delegated",
            Self::Auto => "auto",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResultRollupPolicy {
    SummaryToParent,
    None,
}

impl ResultRollupPolicy {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::SummaryToParent => "summary_to_parent",
            Self::None => "none",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DocketTaskStatus {
    Pending,
    Done,
    Blocked,
    Cancelled,
}

impl DocketTaskStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Done => "done",
            Self::Blocked => "blocked",
            Self::Cancelled => "cancelled",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DocketOutcomeDisposition {
    Completed,
    NoChange,
    Delegated,
    Blocked,
    Failed,
    Cancelled,
}

impl DocketOutcomeDisposition {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Completed => "completed",
            Self::NoChange => "no_change",
            Self::Delegated => "delegated",
            Self::Blocked => "blocked",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }

    pub fn is_valid_for(self, status: DocketTaskStatus) -> bool {
        matches!(
            (status, self),
            (
                DocketTaskStatus::Done,
                Self::Completed | Self::NoChange | Self::Delegated
            ) | (DocketTaskStatus::Blocked, Self::Blocked | Self::Failed)
                | (DocketTaskStatus::Cancelled, Self::Cancelled)
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DocketCriterionStatus {
    Unmet,
    Met,
    Waived,
}

impl DocketCriterionStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Unmet => "unmet",
            Self::Met => "met",
            Self::Waived => "waived",
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MutationPolicy {
    #[default]
    Required,
    Optional,
    Forbidden,
}

impl MutationPolicy {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Required => "required",
            Self::Optional => "optional",
            Self::Forbidden => "forbidden",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DocketJobSurfaceAssignmentInput {
    pub work_surface_id: Uuid,
    pub mutation_policy: MutationPolicy,
}

#[derive(Debug, Clone, FromRow, Serialize)]
pub struct DocketJobSurfaceAssignmentRow {
    pub job_id: Uuid,
    pub work_surface_id: Uuid,
    pub mutation_policy: String,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
}

#[derive(Debug, Clone, FromRow, Serialize)]
pub struct DocketJobRow {
    pub id: Uuid,
    pub bear_id: Uuid,
    pub created_by_user_id: i32,
    pub created_by_role: String,
    pub goal: String,
    /// Compatibility projection of the first dispatchable Git assignment.
    /// The canonical job-to-surface relationship is `job_work_surface_assignments`.
    pub work_surface_id: Option<Uuid>,
    pub commit_policy: Option<String>,
    /// Upstream branch this job's work runs publish to (set on first
    /// pushable dispatch when absent; default `den/job-<short-id>`).
    pub work_branch: Option<String>,
    /// Derived operational status; never persisted in `bear_jobs`.
    pub status: String,
    /// Explicit user lifecycle intent.
    pub lifecycle_intent: Option<String>,
    pub visibility: String,
    pub source_conversation_id: Option<String>,
    pub objective_kind: Option<String>,
    /// The active job explicitly replaced by this job, if any. This preserves
    /// ownership/history without guessing semantic equivalence from prose.
    pub supersedes_job_id: Option<Uuid>,
    pub current_run_id: Option<Uuid>,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
}

#[derive(Debug, Clone, FromRow, Serialize)]
pub struct DocketJobCriterionRow {
    pub id: Uuid,
    pub job_id: Uuid,
    pub kind: String,
    pub description: String,
    pub spec: Option<Json<serde_json::Value>>,
    pub sibling_order: i32,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
}

#[derive(Debug, Clone, FromRow, Serialize)]
pub struct DocketJobRunRow {
    pub id: Uuid,
    pub job_id: Uuid,
    pub trigger: String,
    pub schedule_ref: Option<String>,
    pub state: String,
    pub started_at: Option<OffsetDateTime>,
    pub finished_at: Option<OffsetDateTime>,
    pub outcome: Option<Json<serde_json::Value>>,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
}

#[derive(Debug, Clone, FromRow, Serialize)]
pub struct DocketTaskRow {
    pub id: Uuid,
    pub bear_id: Uuid,
    pub job_id: Option<Uuid>,
    pub parent_task_id: Option<Uuid>,
    pub sibling_order: i32,
    pub kind: String,
    pub scope: String,
    pub title: String,
    pub body: String,
    pub completion_criteria: Json<Vec<String>>,
    pub difficulty: Option<String>,
    pub effort_hint: Option<String>,
    pub routing_strategy: String,
    pub expected_context_size: Option<i32>,
    pub result_rollup_policy: Option<String>,
    pub created_by_role: String,
    pub created_by_user_id: Option<i32>,
    pub created_by_agent_id: Option<String>,
    pub created_in_run_id: Option<Uuid>,
    pub settled_by_entry_id: Option<Uuid>,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
}

#[derive(Debug, Clone, FromRow, Serialize)]
pub struct DocketTaskRunStateRow {
    pub run_id: Uuid,
    pub task_id: Uuid,
    pub status: String,
    pub result_refs: Option<Json<serde_json::Value>>,
    pub result_summary: Option<String>,
    pub started_at: Option<OffsetDateTime>,
    pub finished_at: Option<OffsetDateTime>,
    pub updated_at: OffsetDateTime,
}

#[derive(Debug, Clone, FromRow, Serialize)]
pub struct DocketCriterionStateRow {
    pub run_id: Uuid,
    pub criterion_id: Uuid,
    pub status: String,
    pub evaluated_at: Option<OffsetDateTime>,
    pub evidence: Option<Json<serde_json::Value>>,
    pub updated_at: OffsetDateTime,
}

#[derive(Debug, Clone, Serialize)]
pub struct DocketJobProjection {
    pub job: DocketJobRow,
    pub current_run: Option<DocketJobRunRow>,
    pub criteria: Vec<DocketJobCriterionRow>,
    pub criteria_states: Vec<DocketCriterionStateRow>,
    pub tasks: Vec<DocketTaskRow>,
    pub task_states: Vec<DocketTaskRunStateRow>,
    /// Tasks with a live execution lease. This is the sole execution authority.
    pub active_task_ids: Vec<Uuid>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct DocketCountByStatus {
    pub pending: usize,
    pub in_progress: usize,
    pub done: usize,
    pub blocked: usize,
    pub cancelled: usize,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct DocketCriteriaCountByStatus {
    pub unmet: usize,
    pub met: usize,
    pub waived: usize,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct DocketJobStatusReport {
    pub job_id: Uuid,
    pub run_id: Option<Uuid>,
    pub job_status: String,
    pub run_state: Option<String>,
    pub current_task_id: Option<Uuid>,
    pub current_task_title: Option<String>,
    pub task_counts: DocketCountByStatus,
    pub criteria_counts: DocketCriteriaCountByStatus,
    pub tasks_complete: bool,
    pub criteria_complete: bool,
    pub next_action: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct DocketJobCriterionInput {
    #[serde(default = "default_criterion_kind")]
    pub kind: DocketCriterionKind,
    pub description: String,
    #[serde(default)]
    pub spec: Option<serde_json::Value>,
    #[serde(default)]
    pub sibling_order: i32,
}

fn default_criterion_kind() -> DocketCriterionKind {
    DocketCriterionKind::Narrative
}

#[derive(Debug, Clone, Deserialize)]
pub struct DocketTaskInput {
    #[serde(default)]
    pub client_key: Option<String>,
    #[serde(default)]
    pub parent_client_key: Option<String>,
    #[serde(default)]
    pub parent_task_id: Option<Uuid>,
    #[serde(default)]
    pub sibling_order: Option<i32>,
    #[serde(default = "default_task_kind")]
    pub kind: DocketTaskKind,
    #[serde(default = "default_task_scope")]
    pub scope: DocketTaskScope,
    pub title: String,
    pub body: String,
    #[serde(default)]
    pub completion_criteria: Vec<String>,
    #[serde(default)]
    pub difficulty: Option<DocketTaskDifficulty>,
    #[serde(default)]
    pub effort_hint: Option<DocketEffortHint>,
    #[serde(default)]
    pub routing_strategy: RoutingStrategy,
    #[serde(default)]
    pub expected_context_size: Option<i32>,
    #[serde(default)]
    pub result_rollup_policy: Option<ResultRollupPolicy>,
}

fn default_task_kind() -> DocketTaskKind {
    DocketTaskKind::Execution
}

fn default_task_scope() -> DocketTaskScope {
    DocketTaskScope::Template
}

#[derive(Debug, Clone)]
pub struct DocketJobCreate {
    pub bear_id: Uuid,
    pub created_by_user_id: i32,
    pub created_by_role: String,
    pub goal: String,
    /// Managed surface id, after checking the bear's assignment.
    pub work_surface_id: Option<Uuid>,
    /// Canonical surface assignments. An empty list preserves the
    /// single-surface shorthand above, which becomes a required assignment.
    pub work_surface_assignments: Vec<DocketJobSurfaceAssignmentInput>,
    pub commit_policy: Option<DocketCommitPolicy>,
    /// Explicit upstream branch for work-run publishing; generated
    /// (`den/job-<short-id>`) on first pushable dispatch when absent.
    pub work_branch: Option<String>,
    pub visibility: TaskListVisibility,
    pub source_conversation_id: Option<String>,
    pub objective_kind: Option<String>,
    /// Explicitly replace this active job. Required only when
    /// `overlap_resolution` is `supersede`.
    pub supersedes_job_id: Option<Uuid>,
    pub overlap_resolution: DocketJobOverlapResolution,
    pub criteria: Vec<DocketJobCriterionInput>,
    pub tasks: Vec<DocketTaskInput>,
}

#[derive(Debug, Clone, Default)]
pub struct DocketJobListFilter {
    pub statuses: Option<Vec<DocketJobStatus>>,
    pub include_cancelled: bool,
    pub include_archived: bool,
    pub source_conversation_id: Option<String>,
    pub limit: i64,
}

#[derive(Debug, Clone)]
pub struct DocketJobUpdate {
    pub bear_id: Uuid,
    pub job_id: Uuid,
    pub actor_role: BearProfile,
    pub actor_user_id: Option<i32>,
    pub actor_agent_id: Option<String>,
    pub goal: Option<String>,
    /// Replace the managed surface binding. Work jobs cannot clear it.
    pub work_surface_id: Option<Option<Uuid>>,
    pub commit_policy: Option<Option<DocketCommitPolicy>>,
    /// Explicit publish branch; `Some(None)` clears it so the next pushable
    /// dispatch can generate the canonical `den/job-<id>` branch.
    pub work_branch: Option<Option<String>>,
    pub status: Option<DocketJobStatus>,
    pub visibility: Option<TaskListVisibility>,
}

#[derive(Debug, Clone)]
pub struct DocketCriterionStateUpdate {
    pub bear_id: Uuid,
    pub job_id: Uuid,
    pub run_id: Uuid,
    pub criterion_id: Uuid,
    pub status: DocketCriterionStatus,
    pub evidence: Option<serde_json::Value>,
    pub actor_role: BearProfile,
    pub actor_user_id: Option<i32>,
    pub actor_agent_id: Option<String>,
}

#[derive(Debug, Clone)]
pub struct DocketJobExecuteRequest {
    pub bear_id: Uuid,
    pub job_id: Uuid,
    pub actor_role: BearProfile,
    pub actor_user_id: Option<i32>,
    pub actor_agent_id: Option<String>,
    pub session_id: Option<String>,
    pub source_conversation_id: Option<String>,
    pub source_client_session_id: Option<String>,
}

/// Settles the task currently claimed by a Docket execution session and returns
/// the authoritative successor control result.
#[derive(Debug, Clone)]
pub struct DocketExecutionTaskSettlement {
    pub execution: DocketJobExecuteRequest,
    pub task_id: Uuid,
    pub status: DocketTaskStatus,
    pub outcome_disposition: Option<DocketOutcomeDisposition>,
    pub result_refs: Option<serde_json::Value>,
    pub result_summary: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DocketExecutionBindingKind {
    ClientSession,
    WorkAssignment,
}

impl DocketExecutionBindingKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::ClientSession => "client_session",
            Self::WorkAssignment => "work_assignment",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum DocketExecutionHostKind {
    #[serde(rename = "pair")]
    TurnRun,
    #[serde(rename = "work")]
    WorkRun,
}

impl DocketExecutionHostKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::TurnRun => "pair",
            Self::WorkRun => "work",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DocketFocusedExecutionBinding {
    pub kind: DocketExecutionBindingKind,
    pub id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DocketExecutionHost {
    pub kind: DocketExecutionHostKind,
    pub run_id: String,
}

#[derive(Debug, Clone)]
pub struct DocketFocusedExecutionAcquire {
    pub bear_id: Uuid,
    pub task_id: Uuid,
    pub binding: DocketFocusedExecutionBinding,
    pub host: DocketExecutionHost,
    pub acquisition_key: Uuid,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DocketExecutionAttemptState {
    Authorized,
    Running,
    Paused,
    AwaitingUser,
    Stopping,
    Settled,
    Released,
}

impl DocketExecutionAttemptState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Authorized => "authorized",
            Self::Running => "running",
            Self::Paused => "paused",
            Self::AwaitingUser => "awaiting_user",
            Self::Stopping => "stopping",
            Self::Settled => "settled",
            Self::Released => "released",
        }
    }

    pub fn is_live(self) -> bool {
        matches!(
            self,
            Self::Authorized | Self::Running | Self::Paused | Self::AwaitingUser | Self::Stopping
        )
    }

    pub fn try_from_storage(value: &str) -> Result<Self, DenError> {
        match value {
            "authorized" => Ok(Self::Authorized),
            "running" => Ok(Self::Running),
            "paused" => Ok(Self::Paused),
            "awaiting_user" => Ok(Self::AwaitingUser),
            "stopping" => Ok(Self::Stopping),
            "settled" => Ok(Self::Settled),
            "released" => Ok(Self::Released),
            _ => Err(DenError::ValidationError(
                "invalid execution attempt state".to_string(),
            )),
        }
    }
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct DocketExecutionAttemptRow {
    pub id: Uuid,
    pub bear_id: Uuid,
    pub task_id: Uuid,
    pub binding: DocketFocusedExecutionBinding,
    pub host: DocketExecutionHost,
    pub fence_epoch: i64,
    pub authorization_key: Uuid,
    pub state: DocketExecutionAttemptState,
    pub started_at: Option<OffsetDateTime>,
    pub paused_at: Option<OffsetDateTime>,
    pub settled_at: Option<OffsetDateTime>,
    pub released_at: Option<OffsetDateTime>,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
}

#[derive(Debug, Clone)]
pub struct DocketExecutionAttemptAuthorize {
    pub bear_id: Uuid,
    pub task_id: Uuid,
    pub binding: DocketFocusedExecutionBinding,
    pub host: DocketExecutionHost,
    pub authorization_key: Uuid,
}

#[derive(Debug, Clone)]
pub struct DocketExecutionAttemptStart {
    pub attempt_id: Uuid,
    pub fence_epoch: i64,
}

/// Exact safe-boundary revalidation for a running Work attempt. `boundary_key`
/// makes transport retries idempotent without granting a lease or selecting work.
#[derive(Debug, Clone)]
pub struct DocketWorkBoundaryCheck {
    pub bear_id: Uuid,
    pub attempt_id: Uuid,
    pub fence_epoch: i64,
    pub boundary_key: Uuid,
    /// Trusted runtime evidence observed at this boundary. Docket converts it
    /// into a durable checkpoint; executors do not decide whether to continue.
    pub signal: Option<DocketWorkBoundarySignal>,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DocketWorkBoundarySignal {
    ExcessiveExploration,
    RepeatedFailure,
    NearKo,
}

/// Releases an abandoned or superseded canonical attempt. The reconciler must
/// supply the exact fence and a stable recovery key so retries are idempotent.
#[derive(Debug, Clone)]
pub struct DocketExecutionAttemptRelease {
    pub attempt_id: Uuid,
    pub fence_epoch: i64,
    pub recovery_key: Uuid,
    pub recovery_reason: String,
}

/// A bounded outcome reported by the Pair-local loop. Docket owns the
/// resulting continuation decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DocketPairBoundedOutcome {
    Progress,
    AwaitingUser,
    Settled,
}

#[derive(Debug, Clone)]
pub struct DocketPairBoundedOutcomeReport {
    pub attempt_id: Uuid,
    pub fence_epoch: i64,
    pub outcome: DocketPairBoundedOutcome,
    /// Required when `outcome` is `AwaitingUser`; records the exact question
    /// that blocks continuation instead of treating reconnect as a resume.
    pub awaiting_user_question: Option<DocketPairAwaitingUserQuestion>,
}

#[derive(Debug, Clone)]
pub struct DocketPairAwaitingUserQuestion {
    pub question_key: Uuid,
    pub question_reference: String,
}

/// A trusted authenticated boundary records this explicit response before Pair
/// may start the same paused attempt again.
#[derive(Debug, Clone)]
pub struct DocketPairAwaitingUserResume {
    pub attempt_id: Uuid,
    pub fence_epoch: i64,
    pub question_key: Uuid,
    pub response_key: Uuid,
    pub response_reference: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DocketPairContinuationDecision {
    Continue,
    AwaitUser,
    Stop,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocketPairBoundedOutcomeDecision {
    pub attempt: DocketExecutionAttemptRow,
    pub decision: DocketPairContinuationDecision,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub(super) struct DocketExecutionAttemptDbRow {
    id: Uuid,
    bear_id: Uuid,
    task_id: Uuid,
    binding_kind: String,
    binding_id: String,
    host_kind: String,
    host_run_id: String,
    fence_epoch: i64,
    authorization_key: Uuid,
    state: String,
    started_at: Option<OffsetDateTime>,
    paused_at: Option<OffsetDateTime>,
    settled_at: Option<OffsetDateTime>,
    released_at: Option<OffsetDateTime>,
    created_at: OffsetDateTime,
    updated_at: OffsetDateTime,
}

impl TryFrom<DocketExecutionAttemptDbRow> for DocketExecutionAttemptRow {
    type Error = DenError;

    fn try_from(row: DocketExecutionAttemptDbRow) -> Result<Self, Self::Error> {
        let binding = DocketFocusedExecutionBinding {
            kind: match row.binding_kind.as_str() {
                "client_session" => DocketExecutionBindingKind::ClientSession,
                "work_assignment" => DocketExecutionBindingKind::WorkAssignment,
                _ => {
                    return Err(DenError::ValidationError(
                        "invalid execution binding kind".to_string(),
                    ))
                }
            },
            id: row.binding_id,
        };
        let host = DocketExecutionHost {
            kind: match row.host_kind.as_str() {
                "pair" => DocketExecutionHostKind::TurnRun,
                "work" => DocketExecutionHostKind::WorkRun,
                _ => {
                    return Err(DenError::ValidationError(
                        "invalid execution host kind".to_string(),
                    ))
                }
            },
            run_id: row.host_run_id,
        };
        Ok(Self {
            id: row.id,
            bear_id: row.bear_id,
            task_id: row.task_id,
            binding,
            host,
            fence_epoch: row.fence_epoch,
            authorization_key: row.authorization_key,
            state: DocketExecutionAttemptState::try_from_storage(&row.state)?,
            started_at: row.started_at,
            paused_at: row.paused_at,
            settled_at: row.settled_at,
            released_at: row.released_at,
            created_at: row.created_at,
            updated_at: row.updated_at,
        })
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct DocketExecutionTaskControl {
    /// The task the scheduler would select next from the plan.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub selected_task_id: Option<Uuid>,
    /// The task persisted as execution-session focus.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub focused_task_id: Option<Uuid>,
    /// The task currently protected from concurrent advancement by execution.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub claimed_task_id: Option<Uuid>,
    /// The task safe to show as the user's current work.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current_task_id: Option<Uuid>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DocketExecutionNextAction {
    WorkCurrentTask,
    JobCompleted,
    ReconcileExecution,
    RecoverBlockedRun,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DocketExecutionReason {
    ActiveTaskIsStale,
    CheckpointRequired,
    NoActionableTask,
    JobComplete,
    JobBlocked,
}

impl std::fmt::Display for DocketExecutionReason {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::ActiveTaskIsStale => "active_task_is_stale",
            Self::CheckpointRequired => "checkpoint_required",
            Self::NoActionableTask => "no_actionable_task",
            Self::JobComplete => "job_complete",
            Self::JobBlocked => "job_blocked",
        })
    }
}

impl DocketExecutionReason {
    /// The executor action is part of Docket's scheduler decision, not a
    /// runtime retry heuristic.
    pub fn disposition(&self) -> DocketExecutionDisposition {
        match self {
            Self::ActiveTaskIsStale => DocketExecutionDisposition::Reconcile,
            Self::CheckpointRequired => DocketExecutionDisposition::RequireCheckpoint,
            Self::NoActionableTask | Self::JobComplete | Self::JobBlocked => {
                DocketExecutionDisposition::Stop
            }
        }
    }
}

/// The durable execution binding that authorizes a task dispatch. Consumers
/// treat this as opaque scheduler authority rather than rebuilding task-tree
/// eligibility from projections.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DocketExecutionBinding {
    PairSession { job_run_id: Uuid },
    WorkRun { work_run_id: Uuid, job_run_id: Uuid },
}

/// Safe executor action after Docket declines authorization. This is never a
/// model-retry instruction: task-tree recovery remains Docket-owned.
#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DocketExecutionDisposition {
    Reconcile,
    Stop,
    RequireIntervention,
    RequireCheckpoint,
}

/// A durable instruction requiring evidence before Docket considers another
/// dispatch for an execution-attempt fence.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DocketCheckpointDirectiveState {
    Pending,
    Acknowledged,
    Superseded,
}

impl DocketCheckpointDirectiveState {
    fn parse(value: &str) -> Result<Self, DenError> {
        match value {
            "pending" => Ok(Self::Pending),
            "acknowledged" => Ok(Self::Acknowledged),
            "superseded" => Ok(Self::Superseded),
            _ => Err(DenError::ValidationError(
                "invalid checkpoint directive state".to_string(),
            )),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct DocketCheckpointDirectiveRow {
    pub id: Uuid,
    pub execution_attempt_id: Uuid,
    pub fence_epoch: i64,
    pub state: DocketCheckpointDirectiveState,
    pub acknowledged_artifact_ref: Option<String>,
    pub created_at: OffsetDateTime,
    pub acknowledged_at: Option<OffsetDateTime>,
    pub superseded_at: Option<OffsetDateTime>,
}

/// Exact fenced evidence acknowledgement for one pending Work directive.
#[derive(Debug, Clone)]
pub struct DocketCheckpointDirectiveAcknowledge {
    pub bear_id: Uuid,
    pub directive_id: Uuid,
    pub execution_attempt_id: Uuid,
    pub fence_epoch: i64,
    pub artifact_ref: String,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub(super) struct DocketCheckpointDirectiveDbRow {
    pub id: Uuid,
    pub execution_attempt_id: Uuid,
    pub fence_epoch: i64,
    pub state: String,
    pub acknowledged_artifact_ref: Option<String>,
    pub created_at: OffsetDateTime,
    pub acknowledged_at: Option<OffsetDateTime>,
    pub superseded_at: Option<OffsetDateTime>,
}

impl TryFrom<DocketCheckpointDirectiveDbRow> for DocketCheckpointDirectiveRow {
    type Error = DenError;

    fn try_from(row: DocketCheckpointDirectiveDbRow) -> Result<Self, Self::Error> {
        Ok(Self {
            id: row.id,
            execution_attempt_id: row.execution_attempt_id,
            fence_epoch: row.fence_epoch,
            state: DocketCheckpointDirectiveState::parse(&row.state)?,
            acknowledged_artifact_ref: row.acknowledged_artifact_ref,
            created_at: row.created_at,
            acknowledged_at: row.acknowledged_at,
            superseded_at: row.superseded_at,
        })
    }
}

/// Docket-owned disposition for a live, already-authorized binding. This is
/// deliberately narrower than pre-dispatch gate dispositions: runtime delivery
/// is added separately and cannot create scheduler authority.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DocketSchedulerObservationDisposition {
    Reconcile,
    Stop,
}

impl std::fmt::Display for DocketSchedulerObservationDisposition {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Reconcile => "reconcile",
            Self::Stop => "stop",
        })
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DocketSchedulerObservationDeliveryState {
    Pending,
    Delivered,
}

#[derive(Debug, Clone, sqlx::FromRow, Serialize)]
pub struct DocketSchedulerObservationRow {
    pub id: Uuid,
    pub execution_session_id: Uuid,
    pub job_id: Uuid,
    pub run_id: Uuid,
    pub task_id: Option<Uuid>,
    pub reason: DocketExecutionReason,
    pub occurrence: i32,
    pub disposition: DocketSchedulerObservationDisposition,
    pub delivery_state: DocketSchedulerObservationDeliveryState,
    pub delivered_at: Option<OffsetDateTime>,
    pub created_at: OffsetDateTime,
}

#[derive(Debug, Clone)]
pub struct DocketSchedulerObservationEnqueue {
    pub execution_session_id: Uuid,
    pub task_id: Option<Uuid>,
    pub reason: DocketExecutionReason,
    pub disposition: DocketSchedulerObservationDisposition,
}

/// Authoritative scheduler decision for whether the selected execution session
/// may work a task. Consumers must use this decision rather than re-evaluating
/// task-tree eligibility from projections.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum DocketExecutionGate {
    Allowed {
        task_id: Uuid,
        binding: DocketExecutionBinding,
    },
    Rejected {
        reason: DocketExecutionReason,
        disposition: DocketExecutionDisposition,
    },
}

/// Authoritative execution control returned by scheduler operations.
///
/// `selected`, `focused`, `claimed`, and `current` deliberately remain
/// separate fields: callers must not infer ownership from a display
/// projection. A missing reason means normal actionable execution.
#[derive(Debug, Clone, Serialize)]
pub struct DocketExecutionControl {
    pub run_id: Uuid,
    pub run_state: String,
    pub task: DocketExecutionTaskControl,
    pub next_action: DocketExecutionNextAction,
    pub retryable: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<DocketExecutionReason>,
}

/// The only result that may be rendered as active Docket execution control.
/// Bindings submit observations; Docket publishes this correlated, durable result.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DocketExecutionControlState {
    Requested,
    BoundaryObserved,
    ContinuationEstablished,
    TerminalSettled,
    NotEstablished,
    Blocked,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DocketExecutionControlReference {
    pub kind: String,
    pub id: String,
    pub persisted: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DocketExecutionControlResult {
    pub attempt_id: Uuid,
    pub state: DocketExecutionControlState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub job_id: Option<Uuid>,
    pub job_run_id: Uuid,
    pub task_id: Uuid,
    pub binding: DocketExecutionControlReference,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub boundary: Option<DocketExecutionControlReference>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub continuation: Option<DocketExecutionControlReference>,
    pub authority_verified: bool,
}

impl DocketExecutionControlResult {
    /// Construct the sole active-control state. Keeping this constructor narrow
    /// prevents callers from accidentally promoting transient runtime activity.
    pub fn continuation_established(
        attempt_id: Uuid,
        job_id: Option<Uuid>,
        job_run_id: Uuid,
        task_id: Uuid,
        binding: DocketExecutionControlReference,
        boundary: DocketExecutionControlReference,
        continuation: DocketExecutionControlReference,
    ) -> Self {
        assert!(
            boundary.persisted,
            "established control needs a persisted boundary"
        );
        assert!(
            continuation.persisted,
            "established control needs a persisted continuation"
        );
        Self {
            attempt_id,
            state: DocketExecutionControlState::ContinuationEstablished,
            reason: None,
            job_id,
            job_run_id,
            task_id,
            binding,
            boundary: Some(boundary),
            continuation: Some(continuation),
            authority_verified: true,
        }
    }

    pub fn not_established(
        attempt_id: Uuid,
        job_id: Option<Uuid>,
        job_run_id: Uuid,
        task_id: Uuid,
        binding: DocketExecutionControlReference,
        reason: impl Into<String>,
        boundary: Option<DocketExecutionControlReference>,
        authority_verified: bool,
    ) -> Self {
        Self {
            attempt_id,
            state: DocketExecutionControlState::NotEstablished,
            reason: Some(reason.into()),
            job_id,
            job_run_id,
            task_id,
            binding,
            boundary,
            continuation: None,
            authority_verified,
        }
    }

    pub fn is_established(&self) -> bool {
        self.state == DocketExecutionControlState::ContinuationEstablished
            && self.authority_verified
            && self
                .boundary
                .as_ref()
                .is_some_and(|reference| reference.persisted)
            && self
                .continuation
                .as_ref()
                .is_some_and(|reference| reference.persisted)
    }
}

impl DocketExecutionControl {
    /// Converts the scheduler's authoritative execution result into the compact
    /// gate consumed by executors. This intentionally derives from the same
    /// control object that selected or retained the execution-session claim.
    pub fn gate(&self) -> DocketExecutionGate {
        let reject = |disposition| DocketExecutionGate::Rejected {
            reason: self
                .reason
                .clone()
                .expect("non-working execution control always has a reason"),
            disposition,
        };
        match self.next_action {
            DocketExecutionNextAction::WorkCurrentTask => DocketExecutionGate::Allowed {
                task_id: self
                    .task
                    .claimed_task_id
                    .expect("working execution control always claims a task"),
                binding: DocketExecutionBinding::PairSession {
                    job_run_id: self.run_id,
                },
            },
            DocketExecutionNextAction::ReconcileExecution => {
                reject(DocketExecutionDisposition::Reconcile)
            }
            DocketExecutionNextAction::JobCompleted
            | DocketExecutionNextAction::RecoverBlockedRun => {
                reject(DocketExecutionDisposition::Stop)
            }
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct DocketJobExecuteOutcome {
    pub job: DocketJobProjection,
    pub control: DocketExecutionControl,
    /// Compatibility field. New callers should use `control.task.selected_task_id`.
    pub selected_task_id: Option<Uuid>,
    pub completed: bool,
    pub blocked: bool,
    pub message: String,
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DocketTaskPlacement {
    First,
    Last,
    Before { task_id: Uuid },
    After { task_id: Uuid },
}

#[derive(Debug, Clone)]
pub struct DocketTaskCreate {
    pub bear_id: Uuid,
    pub job_id: Option<Uuid>,
    /// Required for standalone tasks; stored only in bear_pair_task_attachments.
    pub pair_session_id: Option<Uuid>,
    pub parent_task_id: Option<Uuid>,
    pub sibling_order: i32,
    pub placement: Option<DocketTaskPlacement>,
    pub kind: DocketTaskKind,
    pub scope: DocketTaskScope,
    pub title: String,
    pub body: String,
    pub completion_criteria: Vec<String>,
    pub difficulty: Option<DocketTaskDifficulty>,
    pub effort_hint: Option<DocketEffortHint>,
    pub routing_strategy: RoutingStrategy,
    pub expected_context_size: Option<i32>,
    pub result_rollup_policy: Option<ResultRollupPolicy>,
    pub created_by_role: String,
    pub created_by_user_id: Option<i32>,
    pub created_by_agent_id: Option<String>,
    pub created_in_run_id: Option<Uuid>,
}

#[derive(Debug, Clone, Default)]
pub struct DocketTaskListFilter {
    pub job_id: Option<Uuid>,
    /// Filter standalone tasks by an active Pair attachment.
    pub pair_session_id: Option<Uuid>,
    pub parent_task_id: Option<Uuid>,
    pub include_descendants: bool,
    pub limit: i64,
}

#[derive(Debug, Clone)]
pub struct DocketTaskRunStateUpdate {
    pub run_id: Uuid,
    pub status: DocketTaskStatus,
    pub outcome_disposition: Option<DocketOutcomeDisposition>,
    pub result_refs: Option<serde_json::Value>,
    pub result_summary: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct DocketTaskDefinitionPatch {
    pub title: Option<String>,
    pub body: Option<String>,
    pub completion_criteria: Option<Vec<String>>,
    pub parent_task_id: Option<Option<Uuid>>,
    pub sibling_order: Option<i32>,
    pub kind: Option<DocketTaskKind>,
    pub scope: Option<DocketTaskScope>,
    pub difficulty: Option<Option<DocketTaskDifficulty>>,
    pub effort_hint: Option<Option<DocketEffortHint>>,
    pub routing_strategy: Option<RoutingStrategy>,
    pub expected_context_size: Option<Option<i32>>,
    pub result_rollup_policy: Option<Option<ResultRollupPolicy>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DocketEntryScope {
    TaskJournal,
    JobNotebook,
}

impl DocketEntryScope {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::TaskJournal => "task_journal",
            Self::JobNotebook => "job_notebook",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DocketEntryKind {
    Outcome,
    Finding,
    Decision,
    Obstacle,
    FollowUp,
    Milestone,
    Question,
}

impl DocketEntryKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Outcome => "outcome",
            Self::Finding => "finding",
            Self::Decision => "decision",
            Self::Obstacle => "obstacle",
            Self::FollowUp => "follow_up",
            Self::Milestone => "milestone",
            Self::Question => "question",
        }
    }
}

#[derive(Debug, Clone)]
pub struct DocketEntryCreate {
    pub bear_id: Uuid,
    pub job_id: Option<Uuid>,
    pub task_id: Option<Uuid>,
    pub run_id: Option<Uuid>,
    pub scope: DocketEntryScope,
    pub kind: DocketEntryKind,
    pub summary: String,
    pub body: Option<String>,
    pub evidence_refs: Vec<serde_json::Value>,
    pub related_task_ids: Vec<Uuid>,
    pub tags: Vec<String>,
    pub actor_role: BearProfile,
    pub actor_user_id: Option<i32>,
    pub actor_agent_id: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct DocketEntryListFilter {
    pub job_id: Option<Uuid>,
    pub task_id: Option<Uuid>,
    pub limit: i64,
}

#[derive(Debug, Clone)]
pub struct DocketEntryPromotion {
    pub bear_id: Uuid,
    pub entry_id: Uuid,
    pub actor_role: BearProfile,
    pub actor_user_id: Option<i32>,
    pub actor_agent_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, FromRow)]
pub struct DocketEntryRow {
    pub id: Uuid,
    pub job_id: Option<Uuid>,
    pub task_id: Option<Uuid>,
    pub run_id: Option<Uuid>,
    pub scope: String,
    pub kind: String,
    pub summary: String,
    pub body: Option<String>,
    pub disposition: Option<String>,
    pub evidence_refs: serde_json::Value,
    pub related_task_ids: serde_json::Value,
    pub tags: serde_json::Value,
    pub by_role: String,
    pub by_agent_id: Option<String>,
    pub by_user_id: Option<i32>,
    pub source_entry_id: Option<Uuid>,
    pub created_at: OffsetDateTime,
}

pub const DISPATCH_NOTEBOOK_CONTEXT_MAX_ENTRIES: usize = 12;
pub const DISPATCH_NOTEBOOK_CONTEXT_MAX_CHARS: usize = 6_000;

/// Selects durable notebook knowledge worth carrying into a dispatched worker.
///
/// Decisions and follow-ups are always eligible. Other entry kinds must be
/// explicitly tagged. Higher-value kinds win before recency, and the returned
/// text is bounded for direct prompt inclusion.
pub fn select_dispatch_notebook_context(entries: &[DocketEntryRow]) -> Vec<DocketEntryRow> {
    let mut eligible = entries
        .iter()
        .filter(|entry| entry.scope == DocketEntryScope::JobNotebook.as_str())
        .filter_map(|entry| {
            let priority = match entry.kind.as_str() {
                "decision" => 0,
                "follow_up" => 1,
                _ if entry.tags.as_array().is_some_and(|tags| !tags.is_empty()) => 2,
                _ => return None,
            };
            Some((priority, entry))
        })
        .collect::<Vec<_>>();
    eligible.sort_by(|(left_priority, left), (right_priority, right)| {
        left_priority
            .cmp(right_priority)
            .then_with(|| right.created_at.cmp(&left.created_at))
            .then_with(|| left.id.cmp(&right.id))
    });

    let mut used_chars = 0;
    eligible
        .into_iter()
        .take(DISPATCH_NOTEBOOK_CONTEXT_MAX_ENTRIES)
        .filter_map(|(_, entry)| {
            let entry_chars = entry.summary.chars().count()
                + entry.body.as_deref().map_or(0, |body| body.chars().count());
            if used_chars + entry_chars > DISPATCH_NOTEBOOK_CONTEXT_MAX_CHARS {
                return None;
            }
            used_chars += entry_chars;
            Some(entry.clone())
        })
        .collect()
    // ponytail: fixed priority/count/text bounds avoid retrieval machinery;
    // add relevance ranking only when real notebooks exceed these ceilings.
}

#[derive(Debug, Clone)]
pub struct DocketTaskUpdate {
    pub bear_id: Uuid,
    pub job_id: Option<Uuid>,
    pub task_id: Uuid,
    pub actor_role: BearProfile,
    pub actor_user_id: Option<i32>,
    pub actor_agent_id: Option<String>,
    pub definition: DocketTaskDefinitionPatch,
    pub run_state: Option<DocketTaskRunStateUpdate>,
}

#[derive(Debug, Clone, Serialize)]
pub struct DocketTaskProjection {
    pub task: DocketTaskRow,
    pub run_state: Option<DocketTaskRunStateRow>,
    /// Canonical task status for query consumers. A settlement is terminal
    /// even when session-owned work has no run-state row.
    pub status: DocketTaskStatus,
    /// Present only for impossible persisted combinations that need repair.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub integrity_conflict: Option<String>,
}

impl DocketTaskProjection {
    pub fn new(task: DocketTaskRow, run_state: Option<DocketTaskRunStateRow>) -> Self {
        let active_work = run_state
            .as_ref()
            .is_some_and(|state| matches!(state.status.as_str(), "pending" | "in_progress"));
        let integrity_conflict = (task.settled_by_entry_id.is_some() && active_work)
            .then(|| "integrity conflict: task is settled but has active work".to_string());
        let status = if integrity_conflict.is_some() {
            DocketTaskStatus::Blocked
        } else if task.settled_by_entry_id.is_some() {
            DocketTaskStatus::Done
        } else {
            match run_state.as_ref().map(|state| state.status.as_str()) {
                Some("done") => DocketTaskStatus::Done,
                Some("blocked") => DocketTaskStatus::Blocked,
                Some("cancelled") => DocketTaskStatus::Cancelled,
                _ => DocketTaskStatus::Pending,
            }
        };
        Self {
            task,
            run_state,
            status,
            integrity_conflict,
        }
    }
}

/// Settles a session-owned task without fabricating a Job run. The settlement
/// is represented by its authoritative task-journal outcome entry.
#[derive(Debug, Clone)]
pub struct DocketSessionTaskSettlement {
    pub bear_id: Uuid,
    pub pair_session_id: Uuid,
    pub task_id: Uuid,
    pub status: DocketTaskStatus,
    pub outcome_disposition: Option<DocketOutcomeDisposition>,
    pub result_refs: Option<serde_json::Value>,
    pub result_summary: Option<String>,
    pub actor_role: BearProfile,
    pub actor_user_id: Option<i32>,
    pub actor_agent_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DocketValidationError {
    EmptyGoal,
    MissingWorkSurface,
    AmbiguousWorkSurfaceAssignments,
    DuplicateWorkSurfaceAssignment { work_surface_id: Uuid },
    MismatchedWorkSurfaceBinding,
    InvalidJobCreatorRole { role: String },
    EmptyCriterionDescription,
    EmptyTaskTitle,
    EmptyTaskBody,
    EmptyTaskCompletionCriteria,
    EmptyTaskCompletionCriterion,
    TaskMissingAnchor,
    TaskAmbiguousAnchor,
    DuplicateTaskClientKey { client_key: String },
    MissingParentClientKey { client_key: String },
    SupersedeRequiresPredecessor,
    SupersedeRequiresMatchingActiveJob { job_id: Uuid },
    ActiveJobOverlap { job_id: Uuid },
}

impl fmt::Display for DocketValidationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyGoal => f.write_str("Docket job goal must not be empty"),
            Self::MissingWorkSurface => {
                f.write_str("Docket work job requires a managed work surface")
            }
            Self::AmbiguousWorkSurfaceAssignments => f.write_str(
                "Docket job must use either work_surface_id shorthand or work_surface_assignments, not both",
            ),
            Self::DuplicateWorkSurfaceAssignment { work_surface_id } => {
                write!(f, "Docket job assigns work surface `{work_surface_id}` more than once")
            }
            Self::MismatchedWorkSurfaceBinding => f.write_str(
                "Docket work job surface name and managed surface ID must be set together",
            ),
            Self::InvalidJobCreatorRole { role } => {
                write!(
                    f,
                    "Docket jobs must be human-created via chat, pair, or ui, not `{role}`"
                )
            }
            Self::EmptyCriterionDescription => {
                f.write_str("Docket job criterion description must not be empty")
            }
            Self::EmptyTaskTitle => f.write_str("Docket task title must not be empty"),
            Self::EmptyTaskBody => f.write_str("Docket task body must not be empty"),
            Self::EmptyTaskCompletionCriteria => {
                f.write_str("Docket task completion_criteria must include at least one criterion")
            }
            Self::EmptyTaskCompletionCriterion => {
                f.write_str("Docket task completion_criteria must not include blank criteria")
            }
            Self::TaskMissingAnchor => {
                f.write_str("Docket task must be anchored to either a job or an client session")
            }
            Self::TaskAmbiguousAnchor => {
                f.write_str("Docket task must be anchored to exactly one of a job or a client session")
            }
            Self::DuplicateTaskClientKey { client_key } => {
                write!(f, "Docket task client_key `{client_key}` is duplicated")
            }
            Self::MissingParentClientKey { client_key } => {
                write!(
                    f,
                    "Docket task parent_client_key `{client_key}` does not exist"
                )
            }
            Self::SupersedeRequiresPredecessor => {
                f.write_str("Docket job overlap resolution `supersede` requires supersedes_job_id")
            }
            Self::SupersedeRequiresMatchingActiveJob { job_id } => {
                write!(
                    f,
                    "Docket job `{job_id}` is not an active matching predecessor"
                )
            }
            Self::ActiveJobOverlap { job_id } => {
                write!(
                    f,
                    "an active Docket job already owns this goal and work surface: {job_id}"
                )
            }
        }
    }
}

impl std::error::Error for DocketValidationError {}

impl From<DocketValidationError> for DenError {
    fn from(err: DocketValidationError) -> Self {
        DenError::ValidationError(err.to_string())
    }
}

pub fn derived_docket_job_status(projection: &DocketJobProjection) -> String {
    let run_state = projection
        .current_run
        .as_ref()
        .map(|run| run.state.as_str());
    if let Some(intent) = projection.job.lifecycle_intent.as_deref() {
        return intent.to_string();
    }
    if run_state == Some("stalled") {
        return "stalled".to_string();
    }
    if !projection.active_task_ids.is_empty() {
        return "running".to_string();
    }
    if run_state == Some("blocked")
        || projection
            .task_states
            .iter()
            .any(|state| state.status == "blocked")
    {
        return "blocked".to_string();
    }
    let tasks_complete = projection.tasks.iter().all(|task| {
        projection
            .task_states
            .iter()
            .find(|state| state.task_id == task.id)
            .is_some_and(|state| matches!(state.status.as_str(), "done" | "cancelled"))
    });
    let criteria_complete = projection.criteria.iter().all(|criterion| {
        projection
            .criteria_states
            .iter()
            .find(|state| state.criterion_id == criterion.id)
            .is_some_and(|state| matches!(state.status.as_str(), "met" | "waived"))
    });
    if tasks_complete
        && criteria_complete
        && (!projection.tasks.is_empty() || !projection.criteria.is_empty())
    {
        "completed".to_string()
    } else if projection.tasks.is_empty() {
        "draft".to_string()
    } else {
        "ready".to_string()
    }
}

pub fn docket_job_status_report(projection: &DocketJobProjection) -> DocketJobStatusReport {
    let job_status = derived_docket_job_status(projection);
    let mut task_counts = DocketCountByStatus {
        pending: 0,
        in_progress: 0,
        done: 0,
        blocked: 0,
        cancelled: 0,
    };
    let task_states_by_id = projection
        .task_states
        .iter()
        .map(|state| (state.task_id, state.status.as_str()))
        .collect::<std::collections::HashMap<_, _>>();
    let active_task_ids = projection
        .active_task_ids
        .iter()
        .copied()
        .collect::<std::collections::HashSet<_>>();
    let current_task = projection
        .tasks
        .iter()
        .find(|task| active_task_ids.contains(&task.id));
    for task in &projection.tasks {
        if active_task_ids.contains(&task.id) {
            task_counts.in_progress += 1;
            continue;
        }
        match task_states_by_id
            .get(&task.id)
            .copied()
            .unwrap_or("pending")
        {
            "done" => task_counts.done += 1,
            "blocked" => task_counts.blocked += 1,
            "cancelled" => task_counts.cancelled += 1,
            _ => task_counts.pending += 1,
        }
    }

    let mut criteria_counts = DocketCriteriaCountByStatus {
        unmet: 0,
        met: 0,
        waived: 0,
    };
    let criteria_states_by_id = projection
        .criteria_states
        .iter()
        .map(|state| (state.criterion_id, state.status.as_str()))
        .collect::<std::collections::HashMap<_, _>>();
    for criterion in &projection.criteria {
        match criteria_states_by_id
            .get(&criterion.id)
            .copied()
            .unwrap_or("unmet")
        {
            "met" => criteria_counts.met += 1,
            "waived" => criteria_counts.waived += 1,
            _ => criteria_counts.unmet += 1,
        }
    }

    let tasks_complete = task_counts.pending == 0
        && task_counts.in_progress == 0
        && task_counts.blocked == 0
        && task_counts.done + task_counts.cancelled == projection.tasks.len();
    let criteria_complete = projection.criteria.is_empty()
        || criteria_counts.unmet == 0
            && criteria_counts.met + criteria_counts.waived == projection.criteria.len();
    let next_action = if job_status == "stalled" {
        "resolve_stalled_work_run".to_string()
    } else if task_counts.blocked > 0 || job_status == "blocked" {
        "resolve_blocked_task_or_criterion".to_string()
    } else if task_counts.in_progress > 0 {
        "continue_current_task".to_string()
    } else if task_counts.pending > 0 {
        "execute_job_to_select_next_task".to_string()
    } else if !criteria_complete {
        "evaluate_remaining_criteria".to_string()
    } else if job_status != "completed" {
        "execute_job_to_complete".to_string()
    } else {
        "done".to_string()
    };

    DocketJobStatusReport {
        job_id: projection.job.id,
        run_id: projection.current_run.as_ref().map(|run| run.id),
        job_status,
        run_state: projection.current_run.as_ref().map(|run| run.state.clone()),
        current_task_id: current_task.map(|task| task.id),
        current_task_title: current_task.map(|task| task.title.clone()),
        task_counts,
        criteria_counts,
        tasks_complete,
        criteria_complete,
        next_action,
    }
}

pub fn active_task_parent_id(projection: &DocketJobProjection) -> Option<Uuid> {
    let active_task_id = *projection.active_task_ids.first()?;
    projection
        .tasks
        .iter()
        .find(|task| task.id == active_task_id)
        .and_then(|task| task.parent_task_id)
}

pub fn task_list_projection_from_docket_job(
    projection: &DocketJobProjection,
    parent_task_id: Option<Uuid>,
) -> TaskListProjection {
    let states_by_task_id = projection
        .task_states
        .iter()
        .map(|state| (state.task_id, state))
        .collect::<std::collections::HashMap<_, _>>();
    let mut tasks = projection
        .tasks
        .iter()
        .filter(|task| task.parent_task_id == parent_task_id)
        .collect::<Vec<_>>();
    // Keep projection order deterministic at the task-list boundary. Upstream DB
    // queries usually order by sibling_order, but ACP "agent plan" rendering
    // should not depend on every caller preserving that order.
    tasks.sort_by_key(|task| (task.sibling_order, task.created_at, task.id));
    let items = tasks
        .into_iter()
        .map(|task| {
            task_list_item_from_docket_task(
                task,
                states_by_task_id.get(&task.id).copied(),
                projection.active_task_ids.contains(&task.id),
            )
        })
        .collect::<Vec<_>>();
    let current_item = current_task_list_item(&items).cloned();

    TaskListProjection {
        id: projection.job.id,
        bear_id: projection.job.bear_id,
        title: projection.job.goal.clone(),
        summary: "Docket work job checkout".to_string(),
        owner_profile: projection.job.created_by_role.clone(),
        visibility: projection.job.visibility.clone(),
        status: derived_docket_job_status(projection),
        version: 1,
        source_ref: TaskListSourceRef::docket_job(
            projection.job.id.to_string(),
            docket_checkout_refs(projection.job.id, parent_task_id),
        ),
        items,
        current_item,
        source_conversation_id: projection.job.source_conversation_id.clone(),
        source_client_session_id: None,
        handoff_intent_path: None,
        handoff_task_id: None,
        created_at: projection.job.created_at,
        updated_at: projection.job.updated_at,
    }
}

pub fn task_list_projection_from_session_tasks(
    bear_id: Uuid,
    owner_profile: BearProfile,
    conversation_id: &str,
    pair_session_id: Uuid,
    tasks: &[DocketTaskProjection],
) -> Option<TaskListProjection> {
    task_list_projection_from_session_tasks_with_current_task(
        bear_id,
        owner_profile,
        conversation_id,
        pair_session_id,
        tasks,
        None,
    )
}

/// Builds a session task projection, preferring an explicitly selected task
/// when it remains actionable in the anchored task tree.
pub fn task_list_projection_from_session_tasks_with_current_task(
    bear_id: Uuid,
    owner_profile: BearProfile,
    conversation_id: &str,
    pair_session_id: Uuid,
    tasks: &[DocketTaskProjection],
    selected_task_id: Option<Uuid>,
) -> Option<TaskListProjection> {
    let first_task = tasks.first()?;
    let selected_parent_id = selected_task_id.and_then(|selected_task_id| {
        tasks
            .iter()
            .find(|projection| projection.task.id == selected_task_id)
            .and_then(|projection| projection.task.parent_task_id)
    });
    let mut sorted_tasks = tasks
        .iter()
        .filter(|projection| {
            // ACP's agent plan is scoped to the selected task's current level:
            // child siblings share a parent; a root task stays a one-item plan.
            selected_task_id.is_none()
                || selected_parent_id
                    .is_some_and(|parent_id| projection.task.parent_task_id == Some(parent_id))
                || selected_task_id == Some(projection.task.id)
        })
        .collect::<Vec<_>>();
    // See task_list_projection_from_docket_job: ACP plan projection must be
    // deterministic even if the caller hands us an unordered task slice.
    sorted_tasks.sort_by_key(|projection| {
        (
            projection.task.sibling_order,
            projection.task.created_at,
            projection.task.id,
        )
    });
    let items = sorted_tasks
        .into_iter()
        .map(|projection| {
            task_list_item_from_docket_task(&projection.task, projection.run_state.as_ref(), false)
        })
        .collect::<Vec<_>>();
    let current_item = selected_task_id
        .and_then(|task_id| {
            items.iter().find(|item| {
                item.id == task_id.to_string()
                    && matches!(
                        item.status,
                        TaskListItemStatus::Pending | TaskListItemStatus::InProgress
                    )
            })
        })
        .cloned()
        .or_else(|| current_task_list_item(&items).cloned());
    let status = if items
        .iter()
        .all(|item| item.status == TaskListItemStatus::Completed)
    {
        "completed"
    } else if items
        .iter()
        .any(|item| item.status == TaskListItemStatus::Blocked)
    {
        "blocked"
    } else if items
        .iter()
        .any(|item| item.status == TaskListItemStatus::InProgress)
        || (selected_task_id.is_some() && current_item.is_some())
    {
        "active"
    } else {
        "planned"
    };
    Some(TaskListProjection {
        id: pair_session_id,
        bear_id,
        title: "Session tasks".to_string(),
        summary: "Tasks anchored to the current client session".to_string(),
        owner_profile: owner_profile.as_str().to_string(),
        visibility: "private_to_profile".to_string(),
        status: status.to_string(),
        version: 1,
        source_ref: TaskListSourceRef::local(vec![format!("session_anchor:{pair_session_id}")]),
        items,
        current_item,
        source_conversation_id: Some(conversation_id.to_string()),
        source_client_session_id: Some(pair_session_id.to_string()),
        handoff_intent_path: None,
        handoff_task_id: None,
        created_at: first_task.task.created_at,
        updated_at: tasks
            .iter()
            .map(|projection| projection.task.updated_at)
            .max()
            .unwrap_or(first_task.task.updated_at),
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DocketSourceRef {
    Job(Uuid),
    ParentTask(Uuid),
    Task(Uuid),
    MissingJob,
}

impl DocketSourceRef {
    const JOB_PREFIX: &'static str = "docket_job:";
    const PARENT_TASK_PREFIX: &'static str = "docket_parent_task:";
    const TASK_PREFIX: &'static str = "docket_task:";
    const MISSING_JOB_REF: &'static str = "docket_job:<none>";

    fn parse(raw: &str) -> Option<Self> {
        let raw = raw.trim();
        if raw == Self::MISSING_JOB_REF {
            return Some(Self::MissingJob);
        }
        if let Some(id) = raw.strip_prefix(Self::JOB_PREFIX) {
            return Uuid::parse_str(id.trim()).ok().map(Self::Job);
        }
        if let Some(id) = raw.strip_prefix(Self::PARENT_TASK_PREFIX) {
            return Uuid::parse_str(id.trim()).ok().map(Self::ParentTask);
        }
        if let Some(id) = raw.strip_prefix(Self::TASK_PREFIX) {
            return Uuid::parse_str(id.trim()).ok().map(Self::Task);
        }
        None
    }
}

impl fmt::Display for DocketSourceRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Job(id) => write!(f, "{}{id}", Self::JOB_PREFIX),
            Self::ParentTask(id) => write!(f, "{}{id}", Self::PARENT_TASK_PREFIX),
            Self::Task(id) => write!(f, "{}{id}", Self::TASK_PREFIX),
            Self::MissingJob => f.write_str(Self::MISSING_JOB_REF),
        }
    }
}

fn docket_checkout_refs(job_id: Uuid, parent_task_id: Option<Uuid>) -> Vec<String> {
    let mut refs = vec![DocketSourceRef::Job(job_id).to_string()];
    if let Some(parent_task_id) = parent_task_id {
        refs.push(DocketSourceRef::ParentTask(parent_task_id).to_string());
    }
    refs
}

pub fn docket_parent_task_ref(source_ref: &TaskListSourceRef) -> Option<Uuid> {
    source_ref.refs.iter().find_map(|raw| {
        if let Some(DocketSourceRef::ParentTask(id)) = DocketSourceRef::parse(raw) {
            Some(id)
        } else {
            None
        }
    })
}

pub fn task_list_item_status_from_docket_task_status(status: &str) -> TaskListItemStatus {
    match status {
        "in_progress" => TaskListItemStatus::InProgress,
        "done" => TaskListItemStatus::Completed,
        "blocked" => TaskListItemStatus::Blocked,
        "cancelled" => TaskListItemStatus::Cancelled,
        _ => TaskListItemStatus::Pending,
    }
}

pub fn docket_task_status_from_task_list_item_status(
    status: TaskListItemStatus,
) -> DocketTaskStatus {
    match status {
        TaskListItemStatus::Pending => DocketTaskStatus::Pending,
        TaskListItemStatus::InProgress => DocketTaskStatus::Pending,
        TaskListItemStatus::Blocked => DocketTaskStatus::Blocked,
        TaskListItemStatus::Completed => DocketTaskStatus::Done,
        TaskListItemStatus::Cancelled => DocketTaskStatus::Cancelled,
    }
}

fn task_list_item_from_docket_task(
    task: &DocketTaskRow,
    state: Option<&DocketTaskRunStateRow>,
    is_executing: bool,
) -> TaskListItem {
    let settled_with_active_work = task.settled_by_entry_id.is_some()
        && (is_executing
            || state
                .is_some_and(|state| matches!(state.status.as_str(), "pending" | "in_progress")));
    let blocked_after_settlement =
        task.settled_by_entry_id.is_some() && state.is_some_and(|state| state.status == "blocked");
    let status = if settled_with_active_work || blocked_after_settlement {
        TaskListItemStatus::Blocked
    } else if task.settled_by_entry_id.is_some() {
        TaskListItemStatus::Completed
    } else if is_executing {
        TaskListItemStatus::InProgress
    } else {
        state
            .map(|state| task_list_item_status_from_docket_task_status(&state.status))
            .unwrap_or(TaskListItemStatus::Pending)
    };
    TaskListItem {
        id: task.id.to_string(),
        title: task.title.clone(),
        summary: Some(task.body.clone()),
        status,
        blocked_reason: if settled_with_active_work {
            Some("integrity conflict: task is settled but has active work".to_string())
        } else if blocked_after_settlement {
            state.and_then(|state| state.result_summary.clone())
        } else {
            (status == TaskListItemStatus::Blocked)
                .then(|| state.and_then(|state| state.result_summary.clone()))
                .flatten()
        },
        source_ref: TaskListSourceRef::docket_task(
            task.job_id.map(|job_id| job_id.to_string()),
            task.id.to_string(),
            vec![
                task.job_id
                    .map(DocketSourceRef::Job)
                    .unwrap_or(DocketSourceRef::MissingJob)
                    .to_string(),
                DocketSourceRef::Task(task.id).to_string(),
            ],
        ),
        sync_state: TaskListSyncState::Clean,
    }
}

fn current_task_list_item(items: &[TaskListItem]) -> Option<&TaskListItem> {
    items
        .iter()
        .find(|item| item.status == TaskListItemStatus::InProgress)
        .or_else(|| {
            items
                .iter()
                .find(|item| item.status == TaskListItemStatus::Blocked)
        })
        .or_else(|| {
            items
                .iter()
                .find(|item| item.status == TaskListItemStatus::Pending)
        })
}

pub fn validate_docket_job_create(create: &DocketJobCreate) -> Result<(), DocketValidationError> {
    if create.goal.trim().is_empty() {
        return Err(DocketValidationError::EmptyGoal);
    }
    if create.work_surface_id.is_some() && !create.work_surface_assignments.is_empty() {
        return Err(DocketValidationError::AmbiguousWorkSurfaceAssignments);
    }
    if create.work_surface_id.is_none() && create.work_surface_assignments.is_empty() {
        return Err(DocketValidationError::MissingWorkSurface);
    }
    let mut surface_ids = std::collections::HashSet::new();
    for assignment in &create.work_surface_assignments {
        if !surface_ids.insert(assignment.work_surface_id) {
            return Err(DocketValidationError::DuplicateWorkSurfaceAssignment {
                work_surface_id: assignment.work_surface_id,
            });
        }
    }
    if !matches!(create.created_by_role.trim(), "chat" | "pair" | "ui") {
        return Err(DocketValidationError::InvalidJobCreatorRole {
            role: create.created_by_role.clone(),
        });
    }
    for criterion in &create.criteria {
        if criterion.description.trim().is_empty() {
            return Err(DocketValidationError::EmptyCriterionDescription);
        }
    }
    validate_docket_task_inputs(&create.tasks)
}

pub fn docket_job_surface_assignments(
    create: &DocketJobCreate,
) -> Vec<DocketJobSurfaceAssignmentInput> {
    if create.work_surface_assignments.is_empty() {
        create
            .work_surface_id
            .map(|work_surface_id| DocketJobSurfaceAssignmentInput {
                work_surface_id,
                mutation_policy: MutationPolicy::Required,
            })
            .into_iter()
            .collect()
    } else {
        create.work_surface_assignments.clone()
    }
}

pub fn normalize_completion_criteria(criteria: &[String]) -> Vec<String> {
    criteria
        .iter()
        .map(|criterion| criterion.trim())
        .filter(|criterion| !criterion.is_empty())
        .map(str::to_string)
        .collect()
}

pub fn validate_completion_criteria(criteria: &[String]) -> Result<(), DocketValidationError> {
    if criteria.is_empty() {
        return Err(DocketValidationError::EmptyTaskCompletionCriteria);
    }
    if criteria.iter().any(|criterion| criterion.trim().is_empty()) {
        return Err(DocketValidationError::EmptyTaskCompletionCriterion);
    }
    Ok(())
}

pub fn validate_docket_task_create(create: &DocketTaskCreate) -> Result<(), DocketValidationError> {
    if create.job_id.is_none() && create.pair_session_id.is_none() {
        return Err(DocketValidationError::TaskMissingAnchor);
    }
    if create.job_id.is_some() && create.pair_session_id.is_some() {
        return Err(DocketValidationError::TaskAmbiguousAnchor);
    }
    if create.title.trim().is_empty() {
        return Err(DocketValidationError::EmptyTaskTitle);
    }
    if create.body.trim().is_empty() {
        return Err(DocketValidationError::EmptyTaskBody);
    }
    validate_completion_criteria(&create.completion_criteria)?;
    Ok(())
}

fn validate_docket_task_inputs(tasks: &[DocketTaskInput]) -> Result<(), DocketValidationError> {
    let mut keys = std::collections::HashSet::new();
    for task in tasks {
        if task.title.trim().is_empty() {
            return Err(DocketValidationError::EmptyTaskTitle);
        }
        if task.body.trim().is_empty() {
            return Err(DocketValidationError::EmptyTaskBody);
        }
        validate_completion_criteria(&task.completion_criteria)?;
        if let Some(key) = task
            .client_key
            .as_ref()
            .map(|key| key.trim())
            .filter(|key| !key.is_empty())
        {
            if !keys.insert(key.to_string()) {
                return Err(DocketValidationError::DuplicateTaskClientKey {
                    client_key: key.to_string(),
                });
            }
        }
    }
    for task in tasks {
        if let Some(parent_key) = task
            .parent_client_key
            .as_ref()
            .map(|key| key.trim())
            .filter(|key| !key.is_empty())
        {
            if !keys.contains(parent_key) {
                return Err(DocketValidationError::MissingParentClientKey {
                    client_key: parent_key.to_string(),
                });
            }
        }
    }
    Ok(())
}

#[cfg(test)]
fn current_item(items: &[TaskListUpdateItem]) -> Option<&TaskListUpdateItem> {
    items
        .iter()
        .find(|item| item.status == TaskListItemStatus::InProgress)
        .or_else(|| {
            items
                .iter()
                .find(|item| item.status == TaskListItemStatus::Blocked)
        })
        .or_else(|| {
            items
                .iter()
                .find(|item| item.status == TaskListItemStatus::Pending)
        })
}

pub fn validate_task_list_update(update: &TaskListUpdate) -> Result<(), TaskListValidationError> {
    if update.title.trim().is_empty() {
        return Err(TaskListValidationError::EmptyTitle);
    }

    validate_task_list_items(&update.items)
}

pub fn validate_task_list_items(
    items: &[TaskListUpdateItem],
) -> Result<(), TaskListValidationError> {
    let mut in_progress_count = 0;
    for item in items {
        if item.id.trim().is_empty() {
            return Err(TaskListValidationError::EmptyItemId);
        }
        if item.title.trim().is_empty() {
            return Err(TaskListValidationError::EmptyItemTitle {
                item_id: item.id.clone(),
            });
        }
        if item.status == TaskListItemStatus::InProgress {
            in_progress_count += 1;
        }
        if item.status == TaskListItemStatus::Blocked
            && item
                .blocked_reason
                .as_deref()
                .map(|reason| reason.trim().is_empty())
                .unwrap_or(true)
        {
            return Err(TaskListValidationError::BlockedItemMissingReason {
                item_id: item.id.clone(),
            });
        }
    }

    if in_progress_count > 1 {
        return Err(TaskListValidationError::MultipleInProgressItems);
    }
    Ok(())
}

pub fn role_can_update_task_list(role: BearProfile) -> bool {
    matches!(
        role,
        BearProfile::Chat | BearProfile::Pair | BearProfile::Work
    )
}

pub fn role_can_request_task_list_handoff(role: BearProfile) -> bool {
    matches!(role, BearProfile::Chat | BearProfile::Pair)
}

pub fn role_can_read_task_list(
    viewer_role: BearProfile,
    owner_profile: BearProfile,
    visibility: TaskListVisibility,
    same_user: bool,
) -> bool {
    match visibility {
        TaskListVisibility::PrivateToProfile => viewer_role == owner_profile,
        TaskListVisibility::SameUser => same_user || viewer_role == owner_profile,
        TaskListVisibility::BearVisible => true,
        TaskListVisibility::HandoffRequested => {
            matches!(viewer_role, BearProfile::Curate) || viewer_role == owner_profile
        }
    }
}

pub fn render_task_list_prompt_context(task_lists: &[TaskListLocalProjection]) -> String {
    if task_lists.is_empty() {
        return String::new();
    }

    let mut out = String::from(
        "\n\n<system-reminder>\nDen activity context for this Bear. Session task lists are working projections; durable jobs/tasks live in Docket. Use Docket job/task tools for canonical state and `den.task_list.request_handoff` for reviewed promotion or reconciliation.\n",
    );
    for task_list in task_lists.iter().take(5) {
        let _ = write!(
            out,
            "- task_list_id={} owner={} status={} visibility={} title={}",
            task_list.id,
            task_list.owner_profile,
            task_list.status,
            task_list.visibility,
            task_list.title
        );
        if let Some(current) = task_list.current_item.as_ref() {
            let _ = write!(out, " current_item={} ({})", current.title, current.status);
        }
        if !task_list.summary.trim().is_empty() {
            let _ = write!(out, " summary={}", task_list.summary.trim());
        }
        out.push('\n');
    }
    out.push_str("</system-reminder>");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(id: &str, status: TaskListItemStatus) -> TaskListUpdateItem {
        TaskListUpdateItem {
            id: id.to_string(),
            title: format!("Item {id}"),
            summary: None,
            status,
            blocked_reason: None,
            source_refs: Vec::new(),
        }
    }

    #[test]
    fn dispatch_notebook_context_is_explicit_prioritized_and_bounded() {
        fn entry(
            kind: &str,
            summary: &str,
            tags: serde_json::Value,
            seconds: i64,
        ) -> DocketEntryRow {
            DocketEntryRow {
                id: Uuid::new_v4(),
                job_id: Some(Uuid::nil()),
                task_id: Some(Uuid::nil()),
                run_id: None,
                scope: "job_notebook".to_string(),
                kind: kind.to_string(),
                summary: summary.to_string(),
                body: None,
                disposition: None,
                evidence_refs: serde_json::json!([]),
                related_task_ids: serde_json::json!([]),
                tags,
                by_role: "pair".to_string(),
                by_agent_id: None,
                by_user_id: None,
                source_entry_id: None,
                created_at: OffsetDateTime::UNIX_EPOCH + time::Duration::seconds(seconds),
            }
        }

        let entries = vec![
            entry("finding", "ignored", serde_json::json!([]), 4),
            entry("finding", "tagged", serde_json::json!(["api"]), 3),
            entry("follow_up", "follow", serde_json::json!([]), 2),
            entry("decision", "decision", serde_json::json!([]), 1),
        ];

        let selected = select_dispatch_notebook_context(&entries);
        assert_eq!(
            selected
                .iter()
                .map(|entry| entry.summary.as_str())
                .collect::<Vec<_>>(),
            vec!["decision", "follow", "tagged"]
        );
        assert!(selected.len() <= DISPATCH_NOTEBOOK_CONTEXT_MAX_ENTRIES);
    }

    #[test]
    fn outcome_dispositions_match_terminal_lifecycle_states() {
        use DocketOutcomeDisposition as Outcome;
        use DocketTaskStatus as Status;

        for disposition in [Outcome::Completed, Outcome::NoChange, Outcome::Delegated] {
            assert!(disposition.is_valid_for(Status::Done));
        }
        for disposition in [Outcome::Blocked, Outcome::Failed] {
            assert!(disposition.is_valid_for(Status::Blocked));
        }
        assert!(Outcome::Cancelled.is_valid_for(Status::Cancelled));

        for status in [Status::Pending, Status::Blocked, Status::Cancelled] {
            assert!(!Outcome::Completed.is_valid_for(status));
        }
        assert!(!Outcome::Failed.is_valid_for(Status::Done));
        assert!(!Outcome::NoChange.is_valid_for(Status::Blocked));
        assert!(!Outcome::Delegated.is_valid_for(Status::Cancelled));
    }

    #[test]
    fn validates_single_in_progress_item() {
        let items = vec![
            item("one", TaskListItemStatus::Completed),
            item("two", TaskListItemStatus::InProgress),
            item("three", TaskListItemStatus::Pending),
        ];
        assert!(validate_task_list_items(&items).is_ok());
    }

    #[test]
    fn docket_parent_task_ref_uses_typed_source_ref_parser() {
        let parent_id = Uuid::parse_str("00000000-0000-0000-0000-000000000123").unwrap();
        let task_id = Uuid::parse_str("00000000-0000-0000-0000-000000000456").unwrap();
        let source_ref = TaskListSourceRef::docket_job(
            task_id.to_string(),
            vec![
                DocketSourceRef::Task(task_id).to_string(),
                format!("  {}  ", DocketSourceRef::ParentTask(parent_id)),
            ],
        );

        assert_eq!(docket_parent_task_ref(&source_ref), Some(parent_id));
        assert_eq!(
            DocketSourceRef::parse(&DocketSourceRef::ParentTask(parent_id).to_string()),
            Some(DocketSourceRef::ParentTask(parent_id))
        );
    }

    #[test]
    fn deserializes_missing_item_ids_with_stable_generated_slugs() {
        let update: TaskListUpdate = serde_json::from_value(serde_json::json!({
            "title": "Fix ACP plan visibility",
            "visibility": "private_to_profile",
            "status": "active",
            "items": [
                { "title": "Inspect BearWire logs", "status": "completed" },
                { "title": "Patch plan projection", "summary": "Surface plan_update in ACP", "status": "in_progress" }
            ]
        }))
        .expect("task-list update should deserialize with generated item ids");
        let repeated: TaskListUpdate = serde_json::from_value(serde_json::json!({
            "title": "Fix ACP plan visibility",
            "visibility": "private_to_profile",
            "status": "active",
            "items": [
                { "title": "Inspect BearWire logs", "status": "completed" },
                { "title": "Patch plan projection", "summary": "Surface plan_update in ACP", "status": "in_progress" }
            ]
        }))
        .expect("task-list update should deserialize repeatedly");

        assert_eq!(update.items.len(), 2);
        assert!(update.items[0].id.starts_with("inspect_bearwire_logs_"));
        assert!(update.items[1].id.starts_with("patch_plan_projection_"));
        assert_eq!(update.items[0].id, repeated.items[0].id);
        assert_eq!(update.items[1].id, repeated.items[1].id);
        assert!(validate_task_list_update(&update).is_ok());
    }

    #[test]
    fn generated_item_ids_are_unique_for_duplicate_items() {
        let update: TaskListUpdate = serde_json::from_value(serde_json::json!({
            "title": "Duplicate item test",
            "visibility": "private_to_profile",
            "status": "active",
            "items": [
                { "title": "Do the thing", "status": "pending" },
                { "title": "Do the thing", "status": "pending" }
            ]
        }))
        .expect("task-list update should deserialize duplicate generated ids");

        assert_ne!(update.items[0].id, update.items[1].id);
        assert!(update.items[0].id.starts_with("do_the_thing_"));
        assert!(update.items[1].id.starts_with("do_the_thing_"));
        assert!(validate_task_list_update(&update).is_ok());
    }

    #[test]
    fn rejects_multiple_in_progress_items() {
        let items = vec![
            item("one", TaskListItemStatus::InProgress),
            item("two", TaskListItemStatus::InProgress),
        ];
        assert_eq!(
            validate_task_list_items(&items),
            Err(TaskListValidationError::MultipleInProgressItems)
        );
    }

    #[test]
    fn blocked_items_need_reason() {
        let items = vec![item("one", TaskListItemStatus::Blocked)];
        assert_eq!(
            validate_task_list_items(&items),
            Err(TaskListValidationError::BlockedItemMissingReason {
                item_id: "one".to_string()
            })
        );
    }

    #[test]
    fn visibility_preserves_role_boundaries() {
        assert!(role_can_read_task_list(
            BearProfile::Pair,
            BearProfile::Pair,
            TaskListVisibility::PrivateToProfile,
            false
        ));
        assert!(!role_can_read_task_list(
            BearProfile::Chat,
            BearProfile::Pair,
            TaskListVisibility::PrivateToProfile,
            false
        ));
        assert!(role_can_read_task_list(
            BearProfile::Chat,
            BearProfile::Pair,
            TaskListVisibility::BearVisible,
            false
        ));
        assert!(role_can_read_task_list(
            BearProfile::Curate,
            BearProfile::Pair,
            TaskListVisibility::HandoffRequested,
            false
        ));
        assert!(!role_can_read_task_list(
            BearProfile::Work,
            BearProfile::Pair,
            TaskListVisibility::HandoffRequested,
            false
        ));
    }

    #[test]
    fn only_channel_roles_request_task_list_handoff() {
        assert!(role_can_request_task_list_handoff(BearProfile::Chat));
        assert!(role_can_request_task_list_handoff(BearProfile::Pair));
        assert!(!role_can_request_task_list_handoff(BearProfile::Work));
        assert!(!role_can_request_task_list_handoff(BearProfile::Curate));
    }

    fn projection_fixture(items: Vec<TaskListUpdateItem>) -> TaskListLocalProjection {
        TaskListLocalProjection {
            id: Uuid::parse_str("00000000-0000-0000-0000-000000000123").unwrap(),
            bear_id: Uuid::parse_str("00000000-0000-0000-0000-000000000456").unwrap(),
            title: "Build task system".to_string(),
            summary: "Keep status current".to_string(),
            owner_profile: "pair".to_string(),
            visibility: "bear_visible".to_string(),
            status: "active".to_string(),
            version: 1,
            current_item: current_item(&items).cloned(),
            items,
            source_conversation_id: Some("den-conv-test".to_string()),
            source_client_session_id: Some("acp-test".to_string()),
            handoff_intent_path: None,
            handoff_task_id: None,
            created_at: OffsetDateTime::UNIX_EPOCH,
            updated_at: OffsetDateTime::UNIX_EPOCH,
        }
    }

    #[test]
    fn task_list_projection_wraps_task_list_as_local_projection() {
        let plan = projection_fixture(vec![item("one", TaskListItemStatus::InProgress)]);

        let task_list = plan.to_task_list_projection();

        assert_eq!(task_list.source_ref.kind, "local");
        assert_eq!(
            task_list.source_ref.refs,
            vec![format!("task_list:{}", plan.id)]
        );
        assert_eq!(task_list.items.len(), 1);
        assert_eq!(task_list.items[0].source_ref.kind, "local");
        assert_eq!(task_list.items[0].sync_state, TaskListSyncState::LocalOnly);
        assert_eq!(
            task_list.current_item.as_ref().map(|item| item.id.as_str()),
            Some("one")
        );
    }

    #[test]
    fn task_list_projection_preserves_docket_backing_refs_when_present() {
        let mut backed = item("backed", TaskListItemStatus::Pending);
        backed.source_refs = vec![
            "docket_job:job-123".to_string(),
            "docket_task:task-456".to_string(),
        ];
        let plan = projection_fixture(vec![backed]);

        let task_list = task_list_projection_from_local(&plan);
        let item = &task_list.items[0];

        assert_eq!(item.source_ref.kind, "docket_task");
        assert_eq!(item.source_ref.docket_job_id.as_deref(), Some("job-123"));
        assert_eq!(item.source_ref.docket_task_id.as_deref(), Some("task-456"));
        assert_eq!(item.sync_state, TaskListSyncState::CheckedOut);
    }

    #[test]
    fn session_task_projection_is_planned_until_work_starts() {
        let bear_id = Uuid::parse_str("00000000-0000-0000-0000-000000000456").unwrap();
        let pair_session_id = Uuid::parse_str("00000000-0000-0000-0000-000000000789").unwrap();
        let task_id = Uuid::parse_str("00000000-0000-0000-0000-000000000001").unwrap();
        let run_id = Uuid::parse_str("00000000-0000-0000-0000-000000000002").unwrap();
        let mut task = DocketTaskRow {
            id: task_id,
            bear_id,
            job_id: None,
            parent_task_id: None,
            sibling_order: 0,
            kind: "execution".to_string(),
            scope: "run".to_string(),
            title: "Check the plan".to_string(),
            body: "Do not start yet.".to_string(),
            completion_criteria: Json(vec!["Plan is visible".to_string()]),
            difficulty: None,
            effort_hint: None,
            routing_strategy: "auto".to_string(),
            expected_context_size: None,
            result_rollup_policy: None,
            created_by_role: "pair".to_string(),
            created_by_user_id: None,
            created_by_agent_id: None,
            created_in_run_id: None,
            settled_by_entry_id: None,
            created_at: OffsetDateTime::UNIX_EPOCH,
            updated_at: OffsetDateTime::UNIX_EPOCH,
        };
        let planned = task_list_projection_from_session_tasks(
            bear_id,
            BearProfile::Pair,
            "conversation-1",
            pair_session_id,
            &[DocketTaskProjection {
                task: task.clone(),
                run_state: None,
                status: DocketTaskStatus::Pending,
                integrity_conflict: None,
            }],
        )
        .expect("session projection");

        assert_eq!(planned.status, "planned");
        assert_eq!(
            planned.current_item.as_ref().map(|item| item.id.as_str()),
            Some(task_id.to_string().as_str())
        );

        let active = task_list_projection_from_session_tasks(
            bear_id,
            BearProfile::Pair,
            "conversation-1",
            pair_session_id,
            &[DocketTaskProjection {
                task: task.clone(),
                run_state: Some(DocketTaskRunStateRow {
                    run_id,
                    task_id,
                    status: "in_progress".to_string(),
                    result_refs: None,
                    result_summary: None,
                    started_at: None,
                    finished_at: None,
                    updated_at: OffsetDateTime::UNIX_EPOCH,
                }),
                status: DocketTaskStatus::Pending,
                integrity_conflict: None,
            }],
        )
        .expect("active session projection");

        assert_eq!(active.status, "active");

        let completed = task_list_projection_from_session_tasks(
            bear_id,
            BearProfile::Pair,
            "conversation-1",
            pair_session_id,
            &[DocketTaskProjection {
                task: task.clone(),
                run_state: Some(DocketTaskRunStateRow {
                    run_id,
                    task_id,
                    status: "done".to_string(),
                    result_refs: None,
                    result_summary: None,
                    started_at: None,
                    finished_at: Some(OffsetDateTime::UNIX_EPOCH),
                    updated_at: OffsetDateTime::UNIX_EPOCH,
                }),
                status: DocketTaskStatus::Pending,
                integrity_conflict: None,
            }],
        )
        .expect("completed session projection");

        assert_eq!(completed.status, "completed");

        task.settled_by_entry_id =
            Some(Uuid::parse_str("00000000-0000-0000-0000-000000000003").unwrap());
        let settled = task_list_projection_from_session_tasks(
            bear_id,
            BearProfile::Pair,
            "conversation-1",
            pair_session_id,
            &[DocketTaskProjection {
                task: task.clone(),
                run_state: None,
                status: DocketTaskStatus::Pending,
                integrity_conflict: None,
            }],
        )
        .expect("settled session projection");

        assert_eq!(settled.status, "completed");
        assert_eq!(settled.items[0].status, TaskListItemStatus::Completed);

        let blocked_after_settlement = task_list_projection_from_session_tasks(
            bear_id,
            BearProfile::Pair,
            "conversation-1",
            pair_session_id,
            &[DocketTaskProjection {
                task: task.clone(),
                run_state: Some(DocketTaskRunStateRow {
                    run_id,
                    task_id,
                    status: "blocked".to_string(),
                    result_refs: None,
                    result_summary: Some("output finalization is required".to_string()),
                    started_at: None,
                    finished_at: None,
                    updated_at: OffsetDateTime::UNIX_EPOCH,
                }),
                status: DocketTaskStatus::Blocked,
                integrity_conflict: None,
            }],
        )
        .expect("blocked session projection");

        assert_eq!(blocked_after_settlement.status, "blocked");
        assert_eq!(
            blocked_after_settlement.items[0].status,
            TaskListItemStatus::Blocked
        );
        assert_eq!(
            blocked_after_settlement.items[0].blocked_reason.as_deref(),
            Some("output finalization is required")
        );

        let conflict = task_list_projection_from_session_tasks(
            bear_id,
            BearProfile::Pair,
            "conversation-1",
            pair_session_id,
            &[DocketTaskProjection {
                task,
                run_state: Some(DocketTaskRunStateRow {
                    run_id,
                    task_id,
                    status: "in_progress".to_string(),
                    result_refs: None,
                    result_summary: None,
                    started_at: Some(OffsetDateTime::UNIX_EPOCH),
                    finished_at: None,
                    updated_at: OffsetDateTime::UNIX_EPOCH,
                }),
                status: DocketTaskStatus::Pending,
                integrity_conflict: None,
            }],
        )
        .expect("conflicting session projection");

        assert_eq!(conflict.status, "blocked");
        assert_eq!(conflict.items[0].status, TaskListItemStatus::Blocked);
        assert_eq!(
            conflict.items[0].blocked_reason.as_deref(),
            Some("integrity conflict: task is settled but has active work")
        );
    }

    #[test]
    fn session_task_projection_prefers_an_explicit_actionable_task() {
        let bear_id = Uuid::parse_str("00000000-0000-0000-0000-000000000456").unwrap();
        let pair_session_id = Uuid::parse_str("00000000-0000-0000-0000-000000000789").unwrap();
        let first_id = Uuid::parse_str("00000000-0000-0000-0000-000000000001").unwrap();
        let selected_id = Uuid::parse_str("00000000-0000-0000-0000-000000000002").unwrap();
        let task = |id, order, title: &str| DocketTaskProjection {
            task: DocketTaskRow {
                id,
                bear_id,
                job_id: None,
                parent_task_id: None,
                sibling_order: order,
                kind: "execution".to_string(),
                scope: "run".to_string(),
                title: title.to_string(),
                body: String::new(),
                completion_criteria: Json(vec![]),
                difficulty: None,
                effort_hint: None,
                routing_strategy: "auto".to_string(),
                expected_context_size: None,
                result_rollup_policy: None,
                created_by_role: "pair".to_string(),
                created_by_user_id: None,
                created_by_agent_id: None,
                created_in_run_id: None,
                settled_by_entry_id: None,
                created_at: OffsetDateTime::UNIX_EPOCH,
                updated_at: OffsetDateTime::UNIX_EPOCH,
            },
            run_state: None,
            status: DocketTaskStatus::Pending,
            integrity_conflict: None,
        };

        let projection = task_list_projection_from_session_tasks_with_current_task(
            bear_id,
            BearProfile::Pair,
            "conversation-1",
            pair_session_id,
            &[task(first_id, 0, "first"), task(selected_id, 1, "selected")],
            Some(selected_id),
        )
        .expect("session projection");

        assert_eq!(projection.status, "active");
        assert_eq!(projection.current_item.unwrap().id, selected_id.to_string());
        assert_eq!(projection.items.len(), 1);
        assert_eq!(projection.items[0].id, selected_id.to_string());
    }

    #[test]
    fn selected_session_child_projects_its_siblings_in_order() {
        let bear_id = Uuid::parse_str("00000000-0000-0000-0000-000000000456").unwrap();
        let pair_session_id = Uuid::parse_str("00000000-0000-0000-0000-000000000789").unwrap();
        let parent_id = Uuid::parse_str("00000000-0000-0000-0000-000000000010").unwrap();
        let first_id = Uuid::parse_str("00000000-0000-0000-0000-000000000011").unwrap();
        let selected_id = Uuid::parse_str("00000000-0000-0000-0000-000000000012").unwrap();
        let task = |id, parent_task_id, order, title: &str| DocketTaskProjection {
            task: DocketTaskRow {
                id,
                bear_id,
                job_id: None,
                parent_task_id,
                sibling_order: order,
                kind: "execution".to_string(),
                scope: "run".to_string(),
                title: title.to_string(),
                body: String::new(),
                completion_criteria: Json(vec![]),
                difficulty: None,
                effort_hint: None,
                routing_strategy: "auto".to_string(),
                expected_context_size: None,
                result_rollup_policy: None,
                created_by_role: "pair".to_string(),
                created_by_user_id: None,
                created_by_agent_id: None,
                created_in_run_id: None,
                settled_by_entry_id: None,
                created_at: OffsetDateTime::UNIX_EPOCH,
                updated_at: OffsetDateTime::UNIX_EPOCH,
            },
            run_state: None,
            status: DocketTaskStatus::Pending,
            integrity_conflict: None,
        };

        let projection = task_list_projection_from_session_tasks_with_current_task(
            bear_id,
            BearProfile::Pair,
            "conversation-1",
            pair_session_id,
            &[
                task(parent_id, None, 0, "parent"),
                task(selected_id, Some(parent_id), 1, "selected"),
                task(first_id, Some(parent_id), 0, "first"),
            ],
            Some(selected_id),
        )
        .expect("session projection");

        assert_eq!(projection.current_item.unwrap().id, selected_id.to_string());
        assert_eq!(
            projection
                .items
                .iter()
                .map(|item| item.id.clone())
                .collect::<Vec<_>>(),
            vec![first_id.to_string(), selected_id.to_string()]
        );
    }

    #[test]
    fn new_jobs_default_to_one_coherent_publish() {
        assert_eq!(
            DocketCommitPolicy::for_new_job(None),
            DocketCommitPolicy::PerJob
        );
        assert_eq!(
            DocketCommitPolicy::for_new_job(Some(DocketCommitPolicy::None)),
            DocketCommitPolicy::None
        );
    }

    #[test]
    fn docket_job_status_serializes_archived() {
        assert_eq!(DocketJobStatus::Archived.as_str(), "archived");
        assert_eq!(
            serde_json::to_string(&DocketJobStatus::Archived).unwrap(),
            "\"archived\""
        );
        assert_eq!(
            serde_json::from_str::<DocketJobStatus>("\"archived\"").unwrap(),
            DocketJobStatus::Archived
        );
    }

    #[test]
    fn validates_docket_job_created_by_human_surface() {
        let create = DocketJobCreate {
            bear_id: Uuid::parse_str("00000000-0000-0000-0000-000000000456").unwrap(),
            created_by_user_id: 42,
            created_by_role: "work".to_string(),
            goal: "Ship Docket".to_string(),
            work_surface_id: Some(Uuid::parse_str("00000000-0000-0000-0000-000000000999").unwrap()),
            work_surface_assignments: vec![],
            commit_policy: Some(DocketCommitPolicy::None),
            work_branch: None,
            visibility: TaskListVisibility::BearVisible,
            source_conversation_id: None,
            objective_kind: None,
            supersedes_job_id: None,
            overlap_resolution: DocketJobOverlapResolution::Reject,
            criteria: Vec::new(),
            tasks: Vec::new(),
        };

        assert_eq!(
            validate_docket_job_create(&create),
            Err(DocketValidationError::InvalidJobCreatorRole {
                role: "work".to_string()
            })
        );
    }

    #[test]
    fn validates_docket_task_client_key_hierarchy() {
        let create = DocketJobCreate {
            bear_id: Uuid::parse_str("00000000-0000-0000-0000-000000000456").unwrap(),
            created_by_user_id: 42,
            created_by_role: "pair".to_string(),
            goal: "Ship Docket".to_string(),
            work_surface_id: Some(Uuid::parse_str("00000000-0000-0000-0000-000000000999").unwrap()),
            work_surface_assignments: vec![],
            commit_policy: None,
            work_branch: None,
            visibility: TaskListVisibility::BearVisible,
            source_conversation_id: None,
            objective_kind: None,
            supersedes_job_id: None,
            overlap_resolution: DocketJobOverlapResolution::Reject,
            criteria: Vec::new(),
            tasks: vec![DocketTaskInput {
                client_key: Some("child".to_string()),
                parent_client_key: Some("missing-parent".to_string()),
                parent_task_id: None,
                sibling_order: Some(0),
                kind: DocketTaskKind::Execution,
                scope: DocketTaskScope::Template,
                title: "Implement child".to_string(),
                body: "Do the child task.".to_string(),
                completion_criteria: vec!["Child task is actually done".to_string()],
                difficulty: Some(DocketTaskDifficulty::Moderate),
                effort_hint: Some(DocketEffortHint::Medium),
                routing_strategy: RoutingStrategy::Auto,
                expected_context_size: None,
                result_rollup_policy: None,
            }],
        };

        assert_eq!(
            validate_docket_job_create(&create),
            Err(DocketValidationError::MissingParentClientKey {
                client_key: "missing-parent".to_string()
            })
        );
    }

    #[test]
    fn rejects_docket_task_without_completion_criteria() {
        let create = DocketTaskCreate {
            bear_id: Uuid::parse_str("00000000-0000-0000-0000-000000000456").unwrap(),
            job_id: Some(Uuid::parse_str("00000000-0000-0000-0000-000000000777").unwrap()),
            pair_session_id: None,
            parent_task_id: None,
            sibling_order: 0,
            placement: None,
            kind: DocketTaskKind::Investigation,
            scope: DocketTaskScope::Run,
            title: "Investigate".to_string(),
            body: "Find the relevant facts.".to_string(),
            completion_criteria: Vec::new(),
            difficulty: Some(DocketTaskDifficulty::Unknown),
            effort_hint: None,
            routing_strategy: RoutingStrategy::Auto,
            expected_context_size: None,
            result_rollup_policy: None,
            created_by_role: "pair".to_string(),
            created_by_user_id: Some(42),
            created_by_agent_id: None,
            created_in_run_id: None,
        };

        assert_eq!(
            validate_docket_task_create(&create),
            Err(DocketValidationError::EmptyTaskCompletionCriteria)
        );
    }

    #[test]
    fn rejects_task_with_both_session_and_job_anchors() {
        let create = DocketTaskCreate {
            bear_id: Uuid::parse_str("00000000-0000-0000-0000-000000000456").unwrap(),
            job_id: Some(Uuid::parse_str("00000000-0000-0000-0000-000000000777").unwrap()),
            pair_session_id: Some(Uuid::parse_str("00000000-0000-0000-0000-000000000888").unwrap()),
            parent_task_id: None,
            sibling_order: 0,
            placement: None,
            kind: DocketTaskKind::Investigation,
            scope: DocketTaskScope::Run,
            title: "Investigate".to_string(),
            body: "Find the relevant facts.".to_string(),
            completion_criteria: vec!["Relevant facts are identified".to_string()],
            difficulty: Some(DocketTaskDifficulty::Unknown),
            effort_hint: None,
            routing_strategy: RoutingStrategy::Auto,
            expected_context_size: None,
            result_rollup_policy: None,
            created_by_role: "pair".to_string(),
            created_by_user_id: Some(42),
            created_by_agent_id: None,
            created_in_run_id: None,
        };

        assert_eq!(
            validate_docket_task_create(&create),
            Err(DocketValidationError::TaskAmbiguousAnchor)
        );
    }

    #[test]
    fn validates_session_anchored_or_job_anchored_task_create() {
        let create = DocketTaskCreate {
            bear_id: Uuid::parse_str("00000000-0000-0000-0000-000000000456").unwrap(),
            job_id: None,
            pair_session_id: None,
            parent_task_id: None,
            sibling_order: 0,
            placement: None,
            kind: DocketTaskKind::Investigation,
            scope: DocketTaskScope::Run,
            title: "Investigate".to_string(),
            body: "Find the relevant facts.".to_string(),
            completion_criteria: vec!["Relevant facts are identified".to_string()],
            difficulty: Some(DocketTaskDifficulty::Unknown),
            effort_hint: None,
            routing_strategy: RoutingStrategy::Auto,
            expected_context_size: None,
            result_rollup_policy: None,
            created_by_role: "pair".to_string(),
            created_by_user_id: Some(42),
            created_by_agent_id: None,
            created_in_run_id: None,
        };

        assert_eq!(
            validate_docket_task_create(&create),
            Err(DocketValidationError::TaskMissingAnchor)
        );
    }

    fn docket_projection_fixture() -> DocketJobProjection {
        let job_id = Uuid::parse_str("00000000-0000-0000-0000-000000000777").unwrap();
        let root_task_id = Uuid::parse_str("00000000-0000-0000-0000-000000000888").unwrap();
        let run_id = Uuid::parse_str("00000000-0000-0000-0000-000000000abc").unwrap();
        DocketJobProjection {
            job: DocketJobRow {
                id: job_id,
                bear_id: Uuid::parse_str("00000000-0000-0000-0000-000000000456").unwrap(),
                created_by_user_id: 42,
                created_by_role: "pair".to_string(),
                goal: "Ship Docket".to_string(),
                work_surface_id: None,
                commit_policy: Some("none".to_string()),
                work_branch: None,
                status: "running".to_string(),
                lifecycle_intent: None,
                visibility: "bear_visible".to_string(),
                source_conversation_id: Some("conversation-1".to_string()),
                objective_kind: Some("conversation_task_list".to_string()),
                supersedes_job_id: None,
                current_run_id: Some(run_id),
                created_at: OffsetDateTime::UNIX_EPOCH,
                updated_at: OffsetDateTime::UNIX_EPOCH,
            },
            current_run: Some(DocketJobRunRow {
                id: run_id,
                job_id,
                trigger: "manual".to_string(),
                schedule_ref: None,
                state: "running".to_string(),
                started_at: Some(OffsetDateTime::UNIX_EPOCH),
                finished_at: None,
                outcome: None,
                created_at: OffsetDateTime::UNIX_EPOCH,
                updated_at: OffsetDateTime::UNIX_EPOCH,
            }),
            criteria: vec![DocketJobCriterionRow {
                id: Uuid::parse_str("00000000-0000-0000-0000-000000000c01").unwrap(),
                job_id,
                kind: "narrative".to_string(),
                description: "Criterion".to_string(),
                spec: None,
                sibling_order: 0,
                created_at: OffsetDateTime::UNIX_EPOCH,
                updated_at: OffsetDateTime::UNIX_EPOCH,
            }],
            criteria_states: Vec::new(),
            tasks: vec![DocketTaskRow {
                id: root_task_id,
                bear_id: Uuid::parse_str("00000000-0000-0000-0000-000000000456").unwrap(),
                job_id: Some(job_id),
                parent_task_id: None,
                sibling_order: 0,
                kind: "execution".to_string(),
                scope: "template".to_string(),
                title: "Root task".to_string(),
                body: "Do root work.".to_string(),
                completion_criteria: Json(vec!["Root work done".to_string()]),
                difficulty: None,
                effort_hint: None,
                routing_strategy: "auto".to_string(),
                expected_context_size: None,
                result_rollup_policy: None,
                created_by_role: "pair".to_string(),
                created_by_user_id: Some(42),
                created_by_agent_id: None,
                created_in_run_id: None,
                settled_by_entry_id: None,
                created_at: OffsetDateTime::UNIX_EPOCH,
                updated_at: OffsetDateTime::UNIX_EPOCH,
            }],
            task_states: vec![DocketTaskRunStateRow {
                run_id,
                task_id: root_task_id,
                status: "in_progress".to_string(),
                result_refs: None,
                result_summary: None,
                started_at: None,
                finished_at: None,
                updated_at: OffsetDateTime::UNIX_EPOCH,
            }],
            active_task_ids: vec![root_task_id],
        }
    }

    #[test]
    fn docket_status_report_summarizes_current_task_and_next_action() {
        let projection = docket_projection_fixture();

        let report = docket_job_status_report(&projection);

        assert_eq!(report.job_status, "running");
        assert_eq!(report.run_state.as_deref(), Some("running"));
        assert_eq!(report.current_task_title.as_deref(), Some("Root task"));
        assert_eq!(report.task_counts.in_progress, 1);
        assert_eq!(report.criteria_counts.unmet, 1);
        assert!(!report.tasks_complete);
        assert!(!report.criteria_complete);
        assert_eq!(report.next_action, "continue_current_task");
    }

    #[test]
    fn derived_status_prefers_stalled_run_over_stale_job_status() {
        let mut projection = docket_projection_fixture();
        projection.job.status = "ready".to_string();
        projection.current_run.as_mut().unwrap().state = "stalled".to_string();

        let report = docket_job_status_report(&projection);
        let task_list = task_list_projection_from_docket_job(&projection, None);

        assert_eq!(report.job_status, "stalled");
        assert_eq!(report.next_action, "resolve_stalled_work_run");
        assert_eq!(task_list.status, "stalled");
    }

    #[test]
    fn task_list_projection_from_docket_job_projects_conversation_objective_level() {
        let job_id = Uuid::parse_str("00000000-0000-0000-0000-000000000777").unwrap();
        let root_task_id = Uuid::parse_str("00000000-0000-0000-0000-000000000888").unwrap();
        let root_peer_task_id = Uuid::parse_str("00000000-0000-0000-0000-000000000889").unwrap();
        let child_task_id = Uuid::parse_str("00000000-0000-0000-0000-000000000999").unwrap();
        let run_id = Uuid::parse_str("00000000-0000-0000-0000-000000000abc").unwrap();
        let projection = DocketJobProjection {
            job: DocketJobRow {
                id: job_id,
                bear_id: Uuid::parse_str("00000000-0000-0000-0000-000000000456").unwrap(),
                created_by_user_id: 42,
                created_by_role: "pair".to_string(),
                goal: "Ship Docket".to_string(),
                work_surface_id: None,
                commit_policy: Some("none".to_string()),
                work_branch: None,
                status: "running".to_string(),
                lifecycle_intent: None,
                visibility: "bear_visible".to_string(),
                source_conversation_id: Some("conversation-1".to_string()),
                objective_kind: Some("conversation_task_list".to_string()),
                supersedes_job_id: None,
                current_run_id: Some(run_id),
                created_at: OffsetDateTime::UNIX_EPOCH,
                updated_at: OffsetDateTime::UNIX_EPOCH,
            },
            current_run: None,
            criteria: Vec::new(),
            tasks: vec![
                DocketTaskRow {
                    id: root_task_id,
                    bear_id: Uuid::parse_str("00000000-0000-0000-0000-000000000456").unwrap(),
                    job_id: Some(job_id),
                    parent_task_id: None,
                    sibling_order: 0,
                    kind: "execution".to_string(),
                    scope: "template".to_string(),
                    title: "Root task".to_string(),
                    body: "Do root work.".to_string(),
                    completion_criteria: sqlx::types::Json(vec!["Root work done".to_string()]),
                    difficulty: None,
                    effort_hint: None,
                    routing_strategy: "auto".to_string(),
                    expected_context_size: None,
                    result_rollup_policy: None,
                    created_by_role: "pair".to_string(),
                    created_by_user_id: Some(42),
                    created_by_agent_id: None,
                    created_in_run_id: None,
                    settled_by_entry_id: None,
                    created_at: OffsetDateTime::UNIX_EPOCH,
                    updated_at: OffsetDateTime::UNIX_EPOCH,
                },
                DocketTaskRow {
                    id: root_peer_task_id,
                    bear_id: Uuid::parse_str("00000000-0000-0000-0000-000000000456").unwrap(),
                    job_id: Some(job_id),
                    parent_task_id: None,
                    sibling_order: -1,
                    kind: "execution".to_string(),
                    scope: "template".to_string(),
                    title: "Root peer task".to_string(),
                    body: "Do peer work.".to_string(),
                    completion_criteria: sqlx::types::Json(vec!["Peer work done".to_string()]),
                    difficulty: None,
                    effort_hint: None,
                    routing_strategy: "auto".to_string(),
                    expected_context_size: None,
                    result_rollup_policy: None,
                    created_by_role: "pair".to_string(),
                    created_by_user_id: Some(42),
                    created_by_agent_id: None,
                    created_in_run_id: None,
                    settled_by_entry_id: None,
                    created_at: OffsetDateTime::UNIX_EPOCH,
                    updated_at: OffsetDateTime::UNIX_EPOCH,
                },
                DocketTaskRow {
                    id: child_task_id,
                    bear_id: Uuid::parse_str("00000000-0000-0000-0000-000000000456").unwrap(),
                    job_id: Some(job_id),
                    parent_task_id: Some(root_task_id),
                    sibling_order: 0,
                    kind: "execution".to_string(),
                    scope: "template".to_string(),
                    title: "Child task".to_string(),
                    body: "Do child work.".to_string(),
                    completion_criteria: sqlx::types::Json(vec!["Child work done".to_string()]),
                    difficulty: None,
                    effort_hint: None,
                    routing_strategy: "auto".to_string(),
                    expected_context_size: None,
                    result_rollup_policy: None,
                    created_by_role: "pair".to_string(),
                    created_by_user_id: Some(42),
                    created_by_agent_id: None,
                    created_in_run_id: None,
                    settled_by_entry_id: None,
                    created_at: OffsetDateTime::UNIX_EPOCH,
                    updated_at: OffsetDateTime::UNIX_EPOCH,
                },
            ],
            criteria_states: Vec::new(),
            task_states: vec![DocketTaskRunStateRow {
                run_id,
                task_id: root_task_id,
                status: "in_progress".to_string(),
                result_refs: None,
                result_summary: None,
                started_at: None,
                finished_at: None,
                updated_at: OffsetDateTime::UNIX_EPOCH,
            }],
            active_task_ids: vec![root_task_id],
        };

        let root_checkout = task_list_projection_from_docket_job(&projection, None);
        assert_eq!(root_checkout.source_ref.kind, "docket_job");
        assert_eq!(
            root_checkout.source_conversation_id.as_deref(),
            Some("conversation-1")
        );
        assert_eq!(
            root_checkout
                .current_item
                .as_ref()
                .map(|item| item.id.as_str()),
            Some(root_task_id.to_string().as_str())
        );
        assert_eq!(root_checkout.items.len(), 2);
        assert_eq!(root_checkout.items[0].id, root_peer_task_id.to_string());
        assert_eq!(root_checkout.items[1].id, root_task_id.to_string());
        assert_eq!(root_checkout.items[1].sync_state, TaskListSyncState::Clean);
        assert_eq!(
            root_checkout.items[1].status,
            TaskListItemStatus::InProgress
        );

        let child_checkout = task_list_projection_from_docket_job(&projection, Some(root_task_id));
        assert_eq!(
            child_checkout.source_conversation_id.as_deref(),
            Some("conversation-1")
        );
        assert_eq!(
            child_checkout
                .current_item
                .as_ref()
                .map(|item| item.id.as_str()),
            Some(child_task_id.to_string().as_str())
        );
        assert_eq!(child_checkout.items.len(), 1);
        assert_eq!(child_checkout.items[0].id, child_task_id.to_string());
        assert_eq!(
            child_checkout.items[0].source_ref.docket_task_id.as_deref(),
            Some(child_task_id.to_string().as_str())
        );
    }

    #[test]
    fn renders_compact_prompt_context_without_raw_workspace_context() {
        let plan = TaskListLocalProjection {
            id: Uuid::parse_str("00000000-0000-0000-0000-000000000123").unwrap(),
            bear_id: Uuid::parse_str("00000000-0000-0000-0000-000000000456").unwrap(),
            title: "Build task system".to_string(),
            summary: "Keep status current".to_string(),
            owner_profile: "pair".to_string(),
            visibility: "bear_visible".to_string(),
            status: "active".to_string(),
            version: 1,
            items: vec![item("one", TaskListItemStatus::InProgress)],
            current_item: Some(item("one", TaskListItemStatus::InProgress)),
            source_conversation_id: None,
            source_client_session_id: None,
            handoff_intent_path: None,
            handoff_task_id: None,
            created_at: OffsetDateTime::UNIX_EPOCH,
            updated_at: OffsetDateTime::UNIX_EPOCH,
        };

        let rendered = render_task_list_prompt_context(&[plan]);
        assert!(rendered.contains("Den activity context"));
        assert!(rendered.contains("task_list_id="));
        assert!(rendered.contains("durable jobs/tasks live in Docket"));
        assert!(rendered.contains("Build task system"));
        assert!(rendered.contains("Item one"));
        assert!(!rendered.contains("workspace_context"));
    }
}
