use den_core::{BearProfile, DenError};
use serde_json::{json, Value};
use sqlx::{postgres::PgPoolOptions, PgPool};
use uuid::Uuid;

use crate::{
    docket_task_status_from_task_list_item_status, task_list_projection_from_docket_job,
    DocketCommitPolicy, DocketCriterionKind, DocketCriterionStateUpdate, DocketEffortHint,
    DocketEntryCreate, DocketEntryKind, DocketEntryListFilter, DocketEntryPromotion,
    DocketEntryScope, DocketExecutionAttemptAuthorize, DocketExecutionBindingKind,
    DocketExecutionHost, DocketExecutionHostKind, DocketExecutionTaskSettlement,
    DocketFocusedExecutionBinding, DocketJobCreate, DocketJobCriterionInput,
    DocketJobExecuteRequest, DocketJobOverlapResolution, DocketService,
    DocketSessionTaskSettlement, DocketTaskCreate, DocketTaskDefinitionPatch, DocketTaskDifficulty,
    DocketTaskInput, DocketTaskKind, DocketTaskListFilter, DocketTaskRunStateUpdate,
    DocketTaskScope, DocketTaskStatus, DocketTaskUpdate, PgDocketService, RoutingStrategy,
    TaskDispatcher, TaskListSyncRequest, TaskListVisibility,
};

fn primary_output_result_refs() -> Value {
    json!({
        "primary_output": {
            "kind": "git_commit",
            "artifact_ref": "git:0123456789abcdef0123456789abcdef01234567",
            "immutable_identity": "0123456789abcdef0123456789abcdef01234567"
        },
        "validation": {
            "primary_output_ref": "git:0123456789abcdef0123456789abcdef01234567",
            "immutable_identity": "0123456789abcdef0123456789abcdef01234567",
            "command": "cargo test -p den-docket",
            "result": "passed",
            "execution_provenance": "local integration test"
        }
    })
}

pub(super) async fn live_session_attempt(
    pool: &PgPool,
    bear_id: Uuid,
    client_session_id: &str,
) -> Option<(Uuid, Uuid, String)> {
    sqlx::query_as(
        "SELECT task_id, host_run_id, state FROM docket_execution_attempts
         WHERE bear_id = $1 AND binding_kind = 'client_session' AND binding_id = $2
           AND state IN ('authorized', 'running', 'paused', 'awaiting_user', 'stopping')
         ORDER BY updated_at DESC LIMIT 1",
    )
    .bind(bear_id)
    .bind(client_session_id)
    .fetch_optional(pool)
    .await
    .expect("lookup live canonical Pair attempt")
}

pub(super) async fn test_pool() -> Option<PgPool> {
    let database_url = std::env::var("TEST_DATABASE_URL")
        .or_else(|_| std::env::var("DATABASE_URL"))
        .unwrap_or_else(|_| "postgres://postgres:postgres@127.0.0.1/postgres".to_string());
    let pool = PgPoolOptions::new()
        .max_connections(2)
        .connect(&database_url)
        .await
        .ok()?;
    sqlx::migrate!("../../migrations").run(&pool).await.ok()?;
    Some(pool)
}

async fn seed_client_session(pool: &PgPool, user_id: i32, bear_id: Uuid) -> Uuid {
    let id = Uuid::new_v4();
    let suffix = Uuid::new_v4().simple().to_string();
    sqlx::query!(
        r#"
        INSERT INTO client_sessions (
            id, user_id, bear_id, bear_slug, client_session_id, runtime_session_id,
            conversation_id, client
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7, 'integration-test')
        "#,
        id,
        user_id,
        bear_id,
        format!("bear-{suffix}"),
        format!("client-{suffix}"),
        format!("runtime-{suffix}"),
        format!("conversation-{suffix}"),
    )
    .execute(pool)
    .await
    .expect("seed client session");
    id
}

fn test_work_surface_id(bear_id: Uuid) -> Uuid {
    bear_id
}

pub(super) async fn seed_user_and_bear(pool: &PgPool, label: &str) -> (i32, Uuid) {
    let suffix = Uuid::new_v4().simple().to_string();
    let username = format!("u{}", &suffix[..20]);
    let email = format!("{label}-{suffix}@example.test");
    let user_id = sqlx::query_scalar!(
        r"
        INSERT INTO users (email, username, display_name)
        VALUES ($1, $2, $3)
        RETURNING id
        ",
        email,
        username,
        "Docket Test",
    )
    .fetch_one(pool)
    .await
    .expect("seed user");

    let slug = format!("docket-{label}-{}", &suffix[..12]);
    let bear_id = sqlx::query_scalar!(
        r"
        INSERT INTO bears (slug, name, description)
        VALUES ($1, $2, $3)
        RETURNING id
        ",
        slug,
        "Docket Test Bear",
        "integration test bear",
    )
    .fetch_one(pool)
    .await
    .expect("seed bear");

    let surface_id = test_work_surface_id(bear_id);
    let surface_name = format!("surface-{}", &surface_id.simple().to_string()[..12]);
    sqlx::query!(
        r"
        INSERT INTO work_surfaces (id, name, kind, created_by_user_id, created_at, updated_at)
        VALUES ($1, $2, 'git_workspace', $3, NOW(), NOW())
        ",
        surface_id,
        surface_name,
        user_id,
    )
    .execute(pool)
    .await
    .expect("seed work surface");
    sqlx::query!(
        r"
        INSERT INTO git_work_surface_details (id, upstream_url)
        VALUES ($1, $2)
        ",
        surface_id,
        "https://example.test/docket.git",
    )
    .execute(pool)
    .await
    .expect("seed git work surface details");
    sqlx::query!(
        r"
        INSERT INTO work_surface_bears (surface_id, bear_id)
        VALUES ($1, $2)
        ",
        surface_id,
        bear_id,
    )
    .execute(pool)
    .await
    .expect("assign work surface");

    (user_id, bear_id)
}

pub(super) fn two_task_job(user_id: i32, bear_id: Uuid) -> DocketJobCreate {
    DocketJobCreate {
        bear_id,
        created_by_user_id: user_id,
        created_by_role: "pair".to_string(),
        goal: format!("Docket integration lifecycle {}", Uuid::new_v4().simple()),
        work_surface_id: Some(test_work_surface_id(bear_id)),
        work_surface_assignments: vec![],
        commit_policy: Some(DocketCommitPolicy::None),
        work_branch: None,
        visibility: TaskListVisibility::SameUser,
        source_conversation_id: None,
        objective_kind: None,
        supersedes_job_id: None,
        overlap_resolution: DocketJobOverlapResolution::Reject,
        criteria: vec![DocketJobCriterionInput {
            kind: DocketCriterionKind::Narrative,
            description: "Both tasks are done".to_string(),
            spec: None,
            sibling_order: 0,
        }],
        tasks: vec![
            DocketTaskInput {
                client_key: Some("first".to_string()),
                parent_client_key: None,
                parent_task_id: None,
                sibling_order: Some(0),
                kind: DocketTaskKind::Execution,
                scope: DocketTaskScope::Template,
                title: "First task".to_string(),
                body: "Do first task".to_string(),
                completion_criteria: vec!["First task is actually done".to_string()],
                difficulty: Some(DocketTaskDifficulty::Trivial),
                effort_hint: Some(DocketEffortHint::Low),
                routing_strategy: RoutingStrategy::Auto,
                expected_context_size: None,
                result_rollup_policy: None,
            },
            DocketTaskInput {
                client_key: Some("second".to_string()),
                parent_client_key: None,
                parent_task_id: None,
                sibling_order: Some(1),
                kind: DocketTaskKind::Execution,
                scope: DocketTaskScope::Template,
                title: "Second task".to_string(),
                body: "Do second task".to_string(),
                completion_criteria: vec!["Second task is actually done".to_string()],
                difficulty: Some(DocketTaskDifficulty::Trivial),
                effort_hint: Some(DocketEffortHint::Low),
                routing_strategy: RoutingStrategy::Auto,
                expected_context_size: None,
                result_rollup_policy: None,
            },
        ],
    }
}

#[tokio::test]
async fn creates_session_anchored_task_without_job() {
    let Some(pool) = test_pool().await else {
        eprintln!("skipping postgres-backed docket integration test; database unavailable");
        return;
    };
    let (user_id, bear_id) = seed_user_and_bear(&pool, "session-task").await;
    let service = PgDocketService::from_pool(&pool);
    let session_anchor_id = sqlx::query_scalar!(
        r"
        INSERT INTO client_sessions (
            user_id, bear_id, bear_slug, client_session_id, runtime_session_id, conversation_id, client
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7)
        RETURNING id
        ",
        user_id,
        bear_id,
        "docket-session-task",
        "session-task-client",
        "session-task-runtime",
        "session-task-conversation",
        "test",
    )
    .fetch_one(&pool)
    .await
    .expect("seed client session");

    let task = service
        .create_task(DocketTaskCreate {
            bear_id,
            job_id: None,
            session_anchor_id: Some(session_anchor_id),
            parent_task_id: None,
            sibling_order: 0,
            placement: None,
            kind: DocketTaskKind::Investigation,
            scope: DocketTaskScope::Run,
            title: "Session anchored task".to_string(),
            body: "Confirm jobless task creation works".to_string(),
            completion_criteria: vec!["Task row is inserted".to_string()],
            difficulty: Some(DocketTaskDifficulty::Trivial),
            effort_hint: Some(DocketEffortHint::Low),
            routing_strategy: RoutingStrategy::Auto,
            expected_context_size: None,
            result_rollup_policy: None,
            created_by_role: "pair".to_string(),
            created_by_user_id: Some(user_id),
            created_by_agent_id: None,
            created_in_run_id: None,
        })
        .await
        .expect("create session-anchored task");

    assert_eq!(task.job_id, None);
    assert!(service
        .list_session_tasks(bear_id, session_anchor_id)
        .await
        .expect("list attached tasks")
        .iter()
        .any(|projection| projection.task.id == task.id));
    assert_eq!(task.body, "Confirm jobless task creation works");
    assert_eq!(task.completion_criteria.0, vec!["Task row is inserted"]);

    let settled = service
        .settle_session_task(DocketSessionTaskSettlement {
            bear_id,
            session_anchor_id,
            task_id: task.id,
            status: DocketTaskStatus::Done,
            outcome_disposition: None,
            result_refs: None,
            result_summary: Some("Verified session-owned settlement.".to_string()),
            actor_role: BearProfile::Pair,
            actor_user_id: Some(user_id),
            actor_agent_id: None,
        })
        .await
        .expect("settle session-anchored task");
    let entry_id = settled
        .task
        .settled_by_entry_id
        .expect("task points to its settlement entry");
    let outcome = sqlx::query!(
        "SELECT job_id, run_id, scope, kind FROM bear_docket_entries WHERE id = $1",
        entry_id,
    )
    .fetch_one(&pool)
    .await
    .expect("read settlement entry");
    assert_eq!(outcome.job_id, None);
    assert_eq!(outcome.run_id, None);
    assert_eq!(outcome.scope, "task_journal");
    assert_eq!(outcome.kind, "outcome");

    let other_session_id = Uuid::new_v4();
    let error = service
        .settle_session_task(DocketSessionTaskSettlement {
            bear_id,
            session_anchor_id: other_session_id,
            task_id: task.id,
            status: DocketTaskStatus::Cancelled,
            outcome_disposition: None,
            result_refs: None,
            result_summary: Some("Cross-session attempt.".to_string()),
            actor_role: BearProfile::Pair,
            actor_user_id: Some(user_id),
            actor_agent_id: None,
        })
        .await
        .expect_err("cross-session settlement must fail");
    assert!(error.to_string().contains("current session"));
}

#[tokio::test]
async fn lists_session_anchored_task_with_latest_run_state() {
    let Some(pool) = test_pool().await else {
        eprintln!("skipping postgres-backed docket integration test; database unavailable");
        return;
    };
    let (user_id, bear_id) = seed_user_and_bear(&pool, "session-task-state").await;
    let service = PgDocketService::from_pool(&pool);
    let session_anchor_id = sqlx::query_scalar!(
        r"
        INSERT INTO client_sessions (
            user_id, bear_id, bear_slug, client_session_id, runtime_session_id, conversation_id, client
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7)
        RETURNING id
        ",
        user_id,
        bear_id,
        "docket-session-task-state",
        "session-task-state-client",
        "session-task-state-runtime",
        "session-task-state-conversation",
        "test",
    )
    .fetch_one(&pool)
    .await
    .expect("seed client session");
    let job = service
        .create_job(DocketJobCreate {
            tasks: vec![],
            ..two_task_job(user_id, bear_id)
        })
        .await
        .expect("create run source job");
    let run_id = job.job.current_run_id.expect("current run");
    let task = service
        .create_task(DocketTaskCreate {
            bear_id,
            job_id: None,
            session_anchor_id: Some(session_anchor_id),
            parent_task_id: None,
            sibling_order: 0,
            placement: None,
            kind: DocketTaskKind::Execution,
            scope: DocketTaskScope::Run,
            title: "Session task with state".to_string(),
            body: "Confirm session task status projection works".to_string(),
            completion_criteria: vec!["Task status is projected".to_string()],
            difficulty: Some(DocketTaskDifficulty::Trivial),
            effort_hint: Some(DocketEffortHint::Low),
            routing_strategy: RoutingStrategy::Auto,
            expected_context_size: None,
            result_rollup_policy: None,
            created_by_role: "pair".to_string(),
            created_by_user_id: Some(user_id),
            created_by_agent_id: None,
            created_in_run_id: None,
        })
        .await
        .expect("create session task");
    service
        .update_task(DocketTaskUpdate {
            bear_id,
            job_id: None,
            task_id: task.id,
            actor_role: BearProfile::Pair,
            actor_user_id: Some(user_id),
            actor_agent_id: None,
            definition: DocketTaskDefinitionPatch::default(),
            run_state: Some(DocketTaskRunStateUpdate {
                run_id,
                status: DocketTaskStatus::Done,
                outcome_disposition: None,
                result_refs: Some(primary_output_result_refs()),
                result_summary: Some("Verified status projection".to_string()),
            }),
        })
        .await
        .expect("mark session task done");

    let tasks = service
        .list_tasks(
            bear_id,
            DocketTaskListFilter {
                session_anchor_id: Some(session_anchor_id),
                include_descendants: true,
                ..DocketTaskListFilter::default()
            },
        )
        .await
        .expect("list session task tree");

    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0].task.id, task.id);
    assert_eq!(
        tasks[0]
            .run_state
            .as_ref()
            .map(|state| state.status.as_str()),
        Some("done")
    );
}

#[tokio::test]
async fn session_task_attachment_reassignment_fences_stale_owner_and_releases_on_settlement() {
    let Some(pool) = test_pool().await else {
        eprintln!("skipping postgres-backed docket integration test; database unavailable");
        return;
    };
    let (user_id, bear_id) = seed_user_and_bear(&pool, "pair-attachment").await;
    let first_session = seed_client_session(&pool, user_id, bear_id).await;
    let second_session = seed_client_session(&pool, user_id, bear_id).await;
    let service = PgDocketService::from_pool(&pool);
    let created = service
        .create_job(two_task_job(user_id, bear_id))
        .await
        .expect("create durable job");
    let task_id = created.tasks[0].id;

    service
        .checkout_task_list(
            bear_id,
            BearProfile::Pair,
            user_id,
            crate::TaskListCheckoutRequest {
                source: crate::TaskListCheckoutSource::DocketJob {
                    job_id: created.job.id,
                    parent_task_id: None,
                },
                session_anchor_id: Some(first_session),
            },
        )
        .await
        .expect("attach durable job to first client session");
    assert!(service
        .list_session_tasks(bear_id, first_session)
        .await
        .expect("project first client session")
        .iter()
        .any(|task| task.task.id == task_id));
    service
        .attach_task_to_session(bear_id, task_id, second_session)
        .await
        .expect("same Bear can reassign a durable task to another client session");
    assert!(service
        .list_session_tasks(bear_id, second_session)
        .await
        .expect("project reassigned client session")
        .iter()
        .any(|task| task.task.id == task_id));
    assert!(service
        .list_session_tasks(bear_id, first_session)
        .await
        .expect("project stale client session")
        .iter()
        .all(|task| task.task.id != task_id));

    let stale_settlement = DocketSessionTaskSettlement {
        bear_id,
        task_id,
        session_anchor_id: first_session,
        status: DocketTaskStatus::Done,
        outcome_disposition: Some(crate::DocketOutcomeDisposition::Completed),
        result_summary: Some("Session task completed.".to_string()),
        result_refs: None,
        actor_role: BearProfile::Pair,
        actor_user_id: Some(user_id),
        actor_agent_id: None,
    };
    assert!(matches!(
        service.settle_session_task(stale_settlement.clone()).await,
        Err(DenError::ValidationError(message))
            if message == "session task settlement requires a task attached to the current session"
    ));

    service
        .settle_session_task(DocketSessionTaskSettlement {
            session_anchor_id: second_session,
            ..stale_settlement
        })
        .await
        .expect("settle attached task releases current session attachment");
    assert!(service
        .list_session_tasks(bear_id, second_session)
        .await
        .expect("project released client session")
        .iter()
        .all(|task| task.task.id != task_id));
}

#[tokio::test]
async fn bear_can_cancel_orphaned_docket_run_and_release_its_pair_claim() {
    let Some(pool) = test_pool().await else {
        eprintln!("skipping postgres-backed docket integration test; database unavailable");
        return;
    };
    let (user_id, bear_id) = seed_user_and_bear(&pool, "cancel-orphaned-run").await;
    let service = PgDocketService::from_pool(&pool);
    let created = service
        .create_job(two_task_job(user_id, bear_id))
        .await
        .expect("create durable job");

    let session_id = format!("defunct-pair-session-{}", Uuid::new_v4());
    service
        .authorize_execution_attempt(DocketExecutionAttemptAuthorize {
            bear_id,
            task_id: created.tasks[0].id,
            binding: DocketFocusedExecutionBinding {
                kind: DocketExecutionBindingKind::ClientSession,
                id: session_id.clone(),
            },
            host: DocketExecutionHost {
                kind: DocketExecutionHostKind::TurnRun,
                run_id: "defunct-turn-run".to_string(),
            },
            authorization_key: Uuid::new_v4(),
        })
        .await
        .expect("create original Pair execution claim");
    let live_attempt = sqlx::query_scalar!(
        "SELECT EXISTS(SELECT 1 FROM docket_execution_attempts WHERE bear_id = $1 AND binding_id = $2 AND state IN ('authorized', 'running', 'paused', 'awaiting_user', 'stopping')) AS \"exists!: bool\"",
        bear_id,
        &session_id,
    )
    .fetch_one(&pool)
    .await
    .expect("read original Pair execution claim");
    assert!(live_attempt);

    let cancelled = service
        .cancel_job_run(bear_id, created.job.id)
        .await
        .expect("same Bear cancels orphaned Docket run");
    assert_eq!(
        cancelled.current_run.as_ref().map(|run| run.state.as_str()),
        Some("cancelled")
    );
    let claim_released = sqlx::query_scalar!(
        "SELECT NOT EXISTS(SELECT 1 FROM docket_execution_attempts WHERE bear_id = $1 AND binding_id = $2 AND state IN ('authorized', 'running', 'paused', 'awaiting_user', 'stopping')) AS \"released!: bool\"",
        bear_id,
        &session_id,
    )
    .fetch_one(&pool)
    .await
    .expect("read released Pair execution claim");
    assert!(claim_released);

    let resumed = service
        .execute_job(DocketJobExecuteRequest {
            bear_id,
            job_id: created.job.id,
            actor_role: BearProfile::Pair,
            actor_user_id: Some(user_id),
            actor_agent_id: None,
            session_id: None,
            source_conversation_id: None,
            source_client_session_id: None,
        })
        .await
        .expect("same Bear resumes after cancelling orphaned Docket run");
    assert_ne!(
        resumed.job.job.current_run_id, created.job.current_run_id,
        "resuming must use a fresh run rather than revive cancelled evidence"
    );
    assert_eq!(resumed.job.job.status, "ready");
    assert_eq!(
        resumed
            .job
            .current_run
            .as_ref()
            .map(|run| run.state.as_str()),
        Some("running")
    );
    assert_eq!(resumed.selected_task_id, Some(created.tasks[0].id));

    let other_bear = seed_user_and_bear(&pool, "other-bear-cannot-cancel")
        .await
        .1;
    assert!(service
        .cancel_job_run(other_bear, created.job.id)
        .await
        .is_err());
}

#[tokio::test]
async fn docket_pair_lifecycle_completes_after_tasks_and_criteria() {
    let Some(pool) = test_pool().await else {
        eprintln!("skipping postgres-backed docket integration test; database unavailable");
        return;
    };
    let (user_id, bear_id) = seed_user_and_bear(&pool, "lifecycle").await;
    let service = PgDocketService::from_pool(&pool);

    let created = service
        .create_job(two_task_job(user_id, bear_id))
        .await
        .expect("create job");
    let run_id = created.job.current_run_id.expect("current run");
    let criterion_id = created.criteria[0].id;
    let first_task_id = created.tasks[0].id;
    let second_task_id = created.tasks[1].id;

    let finding = service
        .append_entry(DocketEntryCreate {
            bear_id,
            job_id: Some(created.job.id),
            task_id: Some(first_task_id),
            run_id: Some(run_id),
            scope: DocketEntryScope::TaskJournal,
            kind: DocketEntryKind::Finding,
            summary: "Inventory contains two runnable tasks.".to_string(),
            body: None,
            evidence_refs: vec![],
            related_task_ids: vec![second_task_id],
            tags: vec!["inventory".to_string()],
            actor_role: BearProfile::Pair,
            actor_user_id: Some(user_id),
            actor_agent_id: None,
        })
        .await
        .expect("append task finding");
    assert_eq!(finding.kind, "finding");
    let journal = service
        .list_entries(
            bear_id,
            DocketEntryListFilter {
                job_id: Some(created.job.id),
                task_id: Some(first_task_id),
                limit: 10,
            },
        )
        .await
        .expect("list task journal");
    assert_eq!(journal.len(), 1);
    assert_eq!(journal[0].id, finding.id);

    let promoted = service
        .promote_entry(DocketEntryPromotion {
            bear_id,
            entry_id: finding.id,
            actor_role: BearProfile::Pair,
            actor_user_id: Some(user_id),
            actor_agent_id: None,
        })
        .await
        .expect("promote finding");
    assert_eq!(promoted.scope, "job_notebook");
    assert_eq!(promoted.source_entry_id, Some(finding.id));
    let retried = service
        .promote_entry(DocketEntryPromotion {
            bear_id,
            entry_id: finding.id,
            actor_role: BearProfile::Pair,
            actor_user_id: Some(user_id),
            actor_agent_id: None,
        })
        .await
        .expect("retry finding promotion");
    assert_eq!(retried.id, promoted.id);
    let notebook = service
        .list_entries(
            bear_id,
            DocketEntryListFilter {
                job_id: Some(created.job.id),
                task_id: None,
                limit: 10,
            },
        )
        .await
        .expect("list job notebook");
    assert_eq!(
        notebook
            .iter()
            .filter(|entry| entry.source_entry_id == Some(finding.id))
            .count(),
        1
    );

    let first = service
        .execute_job(DocketJobExecuteRequest {
            bear_id,
            job_id: created.job.id,
            actor_role: BearProfile::Pair,
            actor_user_id: Some(user_id),
            actor_agent_id: None,
            session_id: Some("pair-integration-session".to_string()),
            source_conversation_id: None,
            source_client_session_id: Some("pair-integration-session".to_string()),
        })
        .await
        .expect("execute first");
    assert_eq!(first.selected_task_id, Some(first_task_id));
    assert_eq!(first.control.task.selected_task_id, Some(first_task_id));
    assert_eq!(first.control.task.focused_task_id, Some(first_task_id));
    assert_eq!(first.control.task.claimed_task_id, Some(first_task_id));
    assert_eq!(first.control.task.current_task_id, Some(first_task_id));
    assert!(matches!(
        first.control.next_action,
        crate::DocketExecutionNextAction::WorkCurrentTask
    ));
    assert!(first.control.retryable);
    assert!(first.control.reason.is_none());
    assert_eq!(first.job.job.status, "running");
    // Selecting a task establishes only objective context. Pair execution authority
    // starts later through the fenced canonical-attempt gate.
    assert!(
        live_session_attempt(&pool, bear_id, "pair-integration-session")
            .await
            .is_none()
    );

    let focus_event_count = sqlx::query_scalar!(
        r"
        SELECT count(*)
        FROM bear_job_events
        WHERE job_id = $1
          AND run_id = $2
          AND task_id = $3
          AND event_type = 'focus_selected'
          AND payload->>'session_id' = 'pair-integration-session'
          AND payload->>'source_client_session_id' = 'pair-integration-session'
          AND payload->>'state' = 'active'
        ",
        created.job.id,
        run_id,
        first_task_id,
    )
    .fetch_one(&pool)
    .await
    .expect("query focus event");
    assert_eq!(focus_event_count, Some(1));

    let task_definition_count = sqlx::query_scalar!(
        r"
        SELECT count(*)
        FROM bear_task_events
        WHERE task_id = $1
          AND run_id = $2
          AND event_type = 'created'
          AND payload->'definition'->>'title' = 'First task'
          AND payload->'definition'->>'body' = 'Do first task'
          AND payload->'definition'->'completion_criteria'->>0 = 'First task is actually done'
        ",
        first_task_id,
        run_id,
    )
    .fetch_one(&pool)
    .await
    .expect("query task definition event");
    assert_eq!(task_definition_count, Some(1));

    let report_only_completion = service
        .update_task(DocketTaskUpdate {
            bear_id,
            job_id: None,
            task_id: first_task_id,
            actor_role: BearProfile::Pair,
            actor_user_id: Some(user_id),
            actor_agent_id: None,
            definition: DocketTaskDefinitionPatch::default(),
            run_state: Some(DocketTaskRunStateUpdate {
                run_id,
                status: DocketTaskStatus::Done,
                outcome_disposition: None,
                result_refs: None,
                result_summary: Some("Inventory findings recorded in the task result.".to_string()),
            }),
        })
        .await;
    assert!(report_only_completion.is_ok());

    let outcome = sqlx::query!(
        r"
        SELECT entry.id, entry.summary, entry.disposition,
               jsonb_array_length(entry.evidence_refs) AS evidence_count,
               task.settled_by_entry_id
        FROM bear_docket_entries entry
        JOIN bear_tasks task ON task.id = entry.task_id
        WHERE entry.task_id = $1 AND entry.run_id = $2 AND entry.kind = 'outcome'
        ORDER BY entry.created_at DESC
        LIMIT 1
        ",
        first_task_id,
        run_id,
    )
    .fetch_one(&pool)
    .await
    .expect("query terminal outcome journal entry");
    let outcome_summary = outcome.summary;
    let outcome_disposition = outcome.disposition;
    let evidence_count = outcome.evidence_count;
    assert_eq!(outcome.settled_by_entry_id, Some(outcome.id));
    assert_eq!(
        outcome_summary,
        "Inventory findings recorded in the task result."
    );
    assert_eq!(outcome_disposition.as_deref(), Some("completed"));
    assert_eq!(evidence_count, Some(0));

    let identical_retry = service
        .update_task(DocketTaskUpdate {
            bear_id,
            job_id: None,
            task_id: first_task_id,
            actor_role: BearProfile::Pair,
            actor_user_id: Some(user_id),
            actor_agent_id: None,
            definition: DocketTaskDefinitionPatch::default(),
            run_state: Some(DocketTaskRunStateUpdate {
                run_id,
                status: DocketTaskStatus::Done,
                outcome_disposition: None,
                result_refs: None,
                result_summary: Some("Inventory findings recorded in the task result.".to_string()),
            }),
        })
        .await;
    assert!(identical_retry.is_ok());
    let outcome_count = sqlx::query_scalar!(
        "SELECT count(*) FROM bear_docket_entries WHERE task_id = $1 AND run_id = $2 AND kind = 'outcome'",
        first_task_id,
        run_id,
    )
    .fetch_one(&pool)
    .await
    .expect("count terminal outcomes after retry");
    assert_eq!(outcome_count, Some(1));

    let replacement_without_reopen = service
        .update_task(DocketTaskUpdate {
            bear_id,
            job_id: None,
            task_id: first_task_id,
            actor_role: BearProfile::Pair,
            actor_user_id: Some(user_id),
            actor_agent_id: None,
            definition: DocketTaskDefinitionPatch::default(),
            run_state: Some(DocketTaskRunStateUpdate {
                run_id,
                status: DocketTaskStatus::Done,
                outcome_disposition: None,
                result_refs: None,
                result_summary: Some("Changed after settlement.".to_string()),
            }),
        })
        .await;
    assert!(replacement_without_reopen.is_err());

    service
        .update_task(DocketTaskUpdate {
            bear_id,
            job_id: None,
            task_id: first_task_id,
            actor_role: BearProfile::Pair,
            actor_user_id: Some(user_id),
            actor_agent_id: None,
            definition: DocketTaskDefinitionPatch::default(),
            run_state: Some(DocketTaskRunStateUpdate {
                run_id,
                status: DocketTaskStatus::Pending,
                outcome_disposition: None,
                result_refs: None,
                result_summary: None,
            }),
        })
        .await
        .expect("reopen settled task");
    service
        .update_task(DocketTaskUpdate {
            bear_id,
            job_id: None,
            task_id: first_task_id,
            actor_role: BearProfile::Pair,
            actor_user_id: Some(user_id),
            actor_agent_id: None,
            definition: DocketTaskDefinitionPatch::default(),
            run_state: Some(DocketTaskRunStateUpdate {
                run_id,
                status: DocketTaskStatus::Done,
                outcome_disposition: None,
                result_refs: None,
                result_summary: Some("Rechecked after reopening.".to_string()),
            }),
        })
        .await
        .expect("resettle reopened task");
    let outcome_count = sqlx::query_scalar!(
        "SELECT count(*) FROM bear_docket_entries WHERE task_id = $1 AND run_id = $2 AND kind = 'outcome'",
        first_task_id,
        run_id,
    )
    .fetch_one(&pool)
    .await
    .expect("count terminal outcomes after resettlement");
    assert_eq!(outcome_count, Some(2));

    let missing_summary = service
        .update_task(DocketTaskUpdate {
            bear_id,
            job_id: None,
            task_id: first_task_id,
            actor_role: BearProfile::Pair,
            actor_user_id: Some(user_id),
            actor_agent_id: None,
            definition: DocketTaskDefinitionPatch::default(),
            run_state: Some(DocketTaskRunStateUpdate {
                run_id,
                status: DocketTaskStatus::Done,
                outcome_disposition: None,
                result_refs: None,
                result_summary: None,
            }),
        })
        .await;
    assert!(missing_summary.is_err());

    service
        .update_task(DocketTaskUpdate {
            bear_id,
            job_id: None,
            task_id: first_task_id,
            actor_role: BearProfile::Pair,
            actor_user_id: Some(user_id),
            actor_agent_id: None,
            definition: DocketTaskDefinitionPatch::default(),
            run_state: Some(DocketTaskRunStateUpdate {
                run_id,
                status: DocketTaskStatus::Pending,
                outcome_disposition: None,
                result_refs: None,
                result_summary: None,
            }),
        })
        .await
        .expect("reopen task for lifecycle completion");

    service
        .update_task(DocketTaskUpdate {
            bear_id,
            job_id: None,
            task_id: first_task_id,
            actor_role: BearProfile::Pair,
            actor_user_id: Some(user_id),
            actor_agent_id: None,
            definition: DocketTaskDefinitionPatch::default(),
            run_state: Some(DocketTaskRunStateUpdate {
                run_id,
                status: DocketTaskStatus::Done,
                outcome_disposition: None,
                result_refs: None,
                result_summary: Some("First task actually completed".to_string()),
            }),
        })
        .await
        .expect("complete first");

    let second = service
        .execute_job(DocketJobExecuteRequest {
            bear_id,
            job_id: created.job.id,
            actor_role: BearProfile::Pair,
            actor_user_id: Some(user_id),
            actor_agent_id: None,
            session_id: Some("pair-integration-session".to_string()),
            source_conversation_id: None,
            source_client_session_id: Some("pair-integration-session".to_string()),
        })
        .await
        .expect("execute second");
    assert_eq!(second.selected_task_id, Some(second_task_id));

    service
        .update_task(DocketTaskUpdate {
            bear_id,
            job_id: None,
            task_id: second_task_id,
            actor_role: BearProfile::Pair,
            actor_user_id: Some(user_id),
            actor_agent_id: None,
            definition: DocketTaskDefinitionPatch::default(),
            run_state: Some(DocketTaskRunStateUpdate {
                run_id,
                status: DocketTaskStatus::Done,
                outcome_disposition: None,
                result_refs: None,
                result_summary: Some("Second task actually completed".to_string()),
            }),
        })
        .await
        .expect("complete second");

    // Direct task updates intentionally do not mutate the execution claim;
    // reconcile clears the completed task's stale Pair focus before checking criteria.
    service
        .reconcile_execution(DocketJobExecuteRequest {
            bear_id,
            job_id: created.job.id,
            actor_role: BearProfile::Pair,
            actor_user_id: Some(user_id),
            actor_agent_id: None,
            session_id: Some("pair-integration-session".to_string()),
            source_conversation_id: None,
            source_client_session_id: Some("pair-integration-session".to_string()),
        })
        .await
        .expect("reconcile completed task focus before criteria");

    // A criteria-only block must not retain a claim for a terminal task.
    assert!(
        live_session_attempt(&pool, bear_id, "pair-integration-session")
            .await
            .is_none()
    );

    let blocked = service
        .execute_job(DocketJobExecuteRequest {
            bear_id,
            job_id: created.job.id,
            actor_role: BearProfile::Pair,
            actor_user_id: Some(user_id),
            actor_agent_id: None,
            session_id: Some("pair-integration-session".to_string()),
            source_conversation_id: None,
            source_client_session_id: Some("pair-integration-session".to_string()),
        })
        .await
        .expect("blocked before criteria");
    assert!(blocked.blocked);
    assert_eq!(blocked.job.job.status, "ready");

    let evaluated = service
        .evaluate_criterion(DocketCriterionStateUpdate {
            bear_id,
            job_id: created.job.id,
            run_id,
            criterion_id,
            status: crate::DocketCriterionStatus::Met,
            evidence: Some(serde_json::json!({"summary":"Both tasks are done"})),
            actor_role: BearProfile::Pair,
            actor_user_id: Some(user_id),
            actor_agent_id: None,
        })
        .await
        .expect("evaluate criterion");
    assert_eq!(evaluated.criteria_states[0].status, "met");
    assert_eq!(evaluated.job.status, "completed");

    let completed = service
        .execute_job(DocketJobExecuteRequest {
            bear_id,
            job_id: created.job.id,
            actor_role: BearProfile::Pair,
            actor_user_id: Some(user_id),
            actor_agent_id: None,
            session_id: Some("pair-integration-session".to_string()),
            source_conversation_id: None,
            source_client_session_id: Some("pair-integration-session".to_string()),
        })
        .await
        .expect("complete job");
    assert!(completed.completed);
    assert_eq!(completed.job.job.status, "completed");
    assert_eq!(
        completed.job.current_run.as_ref().unwrap().state,
        "completed"
    );
    assert!(
        live_session_attempt(&pool, bear_id, "pair-integration-session")
            .await
            .is_none()
    );
}

#[tokio::test]
async fn pair_task_settlement_does_not_wait_for_per_job_commit_delivery() {
    let Some(pool) = test_pool().await else {
        eprintln!("skipping postgres-backed Pair settlement test; database unavailable");
        return;
    };
    let (user_id, bear_id) = seed_user_and_bear(&pool, "pair-settlement").await;
    let service = PgDocketService::from_pool(&pool);
    let mut create = two_task_job(user_id, bear_id);
    create.commit_policy = Some(DocketCommitPolicy::PerJob);
    let created = service.create_job(create).await.expect("create Pair job");
    let run_id = created.job.current_run_id.expect("current run");
    let task_id = created.tasks[0].id;

    let settled = service
        .update_task(DocketTaskUpdate {
            bear_id,
            job_id: Some(created.job.id),
            task_id,
            actor_role: BearProfile::Pair,
            actor_user_id: Some(user_id),
            actor_agent_id: None,
            definition: DocketTaskDefinitionPatch::default(),
            run_state: Some(DocketTaskRunStateUpdate {
                run_id,
                status: DocketTaskStatus::Done,
                outcome_disposition: None,
                result_refs: None,
                result_summary: Some(
                    "Declared complete; commit delivery is runtime-owned.".to_string(),
                ),
            }),
        })
        .await
        .expect("Pair settlement must not wait for per-job commit delivery");

    assert_eq!(
        settled
            .run_state
            .as_ref()
            .map(|state| state.status.as_str()),
        Some("done")
    );
    let outcome = sqlx::query!(
        "SELECT id, kind, scope FROM bear_docket_entries WHERE task_id = $1 AND run_id = $2 AND kind = 'outcome'",
        task_id,
        run_id,
    )
    .fetch_one(&pool)
    .await
    .expect("canonical Pair settlement outcome");
    assert_eq!(settled.task.settled_by_entry_id, Some(outcome.id));
    assert_eq!(outcome.kind, "outcome");
    assert_eq!(outcome.scope, "task_journal");

    let error = service
        .update_task(DocketTaskUpdate {
            bear_id,
            job_id: Some(created.job.id),
            task_id: created.tasks[1].id,
            actor_role: BearProfile::Pair,
            actor_user_id: Some(user_id),
            actor_agent_id: None,
            definition: DocketTaskDefinitionPatch::default(),
            run_state: Some(DocketTaskRunStateUpdate {
                run_id,
                status: DocketTaskStatus::Done,
                outcome_disposition: None,
                result_refs: Some(primary_output_result_refs()),
                result_summary: Some(
                    "Declared complete with explicit output evidence.".to_string(),
                ),
            }),
        })
        .await
        .expect_err("unregistered explicit primary output must remain rejected");
    assert!(error.to_string().contains("finalized Git commit artifact"));
}

#[tokio::test]
async fn docket_execution_focus_prefers_conversation_over_client_session() {
    let Some(pool) = test_pool().await else {
        eprintln!("skipping postgres-backed docket conversation focus test; database unavailable");
        return;
    };
    let (user_id, bear_id) = seed_user_and_bear(&pool, "conversation-focus").await;
    let service = PgDocketService::from_pool(&pool);
    let created = service
        .create_job(two_task_job(user_id, bear_id))
        .await
        .expect("create job");
    let first_task_id = created.tasks[0].id;
    let second_task_id = created.tasks[1].id;

    let selected = service
        .execute_job(DocketJobExecuteRequest {
            bear_id,
            job_id: created.job.id,
            actor_role: BearProfile::Pair,
            actor_user_id: Some(user_id),
            actor_agent_id: None,
            session_id: Some("adapter-session-1".to_string()),
            source_conversation_id: Some("conversation-1".to_string()),
            source_client_session_id: Some("client-session-1".to_string()),
        })
        .await
        .expect("execute first");
    assert_eq!(selected.selected_task_id, Some(first_task_id));
    assert_eq!(selected.job.active_task_ids, vec![first_task_id]);
    assert_eq!(selected.job.job.status, "running");
    assert_eq!(
        crate::docket_job_status_report(&selected.job).current_task_id,
        Some(first_task_id)
    );

    assert!(live_session_attempt(&pool, bear_id, "conversation-1")
        .await
        .is_none());

    let resumed = service
        .execute_job(DocketJobExecuteRequest {
            bear_id,
            job_id: created.job.id,
            actor_role: BearProfile::Pair,
            actor_user_id: Some(user_id),
            actor_agent_id: None,
            session_id: Some("adapter-session-2".to_string()),
            source_conversation_id: Some("conversation-1".to_string()),
            source_client_session_id: Some("client-session-2".to_string()),
        })
        .await
        .expect("execute after reconnect");
    assert!(matches!(
        resumed.control.next_action,
        crate::DocketExecutionNextAction::WorkCurrentTask
    ));

    let advanced = service
        .settle_execution_task(DocketExecutionTaskSettlement {
            execution: DocketJobExecuteRequest {
                bear_id,
                job_id: created.job.id,
                actor_role: BearProfile::Pair,
                actor_user_id: Some(user_id),
                actor_agent_id: None,
                session_id: Some("adapter-session-2".to_string()),
                source_conversation_id: Some("conversation-1".to_string()),
                source_client_session_id: Some("client-session-2".to_string()),
            },
            task_id: first_task_id,
            status: DocketTaskStatus::Done,
            outcome_disposition: None,
            result_refs: None,
            result_summary: Some("first task complete".to_string()),
        })
        .await
        .expect("settle and automatically advance focused task");
    assert!(matches!(
        advanced.control.next_action,
        crate::DocketExecutionNextAction::WorkCurrentTask
    ));
    assert_eq!(advanced.control.task.current_task_id, Some(second_task_id));
}

#[tokio::test]
async fn released_pair_attempt_can_settle_its_checked_out_run() {
    let Some(pool) = test_pool().await else {
        eprintln!(
            "skipping postgres-backed released-attempt settlement test; database unavailable"
        );
        return;
    };
    let (user_id, bear_id) = seed_user_and_bear(&pool, "released-attempt-settlement").await;
    let service = PgDocketService::from_pool(&pool);
    let created = service
        .create_job(two_task_job(user_id, bear_id))
        .await
        .expect("create job");
    let task_id = created.tasks[0].id;
    let request = DocketJobExecuteRequest {
        bear_id,
        job_id: created.job.id,
        actor_role: BearProfile::Pair,
        actor_user_id: Some(user_id),
        actor_agent_id: None,
        session_id: Some("released-attempt-session".to_string()),
        source_conversation_id: None,
        source_client_session_id: None,
    };
    let selected = service
        .execute_job(request.clone())
        .await
        .expect("check out task");
    let run_id = selected.job.current_run.expect("current run").id;

    sqlx::query!(
        "UPDATE docket_execution_attempts SET state = 'released', released_at = NOW() \
         WHERE bear_id = $1 AND task_id = $2 AND state IN ('authorized', 'running', 'paused', 'awaiting_user', 'stopping')",
        bear_id,
        task_id,
    )
    .execute(&pool)
    .await
    .expect("release Pair attempt");

    service
        .settle_execution_task(DocketExecutionTaskSettlement {
            execution: request,
            task_id,
            status: DocketTaskStatus::Done,
            outcome_disposition: None,
            result_refs: None,
            result_summary: Some("completed after Pair reconnect".to_string()),
        })
        .await
        .expect("released attempt may settle its checked-out run");
    let status = sqlx::query_scalar!(
        "SELECT status FROM bear_task_run_state WHERE run_id = $1 AND task_id = $2",
        run_id,
        task_id,
    )
    .fetch_one(&pool)
    .await
    .expect("task run state");
    assert_eq!(status, "done");
}

#[tokio::test]
async fn execute_job_reconciles_its_own_terminal_session_claim() {
    let Some(pool) = test_pool().await else {
        eprintln!("skipping postgres-backed stale-claim reconciliation test; database unavailable");
        return;
    };
    let (user_id, bear_id) = seed_user_and_bear(&pool, "terminal-session-claim").await;
    let service = PgDocketService::from_pool(&pool);
    let created = service
        .create_job(two_task_job(user_id, bear_id))
        .await
        .expect("create job");
    let first_task_id = created.tasks[0].id;
    let second_task_id = created.tasks[1].id;
    let request = DocketJobExecuteRequest {
        bear_id,
        job_id: created.job.id,
        actor_role: BearProfile::Pair,
        actor_user_id: Some(user_id),
        actor_agent_id: None,
        session_id: Some("terminal-session-claim".to_string()),
        source_conversation_id: None,
        source_client_session_id: None,
    };
    let selected = service
        .execute_job(request.clone())
        .await
        .expect("select task");
    let run_id = selected.job.current_run.expect("current run").id;

    service
        .update_task(DocketTaskUpdate {
            bear_id,
            job_id: Some(created.job.id),
            task_id: first_task_id,
            actor_role: BearProfile::Pair,
            actor_user_id: Some(user_id),
            actor_agent_id: None,
            definition: DocketTaskDefinitionPatch::default(),
            run_state: Some(DocketTaskRunStateUpdate {
                run_id,
                status: DocketTaskStatus::Done,
                outcome_disposition: None,
                result_refs: None,
                result_summary: Some("outcome persisted before focus handoff".to_string()),
            }),
        })
        .await
        .expect("persist terminal outcome without reconciliation");

    let recovered = service
        .execute_job(request)
        .await
        .expect("self-heal stale claim");
    assert_eq!(recovered.selected_task_id, Some(second_task_id));
    assert_eq!(recovered.control.task.current_task_id, Some(second_task_id));
    assert!(matches!(
        recovered.control.next_action,
        crate::DocketExecutionNextAction::WorkCurrentTask
    ));

    let retried = service
        .execute_job(DocketJobExecuteRequest {
            bear_id,
            job_id: created.job.id,
            actor_role: BearProfile::Pair,
            actor_user_id: Some(user_id),
            actor_agent_id: None,
            session_id: Some("terminal-session-claim".to_string()),
            source_conversation_id: None,
            source_client_session_id: None,
        })
        .await
        .expect("retry after stale-claim reconciliation");
    assert_eq!(retried.selected_task_id, Some(second_task_id));

    // Historical interrupted handoffs could retain a pending row after the
    // authoritative settlement entry. Re-execution repairs that skew without
    // reopening the settled task.
    sqlx::query!(
        "UPDATE bear_task_run_state SET status = 'pending', finished_at = NULL WHERE run_id = $1 AND task_id = $2",
        run_id,
        first_task_id,
    )
    .execute(&pool)
    .await
    .expect("create historical settled-state skew");
    service
        .execute_job(DocketJobExecuteRequest {
            bear_id,
            job_id: created.job.id,
            actor_role: BearProfile::Pair,
            actor_user_id: Some(user_id),
            actor_agent_id: None,
            session_id: Some("terminal-session-claim".to_string()),
            source_conversation_id: None,
            source_client_session_id: None,
        })
        .await
        .expect("reconcile historical settled-state skew");
    let repaired = sqlx::query_scalar!(
        "SELECT status FROM bear_task_run_state WHERE run_id = $1 AND task_id = $2",
        run_id,
        first_task_id,
    )
    .fetch_one(&pool)
    .await
    .expect("read repaired state");
    assert_eq!(repaired, "done");
}

#[tokio::test]
async fn docket_task_list_sync_rejects_completed_item_without_evidence() {
    let Some(pool) = test_pool().await else {
        eprintln!("skipping postgres-backed docket sync test; database unavailable");
        return;
    };
    let (user_id, bear_id) = seed_user_and_bear(&pool, "sync").await;
    let service = PgDocketService::from_pool(&pool);
    let created = service
        .create_job(two_task_job(user_id, bear_id))
        .await
        .expect("create job");
    let mut task_list = task_list_projection_from_docket_job(&created, None);
    task_list.owner_profile = "pair".to_string();
    task_list.items[0].status = crate::TaskListItemStatus::Completed;
    task_list.items[0].summary = Some(created.tasks[0].body.clone());
    assert_eq!(
        docket_task_status_from_task_list_item_status(task_list.items[0].status).as_str(),
        "done"
    );

    let outcome = service
        .sync_task_list(TaskListSyncRequest { task_list })
        .await
        .expect("sync outcome");
    assert!(!outcome.applied);
    assert!(!outcome.conflicts.is_empty());
    assert!(outcome.conflicts[0].contains("completion summary"));
}

#[tokio::test]
async fn mark_task_started_allows_each_job_first_pending_leaf() {
    let Some(pool) = test_pool().await else {
        eprintln!("skipping postgres-backed docket dispatcher test; database unavailable");
        return;
    };
    let (user_id, bear_id) = seed_user_and_bear(&pool, "dispatcher-multiple-jobs").await;
    let service = PgDocketService::from_pool(&pool);
    let first = service
        .create_job(two_task_job(user_id, bear_id))
        .await
        .expect("create first job");
    let second = service
        .create_job(two_task_job(user_id, bear_id))
        .await
        .expect("create second job");
    let second_task_id = second.tasks[0].id;
    let second_run_id = second.job.current_run_id.expect("second job run");

    let runnable = service
        .runnable_work_tasks(bear_id, 10)
        .await
        .expect("runnable work tasks");
    assert!(runnable
        .iter()
        .any(|task| task.task.id == first.tasks[0].id));
    assert!(runnable.iter().any(|task| task.task.id == second_task_id));

    let started = service
        .mark_task_started(bear_id, second_task_id, second_run_id, None)
        .await
        .expect("start the first pending leaf of the second job");
    assert_eq!(started.task.id, second_task_id);
}

#[tokio::test]
async fn docket_dispatcher_finds_starts_and_records_work_task_outcomes() {
    let Some(pool) = test_pool().await else {
        eprintln!("skipping postgres-backed docket dispatcher test; database unavailable");
        return;
    };
    let (user_id, bear_id) = seed_user_and_bear(&pool, "dispatcher").await;
    let service = PgDocketService::from_pool(&pool);
    let mut create = two_task_job(user_id, bear_id);
    create.tasks[1].parent_client_key = Some("first".to_string());

    let created = service.create_job(create).await.expect("create job");
    let run_id = created.job.current_run_id.expect("current run");
    let parent_task_id = created.tasks[0].id;
    let child_task_id = created.tasks[1].id;
    assert_eq!(created.tasks[1].parent_task_id, Some(parent_task_id));

    let runnable = service
        .runnable_work_tasks(bear_id, 10)
        .await
        .expect("runnable work tasks");
    assert_eq!(runnable.len(), 1);
    assert_eq!(runnable[0].task.id, child_task_id);

    let started = service
        .mark_task_started(
            bear_id,
            child_task_id,
            run_id,
            Some("dispatcher-test".to_string()),
        )
        .await
        .expect("mark started");
    assert_eq!(started.run_state.as_ref().unwrap().status, "in_progress");

    let done = service
        .record_task_success(
            bear_id,
            child_task_id,
            run_id,
            "work task completed".to_string(),
            Some(serde_json::json!({"artifact":"dispatcher-test"})),
            Some("dispatcher-test".to_string()),
        )
        .await
        .expect("record success");
    assert_eq!(done.run_state.as_ref().unwrap().status, "done");
    assert_eq!(
        done.run_state.as_ref().unwrap().result_summary.as_deref(),
        Some("work task completed")
    );

    let blocked = service
        .record_task_blocked(
            bear_id,
            child_task_id,
            run_id,
            "waiting for sandbox capability".to_string(),
            None,
            Some("dispatcher-test".to_string()),
        )
        .await
        .expect("record blocked");
    assert_eq!(blocked.run_state.as_ref().unwrap().status, "blocked");
    assert_eq!(
        blocked
            .run_state
            .as_ref()
            .unwrap()
            .result_summary
            .as_deref(),
        Some("waiting for sandbox capability")
    );
}

#[tokio::test]
async fn docket_execute_rejects_stale_later_active_task() {
    let Some(pool) = test_pool().await else {
        eprintln!("skipping postgres-backed stale active-task test; database unavailable");
        return;
    };
    let (user_id, bear_id) = seed_user_and_bear(&pool, "stale-active-task").await;
    let service = PgDocketService::from_pool(&pool);
    let created = service
        .create_job(two_task_job(user_id, bear_id))
        .await
        .expect("create job");
    let run_id = created.job.current_run_id.expect("current run");
    let phase_zero_id = created.tasks[0].id;
    let stale_phase_nine_id = created.tasks[1].id;

    // Simulate an active-task record written by the pre-ordering scheduler.
    // The public dispatcher rejects this transition now; direct SQL preserves
    // the historical stale-state case at the execute/resume boundary.
    sqlx::query!(
        "UPDATE bear_task_run_state SET status = 'in_progress', started_at = NOW() \
         WHERE run_id = $1 AND task_id = $2",
        run_id,
        stale_phase_nine_id,
    )
    .execute(&pool)
    .await
    .expect("seed stale later active task");

    let error = service
        .execute_job(DocketJobExecuteRequest {
            bear_id,
            job_id: created.job.id,
            actor_role: BearProfile::Pair,
            actor_user_id: Some(user_id),
            actor_agent_id: None,
            session_id: None,
            source_conversation_id: None,
            source_client_session_id: None,
        })
        .await
        .expect_err("resume rejects Phase 9 while Phase 0 is pending");
    assert!(error.to_string().contains("refusing stale active task"));

    let state = sqlx::query_scalar!(
        "SELECT status FROM bear_task_run_state WHERE run_id = $1 AND task_id = $2",
        run_id,
        phase_zero_id,
    )
    .fetch_optional(&pool)
    .await
    .expect("read Phase 0 state");
    assert_eq!(
        state.as_deref(),
        Some("pending"),
        "Phase 0 remains pending and was not silently skipped or started"
    );
}

#[tokio::test]
async fn docket_dispatcher_follows_depth_first_sibling_order() {
    let Some(pool) = test_pool().await else {
        eprintln!("skipping postgres-backed docket dispatcher test; database unavailable");
        return;
    };
    let (user_id, bear_id) = seed_user_and_bear(&pool, "dispatcher-order").await;
    let service = PgDocketService::from_pool(&pool);
    let mut create = two_task_job(user_id, bear_id);
    create.tasks[0].title = "Phase one".to_string();
    create.tasks[1].title = "Phase two".to_string();
    create.tasks[1].parent_client_key = None;

    let created = service.create_job(create).await.expect("create job");
    let run_id = created.job.current_run_id.expect("current run");
    let phase_one_id = created.tasks[0].id;
    let phase_two_id = created.tasks[1].id;
    let first_child = service
        .create_task(DocketTaskCreate {
            bear_id,
            job_id: Some(created.job.id),
            session_anchor_id: None,
            parent_task_id: Some(phase_one_id),
            sibling_order: 0,
            placement: None,
            kind: DocketTaskKind::Execution,
            scope: DocketTaskScope::Template,
            title: "Phase one, first step".to_string(),
            body: "Do the first step".to_string(),
            completion_criteria: vec!["First step done".to_string()],
            difficulty: None,
            effort_hint: None,
            routing_strategy: RoutingStrategy::Auto,
            expected_context_size: None,
            result_rollup_policy: None,
            created_by_role: "pair".to_string(),
            created_by_user_id: Some(user_id),
            created_by_agent_id: None,
            created_in_run_id: Some(run_id),
        })
        .await
        .expect("create first child");
    let second_child = service
        .create_task(DocketTaskCreate {
            bear_id,
            job_id: Some(created.job.id),
            session_anchor_id: None,
            parent_task_id: Some(phase_one_id),
            sibling_order: 1,
            placement: None,
            kind: DocketTaskKind::Execution,
            scope: DocketTaskScope::Template,
            title: "Phase one, second step".to_string(),
            body: "Do the second step".to_string(),
            completion_criteria: vec!["Second step done".to_string()],
            difficulty: None,
            effort_hint: None,
            routing_strategy: RoutingStrategy::Auto,
            expected_context_size: None,
            result_rollup_policy: None,
            created_by_role: "pair".to_string(),
            created_by_user_id: Some(user_id),
            created_by_agent_id: None,
            created_in_run_id: Some(run_id),
        })
        .await
        .expect("create second child");

    let runnable = service
        .runnable_work_tasks(bear_id, 10)
        .await
        .expect("first runnable task");
    assert_eq!(
        runnable.iter().map(|task| task.task.id).collect::<Vec<_>>(),
        vec![first_child.id]
    );

    service
        .mark_task_started(
            bear_id,
            first_child.id,
            run_id,
            Some("dispatcher-order".to_string()),
        )
        .await
        .expect("start first child");
    let error = service
        .mark_task_started(bear_id, phase_two_id, run_id, None)
        .await
        .expect_err("cannot directly start a later phase");
    assert!(error
        .to_string()
        .contains("not the first eligible pending leaf"));
    let runnable = service
        .runnable_work_tasks(bear_id, 10)
        .await
        .expect("active earlier work blocks later tasks");
    assert!(
        runnable.is_empty(),
        "the next phase must not be offered while the first child is in progress"
    );

    service
        .record_task_success(
            bear_id,
            first_child.id,
            run_id,
            "first step complete".to_string(),
            None,
            None,
        )
        .await
        .expect("complete first child");
    let runnable = service
        .runnable_work_tasks(bear_id, 10)
        .await
        .expect("second runnable task");
    assert_eq!(
        runnable.iter().map(|task| task.task.id).collect::<Vec<_>>(),
        vec![second_child.id]
    );

    service
        .record_task_success(
            bear_id,
            second_child.id,
            run_id,
            "second step complete".to_string(),
            None,
            None,
        )
        .await
        .expect("complete second child");
    let runnable = service
        .runnable_work_tasks(bear_id, 10)
        .await
        .expect("phase two runnable task");
    assert_eq!(
        runnable.iter().map(|task| task.task.id).collect::<Vec<_>>(),
        vec![phase_two_id]
    );
}

#[tokio::test]
async fn docket_completes_parent_after_children_are_terminal() {
    let Some(pool) = test_pool().await else {
        eprintln!("skipping postgres-backed docket parent completion test; database unavailable");
        return;
    };
    let (user_id, bear_id) = seed_user_and_bear(&pool, "parent-rollup").await;
    let service = PgDocketService::from_pool(&pool);
    let mut create = two_task_job(user_id, bear_id);
    create.criteria.clear();
    create.tasks[1].parent_client_key = Some("first".to_string());
    let created = service.create_job(create).await.expect("create job");
    let run_id = created.job.current_run_id.expect("current run");
    let parent_id = created.tasks[0].id;
    let child_id = created.tasks[1].id;

    let error = service
        .record_task_success(
            bear_id,
            parent_id,
            run_id,
            "phase complete".to_string(),
            None,
            None,
        )
        .await
        .expect_err("parent cannot complete before its child");
    assert!(error
        .to_string()
        .contains("child task(s) remain unfinished"));

    service
        .record_task_success(
            bear_id,
            child_id,
            run_id,
            "child complete".to_string(),
            None,
            None,
        )
        .await
        .expect("complete child");
    let projection = service
        .get_job(bear_id, created.job.id)
        .await
        .expect("load job")
        .expect("job exists");
    let parent_state = projection
        .task_states
        .iter()
        .find(|state| state.task_id == parent_id)
        .expect("parent run state");
    assert_eq!(parent_state.status.as_str(), "done");
    assert_eq!(
        parent_state.result_summary.as_deref(),
        Some("Completed automatically after all child tasks reached terminal states.")
    );

    let completed = service
        .execute_job(DocketJobExecuteRequest {
            bear_id,
            job_id: created.job.id,
            actor_role: BearProfile::Pair,
            actor_user_id: Some(user_id),
            actor_agent_id: None,
            session_id: Some("parent-rollup-session".to_string()),
            source_conversation_id: None,
            source_client_session_id: Some("parent-rollup-session".to_string()),
        })
        .await
        .expect("complete job after automatic parent roll-up");
    assert!(completed.completed);
    assert_eq!(completed.selected_task_id, None);
    assert_eq!(completed.job.job.status, "completed");
}

#[tokio::test]
async fn docket_completing_job_settles_current_run() {
    let Some(pool) = test_pool().await else {
        eprintln!("skipping postgres-backed docket run settlement test; database unavailable");
        return;
    };
    let (user_id, bear_id) = seed_user_and_bear(&pool, "terminal-run").await;
    let service = PgDocketService::from_pool(&pool);
    let created = service
        .create_job(DocketJobCreate {
            criteria: vec![DocketJobCriterionInput {
                description: "Both tasks complete".to_string(),
                kind: DocketCriterionKind::Narrative,
                sibling_order: 0,
                spec: None,
            }],
            ..two_task_job(user_id, bear_id)
        })
        .await
        .expect("create job");
    let run_id = created.job.current_run_id.expect("current run");

    service
        .record_task_success(
            bear_id,
            created.tasks[0].id,
            run_id,
            "first task complete".to_string(),
            None,
            None,
        )
        .await
        .expect("complete first task");
    service
        .record_task_success(
            bear_id,
            created.tasks[1].id,
            run_id,
            "second task complete".to_string(),
            None,
            None,
        )
        .await
        .expect("complete second task");
    let completed = service
        .get_job(bear_id, created.job.id)
        .await
        .expect("load ready job")
        .expect("job exists");
    let criterion_id = completed.criteria[0].id;
    service
        .evaluate_criterion(DocketCriterionStateUpdate {
            bear_id,
            job_id: created.job.id,
            run_id,
            criterion_id,
            status: crate::DocketCriterionStatus::Met,
            evidence: None,
            actor_role: BearProfile::Pair,
            actor_user_id: Some(user_id),
            actor_agent_id: None,
        })
        .await
        .expect("meet completion criterion");
    let completed = service
        .get_job(bear_id, created.job.id)
        .await
        .expect("load completed job")
        .expect("job exists");
    assert_eq!(completed.job.status, "completed");

    let run = sqlx::query!(
        "SELECT state, finished_at FROM bear_job_runs WHERE id = $1",
        run_id
    )
    .fetch_one(&pool)
    .await
    .expect("load settled run");
    assert_eq!(run.state, "completed");
    assert!(run.finished_at.is_some());
}

#[tokio::test]
async fn create_job_requires_explicit_resolution_for_exact_active_overlap() {
    let Some(pool) = test_pool().await else {
        eprintln!("skipping postgres-backed docket integration test; database unavailable");
        return;
    };
    let (user_id, bear_id) = seed_user_and_bear(&pool, "job-overlap").await;
    let service = PgDocketService::from_pool(&pool);
    let goal = format!("Exact overlap {}", Uuid::new_v4().simple());
    let create = |resolution, supersedes_job_id| DocketJobCreate {
        goal: goal.clone(),
        overlap_resolution: resolution,
        supersedes_job_id,
        tasks: Vec::new(),
        criteria: Vec::new(),
        ..two_task_job(user_id, bear_id)
    };

    let first = service
        .create_job(create(DocketJobOverlapResolution::Reject, None))
        .await
        .expect("create initial job");

    let duplicate = service
        .create_job(create(DocketJobOverlapResolution::Reject, None))
        .await
        .expect_err("exact active overlap is rejected by default");
    assert!(matches!(
        duplicate,
        den_core::DenError::ValidationError(message) if message.contains(&first.job.id.to_string())
    ));

    let independent = service
        .create_job(create(DocketJobOverlapResolution::Independent, None))
        .await
        .expect("explicit independent job");
    assert_eq!(independent.job.supersedes_job_id, None);

    let replacement = service
        .create_job(create(
            DocketJobOverlapResolution::Supersede,
            Some(independent.job.id),
        ))
        .await
        .expect("explicitly supersede the matching job");
    assert_eq!(replacement.job.supersedes_job_id, Some(independent.job.id));

    let predecessor = service
        .get_job(bear_id, independent.job.id)
        .await
        .expect("read predecessor")
        .expect("predecessor exists");
    assert_eq!(predecessor.job.status, "cancelled");
}
