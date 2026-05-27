use axum::{
    body::Body,
    http::{header, HeaderName, HeaderValue, Response, StatusCode},
    response::{IntoResponse, Response as AxumResponse},
};
use uuid::Uuid;

use crate::{
    api::acp::AdapterContract,
    core::acp_tool_turns::AcpToolResultRequest,
};

pub(super) const BEARS_ACP_ADAPTER_CONTRACT_NAME: &str = "bears.acp.adapter";
pub(super) const BEARS_ACP_ADAPTER_CONTRACT_CURRENT: u32 = 1;
pub(super) const BEARS_ACP_ADAPTER_CONTRACT_MIN_SUPPORTED: u32 = 1;
pub(super) const BEARS_ACP_ADAPTER_CONTRACT_MAX_SUPPORTED: u32 = 1;
// Missing contract metadata is accepted for compatibility with already-running
// adapter processes. Set this to true only when a Den change is actually
// incompatible with adapters that do not send `adapter_contract`.
pub(super) const BEARS_ACP_ADAPTER_CONTRACT_REQUIRED: bool = false;

#[derive(Debug, Clone, Copy)]
pub(super) enum AcpCompatibilityError {
    AdapterOutOfDate { version: u32 },
    DenOutOfDate { version: u32 },
}

#[derive(serde::Serialize)]
struct AcpErrorResponse {
    error: String,
    error_code: &'static str,
    request_id: String,
    adapter_contract_version: Option<u32>,
    minimum_adapter_contract_version: Option<u32>,
    current_adapter_contract_version: Option<u32>,
    maximum_adapter_contract_version: Option<u32>,
    suggested_action: Option<&'static str>,
}

pub(super) fn adapter_contract_from_value(value: &serde_json::Value) -> Option<AdapterContract> {
    serde_json::from_value(value.get("adapter_contract")?.clone()).ok()
}

pub(super) fn check_adapter_contract(
    contract: Option<&AdapterContract>,
) -> Result<(), AcpCompatibilityError> {
    let Some(contract) = contract else {
        if BEARS_ACP_ADAPTER_CONTRACT_REQUIRED {
            return Err(AcpCompatibilityError::AdapterOutOfDate { version: 0 });
        }
        return Ok(());
    };
    if contract.name != BEARS_ACP_ADAPTER_CONTRACT_NAME {
        return Err(AcpCompatibilityError::AdapterOutOfDate {
            version: contract.version,
        });
    }
    if contract.version < BEARS_ACP_ADAPTER_CONTRACT_MIN_SUPPORTED {
        return Err(AcpCompatibilityError::AdapterOutOfDate {
            version: contract.version,
        });
    }
    if contract.version > BEARS_ACP_ADAPTER_CONTRACT_MAX_SUPPORTED {
        return Err(AcpCompatibilityError::DenOutOfDate {
            version: contract.version,
        });
    }
    Ok(())
}

pub(super) fn acp_compatibility_error_response(
    err: AcpCompatibilityError,
    request_id: Uuid,
) -> AxumResponse {
    let (status, error_code, error, suggested_action, adapter_version) = match err {
        AcpCompatibilityError::AdapterOutOfDate { version } => (
            StatusCode::UPGRADE_REQUIRED,
            "adapter_out_of_date",
            "The BEARS ACP adapter is older than this Den server.",
            "Update bears-acp-adapter and restart your ACP client.",
            version,
        ),
        AcpCompatibilityError::DenOutOfDate { version } => (
            StatusCode::CONFLICT,
            "den_out_of_date",
            "This BEARS Den server is older than the ACP adapter.",
            "Deploy the matching BEARS Den server or use an older adapter.",
            version,
        ),
    };
    tracing::warn!(
        %request_id,
        error_code,
        adapter_contract_version = adapter_version,
        minimum_adapter_contract_version = BEARS_ACP_ADAPTER_CONTRACT_MIN_SUPPORTED,
        current_adapter_contract_version = BEARS_ACP_ADAPTER_CONTRACT_CURRENT,
        "ACP adapter contract mismatch"
    );
    let request_id_header = HeaderValue::from_str(&request_id.to_string())
        .unwrap_or_else(|_| HeaderValue::from_static("invalid"));
    let body = serde_json::to_string(&AcpErrorResponse {
        error: error.to_string(),
        error_code,
        request_id: request_id.to_string(),
        adapter_contract_version: Some(adapter_version),
        minimum_adapter_contract_version: Some(BEARS_ACP_ADAPTER_CONTRACT_MIN_SUPPORTED),
        current_adapter_contract_version: Some(BEARS_ACP_ADAPTER_CONTRACT_CURRENT),
        maximum_adapter_contract_version: Some(BEARS_ACP_ADAPTER_CONTRACT_MAX_SUPPORTED),
        suggested_action: Some(suggested_action),
    })
    .unwrap_or_else(|_| "{\"error\":\"response serialization failed\"}".to_string());
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "application/json; charset=utf-8")
        .header(HeaderName::from_static("x-request-id"), request_id_header)
        .body(Body::from(body))
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

pub(super) fn compatibility_tool_result_body(
    err: &AcpCompatibilityError,
    tool_call_id: &str,
    mut original: AcpToolResultRequest,
) -> AcpToolResultRequest {
    let (status, message, phase) = match err {
        AcpCompatibilityError::AdapterOutOfDate { .. } => (
            "error",
            "The BEARS ACP adapter is older than this Den server. Update bears-acp-adapter and restart your ACP client.",
            "adapter_contract_out_of_date",
        ),
        AcpCompatibilityError::DenOutOfDate { .. } => (
            "error",
            "This BEARS Den server is older than the ACP adapter. Deploy the matching Den server or use an older adapter.",
            "den_contract_out_of_date",
        ),
    };
    original.tool_call_id = Some(tool_call_id.to_string());
    original.status = status.to_string();
    original.content = Some(message.to_string());
    original.structured_content = serde_json::json!({});
    original.diagnostic = serde_json::json!({
        "component": "den.acp",
        "phase": phase,
        "tool_call_id": tool_call_id,
        "minimum_adapter_contract_version": BEARS_ACP_ADAPTER_CONTRACT_MIN_SUPPORTED,
        "current_adapter_contract_version": BEARS_ACP_ADAPTER_CONTRACT_CURRENT,
        "maximum_adapter_contract_version": BEARS_ACP_ADAPTER_CONTRACT_MAX_SUPPORTED,
    });
    original
}
