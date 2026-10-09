//! Conversation inventory and explicit history selection are separate projections.

use super::{
    canonical_chat_id, checked_chat_id, conversation_viewer, normalize_client_conversation_id,
    require_chat_id, ChatApiError, ChatConversationRow, ChatConversationsQuery,
    ChatConversationsResponse, ChatHatChoice,
};
use crate::{auth_backend::AuthSession, errors::CustomError, web::AppState};
use axum::{extract::State, Json};
use axum_extra::extract::{Query, QueryRejection};
use den_core::{
    ids::{BearId, UserId},
    DenError,
};
use den_service::{
    archived_conversations,
    bears::{db as bears_db, hats},
    conversation::{persistence as conversation_persistence, viewer::require_ordinary_tool_source},
};
use sqlx::PgPool;
use std::collections::{HashMap, HashSet};
use uuid::Uuid;

fn listed_external_id(id: &str, default_id: &str, archived_ids: &HashSet<String>) -> bool {
    !id.starts_with("new-") && !archived_ids.contains(id) && (id != "default" || id == default_id)
}

fn selected_access_error(error: CustomError) -> ChatApiError {
    match error {
        CustomError::Authorization(_) => ChatApiError::ConversationUnavailable,
        error => error.into(),
    }
}

async fn selected_conversation(
    pool: &PgPool,
    bear_id: Uuid,
    user_id: i32,
    requested: &str,
    default_id: &str,
    archived_ids: &HashSet<String>,
) -> Result<conversation_persistence::ConversationRecord, ChatApiError> {
    let requested = normalize_client_conversation_id(Some(requested))?;
    let (viewer, external) = checked_chat_id(pool, bear_id, user_id, &requested)
        .await
        .map_err(selected_access_error)?;
    let selected =
        conversation_persistence::get_conversation_for_external_id(pool, bear_id, &external)
            .await?
            .ok_or(ChatApiError::ConversationUnavailable)?;
    require_chat_id(pool, &viewer, &external)
        .await
        .map_err(selected_access_error)?;
    if !listed_external_id(&external, default_id, archived_ids) {
        return Err(ChatApiError::ConversationUnavailable);
    }
    Ok(selected)
}

pub(super) async fn ordinary_source_can_send(
    pool: &PgPool,
    bear_id: BearId,
    user_id: UserId,
    external: &str,
) -> Result<bool, DenError> {
    match require_ordinary_tool_source(pool, bear_id, user_id, external).await {
        Ok(()) => Ok(true),
        Err(DenError::Authorization(_) | DenError::NotFound(_)) => Ok(false),
        Err(error) => Err(error),
    }
}

pub(super) async fn chat_conversations(
    State(state): State<AppState>,
    auth_session: AuthSession,
    query: Result<Query<ChatConversationsQuery>, QueryRejection>,
) -> Result<Json<ChatConversationsResponse>, ChatApiError> {
    let Query(q) = query?;
    let user_id = auth_session
        .user
        .as_ref()
        .map(|user| user.id)
        .ok_or_else(|| CustomError::Authentication("login required".into()))?;
    let pool = state.sqlx_pool();
    let viewer = conversation_viewer(pool, q.bear_id, user_id).await?;
    let default_id = canonical_chat_id(pool, q.bear_id, user_id, "default").await?;
    let bear = bears_db::get_bear(pool, q.bear_id)
        .await?
        .ok_or_else(|| CustomError::NotFound("bear not found".into()))?;
    let archived_ids = archived_conversations::list_for_bear(pool, bear.id).await?;
    let mut visible = viewer.list_visible(pool, 100).await?;
    let selected = if let Some(requested) = q.conversation_id.as_deref() {
        let selected = selected_conversation(
            pool,
            bear.id,
            user_id,
            requested,
            &default_id,
            &archived_ids,
        )
        .await?;
        let id = selected.id;
        // A bounded recent inventory must not strand an authorized exact history link.
        if !visible.iter().any(|row| row.id == id) {
            visible.push(selected);
        }
        Some(id)
    } else {
        None
    };
    let ids: Vec<Uuid> = visible.iter().map(|row| row.id).collect();
    let hat_bindings: HashMap<Uuid, (Option<Uuid>, Option<i32>)> = sqlx::query!(
        "SELECT id, hat_id, created_by_user_id FROM conversations WHERE bear_id = $1 AND id = ANY($2)",
        bear.id,
        &ids,
    )
    .fetch_all(pool)
    .await?
    .into_iter()
    .map(|row| (row.id, (row.hat_id, row.created_by_user_id)))
    .collect();
    let mut conversations = Vec::with_capacity(visible.len());
    let mut selected_conversation_id = None;
    for row in visible {
        let Some(external) = row.external_conversation_id else {
            continue;
        };
        if !listed_external_id(&external, &default_id, &archived_ids) {
            continue;
        }
        let hat_id = hat_bindings.get(&row.id).and_then(|binding| binding.0);
        let owned_bound = hat_bindings
            .get(&row.id)
            .is_some_and(|(hat, owner)| hat.is_some() && *owner == Some(user_id));
        let own_notes_available = owned_bound && viewer.may_read_own_source(pool, row.id).await?;
        // Notes visibility is not execution authority. Only known source denials
        // become false; database and other infrastructure failures stay failures.
        let can_send = own_notes_available
            && !archived_ids.contains(&external)
            && ordinary_source_can_send(
                pool,
                BearId::new(bear.id),
                UserId::new(user_id),
                &external,
            )
            .await?;
        let display_id = if external == default_id {
            "default"
        } else {
            &external
        };
        if selected == Some(row.id) {
            selected_conversation_id = Some(display_id.to_string());
        }
        conversations.push(ChatConversationRow {
            id: display_id.to_string(),
            hat_id,
            own_notes_available,
            can_send,
            title: row
                .current_title
                .filter(|title| !title.trim().is_empty())
                .unwrap_or_else(|| {
                    if external == default_id {
                        "Main chat".into()
                    } else {
                        external.clone()
                    }
                }),
            last_message_at: row
                .updated_at
                .format(&time::format_description::well_known::Rfc3339)
                .ok(),
            latest_context_budget: row.latest_context_budget,
            latest_context_budget_updated_at: row.latest_context_budget_updated_at.and_then(
                |value| {
                    value
                        .format(&time::format_description::well_known::Rfc3339)
                        .ok()
                },
            ),
        });
    }
    let hats = hats::list_hats(pool, BearId::new(bear.id))
        .await?
        .into_iter()
        .map(|hat| ChatHatChoice {
            id: hat.id.as_uuid(),
            name: hat.name,
            purpose: hat.purpose,
        })
        .collect();
    Ok(Json(ChatConversationsResponse {
        conversations,
        hats,
        selected_conversation_id,
    }))
}
