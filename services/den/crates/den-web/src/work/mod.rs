// ROUTES: When modifying routes in this file, update /src/ROUTES.md.
//! Operator UI for autonomous work: Docket jobs with work tasks, work runs,
//! and their sandbox execution results. Controls flow through the same durable
//! state the dispatch worker uses — this UI never touches the sandbox host
//! directly.

use axum::{
    extract::{Path, Query, State},
    response::{IntoResponse, Redirect, Response},
    routing::{get, post},
    Router,
};
use axum_extra::extract::Form;
use minijinja::context;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

const DISPLAY_ID_HEX_LEN: usize = 8;
const ROUTE_ID_HEX_LEN: usize = 16;

fn uuid_hex_prefix(id: Uuid, len: usize) -> String {
    id.simple().to_string()[..len].to_string()
}

fn tasks_parent_first(
    tasks: &[den_docket::DocketTaskRow],
) -> Vec<(&den_docket::DocketTaskRow, usize)> {
    fn visit<'a>(
        tasks: &'a [den_docket::DocketTaskRow],
        parent: Option<Uuid>,
        depth: usize,
        ordered: &mut Vec<(&'a den_docket::DocketTaskRow, usize)>,
    ) {
        let mut children: Vec<_> = tasks
            .iter()
            .filter(|task| task.parent_task_id == parent)
            .collect();
        children.sort_by_key(|task| task.sibling_order);
        for task in children {
            ordered.push((task, depth));
            visit(tasks, Some(task.id), depth + 1, ordered);
        }
    }

    let mut ordered = Vec::with_capacity(tasks.len());
    visit(tasks, None, 0, &mut ordered);
    ordered
}

pub(crate) fn entity_ref(
    id: Uuid,
    kind: &str,
    title: impl Into<String>,
    status: Option<&str>,
) -> serde_json::Value {
    let marker = match status {
        Some("running" | "in_progress" | "queued" | "claimed" | "provisioning" | "reporting") => {
            "▶️ "
        }
        Some("blocked" | "failed" | "timed_out") => "⚠️ ",
        Some("completed" | "done" | "succeeded") => "✅ ",
        _ => "",
    };
    serde_json::json!({
        "kind": kind,
        "id": uuid_hex_prefix(id, DISPLAY_ID_HEX_LEN),
        "route_id": uuid_hex_prefix(id, ROUTE_ID_HEX_LEN),
        "full_id": id.to_string(),
        "title": format!("{marker}{}", title.into()),
    })
}

pub(crate) fn route_id(id: Uuid) -> String {
    uuid_hex_prefix(id, ROUTE_ID_HEX_LEN)
}

fn normalized_route_prefix(prefix: &str) -> Result<String, CustomError> {
    let compact = prefix.replace('-', "");
    if compact.len() < ROUTE_ID_HEX_LEN
        || compact.len() > 32
        || !compact.chars().all(|c| c.is_ascii_hexdigit())
    {
        return Err(CustomError::NotFound("entity not found".to_string()));
    }
    Ok(compact.to_ascii_lowercase())
}

fn unique_scoped_id(ids: Vec<Uuid>) -> Result<Uuid, CustomError> {
    match ids.as_slice() {
        [id] => Ok(*id),
        _ => Err(CustomError::NotFound(
            "entity not found or reference is ambiguous".to_string(),
        )),
    }
}

/// Resolve a global operator-owned work-surface reference. Docket resources
/// must use the Bear-scoped resolvers below instead.
async fn resolve_work_surface_prefix(
    pool: &sqlx::PgPool,
    prefix: &str,
) -> Result<Uuid, CustomError> {
    let prefix = normalized_route_prefix(prefix)?;
    let ids = sqlx::query_scalar!(
        "SELECT id FROM work_surfaces WHERE replace(id::text, '-', '') LIKE $1 || '%' LIMIT 2",
        prefix,
    )
    .fetch_all(pool)
    .await
    .map_err(den_core::DenError::from)?;
    unique_scoped_id(ids)
}

async fn resolve_job_prefix(
    pool: &sqlx::PgPool,
    bear_id: Uuid,
    prefix: &str,
) -> Result<Uuid, CustomError> {
    let prefix = normalized_route_prefix(prefix)?;
    let ids = sqlx::query_scalar!(
        "SELECT id FROM bear_jobs WHERE replace(id::text, '-', '') LIKE $1 || '%' AND bear_id = $2 LIMIT 2",
        prefix,
        bear_id,
    )
    .fetch_all(pool)
    .await
    .map_err(den_core::DenError::from)?;
    unique_scoped_id(ids)
}

async fn resolve_run_prefix(
    pool: &sqlx::PgPool,
    bear_id: Uuid,
    prefix: &str,
) -> Result<Uuid, CustomError> {
    let prefix = normalized_route_prefix(prefix)?;
    let ids = sqlx::query_scalar!(
        "SELECT r.id FROM bear_work_runs r JOIN bear_jobs j ON j.id = r.job_id WHERE replace(r.id::text, '-', '') LIKE $1 || '%' AND j.bear_id = $2 LIMIT 2",
        prefix,
        bear_id,
    )
    .fetch_all(pool)
    .await
    .map_err(den_core::DenError::from)?;
    unique_scoped_id(ids)
}

async fn resolve_task_prefix(
    pool: &sqlx::PgPool,
    bear_id: Uuid,
    prefix: &str,
) -> Result<Uuid, CustomError> {
    let prefix = normalized_route_prefix(prefix)?;
    let ids = sqlx::query_scalar!(
        "SELECT t.id FROM bear_tasks t JOIN bear_jobs j ON j.id = t.job_id WHERE replace(t.id::text, '-', '') LIKE $1 || '%' AND j.bear_id = $2 LIMIT 2",
        prefix,
        bear_id,
    )
    .fetch_all(pool)
    .await
    .map_err(den_core::DenError::from)?;
    unique_scoped_id(ids)
}

use crate::{
    auth_backend::AuthSession,
    errors::CustomError,
    web::{self, AppState},
};
use den_docket::work_runs::{self, WorkRunListFilter, WorkRunRow};
use den_docket::{
    DocketCommitPolicy, DocketCriterionStateUpdate, DocketCriterionStatus, DocketEffortHint,
    DocketEntryListFilter, DocketJobCreate, DocketJobCriterionInput, DocketJobListFilter,
    DocketJobStatus, DocketJobUpdate, DocketService, DocketTaskCreate, DocketTaskDefinitionPatch,
    DocketTaskDifficulty, DocketTaskInput, DocketTaskKind, DocketTaskRunStateUpdate,
    DocketTaskScope, DocketTaskStatus, DocketTaskUpdate, PgDocketService, TaskListVisibility,
};
use den_sandbox::protocol::CatalogResponse;
use den_sandbox::SandboxClient;
use den_service::bears::db as bears_db;
use den_service::bears::BearProfile;

pub mod surfaces;

#[cfg(test)]
mod tests;

/// Global/operator work-surface management. A work surface may be assigned to
/// multiple Bears, so it deliberately has no Bear-scoped canonical URL.
pub fn router() -> Router<AppState> {
    surfaces::router()
}

/// Bear-owned Docket jobs and work runs.
///
/// Mount this below `/bear/{bear_slug}` so job/run URLs always carry the Bear
/// scope used for resolution and presentation.
pub fn docket_router() -> Router<AppState> {
    Router::new()
        .route("/jobs", get(index))
        .route("/jobs/new", get(new_job_form).post(create_job))
        .route("/jobs/{job_id}", get(job_detail))
        .route("/jobs/{job_id}/edit", post(edit_job))
        .route("/jobs/{job_id}/duplicate", post(duplicate_job))
        .route("/jobs/{job_id}/complete", post(complete_job))
        .route("/jobs/{job_id}/cancel", post(cancel_job_run))
        .route("/jobs/{job_id}/archive", post(archive_job))
        .route("/jobs/{job_id}/tasks", post(add_top_level_task))
        .route("/jobs/{job_id}/dispatch", post(dispatch_job))
        .route("/jobs/{job_id}/tasks/{task_id}/retry", post(retry_task))
        .route(
            "/jobs/{job_id}/tasks/{task_id}/children",
            post(add_child_task),
        )
        .route(
            "/jobs/{job_id}/tasks/{task_id}/move/{direction}",
            post(move_task),
        )
        .route("/jobs/runs/{run_id}", get(run_detail))
        .route("/jobs/runs/{run_id}/cancel", post(cancel_run))
        .route("/jobs/runs/{run_id}/pause", post(pause_run))
        .route("/jobs/runs/{run_id}/resume", post(resume_run))
        .route("/jobs/runs/{run_id}/retry", post(retry_run))
}

/// Best-effort fetch of the sandbox provider's root/image catalog for form
/// selects. `None` (provider unconfigured or down) degrades the forms to free
/// text inputs rather than failing the page.
async fn provider_catalog(state: &AppState) -> Option<CatalogResponse> {
    let url = state.config.sandbox_server_url.as_deref()?.trim();
    if url.is_empty() {
        return None;
    }
    let client = SandboxClient::new(url, &state.config.sandbox_server_token);
    match client.catalog().await {
        Ok(catalog) => Some(catalog),
        Err(err) => {
            tracing::warn!(error = %err, "work UI: sandbox catalog fetch failed");
            None
        }
    }
}

#[derive(Serialize)]
struct RunView {
    id: String,
    display_id: String,
    route_id: String,
    full_id: String,
    title: String,
    state: String,
    is_active: bool,
    attempt: i32,
    bear_slug: String,
    job_id: String,
    job_display_id: String,
    job_route_id: String,
    job_full_id: String,
    job_title: String,
    git_ref: Option<String>,
    image: Option<String>,
    sandbox_type: Option<String>,
    sandbox_strength: Option<String>,
    execution_target: String,
    attached_client_session_id: Option<String>,
    attachment_state: Option<String>,
    attachment_warning: Option<String>,
    disconnected_at: Option<String>,
    disconnect_deadline_at: Option<String>,
    result_summary: Option<String>,
    error: Option<String>,
    cancel_requested: bool,
    queued_at: String,
    started_at: Option<String>,
    finished_at: Option<String>,
    duration_secs: Option<i64>,
    cleanup_failed: bool,
    /// Publish outcome from result_refs: branch/commit pushed upstream.
    published_branch: Option<String>,
    published_commit: Option<String>,
    publish_failed: Option<String>,
    /// For queued runs: 1-based position in the job's queue (runs serialize
    /// per job) and the in-flight run this one waits behind.
    queue_position: Option<i64>,
    waiting_on_run_id: Option<String>,
}

#[derive(Serialize)]
struct RunDiagnostic {
    title: &'static str,
    operation: &'static str,
    evidence: &'static str,
    recovery: &'static str,
}

#[derive(Debug, Serialize)]
struct WatchdogFailureView {
    task_title: Option<String>,
    task_status: Option<String>,
    tool_name: Option<String>,
    request_class: Option<String>,
    runtime_event_count: Option<u64>,
    idle_ms: Option<u64>,
}

/// Project only the safe, persisted watchdog fields into the run page. Tool
/// arguments and raw provider payloads never belong in the web view.
fn watchdog_failure_view(refs: Option<&serde_json::Value>) -> Option<WatchdogFailureView> {
    let outcome = refs?.get("outcome")?;
    if outcome.get("code")?.as_str()? != "continuation_watchdog_timeout" {
        return None;
    }
    let affected_task = outcome.get("affected_task");
    let forensics = outcome.get("forensics");
    let request = forensics.and_then(|value| value.get("last_tool_request"));
    Some(WatchdogFailureView {
        task_title: affected_task
            .and_then(|value| value.get("title"))
            .and_then(serde_json::Value::as_str)
            .map(str::to_string),
        task_status: affected_task
            .and_then(|value| value.get("status"))
            .and_then(serde_json::Value::as_str)
            .map(str::to_string),
        tool_name: request
            .and_then(|value| value.get("tool_name"))
            .and_then(serde_json::Value::as_str)
            .map(str::to_string),
        request_class: request
            .and_then(|value| value.get("request_class"))
            .and_then(serde_json::Value::as_str)
            .map(str::to_string),
        runtime_event_count: forensics
            .and_then(|value| value.get("runtime_event_count"))
            .and_then(serde_json::Value::as_u64),
        idle_ms: forensics
            .and_then(|value| value.get("last_event_age_ms"))
            .and_then(serde_json::Value::as_u64),
    })
}

/// Recognize only failures for which this UI can give a concrete, safe next
/// step. Everything else remains visible in the stored outcome and raw log.
fn run_diagnostic(
    result_summary: Option<&str>,
    error: Option<&str>,
    log_tail: &str,
) -> Option<RunDiagnostic> {
    let evidence = [
        result_summary.unwrap_or_default(),
        error.unwrap_or_default(),
        log_tail,
    ]
    .join("\n")
    .to_ascii_lowercase();
    let cargo = evidence.contains("cargo")
        && (evidence.contains("crates.io")
            || evidence.contains("crates.io index")
            || evidence.contains("updating the crates.io index"));
    let network_failure = evidence.contains("tls")
        || evidence.contains("transfer")
        || evidence.contains("timeout")
        || evidence.contains("network failure");
    (cargo && network_failure).then_some(RunDiagnostic {
        title: "Cargo dependency access failed",
        operation: "Cargo attempted to update the crates.io index.",
        evidence: "The run recorded a TLS, transfer, or timeout failure while accessing the registry.",
        recovery: "Restore sandbox access to the required registry host, or provide a local Cargo index and dependency cache; then retry the blocked task and dispatch the job again.",
    })
}

/// Render the canonical outcome persisted by Docket. Older runs without it
/// retain the legacy fallback until they are retried or finalized again.
fn work_run_outcome(
    run: &WorkRunRow,
    task_statuses: &[(Uuid, String)],
    cargo_failure: Option<&serde_json::Value>,
) -> String {
    if let Some(summary) = run
        .result_refs
        .as_ref()
        .and_then(|refs| refs.pointer("/outcome/summary"))
        .and_then(serde_json::Value::as_str)
        .filter(|summary| !summary.trim().is_empty())
    {
        return summary.to_string();
    }
    if cargo_failure
        .and_then(|failure| failure.get("code"))
        .and_then(serde_json::Value::as_str)
        == Some("cargo_offline_cache_miss")
    {
        let package = cargo_failure
            .and_then(|failure| failure.get("required_package"))
            .and_then(serde_json::Value::as_str)
            .map(|package| format!(" `{package}`"))
            .unwrap_or_default();
        return format!(
            "Blocked: Rust dependencies are unavailable in the offline cache.{package} could not be resolved. Dependency preparation was not attempted; prepare Rust dependencies, then retry Cargo."
        );
    }
    let unfinished = task_statuses
        .iter()
        .filter(|(_, status)| status != "done" && status != "cancelled")
        .collect::<Vec<_>>();
    if !unfinished.is_empty() && run.state == "succeeded" {
        let blocked = unfinished
            .iter()
            .filter(|(_, status)| status.as_str() == "blocked")
            .count();
        let status = if blocked > 0 { "blocked" } else { "incomplete" };
        return format!(
            "Work {status}: {} task{} remain {}. The Armature terminal event only confirms that the agent turn ended.",
            unfinished.len(),
            if unfinished.len() == 1 { "" } else { "s" },
            if blocked > 0 { "blocked" } else { "unfinished" },
        );
    }
    if let Some(summary) = run
        .result_summary
        .as_deref()
        .filter(|summary| !summary.trim().is_empty())
    {
        return summary.to_string();
    }
    if let Some(error) = run
        .error
        .as_deref()
        .filter(|error| !error.trim().is_empty())
    {
        return format!("Execution failed: {error}");
    }
    match run.state.as_str() {
        "succeeded" => "The worker completed without a detailed summary.".to_string(),
        "blocked" => "Execution is blocked; inspect the diagnostic and run output for the cause.".to_string(),
        "failed" | "timed_out" => "Execution stopped before producing a detailed outcome; inspect the diagnostic and run output.".to_string(),
        "cancelled" => "Execution was cancelled before completion.".to_string(),
        _ => "Execution is still in progress.".to_string(),
    }
}

fn ts(value: time::OffsetDateTime) -> String {
    value
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_default()
}

fn run_view(run: &WorkRunRow, bear_slug: &str, job_title: &str) -> RunView {
    let is_active = run.state_enum().is_some_and(|state| !state.is_terminal());
    let duration_secs = run.started_at.map(|started| {
        let end = run
            .finished_at
            .unwrap_or_else(time::OffsetDateTime::now_utc);
        (end - started).whole_seconds()
    });
    let cleanup_failed = run
        .result_refs
        .as_ref()
        .and_then(|refs| refs.get("cleanup"))
        .and_then(serde_json::Value::as_str)
        == Some("failed");
    let ref_str = |pointer: &str| {
        run.result_refs
            .as_ref()
            .and_then(|refs| refs.pointer(pointer))
            .and_then(serde_json::Value::as_str)
            .map(str::to_string)
    };
    let reference = entity_ref(run.id, "Run", job_title, Some(&run.state));
    RunView {
        id: run.id.to_string(),
        display_id: reference["id"].as_str().unwrap_or_default().to_string(),
        route_id: reference["route_id"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
        full_id: reference["full_id"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
        title: reference["title"].as_str().unwrap_or_default().to_string(),
        state: run.state.clone(),
        is_active,
        attempt: run.attempt,
        bear_slug: bear_slug.to_string(),
        job_id: route_id(run.job_id),
        job_display_id: uuid_hex_prefix(run.job_id, DISPLAY_ID_HEX_LEN),
        job_route_id: route_id(run.job_id),
        job_full_id: run.job_id.to_string(),
        job_title: entity_ref(run.job_id, "Job", job_title, None)["title"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
        git_ref: run.git_ref.clone(),
        image: run.image_name.clone(),
        sandbox_type: run.sandbox_type.clone(),
        sandbox_strength: run.sandbox_strength.clone(),
        execution_target: run.execution_target.clone(),
        attached_client_session_id: run.attached_client_session_id.clone(),
        attachment_state: run.attachment_state.clone(),
        attachment_warning: run.attachment_warning.clone(),
        disconnected_at: run.disconnected_at.map(ts),
        disconnect_deadline_at: run.disconnect_deadline_at.map(ts),
        result_summary: run.result_summary.clone(),
        error: run.error.clone(),
        cancel_requested: run.cancel_requested,
        queued_at: ts(run.queued_at),
        started_at: run.started_at.map(ts),
        finished_at: run.finished_at.map(ts),
        duration_secs,
        cleanup_failed,
        published_branch: ref_str("/published/branch"),
        published_commit: ref_str("/published/commit"),
        publish_failed: ref_str("/publish_failed"),
        queue_position: None,
        waiting_on_run_id: None,
    }
}

/// Annotate queued runs with their queue placement (one batch query).
async fn attach_queue_info(
    state: &AppState,
    runs: &[WorkRunRow],
    views: &mut [RunView],
) -> Result<(), CustomError> {
    let queued_ids: Vec<Uuid> = runs
        .iter()
        .filter(|run| run.state == "queued")
        .map(|run| run.id)
        .collect();
    if queued_ids.is_empty() {
        return Ok(());
    }
    let infos = work_runs::queued_run_positions(state.sqlx_pool(), &queued_ids).await?;
    let by_id: std::collections::HashMap<String, &work_runs::WorkRunQueueInfo> = infos
        .iter()
        .map(|info| (info.run_id.to_string(), info))
        .collect();
    for view in views.iter_mut() {
        if let Some(info) = by_id.get(&view.id) {
            view.queue_position = Some(info.position);
            view.waiting_on_run_id = info.waiting_on_run_id.map(|id| id.to_string());
        }
    }
    Ok(())
}

fn require_user(auth_session: &AuthSession) -> Result<i32, CustomError> {
    auth_session
        .user
        .as_ref()
        .map(|user| user.id)
        .ok_or_else(|| CustomError::Authentication("login required".to_string()))
}

/// A verified Bear scope for Docket routes.
///
/// Construct this once from the route slug and authenticated membership; use
/// `id` for every Docket query and `slug` only when rendering canonical URLs.
#[derive(Clone)]
struct BearContext {
    id: Uuid,
    slug: String,
}

async fn bear_context(
    state: &AppState,
    auth_session: &AuthSession,
    raw_slug: &str,
) -> Result<BearContext, CustomError> {
    let user_id = require_user(auth_session)?;
    let slug = raw_slug.trim();
    if slug.is_empty() {
        return Err(CustomError::NotFound("bear not found".to_string()));
    }
    let bear = bears_db::bear_for_user_by_slug(state.sqlx_pool(), user_id, slug)
        .await?
        .ok_or_else(|| CustomError::NotFound("bear not found".to_string()))?;
    Ok(BearContext {
        id: bear.id,
        slug: bear.slug,
    })
}

/// Bears the user is a member of, as (id → slug). Used only by the global
/// work-surface UI; Bear-scoped Docket routes use `BearContext` instead.
async fn member_bears(
    state: &AppState,
    user_id: i32,
) -> Result<std::collections::HashMap<Uuid, String>, CustomError> {
    let rows = bears_db::list_bears_for_user(state.sqlx_pool(), user_id).await?;
    Ok(rows
        .into_iter()
        .map(|row| (row.bear.id, row.bear.slug))
        .collect())
}

#[derive(Deserialize, Default)]
struct WorkIndexQuery {
    completed: Option<String>,
    archived: Option<String>,
}

async fn index(
    State(state): State<AppState>,
    auth_session: AuthSession,
    Path(bear_slug): Path<String>,
    Query(query): Query<WorkIndexQuery>,
) -> Result<Response, CustomError> {
    let bear = bear_context(&state, &auth_session, &bear_slug).await?;
    let show_completed = query.completed.as_deref() == Some("show");
    let show_archived = query.archived.as_deref() == Some("show");
    let bear_id = bear.id;
    let bear_slug = &bear.slug;
    let mut jobs_with_work: Vec<serde_json::Value> = Vec::new();

    let service = PgDocketService::from_pool(state.sqlx_pool());
    let jobs = service
        .list_jobs(
            bear_id,
            DocketJobListFilter {
                include_archived: show_archived,
                ..DocketJobListFilter::default()
            },
        )
        .await?;
    let job_ids: Vec<Uuid> = jobs.iter().map(|job| job.id).collect();
    let run_counts: std::collections::HashMap<Uuid, i64> = sqlx::query!(
        "SELECT job_id AS \"job_id!: Uuid\", count(*)::bigint AS \"count!: i64\" FROM bear_work_runs WHERE job_id = ANY($1) GROUP BY job_id",
        &job_ids,
    )
    .fetch_all(state.sqlx_pool())
    .await
    .map_err(den_core::DenError::from)?
    .into_iter()
    .map(|row| (row.job_id, row.count))
    .collect();
    for job in jobs {
        if !show_completed && job.status == "completed" {
            continue;
        }
        let run_count = run_counts.get(&job.id).copied().unwrap_or_default();
        jobs_with_work.push(serde_json::json!({
            "id": job.id.to_string(),
            "display_id": uuid_hex_prefix(job.id, DISPLAY_ID_HEX_LEN),
            "route_id": uuid_hex_prefix(job.id, ROUTE_ID_HEX_LEN),
            "full_id": job.id.to_string(),
            "title": entity_ref(job.id, "Job", &job.goal, Some(&job.status))["title"],
            "bear_slug": bear_slug,
            "goal": job.goal,
            "status": job.status,
            "work_surface_id": job.work_surface_id,
            "docket_run_id": job.current_run_id.map(|id| uuid_hex_prefix(id, DISPLAY_ID_HEX_LEN)),
            "docket_run_state": job.status,
            // ponytail: lifecycle runs and sandbox attempts are separate rows.
            "has_docket_run": job.current_run_id.is_some(),
            "run_count": run_count,
        }));
    }

    // Dispatch-path status so "why is my queued run not starting?" is
    // answerable from this page: is a provider configured, and is it
    // reachable? (The dispatch worker itself runs in the workers process;
    // its liveness is visible in that process's logs.)
    let sandbox_server_url = state
        .config
        .sandbox_server_url
        .as_deref()
        .map(str::trim)
        .filter(|url| !url.is_empty())
        .map(str::to_string);
    let provider_status = match &sandbox_server_url {
        None => serde_json::json!({ "configured": false }),
        Some(url) => {
            let client = SandboxClient::new(url, &state.config.sandbox_server_token);
            match client.health().await {
                Ok(health) => serde_json::json!({
                    "configured": true,
                    "url": url,
                    "reachable": true,
                    "backend_available": health.backend_available,
                    "active_sandboxes": health.active_sandboxes,
                }),
                Err(err) => serde_json::json!({
                    "configured": true,
                    "url": url,
                    "reachable": false,
                    "error": err.to_string(),
                }),
            }
        }
    };

    web::render_template(
        &state,
        "work/index.html",
        auth_session,
        context! {
            title => "Docket",
            jobs => jobs_with_work,
            provider_status => provider_status,
            bear_slug => bear_slug,
            show_completed => show_completed,
            show_archived => show_archived,
        },
    )
    .await
}

async fn new_job_form(
    State(state): State<AppState>,
    auth_session: AuthSession,
    Path(bear_slug): Path<String>,
) -> Result<Response, CustomError> {
    let bear = bear_context(&state, &auth_session, &bear_slug).await?;
    let user_id = require_user(&auth_session)?;
    let bears = member_bears(&state, user_id).await?;
    let mut bear_slugs: Vec<(String, String)> = bears
        .iter()
        .map(|(id, slug)| (id.to_string(), slug.clone()))
        .collect();
    bear_slugs.sort_by(|a, b| a.1.cmp(&b.1));
    let catalog = provider_catalog(&state).await;

    // Managed surfaces available to any of the user's bears; the create
    // handler re-checks the selected bear's assignment server-side.
    let bear_ids: Vec<Uuid> = bears.keys().copied().collect();
    let mut surfaces: Vec<serde_json::Value> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for surface in
        den_service::work_surfaces::list_surfaces_for_bears(state.sqlx_pool(), &bear_ids).await?
    {
        if seen.insert(surface.id) {
            surfaces.push(serde_json::json!({
                "id": surface.id.to_string(),
                "name": surface.name,
                "default_ref": surface.default_ref,
            }));
        }
    }

    web::render_template(
        &state,
        "work/new.html",
        auth_session,
        context! {
            title => "New work job",
            bears => bear_slugs,
            bear_slug => bear.slug,
            catalog => catalog,
            surfaces => surfaces,
        },
    )
    .await
}

/// One task row per repeated `task_title[]` input; criteria are
/// semicolon-separated within the row. Blank rows are skipped.
#[derive(Debug, Deserialize)]
struct NewJobForm {
    goal: String,
    /// Managed work surface id (preferred; from the surface select).
    #[serde(default)]
    surface_id: String,
    #[serde(default)]
    commit_policy: String,
    #[serde(default)]
    work_branch: String,
    #[serde(default)]
    allow_default_ref: bool,
    #[serde(default)]
    task_title: Vec<String>,
    #[serde(default)]
    task_criteria: Vec<String>,
}

async fn create_job(
    State(state): State<AppState>,
    auth_session: AuthSession,
    Path(bear_slug): Path<String>,
    Form(form): Form<NewJobForm>,
) -> Result<Response, CustomError> {
    let bear = bear_context(&state, &auth_session, &bear_slug).await?;
    let user_id = require_user(&auth_session)?;

    let commit_policy = match form.commit_policy.trim() {
        "" => {
            return Err(CustomError::ValidationError(
                "choose a commit policy explicitly".to_string(),
            ));
        }
        raw => Some(
            serde_json::from_value::<DocketCommitPolicy>(serde_json::json!(raw)).map_err(|_| {
                CustomError::ValidationError(format!("invalid commit policy '{raw}'"))
            })?,
        ),
    };

    let mut tasks = Vec::new();
    for (index, title) in form.task_title.iter().enumerate() {
        let title = title.trim();
        if title.is_empty() {
            continue;
        }
        let criteria: Vec<String> = form
            .task_criteria
            .get(index)
            .map(|raw| {
                raw.split(';')
                    .map(str::trim)
                    .filter(|criterion| !criterion.is_empty())
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();
        if criteria.is_empty() {
            return Err(CustomError::ValidationError(format!(
                "task '{title}' needs at least one completion criterion (semicolon-separated)"
            )));
        }
        tasks.push(DocketTaskInput {
            client_key: Some(format!("ui-task-{index}")),
            parent_client_key: None,
            parent_task_id: None,
            sibling_order: Some(i32::try_from(index).unwrap_or(0)),
            kind: DocketTaskKind::Execution,
            scope: DocketTaskScope::Template,
            title: title.to_string(),
            // Docket requires a non-empty body; the quick-create form only
            // collects titles, so the title doubles as the body.
            body: title.to_string(),
            completion_criteria: criteria,
            difficulty: None,
            effort_hint: None,
            routing_strategy: Default::default(),
            expected_context_size: None,
            result_rollup_policy: None,
        });
    }
    if tasks.is_empty() {
        return Err(CustomError::ValidationError(
            "a work job needs at least one task".to_string(),
        ));
    }

    let work_surface_id =
        form.surface_id.trim().parse::<Uuid>().map_err(|_| {
            CustomError::ValidationError("choose a managed work surface".to_string())
        })?;
    let surface = den_service::work_surfaces::surface_by_id(state.sqlx_pool(), work_surface_id)
        .await?
        .ok_or_else(|| CustomError::NotFound("work surface not found".to_string()))?;
    if !den_service::work_surfaces::bear_may_use_surface(state.sqlx_pool(), bear.id, surface.id)
        .await?
    {
        return Err(CustomError::ValidationError(format!(
            "bear is not assigned to work surface '{}'",
            surface.name
        )));
    }
    let work_surface_id = Some(surface.id);

    let surface_default_ref = Some(surface.default_ref);
    let entered_branch = clean_form_field(&form.work_branch);
    let work_branch = if form.allow_default_ref {
        Some(surface_default_ref.clone().ok_or_else(|| {
            CustomError::ValidationError(
                "working on the default branch requires a managed work surface".to_string(),
            )
        })?)
    } else {
        if entered_branch.is_some() && entered_branch.as_deref() == surface_default_ref.as_deref() {
            return Err(CustomError::ValidationError(
                "work branch matches the surface default; use the explicit default-branch override instead"
                    .to_string(),
            ));
        }
        entered_branch
    };

    let job = PgDocketService::from_pool(state.sqlx_pool())
        .create_job(DocketJobCreate {
            bear_id: bear.id,
            created_by_user_id: user_id,
            created_by_role: "ui".to_string(),
            goal: form.goal,
            work_surface_id,
            work_surface_assignments: Vec::new(),
            commit_policy,
            work_branch,
            visibility: TaskListVisibility::SameUser,
            source_conversation_id: None,
            objective_kind: None,
            supersedes_job_id: None,
            overlap_resolution: den_docket::DocketJobOverlapResolution::Reject,
            criteria: vec![DocketJobCriterionInput {
                kind: den_docket::DocketCriterionKind::Narrative,
                description: "All tasks completed to their criteria".to_string(),
                spec: None,
                sibling_order: 0,
            }],
            tasks,
        })
        .await?;
    Ok(Redirect::to(&format!(
        "/bear/{}/jobs/{}",
        bear.slug,
        route_id(job.job.id)
    ))
    .into_response())
}

#[derive(Debug, Deserialize)]
struct EditJobForm {
    goal: String,
    #[serde(default)]
    surface_id: Option<Uuid>,
    commit_policy: String,
    #[serde(default)]
    work_branch: String,
    #[serde(default)]
    allow_default_ref: bool,
}

async fn edit_job(
    State(state): State<AppState>,
    auth_session: AuthSession,
    Path((bear_slug, job_ref)): Path<(String, String)>,
    Form(form): Form<EditJobForm>,
) -> Result<Response, CustomError> {
    let bear = bear_context(&state, &auth_session, &bear_slug).await?;
    let job_id = resolve_job_prefix(state.sqlx_pool(), bear.id, &job_ref).await?;
    let user_id = require_user(&auth_session)?;
    let bears = member_bears(&state, user_id).await?;
    let bear_id: Option<Uuid> =
        sqlx::query_scalar!("SELECT bear_id FROM bear_jobs WHERE id = $1", job_id)
            .fetch_optional(state.sqlx_pool())
            .await
            .map_err(den_core::DenError::from)?;
    let Some(bear_id) = bear_id.filter(|bear_id| bears.contains_key(bear_id)) else {
        return Err(CustomError::NotFound("job not found".to_string()));
    };
    let goal = form.goal.trim();
    if goal.is_empty() {
        return Err(CustomError::ValidationError(
            "job goal is required".to_string(),
        ));
    }
    let commit_policy =
        parse_docket_enum::<DocketCommitPolicy>("commit policy", form.commit_policy.trim())?;
    let entered_branch = clean_form_field(&form.work_branch);
    if entered_branch
        .as_deref()
        .is_some_and(|branch| branch.starts_with('-') || branch.chars().any(char::is_whitespace))
    {
        return Err(CustomError::ValidationError(
            "work branch must be a branch name, not a Git option or command".to_string(),
        ));
    }
    let (work_surface_id, surface_default_ref) = match form.surface_id {
        Some(surface_id) => {
            let surface = den_service::work_surfaces::surface_by_id(state.sqlx_pool(), surface_id)
                .await?
                .ok_or_else(|| CustomError::NotFound("work surface not found".to_string()))?;
            if !den_service::work_surfaces::bear_may_use_surface(
                state.sqlx_pool(),
                bear_id,
                surface_id,
            )
            .await?
            {
                return Err(CustomError::ValidationError(
                    "the job's Bear is not assigned to that work surface".to_string(),
                ));
            }
            (Some(surface.id), Some(surface.default_ref))
        }
        None => {
            return Err(CustomError::ValidationError(
                "choose a managed work surface for this work job".to_string(),
            ));
        }
    };
    let work_branch = if form.allow_default_ref {
        Some(surface_default_ref.clone().ok_or_else(|| {
            CustomError::ValidationError(
                "working on the default branch requires a managed work surface".to_string(),
            )
        })?)
    } else {
        if entered_branch.is_some() && entered_branch.as_deref() == surface_default_ref.as_deref() {
            return Err(CustomError::ValidationError(
                "work branch matches the surface default; use the explicit default-branch override instead"
                    .to_string(),
            ));
        }
        entered_branch
    };
    PgDocketService::from_pool(state.sqlx_pool())
        .update_job(DocketJobUpdate {
            bear_id,
            job_id,
            actor_role: BearProfile::Pair,
            actor_user_id: Some(user_id),
            actor_agent_id: None,
            goal: Some(goal.to_string()),
            work_surface_id: Some(work_surface_id),
            commit_policy: Some(Some(commit_policy)),
            work_branch: Some(work_branch),
            status: None,
            visibility: None,
        })
        .await?;
    Ok(Redirect::to(&format!("/bear/{}/jobs/{}", bear.slug, route_id(job_id))).into_response())
}

fn commit_policy_label(policy: Option<&str>) -> &'static str {
    match policy {
        Some("per_task") => "Publish after each task",
        Some("per_job") => "Publish to the job branch",
        Some("none") | None => "No source changes expected",
        Some(_) => "Unknown policy",
    }
}

fn parse_docket_enum<T: serde::de::DeserializeOwned>(
    field: &str,
    value: &str,
) -> Result<T, CustomError> {
    serde_json::from_value(serde_json::json!(value)).map_err(|_| {
        CustomError::ValidationError(format!("job contains invalid {field} value '{value}'"))
    })
}

async fn duplicate_job(
    State(state): State<AppState>,
    auth_session: AuthSession,
    Path((bear_slug, job_ref)): Path<(String, String)>,
) -> Result<Response, CustomError> {
    let bear = bear_context(&state, &auth_session, &bear_slug).await?;
    let job_id = resolve_job_prefix(state.sqlx_pool(), bear.id, &job_ref).await?;
    let user_id = require_user(&auth_session)?;
    let bears = member_bears(&state, user_id).await?;
    let owner = sqlx::query_scalar!("SELECT bear_id FROM bear_jobs WHERE id = $1", job_id)
        .fetch_optional(state.sqlx_pool())
        .await
        .map_err(den_core::DenError::from)?;
    let Some(bear_id) = owner.filter(|bear_id| bears.contains_key(bear_id)) else {
        return Err(CustomError::NotFound("job not found".to_string()));
    };

    let source = PgDocketService::from_pool(state.sqlx_pool())
        .get_job(bear_id, job_id)
        .await?
        .ok_or_else(|| CustomError::NotFound("job not found".to_string()))?;
    let task_keys: std::collections::HashMap<Uuid, String> = source
        .tasks
        .iter()
        .map(|task| (task.id, format!("duplicate-{}", task.id.simple())))
        .collect();
    let tasks = source
        .tasks
        .iter()
        .map(|task| {
            Ok(DocketTaskInput {
                client_key: task_keys.get(&task.id).cloned(),
                parent_client_key: task
                    .parent_task_id
                    .and_then(|parent_id| task_keys.get(&parent_id).cloned()),
                parent_task_id: None,
                sibling_order: Some(task.sibling_order),
                kind: parse_docket_enum("task kind", &task.kind)?,
                scope: parse_docket_enum("task scope", &task.scope)?,
                title: task.title.clone(),
                body: task.body.clone(),
                completion_criteria: task.completion_criteria.0.clone(),
                difficulty: task
                    .difficulty
                    .as_deref()
                    .map(|value| {
                        parse_docket_enum::<DocketTaskDifficulty>("task difficulty", value)
                    })
                    .transpose()?,
                effort_hint: task
                    .effort_hint
                    .as_deref()
                    .map(|value| parse_docket_enum::<DocketEffortHint>("task effort", value))
                    .transpose()?,
                routing_strategy: parse_docket_enum("routing strategy", &task.routing_strategy)?,
                expected_context_size: task.expected_context_size,
                result_rollup_policy: task
                    .result_rollup_policy
                    .as_deref()
                    .map(|value| parse_docket_enum("result rollup policy", value))
                    .transpose()?,
            })
        })
        .collect::<Result<Vec<_>, CustomError>>()?;
    let criteria = source
        .criteria
        .iter()
        .map(|criterion| {
            Ok(DocketJobCriterionInput {
                kind: parse_docket_enum("criterion kind", &criterion.kind)?,
                description: criterion.description.clone(),
                spec: criterion.spec.as_ref().map(|spec| spec.0.clone()),
                sibling_order: criterion.sibling_order,
            })
        })
        .collect::<Result<Vec<_>, CustomError>>()?;
    let commit_policy = source
        .job
        .commit_policy
        .as_deref()
        .map(|value| parse_docket_enum::<DocketCommitPolicy>("commit policy", value))
        .transpose()?;
    let visibility = parse_docket_enum::<TaskListVisibility>("visibility", &source.job.visibility)?;

    let duplicate = PgDocketService::from_pool(state.sqlx_pool())
        .create_job(DocketJobCreate {
            bear_id,
            created_by_user_id: user_id,
            created_by_role: "ui".to_string(),
            goal: format!("{} (copy)", source.job.goal),
            work_surface_id: source.job.work_surface_id,
            work_surface_assignments: Vec::new(),
            commit_policy,
            work_branch: None,
            visibility,
            source_conversation_id: None,
            objective_kind: source.job.objective_kind,
            supersedes_job_id: None,
            overlap_resolution: den_docket::DocketJobOverlapResolution::Independent,
            criteria,
            tasks,
        })
        .await?;
    Ok(Redirect::to(&format!(
        "/bear/{}/jobs/{}",
        bear.slug,
        route_id(duplicate.job.id)
    ))
    .into_response())
}

async fn complete_job(
    State(state): State<AppState>,
    auth_session: AuthSession,
    Path((bear_slug, job_ref)): Path<(String, String)>,
) -> Result<Response, CustomError> {
    let bear = bear_context(&state, &auth_session, &bear_slug).await?;
    let job_id = resolve_job_prefix(state.sqlx_pool(), bear.id, &job_ref).await?;
    let user_id = require_user(&auth_session)?;
    let bears = member_bears(&state, user_id).await?;
    let owner = sqlx::query_scalar!("SELECT bear_id FROM bear_jobs WHERE id = $1", job_id)
        .fetch_optional(state.sqlx_pool())
        .await
        .map_err(den_core::DenError::from)?;
    let Some(bear_id) = owner.filter(|bear_id| bears.contains_key(bear_id)) else {
        return Err(CustomError::NotFound("job not found".to_string()));
    };
    let service = PgDocketService::from_pool(state.sqlx_pool());
    let projection = service
        .get_job(bear_id, job_id)
        .await?
        .ok_or_else(|| CustomError::NotFound("job not found".to_string()))?;
    let report = den_docket::docket_job_status_report(&projection);
    if !report.tasks_complete {
        return Err(CustomError::ValidationError(
            "complete every task before marking the job completed".to_string(),
        ));
    }
    let run_id = projection
        .job
        .current_run_id
        .ok_or_else(|| CustomError::ValidationError("job has no current Docket run".to_string()))?;
    let criterion_states: std::collections::HashMap<Uuid, &str> = projection
        .criteria_states
        .iter()
        .map(|state| (state.criterion_id, state.status.as_str()))
        .collect();
    for criterion in &projection.criteria {
        if !matches!(
            criterion_states.get(&criterion.id).copied(),
            Some("met" | "waived")
        ) {
            service
                .evaluate_criterion(DocketCriterionStateUpdate {
                    bear_id,
                    job_id,
                    run_id,
                    criterion_id: criterion.id,
                    status: DocketCriterionStatus::Met,
                    evidence: Some(serde_json::json!({
                        "source": "work_ui_human_completion",
                        "accepted_by_user_id": user_id,
                    })),
                    actor_role: BearProfile::Pair,
                    actor_user_id: Some(user_id),
                    actor_agent_id: None,
                })
                .await?;
        }
    }
    Ok(Redirect::to(&format!("/bear/{}/jobs/{}", bear.slug, route_id(job_id))).into_response())
}

async fn archive_job(
    State(state): State<AppState>,
    auth_session: AuthSession,
    Path((bear_slug, job_ref)): Path<(String, String)>,
) -> Result<Response, CustomError> {
    let bear = bear_context(&state, &auth_session, &bear_slug).await?;
    let job_id = resolve_job_prefix(state.sqlx_pool(), bear.id, &job_ref).await?;
    let user_id = require_user(&auth_session)?;
    let bears = member_bears(&state, user_id).await?;
    let owner = sqlx::query_scalar!("SELECT bear_id FROM bear_jobs WHERE id = $1", job_id)
        .fetch_optional(state.sqlx_pool())
        .await
        .map_err(den_core::DenError::from)?;
    let Some(bear_id) = owner.filter(|bear_id| bears.contains_key(bear_id)) else {
        return Err(CustomError::NotFound("job not found".to_string()));
    };
    let service = PgDocketService::from_pool(state.sqlx_pool());
    let job = service
        .get_job(bear_id, job_id)
        .await?
        .ok_or_else(|| CustomError::NotFound("job not found".to_string()))?;
    let status = if job.job.status == "archived" {
        DocketJobStatus::Ready
    } else {
        DocketJobStatus::Archived
    };
    service
        .update_job(DocketJobUpdate {
            bear_id,
            job_id,
            actor_role: BearProfile::Pair,
            actor_user_id: Some(user_id),
            actor_agent_id: None,
            goal: None,
            work_surface_id: None,
            commit_policy: None,
            work_branch: None,
            status: Some(status),
            visibility: None,
        })
        .await?;
    Ok(Redirect::to(&format!("/bear/{}/jobs/{}", bear.slug, route_id(job_id))).into_response())
}

#[derive(Debug, Deserialize)]
struct AddTopLevelTaskForm {
    title: String,
    #[serde(default)]
    body: String,
    criteria: String,
}

#[derive(Debug, Deserialize)]
struct AddChildTaskForm {
    title: String,
    #[serde(default)]
    body: String,
    criteria: String,
}

async fn add_top_level_task(
    State(state): State<AppState>,
    auth_session: AuthSession,
    Path((bear_slug, job_ref)): Path<(String, String)>,
    Form(form): Form<AddTopLevelTaskForm>,
) -> Result<Response, CustomError> {
    let bear = bear_context(&state, &auth_session, &bear_slug).await?;
    let job_id = resolve_job_prefix(state.sqlx_pool(), bear.id, &job_ref).await?;
    ensure_safe_task_mutation_boundary(state.sqlx_pool(), job_id).await?;
    let user_id = require_user(&auth_session)?;
    let bears = member_bears(&state, user_id).await?;
    let owner = sqlx::query_scalar!("SELECT bear_id FROM bear_jobs WHERE id = $1", job_id)
        .fetch_optional(state.sqlx_pool())
        .await
        .map_err(den_core::DenError::from)?;
    let Some(bear_id) = owner.filter(|bear_id| bears.contains_key(bear_id)) else {
        return Err(CustomError::NotFound("job not found".to_string()));
    };
    let service = PgDocketService::from_pool(state.sqlx_pool());
    let projection = service
        .get_job(bear_id, job_id)
        .await?
        .ok_or_else(|| CustomError::NotFound("job not found".to_string()))?;
    if matches!(
        projection.job.status.as_str(),
        "completed" | "cancelled" | "archived"
    ) {
        return Err(CustomError::ValidationError(
            "completed, cancelled, or archived jobs cannot receive new tasks; duplicate the job instead"
                .to_string(),
        ));
    }
    let title = form.title.trim();
    if title.is_empty() {
        return Err(CustomError::ValidationError(
            "task title is required".to_string(),
        ));
    }
    let criteria: Vec<String> = form
        .criteria
        .split(';')
        .map(str::trim)
        .filter(|criterion| !criterion.is_empty())
        .map(str::to_string)
        .collect();
    if criteria.is_empty() {
        return Err(CustomError::ValidationError(
            "task needs at least one semicolon-separated criterion".to_string(),
        ));
    }
    let run_id = projection
        .job
        .current_run_id
        .ok_or_else(|| CustomError::ValidationError("job has no current Docket run".to_string()))?;
    let sibling_order = projection
        .tasks
        .iter()
        .map(|task| task.sibling_order)
        .max()
        .unwrap_or(-1)
        .saturating_add(1);
    service
        .create_task(DocketTaskCreate {
            bear_id,
            job_id: Some(job_id),
            session_anchor_id: None,
            parent_task_id: None,
            sibling_order,
            placement: None,
            kind: DocketTaskKind::Execution,
            scope: DocketTaskScope::Template,
            title: title.to_string(),
            body: clean_form_field(&form.body).unwrap_or_else(|| title.to_string()),
            completion_criteria: criteria,
            difficulty: None,
            effort_hint: None,
            routing_strategy: Default::default(),
            expected_context_size: None,
            result_rollup_policy: None,
            created_by_role: "ui".to_string(),
            created_by_user_id: Some(user_id),
            created_by_agent_id: None,
            created_in_run_id: Some(run_id),
        })
        .await?;
    Ok(Redirect::to(&format!("/bear/{}/jobs/{}", bear.slug, route_id(job_id))).into_response())
}

#[derive(Deserialize)]
struct JobDetailQuery {
    task: Option<String>,
}

async fn job_detail(
    State(state): State<AppState>,
    auth_session: AuthSession,
    Path((bear_slug, job_ref)): Path<(String, String)>,
    Query(query): Query<JobDetailQuery>,
) -> Result<Response, CustomError> {
    let bear = bear_context(&state, &auth_session, &bear_slug).await?;
    let job_id = resolve_job_prefix(state.sqlx_pool(), bear.id, &job_ref).await?;
    let user_id = require_user(&auth_session)?;
    let bears = member_bears(&state, user_id).await?;

    // Resolve which member bear owns this job.
    let owner: Option<(Uuid, String)> = {
        let row = sqlx::query_scalar!("SELECT bear_id FROM bear_jobs WHERE id = $1", job_id)
            .fetch_optional(state.sqlx_pool())
            .await
            .map_err(den_core::DenError::from)?;
        row.and_then(|bear_id| bears.get(&bear_id).map(|slug| (bear_id, slug.clone())))
    };
    let Some((bear_id, bear_slug)) = owner else {
        return Err(CustomError::NotFound("job not found".to_string()));
    };
    let bear_name = bears_db::list_bears_for_user(state.sqlx_pool(), user_id)
        .await?
        .into_iter()
        .find(|row| row.bear.id == bear_id)
        .map(|row| row.bear.name)
        .unwrap_or_else(|| bear_slug.clone());

    let service = PgDocketService::from_pool(state.sqlx_pool());
    let projection = service
        .get_job(bear_id, job_id)
        .await?
        .ok_or_else(|| CustomError::NotFound("job not found".to_string()))?;
    let entries = service
        .list_entries(
            bear_id,
            DocketEntryListFilter {
                job_id: Some(job_id),
                task_id: None,
                limit: 500,
            },
        )
        .await?;
    let notebook_entries: Vec<_> = entries
        .iter()
        .filter(|entry| entry.scope == "job_notebook")
        .map(|entry| {
            serde_json::json!({
                "kind": entry.kind,
                "summary": entry.summary,
                "body": entry.body,
                "task_route_id": entry.task_id.map(route_id),
                "task_display_id": entry.task_id.map(|id| uuid_hex_prefix(id, DISPLAY_ID_HEX_LEN)),
                "tags": entry.tags,
                "by_role": entry.by_role,
                "source_entry_id": entry.source_entry_id,
                "created_at": entry.created_at,
            })
        })
        .collect();
    let outcome_entries: Vec<_> = entries
        .iter()
        .filter(|entry| entry.kind == "outcome")
        .map(|entry| {
            serde_json::json!({
                "summary": entry.summary,
                "disposition": entry.disposition,
                "task_route_id": entry.task_id.map(route_id),
                "task_display_id": entry.task_id.map(|id| uuid_hex_prefix(id, DISPLAY_ID_HEX_LEN)),
                "run_route_id": entry.run_id.map(route_id),
                "run_display_id": entry.run_id.map(|id| uuid_hex_prefix(id, DISPLAY_ID_HEX_LEN)),
                "evidence_refs": entry.evidence_refs,
                "by_role": entry.by_role,
                "created_at": entry.created_at,
            })
        })
        .collect();
    let runs = work_runs::list_work_runs(
        state.sqlx_pool(),
        WorkRunListFilter {
            bear_id: Some(bear_id),
            job_id: Some(job_id),
            limit: 100,
            ..WorkRunListFilter::default()
        },
    )
    .await?;
    let mut run_views: Vec<RunView> = runs
        .iter()
        .map(|run| run_view(run, &bear_slug, &projection.job.goal))
        .collect();
    attach_queue_info(&state, &runs, &mut run_views).await?;

    let task_states: std::collections::HashMap<Uuid, &den_docket::DocketTaskRunStateRow> =
        projection
            .task_states
            .iter()
            .map(|state| (state.task_id, state))
            .collect();
    let tasks: Vec<serde_json::Value> = tasks_parent_first(&projection.tasks)
        .into_iter()
        .map(|(task, depth)| {
            let state = task_states.get(&task.id).copied();
            let status = state.map(|state| state.status.as_str()).unwrap_or("pending");
            serde_json::json!({
                "id": route_id(task.id),
                "display_id": uuid_hex_prefix(task.id, DISPLAY_ID_HEX_LEN),
                "full_id": task.id.to_string(),
                "title": task.title,
                "description": task.body,
                "depth": depth,
                "kind": task.kind,
                "status": status,
                "completion_criteria": task.completion_criteria.0,
                "can_retry": status == "blocked",
                "parent_task_id": task.parent_task_id.map(|id| id.to_string()),
                "sibling_order": task.sibling_order,
                "blocker_reason": (status == "blocked").then(|| state.and_then(|state| state.result_summary.clone())).flatten(),
                "retry_reason": state
                    .and_then(|state| state.result_refs.as_ref())
                    .and_then(|refs| refs.get("retry"))
                    .and_then(|retry| retry.get("reason"))
                    .and_then(serde_json::Value::as_str),
                "previous_blocked_reason": state
                    .and_then(|state| state.result_refs.as_ref())
                    .and_then(|refs| refs.get("retry"))
                    .and_then(|retry| retry.get("previous_blocked_reason"))
                    .and_then(serde_json::Value::as_str),
                "run_display_id": state.map(|state| uuid_hex_prefix(state.run_id, DISPLAY_ID_HEX_LEN)),
                "run_route_id": state.map(|state| route_id(state.run_id)),
            })
        })
        .collect();

    let selected_task_id = match query.task.as_deref() {
        Some(task_ref) => resolve_task_prefix(state.sqlx_pool(), bear_id, task_ref)
            .await
            .ok(),
        None => None,
    };
    let selected_task = selected_task_id.and_then(|task_id| {
        projection
            .tasks
            .iter()
            .find(|task| task.id == task_id)
            .map(|task| {
                let state = task_states.get(&task.id).copied();
                serde_json::json!({
                    "id": route_id(task.id),
                    "display_id": uuid_hex_prefix(task.id, DISPLAY_ID_HEX_LEN),
                    "title": task.title,
                    "description": task.body,
                    "status": state.map(|state| state.status.as_str()).unwrap_or("pending"),
                    "run_route_id": state.map(|state| route_id(state.run_id)),
                })
            })
    });
    let selected_diagnostics = match selected_task_id
        .and_then(|task_id| task_states.get(&task_id).map(|state| state.run_id))
    {
        Some(run_id) => Some(den_docket::run_diagnostics(state.sqlx_pool(), run_id).await?),
        None => None,
    };

    let has_runnable_work = tasks.iter().any(|task| {
        matches!(
            task.get("status").and_then(serde_json::Value::as_str),
            Some("pending" | "blocked")
        )
    });
    let active_work_run = run_views.iter().find(|run| run.is_active);
    let attention_run = run_views
        .iter()
        .find(|run| matches!(run.state.as_str(), "blocked" | "failed" | "timed_out"));
    let status_report = den_docket::docket_job_status_report(&projection);
    let available_surfaces =
        den_service::work_surfaces::list_surfaces_for_bears(state.sqlx_pool(), &[bear_id]).await?;
    let selected_work_surface_id = projection.job.work_surface_id;
    let selected_work_surface = selected_work_surface_id.and_then(|surface_id| {
        available_surfaces
            .iter()
            .find(|surface| surface.id == surface_id)
    });
    let allow_default_ref = selected_work_surface.is_some_and(|surface| {
        projection.job.work_branch.as_deref() == Some(surface.default_ref.as_str())
    });
    let dispatch_preflight = den_docket::preflight_dispatch(
        &work_runs::WorkExecutionTarget::Sandbox,
        den_docket::DurableResultKind::RepositoryChanges,
        match projection.job.commit_policy.as_deref() {
            Some("none") => Some(den_docket::DocketCommitPolicy::None),
            Some("per_task") => Some(den_docket::DocketCommitPolicy::PerTask),
            Some("per_job") => Some(den_docket::DocketCommitPolicy::PerJob),
            _ => None,
        },
        projection.job.work_branch.as_deref(),
    );
    let catalog = provider_catalog(&state).await;
    web::render_template(
        &state,
        "work/job.html",
        auth_session,
        context! {
            title => "Work job",
            bear_id => bear_id.to_string(),
            bear_slug => bear_slug,
            bear_name => bear_name,
            job_id => route_id(job_id),
            goal => projection.job.goal,
            job_display_id => uuid_hex_prefix(job_id, DISPLAY_ID_HEX_LEN),
            job_full_id => job_id.to_string(),
            job_title => entity_ref(job_id, "Job", &projection.job.goal, Some(&projection.job.status))["title"],
            status => projection.job.status,
            docket_run_state => projection.current_run.as_ref().map(|run| run.state.clone()),
            docket_run_id => projection.current_run.as_ref().map(|run| uuid_hex_prefix(run.id, DISPLAY_ID_HEX_LEN)),
            work_surface_id => projection.job.work_surface_id.map(route_id),
            selected_work_surface_id => selected_work_surface_id,
            work_surface_name => selected_work_surface.map(|surface| surface.name.clone()),
            work_surface_default_ref => selected_work_surface.map(|surface| surface.default_ref.clone()),
            available_surfaces => available_surfaces,
            commit_policy_label => commit_policy_label(projection.job.commit_policy.as_deref()),
            commit_policy => projection.job.commit_policy,
            work_branch => projection.job.work_branch,
            allow_default_ref => allow_default_ref,
            tasks => tasks,
            selected_task => selected_task,
            selected_diagnostics => selected_diagnostics,
            notebook_entries => notebook_entries,
            outcome_entries => outcome_entries,
            runs => run_views,
            catalog => catalog,
            tasks_complete => status_report.tasks_complete,
            criteria_complete => status_report.criteria_complete,
            next_action => status_report.next_action,
            has_runnable_work => has_runnable_work,
            has_active_work_run => active_work_run.is_some(),
            active_work_run => active_work_run,
            attention_run => attention_run,
            dispatch_preflight => dispatch_preflight,
        },
    )
    .await
}

async fn ensure_safe_task_mutation_boundary(
    pool: &sqlx::PgPool,
    job_id: Uuid,
) -> Result<(), CustomError> {
    let unsafe_active = sqlx::query_scalar!(
        "SELECT EXISTS (SELECT 1 FROM bear_work_runs WHERE job_id=$1 AND state IN ('claimed','provisioning','running','reporting')) AS \"exists!: bool\"",
        job_id,
    )
    .fetch_one(pool)
    .await
    .map_err(den_core::DenError::from)?;
    if unsafe_active {
        return Err(CustomError::ValidationError(
            "pause the active run before adding or reordering tasks".to_string(),
        ));
    }
    Ok(())
}

async fn add_child_task(
    State(state): State<AppState>,
    auth_session: AuthSession,
    Path((bear_slug, job_ref, parent_ref)): Path<(String, String, String)>,
    Form(form): Form<AddChildTaskForm>,
) -> Result<Response, CustomError> {
    let bear = bear_context(&state, &auth_session, &bear_slug).await?;
    let job_id = resolve_job_prefix(state.sqlx_pool(), bear.id, &job_ref).await?;
    ensure_safe_task_mutation_boundary(state.sqlx_pool(), job_id).await?;
    let parent_task_id = resolve_task_prefix(state.sqlx_pool(), bear.id, &parent_ref).await?;
    let user_id = require_user(&auth_session)?;
    let bears = member_bears(&state, user_id).await?;
    let Some(bear_id) = sqlx::query_scalar!("SELECT bear_id FROM bear_jobs WHERE id = $1", job_id)
        .fetch_optional(state.sqlx_pool())
        .await
        .map_err(den_core::DenError::from)?
        .filter(|bear_id| bears.contains_key(bear_id))
    else {
        return Err(CustomError::NotFound("job not found".to_string()));
    };
    let title = form.title.trim();
    let criteria: Vec<String> = form
        .criteria
        .split(';')
        .map(str::trim)
        .filter(|criterion| !criterion.is_empty())
        .map(str::to_string)
        .collect();
    if title.is_empty() || criteria.is_empty() {
        return Err(CustomError::ValidationError(
            "child task title and at least one completion criterion are required".to_string(),
        ));
    }
    let service = PgDocketService::from_pool(state.sqlx_pool());
    let projection = service
        .get_job(bear_id, job_id)
        .await?
        .ok_or_else(|| CustomError::NotFound("job not found".to_string()))?;
    if !projection
        .tasks
        .iter()
        .any(|task| task.id == parent_task_id)
    {
        return Err(CustomError::NotFound(
            "parent task not found in this job".to_string(),
        ));
    }
    let sibling_order = projection
        .tasks
        .iter()
        .filter(|task| task.parent_task_id == Some(parent_task_id))
        .map(|task| task.sibling_order)
        .max()
        .unwrap_or(-1)
        .saturating_add(1);
    service
        .create_task(DocketTaskCreate {
            bear_id,
            job_id: Some(job_id),
            session_anchor_id: None,
            parent_task_id: Some(parent_task_id),
            sibling_order,
            placement: None,
            kind: DocketTaskKind::Execution,
            scope: DocketTaskScope::Template,
            title: title.to_string(),
            body: clean_form_field(&form.body).unwrap_or_else(|| title.to_string()),
            completion_criteria: criteria,
            difficulty: None,
            effort_hint: None,
            routing_strategy: Default::default(),
            expected_context_size: None,
            result_rollup_policy: None,
            created_by_role: "ui".to_string(),
            created_by_user_id: Some(user_id),
            created_by_agent_id: None,
            created_in_run_id: projection.job.current_run_id,
        })
        .await?;
    Ok(Redirect::to(&format!("/bear/{}/jobs/{}", bear.slug, route_id(job_id))).into_response())
}

async fn move_task(
    State(state): State<AppState>,
    auth_session: AuthSession,
    Path((bear_slug, job_ref, task_ref, direction)): Path<(String, String, String, String)>,
) -> Result<Response, CustomError> {
    let bear = bear_context(&state, &auth_session, &bear_slug).await?;
    let job_id = resolve_job_prefix(state.sqlx_pool(), bear.id, &job_ref).await?;
    ensure_safe_task_mutation_boundary(state.sqlx_pool(), job_id).await?;
    let task_id = resolve_task_prefix(state.sqlx_pool(), bear.id, &task_ref).await?;
    let user_id = require_user(&auth_session)?;
    let bears = member_bears(&state, user_id).await?;
    let Some(bear_id) = sqlx::query_scalar!("SELECT bear_id FROM bear_jobs WHERE id = $1", job_id)
        .fetch_optional(state.sqlx_pool())
        .await
        .map_err(den_core::DenError::from)?
        .filter(|bear_id| bears.contains_key(bear_id))
    else {
        return Err(CustomError::NotFound("job not found".to_string()));
    };
    let service = PgDocketService::from_pool(state.sqlx_pool());
    let projection = service
        .get_job(bear_id, job_id)
        .await?
        .ok_or_else(|| CustomError::NotFound("job not found".to_string()))?;
    let task = projection
        .tasks
        .iter()
        .find(|task| task.id == task_id)
        .ok_or_else(|| CustomError::NotFound("task not found in this job".to_string()))?;
    let mut siblings: Vec<_> = projection
        .tasks
        .iter()
        .filter(|other| other.parent_task_id == task.parent_task_id)
        .collect();
    siblings.sort_by_key(|task| (task.sibling_order, task.created_at));
    let position = siblings
        .iter()
        .position(|other| other.id == task_id)
        .ok_or_else(|| CustomError::NotFound("task not found in this job".to_string()))?;
    let other_position = match direction.as_str() {
        "up" if position > 0 => position - 1,
        "down" if position + 1 < siblings.len() => position + 1,
        "up" | "down" => {
            return Ok(
                Redirect::to(&format!("/bear/{}/jobs/{}", bear.slug, route_id(job_id)))
                    .into_response(),
            )
        }
        _ => {
            return Err(CustomError::ValidationError(
                "move direction must be up or down".to_string(),
            ))
        }
    };
    let other = siblings[other_position];
    let temporary_order = -2_147_483_648_i32;
    sqlx::query!(
        "UPDATE bear_tasks SET sibling_order = $1, updated_at = NOW() WHERE bear_id = $2 AND id = $3",
        temporary_order,
        bear_id,
        task.id,
    )
        .execute(state.sqlx_pool())
        .await
        .map_err(den_core::DenError::from)?;
    sqlx::query!(
        "UPDATE bear_tasks SET sibling_order = $1, updated_at = NOW() WHERE bear_id = $2 AND id = $3",
        task.sibling_order,
        bear_id,
        other.id,
    )
        .execute(state.sqlx_pool())
        .await
        .map_err(den_core::DenError::from)?;
    sqlx::query!(
        "UPDATE bear_tasks SET sibling_order = $1, updated_at = NOW() WHERE bear_id = $2 AND id = $3",
        other.sibling_order,
        bear_id,
        task.id,
    )
        .execute(state.sqlx_pool())
        .await
        .map_err(den_core::DenError::from)?;
    Ok(Redirect::to(&format!("/bear/{}/jobs/{}", bear.slug, route_id(job_id))).into_response())
}

async fn retry_task(
    State(state): State<AppState>,
    auth_session: AuthSession,
    Path((bear_slug, job_ref, task_ref)): Path<(String, String, String)>,
    Form(form): Form<RetryTaskForm>,
) -> Result<Response, CustomError> {
    let bear = bear_context(&state, &auth_session, &bear_slug).await?;
    let job_id = resolve_job_prefix(state.sqlx_pool(), bear.id, &job_ref).await?;
    let task_id = resolve_task_prefix(state.sqlx_pool(), bear.id, &task_ref).await?;
    let user_id = require_user(&auth_session)?;
    let bears = member_bears(&state, user_id).await?;
    let Some(bear_id) = sqlx::query_scalar!("SELECT bear_id FROM bear_jobs WHERE id = $1", job_id)
        .fetch_optional(state.sqlx_pool())
        .await
        .map_err(den_core::DenError::from)?
        .filter(|bear_id| bears.contains_key(bear_id))
    else {
        return Err(CustomError::NotFound("job not found".to_string()));
    };
    let reason = form.reason.trim();
    if reason.is_empty() {
        return Err(CustomError::ValidationError(
            "a retry reason is required".to_string(),
        ));
    }
    let service = PgDocketService::from_pool(state.sqlx_pool());
    let projection = service
        .get_job(bear_id, job_id)
        .await?
        .ok_or_else(|| CustomError::NotFound("job not found".to_string()))?;
    let run_id = projection
        .job
        .current_run_id
        .ok_or_else(|| CustomError::ValidationError("job has no current run".to_string()))?;
    let state_row = projection
        .task_states
        .iter()
        .find(|state| state.task_id == task_id && state.run_id == run_id)
        .ok_or_else(|| {
            CustomError::NotFound("task not found in the job's current run".to_string())
        })?;
    if state_row.status != "blocked" {
        return Err(CustomError::ValidationError(
            "only blocked tasks can be retried".to_string(),
        ));
    }
    service
        .update_task(DocketTaskUpdate {
            bear_id,
            job_id: Some(job_id),
            task_id,
            actor_role: BearProfile::Pair,
            actor_user_id: Some(user_id),
            actor_agent_id: None,
            definition: DocketTaskDefinitionPatch::default(),
            run_state: Some(DocketTaskRunStateUpdate {
                run_id,
                status: DocketTaskStatus::Pending,
                outcome_disposition: None,
                result_refs: Some(serde_json::json!({
                    "retry": {
                        "reason": reason,
                        "previous_blocked_reason": state_row.result_summary,
                    }
                })),
                result_summary: Some(format!("Retried: {reason}")),
            }),
        })
        .await?;
    Ok(Redirect::to(&format!("/bear/{}/jobs/{}", bear.slug, route_id(job_id))).into_response())
}

#[derive(Debug, Deserialize)]
struct RetryTaskForm {
    reason: String,
}

async fn run_detail(
    State(state): State<AppState>,
    auth_session: AuthSession,
    Path((bear_slug, run_ref)): Path<(String, String)>,
) -> Result<Response, CustomError> {
    let bear = bear_context(&state, &auth_session, &bear_slug).await?;
    let run_id = resolve_run_prefix(state.sqlx_pool(), bear.id, &run_ref).await?;
    let user_id = require_user(&auth_session)?;
    let bears = member_bears(&state, user_id).await?;
    let run = work_runs::get_work_run(state.sqlx_pool(), run_id)
        .await?
        .filter(|run| bears.contains_key(&run.bear_id))
        .ok_or_else(|| CustomError::NotFound("work run not found".to_string()))?;
    let bear_slug = bears.get(&run.bear_id).cloned().unwrap_or_default();

    let dispatch_context =
        work_runs::get_work_run_dispatch_context(state.sqlx_pool(), run.id).await?;
    let canonical_diagnostics =
        den_docket::run_diagnostics(state.sqlx_pool(), run.job_run_id).await?;
    let refs = run.result_refs.clone().unwrap_or(serde_json::Value::Null);
    let log_tail = refs
        .get("log_tail")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("")
        .to_string();
    let diff_patch = refs
        .get("diff_patch")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("")
        .to_string();
    let changed_files: Vec<serde_json::Value> = refs
        .get("changed_files")
        .and_then(serde_json::Value::as_array)
        .cloned()
        .unwrap_or_default();
    let armature_report = refs.get("armature_report").cloned();
    let turn_outcome = refs.get("turn_outcome").cloned();
    let dependency_preparation = refs.get("rust_dependency_preparation").cloned();
    let cargo_failure = refs.get("cargo_failure").cloned();
    let task_statuses =
        work_runs::get_job_work_task_run_statuses(state.sqlx_pool(), run.job_id, run.job_run_id)
            .await?;
    let diagnostic = run_diagnostic(
        run.result_summary.as_deref(),
        run.error.as_deref(),
        &log_tail,
    );
    let conversation_id: Option<String> = match run.bearwire_session_id.as_deref() {
        Some(session_id) => sqlx::query_scalar!(
            "SELECT COALESCE(NULLIF(resolved_conversation_id, ''), conversation_id) AS \"conversation_id!: String\" \
             FROM client_sessions WHERE client_session_id = $1 AND bear_id = $2 \
             ORDER BY updated_at DESC LIMIT 1",
            session_id,
            run.bear_id,
        )
        .fetch_optional(state.sqlx_pool())
        .await
        .map_err(den_core::DenError::from)?,
        None => None,
    };
    let activity = match run.bearwire_session_id.as_deref() {
        Some(session_id) => {
            let events = den_runtime::bearwire_events::list_bearwire_events_after(
                state.sqlx_pool(),
                session_id,
                None,
                500,
            )
            .await?;
            den_runtime::work_activity::project_work_activity_for(
                events,
                den_runtime::work_activity::WorkActivityAudience::Audit,
            )
        }
        None => Vec::new(),
    };
    let work_surface = run.work_surface.clone();
    let work_surface_link = match dispatch_context.work_surface_name.as_deref() {
        Some(name) => den_service::work_surfaces::surface_by_name(state.sqlx_pool(), name)
            .await?
            .map(|surface| {
                serde_json::json!({
                    "id": surface.id.to_string(),
                    "name": surface.name,
                })
            }),
        None => None,
    };
    let usage = run.usage.clone();
    let mut views = vec![run_view(&run, &bear_slug, &dispatch_context.job_goal)];
    attach_queue_info(&state, std::slice::from_ref(&run), &mut views).await?;
    let view = views.remove(0);
    let can_retry = if run.execution_target == "attached_armature" {
        run.state == "timed_out"
            && run
                .result_refs
                .as_ref()
                .and_then(|refs| refs.pointer("/outcome/code"))
                .and_then(serde_json::Value::as_str)
                == Some("armature_disconnect_timeout")
    } else {
        !view.is_active
    };
    let can_pause = run.state == "running";
    let can_resume = run.state == "paused";

    web::render_template(
        &state,
        "work/run.html",
        auth_session,
        context! {
            title => "Work run",
            bear_slug => bear_slug,
            run => view,
            outcome => work_run_outcome(&run, &task_statuses, cargo_failure.as_ref()),
            watchdog_failure => watchdog_failure_view(run.result_refs.as_ref()),
            log_tail => log_tail,
            diff_patch => diff_patch,
            changed_files => changed_files,
            armature_report => armature_report,
            turn_outcome => turn_outcome,
            dependency_preparation => dependency_preparation,
            cargo_failure => cargo_failure,
            diagnostic => diagnostic,
            canonical_diagnostics => canonical_diagnostics,
            activity => activity,
            conversation_id => conversation_id,
            work_surface => work_surface,
            work_surface_link => work_surface_link,
            usage => usage,
            can_retry => can_retry,
            can_pause => can_pause,
            can_resume => can_resume,
        },
    )
    .await
}

#[derive(Debug, Default, Deserialize)]
struct DispatchForm {
    #[serde(default)]
    image: String,
    #[serde(default)]
    git_ref: String,
}

fn clean_form_field(raw: &str) -> Option<String> {
    Some(raw.trim().to_string()).filter(|value| !value.is_empty())
}

async fn dispatch_job(
    State(state): State<AppState>,
    auth_session: AuthSession,
    Path((bear_slug, job_ref)): Path<(String, String)>,
    Form(form): Form<DispatchForm>,
) -> Result<Response, CustomError> {
    let bear = bear_context(&state, &auth_session, &bear_slug).await?;
    let job_id = resolve_job_prefix(state.sqlx_pool(), bear.id, &job_ref).await?;
    let user_id = require_user(&auth_session)?;
    let bears = member_bears(&state, user_id).await?;
    let bear_id: Option<Uuid> =
        sqlx::query_scalar!("SELECT bear_id FROM bear_jobs WHERE id = $1", job_id)
            .fetch_optional(state.sqlx_pool())
            .await
            .map_err(den_core::DenError::from)?;
    let Some(bear_id) = bear_id.filter(|bear_id| bears.contains_key(bear_id)) else {
        return Err(CustomError::NotFound("job not found".to_string()));
    };
    let runs = work_runs::enqueue_work_job(
        state.sqlx_pool(),
        work_runs::WorkJobEnqueue {
            bear_id,
            job_id,
            durable_result: den_docket::DurableResultKind::RepositoryChanges,
            git_ref: clean_form_field(&form.git_ref),
            image_name: clean_form_field(&form.image),
            requested_by_user_id: Some(user_id),
            execution_target: work_runs::WorkExecutionTarget::Sandbox,
            attachment_warning: None,
        },
    )
    .await?;
    let run = runs.last().ok_or_else(|| {
        CustomError::ValidationError("dispatch did not create a work run".to_string())
    })?;
    Ok(Redirect::to(&format!(
        "/bear/{}/jobs/runs/{}",
        bear.slug,
        route_id(run.id)
    ))
    .into_response())
}

async fn pause_run(
    State(state): State<AppState>,
    auth_session: AuthSession,
    Path((bear_slug, run_ref)): Path<(String, String)>,
) -> Result<Response, CustomError> {
    steer_run(&state, &auth_session, &bear_slug, &run_ref, true).await
}

async fn resume_run(
    State(state): State<AppState>,
    auth_session: AuthSession,
    Path((bear_slug, run_ref)): Path<(String, String)>,
) -> Result<Response, CustomError> {
    steer_run(&state, &auth_session, &bear_slug, &run_ref, false).await
}

async fn steer_run(
    state: &AppState,
    auth_session: &AuthSession,
    bear_slug: &str,
    run_ref: &str,
    paused: bool,
) -> Result<Response, CustomError> {
    let bear = bear_context(state, auth_session, bear_slug).await?;
    let run_id = resolve_run_prefix(state.sqlx_pool(), bear.id, run_ref).await?;
    let user_id = require_user(auth_session)?;
    let bears = member_bears(state, user_id).await?;
    let run = work_runs::get_work_run(state.sqlx_pool(), run_id)
        .await?
        .filter(|run| bears.contains_key(&run.bear_id))
        .ok_or_else(|| CustomError::NotFound("work run not found".to_string()))?;
    if !den_docket::supervisor::set_work_run_paused(state.sqlx_pool(), run.id, paused).await? {
        return Err(CustomError::ValidationError(format!(
            "run changed state before it could be {}; refresh and try again",
            if paused { "paused" } else { "resumed" }
        )));
    }
    Ok(Redirect::to(&format!(
        "/bear/{}/jobs/runs/{}",
        bear.slug,
        route_id(run.id)
    ))
    .into_response())
}

async fn cancel_job_run(
    State(state): State<AppState>,
    auth_session: AuthSession,
    Path((bear_slug, job_ref)): Path<(String, String)>,
) -> Result<Response, CustomError> {
    let bear = bear_context(&state, &auth_session, &bear_slug).await?;
    let job_id = resolve_job_prefix(state.sqlx_pool(), bear.id, &job_ref).await?;
    PgDocketService::from_pool(state.sqlx_pool())
        .cancel_job_run(bear.id, job_id)
        .await?;
    Ok(Redirect::to(&format!("/bear/{}/jobs/{}", bear.slug, route_id(job_id))).into_response())
}

async fn cancel_run(
    State(state): State<AppState>,
    auth_session: AuthSession,
    Path((bear_slug, run_ref)): Path<(String, String)>,
) -> Result<Response, CustomError> {
    let bear = bear_context(&state, &auth_session, &bear_slug).await?;
    let run_id = resolve_run_prefix(state.sqlx_pool(), bear.id, &run_ref).await?;
    let user_id = require_user(&auth_session)?;
    let bears = member_bears(&state, user_id).await?;
    let run = work_runs::get_work_run(state.sqlx_pool(), run_id)
        .await?
        .filter(|run| bears.contains_key(&run.bear_id))
        .ok_or_else(|| CustomError::NotFound("work run not found".to_string()))?;
    work_runs::request_work_run_cancel_with_provenance(
        state.sqlx_pool(),
        run.id,
        run.bear_id,
        &work_runs::WorkRunCancelRequest {
            requested_by: format!("web:user:{user_id}"),
            reason: "cancelled from the Den web UI".into(),
        },
    )
    .await?;
    Ok(Redirect::to(&format!(
        "/bear/{}/jobs/runs/{}",
        bear.slug,
        route_id(run_id)
    ))
    .into_response())
}

async fn retry_run(
    State(state): State<AppState>,
    auth_session: AuthSession,
    Path((bear_slug, run_ref)): Path<(String, String)>,
) -> Result<Response, CustomError> {
    let bear = bear_context(&state, &auth_session, &bear_slug).await?;
    let run_id = resolve_run_prefix(state.sqlx_pool(), bear.id, &run_ref).await?;
    let user_id = require_user(&auth_session)?;
    let bears = member_bears(&state, user_id).await?;
    let run = work_runs::get_work_run(state.sqlx_pool(), run_id)
        .await?
        .filter(|run| bears.contains_key(&run.bear_id))
        .ok_or_else(|| CustomError::NotFound("work run not found".to_string()))?;
    let retry = if run.execution_target == "attached_armature" {
        work_runs::recover_attached_work_run(state.sqlx_pool(), run.id, run.bear_id).await?
    } else {
        let mut retries = work_runs::enqueue_work_job(
            state.sqlx_pool(),
            work_runs::WorkJobEnqueue {
                bear_id: run.bear_id,
                job_id: run.job_id,
                durable_result: den_docket::DurableResultKind::RepositoryChanges,
                git_ref: run.git_ref.clone(),
                image_name: run.image_name.clone(),
                requested_by_user_id: Some(user_id),
                execution_target: work_runs::WorkExecutionTarget::Sandbox,
                attachment_warning: None,
            },
        )
        .await?;
        retries
            .pop()
            .expect("enqueue_work_job returns one job-scoped work run")
    };
    Ok(Redirect::to(&format!(
        "/bear/{}/jobs/runs/{}",
        bear.slug,
        route_id(retry.id)
    ))
    .into_response())
}
