//! Model inspection never creates or claims a canonical source.

use super::{
    access, authenticated_bear, bears_db, bearwire_events, client_sessions, json, parse_params,
    BearWireEvent, CustomError, DenState, HeaderMap, SessionIdRequest, SessionModelSetRequest,
    Value,
};
use den_service::{conversation::persistence, model_selection};

async fn session_model_payload(
    state: &DenState,
    user_id: i32,
    bear: &den_service::bears::Bear,
    session_id: &str,
) -> Result<Value, CustomError> {
    let session = client_sessions::find_for_user_bear_session(
        &state.sqlx_pool,
        user_id,
        &bear.slug,
        session_id,
    )
    .await?
    .ok_or_else(|| CustomError::NotFound("BearWire session not found".into()))?;
    let source = access::project_source(state, &session).await?;
    let model_state = match source.conversation.as_ref() {
        Some(conversation) => {
            persistence::get_conversation_model_state(&state.sqlx_pool, conversation.id).await?
        }
        None => None,
    };
    let selected = match source.conversation.as_ref() {
        Some(conversation) => {
            persistence::resolve_conversation_selected_model(&state.sqlx_pool, conversation.id)
                .await?
        }
        None => None,
    };
    let base_model =
        bears_db::resolve_model_for_bear(bear, state.config.default_llm_model.as_str());
    Ok(json!({
        "ok": true,
        "session_id": session_id,
        "conversation_id": source.conversation.as_ref().and_then(|c| c.external_conversation_id.as_deref()),
        "access": source.access,
        "selection_mode": model_state.as_ref().map(|m| m.selection_mode.as_str()).unwrap_or("auto"),
        "requested_model": model_state.as_ref().and_then(|m| m.requested_model.as_deref()),
        "selected_model": model_state.as_ref().and_then(|m| m.selected_model.as_deref()),
        "effective_model": selected.unwrap_or(base_model),
        "model_options": model_selection::list_selectable_model_options_for_acp(&state.sqlx_pool).await?,
    }))
}

pub(crate) async fn session_model_get_result(
    state: &DenState,
    headers: &HeaderMap,
    params: &Value,
) -> Result<Value, CustomError> {
    let (user_id, bear) = authenticated_bear(state, headers, params).await?;
    let request: SessionIdRequest = parse_params(params)?;
    session_model_payload(state, user_id, &bear, &request.session_id).await
}

pub(crate) async fn session_model_set_result(
    state: &DenState,
    headers: &HeaderMap,
    params: &Value,
) -> Result<Value, CustomError> {
    let (user_id, bear) = authenticated_bear(state, headers, params).await?;
    let request: SessionModelSetRequest = parse_params(params)?;
    let session_id = request.session_id;
    let session = client_sessions::find_for_user_bear_session(
        &state.sqlx_pool,
        user_id,
        &bear.slug,
        &session_id,
    )
    .await?
    .ok_or_else(|| CustomError::NotFound("BearWire session not found".into()))?;
    let conversation = access::require_live_source(state, &session).await?;
    let mode = request.selection_mode.unwrap_or_else(|| "auto".into());
    let model_state = model_selection::apply_conversation_model_selection(
        &state.sqlx_pool,
        conversation.id,
        &mode,
        request.model.as_deref(),
        "acp_selected",
        "inherit_stance_or_bear_default",
    )
    .await?;
    let mut event = BearWireEvent::ephemeral(
        "model.selection.changed",
        json!({
            "session_id": session_id,
            "conversation_id": conversation.external_conversation_id,
            "selection_mode": model_state.selection_mode,
            "selected_model": model_state.selected_model.or(model_state.requested_model),
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
    let mut payload = session_model_payload(state, user_id, &bear, &session_id).await?;
    if let Some(object) = payload.as_object_mut() {
        object.insert("event_sequence".into(), json!(persisted.sequence_no));
    }
    Ok(payload)
}
