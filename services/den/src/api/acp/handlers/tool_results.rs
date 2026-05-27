use axum::{
    extract::{Path, State},
    http::HeaderMap,
    response::{IntoResponse, Response},
    Json,
};
use uuid::Uuid;

use crate::{
    api::{
        acp::{
            compat::{
                acp_compatibility_error_response, adapter_contract_from_value,
                check_adapter_contract, compatibility_tool_result_body,
            },
            responses::{acp_error_response, api_auth_error_response},
            tool_results::acp_tool_result_response_from_delivery,
        },
        auth,
        service::ApiState,
    },
    core::{
        acp_tokens,
        acp_tool_turns::{AcpToolResultDelivery, AcpToolResultRequest},
        acp_tools::{acp_diag_phase, AcpToolStatus},
    },
    errors::CustomError,
};

use super::auth::authenticate_acp_code_token_with_auth;

pub(in crate::api::acp) async fn tool_result(
    State(state): State<ApiState>,
    Path((slug, session_id, tool_call_id)): Path<(String, String, String)>,
    headers: HeaderMap,
    Json(body): Json<AcpToolResultRequest>,
) -> Response {
    let request_id = Uuid::new_v4();
    let contract = body
        .adapter_contract
        .as_ref()
        .and_then(adapter_contract_from_value);
    if let Err(err) = check_adapter_contract(contract.as_ref()) {
        let token = match auth::extract_bearer_token(&headers) {
            Ok(token) => token,
            Err(auth_err) => return api_auth_error_response(auth_err, request_id),
        };
        if let Ok(auth) = authenticate_acp_code_token_with_auth(&state, &token, &slug).await {
            let synthetic = compatibility_tool_result_body(&err, &tool_call_id, body);
            let _ = state.acp_tool_turns.deliver_result(
                auth.user_id,
                &slug,
                &session_id,
                &tool_call_id,
                synthetic,
            );
        }
        return acp_compatibility_error_response(err, request_id);
    }
    match tool_result_inner(state, slug, session_id, tool_call_id, headers, body).await {
        Ok(response) => response,
        Err(err) => acp_error_response(err, request_id),
    }
}

pub(super) async fn tool_result_inner(
    state: ApiState,
    slug: String,
    session_id: String,
    tool_call_id: String,
    headers: HeaderMap,
    body: AcpToolResultRequest,
) -> Result<Response, CustomError> {
    let token = auth::extract_bearer_token(&headers)
        .map_err(|err| CustomError::Authentication(err.message))?;
    let auth = authenticate_acp_code_token_with_auth(&state, &token, &slug).await?;
    if !acp_tokens::scopes_contains(&auth.scopes, acp_tokens::acp_tools_scope()) {
        return Err(CustomError::Authorization(
            "ACP token is missing required acp:tools scope".to_string(),
        ));
    }
    let user_id = auth.user_id;
    let parsed_status = AcpToolStatus::parse(&body.status).ok_or_else(|| {
        CustomError::ValidationError(format!("invalid ACP tool result status: {}", body.status))
    })?;
    let delivery =
        state
            .acp_tool_turns
            .deliver_result(user_id, &slug, &session_id, &tool_call_id, body)?;
    match delivery {
        AcpToolResultDelivery::Delivered {
            mut body,
            request_id,
            bear_id,
            tool_name,
        } => {
            body.status = parsed_status.as_str().to_string();
            tracing::info!(
                request_id = %request_id,
                bear_id = %bear_id,
                acp_session_id = %session_id,
                tool_call_id = %tool_call_id,
                tool_name = %tool_name,
                body_request_id = body.request_id.as_deref(),
                body_tool_call_id = body.tool_call_id.as_deref(),
                body_approval_request_id = body.approval_request_id.as_deref(),
                status = %parsed_status.as_str(),
                content_bytes = body.content.as_deref().map(str::len).unwrap_or(0),
                structured_content_bytes = body.structured_content.to_string().len(),
                diagnostic = ?body.diagnostic,
                phase = acp_diag_phase::DEN_RESULT_DELIVERED,
                "ACP tool result received"
            );
            Ok(Json(acp_tool_result_response_from_delivery(
                AcpToolResultDelivery::Delivered {
                    body,
                    request_id,
                    bear_id,
                    tool_name,
                },
                &session_id,
                tool_call_id,
                parsed_status,
                &state.acp_tool_turns,
            ))
            .into_response())
        }
        delivery @ (AcpToolResultDelivery::TurnMissing { .. }
        | AcpToolResultDelivery::AlreadySettled { .. }
        | AcpToolResultDelivery::RecentlySettled { .. }) => {
            Ok(Json(acp_tool_result_response_from_delivery(
                delivery,
                &session_id,
                tool_call_id,
                parsed_status,
                &state.acp_tool_turns,
            ))
            .into_response())
        }
    }
}
