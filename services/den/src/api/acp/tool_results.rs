use crate::{
    api::acp::AcpToolResultResponse,
    core::{
        acp_tool_turns::{AcpToolResultDelivery, AcpToolTurnCoordinator},
        acp_tools::{acp_diag_phase, AcpToolStatus},
    },
};

pub(super) fn default_unavailable_context_budget() -> serde_json::Value {
    serde_json::json!({
        "status": "unavailable",
        "reason": "Letta/provider context usage data is not wired into Den session_info yet",
        "source": "den.acp",
    })
}

fn late_result_settlement_from_status(status: &str) -> &'static str {
    match status {
        "timeout" => "timed_out",
        "cancelled" => "cancelled",
        "ok" | "error" | "unsupported" => "already_settled",
        _ => "unknown",
    }
}

pub(super) fn acp_tool_result_response_from_delivery(
    delivery: AcpToolResultDelivery,
    session_id: &str,
    tool_call_id_param: String,
    parsed_status: AcpToolStatus,
    tool_turns: &AcpToolTurnCoordinator,
) -> AcpToolResultResponse {
    match delivery {
        AcpToolResultDelivery::Delivered { body, .. } => AcpToolResultResponse {
            accepted: true,
            reason: "delivered".to_string(),
            settlement: None,
            turn_id: body.turn_id,
            tool_call_id: tool_call_id_param,
            diagnostic: Some(serde_json::json!({
                "component": "den.acp",
                "phase": acp_diag_phase::DEN_RESULT_DELIVERED,
                "status": parsed_status.as_str(),
            })),
        },
        AcpToolResultDelivery::TurnMissing {
            turn_id,
            tool_call_id,
        } => AcpToolResultResponse {
            accepted: false,
            reason: "late_result_ignored".to_string(),
            settlement: Some("unknown".to_string()),
            turn_id,
            tool_call_id,
            diagnostic: Some(serde_json::json!({
                "component": "den.acp",
                "phase": "late_tool_result_ignored",
            })),
        },
        AcpToolResultDelivery::AlreadySettled {
            turn_id,
            tool_call_id,
        } => AcpToolResultResponse {
            accepted: false,
            reason: "late_result_ignored".to_string(),
            settlement: Some("already_settled".to_string()),
            turn_id,
            tool_call_id: tool_call_id.clone(),
            diagnostic: tool_turns
                .recently_settled(session_id, &tool_call_id)
                .map(|cached| cached.diagnostic()),
        },
        AcpToolResultDelivery::RecentlySettled {
            turn_id,
            tool_call_id,
            cached,
        } => AcpToolResultResponse {
            accepted: false,
            reason: "late_result_ignored".to_string(),
            settlement: Some(late_result_settlement_from_status(&cached.status).to_string()),
            turn_id,
            tool_call_id,
            diagnostic: Some(cached.diagnostic()),
        },
    }
}
