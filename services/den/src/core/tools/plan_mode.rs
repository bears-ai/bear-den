//! `den`-side wiring for the client plan-mode tools.
//!
//! Argument parsing/validation and the static response envelopes now live in
//! `den_core::tools::plan_mode`; this module provides the concrete [`PlanModeOps`]
//! implementation (DB rows, mode switches, canonical plan submission,
//! `turn_state` rendering), wired into the dispatcher via `DenToolContext`. See
//! `docs/roadmap/DEN_CRATE_SPLIT_PLAN.md` (Phase B).

use serde_json::{json, Value};
use sqlx::PgPool;
use uuid::Uuid;

use den_core::tools::plan_mode::{PlanModeExitView, PlanModeOps, PlanModeStatusView, PlanModeView};

use crate::{core::tools::session::DenToolInvocationContext, errors::DenError};
use den_core::client_tools::{ResolvedSessionPolicy, ToolEnablementState};
use den_runtime::{
    plan_mode::{
        self, EnterPlanModeParams, PlanModeRequestedBy, PlanModeSessionRow, SubmitPlanModeParams,
    },
    turn_state,
};
use den_service::{client_sessions, client_sessions::ClientSessionMode};

type WorkplanPayloadFn = fn(&PlanModeSessionRow) -> Value;
type NoActiveWorkplanFn = fn() -> Value;

fn workflow_state_json(
    mode_label: &'static str,
    tool_enablement: ToolEnablementState,
    plan_mode_state: String,
) -> Value {
    turn_state::turn_state_json(
        &ResolvedSessionPolicy {
            mode_label,
            tool_enablement,
            plan_mode_state: Some(plan_mode_state),
        },
        None,
    )
}

/// Concrete [`PlanModeOps`] over the canonical Postgres state.
pub(crate) struct DenPlanModeOps<'a> {
    pub(crate) pool: &'a PgPool,
    pub(crate) workplan_payload: WorkplanPayloadFn,
    pub(crate) no_active_workplan: NoActiveWorkplanFn,
}

impl PlanModeOps for DenPlanModeOps<'_> {
    async fn enter(
        &self,
        context: &DenToolInvocationContext,
        client_session_id: &str,
        reason: String,
        previous_permission_mode: Option<String>,
    ) -> Result<PlanModeView, DenError> {
        let row = plan_mode::enter_plan_mode(
            self.pool,
            EnterPlanModeParams {
                user_id: context.user_id,
                bear_id: context.bear_id,
                bear_slug: context.bear_slug.clone(),
                client_session_id: client_session_id.to_string(),
                reason,
                requested_by: PlanModeRequestedBy::Pair,
                previous_permission_mode,
            },
        )
        .await?;
        client_sessions::set_current_mode(
            self.pool,
            context.user_id,
            context.bear_id,
            client_session_id,
            ClientSessionMode::Plan,
        )
        .await?;
        Ok(PlanModeView {
            workplan: (self.workplan_payload)(&row),
            workflow_state: workflow_state_json(
                "Plan",
                ToolEnablementState::ReadOnly,
                row.state.clone(),
            ),
            plan_mode: serde_json::to_value(&row)?,
        })
    }

    async fn status(
        &self,
        context: &DenToolInvocationContext,
        client_session_id: &str,
    ) -> Result<PlanModeStatusView, DenError> {
        let row = plan_mode::active_for_session(
            self.pool,
            context.user_id,
            context.bear_id,
            client_session_id,
        )
        .await?;
        let workplan = row
            .as_ref()
            .map(self.workplan_payload)
            .unwrap_or_else(self.no_active_workplan);
        Ok(PlanModeStatusView {
            workplan,
            active: row.is_some(),
            plan_mode: serde_json::to_value(&row)?,
        })
    }

    async fn record_approval(
        &self,
        context: &DenToolInvocationContext,
        client_session_id: &str,
        plan_mode_id: Option<Uuid>,
    ) -> Result<PlanModeView, DenError> {
        let current = plan_mode::get_for_session(
            self.pool,
            context.user_id,
            context.bear_id,
            client_session_id,
            plan_mode_id,
        )
        .await?
        .ok_or_else(|| {
            DenError::NotFound("submitted client plan mode session not found".to_string())
        })?;
        if current.state != "submitted" {
            return Err(DenError::ValidationError(format!(
                "plan approval requires a submitted plan; current state is {}",
                current.state
            )));
        }
        let row = plan_mode::approve_plan_mode(
            self.pool,
            context.user_id,
            context.bear_id,
            client_session_id,
            current.id,
        )
        .await?;
        client_sessions::set_current_mode(
            self.pool,
            context.user_id,
            context.bear_id,
            client_session_id,
            ClientSessionMode::Write,
        )
        .await?;
        Ok(PlanModeView {
            workplan: (self.workplan_payload)(&row),
            workflow_state: workflow_state_json(
                "Write",
                ToolEnablementState::AllTools,
                row.state.clone(),
            ),
            plan_mode: serde_json::to_value(&row)?,
        })
    }

    async fn exit(
        &self,
        context: &DenToolInvocationContext,
        client_session_id: &str,
        plan_mode_id: Option<Uuid>,
        title: &str,
        body: &str,
    ) -> Result<PlanModeExitView, DenError> {
        let current_plan = plan_mode::get_for_session(
            self.pool,
            context.user_id,
            context.bear_id,
            client_session_id,
            plan_mode_id,
        )
        .await?
        .ok_or_else(|| {
            DenError::NotFound("active client plan mode session not found".to_string())
        })?;
        // Preserve the canonical plan reference without materializing a second
        // copy in Bear memory. Submission and its audit events live in Postgres.
        let artifact_path = current_plan
            .plan_artifact_path
            .clone()
            .unwrap_or_else(|| format!("pair/plans/plan-mode-{}.md", current_plan.id));
        let row = plan_mode::submit_plan_artifact(
            self.pool,
            SubmitPlanModeParams {
                user_id: context.user_id,
                bear_id: context.bear_id,
                client_session_id: client_session_id.to_string(),
                plan_mode_id: Some(current_plan.id),
                title: title.to_string(),
                body: body.to_string(),
                artifact_path: artifact_path.clone(),
                approval_request_id: Some(format!("plan-mode-{}", current_plan.id)),
            },
        )
        .await?;
        client_sessions::set_current_mode(
            self.pool,
            context.user_id,
            context.bear_id,
            client_session_id,
            ClientSessionMode::Plan,
        )
        .await?;
        let storage = "postgres";
        Ok(PlanModeExitView {
            workplan: (self.workplan_payload)(&row),
            workflow_state: workflow_state_json(
                "Plan",
                ToolEnablementState::ReadOnly,
                row.state.clone(),
            ),
            submitted_plan: json!({
                "title": row.plan_title,
                "body": row.plan_body,
                "artifact_path": row.plan_artifact_path,
            }),
            artifact_path,
            storage: storage.to_string(),
            plan_mode: serde_json::to_value(&row)?,
        })
    }

    async fn cancel(
        &self,
        context: &DenToolInvocationContext,
        client_session_id: &str,
        plan_mode_id: Option<Uuid>,
    ) -> Result<PlanModeView, DenError> {
        let row = plan_mode::cancel_plan_mode(
            self.pool,
            context.user_id,
            context.bear_id,
            client_session_id,
            plan_mode_id,
        )
        .await?;
        client_sessions::set_current_mode(
            self.pool,
            context.user_id,
            context.bear_id,
            client_session_id,
            ClientSessionMode::Ask,
        )
        .await?;
        Ok(PlanModeView {
            workplan: (self.workplan_payload)(&row),
            workflow_state: workflow_state_json(
                "Ask",
                ToolEnablementState::ReadOnly,
                row.state.clone(),
            ),
            plan_mode: serde_json::to_value(&row)?,
        })
    }
}
