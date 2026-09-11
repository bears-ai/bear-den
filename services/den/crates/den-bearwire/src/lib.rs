use axum::{
    routing::{get, post},
    Router,
};
use den_service::DenState;

mod auth;
mod events;
mod methods;
mod obligation_expiry;
mod open_reflection;
mod rpc;

pub use methods::focused_execution::acquire_selected_task_for_run;
pub use obligation_expiry::{expire_client_obligations_once, run_client_obligation_expiry_loop};
pub use open_reflection::run_open_session_reflection_loop;

pub fn router() -> Router<DenState> {
    Router::new().route("/v1/rpc", post(rpc::rpc)).route(
        "/v1/sessions/{session_id}/events/page",
        get(events::events_page),
    )
}
