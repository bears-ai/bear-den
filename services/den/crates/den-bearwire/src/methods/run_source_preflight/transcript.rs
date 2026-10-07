//! Transcript ownership is independent of the turn's conversation or Work hat.

use den_core::ids::{BearId, UserId};
use den_http::errors::CustomError;
use den_service::{
    archived_conversations,
    client_sessions::ClientSessionRow,
    conversation::{
        persistence::{self, ConversationRecord},
        viewer::{require_ordinary_tool_source, ConversationViewer},
    },
};
use sqlx::PgPool;

pub(in crate::methods::run) async fn require_live_owned_transcript(
    pool: &PgPool,
    viewer: &ConversationViewer,
    bear: BearId,
    external_id: &str,
) -> Result<Option<ConversationRecord>, CustomError> {
    // This marker remains authoritative even if the canonical row says active,
    // or disappeared. Never recreate archived history under a fresh default hat.
    if archived_conversations::list_for_bear(pool, bear.as_uuid())
        .await?
        .contains(external_id)
    {
        return Err(CustomError::Authorization(
            "archived history is read-only".into(),
        ));
    }
    let conversation =
        persistence::get_conversation_for_external_id(pool, bear.as_uuid(), external_id).await?;
    if let Some(conversation) = &conversation {
        if !viewer.may_read_own_source(pool, conversation.id).await? {
            return Err(CustomError::Authorization(
                "run startup requires a live owned transcript".into(),
            ));
        }
    }
    Ok(conversation)
}

pub(in crate::methods::run) async fn require_existing_session(
    pool: &PgPool,
    viewer: &ConversationViewer,
    bear: BearId,
    user: UserId,
    session: &ClientSessionRow,
    requested: Option<&str>,
) -> Result<(), CustomError> {
    if session.bear_id != bear.as_uuid()
        || session.user_id != user.get()
        || session.closed_at.is_some()
        || session.archived_at.is_some()
    {
        return Err(CustomError::Authorization("session is not live".into()));
    }
    if requested.is_some_and(|requested| {
        requested != session.conversation_id
            && Some(requested) != session.resolved_conversation_id.as_deref()
    }) {
        return Err(CustomError::Authorization(
            "run startup cannot change the canonical session conversation".into(),
        ));
    }
    let work =
        den_docket::work_runs::get_live_work_run_by_session(pool, &session.client_session_id)
            .await?;
    let stored =
        require_live_owned_transcript(pool, viewer, bear, &session.conversation_id).await?;
    if stored.is_none() && !session.conversation_id.starts_with("new-") {
        return Err(CustomError::Authorization(
            "canonical session conversation disappeared".into(),
        ));
    }
    if stored.is_some() && work.is_none() {
        require_ordinary_tool_source(pool, bear, user, &session.conversation_id).await?;
    }
    if let Some(resolved) = session.resolved_conversation_id.as_deref() {
        require_live_owned_transcript(pool, viewer, bear, resolved)
            .await?
            .ok_or_else(|| {
                CustomError::Authorization("canonical session conversation disappeared".into())
            })?;
        if work.is_none() {
            require_ordinary_tool_source(pool, bear, user, resolved).await?;
        }
    }
    Ok(())
}
