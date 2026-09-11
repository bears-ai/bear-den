use axum::{extract::State, http::HeaderMap, response::IntoResponse, Json};
use serde_json::{json, Value};

use bearwire_protocol::rpc::{JsonRpcRequest, JsonRpcResponse};
use den_http::errors::CustomError;
use den_service::DenState;

use crate::methods;

pub(crate) async fn rpc(
    State(state): State<DenState>,
    headers: HeaderMap,
    Json(request): Json<JsonRpcRequest>,
) -> Result<impl IntoResponse, CustomError> {
    if request.jsonrpc.as_deref().unwrap_or("2.0") != "2.0" {
        return Ok(Json(JsonRpcResponse::error(
            request.id,
            -32600,
            "Invalid JSON-RPC version",
            None,
        )));
    }

    let response = match request.method.as_str() {
        "initialize" => JsonRpcResponse::ok(request.id, methods::initialize_result(&state)),
        "session.open" | "session.resume" => method_response(
            request.id,
            methods::session::session_open_result(&state, &headers, &request.params).await,
            format!("BearWire {} failed", request.method),
        ),
        "session.close" => method_response(
            request.id,
            methods::session::session_close_result(&state, &headers, &request.params).await,
            "BearWire session.close failed",
        ),
        "session.compact" => method_response(
            request.id,
            methods::session::session_compact_result(&state, &headers, &request.params).await,
            "BearWire session.compact failed",
        ),
        "session.state" => {
            let session_id = request
                .params
                .get("session_id")
                .and_then(Value::as_str)
                .unwrap_or("<not provided>");
            let bear_slug = request
                .params
                .get("bear_slug")
                .and_then(Value::as_str)
                .unwrap_or("<not provided>");
            let result =
                methods::session::session_state_result(&state, &headers, &request.params).await;
            if let Err(error) = &result {
                tracing::error!(
                    error = %error,
                    rpc_method = "session.state",
                    request_id = ?request.id,
                    bear_slug,
                    session_id,
                    "BearWire session.state request failed"
                );
            }
            method_response(request.id, result, "BearWire session.state failed")
        }
        "session.execution.diagnostics" => method_response(
            request.id,
            methods::session::session_execution_diagnostics_result(
                &state,
                &headers,
                &request.params,
            )
            .await,
            "BearWire session.execution.diagnostics failed",
        ),
        "session.current_task.selection_request" => method_response(
            request.id,
            methods::session::session_current_task_selection_request_result(
                &state,
                &headers,
                &request.params,
            )
            .await,
            "BearWire session.current_task.selection_request failed",
        ),
        "session.current_task.select" => method_response(
            request.id,
            methods::session::session_current_task_select_result(&state, &headers, &request.params)
                .await,
            "BearWire session.current_task.select failed",
        ),
        "session.current_task.start" => method_response(
            request.id,
            methods::session::session_current_task_start_result(&state, &headers, &request.params)
                .await,
            "BearWire session.current_task.start failed",
        ),
        "session.current_task.clear" => method_response(
            request.id,
            methods::session::session_current_task_clear_result(&state, &headers, &request.params)
                .await,
            "BearWire session.current_task.clear failed",
        ),
        "session.model.get" => method_response(
            request.id,
            methods::session::session_model_get_result(&state, &headers, &request.params).await,
            "BearWire session.model.get failed",
        ),
        "session.model.set" => method_response(
            request.id,
            methods::session::session_model_set_result(&state, &headers, &request.params).await,
            "BearWire session.model.set failed",
        ),
        "conversation.history" => method_response(
            request.id,
            methods::conversation::conversation_history_result(&state, &headers, &request.params)
                .await,
            "BearWire conversation.history failed",
        ),
        "conversation.surface_history" => method_response(
            request.id,
            methods::conversation::conversation_surface_history_result(
                &state,
                &headers,
                &request.params,
            )
            .await,
            "BearWire conversation.surface_history failed",
        ),
        "conversation.diagnostics" => method_response(
            request.id,
            methods::conversation::conversation_diagnostics_result(
                &state,
                &headers,
                &request.params,
            )
            .await,
            "BearWire conversation.diagnostics failed",
        ),
        "docket.jobs.list" => method_response(
            request.id,
            methods::docket::docket_jobs_list_result(&state, &headers, &request.params).await,
            "BearWire docket.jobs.list failed",
        ),
        "runtime.diagnostics.list" => method_response(
            request.id,
            methods::docket::runtime_diagnostics_list_result(&state, &headers, &request.params)
                .await,
            "BearWire runtime.diagnostics.list failed",
        ),
        "docket.jobs.diagnostics" => method_response(
            request.id,
            methods::docket::docket_job_diagnostics_result(&state, &headers, &request.params).await,
            "BearWire docket.jobs.diagnostics failed",
        ),
        "docket.jobs.cancel_run" => method_response(
            request.id,
            methods::docket::docket_jobs_cancel_run_result(&state, &headers, &request.params).await,
            "BearWire docket.jobs.cancel_run failed",
        ),
        "docket.jobs.execute" => method_response(
            request.id,
            methods::docket::docket_jobs_execute_result(&state, &headers, &request.params).await,
            "BearWire docket.jobs.execute failed",
        ),
        "docket.jobs.reconcile" => method_response(
            request.id,
            methods::docket::docket_jobs_reconcile_result(&state, &headers, &request.params).await,
            "BearWire docket.jobs.reconcile failed",
        ),
        "docket.jobs.settle_task" => method_response(
            request.id,
            methods::docket::docket_jobs_settle_task_result(&state, &headers, &request.params)
                .await,
            "BearWire docket.jobs.settle_task failed",
        ),
        "docket.session_tasks.settle" => method_response(
            request.id,
            methods::docket::docket_session_tasks_settle_result(&state, &headers, &request.params)
                .await,
            "BearWire docket.session_tasks.settle failed",
        ),
        "run.state" | "run.timeline" => method_response(
            request.id,
            methods::run::run_state_result(&state, &headers, &request.params).await,
            format!("BearWire {} failed", request.method),
        ),
        "run.cancel" => method_response(
            request.id,
            methods::run::run_cancel_result(&state, &headers, &request.params).await,
            "BearWire run.cancel failed",
        ),
        "run.recover" => method_response(
            request.id,
            methods::run::run_recover_result(&state, &headers, &request.params).await,
            "BearWire run.recover failed",
        ),
        "resource.update" => method_response(
            request.id,
            methods::resource::resource_update_result(&state, &headers, &request.params).await,
            "BearWire resource.update failed",
        ),
        "run.start" => method_response(
            request.id,
            methods::run::run_start_result(&state, &headers, &request.params).await,
            "BearWire run.start failed",
        ),
        "client.tool.result" => method_response(
            request.id,
            methods::client::client_tool_result_result(&state, &headers, &request.params).await,
            "BearWire client.tool.result failed",
        ),
        "client.tool.claim" => method_response(
            request.id,
            methods::client::client_tool_claim_result(&state, &headers, &request.params).await,
            "BearWire client.tool.claim failed",
        ),
        "client.tool.renew" => method_response(
            request.id,
            methods::client::client_tool_renew_result(&state, &headers, &request.params).await,
            "BearWire client.tool.renew failed",
        ),
        "client.permission.result" => method_response(
            request.id,
            methods::client::client_permission_result_result(&state, &headers, &request.params)
                .await,
            "BearWire client.permission.result failed",
        ),
        "work.checkout" => method_response(
            request.id,
            methods::work::work_checkout_result(&state, &headers, &request.params).await,
            "BearWire work.checkout failed",
        ),
        "work.boundary" => method_response(
            request.id,
            methods::work::work_boundary_result(&state, &headers, &request.params).await,
            "BearWire work.boundary failed",
        ),
        "work.checkpoint_evidence" => method_response(
            request.id,
            methods::work::work_checkpoint_evidence_result(&state, &headers, &request.params).await,
            "BearWire work.checkpoint_evidence failed",
        ),
        "work.acknowledge_checkpoint" => method_response(
            request.id,
            methods::work::work_acknowledge_checkpoint_result(&state, &headers, &request.params)
                .await,
            "BearWire work.acknowledge_checkpoint failed",
        ),
        "work.report" => method_response(
            request.id,
            methods::work::work_report_result(&state, &headers, &request.params).await,
            "BearWire work.report failed",
        ),
        other => JsonRpcResponse::error(
            request.id,
            -32601,
            format!("Method not found: {other}"),
            Some(json!({ "method": other })),
        ),
    };

    Ok(Json(response))
}

fn method_response(
    id: Option<Value>,
    result: Result<Value, CustomError>,
    message: impl Into<String>,
) -> JsonRpcResponse {
    match result {
        Ok(result) => JsonRpcResponse::ok(id, result),
        Err(err) => JsonRpcResponse::error(
            id,
            -32001,
            message,
            Some(json!({ "error": err.to_string() })),
        ),
    }
}
