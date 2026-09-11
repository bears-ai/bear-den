//! Runtime current-task resolution.
//!
//! Pair resolves only its persisted current-task selection. The task-list
//! projection is a volatile cache used to seed prompts and tools; runtime
//! behavior resolves persisted state rather than treating cached or legacy
//! execution state as authoritative.

use crate::agent_loop::{
    ObjectiveOrientation, OrientationTaskRef, OrientedChildTaskPolicy, TaskOrientation,
};
use den_core::DenError;
use den_docket::{
    task_list_projection_from_session_tasks_with_current_task, DocketService, PgDocketService,
    TaskListProjection,
};
use den_service::{bears::BearProfile, client_sessions};
use sqlx::PgPool;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeTaskSource {
    SessionCurrentTask,
    None,
}

impl RuntimeTaskSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::SessionCurrentTask => "session_current_task",
            Self::None => "none",
        }
    }
}

#[derive(Debug, Clone)]
pub struct RuntimeTaskContext {
    pub source: RuntimeTaskSource,
    /// The explicitly selected task for this session, when one exists.
    ///
    /// This remains separate from the task-list projection: the projection
    /// supplies surrounding task-tree context, while this ID identifies the
    /// task Pair should treat as its current objective.
    pub current_task_id: Option<Uuid>,
    pub cached_activity_plan_projection: Option<TaskListProjection>,
}

impl RuntimeTaskContext {
    pub fn active_activity_plan(&self) -> Option<&TaskListProjection> {
        match self.source {
            RuntimeTaskSource::SessionCurrentTask => self.cached_activity_plan_projection.as_ref(),
            RuntimeTaskSource::None => None,
        }
    }

    pub fn focused_orientation(&self) -> Option<ObjectiveOrientation> {
        let current_task_id = self.current_task_id?;
        let plan = self.active_activity_plan()?;
        let item = plan
            .items
            .iter()
            .find(|item| item.id == current_task_id.to_string())?;
        Some(ObjectiveOrientation::Oriented {
            task: TaskOrientation {
                task_ref: orientation_task_ref_from_item(plan, item),
                child_policy: OrientedChildTaskPolicy::default(),
            },
        })
    }
}

pub(crate) fn orientation_task_ref_from_item(
    plan: &TaskListProjection,
    item: &den_docket::TaskListItem,
) -> OrientationTaskRef {
    if let Some(task_id) = item.source_ref.docket_task_id.clone() {
        return OrientationTaskRef::DocketTask {
            job_id: item
                .source_ref
                .docket_job_id
                .clone()
                .or_else(|| plan.source_ref.docket_job_id.clone()),
            task_id,
            title: Some(item.title.clone()),
        };
    }
    OrientationTaskRef::TaskListItem {
        task_list_id: plan.id.to_string(),
        item_id: item.id.clone(),
        title: Some(item.title.clone()),
    }
}

#[derive(Debug, Clone)]
pub struct RuntimeTaskResolveRequest {
    pub bear_id: Uuid,
    pub profile: BearProfile,
    pub user_id: Option<i32>,
    pub conversation_id: String,
    pub client_session_id: String,
    pub cached_activity_plan_projection: Option<TaskListProjection>,
}

fn is_actionable_session_task_status(status: den_docket::DocketTaskStatus) -> bool {
    status == den_docket::DocketTaskStatus::Pending
}

pub async fn resolve_runtime_task_context(
    pool: &PgPool,
    request: RuntimeTaskResolveRequest,
) -> Result<RuntimeTaskContext, DenError> {
    let RuntimeTaskResolveRequest {
        bear_id,
        profile,
        user_id,
        conversation_id,
        client_session_id,
        cached_activity_plan_projection: _,
    } = request;

    let Some(user_id) = user_id else {
        return Ok(RuntimeTaskContext {
            source: RuntimeTaskSource::None,
            current_task_id: None,
            cached_activity_plan_projection: None,
        });
    };
    let Some(session) =
        client_sessions::find_for_user_bear_session_id(pool, user_id, bear_id, &client_session_id)
            .await?
    else {
        return Ok(RuntimeTaskContext {
            source: RuntimeTaskSource::None,
            current_task_id: None,
            cached_activity_plan_projection: None,
        });
    };
    let service = PgDocketService::from_pool(pool);
    let tasks = service.list_session_tasks(bear_id, session.id).await?;
    let current_task_id = session.current_task_id.filter(|selected_task_id| {
        tasks
            .iter()
            .find(|task| task.task.id == *selected_task_id)
            .is_some_and(|task| is_actionable_session_task_status(task.status))
    });
    if current_task_id.is_some() {
        let plan = task_list_projection_from_session_tasks_with_current_task(
            bear_id,
            profile,
            &conversation_id,
            session.id,
            &tasks,
            current_task_id,
        );
        return Ok(RuntimeTaskContext {
            source: RuntimeTaskSource::SessionCurrentTask,
            current_task_id,
            cached_activity_plan_projection: plan,
        });
    }

    let plan = task_list_projection_from_session_tasks_with_current_task(
        bear_id,
        profile,
        &conversation_id,
        session.id,
        &tasks,
        current_task_id,
    );
    Ok(RuntimeTaskContext {
        source: RuntimeTaskSource::None,
        current_task_id: None,
        cached_activity_plan_projection: plan,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminal_session_tasks_are_not_current_task_candidates() {
        assert!(is_actionable_session_task_status(
            den_docket::DocketTaskStatus::Pending
        ));
        assert!(!is_actionable_session_task_status(
            den_docket::DocketTaskStatus::Done
        ));
        assert!(!is_actionable_session_task_status(
            den_docket::DocketTaskStatus::Blocked
        ));
        assert!(!is_actionable_session_task_status(
            den_docket::DocketTaskStatus::Cancelled
        ));
    }
}
