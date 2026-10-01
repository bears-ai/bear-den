//! Work dispatch worker: drains queued `bear_work_runs` into sandboxes on the
//! configured sandbox provider and reconciles their outcomes back into Docket.
//!
//! Loop shape follows the reflection conductor convention:
//! `select!(cancelled, sleep)` per tick, lease-based claims, cooperative
//! cancellation. One worker owns a run at a time (lease + runner id); a
//! crashed worker's runs are reclaimed after lease expiry.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use sqlx::PgPool;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use den_core::{config::Config, ids::BearId, DenError};
use den_docket::work_runs::{
    self, WorkRunDispatchContext, WorkRunFinalize, WorkRunProvisioned, WorkRunRow, WorkRunState,
};
use den_docket::{PgDocketService, TaskDispatcher};
use den_sandbox::protocol::{
    CreateSandboxRequest, NetworkMode, PublishRequest, SandboxLimits, SandboxType,
};
use den_sandbox::SandboxClient;
use den_service::bears::hats::memory_binding;

use crate::runtime_exception_events::{
    self, NewRuntimeExceptionEvent, RuntimeExceptionContext, RuntimeExceptionSeverity,
};

mod network_policy;

const LEASE: Duration = Duration::from_mins(2);
const ORPHAN_SWEEP_INTERVAL: Duration = Duration::from_hours(1);
const SURFACE_SYNC_INTERVAL: Duration = Duration::from_mins(5);
const LOG_TAIL_BYTES: u64 = 64 * 1024;
const DIFF_PATCH_BYTES: u64 = 256 * 1024;
/// Margin under the container timeout so the armature self-kills (and reports)
/// before the provider's reaper hard-destroys the sandbox.
const DEADLINE_MARGIN_SECS: u64 = 60;

pub async fn run_work_dispatch_worker_loop(
    pool: PgPool,
    config: Arc<Config>,
    worker_token: CancellationToken,
    poll_interval: Duration,
) -> Result<(), DenError> {
    let Some(sandbox_url) = config
        .sandbox_server_url
        .clone()
        .filter(|url| !url.trim().is_empty())
    else {
        tracing::info!("Workers: work_dispatch loop disabled (SANDBOX_SERVER_URL unset)");
        return Ok(());
    };
    let client = SandboxClient::new(&sandbox_url, &config.sandbox_server_token);
    let runner_id = format!("work-dispatch-{}", Uuid::new_v4().simple());
    tracing::info!(
        runner_id,
        sandbox_url,
        "Workers: work_dispatch loop starting"
    );

    // Seed the provider with the current managed config (surfaces + image
    // catalog) so a fresh or wiped provider can serve provisions immediately.
    if let Err(err) = crate::surface_sync::reconcile_if_stale(&pool, &config, &client, true).await {
        tracing::warn!(error = %err, "work_dispatch: startup managed-config push failed (will retry on the sync tick)");
    }
    let mut last_surface_sync = std::time::Instant::now();

    let mut last_orphan_sweep: Option<std::time::Instant> = None;
    loop {
        tokio::select! {
            () = worker_token.cancelled() => break,
            () = tokio::time::sleep(poll_interval) => {}
        }

        if config.work_dispatch_auto {
            auto_enqueue(&pool).await;
        }
        if let Err(err) = work_runs::timeout_disconnected_work_runs(&pool).await {
            tracing::warn!(error = %err, "work_dispatch: attached disconnect timeout sweep failed");
        }
        monitor_owned_runs(&pool, &config, &client, &runner_id).await;
        claim_and_provision(&pool, &config, &client, &runner_id).await;

        if last_surface_sync.elapsed() >= SURFACE_SYNC_INTERVAL {
            last_surface_sync = std::time::Instant::now();
            if let Err(err) =
                crate::surface_sync::reconcile_if_stale(&pool, &config, &client, false).await
            {
                tracing::warn!(error = %err, "work_dispatch: managed-config reconcile failed");
            }
        }

        let sweep_due = last_orphan_sweep.is_none_or(|at| at.elapsed() >= ORPHAN_SWEEP_INTERVAL);
        if sweep_due {
            last_orphan_sweep = Some(std::time::Instant::now());
            orphan_sweep(&pool, &config, &client).await;
        }

        if worker_token.is_cancelled() {
            break;
        }
    }
    tracing::info!(runner_id, "Workers: work_dispatch loop stopped");
    Ok(())
}

/// Optional sweep: queue every job with runnable work tasks. Off by default —
/// explicit and automatic dispatch share the same job-level operation.
async fn auto_enqueue(pool: &PgPool) {
    let bears = match work_runs::list_bears_with_work_tasks(pool).await {
        Ok(bears) => bears,
        Err(err) => {
            tracing::warn!(error = %err, "work_dispatch: auto-enqueue bear listing failed");
            return;
        }
    };
    let service = PgDocketService::from_pool(pool);
    for bear_id in bears {
        let tasks = match service.runnable_work_tasks(bear_id, 20).await {
            Ok(tasks) => tasks,
            Err(err) => {
                tracing::warn!(error = %err, %bear_id, "work_dispatch: runnable task scan failed");
                continue;
            }
        };
        let jobs: std::collections::BTreeMap<Uuid, Option<i32>> = tasks
            .into_iter()
            .filter_map(|projection| {
                projection
                    .task
                    .job_id
                    .map(|job_id| (job_id, projection.task.created_by_user_id))
            })
            .collect();
        for (job_id, requested_by_user_id) in jobs {
            if let Err(err) =
                memory_binding::require_eligible_job(pool, BearId::new(bear_id), job_id).await
            {
                match err {
                    DenError::Authorization(_) => {
                        tracing::debug!(%bear_id, %job_id, error = %err, "work_dispatch: skipping ineligible automatic Job")
                    }
                    _ => {
                        tracing::warn!(%bear_id, %job_id, error = %err, "work_dispatch: failed to check automatic Job eligibility")
                    }
                }
                continue;
            }
            match work_runs::enqueue_work_job(
                pool,
                work_runs::WorkJobEnqueue {
                    bear_id,
                    job_id,
                    durable_result: den_docket::DurableResultKind::RepositoryChanges,
                    git_ref: None,
                    image_name: None,
                    requested_by_user_id,
                    execution_target: work_runs::WorkExecutionTarget::Sandbox,
                    attachment_warning: None,
                },
            )
            .await
            {
                Ok(runs) => {
                    tracing::info!(job_id = %job_id, queued_tasks = runs.len(), "work_dispatch: auto-enqueued job");
                }
                Err(DenError::ValidationError(_)) => {}
                Err(err) => {
                    tracing::warn!(error = %err, %job_id, "work_dispatch: auto-enqueue job failed");
                }
            }
        }
    }
}

async fn claim_and_provision(
    pool: &PgPool,
    config: &Arc<Config>,
    client: &SandboxClient,
    runner_id: &str,
) {
    loop {
        let owned = match work_runs::list_owned_work_runs(pool, runner_id).await {
            Ok(owned) => owned,
            Err(err) => {
                tracing::warn!(error = %err, "work_dispatch: owned-run listing failed");
                return;
            }
        };
        if owned.len() >= config.sandbox_max_concurrent {
            return;
        }
        let run = match work_runs::claim_next_work_run(pool, runner_id, LEASE).await {
            Ok(Some(run)) => run,
            Ok(None) => return,
            Err(err) => {
                tracing::warn!(error = %err, "work_dispatch: claim failed");
                return;
            }
        };
        match run.state_enum() {
            Some(WorkRunState::Claimed) => {
                provision_run(pool, config, client, &run).await;
            }
            // Taken over from a crashed worker mid-flight; the monitor step
            // reconciles it on the next tick.
            _ => {
                tracing::info!(
                    work_run_id = %run.id,
                    state = %run.state,
                    "work_dispatch: adopted in-flight run from expired lease"
                );
            }
        }
    }
}

async fn provision_run(
    pool: &PgPool,
    config: &Arc<Config>,
    client: &SandboxClient,
    run: &WorkRunRow,
) {
    // Recheck the canonical Job hat before touching task state, minting a token,
    // or creating a sandbox. Also applies when adopting a previously claimed run.
    let binding = match memory_binding::for_work_run(pool, BearId::new(run.bear_id), run.id).await {
        Ok(binding) => binding,
        Err(err) => {
            fail_run(pool, run, "work_hat_ineligible", &err.to_string(), None).await;
            return;
        }
    };
    let context = match work_runs::get_work_run_dispatch_context(pool, run.id).await {
        Ok(context) => context,
        Err(err) => {
            fail_run(pool, run, "dispatch_context", &err.to_string(), None).await;
            return;
        }
    };

    let Some(root) = context
        .work_surface_name
        .clone()
        .filter(|surface| !surface.trim().is_empty())
    else {
        fail_run(
            pool,
            run,
            "work_surface_required",
            "work run has no usable managed work surface",
            None,
        )
        .await;
        return;
    };

    let network = if config.work_sandbox_network.eq_ignore_ascii_case("open") {
        NetworkMode::Open
    } else {
        NetworkMode::Restricted
    };
    let allowed_outbound_hosts =
        match network_policy::for_run(pool, BearId::new(run.bear_id), binding, &context, &root)
            .await
        {
            Ok(hosts) => hosts,
            Err(err) => {
                fail_run(pool, run, "work_hat_egress", &err.to_string(), None).await;
                return;
            }
        };
    if allowed_outbound_hosts.is_some() && network == NetworkMode::Open {
        fail_run(
            pool,
            run,
            "work_hat_open_network",
            "hat-bound Work requires a restricted sandbox network",
            None,
        )
        .await;
        return;
    }
    if allowed_outbound_hosts.is_some() {
        match client.health().await {
            Ok(health) if network_policy::require_provider_run_ceiling(&health).is_ok() => {}
            Ok(_) | Err(_) => {
                fail_run(pool, run, "work_hat_provider_capability",
                    "hat-bound Work needs a reachable provider that enforces run-scoped outbound ceilings", None).await;
                return;
            }
        }
    }

    // Docket owns sibling ordering. Claim the exact runnable task before
    // minting credentials or creating a sandbox, so an out-of-order run never
    // consumes a sandbox only to be rejected afterwards.
    let service = PgDocketService::from_pool(pool);
    let task = match service.runnable_work_tasks(run.bear_id, 500).await {
        Ok(tasks) => tasks
            .into_iter()
            .find(|task| task.task.job_id == Some(run.job_id)),
        Err(err) => {
            fail_run(pool, run, "runnable_task_lookup", &err.to_string(), None).await;
            return;
        }
    };
    let Some(task) = task else {
        fail_run(
            pool,
            run,
            "no_runnable_task",
            "work run has no eligible pending task",
            None,
        )
        .await;
        return;
    };
    if let Err(err) = service
        .mark_task_started(
            run.bear_id,
            task.task.id,
            run.job_run_id,
            Some("work-dispatch".to_string()),
        )
        .await
    {
        fail_run(pool, run, "task_start", &err.to_string(), None).await;
        return;
    }
    if let Err(err) =
        work_runs::merge_work_run_result_refs(pool, run.id, &json!({ "task_id": task.task.id }))
            .await
    {
        tracing::warn!(error = %err, work_run_id = %run.id, task_id = %task.task.id, "work_dispatch: failed to persist active task");
    }

    // Ephemeral armature token, minted as the job creator (v1: only
    // user-created jobs dispatch to work). The id is persisted immediately so
    // even a crashed worker's successor can revoke it.
    let token = match den_http::armature_tokens::create_for_bear(
        pool,
        context.created_by_user_id,
        run.bear_id,
        &format!("work-run-{}", run.id),
    )
    .await
    {
        Ok(token) => token,
        Err(err) => {
            fail_run(pool, run, "token_mint", &err.to_string(), None).await;
            return;
        }
    };
    let token_refs = json!({
        "armature_token_id": token.id,
        "armature_token_user_id": context.created_by_user_id,
    });
    if let Err(err) = work_runs::merge_work_run_result_refs(pool, run.id, &token_refs).await {
        tracing::warn!(error = %err, work_run_id = %run.id, "work_dispatch: failed to persist token id");
    }

    // Pushable jobs get their work branch pinned before the first sandbox
    // exists, and later runs provision from it (the provider falls back to
    // the default ref while the branch has no commits yet) so tasks in one
    // job build on each other.
    let work_branch = if context.publishes() {
        match work_runs::ensure_job_work_branch(pool, run.job_id).await {
            Ok(branch) => Some(branch),
            Err(err) => {
                revoke_token_for_run(pool, run.id).await;
                fail_run(pool, run, "work_branch", &err.to_string(), None).await;
                return;
            }
        }
    } else {
        None
    };

    let timeout_secs = config.sandbox_default_timeout_secs;
    let deadline_secs = timeout_secs.saturating_sub(DEADLINE_MARGIN_SECS).max(30);
    let mut env = std::collections::BTreeMap::new();
    env.insert(
        "DEN_API_URL".to_string(),
        config.sandbox_callback_api_url.clone(),
    );
    env.insert("BEAR_SLUG".to_string(), context.bear_slug.clone());
    env.insert("DEN_TOKEN".to_string(), token.raw_token.clone());
    env.insert("DEN_WORK_ORDER_ID".to_string(), run.id.to_string());
    env.insert("DEN_WORKSPACE".to_string(), "/workspace".to_string());
    // The provider mounts a Den-managed cache read-only. Cargo must never
    // fall back to network access from the restricted task sandbox.
    env.insert("CARGO_HOME".to_string(), "/den/cargo-home".to_string());
    env.insert("CARGO_NET_OFFLINE".to_string(), "true".to_string());
    env.insert(
        "DEN_HEADLESS_DEADLINE_SECS".to_string(),
        deadline_secs.to_string(),
    );
    // In-run commits carry the bear's identity (the provider's auto-commit of
    // leftovers uses its own).
    let git_identity = context.bear_name.clone();
    let git_email = format!("{}@work.den.invalid", context.bear_slug);
    env.insert("GIT_AUTHOR_NAME".to_string(), git_identity.clone());
    env.insert("GIT_AUTHOR_EMAIL".to_string(), git_email.clone());
    env.insert("GIT_COMMITTER_NAME".to_string(), git_identity);
    env.insert("GIT_COMMITTER_EMAIL".to_string(), git_email);

    let request = CreateSandboxRequest {
        root,
        git_ref: work_branch.or_else(|| run.git_ref.clone()),
        sandbox_type: SandboxType::Container,
        requires_write: true,
        image: run.image_name.clone(),
        network,
        allowed_outbound_hosts,
        env,
        limits: SandboxLimits {
            timeout_secs,
            max_log_bytes: Some(config.sandbox_max_log_bytes),
            ..SandboxLimits::default()
        },
        labels: std::collections::BTreeMap::from([("work_run_id".to_string(), run.id.to_string())]),
        // ponytail: this run-scoped cache volume survives container replacement
        // but is not yet content-addressed by Cargo.lock. Upgrade path: publish
        // immutable cache volumes keyed by lockfile/toolchain digest.
        cargo_home_volume: Some(format!("den-cargo-work-run-{}", run.id)),
    };

    let descriptor = match client.create_sandbox(&request).await {
        Ok(descriptor) => descriptor,
        Err(err) => {
            revoke_token_for_run(pool, run.id).await;
            fail_run(pool, run, "provision", &err.to_string(), None).await;
            return;
        }
    };

    let provisioned = WorkRunProvisioned {
        sandbox_server_url: client.base_url().to_string(),
        sandbox_id: descriptor.id.clone(),
        sandbox_type: descriptor.sandbox_type.as_str().to_string(),
        sandbox_strength: descriptor.strength_label.clone(),
        work_surface: serde_json::to_value(&descriptor.work_surface).unwrap_or(Value::Null),
        rust_dependency_preparation: descriptor
            .rust_dependency_preparation
            .as_ref()
            .and_then(|result| serde_json::to_value(result).ok()),
    };
    if let Err(err) = work_runs::record_work_run_provisioned(pool, run.id, &provisioned).await {
        tracing::warn!(error = %err, work_run_id = %run.id, "work_dispatch: record_provisioned failed");
        let _ = client.destroy(&descriptor.id, false).await;
        revoke_token_for_run(pool, run.id).await;
        fail_run(pool, run, "record_provisioned", &err.to_string(), None).await;
        return;
    }
    tracing::info!(
        work_run_id = %run.id,
        sandbox_id = %descriptor.id,
        job_id = %run.job_id,
        "work_dispatch: sandbox provisioned, armature launching"
    );
}

async fn monitor_owned_runs(
    pool: &PgPool,
    config: &Arc<Config>,
    client: &SandboxClient,
    runner_id: &str,
) {
    let owned = match work_runs::list_owned_work_runs(pool, runner_id).await {
        Ok(owned) => owned,
        Err(err) => {
            tracing::warn!(error = %err, "work_dispatch: owned-run listing failed");
            return;
        }
    };
    for run in owned {
        match work_runs::heartbeat_work_run(pool, run.id, runner_id, LEASE).await {
            Ok(true) => {}
            Ok(false) => {
                tracing::warn!(work_run_id = %run.id, "work_dispatch: lease lost; dropping run");
                continue;
            }
            Err(err) => {
                tracing::warn!(error = %err, work_run_id = %run.id, "work_dispatch: heartbeat failed");
                continue;
            }
        }

        if run.cancel_requested {
            cancel_run(pool, config, client, &run).await;
            continue;
        }

        match run.state_enum() {
            Some(WorkRunState::Running) => reconcile_running(pool, config, client, &run).await,
            Some(WorkRunState::Reporting) => harvest_run(pool, config, client, &run).await,
            // Claimed/provisioning runs adopted from a dead worker: nothing
            // was provisioned under our runner id — restart provisioning.
            Some(WorkRunState::Claimed) if run.sandbox_id.is_none() => {
                provision_run(pool, config, client, &run).await;
            }
            _ => {}
        }
    }
}

fn sandbox_exit_failure(result_refs: Option<&Value>) -> (&'static str, String) {
    let report = result_refs.and_then(|refs| refs.get("armature_report"));
    let summary = report
        .and_then(|report| report.get("summary"))
        .and_then(Value::as_str)
        .filter(|summary| !summary.trim().is_empty());

    match summary {
        Some(summary) => ("armature_failed", summary.to_string()),
        None => (
            "turn_lost",
            "sandbox exited without a terminal turn outcome (armature crash, deadline, or Den restart)"
                .to_string(),
        ),
    }
}

/// Reconcile a running run after its sandbox exits. An armature report is an
/// authoritative terminal failure; `turn_lost` is only the no-report fallback.
async fn reconcile_running(
    pool: &PgPool,
    config: &Arc<Config>,
    client: &SandboxClient,
    run: &WorkRunRow,
) {
    let Some(sandbox_id) = run.sandbox_id.as_deref() else {
        fail_run(
            pool,
            run,
            "missing_sandbox",
            "running work run has no sandbox id",
            None,
        )
        .await;
        return;
    };
    let descriptor = match client.get_sandbox(sandbox_id).await {
        Ok(descriptor) => descriptor,
        Err(err) if err.kind() == Some("unknown_sandbox") => {
            revoke_token_for_run(pool, run.id).await;
            fail_run(
                pool,
                run,
                "sandbox_lost",
                "sandbox disappeared from the provider (provider restart or manual removal)",
                None,
            )
            .await;
            maybe_requeue(pool, config, run).await;
            return;
        }
        Err(err) => {
            tracing::warn!(error = %err, work_run_id = %run.id, "work_dispatch: sandbox status poll failed");
            return;
        }
    };

    let sandbox_done = matches!(
        descriptor.state,
        den_sandbox::protocol::SandboxLifecycleState::Exited
            | den_sandbox::protocol::SandboxLifecycleState::Failed
            | den_sandbox::protocol::SandboxLifecycleState::Destroyed
    );
    if !sandbox_done {
        return;
    }

    // Re-check: the run hook may have flipped the run to reporting between
    // our listing and the sandbox poll.
    match work_runs::get_work_run(pool, run.id).await {
        Ok(Some(current)) if current.state == "running" => {
            let log_tail = client
                .logs(sandbox_id, Some(LOG_TAIL_BYTES))
                .await
                .map(|logs| logs.content)
                .unwrap_or_default();
            revoke_token_for_run(pool, run.id).await;
            let (reason, message) = sandbox_exit_failure(current.result_refs.as_ref());
            let refs = json!({
                "log_tail": log_tail,
                "sandbox_exit_code": descriptor.exit_code,
            });
            fail_run(pool, &current, reason, &message, Some(refs)).await;
            teardown_sandbox(pool, config, client, &current, false).await;
            maybe_requeue(pool, config, &current).await;
        }
        _ => {}
    }
}

fn work_run_succeeded(
    commit_policy: Option<&str>,
    active_task_id: Option<Uuid>,
    task_statuses: &[(Uuid, String)],
) -> bool {
    match commit_policy {
        Some("per_task") => active_task_id
            .and_then(|task_id| task_statuses.iter().find(|(id, _)| *id == task_id))
            .is_some_and(|(_, status)| status == "done"),
        _ => !task_statuses.is_empty() && task_statuses.iter().all(|(_, status)| status == "done"),
    }
}

fn work_run_final_state(
    task_succeeded: bool,
    publication_required: bool,
    publication_succeeded: bool,
) -> WorkRunState {
    if !task_succeeded {
        WorkRunState::Blocked
    } else if publication_required && !publication_succeeded {
        WorkRunState::Failed
    } else {
        WorkRunState::Succeeded
    }
}

fn work_run_outcome_summary(
    succeeded: bool,
    armature_summary: Option<String>,
    changed_files: usize,
    task_status: Option<&str>,
) -> String {
    if succeeded {
        return armature_summary.unwrap_or_else(|| {
            format!("task marked done in-turn; {changed_files} file(s) changed")
        });
    }

    let status = task_status.unwrap_or("pending");
    let incomplete = format!(
        "sandbox turn completed without recording a terminal task status (task run status: {status})"
    );
    match armature_summary {
        Some(summary) if !summary.trim().is_empty() => {
            format!("{incomplete}; armature: {summary}")
        }
        _ => incomplete,
    }
}

/// Harvest a run whose turn reached a terminal event: collect diff/logs/usage,
/// decide the Docket outcome (done ⟺ the model marked the task done
/// in-turn), finalize, and tear down.
async fn harvest_run(
    pool: &PgPool,
    config: &Arc<Config>,
    client: &SandboxClient,
    run: &WorkRunRow,
) {
    let sandbox_id = run.sandbox_id.as_deref();
    let diff = match sandbox_id {
        Some(id) => client.diff(id, Some(DIFF_PATCH_BYTES)).await.ok(),
        None => None,
    };
    let log_tail = match sandbox_id {
        Some(id) => client
            .logs(id, Some(LOG_TAIL_BYTES))
            .await
            .map(|logs| logs.content)
            .ok(),
        None => None,
    };
    let usage = match sandbox_id {
        Some(id) => client
            .get_sandbox(id)
            .await
            .ok()
            .map(|descriptor| serde_json::to_value(descriptor.usage).unwrap_or(Value::Null)),
        None => None,
    };

    let task_statuses = work_runs::get_job_work_task_run_statuses(pool, run.job_id, run.job_run_id)
        .await
        .unwrap_or_default();
    let context = work_runs::get_work_run_dispatch_context(pool, run.id)
        .await
        .ok();
    let active_task_id = run
        .result_refs
        .as_ref()
        .and_then(|refs| refs.get("task_id"))
        .and_then(Value::as_str)
        .and_then(|id| Uuid::parse_str(id).ok());
    let succeeded = work_run_succeeded(
        context
            .as_ref()
            .and_then(|context| context.commit_policy.as_deref()),
        active_task_id,
        &task_statuses,
    );

    let turn_summary = run
        .result_refs
        .as_ref()
        .and_then(|refs| refs.pointer("/armature_report/summary"))
        .and_then(Value::as_str)
        .filter(|summary| !summary.trim().is_empty())
        .map(str::to_string);
    let changed_files = diff
        .as_ref()
        .map(|diff| diff.changed_files.len())
        .unwrap_or(0);

    // A pushable job has an explicit delivery obligation. Record its outcome
    // on every path so `published: null` never masquerades as success.
    let publication_required = context
        .as_ref()
        .is_some_and(WorkRunDispatchContext::publishes);
    let mut published: Option<Value> = None;
    let mut publish_failed: Option<String> = None;
    let mut publication_status = if publication_required {
        "not_attempted"
    } else {
        "not_required"
    };
    if publication_required && succeeded {
        let context = context
            .as_ref()
            .expect("publication requirement has context");
        match (sandbox_id, context.work_branch.as_deref()) {
            (Some(id), Some(branch)) => {
                let request = PublishRequest {
                    branch: branch.to_string(),
                    auto_commit_leftovers: true,
                    allow_default_ref: context.allow_default_ref,
                    author_name: Some(context.bear_name.clone()),
                    run_label: Some(run.id.to_string()),
                };
                match client.publish(id, &request).await {
                    Ok(outcome) => {
                        tracing::info!(
                            work_run_id = %run.id,
                            branch = %outcome.branch,
                            commits = outcome.commits_pushed,
                            pushed = outcome.pushed,
                            "work_dispatch: run published to upstream"
                        );
                        published = Some(serde_json::to_value(&outcome).unwrap_or(Value::Null));
                        publication_status = "succeeded";
                    }
                    Err(err) => {
                        publish_failed = Some(err.to_string());
                        publication_status = "failed";
                    }
                }
            }
            (None, _) => {
                publish_failed = Some("run has no sandbox to publish from".into());
                publication_status = "failed";
            }
            (_, None) => {
                publish_failed = Some("job has no work branch recorded".into());
                publication_status = "failed";
            }
        }
    }

    let publication_succeeded = publication_status == "succeeded";
    let final_state = work_run_final_state(succeeded, publication_required, publication_succeeded);
    let summary = work_run_outcome_summary(
        succeeded,
        turn_summary,
        changed_files,
        (!succeeded).then_some("one or more work tasks are incomplete"),
    );

    let summary = match &publish_failed {
        Some(reason) => format!("{summary}; PUBLISH FAILED: {reason}"),
        None => summary,
    };
    let refs = json!({
        "sandbox_id": run.sandbox_id,
        "changed_files": diff.as_ref().map(|diff| serde_json::to_value(&diff.changed_files).unwrap_or(Value::Null)),
        "diff_patch": diff.as_ref().map(|diff| diff.patch.clone()),
        "diff_patch_truncated": diff.as_ref().map(|diff| diff.patch_truncated),
        "log_tail": log_tail,
        "published": published,
        "publish_failed": publish_failed,
        "publication": {
            "required": publication_required,
            "status": publication_status,
        },
    });

    revoke_token_for_run(pool, run.id).await;

    let finalize = WorkRunFinalize {
        result_summary: Some(summary),
        result_refs: Some(refs),
        usage,
        error: None,
    };
    match work_runs::finalize_work_run(pool, run.id, final_state, finalize).await {
        Ok(finalized) => {
            tracing::info!(
                work_run_id = %finalized.id,
                final_state = %finalized.state,
                changed_files,
                "work_dispatch: run finalized"
            );
        }
        Err(err) => {
            tracing::warn!(error = %err, work_run_id = %run.id, "work_dispatch: finalize failed");
        }
    }

    teardown_sandbox(
        pool,
        config,
        client,
        run,
        final_state != WorkRunState::Succeeded,
    )
    .await;
}

async fn cancel_run(pool: &PgPool, config: &Arc<Config>, client: &SandboxClient, run: &WorkRunRow) {
    tracing::info!(work_run_id = %run.id, "work_dispatch: cancelling run");
    // ponytail: no in-flight turn interruption — destroying the sandbox
    // starves the turn's obligations and the continuation watchdog fails it.
    // Upgrade path: a durable cancel signal delivered through the BearWire
    // event log / an internal cancel endpoint on the API process.
    revoke_token_for_run(pool, run.id).await;
    teardown_sandbox(pool, config, client, run, false).await;
    let _ = work_runs::finalize_work_run(
        pool,
        run.id,
        WorkRunState::Cancelled,
        WorkRunFinalize {
            result_summary: Some("cancelled by operator".to_string()),
            ..WorkRunFinalize::default()
        },
    )
    .await;
}

async fn teardown_sandbox(
    pool: &PgPool,
    config: &Arc<Config>,
    client: &SandboxClient,
    run: &WorkRunRow,
    failed: bool,
) {
    let Some(sandbox_id) = run.sandbox_id.as_deref() else {
        return;
    };
    let preserve = failed && config.sandbox_preserve_failed;
    match client.destroy(sandbox_id, preserve).await {
        Ok(descriptor) => {
            if let den_sandbox::protocol::CleanupState::Failed { reason } = &descriptor.cleanup {
                let _ = work_runs::merge_work_run_result_refs(
                    pool,
                    run.id,
                    &json!({ "cleanup": "failed", "cleanup_reason": reason }),
                )
                .await;
            }
        }
        Err(err) => {
            tracing::warn!(error = %err, work_run_id = %run.id, sandbox_id, "work_dispatch: sandbox destroy failed");
            let _ = work_runs::merge_work_run_result_refs(
                pool,
                run.id,
                &json!({ "cleanup": "failed", "cleanup_reason": err.to_string() }),
            )
            .await;
        }
    }
}

fn should_block_failed_work_task(
    active_task_id: Option<Uuid>,
    task_id: Uuid,
    status: &str,
) -> bool {
    Some(task_id) == active_task_id && status != "done"
}

async fn fail_run(
    pool: &PgPool,
    run: &WorkRunRow,
    reason: &str,
    message: &str,
    refs: Option<Value>,
) {
    tracing::warn!(work_run_id = %run.id, reason, message, "work_dispatch: run failed");
    if let Err(error) = runtime_exception_events::record(
        pool,
        NewRuntimeExceptionEvent {
            severity: RuntimeExceptionSeverity::Error,
            component: "den_runtime.work_dispatch".to_string(),
            event_code: format!("work_run_failed_{reason}"),
            message: message.to_string(),
            details: refs.clone().unwrap_or_else(|| json!({})),
            context: RuntimeExceptionContext {
                work_run_id: Some(run.id),
                docket_job_id: Some(run.job_id),
                docket_task_id: run.executing_task_id,
                bear_id: Some(run.bear_id),
                ..RuntimeExceptionContext::default()
            },
        },
    )
    .await
    {
        tracing::warn!(error = %error, work_run_id = %run.id, "work_dispatch: failed to persist runtime exception event");
    }
    let service = PgDocketService::from_pool(pool);
    let active_task_id = run
        .result_refs
        .as_ref()
        .and_then(|refs| refs.get("task_id"))
        .and_then(Value::as_str)
        .and_then(|id| Uuid::parse_str(id).ok());
    for (task_id, status) in
        work_runs::get_job_work_task_run_statuses(pool, run.job_id, run.job_run_id)
            .await
            .unwrap_or_default()
    {
        if !should_block_failed_work_task(active_task_id, task_id, &status) {
            continue;
        }
        let _ = service
            .record_task_blocked(
                run.bear_id,
                task_id,
                run.job_run_id,
                format!("work run failed ({reason}): {message}"),
                None,
                Some("work-dispatch".to_string()),
            )
            .await;
    }
    let _ = work_runs::finalize_work_run(
        pool,
        run.id,
        WorkRunState::Failed,
        WorkRunFinalize {
            result_summary: Some(format!("{reason}: {message}")),
            result_refs: refs,
            usage: None,
            error: Some(format!("{reason}: {message}")),
        },
    )
    .await;
}

/// Best-effort retry after infrastructure failures (never after a judged
/// blocked/succeeded outcome): re-enqueue while attempts remain.
async fn maybe_requeue(pool: &PgPool, config: &Arc<Config>, run: &WorkRunRow) {
    if run.attempt >= i32::try_from(config.work_max_attempts).unwrap_or(i32::MAX) {
        return;
    }
    let context = match work_runs::get_work_run_dispatch_context(pool, run.id).await {
        Ok(context) => context,
        Err(_) => return,
    };
    match work_runs::enqueue_work_job(
        pool,
        work_runs::WorkJobEnqueue {
            bear_id: run.bear_id,
            job_id: run.job_id,
            durable_result: den_docket::DurableResultKind::RepositoryChanges,
            git_ref: run.git_ref.clone(),
            image_name: run.image_name.clone(),
            requested_by_user_id: Some(context.created_by_user_id),
            execution_target: work_runs::WorkExecutionTarget::Sandbox,
            attachment_warning: None,
        },
    )
    .await
    {
        Ok(retry) => {
            let retry = &retry[0];
            tracing::info!(
                work_run_id = %retry.id,
                previous = %run.id,
                attempt = retry.attempt,
                "work_dispatch: requeued after infrastructure failure"
            );
        }
        Err(err) => {
            tracing::warn!(error = %err, previous = %run.id, "work_dispatch: requeue failed");
        }
    }
}

/// Revoke the ephemeral armature token minted for this run, using the id
/// persisted in result_refs (survives worker restarts).
async fn revoke_token_for_run(pool: &PgPool, run_id: Uuid) {
    let Ok(Some(run)) = work_runs::get_work_run(pool, run_id).await else {
        return;
    };
    let Some(refs) = run.result_refs.as_ref() else {
        return;
    };
    let (Some(token_id), Some(user_id)) = (
        refs.get("armature_token_id")
            .and_then(Value::as_str)
            .and_then(|id| Uuid::parse_str(id).ok()),
        refs.get("armature_token_user_id")
            .and_then(Value::as_i64)
            .and_then(|id| i32::try_from(id).ok()),
    ) else {
        return;
    };
    if let Err(err) = den_http::armature_tokens::revoke_for_user(pool, user_id, token_id).await {
        tracing::warn!(error = %err, work_run_id = %run_id, "work_dispatch: token revoke failed");
    }
}

/// Reconcile provider-side sandboxes with durable run state: destroy
/// sandboxes whose runs are already terminal (leaked by a crashed worker).
async fn orphan_sweep(pool: &PgPool, config: &Arc<Config>, client: &SandboxClient) {
    let sandboxes = match client.list_sandboxes(None).await {
        Ok(sandboxes) => sandboxes,
        Err(err) => {
            tracing::warn!(error = %err, "work_dispatch: orphan sweep listing failed");
            return;
        }
    };
    for descriptor in sandboxes {
        if matches!(
            descriptor.state,
            den_sandbox::protocol::SandboxLifecycleState::Destroyed
        ) {
            continue;
        }
        let Some(work_run_id) = descriptor
            .labels
            .get("work_run_id")
            .and_then(|id| Uuid::parse_str(id).ok())
        else {
            continue;
        };
        let run = match work_runs::get_work_run(pool, work_run_id).await {
            Ok(run) => run,
            Err(_) => continue,
        };
        let terminal = run
            .as_ref()
            .and_then(WorkRunRow::state_enum)
            .is_none_or(WorkRunState::is_terminal);
        if terminal {
            tracing::warn!(
                sandbox_id = %descriptor.id,
                %work_run_id,
                "work_dispatch: destroying orphaned sandbox for terminal run"
            );
            let preserve =
                config.sandbox_preserve_failed && run.as_ref().is_some_and(|r| r.state == "failed");
            let _ = client.destroy(&descriptor.id, preserve).await;
        }
    }
}

#[cfg(test)]
#[path = "work_dispatch/hat_tests.rs"]
mod hat_tests;

#[cfg(test)]
mod tests {
    use serde_json::json;
    use uuid::Uuid;

    use super::{
        sandbox_exit_failure, should_block_failed_work_task, work_run_final_state,
        work_run_outcome_summary, work_run_succeeded,
    };

    #[test]
    fn per_task_succeeds_when_its_checked_out_task_is_done() {
        let done = Uuid::new_v4();
        let pending = Uuid::new_v4();
        let statuses = vec![(done, "done".to_string()), (pending, "pending".to_string())];

        assert!(work_run_succeeded(Some("per_task"), Some(done), &statuses));
        assert!(!work_run_succeeded(
            Some("per_task"),
            Some(pending),
            &statuses
        ));
        assert!(!work_run_succeeded(Some("per_task"), None, &statuses));
        assert!(!work_run_succeeded(Some("per_job"), Some(done), &statuses));
        assert!(!work_run_succeeded(None, Some(done), &statuses));
    }

    #[test]
    fn required_publication_prevents_success_when_not_published() {
        assert_eq!(
            work_run_final_state(true, true, false),
            den_docket::work_runs::WorkRunState::Failed
        );
        assert_eq!(
            work_run_final_state(true, true, true),
            den_docket::work_runs::WorkRunState::Succeeded
        );
        assert_eq!(
            work_run_final_state(true, false, false),
            den_docket::work_runs::WorkRunState::Succeeded
        );
    }

    #[test]
    fn failed_work_run_blocks_only_its_active_unfinished_task() {
        let active = Uuid::new_v4();
        let unattempted = Uuid::new_v4();

        assert!(should_block_failed_work_task(
            Some(active),
            active,
            "in_progress"
        ));
        assert!(!should_block_failed_work_task(
            Some(active),
            unattempted,
            "pending"
        ));
        assert!(!should_block_failed_work_task(Some(active), active, "done"));
        assert!(!should_block_failed_work_task(None, active, "pending"));
    }

    #[test]
    fn sandbox_exit_prefers_armature_report_over_turn_lost() {
        let refs = json!({
            "armature_report": {
                "status_hint": "failed",
                "summary": "headless turn failed: unsupported required BearWire client obligation"
            }
        });

        assert_eq!(
            sandbox_exit_failure(Some(&refs)),
            (
                "armature_failed",
                "headless turn failed: unsupported required BearWire client obligation".to_string()
            )
        );
    }

    #[test]
    fn sandbox_exit_is_turn_lost_only_without_armature_report() {
        assert_eq!(
            sandbox_exit_failure(None),
            (
                "turn_lost",
                "sandbox exited without a terminal turn outcome (armature crash, deadline, or Den restart)"
                    .to_string()
            )
        );
    }

    #[test]
    fn incomplete_summary_identifies_missing_terminal_task_status() {
        let summary = work_run_outcome_summary(
            false,
            Some("headless turn reached a terminal run event".to_string()),
            0,
            Some("in_progress"),
        );

        assert!(summary.contains("without recording a terminal task status"));
        assert!(summary.contains("task run status: in_progress"));
        assert!(summary.contains("armature: headless turn reached"));
    }
}
