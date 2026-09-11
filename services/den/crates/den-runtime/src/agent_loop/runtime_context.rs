use den_core::DenError;
use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;

use den_service::bears::prompt_fragments::{
    render_turn_fragment, repository_prompt_fragment_registry,
};
use den_service::prompt_memory_block_store::{
    select_prompt_memory_blocks_for_runtime, PromptMemoryBlockQuery, PromptMemoryRuntimeSelection,
};
use den_service::prompt_memory_blocks::{
    compile_prompt_memory_blocks, render_prompt_memory_block_context, PromptMemoryCompilationInput,
};

use crate::agent_loop::ObjectiveOrientation;

const OBJECTIVE_ORIENTATION_MARKER: &str = "Den objective orientation is Den-owned";
const FOCUSED_ACTIVE_TASK_GUIDANCE: &str = "advancing the active task";
const FOCUSED_MUTABLE_NEXT_TASK_GUIDANCE: &str = "choosing or creating the next concrete task";
const FOCUSED_IMMUTABLE_NEXT_TASK_GUIDANCE: &str = "choosing the next existing concrete task";
const FOCUSED_MUTABLE_STRUCTURE_GUIDANCE: &str = "Add child tasks when useful.";
const FOCUSED_IMMUTABLE_STRUCTURE_GUIDANCE: &str =
    "Task-definition edits are unavailable while Docket execution is immutable; choose existing tasks and update status/results instead.";

fn system_reminder(body: String) -> String {
    format!("<system-reminder>\n{body}\n</system-reminder>")
}

fn oriented_root_task_id(task_ref: &crate::agent_loop::OrientationTaskRef) -> &str {
    match task_ref {
        crate::agent_loop::OrientationTaskRef::TaskListItem { item_id, .. } => item_id,
        crate::agent_loop::OrientationTaskRef::DocketTask { task_id, .. } => task_id,
    }
}

pub fn runtime_context_already_includes_den_owned_blocks(runtime_context: &str) -> bool {
    let trimmed = runtime_context.trim();
    !trimmed.is_empty()
        && (trimmed.contains("Prompt memory blocks are Den-owned")
            || trimmed.contains("Runtime compaction context is Den-owned")
            || trimmed.contains(OBJECTIVE_ORIENTATION_MARKER))
}

fn render_runtime_fragment(
    fragment_id: &str,
    context: serde_json::Value,
) -> Result<String, DenError> {
    let fragments = repository_prompt_fragment_registry()?;
    let fragment = fragments.require(fragment_id)?;
    render_turn_fragment(fragment, &context).map(system_reminder)
}

fn render_objective_orientation_context(
    profile_slug: &str,
    orientation: &ObjectiveOrientation,
) -> Result<String, DenError> {
    // Keep reusable steering prose in prompt fragments. Rust should only choose the
    // fragment and supply structured state so context compilation can audit,
    // suppress, and regression-test steering precedence in one place.
    match orientation {
        ObjectiveOrientation::Freeform { policy } => {
            let task_definition_tools = if policy.may_define_task {
                "available"
            } else {
                "unavailable"
            };
            render_runtime_fragment(
                "runtime_objective_freeform",
                json!({
                    "orientation": {
                        "profile_slug": profile_slug,
                        "may_define_task": policy.may_define_task,
                        "task_definition_tools": task_definition_tools,
                    }
                }),
            )
        }
        ObjectiveOrientation::Oriented { task } => {
            let task_ref = serde_json::to_string(&task.task_ref)
                .unwrap_or_else(|_| "{\"kind\":\"unknown\"}".to_string());
            let root_task_id = oriented_root_task_id(&task.task_ref);
            render_runtime_fragment(
                "runtime_objective_oriented",
                json!({
                    "orientation": {
                        "task_ref": task_ref,
                        "root_task_id": root_task_id,
                        "max_children": task.child_policy.max_children,
                        "max_depth_below_oriented_task": task.child_policy.max_depth_below_oriented_task,
                    }
                }),
            )
        }
        ObjectiveOrientation::DocketExecution { job } => {
            let active_task_ref = job
                .active_task_ref
                .as_ref()
                .and_then(|task| serde_json::to_string(task).ok())
                .unwrap_or_else(|| "null".to_string());
            let task_guidance = if job.active_task_ref.is_some() {
                FOCUSED_ACTIVE_TASK_GUIDANCE
            } else if job.mutable {
                FOCUSED_MUTABLE_NEXT_TASK_GUIDANCE
            } else {
                FOCUSED_IMMUTABLE_NEXT_TASK_GUIDANCE
            };
            let structure_guidance = if job.mutable {
                FOCUSED_MUTABLE_STRUCTURE_GUIDANCE
            } else {
                FOCUSED_IMMUTABLE_STRUCTURE_GUIDANCE
            };
            let task_definition_tools = if job.mutable {
                "available"
            } else {
                "unavailable"
            };
            render_runtime_fragment(
                "runtime_objective_docket_execution",
                json!({
                    "orientation": {
                        "job_id": job.job_id,
                        "job_mutable": job.mutable,
                        "task_definition_tools": task_definition_tools,
                        "active_task_ref": active_task_ref,
                        "task_guidance": task_guidance,
                        "structure_guidance": structure_guidance,
                    }
                }),
            )
        }
    }
}

async fn load_prompt_memory_runtime_text(
    pool: &PgPool,
    bear_id: Uuid,
    profile_slug: &str,
    session_id: &str,
    workspace_roots: &[String],
) -> Result<String, DenError> {
    let selection = match select_prompt_memory_blocks_for_runtime(
        pool,
        PromptMemoryBlockQuery {
            bear_id: Some(bear_id),
            profile_slug,
            session_id,
            work_surfaces: workspace_roots,
        },
    )
    .await
    {
        Ok(selection) => selection,
        Err(err) => PromptMemoryRuntimeSelection {
            diagnostic: json!({
                "source": "prompt_memory_blocks",
                "persisted": true,
                "status": "selection_error",
                "session_id": session_id,
                "work_surfaces": workspace_roots,
                "error": err.to_string(),
                "matched_count": 0,
            }),
            blocks: Vec::new(),
        },
    };
    let compilation = compile_prompt_memory_blocks(
        &selection.blocks,
        PromptMemoryCompilationInput {
            role: profile_slug,
            work_surfaces: workspace_roots,
            session_id,
            max_blocks: 6,
        },
    );
    Ok(render_prompt_memory_block_context(&compilation))
}

pub fn render_capability_discovery_guidance() -> Result<String, DenError> {
    render_runtime_fragment("runtime_capability_discovery", json!({}))
}

pub async fn assemble_den_owned_runtime_supplement(
    pool: &PgPool,
    bear_id: Uuid,
    profile_slug: &str,
    session_id: &str,
    workspace_roots: &[String],
    objective_orientation: &ObjectiveOrientation,
) -> Result<String, DenError> {
    let mut parts = Vec::new();
    parts.push(render_objective_orientation_context(
        profile_slug,
        objective_orientation,
    )?);
    let prompt_memory =
        load_prompt_memory_runtime_text(pool, bear_id, profile_slug, session_id, workspace_roots)
            .await?;
    if !prompt_memory.trim().is_empty() {
        parts.push(prompt_memory);
    }
    Ok(parts.join("\n\n"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_loop::{DocketExecutionOrientation, FreeformPolicy, OrientationTaskRef};

    #[test]
    fn runtime_context_already_includes_den_owned_blocks_detects_compaction() {
        assert!(runtime_context_already_includes_den_owned_blocks(
            "Runtime compaction context is Den-owned."
        ));
        assert!(!runtime_context_already_includes_den_owned_blocks(
            "plain runtime notes"
        ));
    }

    #[test]
    fn capability_discovery_guidance_explains_lazy_loading_and_authority() {
        let guidance = render_capability_discovery_guidance().unwrap();
        assert!(guidance.contains("full catalog is not projected"));
        assert!(guidance.contains("capability_search"));
        assert!(guidance.contains("Code Mode"));
        assert!(guidance.contains("not an authority grant"));
    }

    #[test]
    fn pair_freeform_guidance_prefers_task_lists_over_jobs() {
        let pair = render_objective_orientation_context(
            "pair",
            &ObjectiveOrientation::Freeform {
                policy: FreeformPolicy::task_definition_permitted(),
            },
        )
        .unwrap();
        assert!(pair.contains("Pair task-orientation hint"));
        assert!(pair.contains("do bounded work here"));
        assert!(pair.contains(
            "Create a Job only when it needs its own lifecycle, work surface, commit policy, or background execution"
        ));
        assert!(pair.contains("Do not taskify ordinary Q&A"));

        let chat = render_objective_orientation_context(
            "chat",
            &ObjectiveOrientation::Freeform {
                policy: FreeformPolicy::task_definition_permitted(),
            },
        )
        .unwrap();
        assert!(!chat.contains("Pair task-orientation hint"));

        let pair_closed = render_objective_orientation_context(
            "pair",
            &ObjectiveOrientation::Freeform {
                policy: FreeformPolicy::closed(),
            },
        )
        .unwrap();
        assert!(!pair_closed.contains("Pair task-orientation hint"));
        assert!(pair_closed.contains("task_definition_tools=unavailable"));
        assert!(pair_closed
            .contains("Task-definition tools are unavailable in closed freeform orientation"));
    }

    #[test]
    fn docket_execution_guidance_branches_on_active_task_and_mutability() {
        let active_task = OrientationTaskRef::DocketTask {
            job_id: Some("job-1".to_string()),
            task_id: "task-1".to_string(),
            title: Some("Do it".to_string()),
        };

        let active_mutable = render_objective_orientation_context(
            "pair",
            &ObjectiveOrientation::DocketExecution {
                job: DocketExecutionOrientation {
                    job_id: "job-1".to_string(),
                    active_task_ref: Some(active_task),
                    mutable: true,
                },
            },
        )
        .unwrap();
        assert!(active_mutable.contains("Advance the assigned active task when one is present"));
        assert!(active_mutable.contains("Add child tasks when useful."));
        assert!(!active_mutable.contains("If active_task_ref"));

        let no_active_mutable = render_objective_orientation_context(
            "pair",
            &ObjectiveOrientation::DocketExecution {
                job: DocketExecutionOrientation {
                    job_id: "job-1".to_string(),
                    active_task_ref: None,
                    mutable: true,
                },
            },
        )
        .unwrap();
        assert!(no_active_mutable.contains("choosing or creating the next concrete task"));
        assert!(no_active_mutable.contains("Add child tasks when useful."));

        let no_active_immutable = render_objective_orientation_context(
            "pair",
            &ObjectiveOrientation::DocketExecution {
                job: DocketExecutionOrientation {
                    job_id: "job-1".to_string(),
                    active_task_ref: None,
                    mutable: false,
                },
            },
        )
        .unwrap();
        assert!(no_active_immutable.contains("choosing the next existing concrete task"));
        assert!(no_active_immutable.contains("task_definition_tools=unavailable"));
        assert!(no_active_immutable
            .contains("Task-definition edits are unavailable while Docket execution is immutable"));
    }

    #[test]
    fn oriented_guidance_allows_planning_pause() {
        let oriented = render_objective_orientation_context(
            "pair",
            &ObjectiveOrientation::Oriented {
                task: crate::agent_loop::TaskOrientation {
                    task_ref: OrientationTaskRef::DocketTask {
                        job_id: None,
                        task_id: "task-1".to_string(),
                        title: Some("Plan the work".to_string()),
                    },
                    child_policy: crate::agent_loop::OrientedChildTaskPolicy {
                        max_children: 3,
                        max_depth_below_oriented_task: 1,
                    },
                },
            },
        )
        .unwrap();

        assert!(oriented.contains("pause when the user asked only to plan"));
        assert!(oriented.contains("exceed the requested scope"));
        assert!(oriented.contains("oriented_root_task_id=task-1"));
        assert!(oriented.contains("max_children=3"));
        assert!(oriented.contains("max_depth_below_oriented_task=1"));
    }
}
