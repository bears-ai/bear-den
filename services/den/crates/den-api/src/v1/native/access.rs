use axum::{http::HeaderMap, http::StatusCode};
use den_core::ids::{BearId, UserId};
use den_oauth::{
    auth::{authenticate_bearer_active, require_scope, BearerPrincipal},
    oauth::OAuthScope,
};
use den_service::{
    bears::db,
    conversation::{
        persistence::{self, ConversationRecord},
        viewer::{require_ordinary_tool_source, ConversationViewer},
    },
    DenState,
};
use uuid::Uuid;

use super::error::Error;

pub(super) async fn authenticate(
    state: &DenState,
    headers: &HeaderMap,
    scope: OAuthScope,
) -> Result<BearerPrincipal, Error> {
    let principal = authenticate_bearer_active(headers, &state.sqlx_pool).await?;
    require_scope(&principal, scope)?;
    Ok(principal)
}

pub(super) async fn viewer(
    state: &DenState,
    bear: Uuid,
    user: i32,
) -> Result<ConversationViewer, Error> {
    ConversationViewer::resolve(&state.sqlx_pool, BearId::new(bear), UserId::new(user))
        .await?
        .ok_or_else(Error::unavailable)
}

pub(super) async fn readable(
    state: &DenState,
    user: i32,
    id: Uuid,
) -> Result<ConversationRecord, Error> {
    let conversation = persistence::get_conversation_by_id(&state.sqlx_pool, id)
        .await?
        .ok_or_else(Error::unavailable)?;
    let viewer = viewer(state, conversation.bear_id, user).await?;
    if !viewer.may_access_id(&state.sqlx_pool, id).await? {
        return Err(Error::unavailable());
    }
    Ok(conversation)
}

pub(super) async fn selected(
    state: &DenState,
    user: i32,
    bear: Uuid,
    external: &str,
) -> Result<ConversationRecord, Error> {
    validate_external_id(external)?;
    let viewer = viewer(state, bear, user).await?;
    let conversation =
        persistence::get_conversation_for_external_id(&state.sqlx_pool, bear, external)
            .await?
            .ok_or_else(Error::unavailable)?;
    if !viewer
        .may_access_id(&state.sqlx_pool, conversation.id)
        .await?
    {
        return Err(Error::unavailable());
    }
    Ok(conversation)
}

pub(super) async fn executable(
    state: &DenState,
    user: i32,
    conversation: &ConversationRecord,
) -> Result<(), Error> {
    let external = conversation
        .external_conversation_id
        .as_deref()
        .ok_or_else(Error::unavailable)?;
    require_ordinary_tool_source(
        &state.sqlx_pool,
        BearId::new(conversation.bear_id),
        UserId::new(user),
        external,
    )
    .await?;
    if den_service::archived_conversations::list_for_bear(&state.sqlx_pool, conversation.bear_id)
        .await?
        .contains(external)
    {
        return Err(Error::new(
            StatusCode::FORBIDDEN,
            "read_only",
            "Archived conversations are read-only.",
        ));
    }
    // Work transcripts remain inspection-only at this ordinary channel edge.
    if let Some(session) = den_service::client_sessions::find_latest_for_bear_conversation(
        &state.sqlx_pool,
        conversation.bear_id,
        external,
    )
    .await?
    {
        if den_docket::work_runs::get_work_run_by_session(
            &state.sqlx_pool,
            &session.client_session_id,
        )
        .await?
        .is_some()
        {
            return Err(Error::new(
                StatusCode::FORBIDDEN,
                "read_only",
                "Work transcripts are read-only in chat.",
            ));
        }
    }
    Ok(())
}

pub(super) async fn can_send(
    state: &DenState,
    user: i32,
    conversation: &ConversationRecord,
) -> Result<bool, Error> {
    match executable(state, user, conversation).await {
        Ok(()) => Ok(true),
        Err(error) if matches!(error.status, StatusCode::FORBIDDEN | StatusCode::NOT_FOUND) => {
            Ok(false)
        }
        Err(error) => Err(error),
    }
}

pub(super) async fn membership(
    state: &DenState,
    user: i32,
    bear: Uuid,
) -> Result<den_service::bears::Bear, Error> {
    if !db::user_may_use_bear(&state.sqlx_pool, user, bear).await? {
        return Err(Error::unavailable());
    }
    db::get_bear(&state.sqlx_pool, bear)
        .await?
        .ok_or_else(Error::unavailable)
}

pub(super) fn validate_external_id(id: &str) -> Result<(), Error> {
    if id.is_empty() || id.len() > 256 || id.chars().any(char::is_control) {
        return Err(Error::invalid("Invalid conversation identifier."));
    }
    Ok(())
}
