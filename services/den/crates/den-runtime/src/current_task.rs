use den_core::{BearCapability, CapabilitySet};
use den_docket::{
    task_list_projection_from_session_tasks_with_current_task, DocketService, PgDocketService,
    TaskListItemStatus, TaskListProjection,
};
use den_http::errors::CustomError;
use den_service::{
    bears::BearProfile, client_sessions, conversation::persistence as conversation_persistence,
};
use sqlx::PgPool;
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct SessionCurrentTaskSelection {
    pub title: Option<String>,
    pub task_list: Option<TaskListProjection>,
}

pub async fn preview_session_current_task_selection(
    pool: &PgPool,
    user_id: i32,
    bear_id: Uuid,
    client_session_id: &str,
    task_id: Uuid,
) -> Result<String, CustomError> {
    let session =
        client_sessions::find_for_user_bear_session_id(pool, user_id, bear_id, client_session_id)
            .await?
            .ok_or_else(|| CustomError::NotFound("client session not found".to_string()))?;
    let tasks = PgDocketService::from_pool(pool)
        .list_session_tasks(bear_id, session.id)
        .await?;
    actionable_task_title(
        bear_id,
        &session.conversation_id,
        session.id,
        &tasks,
        task_id,
    )
}

fn actionable_task_title(
    bear_id: Uuid,
    conversation_id: &str,
    session_id: Uuid,
    tasks: &[den_docket::DocketTaskProjection],
    task_id: Uuid,
) -> Result<String, CustomError> {
    let selected_tasks = task_list_projection_from_session_tasks_with_current_task(
        bear_id,
        BearProfile::Pair,
        conversation_id,
        session_id,
        tasks,
        Some(task_id),
    );
    selected_tasks
        .as_ref()
        .and_then(|task_list| task_list.current_item.as_ref())
        .filter(|task| task.id == task_id.to_string())
        .filter(|task| {
            matches!(
                task.status,
                TaskListItemStatus::Pending | TaskListItemStatus::InProgress
            )
        })
        .map(|task| task.title.clone())
        .ok_or_else(|| {
            let actionable = task_list_projection_from_session_tasks_with_current_task(
                bear_id,
                BearProfile::Pair,
                conversation_id,
                session_id,
                tasks,
                None,
            )
            .map(|task_list| {
                task_list
                    .items
                    .into_iter()
                    .filter(|task| {
                        matches!(
                            task.status,
                            TaskListItemStatus::Pending | TaskListItemStatus::InProgress
                        )
                    })
                    .map(|task| format!("{} ({})", task.id, task.title))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
            CustomError::ValidationError(format!(
                "selected task must be an actionable task anchored to the current session; call get_task_list_status before retrying. Actionable task candidates: {}",
                if actionable.is_empty() { "none".to_string() } else { actionable.join(", ") }
            ))
        })
}

/// Canonical client-session current-task mutation. All client and model transports
/// must use this operation rather than writing `client_sessions.current_task_id`.
pub async fn select_session_current_task(
    pool: &PgPool,
    user_id: i32,
    bear_id: Uuid,
    client_session_id: &str,
    task_id: Option<Uuid>,
    capabilities: &CapabilitySet,
) -> Result<SessionCurrentTaskSelection, CustomError> {
    capabilities.require(BearCapability::SelectSessionTask)?;
    let session =
        client_sessions::find_for_user_bear_session_id(pool, user_id, bear_id, client_session_id)
            .await?
            .ok_or_else(|| CustomError::NotFound("client session not found".to_string()))?;
    let tasks = PgDocketService::from_pool(pool)
        .list_session_tasks(bear_id, session.id)
        .await?;
    let selected_title = task_id
        .map(|task_id| {
            actionable_task_title(
                bear_id,
                &session.conversation_id,
                session.id,
                &tasks,
                task_id,
            )
        })
        .transpose()?;

    if session.current_task_id != task_id {
        client_sessions::set_current_task(pool, user_id, bear_id, client_session_id, task_id)
            .await?;
    }
    let conversation_id = session
        .resolved_conversation_id
        .as_deref()
        .filter(|id| !id.trim().is_empty())
        .unwrap_or(&session.conversation_id);
    if let Some(title) = selected_title.as_deref() {
        conversation_persistence::set_conversation_title_and_sync_client_sessions(
            pool,
            bear_id,
            conversation_id,
            title,
        )
        .await?;
    }
    let task_list = task_list_projection_from_session_tasks_with_current_task(
        bear_id,
        BearProfile::Pair,
        conversation_id,
        session.id,
        &tasks,
        task_id,
    );
    crate::native_runtime::update_native_client_session_cached_activity_plan_projection(
        conversation_id,
        client_session_id,
        task_list.clone(),
    );
    Ok(SessionCurrentTaskSelection {
        title: selected_title,
        task_list,
    })
}
