//! Session startup persists client identity, not an unadmitted conversation.

use super::{
    access, admission, authenticated_bear, authorize_existing_conversation, bearwire_events,
    client_sessions, conversation_viewer, hats, json, parse_params,
    require_exclusive_client_session_id, session_state_payload, BearId, BearWireEvent,
    ClientSessionId, CustomError, DenState, HeaderMap, SessionOpenRequest, UserId, Value,
    DEFAULT_CLIENT,
};
use crate::methods::run::source_preflight::publication;

pub(crate) async fn session_open_result(
    state: &DenState,
    headers: &HeaderMap,
    params: &Value,
) -> Result<Value, CustomError> {
    let (user_id, bear) = authenticated_bear(state, headers, params).await?;
    let request: SessionOpenRequest = parse_params(params)?;
    let session_id = request.session_id;
    require_exclusive_client_session_id(
        &state.sqlx_pool,
        &ClientSessionId::new(session_id.clone())?,
        UserId::new(user_id),
        BearId::new(bear.id),
    )
    .await?;
    if let Some(expected) = request.expected_work_source {
        crate::methods::run::source_preflight::require_expected_work_source(
            &state.sqlx_pool,
            BearId::new(bear.id),
            UserId::new(user_id),
            &session_id,
            expected,
        )
        .await?;
    }
    let existing = client_sessions::find_for_user_bear_session(
        &state.sqlx_pool,
        user_id,
        &bear.slug,
        &session_id,
    )
    .await?;
    let first_open = existing.is_none();
    let client = request.client.unwrap_or_else(|| DEFAULT_CLIENT.to_string());
    let viewer = conversation_viewer(state, bear.id, user_id).await?;
    let (conversation_id, mut resolved_conversation_id, pending_selection) = if let Some(session) =
        existing.as_ref()
    {
        // Reconnect cannot substitute either a history selection or its resolved source.
        let pending_source = access::readable_source(state, session).await?.is_none()
            && admission::PendingConversationId::parse(&session.conversation_id).is_some();
        if request.conversation_id.as_deref().is_some_and(|requested| {
            requested != session.conversation_id
                && Some(requested) != session.resolved_conversation_id.as_deref()
        }) {
            return Err(CustomError::Authorization(
                "reconnect cannot change the canonical session conversation".into(),
            ));
        }
        (
            session.conversation_id.clone(),
            session.resolved_conversation_id.clone(),
            pending_source,
        )
    } else if let Some(requested) = request.conversation_id.as_deref() {
        match authorize_existing_conversation(&viewer, &state.sqlx_pool, bear.id, requested).await?
        {
            Some(_) => (requested.to_string(), None, false),
            None => {
                let pending = admission::PendingConversationId::parse(requested)
                    .ok_or_else(|| CustomError::NotFound("conversation not found".into()))?;
                (pending.as_str().to_string(), None, true)
            }
        }
    } else {
        (
            format!("new-acp-{client}-{}", uuid::Uuid::new_v4().simple()),
            None,
            true,
        )
    };
    let current_mode = request
        .mode
        .as_deref()
        .map(client_sessions::ClientSessionMode::try_from_storage)
        .transpose()?;
    if let Some(work) =
        den_docket::work_runs::get_live_work_run_by_session(&state.sqlx_pool, &session_id).await?
    {
        access::require_work_source(
            &state.sqlx_pool,
            BearId::new(bear.id),
            UserId::new(user_id),
            &session_id,
            resolved_conversation_id
                .as_deref()
                .unwrap_or(&conversation_id),
            work.id,
        )
        .await?;
        // Work has already admitted the exact live Job run. Its transcript is
        // not an ordinary conversation hat binding, and reconnect keeps its ID.
        if pending_selection {
            let winner = publication::materialize_run_source(
                &state.sqlx_pool,
                publication::NewRunSource {
                    bear: BearId::new(bear.id),
                    user: UserId::new(user_id),
                    session_id: &session_id,
                    selection: &conversation_id,
                    authority: publication::NewSourceAuthority::WorkRun(work.id),
                    initial_mode: current_mode,
                },
            )
            .await?;
            resolved_conversation_id = Some(winner);
        } else {
            let target = resolved_conversation_id
                .as_deref()
                .unwrap_or(&conversation_id);
            authorize_existing_conversation(&viewer, &state.sqlx_pool, bear.id, target)
                .await?
                .ok_or_else(|| {
                    CustomError::NotFound("canonical session conversation disappeared".into())
                })?;
        }
    }
    let runtime_session_id = request
        .runtime_session_id
        .or_else(|| existing.as_ref().map(|s| s.runtime_session_id.clone()))
        .unwrap_or_else(|| format!("bearwire:{}:{}", bear.id, session_id));
    publication::publish_open_metadata(
        &state.sqlx_pool,
        client_sessions::UpsertClientSession {
            user_id,
            bear_id: bear.id,
            bear_slug: bear.slug.clone(),
            client_session_id: session_id.clone(),
            runtime_session_id,
            conversation_id,
            resolved_conversation_id,
            client,
            cwd: request.cwd,
            current_mode,
        },
    )
    .await?;
    require_exclusive_client_session_id(
        &state.sqlx_pool,
        &ClientSessionId::new(session_id.clone())?,
        UserId::new(user_id),
        BearId::new(bear.id),
    )
    .await?;
    let session = client_sessions::find_for_user_bear_session(
        &state.sqlx_pool,
        user_id,
        &bear.slug,
        &session_id,
    )
    .await?
    .ok_or_else(|| CustomError::NotFound("IDE session not found".into()))?;
    let source = access::project_source(state, &session).await?;
    if first_open
        && source.access.state == bearwire_protocol::session::SessionAccessState::AwaitingHat
    {
        if let Some(default_hat) =
            hats::ide_default_hat(&state.sqlx_pool, BearId::new(bear.id)).await?
        {
            admission::materialize_pending(&state.sqlx_pool, &session, default_hat).await?;
        }
    }
    let reconnected =
        den_docket::work_runs::reconnect_attached_work_run(&state.sqlx_pool, &session_id)
            .await?
            .is_some();
    if let Some(client_context) = request.client_context.as_ref() {
        client_sessions::update_adapter_environment(
            &state.sqlx_pool,
            user_id,
            bear.id,
            &session_id,
            client_context,
        )
        .await?;
    }
    let session = client_sessions::find_for_user_bear_session(
        &state.sqlx_pool,
        user_id,
        &bear.slug,
        &session_id,
    )
    .await?
    .ok_or_else(|| CustomError::NotFound("IDE session not found".into()))?;
    let session = session_state_payload(state, session, bear.work_enabled).await?;
    let mut event = BearWireEvent::ephemeral(
        "session.opened",
        json!({
            "session_id": session_id, "bear_slug": bear.slug,
        }),
    );
    event.bear_id = Some(bear.id.to_string());
    event.human_id = Some(user_id.to_string());
    event.session_id = Some(session_id.clone());
    let persisted = bearwire_events::append_bearwire_event(
        &state.sqlx_pool,
        &session_id,
        Some(bear.id),
        Some(user_id),
        event,
    )
    .await?;
    Ok(json!({
        "ok": true, "session": session, "event_sequence": persisted.sequence_no,
        "attached_work_reconnected": reconnected,
    }))
}
