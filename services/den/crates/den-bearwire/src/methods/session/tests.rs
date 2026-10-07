use super::*;
use den_docket::{
    TaskListItem, TaskListItemStatus, TaskListProjection, TaskListSourceRef, TaskListSyncState,
};
use den_runtime::runtime::task_context::{RuntimeTaskContext, RuntimeTaskSource};
use sqlx::types::time::OffsetDateTime;
use uuid::Uuid;

fn session_current_task_context(task_id: Uuid) -> RuntimeTaskContext {
    let item = TaskListItem {
        id: task_id.to_string(),
        title: "Selected task".to_string(),
        summary: Some("Current session task".to_string()),
        status: TaskListItemStatus::Pending,
        blocked_reason: None,
        source_ref: TaskListSourceRef::local(vec![]),
        sync_state: TaskListSyncState::CheckedOut,
    };
    RuntimeTaskContext {
        source: RuntimeTaskSource::SessionCurrentTask,
        current_task_id: Some(task_id),
        cached_activity_plan_projection: Some(TaskListProjection {
            id: Uuid::new_v4(),
            bear_id: Uuid::new_v4(),
            title: "Session tasks".to_string(),
            summary: String::new(),
            owner_profile: "pair".to_string(),
            visibility: "private_to_profile".to_string(),
            status: "active".to_string(),
            version: 1,
            source_ref: TaskListSourceRef::local(vec![]),
            items: vec![item.clone()],
            current_item: Some(item),
            source_conversation_id: None,
            source_client_session_id: None,
            handoff_intent_path: None,
            handoff_task_id: None,
            created_at: OffsetDateTime::UNIX_EPOCH,
            updated_at: OffsetDateTime::UNIX_EPOCH,
        }),
    }
}

#[test]
fn current_task_projects_session_focus_only() {
    let task_id = Uuid::new_v4();
    let projected = session_current_task_projection(&session_current_task_context(task_id))
        .expect("session-selected task should project");
    assert_eq!(projected["id"], task_id.to_string());
    assert_eq!(projected["title"], "Selected task");

    let plan = session_current_task_context(task_id)
        .active_activity_plan()
        .cloned()
        .expect("session task plan");
    let acp_projection = active_activity_plan_projection(
        plan,
        RuntimeTaskSource::SessionCurrentTask.as_str(),
        Some(projected),
    );
    assert_eq!(acp_projection["current_task"]["id"], task_id.to_string());
    assert_eq!(acp_projection["status"], "active");
    assert_eq!(acp_projection["items"][0]["status"], "pending");
    assert_eq!(acp_projection["items"][0]["selection"], "current");

    let mut no_selection = session_current_task_context(task_id);
    no_selection.current_task_id = None;
    assert!(session_current_task_projection(&no_selection).is_none());

    let no_selection_plan = no_selection
        .active_activity_plan()
        .cloned()
        .expect("session task plan");
    let acp_without_selection = active_activity_plan_projection(
        no_selection_plan,
        RuntimeTaskSource::SessionCurrentTask.as_str(),
        None,
    );
    assert!(acp_without_selection["current_task"].is_null());
}
