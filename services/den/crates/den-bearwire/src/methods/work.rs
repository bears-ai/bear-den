//! `work.*` methods: the in-sandbox headless armature's handshake with its
//! dispatched work run.
//!
//! `work.checkout` binds the armature's BearWire session to the
//! `bear_work_runs` row it was launched for, opens the Docket execution
//! session whose job/task focus satisfies the Work-stance gate, and returns
//! the prompt built Den-side from the durable task definition.
//! `work.report` stores the armature's advisory summary; the authoritative
//! outcome is the run-completion hook plus Docket task state.

use axum::http::HeaderMap;
use bearwire_protocol::compatibility::{CompatibilityManifest, REQUIRED_WORK_CAPABILITIES};
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

use den_core::BearProfile;
use den_docket::{
    work_runs, DocketCheckpointDirectiveAcknowledge, DocketService, DocketWorkBoundaryCheck,
    DocketWorkBoundarySignal, PgDocketService,
};
use den_http::errors::CustomError;
use den_service::{
    artifacts::{
        self, ArtifactStorageKind, ArtifactVisibility, AttachArtifactInput,
        CreateJsonArtifactInput, ReserveArtifactInput,
    },
    bears::{render_turn_fragment, repository_prompt_fragment_registry},
    DenState,
};

use crate::auth::authenticated_bear;
use crate::methods::parse_params;

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
    let (_user_id, bear) = authenticated_bear(state, headers, params).await?;
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

    // Compatibility must be checked before checkout: checkout binds the
    // session and mutates durable run/task state.
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
    let (_user_id, bear) = authenticated_bear(state, headers, params).await?;
    let request: WorkBoundaryRequest = parse_params(params)?;
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
    let (_user_id, bear) = authenticated_bear(state, headers, params).await?;
    let request: WorkCheckpointEvidenceRequest = parse_params(params)?;
    let summary = request.summary.trim();
    if summary.is_empty() || summary.len() > 4_000 {
        return Err(CustomError::ValidationError(
            "checkpoint summary must be 1..=4000 characters".to_string(),
        ));
    }
    // The directive, evidence artifact, link, and released fence form one
    // handoff. Keep all of them in this transaction so a failed acknowledgement
    // cannot leave an orphaned checkpoint artifact behind.
    let mut tx = state.sqlx_pool.begin().await?;
    let work_run_id: Uuid = sqlx::query_scalar(
        "SELECT binding_id::uuid FROM docket_execution_attempts WHERE id = $1 AND fence_epoch = $2 AND binding_kind = 'work_assignment' AND bear_id = $3",
    )
    .bind(request.execution_attempt_id)
    .bind(request.fence_epoch)
    .bind(bear.id)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| CustomError::NotFound("work execution attempt not found".to_string()))?;
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
                created_by_user_id: Some(_user_id),
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
            created_by_user_id: Some(_user_id),
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
    let (_user_id, _bear) = authenticated_bear(state, headers, params).await?;
    let request: WorkAcknowledgeCheckpointRequest = parse_params(params)?;
    let directive = PgDocketService::from_pool(&state.sqlx_pool)
        .acknowledge_checkpoint_directive(DocketCheckpointDirectiveAcknowledge {
            bear_id: _bear.id,
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
    #[allow(dead_code)]
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
    let (_user_id, bear) = authenticated_bear(state, headers, params).await?;
    let request: WorkReportRequest = parse_params(params)?;

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
