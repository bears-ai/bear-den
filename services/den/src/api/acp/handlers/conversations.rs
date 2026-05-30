use axum::{
    extract::{Path, Query, State},
    http::HeaderMap,
    response::{IntoResponse, Response},
    Json,
};
use uuid::Uuid;

use crate::{
    api::service::ApiState,
    core::{
        acp_runtime::{require_pair_runtime_binding, verify_acp_conversation_belongs_to_binding},
        archived_conversations,
        bears::db as bears_db,
        letta::load_agent_conversations as load_runtime_conversations,
    },
    errors::CustomError,
};

use crate::api::acp::{
    history::{map_acp_history_page, map_compaction_status_for_history},
    normalize_acp_conversation_id,
    responses::acp_error_response,
    AcpConversationHistoryQuery, AcpConversationHistoryResponse, AcpConversationRow,
    AcpConversationsQuery, AcpConversationsResponse,
};

use super::auth::authenticate_acp_code_token;

pub(in crate::api::acp) async fn conversations(
    State(state): State<ApiState>,
    Path(slug): Path<String>,
    Query(query): Query<AcpConversationsQuery>,
    headers: HeaderMap,
) -> Response {
    let request_id = Uuid::new_v4();
    match conversations_inner(state, slug, query, headers).await {
        Ok(response) => response,
        Err(err) => acp_error_response(err, request_id),
    }
}

pub(super) async fn conversations_inner(
    state: ApiState,
    slug: String,
    query: AcpConversationsQuery,
    headers: HeaderMap,
) -> Result<Response, CustomError> {
    let user_id = authenticate_acp_code_token(&state, &headers, &slug).await?;
    let bear = bears_db::bear_for_user_by_slug(&state.sqlx_pool, user_id, slug.trim())
        .await?
        .ok_or_else(|| {
            CustomError::NotFound("bear not found or you do not have access".to_string())
        })?;

    let default_only = || {
        Json(AcpConversationsResponse {
            conversations: vec![AcpConversationRow {
                id: "default".to_string(),
                title: "Main chat".to_string(),
                last_message_at: None,
                archived: false,
            }],
        })
        .into_response()
    };

    if !state.letta.is_enabled() {
        return Ok(default_only());
    }
    let runtime_binding =
        require_pair_runtime_binding(&state.sqlx_pool, state.letta.as_ref(), &bear).await?;
    let runtime_binding_id = runtime_binding.binding_id;

    let archived_ids = archived_conversations::list_for_bear(&state.sqlx_pool, bear.id).await?;
    let snap = load_runtime_conversations(state.letta.as_ref(), &runtime_binding_id).await;
    let source: Vec<_> = if query.include_archived {
        snap.all
            .into_iter()
            .map(|mut row| {
                if archived_ids.contains(&row.id) {
                    row.archived = true;
                }
                row
            })
            .collect()
    } else {
        snap.all
            .into_iter()
            .filter(|row| !row.archived && !archived_ids.contains(&row.id))
            .collect()
    };
    let conversations = source
        .into_iter()
        .map(|row| AcpConversationRow {
            id: row.id,
            title: row.title,
            last_message_at: row.last_message_at,
            archived: row.archived,
        })
        .collect();
    Ok(Json(AcpConversationsResponse { conversations }).into_response())
}

pub(in crate::api::acp) async fn conversation_history(
    State(state): State<ApiState>,
    Path((slug, conversation_id)): Path<(String, String)>,
    Query(query): Query<AcpConversationHistoryQuery>,
    headers: HeaderMap,
) -> Response {
    let request_id = Uuid::new_v4();
    match conversation_history_inner(state, slug, conversation_id, query, headers).await {
        Ok(response) => response,
        Err(err) => acp_error_response(err, request_id),
    }
}

pub(super) async fn conversation_history_inner(
    state: ApiState,
    slug: String,
    conversation_id: String,
    query: AcpConversationHistoryQuery,
    headers: HeaderMap,
) -> Result<Response, CustomError> {
    let user_id = authenticate_acp_code_token(&state, &headers, &slug).await?;
    let bear = bears_db::bear_for_user_by_slug(&state.sqlx_pool, user_id, slug.trim())
        .await?
        .ok_or_else(|| {
            CustomError::NotFound("bear not found or you do not have access".to_string())
        })?;
    if !state.letta.is_enabled() {
        return Ok(Json(AcpConversationHistoryResponse {
            messages: vec![],
            has_more: false,
            next_before: None,
            compaction: None,
        })
        .into_response());
    }
    let runtime_binding =
        require_pair_runtime_binding(&state.sqlx_pool, state.letta.as_ref(), &bear).await?;
    let agent_id = runtime_binding.binding_id.clone();
    let conv_id = normalize_acp_conversation_id(Some(&conversation_id))?;
    if conv_id.starts_with("new-") {
        return Err(CustomError::ValidationError(
            "history is only available for default or saved conv- conversations".to_string(),
        ));
    }
    if conv_id.starts_with("conv-") {
        verify_acp_conversation_belongs_to_binding(
            state.letta.as_ref(),
            &runtime_binding,
            &conv_id,
        )
        .await?;
    }
    let limit = query.limit.unwrap_or(50).clamp(1, 100);
    let before = query
        .before
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let binding_for_conv = if conv_id == "default" {
        Some(agent_id.as_str())
    } else {
        None
    };
    let body = state
        .letta
        .list_conversation_messages(&conv_id, binding_for_conv, limit, before, false)
        .await?;
    let (messages, has_more, next_before) = map_acp_history_page(&body, limit);
    let compaction = Some(map_compaction_status_for_history(&conv_id, &body));
    Ok(Json(AcpConversationHistoryResponse {
        messages,
        has_more,
        next_before,
        compaction,
    })
    .into_response())
}
