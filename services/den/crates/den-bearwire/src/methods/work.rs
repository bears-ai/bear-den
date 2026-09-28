//! `work.*` methods: the in-sandbox headless armature's handshake with its
//! dispatched work run.
//!
//! `work.checkout` binds the armature's BearWire session to the
//! `bear_work_runs` row it was launched for, opens the Docket execution
//! session whose job/task focus satisfies the Work-stance gate, and returns
//! the prompt built Den-side from the durable task definition.
//! `work.report` stores the armature's advisory summary; the authoritative
//! outcome is the run-completion hook plus Docket task state.

use axum::http::{header, HeaderMap};
use bearwire_protocol::compatibility::{CompatibilityManifest, REQUIRED_WORK_CAPABILITIES};
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

use den_core::BearProfile;
use den_docket::{
    work_runs, DocketCheckpointDirectiveAcknowledge, DocketService, DocketWorkBoundaryCheck,
    DocketWorkBoundarySignal, PgDocketService, TaskListVisibility,
};
use den_http::{armature_tokens, errors::CustomError};
use den_service::client_sessions;
use den_service::{
    artifacts::{
        self, ArtifactStorageKind, ArtifactVisibility, AttachArtifactInput,
        CreateJsonArtifactInput, ReserveArtifactInput,
    },
    bears::{db as bears_db, render_turn_fragment, repository_prompt_fragment_registry},
    DenState,
};

use crate::auth::authenticated_bear;
use crate::methods::parse_params;

// Resolve the canonical Work run -> Job relationship before consulting any run,
// attempt, directive, or prompt content. Never authorize from a caller-supplied
// session name, work order, or an attempt's unverified binding alone.
struct AuthorizedWorkRun {
    session_id: Option<String>,
}

async fn require_work_run_access(
    state: &DenState,
    headers: &HeaderMap,
    bear_id: Uuid,
    user_id: i32,
    run_id: Uuid,
    checkout_session_id: Option<&str>,
) -> Result<AuthorizedWorkRun, CustomError> {
    let run = work_runs::get_work_run(&state.sqlx_pool, run_id)
        .await?
        .filter(|run| run.bear_id == bear_id)
        .ok_or_else(|| CustomError::NotFound("work run not found".to_string()))?;
    let job = PgDocketService::from_pool(&state.sqlx_pool)
        .get_job(bear_id, run.job_id)
        .await?
        .ok_or_else(|| CustomError::NotFound("work run not found".to_string()))?;
    let role = bears_db::membership_role_for_user(&state.sqlx_pool, user_id, bear_id)
        .await?
        .ok_or_else(|| CustomError::NotFound("work run not found".to_string()))?;
    // The enqueue requester's ID is not persisted on bear_work_runs. Only a
    // token explicitly dispatched for this run can act for a different member
    // on a shared job; job visibility or membership alone cannot bind work.
    let visibility = TaskListVisibility::parse(&job.job.visibility)
        .map_err(|_| CustomError::NotFound("work run not found".to_string()))?;
    if job.job.created_by_user_id != user_id && !bears_db::role_is_bear_admin(role.as_deref()) {
        if visibility != TaskListVisibility::BearVisible {
            return Err(CustomError::NotFound("work run not found".to_string()));
        }
        let token_hash = headers
            .get(header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.strip_prefix("Bearer "))
            .map(armature_tokens::hash_raw_token_for_seed);
        let dispatched_token = if let Some(token_hash) = token_hash {
            sqlx::query_scalar!(
                "SELECT EXISTS (SELECT 1 FROM armature_tokens t \
                 WHERE t.token_hash = $1 AND t.id::text = r.result_refs ->> 'armature_token_id' \
                   AND t.user_id = $2) AS \"authorized!\" \
                 FROM bear_work_runs r WHERE r.id = $3 AND r.bear_id = $4",
                token_hash,
                user_id,
                run_id,
                bear_id,
            )
            .fetch_optional(&state.sqlx_pool)
            .await?
            .unwrap_or(false)
        } else {
            false
        };
        if !dispatched_token {
            return Err(CustomError::NotFound("work run not found".to_string()));
        }
    }

    // Client sessions are owned by humans, not by arbitrary Bear members. A
    // sandbox may checkout before session.open, but must not reuse an existing
    // session belonging to another human (including on checkout replay).
    for session_id in [checkout_session_id, run.bearwire_session_id.as_deref()]
        .into_iter()
        .flatten()
    {
        let session_owner = sqlx::query_scalar!(
            "SELECT user_id FROM client_sessions WHERE bear_id = $1 AND client_session_id = $2",
            bear_id,
            session_id,
        )
        .fetch_optional(&state.sqlx_pool)
        .await?;
        if session_owner.is_some_and(|owner| owner != user_id)
            || (session_owner.is_some()
                && client_sessions::find_for_user_bear_session_id(
                    &state.sqlx_pool,
                    user_id,
                    bear_id,
                    session_id,
                )
                .await?
                .is_none())
        {
            return Err(CustomError::NotFound("work run not found".to_string()));
        }
    }
    if let Some(assigned) = run.attached_client_session_id.as_deref() {
        let assigned_owner = sqlx::query_scalar!(
            "SELECT user_id FROM client_sessions WHERE bear_id = $1 AND client_session_id = $2",
            bear_id,
            assigned,
        )
        .fetch_optional(&state.sqlx_pool)
        .await?;
        if assigned_owner.is_some_and(|owner| owner != user_id)
            || (run.execution_target == "attached_armature" && assigned_owner != Some(user_id))
            || checkout_session_id.is_some_and(|session| session != assigned)
        {
            return Err(CustomError::NotFound("work run not found".to_string()));
        }
    }
    if let Some(session_id) = checkout_session_id {
        let conflicting_run = sqlx::query_scalar!(
            "SELECT id FROM bear_work_runs WHERE bearwire_session_id = $1 AND id <> $2 \
             AND state IN ('claimed', 'provisioning', 'running', 'reporting') LIMIT 1",
            session_id,
            run_id,
        )
        .fetch_optional(&state.sqlx_pool)
        .await?;
        if conflicting_run.is_some() {
            return Err(CustomError::NotFound("work run not found".to_string()));
        }
    }
    Ok(AuthorizedWorkRun {
        session_id: run.bearwire_session_id,
    })
}

async fn require_work_attempt_access(
    state: &DenState,
    headers: &HeaderMap,
    bear_id: Uuid,
    user_id: i32,
    attempt_id: Uuid,
    fence_epoch: i64,
    require_bound_session: bool,
) -> Result<Uuid, CustomError> {
    let run_id = sqlx::query_scalar!(
        "SELECT r.id FROM docket_execution_attempts a \
         JOIN bear_work_runs r ON r.id::text = a.binding_id AND r.bear_id = a.bear_id \
         WHERE a.id = $1 AND a.fence_epoch = $2 AND a.binding_kind = 'work_assignment' \
           AND a.bear_id = $3",
        attempt_id,
        fence_epoch,
        bear_id,
    )
    .fetch_optional(&state.sqlx_pool)
    .await?
    .ok_or_else(|| CustomError::NotFound("work execution attempt not found".to_string()))?;
    let access = require_work_run_access(state, headers, bear_id, user_id, run_id, None).await?;
    // A rejected re-checkout clears the run session while a checkpoint is
    // pending. Evidence/acknowledgement still requires the exact attempt and
    // authorized run caller, but boundary checks need an active session binding.
    if require_bound_session && access.session_id.is_none() {
        return Err(CustomError::NotFound(
            "work execution attempt not found".to_string(),
        ));
    }
    Ok(run_id)
}

#[derive(Deserialize)]
struct WorkCheckoutRequest {
    session_id: String,
    work_order_id: Uuid,
    compatibility: CompatibilityManifest,
}

pub(crate) async fn work_checkout_result(
    state: &DenState,
    headers: &HeaderMap,
    params: &Value,
) -> Result<Value, CustomError> {
    let (user_id, bear) = authenticated_bear(state, headers, params).await?;
    let request: WorkCheckoutRequest = parse_params(params)?;

    let missing = request
        .compatibility
        .missing(REQUIRED_WORK_CAPABILITIES)
        .collect::<Vec<_>>();
    if request.compatibility.protocol != 1 || !missing.is_empty() {
        return Err(CustomError::ValidationError(format!(
            "incompatible sandbox armature: protocol={}, required_protocol=1, missing_capabilities={missing:?}",
            request.compatibility.protocol
        )));
    }

    // Authorization and provenance must precede the bind (and its prompt read).
    require_work_run_access(
        state,
        headers,
        bear.id,
        user_id,
        request.work_order_id,
        Some(&request.session_id),
    )
    .await?;
    let checkout = work_runs::checkout_work_run_for_session(
        &state.sqlx_pool,
        request.work_order_id,
        bear.id,
        &request.session_id,
    )
    .await?;

    tracing::info!(
        work_run_id = %checkout.run.id,
        job_id = %checkout.run.job_id,
        task_id = ?checkout.run.executing_task_id,
        session_id = %request.session_id,
        execution_attempt_id = ?checkout.execution_attempt.as_ref().map(|attempt| attempt.id),
        fence_epoch = ?checkout.execution_attempt.as_ref().map(|attempt| attempt.fence_epoch),
        bear_slug = %bear.slug,
        "work.checkout bound armature session to work run"
    );

    let prompt = match checkout.prompt_context.as_ref() {
        Some(prompt_context) => {
            let prompt_registry = repository_prompt_fragment_registry()?;
            let prompt_fragment = prompt_registry.require("runtime_work_checkout")?;
            render_turn_fragment(prompt_fragment, &json!({ "work": prompt_context }))?
        }
        None => String::new(),
    };

    let authorized = matches!(
        &checkout.gate,
        den_docket::DocketExecutionGate::Allowed { .. }
    );
    Ok(json!({
        "ok": authorized,
        "work_run_id": checkout.run.id,
        "job_id": checkout.run.job_id,
        "task_title": checkout.task_title,
        "gate": checkout.gate,
        "attempt": checkout.run.attempt,
        "execution_attempt_id": checkout.execution_attempt.as_ref().map(|attempt| attempt.id),
        "execution_attempt_fence_epoch": checkout.execution_attempt.as_ref().map(|attempt| attempt.fence_epoch),
        "prompt": prompt,
        "permission_mode": if authorized { "workspace_write" } else { "none" },
        // Deadline is enforced by the sandbox provider + armature env; no
        // per-run override is stored yet.
        "deadline_secs": Value::Null,
    }))
}

pub(crate) async fn work_boundary_result(
    state: &DenState,
    headers: &HeaderMap,
    params: &Value,
) -> Result<Value, CustomError> {
    let (user_id, bear) = authenticated_bear(state, headers, params).await?;
    let request: WorkBoundaryRequest = parse_params(params)?;
    require_work_attempt_access(
        state,
        headers,
        bear.id,
        user_id,
        request.execution_attempt_id,
        request.fence_epoch,
        true,
    )
    .await?;
    let gate = PgDocketService::from_pool(&state.sqlx_pool)
        .check_work_boundary(DocketWorkBoundaryCheck {
            bear_id: bear.id,
            attempt_id: request.execution_attempt_id,
            fence_epoch: request.fence_epoch,
            boundary_key: request.boundary_key,
            signal: request.signal,
        })
        .await?;
    Ok(
        json!({ "ok": matches!(gate, den_docket::DocketExecutionGate::Allowed { .. }), "gate": gate }),
    )
}

#[derive(Deserialize)]
struct WorkBoundaryRequest {
    execution_attempt_id: Uuid,
    fence_epoch: i64,
    boundary_key: Uuid,
    #[serde(default)]
    signal: Option<DocketWorkBoundarySignal>,
}

#[derive(Deserialize)]
struct WorkCheckpointEvidenceRequest {
    directive_id: Uuid,
    execution_attempt_id: Uuid,
    fence_epoch: i64,
    summary: String,
}

pub(crate) async fn work_checkpoint_evidence_result(
    state: &DenState,
    headers: &HeaderMap,
    params: &Value,
) -> Result<Value, CustomError> {
    let (user_id, bear) = authenticated_bear(state, headers, params).await?;
    let request: WorkCheckpointEvidenceRequest = parse_params(params)?;
    let summary = request.summary.trim();
    if summary.is_empty() || summary.len() > 4_000 {
        return Err(CustomError::ValidationError(
            "checkpoint summary must be 1..=4000 characters".to_string(),
        ));
    }
    let authorized_run_id = require_work_attempt_access(
        state,
        headers,
        bear.id,
        user_id,
        request.execution_attempt_id,
        request.fence_epoch,
        false,
    )
    .await?;
    // The directive, evidence artifact, link, and released fence form one
    // handoff. Keep all of them in this transaction so a failed acknowledgement
    // cannot leave an orphaned checkpoint artifact behind.
    let mut tx = state.sqlx_pool.begin().await?;
    let work_run_id: Uuid = sqlx::query_scalar(
        "SELECT r.id FROM docket_execution_attempts a JOIN bear_work_runs r ON r.id::text = a.binding_id AND r.bear_id = a.bear_id WHERE a.id = $1 AND a.fence_epoch = $2 AND a.binding_kind = 'work_assignment' AND a.bear_id = $3",
    )
    .bind(request.execution_attempt_id)
    .bind(request.fence_epoch)
    .bind(bear.id)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| CustomError::NotFound("work execution attempt not found".to_string()))?;
    if work_run_id != authorized_run_id {
        return Err(CustomError::NotFound(
            "work execution attempt not found".to_string(),
        ));
    }
    let directive: Option<(String, Option<String>)> = sqlx::query_as(
        "SELECT directive.state, directive.acknowledged_artifact_ref \
         FROM docket_checkpoint_directives directive \
         JOIN docket_execution_attempts attempt ON attempt.id = directive.execution_attempt_id \
         WHERE directive.id = $1 AND directive.execution_attempt_id = $2 \
           AND directive.fence_epoch = $3 AND attempt.binding_kind = 'work_assignment' \
           AND attempt.bear_id = $4 FOR UPDATE",
    )
    .bind(request.directive_id)
    .bind(request.execution_attempt_id)
    .bind(request.fence_epoch)
    .bind(bear.id)
    .fetch_optional(&mut *tx)
    .await?;
    let Some((directive_state, acknowledged_artifact_ref)) = directive else {
        return Err(CustomError::NotFound(
            "checkpoint directive is not pending for this exact attempt and fence".to_string(),
        ));
    };
    if directive_state == "acknowledged" {
        let artifact_ref = acknowledged_artifact_ref.ok_or_else(|| {
            CustomError::ValidationError(
                "acknowledged checkpoint directive has no artifact".to_string(),
            )
        })?;
        tx.commit().await?;
        return Ok(
            json!({ "ok": true, "directive_id": request.directive_id, "checkpoint_artifact_ref": artifact_ref }),
        );
    }
    if directive_state != "pending" {
        return Err(CustomError::NotFound(
            "checkpoint directive is not pending for this exact attempt and fence".to_string(),
        ));
    }
    let artifact = artifacts::create_json_artifact_in_tx(
        &mut tx,
        CreateJsonArtifactInput {
            reserve: ReserveArtifactInput {
                bear_id: bear.id,
                created_by_user_id: Some(user_id),
                owner_profile: BearProfile::Work,
                kind: "runtime_checkpoint".to_string(),
                title: Some("Work checkpoint acknowledgement".to_string()),
                summary: Some(summary.to_string()),
                content_type: Some("application/json".to_string()),
                storage_kind: ArtifactStorageKind::DbText,
                visibility: ArtifactVisibility::PrivateToProfile,
                provenance: json!({ "creating_stance": "work", "directive_id": request.directive_id, "execution_attempt_id": request.execution_attempt_id, "fence_epoch": request.fence_epoch }),
                metadata: json!({}),
                expires_at: None,
            },
            payload: json!({ "directive_id": request.directive_id, "execution_attempt_id": request.execution_attempt_id, "fence_epoch": request.fence_epoch, "summary": summary }),
        },
    )
    .await?;
    artifacts::attach_artifact_in_tx(
        &mut tx,
        AttachArtifactInput {
            artifact_ref: artifact.artifact_ref.clone(),
            bear_id: bear.id,
            target_kind: "work_run".to_string(),
            target_id: work_run_id.to_string(),
            role: "runtime_checkpoint".to_string(),
            metadata: json!({}),
            created_by_user_id: Some(user_id),
        },
    )
    .await?;
    sqlx::query(
        "UPDATE docket_checkpoint_directives SET state = 'acknowledged', \
         acknowledged_artifact_ref = $2, acknowledged_at = NOW() \
         WHERE id = $1 AND state = 'pending'",
    )
    .bind(request.directive_id)
    .bind(&artifact.artifact_ref)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "UPDATE docket_execution_attempts SET state = 'released', released_at = NOW(), \
         updated_at = NOW() WHERE id = $1 AND fence_epoch = $2 AND state = 'running'",
    )
    .bind(request.execution_attempt_id)
    .bind(request.fence_epoch)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(
        json!({ "ok": true, "directive_id": request.directive_id, "checkpoint_artifact_ref": artifact.artifact_ref }),
    )
}

#[derive(Deserialize)]
struct WorkAcknowledgeCheckpointRequest {
    directive_id: Uuid,
    execution_attempt_id: Uuid,
    fence_epoch: i64,
    checkpoint_artifact_ref: String,
}

pub(crate) async fn work_acknowledge_checkpoint_result(
    state: &DenState,
    headers: &HeaderMap,
    params: &Value,
) -> Result<Value, CustomError> {
    let (user_id, bear) = authenticated_bear(state, headers, params).await?;
    let request: WorkAcknowledgeCheckpointRequest = parse_params(params)?;
    require_work_attempt_access(
        state,
        headers,
        bear.id,
        user_id,
        request.execution_attempt_id,
        request.fence_epoch,
        false,
    )
    .await?;
    let directive = PgDocketService::from_pool(&state.sqlx_pool)
        .acknowledge_checkpoint_directive(DocketCheckpointDirectiveAcknowledge {
            bear_id: bear.id,
            directive_id: request.directive_id,
            execution_attempt_id: request.execution_attempt_id,
            fence_epoch: request.fence_epoch,
            artifact_ref: request.checkpoint_artifact_ref,
        })
        .await?;

    Ok(json!({
        "ok": true,
        "directive_id": directive.id,
        "state": directive.state,
        "acknowledged_artifact_ref": directive.acknowledged_artifact_ref,
    }))
}
#[derive(Deserialize)]
struct WorkReportRequest {
    session_id: String,
    work_order_id: Uuid,
    #[serde(default)]
    status_hint: Option<String>,
    #[serde(default)]
    summary: Option<String>,
}

pub(crate) async fn work_report_result(
    state: &DenState,
    headers: &HeaderMap,
    params: &Value,
) -> Result<Value, CustomError> {
    let (user_id, bear) = authenticated_bear(state, headers, params).await?;
    let request: WorkReportRequest = parse_params(params)?;
    let access = require_work_run_access(
        state,
        headers,
        bear.id,
        user_id,
        request.work_order_id,
        None,
    )
    .await?;
    if access.session_id.as_deref() != Some(request.session_id.as_str()) {
        return Err(CustomError::NotFound("work run not found".to_string()));
    }

    work_runs::record_work_run_report(
        &state.sqlx_pool,
        request.work_order_id,
        bear.id,
        request.status_hint.as_deref().unwrap_or("unknown"),
        request.summary.as_deref().unwrap_or(""),
    )
    .await?;

    Ok(json!({ "ok": true }))
}
