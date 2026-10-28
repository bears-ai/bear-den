//! AHP 1.0 text-only WebSocket projection over canonical Den conversations.
mod connection;
mod projection;
mod turn;
mod wire;

use super::{access, error::Error};
use axum::{
    extract::{ws::WebSocketUpgrade, State},
    http::HeaderMap,
    response::Response,
};
use den_oauth::oauth::OAuthScope;
use den_service::DenState;

pub(super) async fn upgrade(
    State(state): State<DenState>,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> Result<Response, Error> {
    let principal = access::authenticate(&state, &headers, OAuthScope::DataRead).await?;
    Ok(ws
        .max_message_size(wire::MAX_FRAME)
        .max_frame_size(wire::MAX_FRAME)
        .on_upgrade(move |socket| connection::serve(socket, state, headers, principal.user_id)))
}

#[cfg(test)]
pub(super) async fn chat_snapshot_for_test(
    state: &DenState,
    user: i32,
    conversation: uuid::Uuid,
) -> Result<serde_json::Value, Error> {
    Ok(
        projection::snapshot(state, user, wire::Channel::Chat(conversation), 1, None)
            .await?
            .state,
    )
}

#[cfg(test)]
mod tests;
