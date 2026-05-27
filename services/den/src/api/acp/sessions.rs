use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use uuid::Uuid;

use crate::{
    core::{acp_sessions, work_plans::WorkPlanProjection},
    errors::CustomError,
};

use super::{
    format_acp_session_timestamp, workflow_state_json_from_sources, AcpResolvedTurnContext,
    AcpSessionHttp,
};
use time::format_description::well_known::Rfc3339;

#[derive(Debug, Clone)]
pub(super) struct AcpSessionsCursor {
    pub(super) updated_at: time::OffsetDateTime,
    pub(super) id: Uuid,
}

pub(super) fn encode_acp_sessions_cursor(row: &acp_sessions::AcpSessionRow) -> String {
    let payload = serde_json::json!({
        "updated_at": format_acp_session_timestamp(row.updated_at),
        "id": row.id,
    });
    URL_SAFE_NO_PAD.encode(
        serde_json::to_string(&payload)
            .unwrap_or_else(|_| r#"{}"#.to_string())
            .as_bytes(),
    )
}

pub(super) fn decode_acp_sessions_cursor(
    raw: Option<&str>,
) -> Result<Option<AcpSessionsCursor>, CustomError> {
    let Some(raw) = raw.map(str::trim).filter(|s| !s.is_empty()) else {
        return Ok(None);
    };
    let bytes = URL_SAFE_NO_PAD
        .decode(raw.as_bytes())
        .map_err(|_| CustomError::ValidationError("invalid sessions cursor".to_string()))?;
    let value: serde_json::Value = serde_json::from_slice(&bytes)
        .map_err(|_| CustomError::ValidationError("invalid sessions cursor payload".to_string()))?;
    if value.get("offset").is_some() {
        return Err(CustomError::ValidationError(
            "stale offset-based sessions cursor; restart pagination".to_string(),
        ));
    }
    let updated_at_raw = value
        .get("updated_at")
        .and_then(|v| v.as_str())
        .ok_or_else(|| {
            CustomError::ValidationError("invalid sessions cursor updated_at".to_string())
        })?;
    let updated_at = time::OffsetDateTime::parse(updated_at_raw, &Rfc3339).map_err(|_| {
        CustomError::ValidationError("invalid sessions cursor updated_at".to_string())
    })?;
    let id = value
        .get("id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| CustomError::ValidationError("invalid sessions cursor id".to_string()))?
        .parse::<Uuid>()
        .map_err(|_| CustomError::ValidationError("invalid sessions cursor id".to_string()))?;
    Ok(Some(AcpSessionsCursor { updated_at, id }))
}

pub(crate) fn resolve_acp_turn_context(
    row: &acp_sessions::AcpSessionRow,
    plan_mode_row: Option<&crate::core::acp_plan_mode::AcpPlanModeSessionRow>,
    activity_plan: Option<&WorkPlanProjection>,
) -> AcpResolvedTurnContext {
    let policy = super::resolve_session_policy_for_mode(
        &row.current_mode,
        plan_mode_row.map(|value| value.state.as_str()),
    );
    let workflow_state = workflow_state_json_from_sources(&policy, plan_mode_row, activity_plan);
    let effective_mode = policy.mode_label.to_ascii_lowercase();
    AcpResolvedTurnContext {
        policy,
        workflow_state,
        effective_mode,
    }
}

pub(crate) fn acp_session_row_to_http_with_modes(
    row: acp_sessions::AcpSessionRow,
    plan_mode: Option<serde_json::Value>,
) -> AcpSessionHttp {
    let plan_mode_row = plan_mode
        .as_ref()
        .and_then(|value| serde_json::from_value(value.clone()).ok());
    let turn_context = resolve_acp_turn_context(&row, plan_mode_row.as_ref(), None);
    AcpSessionHttp {
        acp_session_id: row.acp_session_id,
        runtime_session_id: row.runtime_session_id,
        conversation_id: row.conversation_id,
        resolved_conversation_id: row.resolved_conversation_id,
        client: row.client,
        cwd: row.cwd,
        title: row.conversation_title,
        conversation_title_updated_at: row
            .conversation_title_updated_at
            .map(format_acp_session_timestamp),
        conversation_title_synced_at: row
            .conversation_title_synced_at
            .map(format_acp_session_timestamp),
        closed_at: row.closed_at.map(format_acp_session_timestamp),
        archived_at: row.archived_at.map(format_acp_session_timestamp),
        created_at: format_acp_session_timestamp(row.created_at),
        updated_at: format_acp_session_timestamp(row.updated_at),
        plan_mode,
        session_policy: turn_context.policy.to_json(),
        workflow_state: turn_context.workflow_state,
    }
}
