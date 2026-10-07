//! Read access and execution authority are different projections of canonical state.

use super::{
    admission, authorize_existing_conversation, client_sessions, conversation_viewer,
    require_exclusive_client_session_id, resolved_or_stored_conversation_id, BearId,
    ClientSessionId, CustomError, DenState, PgPool, UserId,
};
use bearwire_protocol::session::{SessionAccess, SessionAccessState};
use den_service::conversation::{
    persistence::ConversationRecord, viewer::require_ordinary_tool_source,
};

pub(super) struct SessionSource {
    pub conversation: Option<ConversationRecord>,
    pub access: SessionAccess,
}

pub(super) async fn readable_source(
    state: &DenState,
    session: &client_sessions::ClientSessionRow,
) -> Result<Option<ConversationRecord>, CustomError> {
    require_exclusive_client_session_id(
        &state.sqlx_pool,
        &ClientSessionId::new(session.client_session_id.clone())?,
        UserId::new(session.user_id),
        BearId::new(session.bear_id),
    )
    .await?;
    let viewer = conversation_viewer(state, session.bear_id, session.user_id).await?;
    let stored = authorize_existing_conversation(
        &viewer,
        &state.sqlx_pool,
        session.bear_id,
        &session.conversation_id,
    )
    .await?;
    if let Some(resolved) = session.resolved_conversation_id.as_deref() {
        return authorize_existing_conversation(
            &viewer,
            &state.sqlx_pool,
            session.bear_id,
            resolved,
        )
        .await?
        .map(Some)
        .ok_or_else(|| CustomError::NotFound("canonical session conversation disappeared".into()));
    }
    Ok(stored)
}

pub(super) async fn require_work_source(
    pool: &PgPool,
    bear: BearId,
    user: UserId,
    session_id: &str,
    conversation_id: &str,
    work_run_id: uuid::Uuid,
) -> Result<(), CustomError> {
    den_runtime::agent_loop::require_ordinary_session_source(
        pool,
        den_runtime::agent_loop::OrdinarySessionSource {
            bear_id: bear.as_uuid(),
            user_id: Some(user.get()),
            origin: den_core::TurnExecutionOrigin::AuthorizedWorkRun(
                den_core::ArmatureAvailability::Connected,
            ),
            profile: den_core::RuntimeContextLabel::JobRun,
            conversation_id,
            client_session_id: session_id,
            work_run_id: Some(work_run_id),
        },
    )
    .await?;
    Ok(())
}

pub(super) async fn require_live_source(
    state: &DenState,
    session: &client_sessions::ClientSessionRow,
) -> Result<ConversationRecord, CustomError> {
    let conversation = readable_source(state, session).await?.ok_or_else(|| {
        CustomError::Authorization("select a hat before configuring this session".into())
    })?;
    if session.closed_at.is_some() || session.archived_at.is_some() {
        return Err(CustomError::Authorization("session is not live".into()));
    }
    let pool = &state.sqlx_pool;
    let external_id = resolved_or_stored_conversation_id(session);
    if den_service::archived_conversations::list_for_bear(pool, session.bear_id)
        .await?
        .contains(external_id)
    {
        return Err(CustomError::Authorization(
            "archived history is read-only".into(),
        ));
    }
    if let Some(work) =
        den_docket::work_runs::get_live_work_run_by_session(pool, &session.client_session_id)
            .await?
    {
        require_work_source(
            pool,
            BearId::new(session.bear_id),
            UserId::new(session.user_id),
            &session.client_session_id,
            external_id,
            work.id,
        )
        .await?;
        let viewer = conversation_viewer(state, session.bear_id, session.user_id).await?;
        if !viewer.may_read_own_source(pool, conversation.id).await? {
            return Err(CustomError::Authorization(
                "Work configuration requires its live owned transcript".into(),
            ));
        }
    } else {
        let viewer = conversation_viewer(state, session.bear_id, session.user_id).await?;
        if !viewer.may_read_own_source(pool, conversation.id).await? {
            return Err(CustomError::Authorization(
                "session configuration requires its live owned conversation".into(),
            ));
        }
        require_ordinary_tool_source(
            pool,
            BearId::new(session.bear_id),
            UserId::new(session.user_id),
            external_id,
        )
        .await?;
    }
    Ok(conversation)
}

pub(super) async fn project_source(
    state: &DenState,
    session: &client_sessions::ClientSessionRow,
) -> Result<SessionSource, CustomError> {
    let conversation = readable_source(state, session).await?;
    let work = den_docket::work_runs::get_live_work_run_by_session(
        &state.sqlx_pool,
        &session.client_session_id,
    )
    .await?;
    let live = session.closed_at.is_none() && session.archived_at.is_none();
    let pending = conversation.is_none()
        && session.resolved_conversation_id.is_none()
        && admission::PendingConversationId::parse(&session.conversation_id).is_some()
        && work.is_none();
    let access = if pending && live {
        SessionAccess {
            state: SessionAccessState::AwaitingHat,
            may_select_hat: admission::may_admit_pending_hat(&state.sqlx_pool, session).await?,
        }
    } else {
        let executable = match require_live_source(state, session).await {
            Ok(_) => true,
            Err(CustomError::Authorization(_) | CustomError::NotFound(_)) => false,
            Err(error) => return Err(error),
        };
        let may_select_hat = if executable && work.is_none() {
            let canonical = conversation
                .as_ref()
                .ok_or_else(|| CustomError::System("executable source is missing".into()))?;
            admission::may_select_initial_hat(&state.sqlx_pool, session, canonical.id).await?
        } else {
            false
        };
        SessionAccess {
            state: if executable {
                SessionAccessState::Executable
            } else {
                SessionAccessState::ReadOnly
            },
            may_select_hat,
        }
    };
    Ok(SessionSource {
        conversation,
        access,
    })
}
