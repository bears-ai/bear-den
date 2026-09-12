//! Minimal dispatch seam for job-level work execution.
//!
//! Docket owns job/task state, but it must not execute task bodies itself
//! (ADR-0034 execution invariant). Runtime code can use this trait to poll for
//! runnable work tasks and record outcomes after a Bear runtime executes them.

use std::collections::{HashMap, HashSet};

use uuid::Uuid;

use den_core::{BearProfile, DenError};

use crate::db;
use crate::model::{
    DocketTaskDefinitionPatch, DocketTaskListFilter, DocketTaskProjection,
    DocketTaskRunStateUpdate, DocketTaskStatus, DocketTaskUpdate,
};
use crate::service::{DocketService, PgDocketService};

#[allow(async_fn_in_trait)]
pub trait TaskDispatcher: Send + Sync {
    async fn runnable_work_tasks(
        &self,
        bear_id: Uuid,
        limit: i64,
    ) -> Result<Vec<DocketTaskProjection>, DenError>;

    async fn mark_task_started(
        &self,
        bear_id: Uuid,
        task_id: Uuid,
        run_id: Uuid,
        actor_agent_id: Option<String>,
    ) -> Result<DocketTaskProjection, DenError>;

    async fn record_task_success(
        &self,
        bear_id: Uuid,
        task_id: Uuid,
        run_id: Uuid,
        result_summary: String,
        result_refs: Option<serde_json::Value>,
        actor_agent_id: Option<String>,
    ) -> Result<DocketTaskProjection, DenError>;

    async fn record_task_blocked(
        &self,
        bear_id: Uuid,
        task_id: Uuid,
        run_id: Uuid,
        blocked_reason: String,
        result_refs: Option<serde_json::Value>,
        actor_agent_id: Option<String>,
    ) -> Result<DocketTaskProjection, DenError>;
}

/// Select one pending leaf per job in depth-first sibling order. Parent tasks
/// are phase roll-ups, so dispatch only sees executable leaves.
fn first_pending_leaves_in_plan_order(
    tasks: Vec<DocketTaskProjection>,
) -> Vec<DocketTaskProjection> {
    let children = tasks.iter().enumerate().fold(
        HashMap::<(Option<Uuid>, Option<Uuid>), Vec<usize>>::new(),
        |mut children, (index, projection)| {
            children
                .entry((projection.task.job_id, projection.task.parent_task_id))
                .or_default()
                .push(index);
            children
        },
    );
    let jobs = tasks
        .iter()
        .filter_map(|task| task.task.job_id)
        .collect::<HashSet<_>>();
    let mut selected = Vec::new();
    for job_id in jobs {
        let mut visited = HashSet::new();
        if let Some(index) =
            first_pending_leaf_for_parent(job_id, None, &tasks, &children, &mut visited)
                .ok()
                .flatten()
        {
            selected.push(tasks[index].clone());
        }
    }
    selected
}

fn first_pending_leaf_for_parent(
    job_id: Uuid,
    parent_id: Option<Uuid>,
    tasks: &[DocketTaskProjection],
    children: &HashMap<(Option<Uuid>, Option<Uuid>), Vec<usize>>,
    visited: &mut HashSet<Uuid>,
) -> Result<Option<usize>, ()> {
    let Some(siblings) = children.get(&(Some(job_id), parent_id)) else {
        return Ok(None);
    };
    let mut siblings = siblings.clone();
    siblings.sort_by_key(|index| {
        let task = &tasks[*index].task;
        (task.sibling_order, task.created_at)
    });
    for index in siblings {
        let task = &tasks[index].task;
        if !visited.insert(task.id) {
            continue;
        }
        if children.contains_key(&(Some(job_id), Some(task.id))) {
            match first_pending_leaf_for_parent(job_id, Some(task.id), tasks, children, visited) {
                Ok(Some(next)) => return Ok(Some(next)),
                Err(()) => return Err(()),
                Ok(None) => continue,
            }
        }
        match tasks[index]
            .run_state
            .as_ref()
            .map(|state| state.status.as_str())
            .unwrap_or("pending")
        {
            "done" | "cancelled" => {}
            "pending" => return Ok(Some(index)),
            // Do not skip earlier claimed or blocked work to offer a later
            // sibling in the same sequential plan.
            _ => return Err(()),
        }
    }
    Ok(None)
}

impl TaskDispatcher for PgDocketService {
    async fn runnable_work_tasks(
        &self,
        bear_id: Uuid,
        limit: i64,
    ) -> Result<Vec<DocketTaskProjection>, DenError> {
        let scan_limit = if limit <= 0 { 100 } else { limit.min(500) };
        let tasks = db::list_tasks(
            &self.pool,
            bear_id,
            DocketTaskListFilter {
                include_descendants: true,
                // ponytail: scan the bounded local task tree until queue/lease
                // dispatch is needed for very large jobs.
                limit: 500,
                ..DocketTaskListFilter::default()
            },
        )
        .await?;

        // ponytail: fetch each selected job until dispatch becomes a single
        // queue query; the task scan is already capped at 500.
        let mut runnable = Vec::new();
        for task in first_pending_leaves_in_plan_order(tasks) {
            let Some(job_id) = task.task.job_id else {
                continue;
            };
            let Some(job) = db::get_job(&self.pool, bear_id, job_id).await? else {
                continue;
            };
            if job.job.status == "blocked"
                || job
                    .current_run
                    .as_ref()
                    .is_some_and(|run| run.state == "blocked")
            {
                continue;
            }
            runnable.push(task);
            if runnable.len() == scan_limit as usize {
                break;
            }
        }
        Ok(runnable)
    }

    async fn mark_task_started(
        &self,
        bear_id: Uuid,
        task_id: Uuid,
        run_id: Uuid,
        actor_agent_id: Option<String>,
    ) -> Result<DocketTaskProjection, DenError> {
        let runnable = self.runnable_work_tasks(bear_id, 500).await?;
        if !runnable.iter().any(|task| task.task.id == task_id) {
            return Err(DenError::ValidationError(format!(
                "Docket task {task_id} is not the first eligible pending leaf in sibling order"
            )));
        }
        let _ = (run_id, actor_agent_id);
        self.list_tasks(
            bear_id,
            DocketTaskListFilter {
                job_id: None,
                parent_task_id: None,
                session_anchor_id: None,
                include_descendants: true,
                limit: 500,
            },
        )
        .await?
        .into_iter()
        .find(|task| task.task.id == task_id)
        .ok_or_else(|| DenError::NotFound(format!("Docket task {task_id} not found")))
    }

    async fn record_task_success(
        &self,
        bear_id: Uuid,
        task_id: Uuid,
        run_id: Uuid,
        result_summary: String,
        result_refs: Option<serde_json::Value>,
        actor_agent_id: Option<String>,
    ) -> Result<DocketTaskProjection, DenError> {
        update_run_state(
            self,
            bear_id,
            task_id,
            run_id,
            DocketTaskStatus::Done,
            result_refs,
            Some(result_summary),
            actor_agent_id,
        )
        .await
    }

    async fn record_task_blocked(
        &self,
        bear_id: Uuid,
        task_id: Uuid,
        run_id: Uuid,
        blocked_reason: String,
        result_refs: Option<serde_json::Value>,
        actor_agent_id: Option<String>,
    ) -> Result<DocketTaskProjection, DenError> {
        update_run_state(
            self,
            bear_id,
            task_id,
            run_id,
            DocketTaskStatus::Blocked,
            result_refs,
            Some(blocked_reason),
            actor_agent_id,
        )
        .await
    }
}

async fn update_run_state(
    service: &PgDocketService,
    bear_id: Uuid,
    task_id: Uuid,
    run_id: Uuid,
    status: DocketTaskStatus,
    result_refs: Option<serde_json::Value>,
    result_summary: Option<String>,
    actor_agent_id: Option<String>,
) -> Result<DocketTaskProjection, DenError> {
    db::update_task(
        &service.pool,
        DocketTaskUpdate {
            bear_id,
            job_id: None,
            task_id,
            actor_role: BearProfile::Work,
            actor_user_id: None,
            actor_agent_id,
            definition: DocketTaskDefinitionPatch::default(),
            run_state: Some(DocketTaskRunStateUpdate {
                run_id,
                status,
                outcome_disposition: None,
                result_refs,
                result_summary,
            }),
        },
    )
    .await
}
