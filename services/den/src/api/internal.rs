use axum::{
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::post,
    Json, Router,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::{
    api::service::ApiState,
    core::den_tools::{self, DenToolInvocationContext},
    errors::CustomError,
};

pub fn router() -> Router<ApiState> {
    Router::new().route("/den-tools/invoke", post(invoke_den_tool))
}

#[derive(Debug, Deserialize)]
struct InvokeDenToolRequest {
    tool_name: String,
    #[serde(default)]
    arguments: Value,
    context: DenToolInvocationContext,
}

#[derive(Debug, Serialize)]
struct InvokeDenToolResponse {
    ok: bool,
    tool_name: String,
    result: Value,
}

async fn invoke_den_tool(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Json(payload): Json<InvokeDenToolRequest>,
) -> Response {
    if let Err(response) = authorize_internal_request(&state, &headers) {
        return response;
    }

    let tool_name = payload.tool_name.trim().to_string();
    if !den_tools::is_builtin_den_tool(&tool_name) {
        return json_error(
            StatusCode::NOT_FOUND,
            "not_found",
            format!("unknown Den tool: {tool_name}"),
        );
    }

    let request_id = payload.context.request_id.clone().unwrap_or_default();
    tracing::info!(
        tool_name = %tool_name,
        bear_id = %payload.context.bear_id,
        user_id = payload.context.user_id,
        request_id = %request_id,
        "den tool invocation started"
    );

    match den_tools::invoke_den_tool(
        &state.sqlx_pool,
        &tool_name,
        payload.arguments,
        payload.context,
    )
    .await
    {
        Ok(result) => {
            tracing::info!(tool_name = %tool_name, request_id = %request_id, "den tool invocation finished");
            Json(InvokeDenToolResponse {
                ok: true,
                tool_name,
                result,
            })
            .into_response()
        }
        Err(err) => map_tool_error(err),
    }
}

fn authorize_internal_request(state: &ApiState, headers: &HeaderMap) -> Result<(), Response> {
    let expected = state.config.codepool_internal_token.trim();
    if expected.is_empty() {
        return Ok(());
    }
    let Some(raw) = headers.get(axum::http::header::AUTHORIZATION) else {
        return Err(json_error(
            StatusCode::UNAUTHORIZED,
            "unauthorized",
            "missing Authorization header",
        ));
    };
    let value = raw.to_str().unwrap_or_default();
    let ok = value == expected || value == format!("Bearer {expected}");
    if ok {
        Ok(())
    } else {
        Err(json_error(
            StatusCode::UNAUTHORIZED,
            "unauthorized",
            "invalid internal token",
        ))
    }
}

fn map_tool_error(err: CustomError) -> Response {
    match err {
        CustomError::Authorization(message) => {
            json_error(StatusCode::FORBIDDEN, "forbidden", message)
        }
        CustomError::Authentication(message) => {
            json_error(StatusCode::UNAUTHORIZED, "unauthorized", message)
        }
        CustomError::NotFound(message) => json_error(StatusCode::NOT_FOUND, "not_found", message),
        CustomError::ValidationError(message) => {
            json_error(StatusCode::BAD_REQUEST, "bad_request", message)
        }
        other => json_error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "tool_error",
            other.to_string(),
        ),
    }
}

fn json_error(status: StatusCode, code: &'static str, message: impl Into<String>) -> Response {
    (
        status,
        Json(json!({
            "ok": false,
            "error": {
                "code": code,
                "message": message.into()
            }
        })),
    )
        .into_response()
}
