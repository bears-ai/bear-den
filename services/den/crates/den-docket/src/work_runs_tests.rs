//! Postgres-backed tests for the work-run dispatch/claim/lease state machine.
//! Same convention as `integration_tests.rs`: skip when no database is
//! reachable.

use den_core::{BearProfile, DenError};
use sqlx::PgPool;
use uuid::Uuid;

use crate::db::get_job;
use crate::execution_profiles::{ProfileProvenance, ResolvedExecutionProfile};
use crate::recovery::{
    apply_supervisor_disposition, claim_turn_attempt, parent_rollup_context,
    persist_attention_outbox, persist_result_rollup, terminalize_turn_attempt, AttemptOutcome,
    ResultRollup, RetryDisposition, SupervisorDisposition,
};
use crate::supervisor::set_work_run_paused;
use crate::work_runs::{
    checkout_work_run_for_session, claim_next_work_run, disconnect_attached_work_run,
    enqueue_work_job, enqueue_work_run, ensure_job_work_branch, finalize_work_run,
    get_live_work_run_by_session, get_work_run, get_work_run_dispatch_context, heartbeat_work_run,
    reconnect_attached_work_run, record_work_run_provisioned, record_work_run_turn_outcome,
    recover_attached_work_run, request_work_run_cancel, request_work_run_cancel_with_provenance,
    timeout_disconnected_work_runs, WorkExecutionTarget, WorkJobEnqueue, WorkRunCancelRequest,
    WorkRunEnqueue, WorkRunFinalize, WorkRunProvisioned, WorkRunState,
};
use crate::{
    DocketCommitPolicy, DocketCriterionKind, DocketExecutionAttemptState, DocketExecutionBinding,
    DocketExecutionBindingKind, DocketExecutionDisposition, DocketExecutionGate,
    DocketExecutionHostKind, DocketExecutionReason, DocketJobCreate, DocketJobCriterionInput,
    DocketJobExecuteRequest, DocketJobOverlapResolution, DocketService, DocketTaskDefinitionPatch,
    DocketTaskDifficulty, DocketTaskInput, DocketTaskKind, DocketTaskRunStateUpdate,
    DocketTaskScope, DocketTaskUpdate, PgDocketService, RoutingStrategy, TaskListVisibility,
};

/// `claim_next_work_run` is deliberately global (any runner takes the oldest
/// claimable run), so tests in this module serialize on one lock: a parallel
/// test's queued run would otherwise satisfy another test's claim.
static DB_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Cancel claimable leftovers (queued, or non-terminal with an expired lease)
/// from earlier test runs so claim-order assertions see only this test's runs.
async fn purge_claimable_runs(pool: &PgPool) {
    sqlx::query!(
        "UPDATE bear_work_runs
         SET state = 'cancelled', runner_id = NULL, lease_expires_at = NULL,
             finished_at = now(), updated_at = now()
         WHERE state = 'queued'
            OR (state IN ('claimed', 'provisioning', 'running', 'reporting')
                AND lease_expires_at IS NOT NULL AND lease_expires_at < now())",
    )
    .execute(pool)
    .await
    .expect("purge claimable work runs");
}

async fn test_pool() -> Option<PgPool> {
    let database_url = std::env::var("TEST_DATABASE_URL")
        .or_else(|_| std::env::var("DATABASE_URL"))
        .unwrap_or_else(|_| "postgres://postgres:postgres@127.0.0.1/postgres".to_string());
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(4)
        .connect(&database_url)
        .await
        .ok()?;
    sqlx::migrate!("../../migrations").run(&pool).await.ok()?;
    Some(pool)
}

async fn seed_user_and_bear(pool: &PgPool, label: &str) -> (i32, Uuid) {
    let suffix = Uuid::new_v4().simple().to_string();
    let user_id = sqlx::query_scalar!(
        "INSERT INTO users (email, username, display_name) VALUES ($1, $2, $3) RETURNING id",
        format!("{label}-{suffix}@example.test"),
        format!("w{}", &suffix[..20]),
        "Work Run Test",
    )
    .fetch_one(pool)
    .await
    .expect("seed user");
    let bear_id = sqlx::query_scalar!(
        "INSERT INTO bears (slug, name, description) VALUES ($1, $2, $3) RETURNING id",
        format!("workrun-{label}-{}", &suffix[..12]),
        "Work Run Test Bear",
        "work run test bear",
    )
    .fetch_one(pool)
    .await
    .expect("seed bear");
    (user_id, bear_id)
}

fn work_task(title: &str, order: i32, _stance: BearProfile) -> DocketTaskInput {
    DocketTaskInput {
        client_key: Some(format!("k{order}")),
        parent_client_key: None,
        parent_task_id: None,
        sibling_order: Some(order),
        kind: DocketTaskKind::Execution,
        scope: DocketTaskScope::Template,
        title: title.to_string(),
        body: format!("Body of {title}"),
        completion_criteria: vec![format!("{title} is verifiably complete")],
        difficulty: Some(DocketTaskDifficulty::Trivial),
        effort_hint: None,
        routing_strategy: RoutingStrategy::Auto,
        expected_context_size: None,
        result_rollup_policy: None,
    }
}

/// Job with two work-assigned tasks and one pair task; returns
/// (job_id, [task ids in sibling order]).
async fn seed_work_job(pool: &PgPool, user_id: i32, bear_id: Uuid) -> (Uuid, Vec<Uuid>) {
    seed_work_job_with_policy(pool, user_id, bear_id, DocketCommitPolicy::PerTask).await
}

async fn seed_work_job_with_policy(
    pool: &PgPool,
    user_id: i32,
    bear_id: Uuid,
    commit_policy: DocketCommitPolicy,
) -> (Uuid, Vec<Uuid>) {
    let service = PgDocketService::from_pool(pool);
    let surface_id = Uuid::new_v4();
    let surface_name = format!("work-run-{}", Uuid::new_v4().simple());
    sqlx::query!(
        "INSERT INTO work_surfaces (id, name, kind, created_by_user_id, created_at, updated_at)
         VALUES ($1, $2, 'git_workspace', $3, now(), now())",
        surface_id,
        surface_name,
        user_id,
    )
    .execute(pool)
    .await
    .expect("seed work surface");
    sqlx::query!(
        "INSERT INTO git_work_surface_details (id, upstream_url)
         VALUES ($1, 'https://example.test/work-run.git')",
        surface_id,
    )
    .execute(pool)
    .await
    .expect("seed git work surface details");
    sqlx::query!(
        "INSERT INTO work_surface_bears (surface_id, bear_id) VALUES ($1, $2)",
        surface_id,
        bear_id,
    )
    .execute(pool)
    .await
    .expect("assign work surface");
    let created = service
        .create_job(DocketJobCreate {
            bear_id,
            created_by_user_id: user_id,
            created_by_role: "chat".to_string(),
            goal: format!("Ship the work-run slice {}", Uuid::new_v4().simple()),
            work_surface_id: Some(surface_id),
            work_surface_assignments: vec![],
            commit_policy: Some(commit_policy),
            work_branch: None,
            visibility: TaskListVisibility::SameUser,
            source_conversation_id: None,
            objective_kind: None,
            supersedes_job_id: None,
            overlap_resolution: DocketJobOverlapResolution::Reject,
            criteria: vec![DocketJobCriterionInput {
                kind: DocketCriterionKind::Narrative,
                description: "Everything is done".to_string(),
                spec: None,
                sibling_order: 0,
            }],
            tasks: vec![
                work_task("Alpha work task", 0, BearProfile::Work),
                work_task("Beta work task", 1, BearProfile::Work),
                work_task("Gamma pair task", 2, BearProfile::Pair),
            ],
        })
        .await
        .expect("create work job");
    let task_ids = created.tasks.iter().map(|task| task.id).collect();
    (created.job.id, task_ids)
}

fn enqueue_for(bear_id: Uuid, task_id: Uuid, user_id: i32) -> WorkRunEnqueue {
    WorkRunEnqueue {
        bear_id,
        task_id,
        root_name: Some("demo".to_string()),
        git_ref: None,
        image_name: None,
        requested_by_user_id: Some(user_id),
    }
}

#[tokio::test]
async fn checkout_rejects_terminal_selected_work_task() {
    let Some(pool) = test_pool().await else {
        eprintln!("skipping postgres-backed work_runs test; database unavailable");
        return;
    };
    let _guard = DB_LOCK.lock().await;
    purge_claimable_runs(&pool).await;
    let (user_id, bear_id) = seed_user_and_bear(&pool, "checkout-terminal-task").await;
    let (_job_id, task_ids) = seed_work_job(&pool, user_id, bear_id).await;
    let run = enqueue_work_run(&pool, enqueue_for(bear_id, task_ids[0], user_id))
        .await
        .expect("enqueue work run");
    claim_next_work_run(
        &pool,
        "runner-terminal-task",
        std::time::Duration::from_mins(1),
    )
    .await
    .expect("claim work run")
    .expect("queued run");
    record_work_run_provisioned(
        &pool,
        run.id,
        &WorkRunProvisioned {
            sandbox_server_url: "http://sandbox:3002".into(),
            sandbox_id: "terminal-task".into(),
            sandbox_type: "container".into(),
            sandbox_strength: "container: test".into(),
            work_surface: serde_json::json!({}),
            rust_dependency_preparation: None,
        },
    )
    .await
    .expect("provision work run");
    sqlx::query!(
        "INSERT INTO bear_task_run_state (run_id, task_id, status)
         VALUES ($1, $2, 'done')
         ON CONFLICT (run_id, task_id) DO UPDATE SET status = 'done'",
        run.job_run_id,
        task_ids[0],
    )
    .execute(&pool)
    .await
    .expect("settle selected task");
    sqlx::query!(
        "UPDATE bear_work_runs SET executing_task_id = $2 WHERE id = $1",
        run.id,
        task_ids[0],
    )
    .execute(&pool)
    .await
    .expect("retain stale Work task selection");

    let session_id = format!("headless-{}", Uuid::new_v4().simple());
    let checkout = checkout_work_run_for_session(&pool, run.id, bear_id, &session_id)
        .await
        .expect("terminal selected task is a scheduler result");
    assert!(matches!(
        checkout.gate,
        DocketExecutionGate::Rejected {
            reason: DocketExecutionReason::ActiveTaskIsStale,
            disposition: DocketExecutionDisposition::Reconcile,
        }
    ));
    assert!(checkout.prompt_context.is_none());
    assert!(checkout.task_title.is_none());
    let persisted = get_work_run(&pool, run.id)
        .await
        .expect("load work run")
        .expect("work run exists");
    assert_eq!(persisted.executing_task_id, Some(task_ids[0]));
    assert!(persisted.bearwire_session_id.is_none());

    let repeated_session_id = format!("headless-{}", Uuid::new_v4().simple());
    let repeated = checkout_work_run_for_session(&pool, run.id, bear_id, &repeated_session_id)
        .await
        .expect("repeated stale checkout is a scheduler result");
    assert!(matches!(
        repeated.gate,
        DocketExecutionGate::Rejected {
            reason: DocketExecutionReason::ActiveTaskIsStale,
            disposition: DocketExecutionDisposition::RequireIntervention,
        }
    ));
    let repeated_persisted = get_work_run(&pool, run.id)
        .await
        .expect("load work run")
        .expect("work run exists");
    assert_eq!(repeated_persisted.executing_task_id, Some(task_ids[0]));
    assert!(repeated_persisted.bearwire_session_id.is_none());
}

#[tokio::test]
async fn checkout_rejects_without_binding_when_no_task_is_actionable() {
    let Some(pool) = test_pool().await else {
        eprintln!("skipping postgres-backed work_runs test; database unavailable");
        return;
    };
    let _guard = DB_LOCK.lock().await;
    purge_claimable_runs(&pool).await;
    let (user_id, bear_id) = seed_user_and_bear(&pool, "checkout-rejected-gate").await;
    let (_job_id, task_ids) = seed_work_job(&pool, user_id, bear_id).await;
    let run = enqueue_work_run(&pool, enqueue_for(bear_id, task_ids[0], user_id))
        .await
        .expect("enqueue work run");
    claim_next_work_run(
        &pool,
        "runner-rejected-gate",
        std::time::Duration::from_mins(1),
    )
    .await
    .expect("claim work run")
    .expect("queued run");
    record_work_run_provisioned(
        &pool,
        run.id,
        &WorkRunProvisioned {
            sandbox_server_url: "http://sandbox:3002".into(),
            sandbox_id: "rejected-gate".into(),
            sandbox_type: "container".into(),
            sandbox_strength: "container: test".into(),
            work_surface: serde_json::json!({}),
            rust_dependency_preparation: None,
        },
    )
    .await
    .expect("provision work run");
    let session_id = format!("headless-{}", Uuid::new_v4().simple());
    let allowed_checkout = checkout_work_run_for_session(&pool, run.id, bear_id, &session_id)
        .await
        .expect("initial checkout creates the canonical Work attempt");
    assert!(matches!(
        allowed_checkout.gate,
        DocketExecutionGate::Allowed { .. }
    ));

    sqlx::query!(
        "INSERT INTO bear_task_run_state (run_id, task_id, status)
         SELECT $1, id, 'done' FROM bear_tasks WHERE job_id = $2
         ON CONFLICT (run_id, task_id) DO UPDATE SET status = 'done'",
        run.job_run_id,
        run.job_id,
    )
    .execute(&pool)
    .await
    .expect("settle all tasks");

    let checkout = checkout_work_run_for_session(&pool, run.id, bear_id, &session_id)
        .await
        .expect("rejected checkout is a scheduler result");
    assert!(matches!(
        checkout.gate,
        DocketExecutionGate::Rejected {
            reason: DocketExecutionReason::NoActionableTask,
            disposition: DocketExecutionDisposition::Stop,
        }
    ));
    assert!(checkout.prompt_context.is_none());
    assert!(checkout.task_title.is_none());
    let persisted = get_work_run(&pool, run.id)
        .await
        .expect("load work run")
        .expect("work run exists");
    assert!(persisted.executing_task_id.is_none());
    assert!(persisted.bearwire_session_id.is_none());
    let active_attempts: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM docket_execution_attempts
         WHERE binding_kind = 'work_assignment' AND binding_id = $1::text
           AND state IN ('authorized', 'running', 'paused', 'awaiting_user', 'stopping')",
    )
    .bind(run.id)
    .fetch_one(&pool)
    .await
    .expect("count canonical execution attempts");
    assert_eq!(active_attempts, 0);

    let second_checkout = checkout_work_run_for_session(&pool, run.id, bear_id, &session_id)
        .await
        .expect("repeated rejected checkout is a scheduler result");
    assert!(matches!(
        second_checkout.gate,
        DocketExecutionGate::Rejected {
            reason: DocketExecutionReason::NoActionableTask,
            disposition: DocketExecutionDisposition::RequireCheckpoint,
        }
    ));
    let directives: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM docket_checkpoint_directives d
         JOIN docket_execution_attempts a ON a.id = d.execution_attempt_id
         WHERE a.work_run_id = $1 AND d.state = 'pending'",
    )
    .bind(run.id)
    .fetch_one(&pool)
    .await
    .expect("count replayed checkpoint directives");
    assert_eq!(directives, 1, "repeated rejection replays one directive");
    assert!(second_checkout.prompt_context.is_none());
    assert!(second_checkout.task_title.is_none());

    sqlx::query!(
        "UPDATE bear_task_run_state SET status = 'pending' WHERE run_id = $1 AND task_id = $2",
        run.job_run_id,
        task_ids[0]
    )
    .execute(&pool)
    .await
    .expect("make one task actionable again");
    let allowed_checkout = checkout_work_run_for_session(&pool, run.id, bear_id, &session_id)
        .await
        .expect("fresh authorization clears the rejection streak");
    assert!(matches!(
        allowed_checkout.gate,
        DocketExecutionGate::Allowed { task_id, .. } if task_id == task_ids[0]
    ));
    let observations: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM bear_execution_rejection_observations WHERE work_run_id = $1",
    )
    .bind(run.id)
    .fetch_one(&pool)
    .await
    .expect("count cleared rejection observations");
    assert_eq!(observations, 0);
}
#[tokio::test]
async fn sandbox_repository_changes_without_publication_creates_no_run() {
    let _guard = DB_LOCK.lock().await;
    let Some(pool) = test_pool().await else {
        eprintln!("skipping postgres-backed work_runs test; database unavailable");
        return;
    };
    let (user_id, bear_id) = seed_user_and_bear(&pool, "sandbox-no-publication").await;
    let (job_id, _) = seed_work_job(&pool, user_id, bear_id).await;
    let before = sqlx::query_scalar!(
        "SELECT count(*) FROM bear_work_runs WHERE job_id = $1",
        job_id
    )
    .fetch_one(&pool)
    .await
    .expect("count runs before rejected dispatch");

    let error = enqueue_work_job(
        &pool,
        WorkJobEnqueue {
            bear_id,
            job_id,
            durable_result: crate::DurableResultKind::RepositoryChanges,
            git_ref: None,
            image_name: None,
            requested_by_user_id: Some(user_id),
            execution_target: WorkExecutionTarget::Sandbox,
            attachment_warning: None,
        },
    )
    .await
    .expect_err("sandbox source changes without publication must be rejected");

    assert!(error
        .to_string()
        .contains("repository_changes_without_publication"));
    let after = sqlx::query_scalar!(
        "SELECT count(*) FROM bear_work_runs WHERE job_id = $1",
        job_id
    )
    .fetch_one(&pool)
    .await
    .expect("count runs after rejected dispatch");
    assert_eq!(
        after, before,
        "rejected dispatch must not create a work run"
    );
}

#[tokio::test]
async fn forbidden_surface_assignment_rejects_mutable_dispatch() {
    let _guard = DB_LOCK.lock().await;
    let Some(pool) = test_pool().await else {
        eprintln!("skipping postgres-backed work_runs test; database unavailable");
        return;
    };
    let (user_id, bear_id) = seed_user_and_bear(&pool, "forbidden-surface-dispatch").await;
    let (job_id, _) = seed_work_job(&pool, user_id, bear_id).await;
    sqlx::query!(
        "UPDATE job_work_surface_assignments SET mutation_policy = 'forbidden' WHERE job_id = $1",
        job_id,
    )
    .execute(&pool)
    .await
    .expect("forbid the assigned work surface");

    let error = enqueue_work_job(
        &pool,
        WorkJobEnqueue {
            bear_id,
            job_id,
            durable_result: crate::DurableResultKind::RepositoryChanges,
            git_ref: None,
            image_name: None,
            requested_by_user_id: Some(user_id),
            execution_target: WorkExecutionTarget::AttachedArmature {
                client_session_id: format!("forbidden-{}", Uuid::new_v4().simple()),
            },
            attachment_warning: None,
        },
    )
    .await
    .expect_err("forbidden surface must reject mutable dispatch");

    assert!(error.to_string().contains("forbids mutation"));
}

/// Regression: `bear_jobs.status` is a derived projection and is not stored.
/// Dispatch must operate against the migrated schema without querying it.
#[tokio::test]
async fn dispatch_works_without_persisted_job_status() {
    let _guard = DB_LOCK.lock().await;
    let Some(pool) = test_pool().await else {
        eprintln!("skipping postgres-backed work_runs test; database unavailable");
        return;
    };
    let (user_id, bear_id) = seed_user_and_bear(&pool, "derived-job-status-dispatch").await;
    let (job_id, _) = seed_work_job(&pool, user_id, bear_id).await;

    let run = enqueue_work_job(
        &pool,
        WorkJobEnqueue {
            bear_id,
            job_id,
            durable_result: crate::DurableResultKind::RepositoryChanges,
            git_ref: None,
            image_name: None,
            requested_by_user_id: Some(user_id),
            execution_target: WorkExecutionTarget::AttachedArmature {
                client_session_id: format!("derived-status-{}", Uuid::new_v4().simple()),
            },
            attachment_warning: None,
        },
    )
    .await
    .expect("dispatch against migrated derived-status schema");

    assert!(!run.is_empty());
}

#[tokio::test]
async fn attached_disconnect_reconnect_and_timeout_are_idempotent() {
    let _guard = DB_LOCK.lock().await;
    let Some(pool) = test_pool().await else {
        eprintln!("skipping postgres-backed work_runs test; database unavailable");
        return;
    };
    let (user_id, bear_id) = seed_user_and_bear(&pool, "attached-lifecycle").await;
    let (job_id, _) = seed_work_job(&pool, user_id, bear_id).await;
    let session_id = format!("attached-{}", Uuid::new_v4().simple());
    let run = enqueue_work_job(
        &pool,
        WorkJobEnqueue {
            bear_id,
            job_id,
            durable_result: crate::DurableResultKind::RepositoryChanges,
            git_ref: None,
            image_name: None,
            requested_by_user_id: Some(user_id),
            execution_target: WorkExecutionTarget::AttachedArmature {
                client_session_id: session_id.clone(),
            },
            attachment_warning: None,
        },
    )
    .await
    .expect("enqueue attached run")
    .remove(0);

    let disconnected =
        disconnect_attached_work_run(&pool, &session_id, std::time::Duration::from_mins(1))
            .await
            .expect("disconnect")
            .expect("active attached run");
    assert_eq!(disconnected.state, "paused");
    assert_eq!(
        disconnected.attachment_state.as_deref(),
        Some("disconnected")
    );

    let reconnected = reconnect_attached_work_run(&pool, &session_id)
        .await
        .expect("reconnect")
        .expect("disconnected run");
    assert_eq!(reconnected.state, "paused");
    assert_eq!(reconnected.attachment_state.as_deref(), Some("attached"));
    assert!(reconnect_attached_work_run(&pool, &session_id)
        .await
        .expect("replayed reconnect")
        .is_none());

    disconnect_attached_work_run(&pool, &session_id, std::time::Duration::ZERO)
        .await
        .expect("second disconnect");
    let timed_out = timeout_disconnected_work_runs(&pool)
        .await
        .expect("timeout sweep");
    let timed_out_run = timed_out
        .iter()
        .find(|row| row.id == run.id)
        .expect("source run timed out");
    let recovered = recover_attached_work_run(&pool, timed_out_run.id, bear_id)
        .await
        .expect("recover timed-out run");
    assert_ne!(recovered.id, timed_out_run.id);
    assert_eq!(recovered.job_id, timed_out_run.job_id);
    assert_eq!(recovered.job_run_id, timed_out_run.job_run_id);
    assert_eq!(recovered.execution_target, "attached_armature");
    let source_id = timed_out_run.id.to_string();
    assert_eq!(
        recovered
            .result_refs
            .as_ref()
            .and_then(|refs| refs.pointer("/recovery/source_work_run_id"))
            .and_then(serde_json::Value::as_str),
        Some(source_id.as_str())
    );
    assert!(recover_attached_work_run(&pool, timed_out_run.id, bear_id)
        .await
        .is_err());
    assert_eq!(
        get_work_run(&pool, timed_out_run.id)
            .await
            .expect("read immutable source")
            .expect("source exists")
            .state,
        "timed_out"
    );
    assert!(timeout_disconnected_work_runs(&pool)
        .await
        .expect("replayed sweep")
        .iter()
        .all(|row| row.id != run.id));
}

#[tokio::test]
async fn enqueue_enforces_managed_surface_assignment() {
    let Some(pool) = test_pool().await else {
        eprintln!("skipping postgres-backed work_runs test; database unavailable");
        return;
    };
    let _guard = DB_LOCK.lock().await;
    let (user_id, bear_id) = seed_user_and_bear(&pool, "surface").await;

    let suffix = Uuid::new_v4().simple().to_string();
    let surface_name = format!("enq-surface-{}", &suffix[..12]);
    let surface_id = Uuid::new_v4();
    sqlx::query!(
        "INSERT INTO work_surfaces (id, name, kind, created_by_user_id, created_at, updated_at)
         VALUES ($1, $2, 'git_workspace', $3, now(), now())",
        surface_id,
        &surface_name,
        user_id,
    )
    .execute(&pool)
    .await
    .expect("seed surface");

    // Job bound to the surface; bear not assigned -> rejected.
    let (job_id, task_ids) = seed_work_job(&pool, user_id, bear_id).await;
    sqlx::query!(
        "UPDATE job_work_surface_assignments SET work_surface_id = $2 WHERE job_id = $1",
        job_id,
        surface_id,
    )
    .execute(&pool)
    .await
    .expect("bind job to surface");
    let mut enqueue = enqueue_for(bear_id, task_ids[0], user_id);
    enqueue.root_name = None;
    let err = enqueue_work_run(&pool, enqueue.clone())
        .await
        .expect_err("unassigned bear must be rejected");
    assert!(
        err.to_string().contains(&surface_name),
        "names the surface: {err}"
    );

    // Explicit root override naming the managed surface is equally gated.
    enqueue.root_name = Some(surface_name.clone());
    let err = enqueue_work_run(&pool, enqueue.clone())
        .await
        .expect_err("unassigned explicit surface root must be rejected");
    assert!(matches!(err, DenError::ValidationError(_)), "{err:?}");

    // Assign the bear -> enqueue succeeds.
    sqlx::query!(
        "INSERT INTO work_surface_bears (surface_id, bear_id) VALUES ($1, $2)",
        surface_id,
        bear_id,
    )
    .execute(&pool)
    .await
    .expect("assign bear");
    let run = enqueue_work_run(&pool, enqueue)
        .await
        .expect("assigned bear enqueues");
    assert_eq!(run.state, "queued");

    // A free-text root override still passes through once the job itself has
    // a valid managed-surface binding (the provider validates the override at
    // provision time). Use a separate job because work runs are job-scoped.
    let (_free_text_job_id, free_text_task_ids) = seed_work_job(&pool, user_id, bear_id).await;
    let mut free_text = enqueue_for(bear_id, free_text_task_ids[0], user_id);
    free_text.root_name = Some(format!("no-such-surface-{}", &suffix[..8]));
    let run = enqueue_work_run(&pool, free_text)
        .await
        .expect("free-text root passes Den-side");
    assert_eq!(run.state, "queued");
}

#[tokio::test]
async fn enqueue_validates_stance_and_uniqueness() {
    let Some(pool) = test_pool().await else {
        eprintln!("skipping postgres-backed work_runs test; database unavailable");
        return;
    };
    let _guard = DB_LOCK.lock().await;
    let (user_id, bear_id) = seed_user_and_bear(&pool, "enqueue").await;
    let (_job_id, task_ids) = seed_work_job(&pool, user_id, bear_id).await;

    // Enqueue is job-scoped: any task ID in the job locates the same job run.
    let run = enqueue_work_run(&pool, enqueue_for(bear_id, task_ids[2], user_id))
        .await
        .expect("enqueue work job");
    assert_eq!(run.state, "queued");
    assert_eq!(run.attempt, 1);

    // Unknown task is NotFound.
    let err = enqueue_work_run(&pool, enqueue_for(bear_id, Uuid::new_v4(), user_id))
        .await
        .expect_err("unknown task");
    assert!(matches!(err, DenError::NotFound(_)), "{err:?}");

    // The active job run is unique regardless of which task locates it.
    let err = enqueue_work_run(&pool, enqueue_for(bear_id, task_ids[0], user_id))
        .await
        .expect_err("duplicate active run");
    assert!(matches!(err, DenError::ValidationError(_)), "{err:?}");
}

#[tokio::test]
async fn concurrent_claims_take_distinct_runs_across_jobs() {
    let Some(pool) = test_pool().await else {
        eprintln!("skipping postgres-backed work_runs test; database unavailable");
        return;
    };
    let _guard = DB_LOCK.lock().await;
    purge_claimable_runs(&pool).await;
    let (user_id, bear_id) = seed_user_and_bear(&pool, "claim").await;
    // Two separate jobs: runs of different jobs may execute concurrently.
    let (_job_a, tasks_a) = seed_work_job(&pool, user_id, bear_id).await;
    let (_job_b, tasks_b) = seed_work_job(&pool, user_id, bear_id).await;
    let run_a = enqueue_work_run(&pool, enqueue_for(bear_id, tasks_a[0], user_id))
        .await
        .unwrap();
    let run_b = enqueue_work_run(&pool, enqueue_for(bear_id, tasks_b[0], user_id))
        .await
        .unwrap();

    let lease = std::time::Duration::from_mins(1);
    let (first, second) = tokio::join!(
        claim_next_work_run(&pool, "runner-1", lease),
        claim_next_work_run(&pool, "runner-2", lease),
    );
    let first = first.unwrap().expect("first claim");
    let second = second.unwrap().expect("second claim");
    assert_ne!(first.id, second.id, "two claimants must get distinct runs");
    let mut claimed: Vec<Uuid> = vec![first.id, second.id];
    claimed.sort();
    let mut enqueued = vec![run_a.id, run_b.id];
    enqueued.sort();
    assert_eq!(claimed, enqueued);
    assert_eq!(first.state, "claimed");

    // Nothing left to claim.
    let third = claim_next_work_run(&pool, "runner-3", lease).await.unwrap();
    assert!(third.is_none());
}

#[tokio::test]
async fn one_active_work_run_per_job() {
    let Some(pool) = test_pool().await else {
        eprintln!("skipping postgres-backed work_runs test; database unavailable");
        return;
    };
    let _guard = DB_LOCK.lock().await;
    purge_claimable_runs(&pool).await;
    let (user_id, bear_id) = seed_user_and_bear(&pool, "one-active-run").await;
    let (_job_id, task_ids) = seed_work_job(&pool, user_id, bear_id).await;
    let run = enqueue_work_run(&pool, enqueue_for(bear_id, task_ids[0], user_id))
        .await
        .expect("first job run");

    let error = enqueue_work_run(&pool, enqueue_for(bear_id, task_ids[1], user_id))
        .await
        .expect_err("job must reject a second active work run");
    assert!(error
        .to_string()
        .contains("job already has an active work run"));

    let claimed = claim_next_work_run(&pool, "runner-1", std::time::Duration::from_mins(1))
        .await
        .unwrap()
        .expect("single run claims");
    assert_eq!(claimed.id, run.id);
}

#[tokio::test]
async fn blocked_job_refuses_pair_dispatch_without_mutating_task_state() {
    let Some(pool) = test_pool().await else {
        eprintln!("skipping postgres-backed work_runs test; database unavailable");
        return;
    };
    let _guard = DB_LOCK.lock().await;
    let (user_id, bear_id) = seed_user_and_bear(&pool, "blocked-dispatch").await;
    let (job_id, task_ids) = seed_work_job(&pool, user_id, bear_id).await;
    let run = enqueue_work_run(&pool, enqueue_for(bear_id, task_ids[0], user_id))
        .await
        .unwrap();

    finalize_work_run(
        &pool,
        run.id,
        WorkRunState::Failed,
        WorkRunFinalize::default(),
    )
    .await
    .unwrap();

    let service = PgDocketService::from_pool(&pool);
    let error = service
        .execute_job(DocketJobExecuteRequest {
            bear_id,
            job_id,
            actor_role: BearProfile::Pair,
            actor_user_id: Some(user_id),
            actor_agent_id: None,
            session_id: None,
            source_conversation_id: None,
            source_client_session_id: None,
        })
        .await
        .expect_err("blocked job must not dispatch");
    assert!(error.to_string().contains("recover its current run"));

    let statuses = sqlx::query!(
        "SELECT task_id, status FROM bear_task_run_state WHERE run_id = $1 ORDER BY task_id",
        run.job_run_id,
    )
    .fetch_all(&pool)
    .await
    .unwrap()
    .into_iter()
    .map(|row| (row.task_id, row.status))
    .collect::<Vec<_>>();
    let mut expected = task_ids
        .into_iter()
        .map(|task_id| (task_id, "pending".to_string()))
        .collect::<Vec<_>>();
    expected.sort_by_key(|(task_id, _)| *task_id);
    assert_eq!(statuses, expected);
}

#[tokio::test]
async fn expired_lease_is_reclaimed_and_heartbeat_fences_old_owner() {
    let Some(pool) = test_pool().await else {
        eprintln!("skipping postgres-backed work_runs test; database unavailable");
        return;
    };
    let _guard = DB_LOCK.lock().await;
    purge_claimable_runs(&pool).await;
    let (user_id, bear_id) = seed_user_and_bear(&pool, "lease").await;
    let (_job_id, task_ids) = seed_work_job(&pool, user_id, bear_id).await;
    enqueue_work_run(&pool, enqueue_for(bear_id, task_ids[0], user_id))
        .await
        .unwrap();

    let claimed = claim_next_work_run(&pool, "runner-old", std::time::Duration::from_secs(0))
        .await
        .unwrap()
        .expect("claim with instant-expiry lease");

    // Lease already expired: another runner takes the run over, state preserved.
    let reclaimed = claim_next_work_run(&pool, "runner-new", std::time::Duration::from_mins(1))
        .await
        .unwrap()
        .expect("reclaim expired lease");
    assert_eq!(reclaimed.id, claimed.id);
    assert_eq!(reclaimed.runner_id.as_deref(), Some("runner-new"));
    assert_eq!(reclaimed.state, "claimed");

    // The old owner's heartbeat is fenced out.
    let ok = heartbeat_work_run(
        &pool,
        claimed.id,
        "runner-old",
        std::time::Duration::from_mins(1),
    )
    .await
    .unwrap();
    assert!(!ok, "old runner must not extend a reclaimed lease");
    let ok = heartbeat_work_run(
        &pool,
        claimed.id,
        "runner-new",
        std::time::Duration::from_mins(1),
    )
    .await
    .unwrap();
    assert!(ok);
}

#[tokio::test]
async fn lifecycle_provision_outcome_finalize_and_cancel() {
    let Some(pool) = test_pool().await else {
        eprintln!("skipping postgres-backed work_runs test; database unavailable");
        return;
    };
    let _guard = DB_LOCK.lock().await;
    purge_claimable_runs(&pool).await;
    let (user_id, bear_id) = seed_user_and_bear(&pool, "lifecycle").await;
    let (_job_id, task_ids) = seed_work_job(&pool, user_id, bear_id).await;
    let run = enqueue_work_run(&pool, enqueue_for(bear_id, task_ids[0], user_id))
        .await
        .unwrap();
    let claimed = claim_next_work_run(&pool, "runner-1", std::time::Duration::from_mins(1))
        .await
        .unwrap()
        .expect("claim");
    assert_eq!(claimed.id, run.id);

    let provisioned = record_work_run_provisioned(
        &pool,
        run.id,
        &WorkRunProvisioned {
            sandbox_server_url: "http://sandbox:3002".into(),
            sandbox_id: "abc123".into(),
            sandbox_type: "container".into(),
            sandbox_strength: "container: test".into(),
            work_surface: serde_json::json!({ "is_git": true }),
            rust_dependency_preparation: None,
        },
    )
    .await
    .unwrap();
    assert_eq!(provisioned.state, "running");
    assert!(provisioned.started_at.is_some());

    // Armature checks out: session binds, execution session opens.
    let session_id = format!("headless-{}", Uuid::new_v4().simple());
    let checkout = checkout_work_run_for_session(&pool, run.id, bear_id, &session_id)
        .await
        .unwrap();
    assert!(matches!(
        checkout.gate,
        DocketExecutionGate::Allowed {
            task_id,
            binding: DocketExecutionBinding::WorkRun { work_run_id, job_run_id },
        } if task_id == task_ids[0] && work_run_id == run.id && job_run_id == run.job_run_id
    ));
    let prompt_context = checkout
        .prompt_context
        .as_ref()
        .expect("runnable checkout has prompt context");
    assert_eq!(prompt_context.job_id, run.job_id);
    assert_eq!(prompt_context.current_task_id, task_ids[0]);
    assert_eq!(prompt_context.tasks.len(), 1);
    assert_eq!(prompt_context.tasks[0].title, "Alpha work task");
    assert!(prompt_context.tasks[0]
        .completion_criteria
        .iter()
        .any(|criterion| criterion.contains("Alpha work task is verifiably complete")));
    let execution_attempt = checkout
        .execution_attempt
        .as_ref()
        .expect("allowed Work checkout starts a canonical attempt");
    assert_eq!(execution_attempt.task_id, task_ids[0]);
    assert_eq!(
        execution_attempt.state,
        DocketExecutionAttemptState::Running
    );
    assert_eq!(
        execution_attempt.binding.kind,
        DocketExecutionBindingKind::WorkAssignment
    );
    assert_eq!(execution_attempt.binding.id, run.id.to_string());
    assert_eq!(
        execution_attempt.host.kind,
        DocketExecutionHostKind::WorkRun
    );
    assert_eq!(execution_attempt.host.run_id, run.id.to_string());
    let work_binding_id = run.id.to_string();
    let execution_count = sqlx::query_scalar!(
        "SELECT COUNT(*) FROM docket_execution_attempts
         WHERE bear_id = $1 AND binding_kind = 'work_assignment' AND binding_id = $2 AND state = 'running'",
        bear_id,
        work_binding_id,
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(execution_count, Some(1));
    assert!(get_live_work_run_by_session(&pool, &session_id)
        .await
        .unwrap()
        .is_some());

    // Terminal turn event moves the run to reporting.
    let reporting = record_work_run_turn_outcome(
        &pool,
        &session_id,
        &serde_json::json!({ "kind": "completed" }),
    )
    .await
    .unwrap()
    .expect("session is bound");
    assert_eq!(reporting.state, "reporting");

    // Cancellation is still possible pre-finalize and takes precedence over a
    // subsequently reported success: the work result cannot be verified after
    // its execution authority was revoked.
    assert!(request_work_run_cancel_with_provenance(
        &pool,
        run.id,
        bear_id,
        &WorkRunCancelRequest {
            requested_by: "test".into(),
            reason: "test cancellation".into(),
        },
    )
    .await
    .unwrap());
    let cancelled = get_work_run(&pool, run.id)
        .await
        .unwrap()
        .expect("run exists");
    assert_eq!(cancelled.cancel_requested_by.as_deref(), Some("test"));
    assert_eq!(
        cancelled.cancel_reason.as_deref(),
        Some("test cancellation")
    );
    assert!(cancelled.cancel_requested_at.is_some());

    let finalized = finalize_work_run(
        &pool,
        run.id,
        WorkRunState::Succeeded,
        WorkRunFinalize {
            result_summary: Some("done".into()),
            result_refs: Some(serde_json::json!({ "changed_files": 1 })),
            usage: Some(serde_json::json!({ "duration_ms": 1234 })),
            error: None,
        },
    )
    .await
    .unwrap();
    assert_eq!(finalized.state, "blocked");
    let task = sqlx::query!(
        "SELECT status, result_summary, result_refs FROM bear_task_run_state \
             WHERE run_id = $1 AND task_id = $2",
        run.job_run_id,
        task_ids[0],
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let task_status = task.status;
    let task_summary = task.result_summary;
    let task_refs = task.result_refs;
    // Active Work is represented by the work-run/session binding; task-run
    // state records durable outcomes only.
    assert_eq!(task_status, "pending");
    assert!(task_summary.is_none());
    assert!(task_refs.is_none());
    let job_run_state = sqlx::query_scalar!(
        "SELECT state FROM bear_job_runs WHERE id = $1",
        run.job_run_id,
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    // Work telemetry cannot block the job; Docket task settlement owns that
    // scheduler transition.
    assert_eq!(job_run_state, "dispatched");
    assert!(finalized.runner_id.is_none());
    assert!(finalized.lease_expires_at.is_none());
    let live_attempts: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM docket_execution_attempts
         WHERE work_run_id = $1
           AND state IN ('authorized', 'running', 'paused', 'awaiting_user', 'stopping')",
    )
    .bind(run.id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(live_attempts, 0, "terminal work run releases its attempt");
    assert!(finalized.finished_at.is_some());
    // Merged refs keep the turn outcome recorded earlier.
    let refs = finalized.result_refs.expect("result refs");
    assert!(refs.get("turn_outcome").is_some());
    assert_eq!(refs.get("changed_files"), Some(&serde_json::json!(1)));

    // Terminal runs cannot be finalized twice or cancelled.
    assert!(finalize_work_run(
        &pool,
        run.id,
        WorkRunState::Failed,
        WorkRunFinalize::default()
    )
    .await
    .is_err());
    assert!(!request_work_run_cancel(&pool, run.id, bear_id)
        .await
        .unwrap());

    // Finalizing a run must not synthesize task completion. Task status is
    // owned by explicit task events from the worker.
    let events = sqlx::query_scalar!(
        "SELECT COUNT(*) FROM bear_task_events WHERE task_id = $1 AND event_type = 'completed'",
        task_ids[0],
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(events, Some(0));

    // A recovered job run can enqueue a new work-run attempt; job state is
    // derived from its current run and never reset on bear_jobs.
    sqlx::query!(
        "UPDATE bear_job_runs SET state = 'running', finished_at = NULL WHERE id = $1",
        run.job_run_id,
    )
    .execute(&pool)
    .await
    .unwrap();

    // A recovered task can be re-enqueued (attempt 2) and checked out. This
    // used to fail here because the terminal first run still owned a live
    // attempt for the same task.
    let retry = enqueue_work_run(&pool, enqueue_for(bear_id, task_ids[0], user_id))
        .await
        .expect("retry enqueue");
    assert_eq!(retry.attempt, 2);
    claim_next_work_run(&pool, "runner-retry", std::time::Duration::from_mins(1))
        .await
        .unwrap()
        .expect("claim retry");
    record_work_run_provisioned(
        &pool,
        retry.id,
        &WorkRunProvisioned {
            sandbox_server_url: "http://sandbox:3002".into(),
            sandbox_id: "retry".into(),
            sandbox_type: "container".into(),
            sandbox_strength: "container: test".into(),
            work_surface: serde_json::json!({}),
            rust_dependency_preparation: None,
        },
    )
    .await
    .expect("provision retry");
    let retry_checkout = checkout_work_run_for_session(
        &pool,
        retry.id,
        bear_id,
        &format!("headless-{}", Uuid::new_v4().simple()),
    )
    .await
    .expect("retry checkout must acquire a fresh canonical attempt");
    assert!(matches!(
        retry_checkout.gate,
        DocketExecutionGate::Allowed { .. }
    ));

    // get_work_run round-trips.
    assert_eq!(
        get_work_run(&pool, run.id).await.unwrap().unwrap().id,
        run.id
    );
}

#[tokio::test]
async fn successful_work_run_settles_completed_docket_job() {
    let Some(pool) = test_pool().await else {
        eprintln!("skipping postgres-backed work_runs test; database unavailable");
        return;
    };
    let _guard = DB_LOCK.lock().await;
    purge_claimable_runs(&pool).await;
    let (user_id, bear_id) = seed_user_and_bear(&pool, "settles-job").await;
    let (job_id, task_ids) = seed_work_job(&pool, user_id, bear_id).await;
    let run = enqueue_work_run(&pool, enqueue_for(bear_id, task_ids[0], user_id))
        .await
        .unwrap();

    for task_id in task_ids {
        sqlx::query!(
            "INSERT INTO bear_task_run_state (run_id, task_id, status, result_summary, finished_at)
             VALUES ($1, $2, 'done', 'completed', NOW())
             ON CONFLICT (run_id, task_id) DO UPDATE
             SET status = 'done', result_summary = 'completed', finished_at = NOW()",
            run.job_run_id,
            task_id,
        )
        .execute(&pool)
        .await
        .unwrap();
    }
    sqlx::query!(
        "INSERT INTO bear_job_criteria_state (run_id, criterion_id, status, evaluated_at)
             SELECT $1, id, 'met', NOW() FROM bear_job_criteria WHERE job_id = $2
             ON CONFLICT (run_id, criterion_id) DO UPDATE
             SET status = 'met', evaluated_at = NOW()",
        run.job_run_id,
        job_id,
    )
    .execute(&pool)
    .await
    .unwrap();

    finalize_work_run(
        &pool,
        run.id,
        WorkRunState::Succeeded,
        WorkRunFinalize::default(),
    )
    .await
    .unwrap();

    let projection = get_job(&pool, bear_id, job_id)
        .await
        .unwrap()
        .expect("job exists");
    let run = sqlx::query!(
        "SELECT state, finished_at FROM bear_job_runs WHERE id = $1",
        run.job_run_id,
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let run_state = run.state;
    let finished_at = run.finished_at;
    assert_eq!(projection.job.status, "completed");
    assert_eq!(run_state, "completed");
    assert!(finished_at.is_some());
}

#[tokio::test]
async fn publish_wiring_image_branch_and_prompt() {
    let Some(pool) = test_pool().await else {
        eprintln!("skipping postgres-backed work_runs test; database unavailable");
        return;
    };
    let _guard = DB_LOCK.lock().await;
    purge_claimable_runs(&pool).await;
    let (user_id, bear_id) = seed_user_and_bear(&pool, "publish").await;
    let (job_id, task_ids) =
        seed_work_job_with_policy(&pool, user_id, bear_id, DocketCommitPolicy::PerTask).await;

    // Enqueue with a catalog image name; it persists on the run.
    let run = enqueue_work_run(
        &pool,
        WorkRunEnqueue {
            bear_id,
            task_id: task_ids[0],
            root_name: Some("demo".to_string()),
            git_ref: None,
            image_name: Some("rust".to_string()),
            requested_by_user_id: Some(user_id),
        },
    )
    .await
    .unwrap();
    assert_eq!(run.image_name.as_deref(), Some("rust"));

    // The dispatch context sees the pushable policy; the work branch is
    // generated on first use and stable afterwards.
    let context = get_work_run_dispatch_context(&pool, run.id).await.unwrap();
    assert!(context.publishes());
    assert!(context.work_branch.is_none());
    assert_eq!(context.child_result_rollups, serde_json::json!([]));

    // Parent dispatch receives validated child rollups, never child transcripts.
    sqlx::query!(
        "UPDATE bear_tasks SET parent_task_id = $1 WHERE id = $2",
        task_ids[0],
        task_ids[1],
    )
    .execute(&pool)
    .await
    .unwrap();
    let rollup = ResultRollup {
        summary: "child completed safely".into(),
        evidence_refs: serde_json::json!({"artifact": "artifact:test"}),
    };
    assert!(
        persist_result_rollup(&pool, run.job_run_id, task_ids[1], task_ids[0], rollup)
            .await
            .unwrap()
    );
    assert!(!persist_result_rollup(
        &pool,
        run.job_run_id,
        task_ids[1],
        task_ids[0],
        ResultRollup {
            summary: "duplicate must not replace the first result".into(),
            evidence_refs: serde_json::json!({"raw_transcript": "must not appear"}),
        },
    )
    .await
    .unwrap());
    let summaries = parent_rollup_context(&pool, run.job_run_id, task_ids[0])
        .await
        .unwrap();
    assert_eq!(summaries.len(), 1);
    assert_eq!(summaries[0].summary, "child completed safely");
    sqlx::query!(
        "UPDATE bear_work_runs SET executing_task_id = $2 WHERE id = $1",
        run.id,
        task_ids[0],
    )
    .execute(&pool)
    .await
    .unwrap();
    let context = get_work_run_dispatch_context(&pool, run.id).await.unwrap();
    assert_eq!(context.child_result_rollups.as_array().unwrap().len(), 1);
    assert_eq!(
        context.child_result_rollups[0]["summary"],
        "child completed safely"
    );

    let branch = ensure_job_work_branch(&pool, job_id).await.unwrap();
    assert!(branch.starts_with("den/job-"), "{branch}");
    assert_eq!(ensure_job_work_branch(&pool, job_id).await.unwrap(), branch);
    let context = get_work_run_dispatch_context(&pool, run.id).await.unwrap();
    assert_eq!(context.work_branch.as_deref(), Some(branch.as_str()));

    // Pushable jobs instruct the armature to commit (and still not push).
    let claimed = claim_next_work_run(&pool, "runner-pub", std::time::Duration::from_mins(1))
        .await
        .unwrap()
        .expect("claim queued run");
    assert_eq!(claimed.id, run.id);
    let session_id = format!("headless-{}", Uuid::new_v4().simple());
    record_work_run_provisioned(
        &pool,
        run.id,
        &WorkRunProvisioned {
            sandbox_server_url: "http://sandbox:3002".into(),
            sandbox_id: "pub123".into(),
            sandbox_type: "container".into(),
            sandbox_strength: "container: test".into(),
            work_surface: serde_json::json!({ "is_git": true }),
            rust_dependency_preparation: None,
        },
    )
    .await
    .unwrap();
    let checkout = checkout_work_run_for_session(&pool, run.id, bear_id, &session_id)
        .await
        .unwrap();

    // Attempt creation and terminalization are idempotent across replay/crash windows.
    let attempt = sqlx::query!(
        "SELECT a.id, a.routing_decision_id
         FROM docket_turn_attempts a
         JOIN docket_routing_decisions d ON d.id = a.routing_decision_id
         WHERE a.work_run_id = $1",
        run.id,
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let attempt_id = attempt.id;
    let decision_id = attempt.routing_decision_id;
    assert_eq!(
        claim_turn_attempt(
            &pool,
            decision_id,
            Some(run.id),
            1,
            ResolvedExecutionProfile {
                profile: None,
                provenance: ProfileProvenance::ConversationFallback,
            },
        )
        .await
        .unwrap()
        .id,
        attempt_id
    );
    assert!(terminalize_turn_attempt(
        &pool,
        attempt_id,
        AttemptOutcome::Completed,
        "task_completed",
        RetryDisposition::None,
        serde_json::json!({"result": "ok"}),
        Some(10),
        Some(25),
    )
    .await
    .unwrap());
    assert!(!terminalize_turn_attempt(
        &pool,
        attempt_id,
        AttemptOutcome::TimedOut,
        "late_watchdog",
        RetryDisposition::Handoff,
        serde_json::json!({"synthetic": true}),
        None,
        None,
    )
    .await
    .unwrap());
    assert_eq!(
        apply_supervisor_disposition(&pool, attempt_id)
            .await
            .unwrap(),
        SupervisorDisposition::Complete
    );
    assert!(persist_attention_outbox(&pool, attempt_id, "inspect")
        .await
        .unwrap());
    assert!(!persist_attention_outbox(&pool, attempt_id, "inspect")
        .await
        .unwrap());

    let prompt_context = checkout
        .prompt_context
        .as_ref()
        .expect("runnable checkout has prompt context");
    assert_eq!(prompt_context.job_id, run.job_id);
    assert_eq!(prompt_context.run_id, run.job_run_id);
    assert_eq!(prompt_context.tasks.len(), 1);
    assert_eq!(prompt_context.commit_policy.as_deref(), Some("per_task"));

    // An explicit branch set at creation is never overwritten.
    let (job2_id, _) =
        seed_work_job_with_policy(&pool, user_id, bear_id, DocketCommitPolicy::PerJob).await;
    sqlx::query!(
        "UPDATE bear_jobs SET work_branch = 'feature/custom' WHERE id = $1",
        job2_id,
    )
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(
        ensure_job_work_branch(&pool, job2_id).await.unwrap(),
        "feature/custom"
    );

    // Cleanup: cancel the queued run so it cannot satisfy later claims.
    assert!(request_work_run_cancel(&pool, run.id, bear_id)
        .await
        .unwrap());
    purge_claimable_runs(&pool).await;
}

#[tokio::test]
async fn attention_and_completion_visibility() {
    let Some(pool) = test_pool().await else {
        eprintln!("skipping postgres-backed work_runs test; database unavailable");
        return;
    };
    let _guard = DB_LOCK.lock().await;
    purge_claimable_runs(&pool).await;
    let (user_id, bear_id) = seed_user_and_bear(&pool, "attention").await;
    let (job_id, task_ids) = seed_work_job(&pool, user_id, bear_id).await;

    // A latest-attempt blocked run needs attention, with job context.
    let run = enqueue_work_run(&pool, enqueue_for(bear_id, task_ids[0], user_id))
        .await
        .unwrap();
    claim_next_work_run(&pool, "runner-att", std::time::Duration::from_mins(1))
        .await
        .unwrap()
        .expect("claim");
    finalize_work_run(
        &pool,
        run.id,
        WorkRunState::Blocked,
        WorkRunFinalize {
            result_summary: Some("missing credentials for the deploy step".into()),
            ..WorkRunFinalize::default()
        },
    )
    .await
    .unwrap();
    let attention = crate::work_runs::attention_work_runs(&pool, bear_id, Some(job_id), 10)
        .await
        .unwrap();
    assert_eq!(attention.len(), 1, "{attention:?}");
    assert_eq!(attention[0].run_id, run.id);
    assert_eq!(attention[0].job_id, job_id);
    assert_eq!(
        attention[0].result_summary.as_deref(),
        Some("missing credentials for the deploy step")
    );

    // A running retry supersedes the failed attempt for attention purposes;
    // job status is derived from its run evidence.
    sqlx::query!(
        "UPDATE bear_job_runs SET state = 'running', finished_at = NULL WHERE id = $1",
        run.job_run_id,
    )
    .execute(&pool)
    .await
    .unwrap();
    let retry = enqueue_work_run(&pool, enqueue_for(bear_id, task_ids[0], user_id))
        .await
        .unwrap();
    let attention = crate::work_runs::attention_work_runs(&pool, bear_id, Some(job_id), 10)
        .await
        .unwrap();
    assert!(attention.is_empty(), "{attention:?}");

    // Not awaiting completion while tasks are open…
    let awaiting = crate::work_runs::jobs_awaiting_completion(&pool, bear_id)
        .await
        .unwrap();
    assert!(awaiting.iter().all(|job| job.id != job_id), "{awaiting:?}");

    // …but once every task is done in the current run, it is.
    let current_run_id =
        sqlx::query_scalar!("SELECT current_run_id FROM bear_jobs WHERE id = $1", job_id,)
            .fetch_one(&pool)
            .await
            .unwrap();
    for task_id in &task_ids {
        sqlx::query!(
            "INSERT INTO bear_task_run_state (run_id, task_id, status)
             VALUES ($1, $2, 'done')
             ON CONFLICT (run_id, task_id) DO UPDATE SET status = 'done'",
            current_run_id,
            task_id,
        )
        .execute(&pool)
        .await
        .unwrap();
    }
    let awaiting = crate::work_runs::jobs_awaiting_completion(&pool, bear_id)
        .await
        .unwrap();
    assert!(awaiting.iter().any(|job| job.id == job_id), "{awaiting:?}");

    // Cleanup the queued retry so later tests' claims see no leftovers.
    finalize_work_run(
        &pool,
        retry.id,
        WorkRunState::Cancelled,
        WorkRunFinalize::default(),
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn active_task_definition_edits_require_a_paused_work_run() {
    let Some(pool) = test_pool().await else {
        eprintln!("skipping postgres-backed work_runs test; database unavailable");
        return;
    };
    let _guard = DB_LOCK.lock().await;
    purge_claimable_runs(&pool).await;
    let (user_id, bear_id) = seed_user_and_bear(&pool, "paused-task-edit").await;
    let (job_id, task_ids) = seed_work_job(&pool, user_id, bear_id).await;
    let service = PgDocketService::from_pool(&pool);
    let run_id = service
        .get_job(bear_id, job_id)
        .await
        .unwrap()
        .expect("job")
        .current_run
        .expect("job run")
        .id;
    let work_run = enqueue_work_run(&pool, enqueue_for(bear_id, task_ids[0], user_id))
        .await
        .unwrap();

    let edit = |title: &str| DocketTaskUpdate {
        bear_id,
        job_id: Some(job_id),
        task_id: task_ids[0],
        actor_role: BearProfile::Work,
        actor_user_id: Some(user_id),
        actor_agent_id: None,
        definition: DocketTaskDefinitionPatch {
            title: Some(title.to_string()),
            ..DocketTaskDefinitionPatch::default()
        },
        run_state: Some(DocketTaskRunStateUpdate {
            run_id,
            status: crate::DocketTaskStatus::Pending,
            outcome_disposition: None,
            result_refs: None,
            result_summary: None,
        }),
    };
    assert!(service.update_task(edit("must pause first")).await.is_err());

    claim_next_work_run(
        &pool,
        "runner-paused-edit",
        std::time::Duration::from_mins(1),
    )
    .await
    .unwrap();
    record_work_run_provisioned(
        &pool,
        work_run.id,
        &WorkRunProvisioned {
            sandbox_server_url: "http://sandbox:3002".into(),
            sandbox_id: "paused-edit".into(),
            sandbox_type: "container".into(),
            sandbox_strength: "container: test".into(),
            work_surface: serde_json::json!({}),
            rust_dependency_preparation: None,
        },
    )
    .await
    .unwrap();
    assert!(set_work_run_paused(&pool, work_run.id, true).await.unwrap());
    service
        .update_task(edit("edited while paused"))
        .await
        .expect("paused work run permits editing its active task");
}

#[tokio::test]
async fn pause_resume_is_compare_and_set() {
    let Some(pool) = test_pool().await else {
        eprintln!("skipping postgres-backed work_runs test; database unavailable");
        return;
    };
    let _guard = DB_LOCK.lock().await;
    purge_claimable_runs(&pool).await;
    let (user_id, bear_id) = seed_user_and_bear(&pool, "pause-resume").await;
    let (_job_id, task_ids) = seed_work_job(&pool, user_id, bear_id).await;
    let run = enqueue_work_run(&pool, enqueue_for(bear_id, task_ids[0], user_id))
        .await
        .unwrap();
    claim_next_work_run(&pool, "runner-pause", std::time::Duration::from_mins(1))
        .await
        .unwrap()
        .expect("claim");
    record_work_run_provisioned(
        &pool,
        run.id,
        &WorkRunProvisioned {
            sandbox_server_url: "http://sandbox:3002".into(),
            sandbox_id: "pause123".into(),
            sandbox_type: "container".into(),
            sandbox_strength: "container: test".into(),
            work_surface: serde_json::json!({}),
            rust_dependency_preparation: None,
        },
    )
    .await
    .unwrap();

    assert!(set_work_run_paused(&pool, run.id, true).await.unwrap());
    assert!(!set_work_run_paused(&pool, run.id, true).await.unwrap());
    assert_eq!(
        get_work_run(&pool, run.id)
            .await
            .unwrap()
            .expect("work run")
            .state_enum(),
        Some(WorkRunState::Paused)
    );
    assert!(request_work_run_cancel_with_provenance(
        &pool,
        run.id,
        bear_id,
        &WorkRunCancelRequest {
            requested_by: "test".into(),
            reason: "stop while paused".into(),
        },
    )
    .await
    .unwrap());
    assert!(
        get_work_run(&pool, run.id)
            .await
            .unwrap()
            .expect("work run")
            .cancel_requested
    );

    assert!(set_work_run_paused(&pool, run.id, false).await.unwrap());
    assert!(!set_work_run_paused(&pool, run.id, false).await.unwrap());
    assert_eq!(
        get_work_run(&pool, run.id)
            .await
            .unwrap()
            .expect("work run")
            .state_enum(),
        Some(WorkRunState::Running)
    );

    finalize_work_run(
        &pool,
        run.id,
        WorkRunState::Cancelled,
        WorkRunFinalize::default(),
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn checkout_rejects_wrong_bear() {
    let Some(pool) = test_pool().await else {
        eprintln!("skipping postgres-backed work_runs test; database unavailable");
        return;
    };
    let _guard = DB_LOCK.lock().await;
    let (user_id, bear_id) = seed_user_and_bear(&pool, "wrongbear").await;
    let (_, other_bear) = seed_user_and_bear(&pool, "otherbear").await;
    let (_job_id, task_ids) = seed_work_job(&pool, user_id, bear_id).await;
    let run = enqueue_work_run(&pool, enqueue_for(bear_id, task_ids[0], user_id))
        .await
        .unwrap();

    let err = checkout_work_run_for_session(&pool, run.id, other_bear, "headless-x")
        .await
        .expect_err("wrong bear must not check out");
    assert!(matches!(err, DenError::NotFound(_)), "{err:?}");
}
